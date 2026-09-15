use super::*;
use codex_config::types::McpServerConfig;
use codex_extension_api::ExtensionFuture;
use codex_extension_api::ExtensionRegistryBuilder;
use codex_extension_api::ToolFinishInput;
use codex_extension_api::ToolLifecycleContributor;
use codex_extension_api::ToolLifecycleFuture;
use codex_extension_api::TurnLifecycleContributor;
use codex_extension_api::TurnStopInput;
use pretty_assertions::assert_eq;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;
use test_case::test_case;
use tokio::sync::Notify;
use wiremock::Mock;
use wiremock::Request;
use wiremock::ResponseTemplate;
use wiremock::matchers::method;
use wiremock::matchers::path;

#[derive(Default)]
struct AcceptedMcpResult {
    reached: Notify,
    idle: Notify,
    hold_stop: AtomicBool,
    stopping: Notify,
    release_stop: Notify,
    calls: AtomicUsize,
    release: Notify,
}
impl ToolLifecycleContributor for AcceptedMcpResult {
    fn on_tool_finish<'a>(&'a self, input: ToolFinishInput<'a>) -> ToolLifecycleFuture<'a> {
        Box::pin(async move {
            if input.tool_name.name.ends_with("held") {
                self.calls.fetch_add(1, Ordering::SeqCst);
                self.reached.notify_one();
                self.release.notified().await;
            }
        })
    }
}

// Keep the first model turn from finishing before the MCP call was dispatched.
impl TurnLifecycleContributor for AcceptedMcpResult {
    fn on_turn_stop<'a>(&'a self, _input: TurnStopInput<'a>) -> ExtensionFuture<'a, ()> {
        Box::pin(async move {
            while self.calls.load(Ordering::SeqCst) == 0 {
                self.reached.notified().await;
            }
            if self.hold_stop.swap(false, Ordering::SeqCst) {
                self.stopping.notify_one();
                self.release_stop.notified().await;
            }
        })
    }
}

impl codex_extension_api::ThreadLifecycleContributor<codex_core::config::Config>
    for AcceptedMcpResult
{
    fn on_thread_idle<'a>(
        &'a self,
        _input: codex_extension_api::ThreadIdleInput<'a>,
    ) -> ExtensionFuture<'a, ()> {
        Box::pin(async {
            self.idle.notify_one();
        })
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum CompletionBoundary {
    Idle,
    Finalizing,
    Interrupt,
}

#[test_case(1, CompletionBoundary::Idle; "already_dispatched")]
#[test_case(2, CompletionBoundary::Idle; "sequential_idle_dispatch")]
#[test_case(2, CompletionBoundary::Finalizing; "sequential_busy_boundary")]
#[test_case(2, CompletionBoundary::Interrupt; "idle_interrupt")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mcp_completion_wakes_after_the_turn_ends(
    calls: usize,
    boundary: CompletionBoundary,
) -> anyhow::Result<()> {
    let server = responses::start_mock_server().await;
    Mock::given(method("POST")).and(path("/mcp"))
        .respond_with(|request: &Request| {
            let body: serde_json::Value = serde_json::from_slice(&request.body).expect("MCP JSON request");
            let result = match body["method"].as_str().expect("MCP method") {
                "initialize" => serde_json::json!({"protocolVersion":"2024-11-05", "capabilities":{"tools":{}}, "serverInfo":{"name":"held", "version":"1"}}),
                "tools/list" => serde_json::json!({"tools":[{"name":"held","description":"Return a held result", "inputSchema":{"type":"object"}, "annotations":{"readOnlyHint":true}}]}),
                "tools/call" => serde_json::json!({"content":[{"type":"text","text":"MCP retained result"}], "isError":false}),
                _ => return ResponseTemplate::new(202),
            };
            ResponseTemplate::new(200).set_body_json(serde_json::json!({"jsonrpc":"2.0","id":body["id"],"result":result}))
        }).mount(&server).await;
    let gate = Arc::new(AcceptedMcpResult::default());
    let mut extensions = ExtensionRegistryBuilder::new();
    extensions.tool_lifecycle_contributor(gate.clone());
    extensions.turn_lifecycle_contributor(gate.clone());
    extensions.thread_lifecycle_contributor(gate.clone());
    let url = format!("{}/mcp", server.uri());
    let test = test_codex()
        .with_model("test-gpt-5.1-codex")
        .with_extensions(Arc::new(extensions.build()))
        .with_config(move |config| {
            config
                .features
                .enable(Feature::CodeMode)
                .expect("enable Code Mode");
            config
                .features
                .enable(Feature::CodeModeInterrupt)
                .expect("enable Cell interruption");
            let mcp: McpServerConfig =
                serde_json::from_value(serde_json::json!({"url":url, "required":true}))
                    .expect("MCP config");
            config
                .mcp_servers
                .set(std::collections::HashMap::from([(
                    "completion".to_string(),
                    mcp,
                )]))
                .expect("set MCP servers");
        })
        .build_with_auto_env(&server)
        .await?;
    test.codex.enable_code_mode_completion_wake();
    core_test_support::wait_for_mcp_server(&test.codex, "completion").await?;
    let code = format!(
        "// @exec: {{\"yield_time_ms\": 10}}\nconst tool = ALL_TOOLS.find(t => t.name.endsWith('held')); for (let i = 0; i < {calls}; i++) text(await tools[tool.name]({{}}));"
    );
    let output = launch(&server, &test, &code).await;
    tokio::time::timeout(Duration::from_secs(10), gate.idle.notified()).await?;
    assert_eq!(gate.calls.load(Ordering::SeqCst), 1);
    let request = output.single_request();
    let cell_id = running_cell_id(&request);
    let busy = if boundary == CompletionBoundary::Finalizing {
        gate.hold_stop.store(true, Ordering::SeqCst);
        let busy = responses::mount_sse_once(&server, responses::sse_completed("busy")).await;
        test.codex.start_or_steer_turn(user("Other work")).await?;
        tokio::time::timeout(Duration::from_secs(10), gate.stopping.notified()).await?;
        Some(busy)
    } else {
        None
    };
    let (wake, result) = wake_and_wait(&server, &cell_id).await;
    no_requests(&wake).await;
    if boundary == CompletionBoundary::Interrupt {
        test.codex.submit(Op::Interrupt).await?;
        // The submission barrier establishes that idle interrupt was processed.
        let (reply, processed) = tokio::sync::oneshot::channel();
        test.codex
            .submit(Op::TurnSettings {
                turn_id: "absent".to_string(),
                update: Default::default(),
                reply,
            })
            .await?;
        let _ = processed.await?;
        gate.release.notify_one();
        no_requests(&wake).await;
        assert_eq!(gate.calls.load(Ordering::SeqCst), 1);
        test.codex.shutdown_and_wait().await?;
        return Ok(());
    }
    for completed in 1..calls {
        gate.release.notify_one();
        tokio::time::timeout(Duration::from_secs(10), async {
            while gate.calls.load(Ordering::SeqCst) <= completed {
                gate.reached.notified().await;
            }
        })
        .await?;
        no_requests(&wake).await;
    }
    gate.release.notify_one();
    if let Some(busy) = busy {
        no_requests(&wake).await;
        assert!(
            !busy
                .single_request()
                .body_contains_text("<code_mode_completion>")
        );
        gate.release_stop.notify_one();
        wait_for_event(&test.codex, |event| {
            matches!(event, EventMsg::TurnComplete(_))
        })
        .await;
    }
    wait_for_event(&test.codex, |event| {
        matches!(event, EventMsg::TurnComplete(_))
    })
    .await;
    assert!(
        wake.single_request()
            .body_contains_text("<code_mode_completion>")
    );
    assert!(
        result
            .single_request()
            .body_contains_text("MCP retained result")
    );
    let observed_calls = server
        .received_requests()
        .await
        .expect("recorded requests")
        .into_iter()
        .filter(|request| {
            request.url.path() == "/mcp"
                && serde_json::from_slice::<serde_json::Value>(&request.body)
                    .expect("MCP JSON request")["method"]
                    == "tools/call"
        })
        .count();
    assert_eq!(observed_calls, calls);
    test.codex.shutdown_and_wait().await?;
    Ok(())
}
