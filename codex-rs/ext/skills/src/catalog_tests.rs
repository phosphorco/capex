use std::collections::HashSet;

use pretty_assertions::assert_eq;

use crate::catalog::SkillAuthority;
use crate::catalog::SkillCatalog;
use crate::catalog::SkillCatalogEntry;
use crate::catalog::SkillPackageId;
use crate::catalog::SkillResourceId;
use crate::catalog::SkillSourceKind;

fn entry(name: &str, tags: &[&str], enabled: bool) -> SkillCatalogEntry {
    let entry = SkillCatalogEntry::new(
        SkillPackageId(format!("/skills/{name}")),
        SkillAuthority::new(SkillSourceKind::Host, "host"),
        name,
        format!("{name} skill"),
        SkillResourceId::new(format!("/skills/{name}/SKILL.md")),
    )
    .with_tags(tags.iter().map(|tag| (*tag).to_string()).collect());
    if enabled { entry } else { entry.disabled() }
}

#[test]
fn tag_filter_uses_any_match_and_exclusions_take_precedence() {
    let source = SkillCatalog {
        entries: vec![
            entry("frontend", &["frontend"], true),
            entry("multitag", &["frontend", "ungranted"], true),
            entry("backend", &["backend", "blocked"], true),
            entry("video", &["video"], true),
            entry("untagged", &[], true),
            entry("disabled", &["frontend"], false),
        ],
        warnings: vec!["preserve warning".to_string()],
    };
    let active = HashSet::from(["frontend".to_string(), "backend".to_string()]);
    let excluded = HashSet::from(["blocked".to_string()]);
    let source_before_filter = source.clone();

    let filtered = source.with_tag_availability(&active, &excluded);

    assert_eq!(
        filtered
            .entries
            .iter()
            .map(|entry| (entry.name.as_str(), entry.enabled))
            .collect::<Vec<_>>(),
        vec![
            ("frontend", true),
            ("multitag", true),
            ("backend", false),
            ("video", false),
            ("untagged", false),
            ("disabled", false),
        ]
    );
    assert_eq!(filtered.warnings, source.warnings);
    assert_eq!(source, source_before_filter);
}

#[test]
fn empty_active_tags_hide_every_skill_without_mutating_source() {
    let source = SkillCatalog {
        entries: vec![
            entry("tagged", &["frontend"], true),
            entry("untagged", &[], true),
        ],
        warnings: Vec::new(),
    };

    let filtered = source.with_tag_availability(&HashSet::new(), &HashSet::new());

    assert!(filtered.entries.iter().all(|entry| !entry.enabled));
    assert!(source.entries.iter().all(|entry| entry.enabled));
}
