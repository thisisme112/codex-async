use super::ContextualUserFragment;
use codex_protocol::models::ContentItemKind;

/// Identity-only command notice, bounded by the shared completion batch cap.
pub(crate) struct ExecCompletion(pub(crate) Vec<i32>);
impl ContextualUserFragment for ExecCompletion {
    fn role(&self) -> &'static str {
        "user"
    }
    fn content_kind(&self) -> ContentItemKind {
        ContentItemKind("exec.completion".to_string())
    }
    fn markers(&self) -> (&'static str, &'static str) {
        Self::type_markers()
    }
    fn type_markers() -> (&'static str, &'static str) {
        ("<exec_completion>", "</exec_completion>")
    }
    fn body(&self) -> String {
        let ids = self
            .0
            .iter()
            .map(i32::to_string)
            .collect::<Vec<_>>()
            .join(", ");
        format!(
            "Background commands completed: {ids}. Use write_stdin (via exec in Code Mode) with each session_id and empty chars to retrieve its output and exit status. Completion may include a nonzero exit."
        )
    }
}
