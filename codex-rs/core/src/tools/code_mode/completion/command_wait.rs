//! Event-driven waits for exact command executions. No result ownership or timers.
use super::CAPACITY;
use super::Completions;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

impl Completions {
    /// Waits for all requested executions without draining output or inferring.
    /// Capture UUIDs once so numeric session ID reuse cannot satisfy an old wait.
    pub(crate) async fn wait_for_commands(
        &self,
        process_ids: &[i32],
        cancellation: &CancellationToken,
    ) -> Result<(), String> {
        if process_ids.is_empty() || process_ids.len() > CAPACITY {
            return Err("wait_for_tasks requires 1 to 64 session IDs".to_string());
        }
        let activity = self.subscribe();
        let executions = {
            let state = self.state();
            let mut executions = Vec::with_capacity(process_ids.len());
            for process_id in process_ids {
                if executions.iter().any(|(id, _)| id == process_id) {
                    return Err("wait_for_tasks requires distinct session IDs".to_string());
                }
                let Some((key, _)) = state.entries.iter().find(|(_, entry)| {
                    entry.yielded
                        && entry
                            .command
                            .as_ref()
                            .is_some_and(|c| c.process_id == *process_id)
                }) else {
                    return Err(format!(
                        "Session {process_id} has no active completion subscription. If already notified, retrieve its result with write_stdin; otherwise use normal terminal interaction."
                    ));
                };
                executions.push((*process_id, *key));
            }
            executions
        };
        self.wait_for_executions(&executions, activity, cancellation)
            .await
    }

    async fn wait_for_executions(
        &self,
        executions: &[(i32, Uuid)],
        mut activity: tokio::sync::watch::Receiver<()>,
        cancellation: &CancellationToken,
    ) -> Result<(), String> {
        loop {
            // Subscribe before inspection and mark before scanning: changes in
            // between inspection and changed().await must not be lost.
            activity.borrow_and_update();
            if cancellation.is_cancelled() || self.shutdown.is_cancelled() {
                return Err(
                    "Task wait interrupted; command output has not been consumed".to_string(),
                );
            }
            {
                let state = self.state();
                if state.suppressed {
                    return Err("Task wait interrupted".to_string());
                }
                let mut all_ready = true;
                for (id, key) in executions {
                    let Some(entry) = state.entries.get(key) else {
                        return Err(format!(
                            "Completion subscription for session {id} was retired; retrieve any remaining terminal result with write_stdin"
                        ));
                    };
                    all_ready &= entry.ready;
                }
                if all_ready {
                    return Ok(());
                }
            }
            tokio::select! {
                _ = cancellation.cancelled() => {},
                _ = self.shutdown.cancelled() => {},
                changed = activity.changed() => {
                    if changed.is_err() {
                        return Err("Completion event stream closed".to_string());
                    }
                }
            }
        }
    }
}

#[cfg(test)]
#[path = "command_wait_tests.rs"]
mod tests;
