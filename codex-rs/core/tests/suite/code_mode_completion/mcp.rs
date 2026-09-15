use super::*;
use codex_config::types::McpServerConfig;
use codex_extension_api::ExtensionRegistryBuilder;
use codex_extension_api::ToolFinishInput;
use codex_extension_api::ToolLifecycleContributor;
use codex_extension_api::ToolLifecycleFuture;
use pretty_assertions::assert_eq;
use tokio::sync::Notify;
use wiremock::Mock;
use wiremock::Request;
use wiremock::ResponseTemplate;
use wiremock::matchers::method;
use wiremock::matchers::path;

#[derive(Default)]
struct AcceptedMcpResult {
    reached: Notify,
    release: Notify,
}
impl ToolLifecycleContributor for AcceptedMcpResult {
    fn on_tool_finish<'a>(&'a self, input: ToolFinishInput<'a>) -> ToolLifecycleFuture<'a> {
        Box::pin(async move {
            if input.tool_name.name.ends_with("held") {
                self.reached.notify_one();
                self.release.notified().await;
            }
        })
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn already_dispatched_mcp_completion_wakes_after_the_turn_ends() -> anyhow::Result<()> {
    let server = responses::start_mock_server().await;
    Mock::given(method("POST")).and(path("/mcp"))
        .respond_with(|request: &Request| {
            let body: serde_json::Value = serde_json::from_slice(&request.body).unwrap();
            let result = match body["method"].as_str().unwrap() {
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
    let url = format!("{}/mcp", server.uri());
    let test = test_codex()
        .with_model("test-gpt-5.1-codex")
        .with_extensions(Arc::new(extensions.build()))
        .with_config(move |config| {
            config.features.enable(Feature::CodeMode).unwrap();
            let mcp: McpServerConfig =
                serde_json::from_value(serde_json::json!({"url":url, "required":true})).unwrap();
            config
                .mcp_servers
                .set(std::collections::HashMap::from([(
                    "completion".to_string(),
                    mcp,
                )]))
                .unwrap();
        })
        .build_with_auto_env(&server)
        .await?;
    test.codex.enable_code_mode_completion_wake();
    core_test_support::wait_for_mcp_server(&test.codex, "completion").await?;
    let output = launch(&server, &test, "// @exec: {\"yield_time_ms\": 10}\nconst tool = ALL_TOOLS.find(t => t.name.endsWith('held')); text(await tools[tool.name]({}));").await;
    tokio::time::timeout(Duration::from_secs(10), gate.reached.notified()).await?;
    let request = output.single_request();
    let cell_id = running_cell_id(&request);
    let (wake, result) = wake_and_wait(&server, &cell_id).await;
    no_requests(&wake).await;
    gate.release.notify_one();
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
    let calls = server
        .received_requests()
        .await
        .unwrap()
        .into_iter()
        .filter(|request| {
            request.url.path() == "/mcp"
                && serde_json::from_slice::<serde_json::Value>(&request.body).unwrap()["method"]
                    == "tools/call"
        })
        .count();
    assert_eq!(calls, 1);
    test.codex.shutdown_and_wait().await?;
    Ok(())
}
