use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;

use codex_code_mode::*;
use codex_features::Feature;
use core_test_support::test_codex::TestCodex;
use core_test_support::test_codex::test_codex;
use wiremock::MockServer;

#[derive(Default)]
pub(super) struct ControlledSession {
    pub(super) delegate: Mutex<Option<Arc<dyn CodeModeSessionDelegate>>>,
    pub(super) result: Mutex<Option<RuntimeResponse>>,
    pub(super) supported: bool,
    pub(super) early: bool,
}

impl ControlledSession {
    pub(super) fn complete(&self) {
        *self.result.lock().expect("result lock") = Some(RuntimeResponse::Result {
            cell_id: cell_id(),
            content_items: vec![FunctionCallOutputContentItem::InputText {
                text: "retained result".to_string(),
            }],
            error_text: None,
            result_buffered_for: Duration::ZERO,
            code_mode_host_duration: None,
        });
        if let Some(delegate) = self.delegate.lock().expect("delegate lock").as_ref()
            && self.supported
            && delegate.wants_completion_ready()
        {
            delegate.cell_completion_ready(&cell_id());
            delegate.cell_completion_ready(&cell_id());
        }
    }
}

pub(super) fn cell_id() -> CellId {
    CellId::new("generation:1".to_string())
}

fn yielded() -> RuntimeResponse {
    RuntimeResponse::Yielded {
        cell_id: cell_id(),
        content_items: Vec::new(),
        code_mode_host_duration: None,
    }
}

impl CodeModeSession for ControlledSession {
    fn execute<'a>(
        &'a self,
        _request: ExecuteRequest,
        delegate: Arc<dyn CodeModeSessionDelegate>,
        _preempt: Option<tokio_util::sync::CancellationToken>,
    ) -> CodeModeSessionResultFuture<'a, StartedCell> {
        Box::pin(async move {
            *self.delegate.lock().expect("delegate lock") = Some(delegate);
            if self.early {
                self.complete();
            }
            let mut started = StartedCell::from_future(cell_id(), async { Ok(yielded()) });
            if self.supported {
                started.completion_ready_support = CompletionReadySupport::Supported;
            }
            Ok(started)
        })
    }

    fn wait<'a>(
        &'a self,
        _request: WaitRequest,
        _preempt: Option<tokio_util::sync::CancellationToken>,
    ) -> CodeModeSessionResultFuture<'a, WaitOutcome> {
        Box::pin(async move {
            if let Some(result) = self.result.lock().expect("result lock").take() {
                // Delay cleanup until shutdown: Core must retire a known claim itself.
                Ok(WaitOutcome::LiveCell(result))
            } else {
                Ok(WaitOutcome::LiveCell(yielded()))
            }
        })
    }

    fn terminate<'a>(
        &'a self,
        _request: TerminateRequest,
    ) -> CodeModeSessionResultFuture<'a, WaitOutcome> {
        Box::pin(async move {
            if let Some(delegate) = self.delegate.lock().expect("delegate lock").take() {
                delegate.cell_closed(&cell_id());
            }
            Ok(WaitOutcome::LiveCell(RuntimeResponse::Terminated {
                cell_id: cell_id(),
                content_items: Vec::new(),
                code_mode_host_duration: None,
            }))
        })
    }

    fn shutdown<'a>(&'a self) -> CodeModeSessionResultFuture<'a, ()> {
        Box::pin(async move {
            if let Some(delegate) = self.delegate.lock().expect("delegate lock").take() {
                delegate.cell_closed(&cell_id());
            }
            Ok(())
        })
    }
}

struct Provider(Arc<ControlledSession>);
impl CodeModeSessionProvider for Provider {
    fn create_session(&self) -> CodeModeSessionProviderFuture<'_> {
        Box::pin(async { Ok(self.0.clone() as Arc<dyn CodeModeSession>) })
    }
}

pub(super) fn builder(
    session: Arc<ControlledSession>,
    client: bool,
) -> core_test_support::test_codex::TestCodexBuilder {
    test_codex()
        .with_model("test-gpt-5.1-codex")
        .with_config(move |config| {
            config
                .features
                .enable(Feature::CodeMode)
                .expect("enable Code Mode");
            // The upstream harness installs its process host after the manager
            // configurer. Keep it from replacing this controlled provider.
            // Real process-host callbacks are exercised by the timer/MCP tests.
            config
                .features
                .disable(Feature::CodeModeHost)
                .expect("preserve controlled Code Mode provider");
        })
        .with_thread_manager(move |manager| {
            if client {
                manager.enable_code_mode_completion_wake();
            }
            manager.with_code_mode_session_provider(Arc::new(Provider(session)))
        })
}

pub(super) async fn build(
    server: &MockServer,
    session: Arc<ControlledSession>,
    client: bool,
) -> anyhow::Result<TestCodex> {
    builder(session, client).build_with_auto_env(server).await
}
