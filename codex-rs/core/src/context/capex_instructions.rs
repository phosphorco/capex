use codex_protocol::models::ContentItemKind;

use super::ContextualUserFragment;

/// A capability's durable, model-visible instructions. This uses an existing
/// response item shape so native Codex can read the shared rollout unchanged.
pub(crate) struct CapexInstructions {
    id: String,
    instructions: String,
}

impl CapexInstructions {
    pub(crate) fn new(id: String, instructions: String) -> Self {
        Self { id, instructions }
    }
}

impl ContextualUserFragment for CapexInstructions {
    fn content_kind(&self) -> ContentItemKind {
        ContentItemKind("capex.instructions".to_string())
    }

    fn role(&self) -> &'static str {
        "developer"
    }

    fn markers(&self) -> (&'static str, &'static str) {
        Self::type_markers()
    }

    fn type_markers() -> (&'static str, &'static str) {
        ("<capex_capability>", "</capex_capability>")
    }

    fn body(&self) -> String {
        format!("Capability {}:\n{}", self.id, self.instructions)
    }
}
