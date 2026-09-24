//! Session-scoped CapEx capability selection.
//!
//! The optional selector has no effect unless `CAPEX_CAPABILITY_MODE=1`.
//! Grants are stored outside the shared Codex rollout format.

use std::collections::BTreeSet;
use std::collections::HashSet;
use std::fs;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Mutex;

use codex_protocol::ThreadId;
use codex_protocol::models::ContentItem;
use codex_protocol::models::ResponseItem;
use codex_rollout::RolloutItem;
use codex_skills::parse_capability_tags;

use crate::capex_state::CapabilityGrantSnapshot;
use crate::capex_state::CapabilitySessionState;
use crate::capex_state::CapabilityStateLoadMode;
use crate::capex_state::load_capability_state;
use crate::capex_state::persist_fork_snapshots;
use crate::capex_state::read_existing_capability_grants;

pub(crate) struct CapexRuntime {
    root: PathBuf,
    excluded_tags: HashSet<String>,
    state: Mutex<CapabilitySessionState>,
}

/// Match only harness-classified capability context, never user/model text
/// that happens to quote a capability marker.
pub(crate) fn is_capex_instruction(
    item: &ResponseItem,
    snapshot: &CapabilityGrantSnapshot,
) -> bool {
    let ResponseItem::Message {
        role,
        content,
        internal_chat_message_metadata_passthrough: Some(metadata),
        ..
    } = item
    else {
        return false;
    };
    if role != "developer" {
        return false;
    }
    let Some(kinds) = metadata.content_item_kinds.as_ref() else {
        return false;
    };
    if content.len() != kinds.len() {
        return false;
    }
    let expected = capex_instruction_text(snapshot);
    content.iter().zip(kinds).any(|(part, kind)| {
        kind.0 == "capex.instructions"
            && matches!(part, ContentItem::InputText { text } if text == &expected)
    })
}

/// Exact-text matching is only for avoiding duplicate guidance after a native
/// reader may have discarded CapEx's private content classification. Never
/// use this predicate to infer a grant or fork provenance.
pub(crate) fn is_capex_instruction_text(
    item: &ResponseItem,
    snapshot: &CapabilityGrantSnapshot,
) -> bool {
    let ResponseItem::Message { role, content, .. } = item else {
        return false;
    };
    if role != "developer" {
        return false;
    }
    let expected = capex_instruction_text(snapshot);
    content
        .iter()
        .any(|part| matches!(part, ContentItem::InputText { text } if text == &expected))
}

fn capex_instruction_text(snapshot: &CapabilityGrantSnapshot) -> String {
    format!(
        "<capex_capability>Capability {}:\n{}</capex_capability>",
        snapshot.id, snapshot.instructions
    )
}

/// Fork history may retain an instruction marker whose grant is after the
/// selected user-message cutoff. Remove inherited CapEx guidance and rebuild
/// it from the child sidecar on the next turn. This is never grant evidence.
pub(crate) fn is_inherited_capex_instruction(item: &ResponseItem) -> bool {
    let ResponseItem::Message {
        role,
        content,
        internal_chat_message_metadata_passthrough,
        ..
    } = item
    else {
        return false;
    };
    if role != "developer" || content.len() != 1 {
        return false;
    }
    let ContentItem::InputText { text } = &content[0] else {
        return false;
    };
    let classified = internal_chat_message_metadata_passthrough
        .as_ref()
        .and_then(|metadata| metadata.content_item_kinds.as_ref())
        .is_some_and(|kinds| kinds.len() == 1 && kinds[0].0 == "capex.instructions");
    classified
        || (text.starts_with("<capex_capability>Capability ")
            && text.ends_with("</capex_capability>"))
}

pub(crate) fn seed_fork_from_history(
    codex_home: &Path,
    parent_thread_id: ThreadId,
    child_thread_id: ThreadId,
    items: &[RolloutItem],
) -> Result<(), String> {
    if std::env::var("CAPEX_CAPABILITY_MODE").as_deref() != Ok("1") {
        return Ok(());
    }
    let Some(parent_grants) = read_existing_capability_grants(codex_home, parent_thread_id)? else {
        return Ok(());
    };
    let selected = parent_grants
        .into_iter()
        .filter(|snapshot| {
            // Initial grants exist from session creation, including before a
            // first turn can persist an instruction marker in the rollout.
            if snapshot.origin_turn_id.is_none() {
                return true;
            }
            // A prompt hook records its instruction before its triggering
            // user message. A fork cut just before that user message can
            // retain the instruction but not the accepted turn. Require a
            // terminal turn boundary as conservative cutoff evidence.
            let origin_retained = snapshot.origin_turn_id.as_ref().is_some_and(|turn_id| {
                items.iter().any(|item| {
                    matches!(item, RolloutItem::EventMsg(codex_protocol::protocol::EventMsg::TurnComplete(event))
                        if &event.turn_id == turn_id)
                })
            });
            origin_retained &&
            items.iter().any(|item| {
                matches!(item, RolloutItem::ResponseItem(envelope)
                    if is_capex_instruction(&envelope.item, snapshot))
            })
        })
        .collect::<Vec<_>>();
    persist_fork_snapshots(codex_home, child_thread_id, &selected)
        .map_err(|error| error.to_string())
}

impl CapexRuntime {
    pub(crate) fn from_environment(
        cwd: &Path,
        codex_home: &Path,
        thread_id: ThreadId,
        load_mode: CapabilityStateLoadMode,
    ) -> Result<Option<Self>, String> {
        let mode = std::env::var("CAPEX_CAPABILITY_MODE").unwrap_or_default();
        if mode.is_empty() || mode == "0" {
            return Ok(None);
        }
        if mode != "1" {
            return Err(format!("invalid CAPEX_CAPABILITY_MODE value `{mode}`"));
        }

        let project_root = cwd
            .ancestors()
            .find(|ancestor| ancestor.join(".git").exists())
            .unwrap_or(cwd);
        let root = project_root.join(".agents").join("capabilities");
        let excluded_tags = comma_separated("CAPEX_ALWAYS_EXCLUDE_TAGS")
            .into_iter()
            .collect();
        let loaded = load_capability_state(codex_home, thread_id, load_mode, || {
            comma_separated("CAPEX_INITIAL_CAPABILITIES")
                .into_iter()
                .map(|id| load_capability(&root, &id, None))
                .collect()
        });
        if let Some(diagnostic) = loaded.diagnostic {
            tracing::warn!("CapEx state: {diagnostic}");
        }
        Ok(Some(Self {
            root,
            excluded_tags,
            state: Mutex::new(loaded.state),
        }))
    }

    pub(crate) fn skill_tags(&self) -> (HashSet<String>, HashSet<String>) {
        let active = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .active_tags()
            .into_iter()
            .collect();
        (active, self.excluded_tags.clone())
    }

    pub(crate) fn skill_is_available(&self, skill: &codex_skills::SkillMetadata) -> bool {
        let active_tags = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .active_tags();
        !skill.tags.is_empty()
            && skill.tags.iter().any(|tag| active_tags.contains(tag))
            && !skill
                .tags
                .iter()
                .any(|tag| self.excluded_tags.contains(tag))
    }

    pub(crate) fn grants(&self) -> Vec<CapabilityGrantSnapshot> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .grants()
            .cloned()
            .collect()
    }

    pub(crate) fn grant(
        &self,
        id: &str,
        origin_turn_id: &str,
    ) -> Result<Option<CapabilityGrantSnapshot>, String> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.grants().any(|grant| grant.id == id) {
            return Ok(None);
        }
        let snapshot = load_capability(&self.root, id, Some(origin_turn_id.to_string()))?;
        let accepted = state
            .grant(snapshot.clone())
            .map_err(|error| error.to_string())?;
        Ok(accepted.then_some(snapshot))
    }
}

fn comma_separated(key: &str) -> Vec<String> {
    let mut seen = HashSet::new();
    std::env::var(key)
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|entry| !entry.is_empty() && seen.insert((*entry).to_string()))
        .map(ToString::to_string)
        .collect()
}

fn load_capability(
    root: &Path,
    id: &str,
    origin_turn_id: Option<String>,
) -> Result<CapabilityGrantSnapshot, String> {
    if id.is_empty()
        || id.len() > 128
        || !id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(format!("invalid capability ID `{id}`"));
    }
    let path = root.join(id).join("CAPABILITY.md");
    let contents = fs::read_to_string(&path).map_err(|error| {
        format!(
            "cannot read capability `{id}` at {}: {error}",
            path.display()
        )
    })?;
    let tags = parse_capability_tags(&contents)
        .map_err(|error| format!("invalid capability `{id}`: {error}"))?
        .into_iter()
        .collect::<BTreeSet<_>>();
    let mut lines = contents.lines();
    if lines.next().map(str::trim) != Some("---") {
        return Err(format!("capability `{id}` is missing YAML frontmatter"));
    }
    let Some(delimiter) = lines.position(|line| line.trim() == "---") else {
        return Err(format!(
            "capability `{id}` has unterminated YAML frontmatter"
        ));
    };
    let instructions = contents
        .lines()
        .skip(delimiter + 2)
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string();
    if instructions.is_empty() {
        return Err(format!("capability `{id}` has no instructions"));
    }
    Ok(CapabilityGrantSnapshot {
        id: id.to_string(),
        tags,
        instructions,
        origin_turn_id,
    })
}
