use super::*;
use crate::session::TurnInput;
use crate::session::tests::make_session_and_context_with_rx;
use crate::state::TaskKind;
use crate::tasks::SessionTask;
use crate::tasks::SessionTaskResult;
use codex_protocol::protocol::EventMsg;
use codex_protocol::request_user_input::RequestUserInputArgs;
use codex_protocol::request_user_input::RequestUserInputResponse;
use pretty_assertions::assert_eq;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

struct HeldTurn;
impl SessionTask for HeldTurn {
    fn kind(&self) -> TaskKind {
        TaskKind::Regular
    }
    fn span_name(&self) -> &'static str {
        "held_test_turn"
    }
    async fn run(
        self: Arc<Self>,
        _session: Arc<Session>,
        _ctx: Arc<TurnContext>,
        _input: Vec<TurnInput>,
        cancellation_token: CancellationToken,
    ) -> SessionTaskResult {
        cancellation_token.cancelled().await;
        Ok(None)
    }
}

#[tokio::test]
async fn idle_prompt_defers_registration_and_keeps_response_owner() {
    let (session, turn, rx) = make_session_and_context_with_rx().await;
    let future = ToolPromptOrigin::Cell.scope(session.request_user_input(
        &turn,
        "deferred".to_string(),
        RequestUserInputArgs {
            questions: Vec::new(),
            is_blocking: true,
            auto_resolution_ms: None,
        },
    ));
    tokio::pin!(future);
    assert!(futures::poll!(&mut future).is_pending());
    assert!(rx.try_recv().is_err());
    assert!(!*session.subscribe_elicitation_pause_state().borrow());
    *session.active_turn.lock().await = Some(ActiveTurn::default());
    session.services.code_mode_service.completions.activity();
    assert!(futures::poll!(&mut future).is_pending());
    assert!(
        rx.try_recv().is_err(),
        "an idle admission reservation is not a running turn"
    );
    session.spawn_task(turn.clone(), Vec::new(), HeldTurn).await;
    session.services.code_mode_service.completions.activity();
    assert!(futures::poll!(&mut future).is_pending());
    while !matches!(rx.recv().await.unwrap().msg, EventMsg::RequestUserInput(_)) {}
    let response = RequestUserInputResponse {
        answers: Default::default(),
    };
    session
        .notify_user_input_response(&turn.sub_id, response.clone())
        .await;
    assert_eq!(future.await.unwrap().response, response);
    assert!(!*session.subscribe_elicitation_pause_state().borrow());
    session
        .abort_all_tasks(codex_protocol::protocol::TurnAbortReason::Interrupted)
        .await;
}

#[tokio::test]
async fn deferred_prompt_cancellation_and_shutdown_do_not_emit_prompts() {
    let (session, turn, rx) = make_session_and_context_with_rx().await;
    for shutdown in [false, true] {
        let token = CancellationToken::new();
        let future =
            token.run_until_cancelled(ToolPromptOrigin::Cell.scope(session.request_user_input(
                &turn,
                "cancelled".to_string(),
                RequestUserInputArgs {
                    questions: Vec::new(),
                    is_blocking: true,
                    auto_resolution_ms: None,
                },
            )));
        tokio::pin!(future);
        assert!(futures::poll!(&mut future).is_pending());
        if shutdown {
            session.services.code_mode_service.shutdown_dispatch();
        } else {
            token.cancel();
        }
        assert!(future.await.flatten().is_none());
        assert!(rx.try_recv().is_err());
    }
}
