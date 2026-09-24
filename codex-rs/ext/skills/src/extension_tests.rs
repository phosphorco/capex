use std::sync::Mutex;

use codex_extension_api::ExtensionMetrics;
use codex_otel::THREAD_SKILLS_DESCRIPTION_TRUNCATED_CHARS_METRIC;
use codex_otel::THREAD_SKILLS_ENABLED_TOTAL_METRIC;
use codex_otel::THREAD_SKILLS_KEPT_TOTAL_METRIC;
use codex_otel::THREAD_SKILLS_TRUNCATED_METRIC;
use pretty_assertions::assert_eq;

use super::*;

#[derive(Default)]
struct RecordingMetrics {
    samples: Mutex<Vec<(String, i64)>>,
}

impl ExtensionMetrics for RecordingMetrics {
    fn histogram_with_boundaries(
        &self,
        name: &str,
        value: i64,
        _boundaries: &[f64],
        tags: &[(&str, &str)],
    ) {
        self.histogram(name, value, tags);
    }

    fn counter(&self, name: &str, _inc: i64, _tags: &[(&str, &str)]) {
        panic!("unexpected counter: {name}");
    }

    fn histogram(&self, name: &str, value: i64, _tags: &[(&str, &str)]) {
        self.samples
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push((name.to_string(), value));
    }
}

#[test]
fn empty_catalog_records_zero_metrics_without_a_fragment() {
    let metrics = RecordingMetrics::default();

    let rendered = render_catalog(
        Some(&metrics),
        CatalogSurface::ThreadContext,
        &SkillCatalog::default(),
        /*include_skills_usage_instructions*/ false,
        SkillCatalogRenderPolicy::ExtensionCompatible,
        SkillMetadataBudget::Characters(8_000),
    );

    assert!(rendered.fragment.is_none());
    assert_eq!(rendered.warning_message, None);

    assert_eq!(
        *metrics
            .samples
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner),
        vec![
            (THREAD_SKILLS_ENABLED_TOTAL_METRIC.to_string(), 0),
            (THREAD_SKILLS_KEPT_TOTAL_METRIC.to_string(), 0),
            (THREAD_SKILLS_TRUNCATED_METRIC.to_string(), 0),
            (
                THREAD_SKILLS_DESCRIPTION_TRUNCATED_CHARS_METRIC.to_string(),
                0,
            ),
        ]
    );
}

#[test]
fn shadow_selection_catalog_refilters_raw_host_skills_after_merge() {
    let thread_state = SkillsThreadState::new(
        SkillsExtensionConfig {
            include_instructions: true,
            max_context_tokens: None,
            bundled_skills_enabled: false,
            cloud_skill_enabled: false,
            shadow_selection_enabled: true,
        },
        /*cloud_skills_available*/ false,
    );
    thread_state.set_capex_filter(Some((
        std::collections::HashSet::from(["frontend".to_string()]),
        std::collections::HashSet::new(),
    )));
    let host_catalog = SkillCatalog {
        entries: vec![
            crate::catalog::SkillCatalogEntry::new(
                crate::catalog::SkillPackageId("/skills/frontend".to_string()),
                crate::catalog::SkillAuthority::new(crate::catalog::SkillSourceKind::Host, "host"),
                "frontend",
                "Frontend skill",
                crate::catalog::SkillResourceId::new("/skills/frontend/SKILL.md"),
            )
            .with_tags(vec!["frontend".to_string()]),
            crate::catalog::SkillCatalogEntry::new(
                crate::catalog::SkillPackageId("/skills/backend".to_string()),
                crate::catalog::SkillAuthority::new(crate::catalog::SkillSourceKind::Host, "host"),
                "backend",
                "Backend skill",
                crate::catalog::SkillResourceId::new("/skills/backend/SKILL.md"),
            )
            .with_tags(vec!["backend".to_string()]),
        ],
        warnings: Vec::new(),
    };

    let shadow_catalog = super::shadow_selection_catalog(
        &thread_state,
        &SkillCatalog::default(),
        Some(&host_catalog),
    );

    assert!(
        shadow_catalog
            .entries
            .iter()
            .find(|entry| entry.name == "frontend")
            .is_some_and(|entry| entry.is_model_visible())
    );
    assert!(
        shadow_catalog
            .entries
            .iter()
            .find(|entry| entry.name == "backend")
            .is_some_and(|entry| !entry.is_model_visible())
    );
}
