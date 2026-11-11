//! Deterministic PermissionRequest barrier shared by the two detached callback tests.
use crate::session::session::Session;
use crate::session::turn_context::TurnContext;
use codex_hooks::HookMcpCall;
use codex_hooks::HookMcpExecutor;
use codex_hooks::HookMcpOutput;
use codex_hooks::Hooks;
use codex_hooks::HooksConfig;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;
use tokio::sync::Notify;

#[derive(Default)]
pub(crate) struct ReviewHook {
    pub(crate) entered: Notify,
    pub(crate) release: Notify,
    pub(crate) turns: Mutex<Vec<String>>,
}
impl HookMcpExecutor for ReviewHook {
    fn execute(
        &self,
        call: HookMcpCall,
    ) -> futures::future::BoxFuture<'_, anyhow::Result<HookMcpOutput>> {
        Box::pin(async move {
            let first = {
                let mut turns = self.turns.lock().unwrap();
                turns.push(call.input["turn"].as_str().unwrap().to_string());
                turns.len() == 1
            };
            if first {
                self.entered.notify_one();
                self.release.notified().await;
            }
            // No decision: continue through the real reviewer/user path.
            Ok(HookMcpOutput {
                text: String::new(),
                source_tool_namespace: None,
            })
        })
    }
}
impl ReviewHook {
    pub(crate) fn install(session: &Session, turn: &TurnContext) -> Arc<Self> {
        let home = &turn.config.codex_home;
        std::fs::create_dir_all(home).unwrap();
        std::fs::write(home.join("hooks.json"), serde_json::json!({"hooks": {
            "PermissionRequest": [{"hooks": [{"type":"mcp_tool", "server":"test", "tool":"barrier",
                "input":{"turn":"${turn_id}"}, "timeout":20}]}]
        }}).to_string()).unwrap();
        let mut config = HooksConfig {
            feature_enabled: true,
            config_layer_stack: Some(turn.config.config_layer_stack.clone()),
            ..Default::default()
        };
        let hooks = codex_hooks::list_hooks(config.clone()).hooks;
        assert_eq!(hooks.len(), 1);
        config.config_layer_stack = Some(
            turn.config
                .config_layer_stack
                .with_user_config(
                    &home.join(codex_config::CONFIG_TOML_FILE),
                    serde_json::from_value(serde_json::json!({"hooks":{"state": {
                        hooks[0].key.clone(): {"trusted_hash":hooks[0].current_hash.clone()}
                    }}}))
                    .unwrap(),
                )
                .unwrap(),
        );
        let gate = Arc::new(Self::default());
        let (hooks, _) = Hooks::new(config, session.thread_id, gate.clone()).unwrap();
        session.services.hooks.store(Arc::new(hooks));
        gate
    }
}

// Exercise the real service entry with failed and synchronously completed providers.
// It must not mutate the shared originating turn of an older direct callback.
pub(crate) async fn sibling_cell_attempt(turn: Arc<TurnContext>, source: &str) {
    use codex_code_mode::*;
    struct ImmediateSession;
    impl CodeModeSession for ImmediateSession {
        fn execute<'a>(
            &'a self,
            request: ExecuteRequest,
            delegate: Arc<dyn CodeModeSessionDelegate>,
            _preempt: Option<tokio_util::sync::CancellationToken>,
        ) -> CodeModeSessionResultFuture<'a, StartedCell> {
            Box::pin(async move {
                if request.source == "fail" {
                    return Err("execute failed".to_string());
                }
                let cell_id = CellId::new("synchronous".to_string());
                delegate.cell_completion_ready(&cell_id);
                Ok(StartedCell::from_future(cell_id.clone(), async move {
                    Ok(RuntimeResponse::Result {
                        cell_id,
                        content_items: Vec::new(),
                        error_text: None,
                        result_buffered_for: Duration::ZERO,
                        code_mode_host_duration: None,
                    })
                }))
            })
        }
        fn wait<'a>(
            &'a self,
            _: WaitRequest,
            _: Option<tokio_util::sync::CancellationToken>,
        ) -> CodeModeSessionResultFuture<'a, WaitOutcome> {
            unreachable!()
        }
        fn terminate<'a>(
            &'a self,
            _: TerminateRequest,
        ) -> CodeModeSessionResultFuture<'a, WaitOutcome> {
            unreachable!()
        }
        fn shutdown<'a>(&'a self) -> CodeModeSessionResultFuture<'a, ()> {
            Box::pin(async { Ok(()) })
        }
    }
    let service = super::CodeModeService::new(
        codex_protocol::ThreadId::default(),
        Arc::new(DisabledCodeModeSessionProvider),
        &turn.config.code_mode,
        Default::default(),
    );
    assert!(service.session.set(Arc::new(ImmediateSession)).is_ok());
    let started = service
        .execute(
            ExecuteRequest {
                tool_call_id: "sibling".to_string(),
                enabled_tools: Vec::new(),
                source: source.to_string(),
                yield_time_ms: None,
                max_output_tokens: None,
            },
            crate::session::step_context::StepContext::for_test(turn),
            /*completion*/ None,
        )
        .await;
    if source == "fail" {
        assert!(started.is_err());
    } else {
        assert!(matches!(
            started.unwrap().initial_response().await.unwrap(),
            RuntimeResponse::Result { .. }
        ));
    }
    service.shutdown().await.unwrap();
}
