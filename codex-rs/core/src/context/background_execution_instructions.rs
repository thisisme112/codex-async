use super::ContextualUserFragment;
use codex_protocol::models::ContentItem;
use codex_protocol::models::ContentItemKind;
use codex_protocol::models::input_text;

/// Stable capability guidance for live interactive completion-enabled clients.
pub(crate) struct BackgroundExecutionInstructions;

impl ContextualUserFragment for BackgroundExecutionInstructions {
    fn role(&self) -> &'static str {
        "developer"
    }
    fn content_kind(&self) -> ContentItemKind {
        ContentItemKind("exec.background_instructions".to_string())
    }
    fn markers(&self) -> (&'static str, &'static str) {
        Self::type_markers()
    }
    fn type_markers() -> (&'static str, &'static str) {
        ("<background_execution>", "</background_execution>")
    }
    fn body(&self) -> ContentItem {
        input_text("This live interactive CLI supports event-driven background command completion. When exec_command yields a session ID with completion_notification_registered=true (or the text 'Completion notification registered'), its completion is subscribed. Continue independent work, then end the turn when nothing useful remains; the CLI will resume this thread automatically after completion. Do not repeatedly call write_stdin, sleep, ps, or other commands solely to check whether that tracked task finished. If you need to wait within the current turn, call wait_for_tasks({session_ids:[...]}) once; it suspends until ALL listed commands finish without polling. In Code Mode call tools.wait_for_tasks through exec; if the outer exec yields, end the turn and let its completion notification arrive rather than repeatedly calling wait. When notified, retrieve each completed command's output and exit status once with write_stdin and analyze the result. Use write_stdin when actual intermediate diagnostics or input are needed. Non-empty stdin and cancellation remain available. Do not wait for completion of servers or tasks requiring input. Automatic notification requires this CLI to remain running; it is not restart-persistent and is not guaranteed for sessions without a registered subscription. Existing approval and permission rules still apply.")
    }
}
