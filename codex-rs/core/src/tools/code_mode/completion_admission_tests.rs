use super::*;
use crate::session::tests::make_session_and_context;
use crate::state::ActiveTurn;
use crate::tasks::RegularTask;
use codex_code_mode::NoopCodeModeSessionDelegate;
use codex_extension_api::ExtensionFuture;
use codex_extension_api::ExtensionRegistryBuilder;
use codex_extension_api::TurnAbortInput;
use codex_extension_api::TurnLifecycleContributor;
use codex_extension_api::TurnStartInput;
use codex_protocol::protocol::TurnAbortReason;
use pretty_assertions::assert_eq;
use std::collections::BTreeSet;
use std::sync::OnceLock;
use std::sync::Weak;
use std::time::Duration;
use test_case::test_case;
use tokio::sync::Notify;

#[derive(Default)]
struct StartGate {
    entered: Notify,
    release: Notify,
    session: OnceLock<Weak<crate::session::session::Session>>,
    live: Mutex<BTreeSet<String>>,
    events: Mutex<Vec<(String, &'static str)>>,
}
impl TurnLifecycleContributor for StartGate {
    fn on_turn_start<'a>(&'a self, input: TurnStartInput<'a>) -> ExtensionFuture<'a, ()> {
        Box::pin(async move {
            let id = input.turn_store.level_id().to_string();
            assert!(self.live.lock().expect("state lock").insert(id.clone()));
            self.events.lock().expect("state lock").push((id, "start"));
            self.entered.notify_one();
            self.release.notified().await;
        })
    }

    fn on_turn_abort<'a>(&'a self, input: TurnAbortInput<'a>) -> ExtensionFuture<'a, ()> {
        Box::pin(async move {
            // Cleanup contributors may acquire Core admission; the rejected start
            // must release that lock before publishing its terminal lifecycle.
            let session = self
                .session
                .get()
                .expect("session installed")
                .upgrade()
                .expect("live session");
            let _active = session.active_turn.lock().await;
            let id = input.turn_store.level_id().to_string();
            assert!(self.live.lock().expect("state lock").remove(&id));
            self.events.lock().expect("state lock").push((id, "abort"));
        })
    }
}

#[test_case("interrupt")]
#[test_case("error")]
#[test_case("withdrawn")]
#[test_case("replaced")]
#[test_case("consumed")]
#[test_case("shutdown")]
#[tokio::test]
async fn completion_task_revalidates_after_async_start_preparation(race: &str) {
    let (mut session, turn) = make_session_and_context().await;
    let gate = Arc::new(StartGate::default());
    let mut extensions = ExtensionRegistryBuilder::new();
    extensions.turn_lifecycle_contributor(gate.clone());
    session.services.extensions = Arc::new(extensions.build());
    let session = Arc::new(session);
    gate.session
        .set(Arc::downgrade(&session))
        .expect("install session once");
    let rejected_id = turn.extension_data.level_id().to_string();
    let completions = Arc::clone(&session.services.code_mode_service.completions);
    completions.state().enabled = true;
    let cell_id = CellId::new("generation:1".to_string());
    let mut reservation = completions
        .reserve("call", "origin")
        .expect("available capacity")
        .expect("enabled completion");
    let delegate = CompletionDelegate::wrap(Arc::new(NoopCodeModeSessionDelegate), &reservation);
    reservation.bind(&cell_id, CompletionReadySupport::Supported);
    delegate.cell_completion_ready(&cell_id);
    reservation.yielded();
    completions.record_outputs("origin", &[super::tests::output("call", &cell_id)]);
    let (trigger, _) = completions.batch().expect("ready batch");
    let active = ActiveTurn::default();
    turn.extension_data.insert(CompletionAdmission {
        trigger,
        turn_state: Arc::clone(&active.turn_state),
    });
    *session.active_turn.lock().await = Some(active);
    let starting = tokio::spawn({
        let session = session.clone();
        async move {
            session
                .start_task_with_completion_admission(
                    Arc::new(turn),
                    Vec::new(),
                    RegularTask::new(),
                )
                .await
        }
    });
    gate.entered.notified().await;
    let replacement = Arc::new(tokio::sync::Mutex::new(crate::state::TurnState::default()));
    match race {
        "interrupt" => session.abort_all_tasks(TurnAbortReason::Interrupted).await,
        "error" => {
            let _active = session.active_turn.lock().await;
            completions.suppress();
        }
        "withdrawn" => *session.active_turn.lock().await = None,
        "replaced" => {
            gate.live
                .lock()
                .expect("state lock")
                .insert("replacement".to_string());
            let active = ActiveTurn {
                turn_state: Arc::clone(&replacement),
                ..Default::default()
            };
            *session.active_turn.lock().await = Some(active);
        }
        "consumed" => delegate.cell_closed(&cell_id),
        "shutdown" => completions.shutdown(),
        _ => unreachable!(),
    }
    gate.release.notify_one();
    assert!(
        !tokio::time::timeout(Duration::from_secs(5), starting)
            .await
            .expect("admission cleanup must not deadlock")
            .expect("start task joined")
    );
    assert_eq!(
        *gate.events.lock().expect("state lock"),
        vec![(rejected_id.clone(), "start"), (rejected_id, "abort")],
    );
    assert_eq!(
        *gate.live.lock().expect("state lock"),
        if race == "replaced" {
            BTreeSet::from(["replacement".to_string()])
        } else {
            BTreeSet::new()
        },
    );
    if race == "replaced" {
        assert!(Arc::ptr_eq(
            &session
                .active_turn
                .lock()
                .await
                .as_ref()
                .expect("replacement retained")
                .turn_state,
            &replacement
        ));
    }
    assert!(
        session
            .active_turn
            .lock()
            .await
            .as_ref()
            .is_none_or(|turn| turn.task.is_none())
    );
    completions.shutdown();
}
