use super::*;
use codex_utils_absolute_path::AbsolutePathBuf;
use codex_utils_absolute_path::test_support::PathBufExt;
use codex_utils_absolute_path::test_support::test_path_buf;
use pretty_assertions::assert_eq;
use std::collections::HashMap;
use std::collections::HashSet;

#[derive(Default)]
struct TestLookup {
    skills: Vec<SkillMetadata>,
    disabled_paths: HashSet<AbsolutePathBuf>,
    skill_discovery_path_by_path: HashMap<AbsolutePathBuf, AbsolutePathBuf>,
}

impl ExplicitSkillLookup for TestLookup {
    fn skills(&self) -> &[SkillMetadata] {
        &self.skills
    }

    fn disabled_paths(&self) -> &HashSet<AbsolutePathBuf> {
        &self.disabled_paths
    }

    fn skill_discovery_path_for_path(&self, path: &AbsolutePathBuf) -> Option<&AbsolutePathBuf> {
        self.skill_discovery_path_by_path.get(path)
    }
}

fn make_skill(name: &str, path: &str) -> SkillMetadata {
    SkillMetadata {
        name: name.to_string(),
        description: format!("{name} skill"),
        short_description: None,
        tags: Vec::new(),
        interface: None,
        dependencies: None,
        policy: None,
        path_to_skills_md: test_path_buf(path).abs(),
        scope: codex_protocol::protocol::SkillScope::User,
        plugin_id: None,
        remote_plugin_id: None,
    }
}

fn linked_skill_mention(name: &str, unix_path: &str) -> String {
    format!("[${name}]({})", test_path_buf(unix_path).display())
}

fn collect_mentions(
    inputs: &[UserInput],
    skills: &[SkillMetadata],
    disabled_paths: &HashSet<AbsolutePathBuf>,
    connector_slug_counts: &HashMap<String, usize>,
) -> Vec<SkillMetadata> {
    let loaded_skills = TestLookup {
        skills: skills.to_vec(),
        disabled_paths: disabled_paths.clone(),
        ..Default::default()
    };
    collect_explicit_skill_mentions(inputs, &loaded_skills, connector_slug_counts)
}

fn skill_outcome_with_discovery_path(skill: SkillMetadata, discovery_path: &str) -> TestLookup {
    TestLookup {
        skill_discovery_path_by_path: HashMap::from([(
            skill.path_to_skills_md.clone(),
            test_path_buf(discovery_path).abs(),
        )]),
        skills: vec![skill],
        ..Default::default()
    }
}

#[test]
fn collect_explicit_skill_mentions_text_respects_skill_order() {
    let alpha = make_skill("alpha-skill", "/tmp/alpha");
    let beta = make_skill("beta-skill", "/tmp/beta");
    let skills = vec![beta.clone(), alpha.clone()];
    let inputs = vec![UserInput::Text {
        text: "first $alpha-skill then $beta-skill".to_string(),
        text_elements: Vec::new(),
    }];
    let connector_counts = HashMap::new();

    let selected = collect_mentions(&inputs, &skills, &HashSet::new(), &connector_counts);

    // Text scanning should not change the previous selection ordering semantics.
    assert_eq!(selected, vec![beta, alpha]);
}

#[test]
fn collect_explicit_skill_mentions_prioritizes_structured_inputs() {
    let alpha = make_skill("alpha-skill", "/tmp/alpha");
    let beta = make_skill("beta-skill", "/tmp/beta");
    let skills = vec![alpha.clone(), beta.clone()];
    let inputs = vec![
        UserInput::Text {
            text: "please run $alpha-skill".to_string(),
            text_elements: Vec::new(),
        },
        UserInput::Skill {
            name: "beta-skill".to_string(),
            path: test_path_buf("/tmp/beta"),
        },
    ];
    let connector_counts = HashMap::new();

    let selected = collect_mentions(&inputs, &skills, &HashSet::new(), &connector_counts);

    assert_eq!(selected, vec![beta, alpha]);
}

#[test]
fn collect_explicit_skill_mentions_accepts_structured_discovery_path() {
    let skill = make_skill("linked-skill", "/tmp/shared/linked-skill/SKILL.md");
    let loaded_skills = skill_outcome_with_discovery_path(
        skill.clone(),
        "/tmp/project/.agents/skills/linked-skill/SKILL.md",
    );
    let inputs = vec![UserInput::Skill {
        name: "linked-skill".to_string(),
        path: test_path_buf("/tmp/project/.agents/skills/linked-skill/SKILL.md"),
    }];

    let selected = collect_explicit_skill_mentions(&inputs, &loaded_skills, &HashMap::new());

    assert_eq!(selected, vec![skill]);
}

#[test]
fn collect_explicit_skill_mentions_accepts_linked_discovery_path() {
    let skill = make_skill("linked-skill", "/tmp/shared/linked-skill/SKILL.md");
    let loaded_skills = skill_outcome_with_discovery_path(
        skill.clone(),
        "/tmp/project/.agents/skills/linked-skill/SKILL.md",
    );
    let inputs = vec![UserInput::Text {
        text: linked_skill_mention(
            "linked-skill",
            "/tmp/project/.agents/skills/linked-skill/SKILL.md",
        ),
        text_elements: Vec::new(),
    }];

    let selected = collect_explicit_skill_mentions(&inputs, &loaded_skills, &HashMap::new());

    assert_eq!(selected, vec![skill]);
}

#[test]
fn collect_explicit_skill_mentions_rejects_disabled_discovery_path() {
    let skill = make_skill("linked-skill", "/tmp/shared/linked-skill/SKILL.md");
    let mut loaded_skills = skill_outcome_with_discovery_path(
        skill.clone(),
        "/tmp/project/.agents/skills/linked-skill/SKILL.md",
    );
    loaded_skills.disabled_paths.insert(skill.path_to_skills_md);
    let inputs = vec![UserInput::Skill {
        name: "linked-skill".to_string(),
        path: test_path_buf("/tmp/project/.agents/skills/linked-skill/SKILL.md"),
    }];

    let selected = collect_explicit_skill_mentions(&inputs, &loaded_skills, &HashMap::new());

    assert_eq!(selected, Vec::new());
}

#[test]
fn collect_explicit_skill_mentions_skips_invalid_structured_and_blocks_plain_fallback() {
    let alpha = make_skill("alpha-skill", "/tmp/alpha");
    let skills = vec![alpha];
    let inputs = vec![
        UserInput::Text {
            text: "please run $alpha-skill".to_string(),
            text_elements: Vec::new(),
        },
        UserInput::Skill {
            name: "alpha-skill".to_string(),
            path: test_path_buf("/tmp/missing"),
        },
    ];
    let connector_counts = HashMap::new();

    let selected = collect_mentions(&inputs, &skills, &HashSet::new(), &connector_counts);

    assert_eq!(selected, Vec::new());
}

#[test]
fn collect_explicit_skill_mentions_skips_disabled_structured_and_blocks_plain_fallback() {
    let alpha = make_skill("alpha-skill", "/tmp/alpha");
    let skills = vec![alpha];
    let inputs = vec![
        UserInput::Text {
            text: "please run $alpha-skill".to_string(),
            text_elements: Vec::new(),
        },
        UserInput::Skill {
            name: "alpha-skill".to_string(),
            path: test_path_buf("/tmp/alpha"),
        },
    ];
    let disabled = HashSet::from([test_path_buf("/tmp/alpha").abs()]);
    let connector_counts = HashMap::new();

    let selected = collect_mentions(&inputs, &skills, &disabled, &connector_counts);

    assert_eq!(selected, Vec::new());
}

#[test]
fn collect_explicit_skill_mentions_dedupes_by_path() {
    let alpha = make_skill("alpha-skill", "/tmp/alpha");
    let skills = vec![alpha.clone()];
    let mention = linked_skill_mention("alpha-skill", "/tmp/alpha");
    let inputs = vec![UserInput::Text {
        text: format!("use {mention} and {mention}"),
        text_elements: Vec::new(),
    }];
    let connector_counts = HashMap::new();

    let selected = collect_mentions(&inputs, &skills, &HashSet::new(), &connector_counts);

    assert_eq!(selected, vec![alpha]);
}

#[test]
fn collect_explicit_skill_mentions_skips_ambiguous_name() {
    let alpha = make_skill("demo-skill", "/tmp/alpha");
    let beta = make_skill("demo-skill", "/tmp/beta");
    let skills = vec![alpha, beta];
    let inputs = vec![UserInput::Text {
        text: "use $demo-skill and again $demo-skill".to_string(),
        text_elements: Vec::new(),
    }];
    let connector_counts = HashMap::new();

    let selected = collect_mentions(&inputs, &skills, &HashSet::new(), &connector_counts);

    assert_eq!(selected, Vec::new());
}

#[test]
fn availability_filter_resolves_name_against_available_skills_only() {
    let mut available = make_skill("demo-skill", "/tmp/available-demo");
    available.tags = vec!["frontend".to_string()];
    let mut gated = make_skill("demo-skill", "/tmp/gated-demo");
    gated.tags = vec!["backend".to_string()];
    let skills = vec![available.clone(), gated];
    let inputs = vec![UserInput::Text {
        text: "use $demo-skill".to_string(),
        text_elements: Vec::new(),
    }];
    let lookup = TestLookup {
        skills,
        ..Default::default()
    };

    let result = collect_explicit_skill_mentions_with_availability(
        &inputs,
        &lookup,
        &HashMap::new(),
        |skill| skill.tags.iter().any(|tag| tag == "frontend"),
    );

    assert_eq!(result.selected, vec![available]);
    assert!(result.unavailable_skill_names.is_empty());
}

#[test]
fn availability_filter_reports_gated_text_and_structured_references() {
    let mut gated = make_skill("gated-skill", "/tmp/gated-skill/SKILL.md");
    gated.tags = vec!["backend".to_string()];
    let lookup = TestLookup {
        skills: vec![gated],
        ..Default::default()
    };
    let is_available = |skill: &SkillMetadata| skill.tags.iter().any(|tag| tag == "frontend");

    let text_result = collect_explicit_skill_mentions_with_availability(
        &[UserInput::Text {
            text: "use $gated-skill".to_string(),
            text_elements: Vec::new(),
        }],
        &lookup,
        &HashMap::new(),
        is_available,
    );
    assert!(text_result.selected.is_empty());
    assert_eq!(text_result.unavailable_skill_names, vec!["gated-skill"]);

    let structured_result = collect_explicit_skill_mentions_with_availability(
        &[UserInput::Skill {
            name: "gated-skill".to_string(),
            path: test_path_buf("/tmp/gated-skill/SKILL.md"),
        }],
        &lookup,
        &HashMap::new(),
        |skill| skill.tags.iter().any(|tag| tag == "frontend"),
    );
    assert!(structured_result.selected.is_empty());
    assert_eq!(
        structured_result.unavailable_skill_names,
        vec!["gated-skill"]
    );
}

#[test]
fn legacy_explicit_skill_resolver_keeps_mode_off_ambiguity_behavior() {
    let mut available = make_skill("demo-skill", "/tmp/available-demo");
    available.tags = vec!["frontend".to_string()];
    let mut gated = make_skill("demo-skill", "/tmp/gated-demo");
    gated.tags = vec!["backend".to_string()];
    let lookup = TestLookup {
        skills: vec![available.clone(), gated],
        ..Default::default()
    };
    let inputs = vec![UserInput::Text {
        text: "use $demo-skill".to_string(),
        text_elements: Vec::new(),
    }];

    // The existing API has no capability predicate and retains its former behavior.
    assert!(collect_explicit_skill_mentions(&inputs, &lookup, &HashMap::new()).is_empty());

    let enabled = collect_explicit_skill_mentions_with_availability(
        &inputs,
        &lookup,
        &HashMap::new(),
        |skill| skill.tags.iter().any(|tag| tag == "frontend"),
    );
    assert_eq!(enabled.selected, vec![available]);
}

#[test]
fn collect_explicit_skill_mentions_prefers_linked_path_over_name() {
    let alpha = make_skill("demo-skill", "/tmp/alpha");
    let beta = make_skill("demo-skill", "/tmp/beta");
    let skills = vec![alpha, beta.clone()];
    let inputs = vec![UserInput::Text {
        text: format!(
            "use $demo-skill and {}",
            linked_skill_mention("demo-skill", "/tmp/beta")
        ),
        text_elements: Vec::new(),
    }];
    let connector_counts = HashMap::new();

    let selected = collect_mentions(&inputs, &skills, &HashSet::new(), &connector_counts);

    assert_eq!(selected, vec![beta]);
}

#[test]
fn collect_explicit_skill_mentions_skips_plain_name_when_connector_matches() {
    let alpha = make_skill("alpha-skill", "/tmp/alpha");
    let skills = vec![alpha];
    let inputs = vec![UserInput::Text {
        text: "use $alpha-skill".to_string(),
        text_elements: Vec::new(),
    }];
    let connector_counts = HashMap::from([("alpha-skill".to_string(), 1)]);

    let selected = collect_mentions(&inputs, &skills, &HashSet::new(), &connector_counts);

    assert_eq!(selected, Vec::new());
}

#[test]
fn collect_explicit_skill_mentions_allows_explicit_path_with_connector_conflict() {
    let alpha = make_skill("alpha-skill", "/tmp/alpha");
    let skills = vec![alpha.clone()];
    let inputs = vec![UserInput::Text {
        text: format!("use {}", linked_skill_mention("alpha-skill", "/tmp/alpha")),
        text_elements: Vec::new(),
    }];
    let connector_counts = HashMap::from([("alpha-skill".to_string(), 1)]);

    let selected = collect_mentions(&inputs, &skills, &HashSet::new(), &connector_counts);

    assert_eq!(selected, vec![alpha]);
}

#[test]
fn collect_explicit_skill_mentions_skips_when_linked_path_disabled() {
    let alpha = make_skill("demo-skill", "/tmp/alpha");
    let beta = make_skill("demo-skill", "/tmp/beta");
    let skills = vec![alpha, beta];
    let inputs = vec![UserInput::Text {
        text: format!("use {}", linked_skill_mention("demo-skill", "/tmp/alpha")),
        text_elements: Vec::new(),
    }];
    let disabled = HashSet::from([test_path_buf("/tmp/alpha").abs()]);
    let connector_counts = HashMap::new();

    let selected = collect_mentions(&inputs, &skills, &disabled, &connector_counts);

    assert_eq!(selected, Vec::new());
}

#[test]
fn collect_explicit_skill_mentions_prefers_resource_path() {
    let alpha = make_skill("demo-skill", "/tmp/alpha");
    let beta = make_skill("demo-skill", "/tmp/beta");
    let skills = vec![alpha, beta.clone()];
    let inputs = vec![UserInput::Text {
        text: format!("use {}", linked_skill_mention("demo-skill", "/tmp/beta")),
        text_elements: Vec::new(),
    }];
    let connector_counts = HashMap::new();

    let selected = collect_mentions(&inputs, &skills, &HashSet::new(), &connector_counts);

    assert_eq!(selected, vec![beta]);
}

#[test]
fn collect_explicit_skill_mentions_skips_missing_path_with_no_fallback() {
    let alpha = make_skill("demo-skill", "/tmp/alpha");
    let beta = make_skill("demo-skill", "/tmp/beta");
    let skills = vec![alpha, beta];
    let inputs = vec![UserInput::Text {
        text: format!("use {}", linked_skill_mention("demo-skill", "/tmp/missing")),
        text_elements: Vec::new(),
    }];
    let connector_counts = HashMap::new();

    let selected = collect_mentions(&inputs, &skills, &HashSet::new(), &connector_counts);

    assert_eq!(selected, Vec::new());
}

#[test]
fn collect_explicit_skill_mentions_skips_missing_path_without_fallback() {
    let alpha = make_skill("demo-skill", "/tmp/alpha");
    let skills = vec![alpha];
    let inputs = vec![UserInput::Text {
        text: format!("use {}", linked_skill_mention("demo-skill", "/tmp/missing")),
        text_elements: Vec::new(),
    }];
    let connector_counts = HashMap::new();

    let selected = collect_mentions(&inputs, &skills, &HashSet::new(), &connector_counts);

    assert_eq!(selected, Vec::new());
}
