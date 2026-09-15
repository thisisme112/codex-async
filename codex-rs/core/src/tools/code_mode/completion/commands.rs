use super::Batch;
use super::Completions;
use super::Reservation;
use super::State;
use super::TRIGGER;
use crate::context::ContextualUserFragment;
use crate::context::ExecCompletion;
use codex_code_mode::CellId;
use codex_protocol::models::ResponseItem;
use std::sync::Arc;
use std::sync::Weak;
use uuid::Uuid;

pub(super) struct Command {
    pub(super) process_id: i32,
    // An awaited command belongs to its Cell. After that Cell finishes and its
    // notification retires, an unobserved yielded command can notify on its own.
    pub(super) owner: Option<Uuid>,
    pub(super) owner_finished: bool,
}

pub(crate) struct CommandCompletion {
    completions: Weak<Completions>,
    execution: Uuid,
}
impl CommandCompletion {
    pub(crate) fn ready(&self) {
        if let Some(registry) = self.completions.upgrade() {
            if let Some(entry) = registry.state().entries.get_mut(&self.execution) {
                entry.ready = true;
            }
            registry.activity();
        }
    }
    pub(crate) fn retire(&self) {
        if let Some(registry) = self.completions.upgrade() {
            registry.retire(self.execution);
        }
    }
}
impl Reservation {
    pub(crate) fn command_handle(&self) -> CommandCompletion {
        CommandCompletion {
            completions: Arc::downgrade(&self.completions),
            execution: self.execution,
        }
    }
}
impl Completions {
    pub(super) fn owner_finished(&self, owner: Uuid) {
        for entry in self.state().entries.values_mut() {
            if let Some(command) = &mut entry.command
                && command.owner == Some(owner)
            {
                command.owner_finished = true;
            }
        }
        self.activity();
    }

    pub(in crate::tools::code_mode) fn cancel_cell_commands(&self, cell: &CellId) {
        let mut state = self.state();
        let owner = state
            .entries
            .iter()
            .find(|(_, entry)| entry.cell_id.as_ref() == Some(cell))
            .map(|(key, _)| *key);
        if let Some(owner) = owner {
            state.entries.retain(|_, entry| {
                entry
                    .command
                    .as_ref()
                    .is_none_or(|command| command.owner != Some(owner))
            });
        }
        drop(state);
        self.activity();
    }

    pub(crate) fn reserve_command(
        self: &Arc<Self>,
        call_id: &str,
        turn_id: &str,
        process_id: i32,
        cell: Option<&CellId>,
    ) -> Result<Option<Reservation>, String> {
        let mut state = self.state();
        let owner = if let Some(cell) = cell {
            let Some((key, entry)) = state
                .entries
                .iter()
                .find(|(_, e)| e.cell_id.as_ref() == Some(cell))
            else {
                // An untracked provider has no trustworthy Cell ownership boundary.
                return Ok(None);
            };
            Some((
                *key,
                entry.call_id.clone(),
                entry.turn_id.clone(),
                entry.exposed,
                entry.ready,
            ))
        } else {
            None
        };
        let Some(reservation) = self.reserve_locked(&mut state, call_id, turn_id)? else {
            return Ok(None);
        };
        let Some(entry) = state.entries.get_mut(&reservation.execution) else {
            // Release the lock before the reservation's cleanup can acquire it.
            drop(state);
            return Err("Background completion reservation is unavailable".to_string());
        };
        entry.command = Some(Command {
            process_id,
            owner: owner.as_ref().map(|(id, ..)| *id),
            owner_finished: owner.as_ref().is_some_and(|(_, _, _, _, ready)| *ready),
        });
        entry.supported = true;
        if let Some((_, call_id, turn_id, exposed, _)) = owner {
            entry.call_id = call_id;
            entry.turn_id = turn_id;
            entry.exposed = exposed;
        }
        Ok(Some(reservation))
    }
    pub(super) fn command_batch(&self, state: &mut State) -> Option<(String, ResponseItem)> {
        let mut notice = ExecCompletion(Vec::new());
        let mut executions = Vec::new();
        for (key, entry) in &state.entries {
            if !state.eligible(key) {
                continue;
            }
            let Some(command) = &entry.command else {
                continue;
            };
            notice.0.push(command.process_id);
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

#[cfg(test)]
#[path = "commands_tests.rs"]
pub(super) mod tests;
