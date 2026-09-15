//! Sampling owns reception while active; unfinished executions extend it through idle.
//! Only handles/counts live here. Results, Cell gates and wake eligibility stay separate.
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::PoisonError;

use super::delegate::CodeModeDispatchWorker;

#[derive(Default)]
pub(super) struct Workers(Mutex<State>);

#[derive(Default)]
struct State {
    generation: u64,
    active: bool,
    closed: bool,
    executions: usize,
    worker: Option<CodeModeDispatchWorker>,
}

impl Workers {
    pub(super) fn has_executions(&self) -> bool {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .executions
            != 0
    }

    pub(super) fn replace(self: &Arc<Self>, worker: CodeModeDispatchWorker) -> WorkerScope {
        let mut state = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        state.worker.take();
        state.generation += 1;
        state.active = !state.closed;
        if !state.closed {
            state.worker = Some(worker);
        }
        WorkerScope {
            workers: self.clone(),
            generation: state.generation,
            succeeded: false,
        }
    }

    pub(super) fn stop(&self) {
        let mut state = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        state.generation += 1;
        state.active = false;
        state.worker.take();
    }

    pub(super) fn shutdown(&self) {
        let mut state = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        state.closed = true;
        state.active = false;
        state.worker.take();
    }

    pub(super) fn execution(self: &Arc<Self>) -> ExecutionStart {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .executions += 1;
        ExecutionStart(Some(Arc::new(Execution(Mutex::new(Some(self.clone()))))))
    }
}

pub(crate) struct WorkerScope {
    workers: Arc<Workers>,
    generation: u64,
    succeeded: bool,
}

impl WorkerScope {
    pub(crate) fn succeeded(&mut self) {
        self.succeeded = true;
    }
}

impl Drop for WorkerScope {
    fn drop(&mut self) {
        let mut state = self
            .workers
            .0
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if state.generation == self.generation {
            state.active = false;
            if !self.succeeded || state.executions == 0 {
                state.worker.take();
            }
        }
    }
}

/// Covers execute failure/future cancellation even if a provider retained its delegate.
pub(super) struct ExecutionStart(pub(super) Option<Arc<Execution>>);
impl Drop for ExecutionStart {
    fn drop(&mut self) {
        if let Some(execution) = &self.0 {
            execution.finish();
        }
    }
}

pub(super) struct Execution(Mutex<Option<Arc<Workers>>>);
impl Execution {
    pub(super) fn finish(&self) {
        if let Some(workers) = self.0.lock().unwrap_or_else(PoisonError::into_inner).take() {
            let mut state = workers.0.lock().unwrap_or_else(PoisonError::into_inner);
            state.executions -= 1;
            if !state.active && state.executions == 0 {
                state.worker.take();
            }
        }
    }
}
impl Drop for Execution {
    fn drop(&mut self) {
        self.finish();
    }
}

#[cfg(test)]
#[path = "worker_lifecycle_tests.rs"]
mod tests;
