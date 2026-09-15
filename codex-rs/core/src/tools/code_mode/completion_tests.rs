use super::*;
use codex_code_mode::NoopCodeModeSessionDelegate;
use codex_protocol::models::FunctionCallOutputPayload;
use pretty_assertions::assert_eq;
use test_case::test_case;

fn registry() -> Arc<Completions> {
    let registry = Arc::new(Completions::default());
    registry.state().enabled = true;
    registry
}

pub(super) fn output(call_id: &str, cell_id: &CellId) -> ResponseItem {
    ResponseItem::CustomToolCallOutput {
        id: None,
        internal_chat_message_metadata_passthrough: None,
        call_id: call_id.to_string(),
        name: None,
        output: FunctionCallOutputPayload::from_text(format!(
            "Script running with cell ID {cell_id}\nOutput:\n"
        )),
    }
}

#[test_case(true; "ready_before_exposure")]
#[test_case(false; "exposure_before_ready")]
fn readiness_requires_recorded_yield_and_keeps_results_unowned(ready_first: bool) {
    let registry = registry();
    let mut reservation = registry.reserve("call", "turn").unwrap().unwrap();
    let delegate = CompletionDelegate::wrap(Arc::new(NoopCodeModeSessionDelegate), &reservation);
    let cell_id = CellId::new("generation:cell".to_string());
    if ready_first {
        delegate.cell_completion_ready(&cell_id);
    }
    assert!(registry.batch().is_none());
    assert!(reservation.bind(&cell_id, CompletionReadySupport::Supported));
    reservation.yielded();
    assert!(registry.batch().is_none());
    registry.record_outputs("turn", &[output("call", &cell_id)]);
    if !ready_first {
        assert!(registry.batch().is_none());
        delegate.cell_completion_ready(&cell_id);
    }
    let (trigger, notice) = registry.batch().unwrap();
    assert!(registry.permits(&trigger));
    assert_eq!(
        notice,
        ContextualUserFragment::into(CodeModeCompletion(vec![cell_id.clone()]))
    );
    registry.delivered();
    delegate.cell_completion_ready(&cell_id);
    assert!(registry.batch().is_none());
    assert!(registry.state().entries.is_empty());
}

#[test_case("unsupported")]
#[test_case("synchronous")]
#[test_case("initial_failure")]
#[test_case("handoff_failure")]
#[test_case("replacement_output")]
#[test_case("consumed")]
#[test_case("disconnect")]
#[test_case("terminate")]
#[test_case("shutdown")]
fn retired_executions_never_wake(reason: &str) {
    let registry = registry();
    let mut reservation = registry.reserve("call", "turn").unwrap().unwrap();
    let delegate = CompletionDelegate::wrap(Arc::new(NoopCodeModeSessionDelegate), &reservation);
    let cell_id = CellId::new("cell".to_string());
    delegate.cell_completion_ready(&cell_id);
    let support = if reason == "unsupported" {
        CompletionReadySupport::Unsupported
    } else {
        CompletionReadySupport::Supported
    };
    reservation.bind(&cell_id, support);
    if !matches!(reason, "synchronous" | "initial_failure") {
        reservation.yielded();
    }
    match reason {
        "handoff_failure" => registry.finish_turn("turn"),
        "replacement_output" => registry.record_outputs(
            "turn",
            &[ResponseItem::CustomToolCallOutput {
                id: None,
                internal_chat_message_metadata_passthrough: None,
                call_id: "call".to_string(),
                name: None,
                output: FunctionCallOutputPayload::from_text("aborted".to_string()),
            }],
        ),
        "consumed" => delegate.cell_closed(&cell_id),
        "terminate" => registry.retire_cell(&cell_id),
        "shutdown" => registry.shutdown(),
        _ => {}
    }
    drop(reservation);
    if reason == "disconnect" {
        drop(delegate);
    } else {
        delegate.cell_completion_ready(&cell_id);
    }
    registry.record_outputs("turn", &[output("call", &cell_id)]);
    assert!(registry.batch().is_none());
    assert!(registry.state().entries.is_empty());
}

#[test]
fn admission_is_bounded_before_execution_and_released_after_failure() {
    let registry = registry();
    let mut reservations = Vec::new();
    for i in 0..CAPACITY {
        reservations.push(registry.reserve(&format!("call-{i}"), "turn").unwrap());
    }
    assert!(registry.reserve("overflow", "turn").is_err());
    reservations.pop();
    assert!(registry.reserve("replacement", "turn").unwrap().is_some());
    assert!(registry.reserve(&"x".repeat(ID_BYTES + 1), "turn").is_err());
}

#[test]
fn exposure_and_late_callbacks_cannot_cross_threads_or_executions() {
    let other_thread = registry();
    let registry = registry();
    let cell_id = CellId::new("generation:cell".to_string());
    let mut old = registry.reserve("call", "old-turn").unwrap().unwrap();
    let old_delegate = CompletionDelegate::wrap(Arc::new(NoopCodeModeSessionDelegate), &old);
    old.bind(&cell_id, CompletionReadySupport::Supported);
    old.yielded();
    old_delegate.cell_closed(&cell_id);

    // Even reused provider IDs cannot route an old execution's callbacks to a new one.
    let mut current = registry.reserve("call", "new-turn").unwrap().unwrap();
    let current_delegate =
        CompletionDelegate::wrap(Arc::new(NoopCodeModeSessionDelegate), &current);
    current.bind(&cell_id, CompletionReadySupport::Supported);
    current.yielded();
    old_delegate.cell_completion_ready(&cell_id);
    registry.record_outputs("new-turn", &[output("call", &cell_id)]);
    assert!(registry.batch().is_none());
    old_delegate.cell_closed(&cell_id);
    current_delegate.cell_completion_ready(&cell_id);
    assert!(registry.batch().is_some());

    let mut other = other_thread.reserve("call", "other-turn").unwrap().unwrap();
    let other_delegate = CompletionDelegate::wrap(Arc::new(NoopCodeModeSessionDelegate), &other);
    other.bind(&cell_id, CompletionReadySupport::Supported);
    other.yielded();
    other_delegate.cell_completion_ready(&cell_id);
    other_thread.record_outputs("new-turn", &[output("call", &cell_id)]);
    assert!(other_thread.batch().is_none());
    other_thread.record_outputs("other-turn", &[output("call", &cell_id)]);
    assert!(other_thread.batch().is_some());
}

#[test]
fn notices_are_bounded_and_admission_revalidates_suppression_and_consumption() {
    let registry = registry();
    let mut delegates = Vec::new();
    for i in 0..CAPACITY {
        let mut reservation = registry
            .reserve(&format!("call-{i}"), "turn")
            .unwrap()
            .unwrap();
        let cell_id = CellId::new(format!("{i}:{}", "x".repeat(120)));
        assert!(reservation.bind(&cell_id, CompletionReadySupport::Supported));
        let delegate =
            CompletionDelegate::wrap(Arc::new(NoopCodeModeSessionDelegate), &reservation);
        delegate.cell_completion_ready(&cell_id);
        reservation.yielded();
        registry.record_outputs("turn", &[output(&format!("call-{i}"), &cell_id)]);
        delegates.push(delegate);
    }
    let (trigger, _) = registry.batch().unwrap();
    assert!(registry.permits(&trigger));
    registry.suppress();
    assert!(!registry.permits(&trigger));
    assert!(registry.batch().is_none());
    registry.resume();
    let mut delivered = 0;
    while let Some((trigger, notice)) = registry.batch() {
        let ResponseItem::Message { content, .. } = notice else {
            panic!("notice")
        };
        let codex_protocol::models::ContentItem::InputText { text } = &content[0] else {
            panic!("text")
        };
        assert!(text.len() <= 512);
        let count = registry.state().batch.as_ref().unwrap().executions.len();
        assert!(count <= 16);
        assert!(registry.permits(&trigger));
        registry.delivered();
        delivered += count;
    }
    assert_eq!(delivered, CAPACITY);
}
