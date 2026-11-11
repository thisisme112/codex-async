use super::*;
use pretty_assertions::assert_eq;
use std::sync::atomic::AtomicUsize;

struct Probe {
    inner: Arc<Inner>,
    state: Arc<CellState>,
    ready: AtomicUsize,
}

impl SessionRuntimeDelegate for Probe {
    async fn invoke_tool(
        &self,
        _: NestedToolCall,
        _: CancellationToken,
    ) -> Result<JsonValue, String> {
        unreachable!()
    }

    async fn notify(
        &self,
        _: String,
        _: CellId,
        _: String,
        _: CancellationToken,
    ) -> Result<(), String> {
        unreachable!()
    }

    fn cell_closed(&self, _: &CellId) {}

    fn cell_completion_ready(&self, _: &CellId) {
        // Neither the store lock nor the phase lock is held during notification.
        assert_eq!(
            *self.inner.stored_values.try_lock().unwrap(),
            HashMap::from([("key".to_string(), Arc::new(serde_json::json!(7)))])
        );
        assert!(self.state.accepting_observations());
        self.ready.fetch_add(/*val*/ 1, Ordering::SeqCst);
    }
}

#[tokio::test]
async fn commit_notifies_without_observer_and_preserves_terminal_result() {
    let runtime = SessionRuntime::new();
    let state = Arc::new(CellState::new(CancellationToken::new()));
    let probe = Arc::new(Probe {
        inner: Arc::clone(&runtime.inner),
        state: Arc::clone(&state),
        ready: AtomicUsize::new(/*v*/ 0),
    });
    let host = RuntimeCellHost {
        delegate: Arc::clone(&probe),
        cell_id: CellId::new("1"),
        inner: Arc::clone(&runtime.inner),
        execution_context: opentelemetry::Context::new(),
    };
    let result = CellEvent::Completed {
        content_items: crate::output::OutputCollector::Client(Vec::new()),
        error_text: Some("script error".to_string()),
        result_buffered_for: std::time::Duration::ZERO,
    };
    assert_eq!(
        host.commit_completion(
            HashMap::from([("key".to_string(), Arc::new(serde_json::json!(7)))]),
            || result.clone(),
            Arc::clone(&state),
        )
        .await,
        CompletionCommit::Committed
    );
    assert_eq!(probe.ready.load(Ordering::SeqCst), 1);
    // A duplicate commit cannot emit a duplicate notification.
    assert_eq!(
        host.commit_completion(HashMap::new(), || result.clone(), Arc::clone(&state))
            .await,
        CompletionCommit::Rejected
    );
    let (sender, receiver) = tokio::sync::oneshot::channel();
    state.route_observation(sender);
    let event = receiver.await.unwrap().unwrap();
    let CellEvent::Completed {
        content_items,
        error_text,
        ..
    } = event
    else {
        panic!("completion result was consumed by the notification");
    };
    assert_eq!(
        content_items,
        crate::output::OutputCollector::Client(Vec::new())
    );
    assert_eq!(error_text, Some("script error".to_string()));
    host.closed().await;
    assert_eq!(probe.ready.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn cancellation_while_waiting_for_store_commit_does_not_notify() {
    let runtime = SessionRuntime::new();
    let state = Arc::new(CellState::new(runtime.inner.shutdown_token.child_token()));
    let probe = Arc::new(Probe {
        inner: Arc::clone(&runtime.inner),
        state: Arc::clone(&state),
        ready: AtomicUsize::new(/*v*/ 0),
    });
    let host = RuntimeCellHost {
        delegate: Arc::clone(&probe),
        cell_id: CellId::new("1"),
        inner: Arc::clone(&runtime.inner),
        execution_context: opentelemetry::Context::new(),
    };
    let result = CellEvent::Completed {
        content_items: crate::output::OutputCollector::Client(Vec::new()),
        error_text: None,
        result_buffered_for: std::time::Duration::ZERO,
    };
    let store = runtime.inner.stored_values.lock().await;
    let commit = host.commit_completion(HashMap::new(), || result.clone(), state);
    tokio::pin!(commit);
    let mut context = std::task::Context::from_waker(std::task::Waker::noop());
    assert!(commit.as_mut().poll(&mut context).is_pending());
    runtime.begin_shutdown();
    drop(store);
    assert_eq!(commit.await, CompletionCommit::Rejected);
    host.closed().await;
    assert_eq!(probe.ready.load(Ordering::SeqCst), 0);
}
