use std::collections::HashMap;
use std::collections::HashSet;

use codex_protocol::user_input::UserInput;
use codex_utils_absolute_path::AbsolutePathBuf;

use crate::SkillMetadata;
use crate::ToolMentionKind;
use crate::ToolMentions;
use crate::build_skill_name_counts;
use crate::extract_tool_mentions;
use crate::normalize_skill_path;
use crate::tool_kind_for_path;

/// Supplies ordered skills, disabled identities, and discovery paths for explicit selection.
///
/// Implementations should preserve skill discovery order and expose the logical discovery path
/// associated with each canonical skill identity when one is available.
pub trait ExplicitSkillLookup {
    fn skills(&self) -> &[SkillMetadata];

    fn disabled_paths(&self) -> &HashSet<AbsolutePathBuf>;

    fn skill_discovery_path_for_path(&self, path: &AbsolutePathBuf) -> Option<&AbsolutePathBuf>;

    fn is_skill_enabled(&self, skill: &SkillMetadata) -> bool {
        !self.disabled_paths().contains(&skill.path_to_skills_md)
    }
}

/// Result of resolving explicit references with an additional availability predicate.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ExplicitSkillMentionResult {
    /// Explicit references that resolved to available skills, in discovery order.
    pub selected: Vec<SkillMetadata>,
    /// Names of config-enabled skills that were explicitly referenced but rejected by the
    /// additional availability predicate. Names are deduplicated in first-reference order.
    pub unavailable_skill_names: Vec<String>,
}

/// Collect explicitly mentioned skills from structured and text mentions.
///
/// Structured `UserInput::Skill` selections are resolved first by path against
/// enabled skills. Text inputs are then scanned to extract `$skill-name` tokens, and we
/// iterate loaded skills in their existing order to preserve prior ordering semantics.
/// Explicit paths match either a skill's canonical identity or its logical discovery
/// path, and plain names are only used when the match is unambiguous.
///
/// Complexity: `O(T + (N_s + N_t) * S)` time, `O(S + M)` space, where:
/// `S` = number of skills, `T` = total text length, `N_s` = number of structured skill inputs,
/// `N_t` = number of text inputs, `M` = max mentions parsed from a single text input.
pub fn collect_explicit_skill_mentions(
    inputs: &[UserInput],
    loaded_skills: &impl ExplicitSkillLookup,
    connector_slug_counts: &HashMap<String, usize>,
) -> Vec<SkillMetadata> {
    collect_explicit_skill_mentions_with_availability(
        inputs,
        loaded_skills,
        connector_slug_counts,
        |_| true,
    )
    .selected
}

/// Collect explicit skill mentions after applying an additional availability predicate.
///
/// The predicate is applied before name counts are calculated, so unavailable skills do not
/// make an otherwise available same-name skill ambiguous. Explicit references that resolve to
/// config-enabled but unavailable skills are returned in `unavailable_skill_names` for the
/// caller to explain. This does not report nonexistent names or skills disabled by the normal
/// skill configuration.
pub fn collect_explicit_skill_mentions_with_availability<F>(
    inputs: &[UserInput],
    loaded_skills: &impl ExplicitSkillLookup,
    connector_slug_counts: &HashMap<String, usize>,
    is_available: F,
) -> ExplicitSkillMentionResult
where
    F: Fn(&SkillMetadata) -> bool,
{
    let selectable_skills = loaded_skills
        .skills()
        .iter()
        .filter(|skill| loaded_skills.is_skill_enabled(skill) && is_available(skill))
        .cloned()
        .collect::<Vec<_>>();
    let skill_name_counts = build_skill_name_counts(&selectable_skills, &HashSet::new()).0;

    let selection_context = SkillSelectionContext {
        loaded_skills,
        is_available: &is_available,
        skill_name_counts: &skill_name_counts,
        connector_slug_counts,
    };
    let mut selected: Vec<SkillMetadata> = Vec::new();
    let mut seen_names: HashSet<String> = HashSet::new();
    let mut seen_paths: HashSet<AbsolutePathBuf> = HashSet::new();
    let mut blocked_plain_names: HashSet<String> = HashSet::new();
    let mut unavailable_names = Vec::new();
    let mut unavailable_names_seen = HashSet::new();

    for input in inputs {
        if let UserInput::Skill { name, path, .. } = input {
            blocked_plain_names.insert(name.clone());
            let Ok(path) = AbsolutePathBuf::relative_to_current_dir(path) else {
                continue;
            };

            let Some(skill) = selection_context
                .loaded_skills
                .skills()
                .iter()
                .find(|skill| {
                    skill.path_to_skills_md == path
                        || selection_context
                            .loaded_skills
                            .skill_discovery_path_for_path(&skill.path_to_skills_md)
                            .is_some_and(|discovery_path| discovery_path == &path)
                })
            else {
                continue;
            };

            if !selection_context.loaded_skills.is_skill_enabled(skill) {
                continue;
            }
            if !(selection_context.is_available)(skill) {
                record_unavailable_name(skill, &mut unavailable_names_seen, &mut unavailable_names);
                continue;
            }
            if seen_paths.contains(&skill.path_to_skills_md) {
                continue;
            }

            seen_paths.insert(skill.path_to_skills_md.clone());
            seen_names.insert(skill.name.clone());
            selected.push(skill.clone());
        }
    }

    for input in inputs {
        if let UserInput::Text { text, .. } = input {
            let mentioned_names = extract_tool_mentions(text);
            select_skills_from_mentions(
                &selection_context,
                &blocked_plain_names,
                &mentioned_names,
                &mut seen_names,
                &mut seen_paths,
                &mut selected,
                &mut unavailable_names_seen,
                &mut unavailable_names,
            );
        }
    }

    ExplicitSkillMentionResult {
        selected,
        unavailable_skill_names: unavailable_names,
    }
}

fn record_unavailable_name(
    skill: &SkillMetadata,
    seen: &mut HashSet<String>,
    unavailable: &mut Vec<String>,
) {
    if seen.insert(skill.name.clone()) {
        unavailable.push(skill.name.clone());
    }
}

struct SkillSelectionContext<'a, F> {
    loaded_skills: &'a dyn ExplicitSkillLookup,
    is_available: &'a F,
    skill_name_counts: &'a HashMap<String, usize>,
    connector_slug_counts: &'a HashMap<String, usize>,
}

/// Select mentioned skills while preserving the order of `skills`.
fn select_skills_from_mentions(
    selection_context: &SkillSelectionContext<'_, impl Fn(&SkillMetadata) -> bool>,
    blocked_plain_names: &HashSet<String>,
    mentions: &ToolMentions<'_>,
    seen_names: &mut HashSet<String>,
    seen_paths: &mut HashSet<AbsolutePathBuf>,
    selected: &mut Vec<SkillMetadata>,
    unavailable_names_seen: &mut HashSet<String>,
    unavailable_names: &mut Vec<String>,
) {
    if mentions.is_empty() {
        return;
    }

    let mention_skill_paths: HashSet<String> = mentions
        .paths()
        .filter(|path| {
            !matches!(
                tool_kind_for_path(path),
                ToolMentionKind::App | ToolMentionKind::Mcp | ToolMentionKind::Plugin
            )
        })
        .map(normalize_host_skill_path)
        .collect();

    for skill in selection_context.loaded_skills.skills() {
        if !selection_context.loaded_skills.is_skill_enabled(skill) {
            continue;
        }

        let canonical_path = normalize_host_skill_path(&skill.path_to_skills_md.to_string_lossy());
        let matches_discovery_path = selection_context
            .loaded_skills
            .skill_discovery_path_for_path(&skill.path_to_skills_md)
            .is_some_and(|discovery_path| {
                mention_skill_paths.contains(&normalize_host_skill_path(
                    &discovery_path.to_string_lossy(),
                ))
            });
        if mention_skill_paths.contains(&canonical_path) || matches_discovery_path {
            if !(selection_context.is_available)(skill) {
                record_unavailable_name(skill, unavailable_names_seen, unavailable_names);
                continue;
            }
            if seen_paths.contains(&skill.path_to_skills_md) {
                continue;
            }
            seen_paths.insert(skill.path_to_skills_md.clone());
            seen_names.insert(skill.name.clone());
            selected.push(skill.clone());
        }
    }

    for skill in selection_context.loaded_skills.skills() {
        if !selection_context.loaded_skills.is_skill_enabled(skill) {
            continue;
        }

        if blocked_plain_names.contains(skill.name.as_str()) {
            continue;
        }
        if !mentions.contains_plain_name(skill.name.as_str()) {
            continue;
        }

        if !(selection_context.is_available)(skill) {
            let available_name_count = selection_context
                .skill_name_counts
                .get(skill.name.as_str())
                .copied()
                .unwrap_or(0);
            let connector_count = selection_context
                .connector_slug_counts
                .get(&skill.name.to_ascii_lowercase())
                .copied()
                .unwrap_or(0);
            if available_name_count == 0 && connector_count == 0 {
                record_unavailable_name(skill, unavailable_names_seen, unavailable_names);
            }
            continue;
        }
        if seen_paths.contains(&skill.path_to_skills_md) {
            continue;
        }

        let skill_count = selection_context
            .skill_name_counts
            .get(skill.name.as_str())
            .copied()
            .unwrap_or(0);
        let connector_count = selection_context
            .connector_slug_counts
            .get(&skill.name.to_ascii_lowercase())
            .copied()
            .unwrap_or(0);
        if skill_count != 1 || connector_count != 0 {
            continue;
        }

        if seen_names.insert(skill.name.clone()) {
            seen_paths.insert(skill.path_to_skills_md.clone());
            selected.push(skill.clone());
        }
    }
}

fn normalize_host_skill_path(path: &str) -> String {
    normalize_skill_path(path).replace('\\', "/")
}

#[cfg(test)]
#[path = "selection_tests.rs"]
mod tests;
