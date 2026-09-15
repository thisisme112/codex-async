use super::*;
use crate::session::tests::make_session_and_context_with_rx;

#[tokio::test]
async fn reservation_withdrawal_notifies_without_requiring_a_turn_stop() {
    let (session, _, _) = make_session_and_context_with_rx().await;
    let mut activity = session.services.code_mode_service.completions.subscribe();
    let active = ActiveTurn::default();
    let reservation = Arc::clone(&active.turn_state);
    *session.active_turn.lock().await = Some(active);
    activity.borrow_and_update();
    session
        .clear_reserved_idle_turn(&reservation, /*notify_on_withdrawal*/ true)
        .await;
    assert!(session.active_turn.lock().await.is_none());
    assert!(activity.has_changed().unwrap());
    activity.borrow_and_update();
    // A stale withdrawal cannot clear a newer reservation or announce availability.
    *session.active_turn.lock().await = Some(ActiveTurn::default());
    session
        .clear_reserved_idle_turn(&reservation, /*notify_on_withdrawal*/ true)
        .await;
    assert!(session.active_turn.lock().await.is_some());
    assert!(!activity.has_changed().unwrap());
}
