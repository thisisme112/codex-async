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
