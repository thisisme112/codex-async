use super::*;
use pretty_assertions::assert_eq;
use tokio::sync::oneshot;

fn worker() -> (CodeModeDispatchWorker, oneshot::Receiver<()>) {
    let (tx, rx) = oneshot::channel();
    (
        CodeModeDispatchWorker {
            shutdown_tx: Some(tx),
        },
        rx,
    )
}

#[test]
fn multiple_executions_commit_once_without_result_retrieval() {
    let workers = Arc::new(Workers::default());
    let (handle, mut stopped) = worker();
    let mut scope = workers.replace(handle);
    let mut first = workers.execution();
    let first = first.0.take().unwrap();
    let mut second = workers.execution();
    let second = second.0.take().unwrap();
    scope.succeeded();
    drop(scope);
    first.finish();
    first.finish();
    drop(first);
    assert_eq!(stopped.try_recv(), Err(oneshot::error::TryRecvError::Empty));
    second.finish();
    assert_eq!(stopped.try_recv(), Ok(()));
    assert!(!workers.has_executions());
}

#[test]
fn early_commit_and_failed_execute_do_not_retain_worker() {
    let workers = Arc::new(Workers::default());
    let (handle, mut stopped) = worker();
    let mut scope = workers.replace(handle);
    let early = workers.execution();
    early.0.as_ref().unwrap().finish();
    drop(early);
    // Provider kept its delegate after execute errored/cancelled.
    let failed = workers.execution();
    let retained_delegate = failed.0.as_ref().unwrap().clone();
    drop(failed);
    assert!(!workers.has_executions());
    assert_eq!(stopped.try_recv(), Err(oneshot::error::TryRecvError::Empty));
    scope.succeeded();
    drop(scope);
    assert_eq!(stopped.try_recv(), Ok(()));
    drop(retained_delegate);
}

#[test]
fn replacement_old_scope_drop_interrupt_and_shutdown() {
    let workers = Arc::new(Workers::default());
    let execution = workers.execution();
    let (handle, mut old_stopped) = worker();
    let old_scope = workers.replace(handle);
    let (handle, mut stopped) = worker();
    let mut scope = workers.replace(handle);
    assert_eq!(old_stopped.try_recv(), Ok(()));
    drop(old_scope); // An old failure must not stop the replacement.
    assert_eq!(stopped.try_recv(), Err(oneshot::error::TryRecvError::Empty));
    workers.stop();
    scope.succeeded();
    drop(scope);
    assert_eq!(stopped.try_recv(), Ok(()));
    workers.shutdown();
    let (handle, mut stopped) = worker();
    drop(workers.replace(handle));
    assert_eq!(stopped.try_recv(), Ok(()));
    drop(execution);
    assert!(!workers.has_executions());
}

#[test]
fn empty_success_and_failed_sampling_stop_reception() {
    let workers = Arc::new(Workers::default());
    let (handle, mut stopped) = worker();
    let mut scope = workers.replace(handle);
    scope.succeeded();
    drop(scope);
    assert_eq!(stopped.try_recv(), Ok(()));
    let execution = workers.execution();
    let (handle, mut stopped) = worker();
    drop(workers.replace(handle));
    assert_eq!(stopped.try_recv(), Ok(()));
    drop(execution);
}

#[tokio::test]
async fn direct_sampling_retains_live_cell_then_releases_session_at_commit() {
    let (session, turn) = crate::session::tests::make_session_and_context().await;
    let session = Arc::new(session);
    let service = &session.services.code_mode_service;
    let mut execution = service.dispatch_broker.workers.execution();
    let execution = execution.0.take().unwrap();
    let step = crate::session::step_context::StepContext::for_test(Arc::new(turn));
    assert!(!step.tool_router.requires_code_mode_worker());
    let mut scope = service
        .start_turn_worker(
            &session,
            step.clone(),
            Arc::new(tokio::sync::Mutex::new(
                crate::turn_diff_tracker::TurnDiffTracker::new(),
            )),
        )
        .unwrap();
    scope.succeeded();
    drop(scope);
    tokio::task::yield_now().await;
    assert!(Arc::strong_count(&session) > 1);
    execution.finish(); // Neither wait, delegate cleanup nor service shutdown.
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while Arc::strong_count(&session) > 1 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    service.shutdown_dispatch();
    assert!(
        service
            .start_turn_worker(
                &session,
                step,
                Arc::new(tokio::sync::Mutex::new(
                    crate::turn_diff_tracker::TurnDiffTracker::new()
                ))
            )
            .is_none()
    );
}
