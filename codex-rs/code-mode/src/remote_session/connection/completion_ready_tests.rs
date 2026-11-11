use super::*;
use codex_code_mode_protocol::CompletionReadySupport;
use pretty_assertions::assert_eq;

struct CompletionDelegate(mpsc::Sender<CellId>);

impl CodeModeSessionDelegate for CompletionDelegate {
    fn invoke_tool<'a>(
        &'a self,
        _: CodeModeNestedToolCall,
        _: CancellationToken,
    ) -> ToolInvocationFuture<'a> {
        Box::pin(async { unreachable!() })
    }
    fn notify<'a>(
        &'a self,
        _: String,
        _: CellId,
        _: String,
        _: CancellationToken,
    ) -> NotificationFuture<'a> {
        Box::pin(async { unreachable!() })
    }
    fn cell_closed(&self, _: &CellId) {}
    fn wants_completion_ready(&self) -> bool {
        true
    }
    fn cell_completion_ready(&self, cell_id: &CellId) {
        self.0.try_send(cell_id.clone()).unwrap();
    }
}

#[tokio::test]
async fn readiness_requires_acknowledgment_and_preserves_generation_identity() {
    for supported in [false, true] {
        let mut harness = DriverHarness::with_completion_ready(supported);
        let mut session = remote_session();
        session.generation = 2;
        harness.open(session.clone()).await;
        let (sender, mut events) = mpsc::channel(/*buffer*/ 4);
        let (response_tx, response_rx) = oneshot::channel();
        harness
            .command_tx
            .send(DriverCommand::Execute {
                yield_signal: None,
                session: session.clone(),
                delegate: Arc::new(CompletionDelegate(sender)),
                request: ExecuteRequest {
                    tool_call_id: "call".to_string(),
                    source: "text(1)".to_string(),
                    enabled_tools: vec![],
                    yield_time_ms: None,
                    max_output_tokens: None,
                },
                caller_cancellation: CancellationToken::new(),
                response_tx,
            })
            .await
            .unwrap();
        let frame = harness.outgoing_rx.recv().await.unwrap();
        let message =
            EncodedFrame::decode_framed::<ClientToHost>(&frame.into_framed_bytes()).unwrap();
        let ClientToHost::Request {
            id,
            request: HostRequest::Execute { request, .. },
        } = message
        else {
            panic!("expected execute")
        };
        assert_eq!(request.completion_ready, supported);
        assert_eq!(
            serde_json::to_value(&request)
                .unwrap()
                .get("completion_ready"),
            supported.then_some(&serde_json::Value::Bool(true))
        );
        harness
            .event_tx
            .send(DriverEvent::HostMessage(HostToClient::Response {
                id,
                result: WireResult::Ok {
                    value: HostResponse::ExecutionStarted {
                        cell_id: CellId::new("1".to_string()).into(),
                    },
                },
            }))
            .await
            .unwrap();
        let delivered = response_rx.await.unwrap().unwrap();
        harness.execute_claim_tx.send(id).unwrap();
        assert_eq!(
            delivered.started.completion_ready_support,
            if supported {
                CompletionReadySupport::Supported
            } else {
                CompletionReadySupport::Unsupported
            }
        );
        for _ in 0..2 {
            harness
                .event_tx
                .send(DriverEvent::HostMessage(
                    HostToClient::CellCompletionReady {
                        session_id: session.id.clone(),
                        cell_id: CellId::new("1".to_string()).into(),
                    },
                ))
                .await
                .unwrap();
        }
        // An ordinary response is an ordering barrier after both events.
        harness
            .event_tx
            .send(DriverEvent::HostMessage(HostToClient::InitialResponse {
                id,
                result: WireResult::Ok {
                    value: WireRuntimeResponse::Yielded {
                        cell_id: CellId::new("1".to_string()).into(),
                        content_items: vec![],
                        code_mode_host_duration_ns: 0,
                    },
                },
            }))
            .await
            .unwrap();
        delivered.started.initial_response().await.unwrap();
        if supported {
            assert_eq!(events.try_recv().unwrap(), CellId::new("g2:1".to_string()));
        }
        assert!(events.try_recv().is_err());
    }
}
