use super::super::CAPACITY;
use super::super::CompletionDelegate;
use super::*;
use codex_code_mode::CompletionReadySupport;
use codex_code_mode::NoopCodeModeSessionDelegate;
use codex_protocol::models::FunctionCallOutputPayload;
use pretty_assertions::assert_eq;
use test_case::test_case;

pub(in crate::tools::code_mode::completion) fn output(call: &str, id: i32) -> ResponseItem {
    ResponseItem::FunctionCallOutput {
        id: None,
        name: None,
        namespace: None,
        internal_chat_message_metadata_passthrough: None,
        call_id: Some(call.to_string()),
        output: FunctionCallOutputPayload::from_text(format!(
            "Process running with session ID {id}\nOutput:\n"
        )),
    }
}
#[test_case("ready_first")]
#[test_case("exposure_first")]
#[test_case("lost_handoff")]
#[test_case("observed")]
#[test_case("observed_while_running")]
#[test_case("suppressed")]
#[test_case("shutdown")]
fn command_handoff_and_retirement(race: &str) {
    let registry = Arc::new(Completions::default());
    registry.state().enabled = true;
    let mut reservation = registry
        .reserve_command("call", "turn", /*process_id*/ 1000, /*cell*/ None)
        .unwrap()
        .unwrap();
    let handle = reservation.command_handle();
    if !matches!(race, "exposure_first" | "observed_while_running") {
        handle.ready();
    }
    assert!(registry.batch().is_none());
    reservation.yielded();
    if race != "lost_handoff" {
        registry.record_outputs("turn", &[output("call", /*id*/ 1000)]);
    }
    if race == "observed_while_running" {
        handle.retire();
    }
    handle.ready();
    registry.finish_turn("turn");
    match race {
        "observed" => handle.retire(),
        "suppressed" => registry.suppress(),
        "shutdown" => registry.shutdown(),
        _ => {}
    }
    if matches!(
        race,
        "lost_handoff" | "observed" | "observed_while_running" | "suppressed" | "shutdown"
    ) {
        assert!(registry.batch().is_none());
        return;
    }
    let (trigger, notice) = registry.batch().unwrap();
    assert_eq!(
        notice,
        ContextualUserFragment::into(ExecCompletion(vec![1000]))
    );
    assert!(registry.permits(&trigger));
    handle.retire();
    assert!(!registry.permits(&trigger));
    let mut replacement = registry
        .reserve_command("new", "turn", /*process_id*/ 1000, /*cell*/ None)
        .unwrap()
        .unwrap();
    replacement.yielded();
    registry.record_outputs("turn", &[output("new", /*id*/ 1000)]);
    handle.ready(); // Reused numeric IDs do not accept callbacks from an old execution.
    assert!(registry.batch().is_none());
    replacement.command_handle().ready();
    assert!(registry.batch().is_some());
    registry.delivered();
    replacement.command_handle().ready();
    assert!(registry.batch().is_none());
}
#[test_case(false; "detached")]
#[test_case(true; "awaited_and_observed")]
fn nested_command_waits_for_cell_ownership_then_survives_cleanup(observed: bool) {
    let registry = Arc::new(Completions::default());
    registry.state().enabled = true;
    let mut cell = registry.reserve("cell-call", "origin").unwrap().unwrap();
    let id = CellId::new("cell".to_string());
    cell.bind(&id, CompletionReadySupport::Supported);
    let delegate = CompletionDelegate::wrap(Arc::new(NoopCodeModeSessionDelegate), &cell);
    let mut command = registry
        .reserve_command("nested", "origin", /*process_id*/ 1000, Some(&id))
        .unwrap()
        .unwrap();
    cell.yielded();
    registry.record_outputs("origin", &[super::super::tests::output("cell-call", &id)]);
    assert_eq!(registry.state().entries.len(), 2);
    command.yielded();
    command.command_handle().ready();
    assert!(registry.batch().is_none());
    if observed {
        command.command_handle().retire();
    }
    delegate.cell_completion_ready(&id);
    assert!(registry.batch().is_some());
    registry.delivered();
    delegate.cell_closed(&id);
    assert_eq!(registry.batch().is_some(), !observed);
}
#[test]
fn cells_and_commands_share_capacity_and_command_notices_are_capped() {
    let registry = Arc::new(Completions::default());
    registry.state().enabled = true;
    let cell = registry.reserve("cell", "turn").unwrap();
    for id in 0..(CAPACITY - 1) as i32 {
        let call = format!("command-{id}");
        let mut reservation = registry
            .reserve_command(&call, "turn", id, /*cell*/ None)
            .unwrap()
            .unwrap();
        reservation.yielded();
        reservation.command_handle().ready();
        registry.record_outputs("turn", &[output(&call, id)]);
    }
    assert!(
        registry
            .reserve_command(
                "overflow", "turn", /*process_id*/ 99, /*cell*/ None
            )
            .is_err()
    );
    drop(cell);
    let mut count = 0;
    while let Some((trigger, notice)) = registry.batch() {
        assert!(registry.permits(&trigger));
        let ResponseItem::Message { content, .. } = notice else {
            panic!("notice");
        };
        let codex_protocol::models::ContentItem::InputText { text } = &content[0] else {
            panic!("text");
        };
        assert!(text.len() <= 512);
        let size = registry.state().batch.as_ref().unwrap().executions.len();
        assert!(size <= 16);
        count += size;
        registry.delivered();
    }
    assert_eq!(count, CAPACITY - 1);
}

#[test]
fn parent_retirement_alone_does_not_transfer_a_live_cells_command() {
    let registry = Arc::new(Completions::default());
    assert!(
        registry
            .reserve_command(
                "direct", "turn", /*process_id*/ 1000, /*cell*/ None
            )
            .unwrap()
            .is_none()
    );
    registry.state().enabled = true;
    let id = CellId::new("cell".to_string());
    assert!(
        registry
            .reserve_command("nested", "turn", /*process_id*/ 1000, Some(&id))
            .unwrap()
            .is_none()
    );
    let mut cell = registry.reserve("cell-call", "turn").unwrap().unwrap();
    cell.bind(&id, CompletionReadySupport::Supported);
    let delegate = CompletionDelegate::wrap(Arc::new(NoopCodeModeSessionDelegate), &cell);
    let mut command = registry
        .reserve_command("nested", "turn", /*process_id*/ 1000, Some(&id))
        .unwrap()
        .unwrap();
    command.yielded();
    cell.yielded();
    registry.record_outputs("turn", &[super::super::tests::output("cell-call", &id)]);
    command.command_handle().ready();
    registry.retire_cell(&id);
    assert!(registry.batch().is_none());
    delegate.cell_completion_ready(&id);
    assert!(registry.batch().is_some());
}
