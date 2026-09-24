use std::collections::HashSet;
use std::sync::Arc;

use pretty_assertions::assert_eq;
use tokio::sync::OnceCell;

use crate::SkillsExtensionConfig;
use crate::catalog::SkillAuthority;
use crate::catalog::SkillCatalog;
use crate::catalog::SkillCatalogEntry;
use crate::catalog::SkillPackageId;
use crate::catalog::SkillResourceId;
use crate::catalog::SkillSourceKind;
use crate::provider::SkillListQuery;
use crate::shadow_selection_experiment::ShadowSelectionExperiment;
use crate::sources::SkillProviders;
use crate::state::SkillsThreadState;

use super::SkillToolAuthoritySelector;
use super::SkillToolContext;

#[tokio::test]
async fn tool_catalog_refilters_cached_source_after_capability_grant() {
    let thread_state = Arc::new(SkillsThreadState::new(
        SkillsExtensionConfig {
            include_instructions: true,
            max_context_tokens: None,
            bundled_skills_enabled: false,
            cloud_skill_enabled: true,
            shadow_selection_enabled: false,
        },
        /*cloud_skills_available*/ true,
    ));
    thread_state.set_capex_filter(Some((HashSet::new(), HashSet::new())));

    let source_catalog = SkillCatalog {
        entries: vec![
            SkillCatalogEntry::new(
                SkillPackageId("/skills/design".into()),
                SkillAuthority::new(SkillSourceKind::Executor, "environment"),
                "design",
                "Design skill",
                SkillResourceId::new("skill://design/SKILL.md"),
            )
            .with_tags(vec!["frontend".into()]),
        ],
        warnings: Vec::new(),
    };
    let executor_catalog = Arc::new(OnceCell::new());
    executor_catalog
        .set(source_catalog.clone())
        .expect("the source catalog cache is initially empty");
    let context = SkillToolContext {
        providers: SkillProviders::new(),
        mcp_resources: None,
        thread_state: Arc::clone(&thread_state),
        analytics: None,
        cloud_available: false,
        executor_query: Some(SkillListQuery {
            turn_id: "turn".into(),
            executor_roots: Vec::new(),
            resolved_executor_roots: Vec::new(),
            host_snapshot: None,
            include_host_skills: false,
            include_bundled_skills: false,
            include_cloud_skills: false,
            mcp_resources: None,
            executor_capability_discovery: None,
        }),
        selected_plugins: None,
        sandbox_contexts: None,
        executor_catalog: Arc::clone(&executor_catalog),
        shadow_selection: Arc::new(ShadowSelectionExperiment::new(None)),
    };

    let initially_filtered = context
        .catalog("turn", SkillToolAuthoritySelector::Executor)
        .await;
    assert_eq!(initially_filtered.entries[0].enabled, false);
    assert_eq!(executor_catalog.get(), Some(&source_catalog));

    thread_state.set_capex_filter(Some((
        HashSet::from(["frontend".to_string()]),
        HashSet::new(),
    )));
    let newly_available = context
        .catalog("turn", SkillToolAuthoritySelector::Executor)
        .await;

    assert_eq!(newly_available.entries[0].enabled, true);
    assert_eq!(executor_catalog.get(), Some(&source_catalog));
}
