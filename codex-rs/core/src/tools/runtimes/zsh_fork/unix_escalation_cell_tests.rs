use super::*;
use crate::session::SessionSettingsUpdate;
use crate::session::TurnInput;
use crate::session::session::Session;
use crate::session::step_settings::StepSettingsUpdate;
use crate::session::tests::make_session_and_context_with_rx;
use crate::session::turn_context::NewTurnContextOptions;
use crate::session::turn_context::TurnContext;
use crate::tools::code_mode::ToolPromptOrigin;
use crate::tools::code_mode::interactive_test_support::ReviewHook;
use crate::tools::code_mode::interactive_test_support::sibling_cell_attempt;
use crate::tools::context::ToolCallSource;
use codex_protocol::protocol::EventMsg;
use codex_protocol::protocol::TurnAbortReason;
use pretty_assertions::assert_eq;
use std::time::Duration;
use test_case::test_case;
use tokio::sync::Notify;

struct HeldTurn(Arc<Notify>);
impl crate::tasks::SessionTask for HeldTurn {
    fn kind(&self) -> crate::state::TaskKind {
        crate::state::TaskKind::Regular
    }
    fn span_name(&self) -> &'static str {
        "cell_review_test"
    }
    async fn run(
        self: Arc<Self>,
        _: Arc<Session>,
        _: Arc<TurnContext>,
        _: Vec<TurnInput>,
        cancellation: CancellationToken,
    ) -> crate::tasks::SessionTaskResult {
        tokio::select! { _ = self.0.notified() => {}, _ = cancellation.cancelled() => {} }
        Ok(None)
    }
}

async fn start_turn(session: &Arc<Session>, id: &str, policy: AskForApproval) -> Arc<Notify> {
    let (turn, _) = session
        .new_turn_with_sub_id(
            id.to_string(),
            SessionSettingsUpdate {
                step_settings: StepSettingsUpdate {
                    approval_policy: Some(policy),
                    approvals_reviewer: Some(codex_config::types::ApprovalsReviewer::User),
                    ..Default::default()
                },
                ..Default::default()
            },
            NewTurnContextOptions::default(),
        )
        .await
        .unwrap();
    let done = Arc::new(Notify::new());
    session
        .start_task(turn, Vec::new(), HeldTurn(done.clone()))
        .await;
    session.services.code_mode_service.completions.activity();
    done
}

fn provider(session: &Arc<Session>, turn: &Arc<TurnContext>) -> CoreShellActionProvider {
    CoreShellActionProvider {
        prompt_origin: ToolPromptOrigin::current(),
        policy: Arc::new(RwLock::new(Policy::empty())),
        session: session.clone(),
        review_context: GuardianReviewContext::from(turn.clone()),
        call_id: "cell-execve".to_string(),
        environment_id: "local".to_string(),
        source: GuardianCommandSource::UnifiedExec,
        tool_name: ToolName::plain("exec_command"),
        approval_policy: AskForApproval::OnRequest,
        permission_profile: PermissionProfile::read_only(),
        sandbox_permissions: SandboxPermissions::RequireEscalated,
        approval_sandbox_permissions: SandboxPermissions::RequireEscalated,
        prompt_permissions: None,
        stopwatch: Stopwatch::unlimited(),
    }
}

async fn cell_provider(session: &Arc<Session>, turn: &Arc<TurnContext>) -> CoreShellActionProvider {
    ToolPromptOrigin::from_source(&ToolCallSource::CodeMode {
        cell_id: "cell".to_string(),
        runtime_tool_call_id: "command".to_string(),
    })
    .scope(async { provider(session, turn) })
    .await
}

#[test_case(AskForApproval::OnRequest; "later_turn_owns_prompt")]
#[test_case(AskForApproval::Never; "later_turn_policy_denies")]
#[tokio::test]
async fn cell_execve_retries_review_when_turn_ends_before_registration(policy: AskForApproval) {
    let (session, origin, events) = make_session_and_context_with_rx().await;
    let gate = ReviewHook::install(&session, &origin);
    let provider = cell_provider(&session, &origin).await;
    let program = AbsolutePathBuf::from_absolute_path("/usr/bin/touch").unwrap();
    let argv = vec!["touch".to_string(), "/tmp/idle-cell-output".to_string()];
    let workdir = AbsolutePathBuf::from_absolute_path("/tmp").unwrap();
    let future = provider.determine_action(&program, &argv, &workdir);
    tokio::pin!(future);
    assert!(futures::poll!(&mut future).is_pending());
    assert!(events.try_recv().is_err());
    let finish_b = start_turn(&session, "B", AskForApproval::OnRequest).await;
    tokio::time::timeout(Duration::from_secs(5), async {
        tokio::select! { _ = gate.entered.notified() => {}, r = &mut future => panic!("premature review: {r:?}") }
    }).await.unwrap();
    finish_b.notify_one();
    tokio::time::timeout(Duration::from_secs(5), async {
        while session.active_turn.lock().await.is_some() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap(); // Proves no active-turn lock is held across the hook.
    while let Ok(event) = events.try_recv() {
        assert!(!matches!(event.msg, EventMsg::ExecApprovalRequest(_)));
    }
    gate.release.notify_one();
    // Drive hook completion and final admission without installing another task.
    tokio::time::timeout(Duration::from_secs(5), async {
        loop { tokio::select! {
            event = events.recv() => { if matches!(event.unwrap().msg, EventMsg::HookCompleted(_)) { break; } },
            r = &mut future => panic!("must defer after B ends: {r:?}"),
        } }
    }).await.unwrap();
    assert!(futures::poll!(&mut future).is_pending());
    assert!(!*session.subscribe_elicitation_pause_state().borrow());
    while let Ok(event) = events.try_recv() {
        assert!(!matches!(event.msg, EventMsg::ExecApprovalRequest(_)));
    }
    let _finish_c = start_turn(&session, "C", policy).await;
    let expected = if policy == AskForApproval::Never {
        EscalationDecision::deny(Some(PROMPT_CONFLICT_REASON.to_string()))
    } else {
        let approval = tokio::time::timeout(Duration::from_secs(5), async {
            loop { tokio::select! {
                event = events.recv() => { if let EventMsg::ExecApprovalRequest(approval) = event.unwrap().msg { break approval; } },
                r = &mut future => panic!("expected C prompt: {r:?}"),
            } }
        }).await.unwrap();
        assert_eq!(approval.turn_id, "C");
        session
            .notify_approval(&approval.approval_id.unwrap(), ReviewDecision::Approved)
            .await;
        EscalationDecision::escalate(EscalationExecution::Unsandboxed)
    };
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(5), &mut future)
            .await
            .unwrap()
            .unwrap(),
        expected
    );
    let expected_turns = if policy == AskForApproval::Never {
        vec!["B"]
    } else {
        vec!["B", "C"]
    };
    assert_eq!(*gate.turns.lock().unwrap(), expected_turns);
    while let Ok(event) = events.try_recv() {
        assert!(!matches!(event.msg, EventMsg::ExecApprovalRequest(_)));
    }
    session.abort_all_tasks(TurnAbortReason::Interrupted).await;
}

#[test_case("fail")]
#[test_case("complete")]
#[tokio::test]
async fn direct_execve_created_before_sibling_cell_keeps_idle_denial(source: &str) {
    let (session, turn, events) = make_session_and_context_with_rx().await;
    let direct = provider(&session, &turn);
    let cell = cell_provider(&session, &turn).await;
    sibling_cell_attempt(turn.clone(), source).await;
    let program = AbsolutePathBuf::from_absolute_path("/usr/bin/touch").unwrap();
    let argv = vec!["touch".to_string(), "/tmp/idle-cell-output".to_string()];
    let workdir = AbsolutePathBuf::from_absolute_path("/tmp").unwrap();
    let expected = EscalationDecision::deny(Some(
        "cannot approve intercepted execution without an active turn".to_string(),
    ));
    assert_eq!(
        tokio::time::timeout(
            Duration::from_secs(5),
            direct.determine_action(&program, &argv, &workdir)
        )
        .await
        .unwrap()
        .unwrap(),
        expected
    );
    let pending = cell.determine_action(&program, &argv, &workdir);
    tokio::pin!(pending);
    assert!(futures::poll!(&mut pending).is_pending());
    assert!(events.try_recv().is_err());
    session.services.code_mode_service.shutdown_dispatch();
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(5), pending)
            .await
            .unwrap()
            .unwrap(),
        expected
    );
}

#[test_case(false, false; "cancel_idle")]
#[test_case(true, false; "shutdown_idle")]
#[test_case(false, true; "cancel_during_hook")]
#[test_case(true, true; "shutdown_during_hook")]
#[tokio::test]
async fn cell_execve_idle_cancellation_releases_callback(shutdown: bool, during_hook: bool) {
    let (session, turn, events) = make_session_and_context_with_rx().await;
    let hook = during_hook.then(|| ReviewHook::install(&session, &turn));
    let provider = cell_provider(&session, &turn).await;
    let program = AbsolutePathBuf::from_absolute_path("/usr/bin/touch").unwrap();
    let argv = vec!["touch".to_string(), "/tmp/idle-cell-output".to_string()];
    let workdir = AbsolutePathBuf::from_absolute_path("/tmp").unwrap();
    let cancellation = CancellationToken::new();
    let pending =
        cancellation.run_until_cancelled(provider.determine_action(&program, &argv, &workdir));
    tokio::pin!(pending);
    assert!(futures::poll!(&mut pending).is_pending());
    if let Some(hook) = &hook {
        let _finish = start_turn(&session, "reviewing", AskForApproval::OnRequest).await;
        tokio::time::timeout(Duration::from_secs(5), async {
            tokio::select! { _ = hook.entered.notified() => {}, r = &mut pending => panic!("premature review: {r:?}") }
        }).await.unwrap();
    }
    if shutdown {
        session.services.code_mode_service.shutdown_dispatch();
    } else {
        cancellation.cancel();
    }
    let result = tokio::time::timeout(Duration::from_secs(5), pending)
        .await
        .unwrap();
    if shutdown {
        assert!(matches!(
            result.unwrap().unwrap(),
            EscalationDecision::Deny { .. }
        ));
    } else {
        assert!(result.is_none());
    }
    while let Ok(event) = events.try_recv() {
        assert!(!matches!(event.msg, EventMsg::ExecApprovalRequest(_)));
    }
    assert!(!*session.subscribe_elicitation_pause_state().borrow());
    session.abort_all_tasks(TurnAbortReason::Interrupted).await;
}
