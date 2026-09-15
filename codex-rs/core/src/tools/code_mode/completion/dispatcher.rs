use std::sync::Arc;
use std::sync::Weak;

use codex_protocol::turn_input::StartIfIdleSubmission;
use codex_protocol::turn_input::TurnInput;
use codex_protocol::turn_input::TurnInputRequest;
use codex_protocol::turn_input::TurnStartOptions;
use tokio::sync::Mutex;

use crate::CodexThread;
use crate::state::ActiveTurn;
use crate::state::TurnState;

use super::Completions;

/// Exact reservation validated again when Core installs a completion task.
/// Other start paths keep their existing admission behavior.
pub(crate) struct CompletionAdmission {
    pub(crate) turn_state: Arc<Mutex<TurnState>>,
    pub(crate) trigger: String,
}

impl CompletionAdmission {
    pub(crate) fn permits(&self, active: &Option<ActiveTurn>, completions: &Completions) -> bool {
        active.as_ref().is_some_and(|turn| {
            turn.task.is_none() && Arc::ptr_eq(&turn.turn_state, &self.turn_state)
        }) && completions.permits(&self.trigger)
    }
}

impl CodexThread {
    /// Attaches shared Cell/command completion wake to an interactive client's live thread.
    /// Existing Code Mode activation is unchanged; other clients remain opted out.
    pub fn enable_code_mode_completion_wake(self: &Arc<Self>) {
        let completions = Arc::clone(&self.session.services.code_mode_service.completions);
        let mut state = completions.state();
        if state.enabled {
            return;
        }
        state.enabled = true;
        drop(state);
        tokio::spawn(dispatch(Arc::downgrade(self), completions));
    }
}

async fn dispatch(thread: Weak<CodexThread>, completions: Arc<Completions>) {
    // Subscribe before the initial scan; watch retains activity during admission.
    let mut activity = completions.subscribe();
    loop {
        activity.borrow_and_update();
        if let Some((trigger, notice)) = completions.batch() {
            let Some(thread) = thread.upgrade() else {
                break;
            };
            let result = thread
                .start_turn_if_idle(
                    TurnInputRequest::new(TurnInput::ResponseItem(notice)).on_start(
                        TurnStartOptions {
                            turn_trigger: Some(trigger),
                            ..Default::default()
                        },
                    ),
                )
                .await;
            completions.state().batch = None;
            if matches!(result, Ok(StartIfIdleSubmission::Started { .. })) {
                continue;
            }
            if let Err(error) = result {
                tracing::warn!(%error, "failed to start background completion turn");
            }
        }
        tokio::select! {
            _ = completions.shutdown.cancelled() => break,
            changed = activity.changed() => if changed.is_err() { break },
        }
    }
}
