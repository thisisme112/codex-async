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
            HashMap::from([("key".to_string(), serde_json::json!(7))])
        );
        assert!(self.state.accepting_observations());
        self.ready.fetch_add(/*val*/ 1, Ordering::SeqCst);
    }
}

#[tokio::test]
async fn commit_notifies_without_observer_and_preserves_the_initial_yield_handoff() {
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
        content_items: vec![],
        error_text: Some("script error".to_string()),
    };
    assert_eq!(
        host.commit_completion(
            HashMap::from([("key".to_string(), serde_json::json!(7))]),
            result.clone(),
            Some(vec![]),
            Arc::clone(&state),
        )
        .await,
        CompletionCommit::Committed
    );
    assert_eq!(probe.ready.load(Ordering::SeqCst), 1);
    // A duplicate commit cannot emit a duplicate notification.
    assert_eq!(
        host.commit_completion(
            HashMap::new(),
            result.clone(),
            /*pending_initial_yield_items*/ None,
            Arc::clone(&state)
        )
        .await,
        CompletionCommit::Rejected(result.clone())
    );
    for expected in [
        CellEvent::Yielded {
            content_items: vec![],
        },
        result,
    ] {
        let (sender, receiver) = tokio::sync::oneshot::channel();
        state.route_observation(ObserveMode::YieldAfter(std::time::Duration::ZERO), sender);
        assert_eq!(receiver.await.unwrap(), Ok(expected));
    }
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
        content_items: vec![],
        error_text: None,
    };
    let store = runtime.inner.stored_values.lock().await;
    let commit = host.commit_completion(
        HashMap::new(),
        result.clone(),
        /*pending_initial_yield_items*/ None,
        state,
    );
    tokio::pin!(commit);
    let mut context = std::task::Context::from_waker(std::task::Waker::noop());
    assert!(commit.as_mut().poll(&mut context).is_pending());
    runtime.begin_shutdown();
    drop(store);
    assert_eq!(commit.await, CompletionCommit::Rejected(result));
    host.closed().await;
    assert_eq!(probe.ready.load(Ordering::SeqCst), 0);
}
