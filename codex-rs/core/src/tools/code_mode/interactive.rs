//! Operation-scoped prompt deferral; no new response owner or active turn.
use crate::session::session::Session;
use crate::session::step_settings::ResolvedStepSettings;
use crate::session::turn_context::TurnContext;
use crate::state::ActiveTurn;
use crate::tools::context::ToolCallSource;
use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use tokio::sync::MutexGuard;

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum ToolPromptOrigin {
    #[default]
    Direct,
    Cell,
}

tokio::task_local! {
    static ORIGIN: ToolPromptOrigin;
    static ADMISSION: Arc<ReviewAdmission>;
}

struct ReviewAdmission {
    turn: Arc<TurnContext>,
    registered: AtomicBool,
    withdrawn: AtomicBool,
}

impl ToolPromptOrigin {
    pub(crate) fn from_source(source: &ToolCallSource) -> Self {
        match source {
            ToolCallSource::CodeMode { .. } => Self::Cell,
            ToolCallSource::Direct | ToolCallSource::DirectPlaintextMessage => Self::Direct,
        }
    }

    // Detached callbacks capture this value at construction, never from a shared turn.
    pub(crate) fn current() -> Self {
        ORIGIN.try_with(|origin| *origin).unwrap_or_default()
    }

    pub(crate) async fn scope<T>(self, future: impl Future<Output = T>) -> T {
        ORIGIN.scope(self, future).await
    }
}

impl Session {
    pub(crate) async fn turn_for_tool_prompt(&self) -> Option<MutexGuard<'_, Option<ActiveTurn>>> {
        if ToolPromptOrigin::current() == ToolPromptOrigin::Cell
            && let Ok(admission) = ADMISSION.try_with(Arc::clone)
        {
            let active = self.active_turn.lock().await;
            if self
                .services
                .code_mode_service
                .shutdown_token
                .is_cancelled()
            {
                return None;
            }
            if !active
                .as_ref()
                .and_then(|turn| turn.task.as_ref())
                .is_some_and(|task| Arc::ptr_eq(&task.turn_context, &admission.turn))
            {
                admission.withdrawn.store(true, Ordering::Release);
                return None;
            }
            admission.registered.store(true, Ordering::Release);
            return Some(active);
        }
        let active = self
            .turn_for_prompt_origin(ToolPromptOrigin::current())
            .await?;
        // Never emit a prompt and await a sender still owned by this stack frame.
        active.is_some().then_some(active)
    }

    pub(crate) async fn turn_for_prompt_origin(
        &self,
        origin: ToolPromptOrigin,
    ) -> Option<MutexGuard<'_, Option<ActiveTurn>>> {
        let service = &self.services.code_mode_service;
        let mut activity = service.completions.subscribe();
        loop {
            activity.borrow_and_update();
            {
                let active = self.active_turn.lock().await;
                if origin == ToolPromptOrigin::Direct {
                    return Some(active);
                }
                if service.shutdown_token.is_cancelled() {
                    return None;
                }
                if active.as_ref().is_some_and(|turn| turn.task.is_some()) {
                    return Some(active);
                }
            }
            tokio::select! {
                _ = service.shutdown_token.cancelled() => return None,
                _ = activity.changed() => {},
            }
        }
    }

    /// Retry only before admission. Hooks/review never hold the active-turn lock;
    /// each retry selects current settings, while callers keep execution ownership.
    pub(crate) async fn with_current_tool_review<T, F, R>(
        &self,
        origin: ToolPromptOrigin,
        mut review: F,
    ) -> Option<T>
    where
        F: FnMut(Arc<TurnContext>, Arc<ResolvedStepSettings>, bool) -> R,
        R: Future<Output = T>,
    {
        loop {
            let (turn, settings, strict) = self.review_context_for_cell_prompt(origin).await?;
            if origin == ToolPromptOrigin::Direct {
                return Some(origin.scope(review(turn, settings, strict)).await);
            }
            let admission = Arc::new(ReviewAdmission {
                turn: turn.clone(),
                registered: AtomicBool::new(false),
                withdrawn: AtomicBool::new(false),
            });
            let result = self
                .services
                .code_mode_service
                .shutdown_token
                .run_until_cancelled(
                    origin
                        .scope(ADMISSION.scope(admission.clone(), review(turn, settings, strict))),
                )
                .await?;
            if admission.registered.load(Ordering::Acquire) {
                return Some(result);
            }
            // Hook/Guardian decisions that bypass a user prompt also require the
            // selected task to still exist. No retry after a registered prompt.
            {
                let active = self.active_turn.lock().await;
                if !admission.withdrawn.load(Ordering::Acquire)
                    && active
                        .as_ref()
                        .and_then(|turn| turn.task.as_ref())
                        .is_some_and(|task| Arc::ptr_eq(&task.turn_context, &admission.turn))
                {
                    return Some(result);
                }
            }
        }
    }
}

#[cfg(test)]
#[path = "interactive_tests.rs"]
mod tests;
