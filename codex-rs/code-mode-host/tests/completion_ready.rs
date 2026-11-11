#![allow(clippy::expect_used)]

use std::sync::Arc;
use std::time::Duration;

use codex_code_mode::*;
use codex_code_mode_runtime::InProcessCodeModeSession;
use pretty_assertions::assert_eq;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

#[path = "support/host.rs"]
mod host;

#[derive(Debug, PartialEq)]
enum Event {
    Ready(CellId),
    Closed(CellId),
}

struct Delegate {
    enabled: bool,
    events: mpsc::Sender<Event>,
}

impl CodeModeSessionDelegate for Delegate {
    fn invoke_tool<'a>(
        &'a self,
        _invocation: CodeModeNestedToolCall,
        _cancellation_token: CancellationToken,
    ) -> ToolInvocationFuture<'a> {
        Box::pin(async { Err("unexpected tool call".to_string()) })
    }

    fn notify<'a>(
        &'a self,
        _call_id: String,
        _cell_id: CellId,
        _text: String,
        _cancellation_token: CancellationToken,
    ) -> NotificationFuture<'a> {
        Box::pin(async { Ok(()) })
    }

    fn wants_completion_ready(&self) -> bool {
        self.enabled
    }

    fn cell_completion_ready(&self, cell_id: &CellId) {
        self.events
            .try_send(Event::Ready(cell_id.clone()))
            .expect("readiness receiver");
    }

    fn cell_closed(&self, cell_id: &CellId) {
        self.events
            .try_send(Event::Closed(cell_id.clone()))
            .expect("cleanup receiver");
    }
}

fn request(source: &str) -> ExecuteRequest {
    ExecuteRequest {
        tool_call_id: "completion-test".to_string(),
        enabled_tools: Vec::new(),
        source: source.to_string(),
        yield_time_ms: Some(1_000),
        max_output_tokens: None,
    }
}

async fn next(events: &mut mpsc::Receiver<Event>) -> Event {
    tokio::time::timeout(Duration::from_secs(/*secs*/ 10), events.recv())
        .await
        .expect("event timeout")
        .expect("event channel closed")
}

#[tokio::test]
async fn completion_contract_across_providers() {
    let host = host::HostHarness::start("grpc://127.0.0.1:0")
        .await
        .unwrap();
    let stdio = ProcessOwnedCodeModeSessionProvider::with_host_program(
        codex_utils_cargo_bin::cargo_bin("codex-code-mode-host").unwrap(),
    );
    let grpc = GrpcCodeModeSessionProvider::new(&host.endpoint);
    let sessions: Vec<Arc<dyn CodeModeSession>> = vec![
        Arc::new(InProcessCodeModeSession::new()),
        stdio.create_session().await.unwrap(),
        grpc.create_session().await.unwrap(),
    ];
    for session in sessions {
        for enabled in [true, false] {
            for ending in ["", "throw new Error('script failed');"] {
                let (events, mut receiver) = mpsc::channel(/*buffer*/ 4);
                let started = session
                    .execute(
                        request(&format!(
                            "await yield_control(); store('committed', 7); text('done'); {ending}"
                        )),
                        Arc::new(Delegate { enabled, events }),
                        /*preempt*/ None,
                    )
                    .await
                    .unwrap();
                let id = started.cell_id.clone();
                assert_eq!(
                    started.completion_ready_support,
                    if enabled {
                        CompletionReadySupport::Supported
                    } else {
                        CompletionReadySupport::Unsupported
                    }
                );
                // Completion may precede consumption of the initial yielded output.
                if enabled {
                    assert_eq!(next(&mut receiver).await, Event::Ready(id.clone()));
                }
                assert!(matches!(
                    started.initial_response().await.unwrap(),
                    RuntimeResponse::Yielded { .. }
                ));
                // Readiness did not claim the result or trigger cleanup.
                assert!(receiver.try_recv().is_err());
                let response: RuntimeResponse = session
                    .wait(
                        WaitRequest {
                            cell_id: id.clone(),
                            yield_time_ms: 1_000,
                            max_output_tokens: None,
                        },
                        /*preempt*/ None,
                    )
                    .await
                    .unwrap()
                    .into();
                let RuntimeResponse::Result {
                    content_items,
                    error_text,
                    ..
                } = response
                else {
                    panic!("expected the retained terminal result");
                };
                assert_eq!(
                    content_items,
                    vec![FunctionCallOutputContentItem::InputText {
                        text: "done".to_string()
                    }]
                );
                assert_eq!(error_text.is_some(), !ending.is_empty());
                assert_eq!(next(&mut receiver).await, Event::Closed(id.clone()));
                assert!(matches!(
                    session
                        .wait(
                            WaitRequest {
                                cell_id: id,
                                yield_time_ms: 1,
                                max_output_tokens: None,
                            },
                            /*preempt*/ None
                        )
                        .await
                        .unwrap(),
                    WaitOutcome::MissingCell(_)
                ));
                let reader = session
                    .execute(
                        request("text(load('committed'));"),
                        Arc::new(NoopCodeModeSessionDelegate),
                        /*preempt*/ None,
                    )
                    .await
                    .unwrap();
                let RuntimeResponse::Result { content_items, .. } =
                    reader.initial_response().await.unwrap()
                else {
                    panic!("expected stored value")
                };
                assert_eq!(
                    content_items,
                    vec![FunctionCallOutputContentItem::InputText {
                        text: "7".to_string()
                    }]
                );
            }
        }
        for shutdown in [false, true] {
            let (events, mut receiver) = mpsc::channel(/*buffer*/ 4);
            let started = session
                .execute(
                    request("await yield_control(); await new Promise(() => {});"),
                    Arc::new(Delegate {
                        enabled: true,
                        events,
                    }),
                    /*preempt*/ None,
                )
                .await
                .unwrap();
            let id = started.cell_id.clone();
            assert!(matches!(
                started.initial_response().await.unwrap(),
                RuntimeResponse::Yielded { .. }
            ));
            if shutdown {
                session.shutdown().await.unwrap();
            } else {
                assert!(matches!(
                    session.terminate(id.clone().into()).await.unwrap(),
                    WaitOutcome::LiveCell(RuntimeResponse::Terminated { .. })
                ));
            }
            assert_eq!(next(&mut receiver).await, Event::Closed(id));
            assert!(receiver.try_recv().is_err());
        }
    }
}
