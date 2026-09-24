use std::collections::HashSet;

use codex_protocol::user_input::UserInput;
use pretty_assertions::assert_eq;

use crate::catalog::SkillAuthority;
use crate::catalog::SkillCatalog;
use crate::catalog::SkillCatalogEntry;
use crate::catalog::SkillPackageId;
use crate::catalog::SkillResourceId;
use crate::catalog::SkillSourceKind;
use crate::selection::collect_explicit_skill_mentions;

#[test]
fn explicit_skill_mentions_obey_tag_filtered_catalog() {
    let mut source = SkillCatalog::default();
    for (name, tags) in [("frontend", vec!["frontend"]), ("backend", vec!["backend"])] {
        source.push_entry(
            SkillCatalogEntry::new(
                SkillPackageId(format!("/skills/{name}")),
                SkillAuthority::new(SkillSourceKind::Host, "host"),
                name,
                format!("{name} skill"),
                SkillResourceId::new(format!("/skills/{name}/SKILL.md")),
            )
            .with_tags(tags.into_iter().map(str::to_string).collect()),
        );
    }
    let active = HashSet::from(["frontend".to_string()]);
    let filtered = source.with_tag_availability(&active, &HashSet::new());
    let inputs = [UserInput::Text {
        text: "use $frontend and $backend".to_string(),
        text_elements: Vec::new(),
    }];

    let selected = collect_explicit_skill_mentions(&inputs, &filtered);

    assert_eq!(
        selected
            .iter()
            .map(|entry| entry.name.as_str())
            .collect::<Vec<_>>(),
        vec!["frontend"]
    );
}
