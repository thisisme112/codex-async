use std::sync::Arc;

use codex_code_mode::CellId;
use codex_code_mode::CodeModeNestedToolCall;
use codex_code_mode::CodeModeSessionDelegate;
use codex_code_mode::NotificationFuture;
use codex_code_mode::ToolInvocationFuture;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use super::Completions;
use super::Reservation;
use super::valid_cell_id;

pub(in crate::tools::code_mode) struct CompletionDelegate {
    inner: Arc<dyn CodeModeSessionDelegate>,
    completions: Arc<Completions>,
    execution: Uuid,
}

impl CompletionDelegate {
    pub(in crate::tools::code_mode) fn wrap(
        inner: Arc<dyn CodeModeSessionDelegate>,
        reservation: &Reservation,
    ) -> Arc<dyn CodeModeSessionDelegate> {
        Arc::new(Self {
            inner,
            completions: Arc::clone(&reservation.completions),
            execution: reservation.execution,
        })
    }
}

impl CodeModeSessionDelegate for CompletionDelegate {
    fn invoke_tool<'a>(
        &'a self,
        invocation: CodeModeNestedToolCall,
        cancellation_token: CancellationToken,
    ) -> ToolInvocationFuture<'a> {
        self.inner.invoke_tool(invocation, cancellation_token)
    }

    fn notify<'a>(
        &'a self,
        call_id: String,
        cell_id: CellId,
        text: String,
        cancellation_token: CancellationToken,
    ) -> NotificationFuture<'a> {
        self.inner
            .notify(call_id, cell_id, text, cancellation_token)
    }

    fn cell_closed(&self, cell_id: &CellId) {
        self.completions.owner_finished(self.execution);
        self.completions.retire(self.execution);
        self.inner.cell_closed(cell_id);
    }

    fn wants_completion_ready(&self) -> bool {
        true
    }

    fn cell_completion_ready(&self, cell_id: &CellId) {
        let mut state = self.completions.state();
        if let Some(entry) = state.entries.get_mut(&self.execution) {
            if !valid_cell_id(cell_id) || entry.cell_id.as_ref().is_some_and(|id| id != cell_id) {
                state.entries.remove(&self.execution);
            } else {
                entry.cell_id = Some(cell_id.clone());
                entry.ready = true;
            }
        }
        drop(state);
        self.completions.owner_finished(self.execution);
    }
}

impl Drop for CompletionDelegate {
    fn drop(&mut self) {
        // Provider disconnect/replacement must not leave a candidate behind.
        self.completions.owner_finished(self.execution);
        self.completions.retire(self.execution);
    }
}
