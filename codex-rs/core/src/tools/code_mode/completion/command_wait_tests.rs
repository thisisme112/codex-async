use super::*;
use crate::tools::code_mode::completion::Reservation;
use pretty_assertions::assert_eq;
use std::sync::Arc;
use std::time::Duration;

fn tracked(registry: &Arc<Completions>, id: i32) -> Reservation {
    registry.state().enabled = true;
    let mut reservation = registry
        .reserve_command(&format!("call-{id}"), "origin", id, /*cell*/ None)
        .unwrap()
        .unwrap();
    reservation.yielded();
    reservation
}

#[tokio::test]
async fn wait_uses_events_and_requires_all_requested_executions() {
    let registry = Arc::new(Completions::default());
    let first = tracked(&registry, 1);
    let second = tracked(&registry, 2);
    let cancel = CancellationToken::new();
    let wait = registry.wait_for_commands(&[1, 2], &cancel);
    tokio::pin!(wait);
    assert!(
        tokio::time::timeout(Duration::from_millis(20), &mut wait)
            .await
            .is_err()
    );
    first.command_handle().ready();
    assert!(
        tokio::time::timeout(Duration::from_millis(20), &mut wait)
            .await
            .is_err()
    );
    second.command_handle().ready();
    assert_eq!(wait.await, Ok(()));
    assert_eq!(registry.state().entries.len(), 2);
}

#[tokio::test]
async fn cancellation_leaves_command_subscription_intact() {
    let registry = Arc::new(Completions::default());
    let _command = tracked(&registry, 1);
    let cancel = CancellationToken::new();
    let wait = registry.wait_for_commands(&[1], &cancel);
    tokio::pin!(wait);
    assert!(
        tokio::time::timeout(Duration::from_millis(20), &mut wait)
            .await
            .is_err()
    );
    cancel.cancel();
    assert!(wait.await.is_err());
    assert_eq!(registry.state().entries.len(), 1);
}

#[tokio::test]
async fn retired_identity_is_not_replaced_by_reused_numeric_id() {
    let registry = Arc::new(Completions::default());
    let command = tracked(&registry, 1);
    let cancel = CancellationToken::new();
    let wait = registry.wait_for_commands(&[1], &cancel);
    tokio::pin!(wait);
    assert!(
        tokio::time::timeout(Duration::from_millis(20), &mut wait)
            .await
            .is_err()
    );
    command.command_handle().retire();
    let replacement = tracked(&registry, 1);
    replacement.command_handle().ready();
    assert!(wait.await.is_err());
}

#[tokio::test]
async fn shutdown_and_suppression_release_pending_waits() {
    for shutdown in [false, true] {
        let registry = Arc::new(Completions::default());
        let _command = tracked(&registry, 1);
        let cancel = CancellationToken::new();
        let wait = registry.wait_for_commands(&[1], &cancel);
        tokio::pin!(wait);
        assert!(
            tokio::time::timeout(Duration::from_millis(20), &mut wait)
                .await
                .is_err()
        );
        if shutdown {
            registry.shutdown();
        } else {
            registry.suppress();
        }
        assert!(wait.await.is_err());
    }
}

#[tokio::test]
async fn invalid_selections_fail_before_waiting() {
    let registry = Arc::new(Completions::default());
    let _command = tracked(&registry, 1);
    let cancel = CancellationToken::new();
    for ids in [vec![], vec![1, 1], vec![1, 99], vec![1; 65]] {
        assert!(registry.wait_for_commands(&ids, &cancel).await.is_err());
    }
}
