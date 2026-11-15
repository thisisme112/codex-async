use super::ContextualUserFragment;
use codex_protocol::models::{ContentItem, ContentItemKind, input_text};

/// Identity-only notice; results remain owned by the child sessions.
pub(crate) struct SubagentCompletion(pub(crate) Vec<String>);
impl ContextualUserFragment for SubagentCompletion {
    fn role(&self) -> &'static str {
        "user"
    }
    fn content_kind(&self) -> ContentItemKind {
        ContentItemKind("multi_agent.completion".to_string())
    }
    fn markers(&self) -> (&'static str, &'static str) {
        Self::type_markers()
    }
    fn type_markers() -> (&'static str, &'static str) {
        ("<subagent_completion>", "</subagent_completion>")
    }
    fn body(&self) -> ContentItem {
        let ids = self.0.iter().cloned().collect::<Vec<_>>().join(", ");
        input_text(format!(
            "Subagents completed: {ids}. Process their delivered results. If a result is not in context, retrieve it once using the available agent tools; do not poll running agents."
        ))
    }
}
