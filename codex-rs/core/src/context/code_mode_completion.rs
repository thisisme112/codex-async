use super::ContextualUserFragment;
use codex_code_mode::CellId;
use codex_protocol::models::ContentItem;
use codex_protocol::models::ContentItemKind;
use codex_protocol::models::input_text;

/// Identity-only notice. Admission limits IDs to 128 ASCII bytes and selects
/// at most 16 whose complete rendered fragment fits within 512 tokens.
pub(crate) struct CodeModeCompletion(pub(crate) Vec<CellId>);

impl ContextualUserFragment for CodeModeCompletion {
    fn role(&self) -> &'static str {
        "user"
    }

    fn content_kind(&self) -> ContentItemKind {
        ContentItemKind("code_mode.completion".to_string())
    }

    fn markers(&self) -> (&'static str, &'static str) {
        Self::type_markers()
    }

    fn type_markers() -> (&'static str, &'static str) {
        ("<code_mode_completion>", "</code_mode_completion>")
    }

    fn body(&self) -> ContentItem {
        let ids = self
            .0
            .iter()
            .map(CellId::as_str)
            .collect::<Vec<_>>()
            .join(", ");
        input_text(format!(
            "Code Mode cells completed: {ids}. Use the existing wait tool with each cell_id to retrieve its result. Completion may include a script error."
        ))
    }
}
