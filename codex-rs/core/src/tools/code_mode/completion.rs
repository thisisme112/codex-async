//! Bounded, identity-only wake bookkeeping. Runtime readiness never owns a result.
mod delegate;
mod dispatcher;

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::MutexGuard;

use codex_code_mode::CellId;
use codex_code_mode::CompletionReadySupport;
use codex_protocol::models::ResponseItem;
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::context::CodeModeCompletion;
use crate::context::ContextualUserFragment;

pub(super) use delegate::CompletionDelegate;
pub(crate) use dispatcher::CompletionAdmission;

const CAPACITY: usize = 64;
const ID_BYTES: usize = 128;
/// Marks an error already suppressed at observation time, before a later user continuation.
pub(crate) struct CompletionErrorObserved;

pub(crate) const TRIGGER: &str = "code_mode_completion:";
pub(super) const GUIDANCE: &str = "\nWhen this cell actually completes, this interactive thread can resume automatically once idle. You may end this turn while it runs; use wait to retrieve the result when notified. Additional tool calls, approval, or notify inside an idle cell may still wait for an active turn. Other automatic continuations are unchanged.";

#[derive(Default)]
struct State {
    enabled: bool,
    suppressed: bool,
    entries: BTreeMap<Uuid, Entry>,
    batch: Option<Batch>,
}

struct Entry {
    call_id: String,
    turn_id: String,
    cell_id: Option<CellId>,
    supported: bool,
    ready: bool,
    yielded: bool,
    exposed: bool,
}

impl Entry {
    fn eligible(&self) -> bool {
        self.supported && self.ready && self.yielded && self.exposed
    }
}

#[derive(Clone)]
struct Batch {
    trigger: String,
    executions: Vec<Uuid>,
}

pub(crate) struct Completions {
    state: Mutex<State>,
    activity: watch::Sender<()>,
    shutdown: CancellationToken,
}

impl Default for Completions {
    fn default() -> Self {
        Self {
            state: Mutex::new(State::default()),
            activity: watch::channel(()).0,
            shutdown: CancellationToken::new(),
        }
    }
}

impl Completions {
    fn state(&self) -> MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub(crate) fn subscribe(&self) -> watch::Receiver<()> {
        self.activity.subscribe()
    }

    pub(crate) fn activity(&self) {
        self.activity.send_replace(());
    }

    pub(super) fn reserve(
        self: &Arc<Self>,
        call_id: &str,
        turn_id: &str,
    ) -> Result<Option<Reservation>, String> {
        let mut state = self.state();
        if !state.enabled {
            return Ok(None);
        }
        if self.shutdown.is_cancelled() {
            return Err("Code Mode completion monitoring is shutting down".to_string());
        }
        if state.entries.len() == CAPACITY {
            return Err("Code Mode completion monitoring is full (64 cells). Retrieve or terminate an existing cell before starting another.".to_string());
        }
        if call_id.len() > ID_BYTES || turn_id.len() > ID_BYTES {
            return Err(
                "Code Mode execution identity exceeds the completion monitoring limit".to_string(),
            );
        }
        let execution = Uuid::new_v4();
        state.entries.insert(
            execution,
            Entry {
                call_id: call_id.to_string(),
                turn_id: turn_id.to_string(),
                cell_id: None,
                supported: false,
                ready: false,
                yielded: false,
                exposed: false,
            },
        );
        Ok(Some(Reservation {
            completions: Arc::clone(self),
            execution,
            handed_off: false,
        }))
    }

    fn retire(&self, execution: Uuid) {
        self.state().entries.remove(&execution);
        self.activity();
    }

    pub(super) fn retire_cell(&self, cell_id: &CellId) {
        self.state()
            .entries
            .retain(|_, entry| entry.cell_id.as_ref() != Some(cell_id));
        self.activity();
    }

    /// Called only after the actual output is recorded in the thread's history.
    /// Failure/replacement outputs cannot acknowledge a yielded cell.
    pub(crate) fn record_outputs(&self, turn_id: &str, items: &[ResponseItem]) {
        let mut state = self.state();
        for item in items {
            let ResponseItem::CustomToolCallOutput {
                call_id, output, ..
            } = item
            else {
                continue;
            };
            state.entries.retain(|_, entry| {
                if entry.turn_id != turn_id || &entry.call_id != call_id {
                    return true;
                }
                let Some(cell_id) = &entry.cell_id else {
                    return false;
                };
                let prefix = format!("Script running with cell ID {cell_id}\n");
                let exposed = entry.yielded
                    && output.success != Some(false)
                    && output
                        .body
                        .to_text()
                        .is_some_and(|text| text.starts_with(&prefix));
                entry.exposed |= exposed;
                exposed
            });
        }
        drop(state);
        self.activity();
    }

    pub(crate) fn finish_turn(&self, turn_id: &str) {
        self.state()
            .entries
            .retain(|_, entry| entry.turn_id != turn_id || entry.exposed);
        self.activity();
    }

    /// Call under Core's active-turn lock before interruption/error releases admission.
    pub(crate) fn suppress(&self) {
        self.state().suppressed = true;
        self.activity();
    }

    pub(crate) fn resume(&self) {
        self.state().suppressed = false;
        self.activity();
    }

    pub(crate) fn shutdown(&self) {
        self.shutdown.cancel();
        self.state().entries.clear();
    }

    pub(crate) fn permits(&self, trigger: &str) -> bool {
        let state = self.state();
        !self.shutdown.is_cancelled()
            && !state.suppressed
            && state.batch.as_ref().is_some_and(|batch| {
                batch.trigger == trigger
                    && batch
                        .executions
                        .iter()
                        .all(|key| state.entries.get(key).is_some_and(Entry::eligible))
            })
    }

    /// Commit at the task installation boundary, under the same active-turn lock
    /// as interruption. Late duplicate callbacks cannot recreate retired entries.
    pub(crate) fn delivered(&self) {
        let mut state = self.state();
        if let Some(batch) = state.batch.take() {
            for key in batch.executions {
                state.entries.remove(&key);
            }
        }
    }

    fn batch(&self) -> Option<(String, ResponseItem)> {
        let mut state = self.state();
        if state.suppressed || self.shutdown.is_cancelled() {
            return None;
        }
        let mut notice = CodeModeCompletion(Vec::new());
        let mut executions = Vec::new();
        for (key, entry) in &state.entries {
            if !entry.eligible() {
                continue;
            }
            let Some(cell_id) = &entry.cell_id else {
                continue;
            };
            notice.0.push(cell_id.clone());
            // Conservative hard cap: ASCII text cannot exceed one token per byte.
            if notice.render().len() > 512 {
                notice.0.pop();
                break;
            }
            executions.push(*key);
            if executions.len() == 16 {
                break;
            }
        }
        if executions.is_empty() {
            return None;
        }
        let trigger = format!("{TRIGGER}{}", Uuid::new_v4());
        state.batch = Some(Batch {
            trigger: trigger.clone(),
            executions,
        });
        Some((trigger, ContextualUserFragment::into(notice)))
    }
}

/// Drops a reservation if execution or the initial response handoff fails.
/// Successful yielded outputs transfer ownership to the bounded registry until
/// history acknowledges them, or the originating turn finishes without exposure.
pub(super) struct Reservation {
    completions: Arc<Completions>,
    execution: Uuid,
    handed_off: bool,
}

impl Reservation {
    pub(super) fn bind(&self, cell_id: &CellId, support: CompletionReadySupport) -> bool {
        let mut state = self.completions.state();
        let Some(entry) = state.entries.get_mut(&self.execution) else {
            return false;
        };
        if support != CompletionReadySupport::Supported
            || !valid_cell_id(cell_id)
            || entry.cell_id.as_ref().is_some_and(|id| id != cell_id)
        {
            state.entries.remove(&self.execution);
            return false;
        }
        entry.cell_id = Some(cell_id.clone());
        entry.supported = true;
        true
    }

    pub(super) fn yielded(&mut self) {
        if let Some(entry) = self.completions.state().entries.get_mut(&self.execution) {
            entry.yielded = true;
        }
        self.handed_off = true;
    }
}

impl Drop for Reservation {
    fn drop(&mut self) {
        if !self.handed_off {
            self.completions.retire(self.execution);
        }
    }
}

fn valid_cell_id(id: &CellId) -> bool {
    !id.as_str().is_empty()
        && id.as_str().len() <= ID_BYTES
        && id
            .as_str()
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-_.:".contains(&byte))
}

#[cfg(test)]
#[path = "completion_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "completion_admission_tests.rs"]
mod admission_tests;
