//! Shared bounded, identity-only Cell/command wake bookkeeping. No result ownership.
//! Kept with the existing service so both producers use its live-thread activation,
//! suppression and idle admission; command-specific bookkeeping lives in `commands`.
mod command_wait;
mod commands;
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

pub(crate) use commands::CommandCompletion;
pub(super) use delegate::CompletionDelegate;
pub(crate) use dispatcher::CompletionAdmission;

const CAPACITY: usize = 64;
const ID_BYTES: usize = 128;
/// Marks an error already suppressed at observation time, before a later user continuation.
pub(crate) struct CompletionErrorObserved;

pub(crate) const TRIGGER: &str = "code_mode_completion:";
pub(super) const GUIDANCE: &str = "\nWhen this cell actually completes, this interactive thread can resume automatically once idle. You may end this turn while it runs; use wait to retrieve the result when notified. Subsequent tools that need no new approval can run while idle. Approval, user input and notify may wait for a new active turn. Other automatic continuations are unchanged.";

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
    command: Option<commands::Command>,
}

impl State {
    fn eligible(&self, key: &Uuid) -> bool {
        self.entries.get(key).is_some_and(|entry| {
            entry.eligible()
                && entry.command.as_ref().is_none_or(|command| {
                    command.owner.is_none_or(|owner| {
                        command.owner_finished && !self.entries.contains_key(&owner)
                    })
                })
        })
    }
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
    pub(crate) fn is_enabled(&self) -> bool {
        self.state().enabled && !self.shutdown.is_cancelled()
    }

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
        self.reserve_locked(&mut self.state(), call_id, turn_id)
    }

    fn reserve_locked(
        self: &Arc<Self>,
        state: &mut State,
        call_id: &str,
        turn_id: &str,
    ) -> Result<Option<Reservation>, String> {
        if !state.enabled {
            return Ok(None);
        }
        if self.shutdown.is_cancelled() {
            return Err("Background completion monitoring is shutting down".to_string());
        }
        if state.entries.len() == CAPACITY {
            return Err("Background completion monitoring is full (64 operations). Retrieve or terminate an existing cell or command before starting another.".to_string());
        }
        if call_id.len() > ID_BYTES || turn_id.len() > ID_BYTES {
            return Err(
                "Background execution identity exceeds the completion monitoring limit".to_string(),
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
                command: None,
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
            let (call_id, output, custom) = match item {
                ResponseItem::CustomToolCallOutput {
                    call_id, output, ..
                } => (call_id, output, true),
                ResponseItem::FunctionCallOutput {
                    call_id: Some(call_id),
                    output,
                    ..
                } => (call_id, output, false),
                _ => continue,
            };
            state.entries.retain(|_, entry| {
                if entry.turn_id != turn_id || &entry.call_id != call_id {
                    return true;
                }
                if let Some(command) = &entry.command {
                    let exposed = output.success != Some(false)
                        && if command.owner.is_some() {
                            custom
                        } else {
                            entry.yielded && !custom && output.success == Some(true)
                        };
                    entry.exposed |= exposed;
                    return exposed;
                }
                if !custom {
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
                batch.trigger == trigger && batch.executions.iter().all(|key| state.eligible(key))
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
            if notice
                .render()
                .into_text()
                .is_none_or(|text| text.len() > 512)
            {
                notice.0.pop();
                break;
            }
            executions.push(*key);
            if executions.len() == 16 {
                break;
            }
        }
        if executions.is_empty() {
            return self.command_batch(&mut state);
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
pub(crate) struct Reservation {
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

    pub(crate) fn yielded(&mut self) {
        if let Some(entry) = self.completions.state().entries.get_mut(&self.execution) {
            entry.yielded = true;
        }
        self.handed_off = true;
        self.completions.activity();
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
