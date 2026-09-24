use std::cell::Cell;
use std::collections::BTreeSet;

use codex_protocol::ThreadId;
use pretty_assertions::assert_eq;
use tempfile::tempdir;

use super::CapabilityGrantSnapshot;
use super::CapabilityStateLoadMode;
use super::load_capability_state;
use super::persist_fork_snapshots;
use super::read_existing_capability_grants;

fn grant(id: &str, tag: &str, instructions: &str) -> CapabilityGrantSnapshot {
    grant_at(id, tag, instructions, None)
}

fn grant_at(
    id: &str,
    tag: &str,
    instructions: &str,
    origin_turn_id: Option<&str>,
) -> CapabilityGrantSnapshot {
    CapabilityGrantSnapshot {
        id: id.to_string(),
        tags: BTreeSet::from([tag.to_string()]),
        instructions: instructions.to_string(),
        origin_turn_id: origin_turn_id.map(str::to_string),
    }
}

#[test]
fn fresh_session_persists_initial_grant_snapshots_before_returning() {
    let codex_home = tempdir().expect("temporary Codex home");
    let thread_id = ThreadId::from_u128(1);
    let initial = grant("baseline", "core", "baseline instructions");

    let loaded = load_capability_state(
        codex_home.path(),
        thread_id,
        CapabilityStateLoadMode::Fresh,
        || Ok(vec![initial.clone()]),
    );

    assert_eq!(
        loaded.state.grants().cloned().collect::<Vec<_>>(),
        vec![initial.clone()]
    );
    assert!(loaded.diagnostic.is_none());

    let resolver_called = Cell::new(false);
    let reloaded = load_capability_state(
        codex_home.path(),
        thread_id,
        CapabilityStateLoadMode::Resume,
        || {
            resolver_called.set(true);
            Ok(vec![grant("baseline", "changed", "changed definition")])
        },
    );
    assert_eq!(
        reloaded.state.grants().cloned().collect::<Vec<_>>(),
        vec![grant("baseline", "core", "baseline instructions")]
    );
    assert!(reloaded.diagnostic.is_none());
    assert!(!resolver_called.get());
}

#[test]
fn fresh_session_replaces_existing_sidecar_grants() {
    let codex_home = tempdir().expect("temporary Codex home");
    let thread_id = ThreadId::from_u128(9);
    let previous_initial = grant("baseline", "old", "old baseline instructions");
    let mut previous = load_capability_state(
        codex_home.path(),
        thread_id,
        CapabilityStateLoadMode::Fresh,
        || Ok(vec![previous_initial]),
    )
    .state;
    previous
        .grant(grant_at(
            "frontend",
            "ui",
            "dynamic instructions",
            Some("turn-1"),
        ))
        .expect("persist dynamic grant");

    let new_initial = grant("baseline", "new", "new baseline instructions");
    let fresh = load_capability_state(
        codex_home.path(),
        thread_id,
        CapabilityStateLoadMode::Fresh,
        || Ok(vec![new_initial.clone()]),
    );

    assert_eq!(
        fresh.state.grants().cloned().collect::<Vec<_>>(),
        vec![new_initial.clone()]
    );
    let resumed = load_capability_state(
        codex_home.path(),
        thread_id,
        CapabilityStateLoadMode::Resume,
        || Ok(Vec::new()),
    );
    assert_eq!(
        resumed.state.grants().cloned().collect::<Vec<_>>(),
        vec![new_initial]
    );
}

#[test]
fn fresh_session_does_not_expose_initial_grants_when_persistence_fails() {
    let codex_home = tempdir().expect("temporary Codex home");
    let thread_id = ThreadId::from_u128(10);
    std::fs::write(codex_home.path().join("capex"), "not a directory")
        .expect("create invalid sidecar directory");

    let loaded = load_capability_state(
        codex_home.path(),
        thread_id,
        CapabilityStateLoadMode::Fresh,
        || Ok(vec![grant("baseline", "core", "baseline instructions")]),
    );

    assert_eq!(loaded.state.grants().count(), 0);
    assert!(
        loaded
            .diagnostic
            .as_deref()
            .is_some_and(|message| message.contains("no grants activated"))
    );
}

#[test]
fn invalid_fresh_initial_definitions_clear_old_grants_and_fail_closed() {
    let codex_home = tempdir().expect("temporary Codex home");
    let thread_id = ThreadId::from_u128(11);
    let baseline = grant("baseline", "core", "baseline instructions");
    let mut previous = load_capability_state(
        codex_home.path(),
        thread_id,
        CapabilityStateLoadMode::Fresh,
        || Ok(vec![baseline.clone()]),
    )
    .state;
    previous
        .grant(grant_at(
            "frontend",
            "ui",
            "dynamic instructions",
            Some("turn-1"),
        ))
        .expect("persist prior dynamic grant");

    let fresh = load_capability_state(
        codex_home.path(),
        thread_id,
        CapabilityStateLoadMode::Fresh,
        || Err("invalid capability frontmatter".to_string()),
    );
    assert_eq!(fresh.state.grants().count(), 0);
    assert!(
        fresh
            .diagnostic
            .as_deref()
            .is_some_and(|message| message.contains("invalid capability frontmatter"))
    );

    let resumed = load_capability_state(
        codex_home.path(),
        thread_id,
        CapabilityStateLoadMode::Resume,
        || Ok(vec![baseline]),
    );
    assert_eq!(resumed.state.grants().count(), 0);
    assert!(resumed.diagnostic.is_none());
}

#[test]
fn missing_resumed_sidecar_uses_initial_grants_and_reports_diagnostic() {
    let codex_home = tempdir().expect("temporary Codex home");
    let thread_id = ThreadId::from_u128(7);
    let initial = grant("baseline", "core", "baseline instructions");

    let loaded = load_capability_state(
        codex_home.path(),
        thread_id,
        CapabilityStateLoadMode::Resume,
        || Ok(vec![initial.clone()]),
    );

    assert_eq!(
        loaded.state.grants().cloned().collect::<Vec<_>>(),
        vec![initial.clone()]
    );
    assert!(
        loaded
            .diagnostic
            .as_deref()
            .is_some_and(|message| message.contains("is missing"))
    );

    let reloaded = load_capability_state(
        codex_home.path(),
        thread_id,
        CapabilityStateLoadMode::Resume,
        || Ok(vec![grant("baseline", "changed", "changed definition")]),
    );
    assert_eq!(
        reloaded.state.grants().cloned().collect::<Vec<_>>(),
        vec![initial]
    );
    assert!(reloaded.diagnostic.is_none());
}

#[test]
fn fail_closed_load_does_not_fallback_or_create_a_sidecar() {
    let codex_home = tempdir().expect("temporary Codex home");
    let thread_id = ThreadId::from_u128(14);
    let resolver_called = Cell::new(false);

    let loaded = load_capability_state(
        codex_home.path(),
        thread_id,
        CapabilityStateLoadMode::FailClosed,
        || {
            resolver_called.set(true);
            Ok(vec![grant("baseline", "core", "must not be activated")])
        },
    );

    assert_eq!(loaded.state.grants().count(), 0);
    assert!(loaded.diagnostic.as_deref().is_some_and(
        |message| message.contains("is missing") && message.contains("no grants activated")
    ));
    assert!(!resolver_called.get());
    assert!(
        !codex_home
            .path()
            .join("capex")
            .join(format!("{thread_id}.json"))
            .exists()
    );
}

#[test]
fn fail_closed_load_restores_valid_sidecar_without_calling_fallback() {
    let codex_home = tempdir().expect("temporary Codex home");
    let thread_id = ThreadId::from_u128(15);
    let saved = grant("frontend", "ui", "fork snapshot");
    persist_fork_snapshots(codex_home.path(), thread_id, std::slice::from_ref(&saved))
        .expect("persist fork snapshot");
    let resolver_called = Cell::new(false);

    let loaded = load_capability_state(
        codex_home.path(),
        thread_id,
        CapabilityStateLoadMode::FailClosed,
        || {
            resolver_called.set(true);
            Ok(vec![grant("baseline", "core", "must not be activated")])
        },
    );

    assert_eq!(
        loaded.state.grants().cloned().collect::<Vec<_>>(),
        vec![saved]
    );
    assert!(loaded.diagnostic.is_none());
    assert!(!resolver_called.get());
}

#[test]
fn fail_closed_load_preserves_corrupt_sidecar_and_activates_nothing() {
    let codex_home = tempdir().expect("temporary Codex home");
    let thread_id = ThreadId::from_u128(16);
    let sidecar = codex_home
        .path()
        .join("capex")
        .join(format!("{thread_id}.json"));
    std::fs::create_dir_all(sidecar.parent().expect("sidecar parent"))
        .expect("create sidecar directory");
    std::fs::write(&sidecar, b"not valid state").expect("write corrupt sidecar");
    let resolver_called = Cell::new(false);

    let loaded = load_capability_state(
        codex_home.path(),
        thread_id,
        CapabilityStateLoadMode::FailClosed,
        || {
            resolver_called.set(true);
            Ok(vec![grant("baseline", "core", "must not be activated")])
        },
    );

    assert_eq!(loaded.state.grants().count(), 0);
    assert!(loaded.diagnostic.is_some());
    assert!(!resolver_called.get());
    assert_eq!(
        std::fs::read(&sidecar).expect("read corrupt sidecar"),
        b"not valid state"
    );
}

#[test]
fn reading_missing_parent_state_does_not_create_a_sidecar() {
    let codex_home = tempdir().expect("temporary Codex home");
    let thread_id = ThreadId::from_u128(12);

    assert_eq!(
        read_existing_capability_grants(codex_home.path(), thread_id)
            .expect("read missing parent state"),
        None
    );
    assert!(!codex_home.path().join("capex").exists());
}

#[test]
fn empty_fork_selection_is_persisted_as_a_valid_empty_sidecar() {
    let codex_home = tempdir().expect("temporary Codex home");
    let child_thread_id = ThreadId::from_u128(13);

    persist_fork_snapshots(codex_home.path(), child_thread_id, &[])
        .expect("persist empty fork selection");

    assert_eq!(
        read_existing_capability_grants(codex_home.path(), child_thread_id)
            .expect("read fork sidecar"),
        Some(Vec::new())
    );
}

#[test]
fn corrupt_sidecar_falls_back_as_a_whole_to_initial_grants() {
    let codex_home = tempdir().expect("temporary Codex home");
    let thread_id = ThreadId::from_u128(2);
    let initial = grant("baseline", "core", "initial body");
    let sidecar = codex_home
        .path()
        .join("capex")
        .join(format!("{thread_id}.json"));
    std::fs::create_dir_all(sidecar.parent().expect("sidecar parent"))
        .expect("create sidecar directory");
    std::fs::write(
        &sidecar,
        br#"{"version":1,"grants":[{"id":"partial","tags":[],"instructions":"bad"}]"#,
    )
    .expect("write malformed sidecar");

    let loaded = load_capability_state(
        codex_home.path(),
        thread_id,
        CapabilityStateLoadMode::Resume,
        || Ok(vec![initial.clone()]),
    );

    assert_eq!(
        loaded.state.grants().cloned().collect::<Vec<_>>(),
        vec![initial.clone()]
    );
    assert!(
        loaded
            .diagnostic
            .as_deref()
            .is_some_and(|message| message.contains("is invalid"))
    );

    let reloaded = load_capability_state(
        codex_home.path(),
        thread_id,
        CapabilityStateLoadMode::Resume,
        || Ok(vec![grant("baseline", "changed", "changed definition")]),
    );
    assert_eq!(
        reloaded.state.grants().cloned().collect::<Vec<_>>(),
        vec![initial]
    );
    assert!(reloaded.diagnostic.is_none());
}

#[test]
fn grants_are_atomic_idempotent_and_reloaded_from_the_sidecar() {
    let codex_home = tempdir().expect("temporary Codex home");
    let thread_id = ThreadId::from_u128(3);
    let initial = grant("baseline", "core", "baseline instructions");
    let mut loaded = load_capability_state(
        codex_home.path(),
        thread_id,
        CapabilityStateLoadMode::Fresh,
        || Ok(vec![initial.clone()]),
    );
    let first = grant_at("frontend", "ui", "frontend instructions", Some("turn-1"));

    assert!(
        loaded
            .state
            .grant(first.clone())
            .expect("persist first grant")
    );
    assert!(
        !loaded
            .state
            .grant(grant_at(
                "frontend",
                "different",
                "replacement",
                Some("turn-2"),
            ))
            .expect("duplicate grant is idempotent")
    );
    assert_eq!(
        loaded.state.grants().cloned().collect::<Vec<_>>(),
        vec![initial.clone(), first.clone()]
    );

    let reloaded = load_capability_state(
        codex_home.path(),
        thread_id,
        CapabilityStateLoadMode::Resume,
        || Ok(vec![initial.clone()]),
    );
    assert_eq!(
        reloaded.state.grants().cloned().collect::<Vec<_>>(),
        vec![initial, first]
    );
    assert!(reloaded.diagnostic.is_none());
}

#[test]
fn fork_persists_only_the_grants_selected_at_its_branch_point() {
    let codex_home = tempdir().expect("temporary Codex home");
    let parent_thread = ThreadId::from_u128(4);
    let fork_thread = ThreadId::from_u128(5);
    let initial = grant("baseline", "core", "baseline instructions");
    let mut parent = load_capability_state(
        codex_home.path(),
        parent_thread,
        CapabilityStateLoadMode::Fresh,
        || Ok(vec![initial.clone()]),
    )
    .state;
    parent
        .grant(grant_at(
            "frontend",
            "ui",
            "frontend instructions",
            Some("turn-1"),
        ))
        .expect("persist parent grant");
    parent
        .grant(grant_at(
            "backend",
            "server",
            "later parent grant",
            Some("turn-2"),
        ))
        .expect("persist later parent grant");
    let branch_point_grants = parent
        .grants()
        .filter(|grant| grant.origin_turn_id.as_deref() != Some("turn-2"))
        .cloned()
        .collect::<Vec<_>>();

    parent
        .persist_snapshot_for_fork(fork_thread, &branch_point_grants)
        .expect("persist branch-point snapshot");

    let fork = load_capability_state(
        codex_home.path(),
        fork_thread,
        CapabilityStateLoadMode::Resume,
        || Ok(Vec::new()),
    )
    .state;
    assert_eq!(
        fork.grants().cloned().collect::<Vec<_>>(),
        vec![
            initial,
            grant_at("frontend", "ui", "frontend instructions", Some("turn-1"))
        ]
    );
}

#[test]
fn active_tags_are_the_deduplicated_union_of_grant_snapshots() {
    let codex_home = tempdir().expect("temporary Codex home");
    let thread_id = ThreadId::from_u128(8);
    let initial = CapabilityGrantSnapshot {
        id: "baseline".to_string(),
        tags: BTreeSet::from(["core".to_string(), "shared".to_string()]),
        instructions: "baseline instructions".to_string(),
        origin_turn_id: None,
    };
    let mut state = load_capability_state(
        codex_home.path(),
        thread_id,
        CapabilityStateLoadMode::Fresh,
        || Ok(vec![initial]),
    )
    .state;
    state
        .grant(CapabilityGrantSnapshot {
            id: "frontend".to_string(),
            tags: BTreeSet::from(["ui".to_string(), "shared".to_string()]),
            instructions: "frontend instructions".to_string(),
            origin_turn_id: Some("turn-1".to_string()),
        })
        .expect("persist frontend grant");

    assert_eq!(
        state.active_tags(),
        BTreeSet::from(["core".to_string(), "shared".to_string(), "ui".to_string()])
    );
}

#[test]
fn invalid_grant_does_not_change_memory_or_disk() {
    let codex_home = tempdir().expect("temporary Codex home");
    let thread_id = ThreadId::from_u128(6);
    let initial = grant("baseline", "core", "baseline instructions");
    let mut loaded = load_capability_state(
        codex_home.path(),
        thread_id,
        CapabilityStateLoadMode::Fresh,
        || Ok(vec![initial.clone()]),
    );
    let before = loaded.state.grants().cloned().collect::<Vec<_>>();
    let sidecar = codex_home
        .path()
        .join("capex")
        .join(format!("{thread_id}.json"));
    let before_bytes = std::fs::read(&sidecar).expect("read initial sidecar");

    assert!(
        loaded
            .state
            .grant(grant("../escape", "bad", "must not be persisted"))
            .is_err()
    );
    assert_eq!(loaded.state.grants().cloned().collect::<Vec<_>>(), before);
    assert!(!codex_home.path().join("escape.json").exists());
    assert_eq!(std::fs::read(&sidecar).expect("read sidecar"), before_bytes);
}
