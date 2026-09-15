use super::CellDelegate;
use super::SessionState;
use codex_code_mode_protocol::grpc;
use std::sync::Arc;

impl SessionState {
    pub(in crate::grpc_session) fn completion_ready(
        &mut self,
        ready: grpc::CellCompletionReady,
    ) -> Result<Option<CellDelegate>, String> {
        self.require_open()?;
        self.check_cell_ownership(&ready.execution_id, &ready.cell_id)?;
        let Some(execution) = self.executions.get_mut(&ready.execution_id) else {
            // Retrieval/cancellation may have retired the execution on another stream.
            return Ok(None);
        };
        execution.accept_cell(&ready.cell_id)?;
        if execution.closed || !execution.completion_ready || execution.completion_notified {
            return Ok(None);
        }
        execution.completion_notified = true;
        Ok(execution
            .cell_id
            .clone()
            .map(|cell_id| (cell_id, Arc::clone(&execution.delegate))))
    }
}
