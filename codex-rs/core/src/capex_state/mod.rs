//! Bounded, per-thread storage for CapEx capability grants.
//!
//! This state is deliberately stored beside Codex's normal session data, never in a rollout.
//! A grant is committed atomically before it becomes visible through [`CapabilitySessionState`].

use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::fs::File;
use std::io::Read;
use std::path::Path;
use std::path::PathBuf;

use codex_protocol::ThreadId;
use serde::Deserialize;
use serde::Serialize;
use thiserror::Error;

const FORMAT_VERSION: u32 = 1;
const MAX_SIDECAR_BYTES: usize = 1024 * 1024;
const MAX_GRANTS: usize = 64;
const MAX_ID_BYTES: usize = 128;
const MAX_TAGS_PER_GRANT: usize = 64;
const MAX_TAG_BYTES: usize = 128;
const MAX_INSTRUCTION_BYTES: usize = 8 * 1024;
const MAX_TOTAL_INSTRUCTION_BYTES: usize = 128 * 1024;

/// A capability's stable identity and immutable instruction snapshot at grant time.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CapabilityGrantSnapshot {
    pub(crate) id: String,
    pub(crate) tags: BTreeSet<String>,
    pub(crate) instructions: String,
    /// Turn that caused this grant; configured initial grants have no originating turn.
    #[serde(default)]
    pub(crate) origin_turn_id: Option<String>,
}

/// The grants loaded for one thread, along with any recovery diagnostic for the caller to log.
pub(crate) struct CapabilityStateLoad {
    pub(crate) state: CapabilitySessionState,
    pub(crate) diagnostic: Option<String>,
}

/// Tells the loader whether a missing sidecar is expected for this lifecycle path.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CapabilityStateLoadMode {
    /// Starts a new or cleared thread by replacing any prior sidecar with initial snapshots.
    Fresh,
    /// Restores the thread's sidecar, using initial snapshots only for a missing/corrupt file.
    Resume,
    /// Restores a fork sidecar if available, but never falls back to new initial grants.
    FailClosed,
}

/// In-memory grants for a single thread, persisted in `codex_home/capex/<thread-id>.json`.
///
/// Callers should keep one instance per live thread. The runtime is expected to serialize
/// grant operations for a thread; atomic replacement protects readers from partial files.
pub(crate) struct CapabilitySessionState {
    codex_home: PathBuf,
    thread_id: ThreadId,
    grants: BTreeMap<String, CapabilityGrantSnapshot>,
}

#[derive(Debug, Error)]
pub(crate) enum CapabilityStateError {
    #[error("invalid capability grant `{id}`: {reason}")]
    InvalidGrant { id: String, reason: &'static str },
    #[error("capability state exceeds the supported size limits")]
    StateTooLarge,
    #[error("failed to encode capability state: {0}")]
    Encode(#[from] serde_json::Error),
    #[error("failed to persist capability state: {0}")]
    Persist(#[from] std::io::Error),
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct PersistedCapabilityState {
    version: u32,
    grants: Vec<CapabilityGrantSnapshot>,
}

struct SidecarReadError {
    diagnostic: String,
    missing: bool,
    replaceable: bool,
}

/// Load saved grants or initialize a new thread from configured snapshots.
///
/// Fresh loads replace any prior sidecar. Resume loads fall back to configured initial grants if
/// the sidecar is absent or structurally corrupt. Unknown or unreadable sidecars are preserved and
/// fail closed to an empty grant set. Initial snapshots are persisted before exposure; persistence
/// failure returns an empty grant set with a diagnostic.
pub(crate) fn load_capability_state(
    codex_home: &Path,
    thread_id: ThreadId,
    mode: CapabilityStateLoadMode,
    load_initial_grants: impl FnOnce() -> Result<Vec<CapabilityGrantSnapshot>, String>,
) -> CapabilityStateLoad {
    let path = sidecar_path(codex_home, thread_id);

    if mode == CapabilityStateLoadMode::FailClosed {
        return match read_state(&path) {
            Ok(grants) => CapabilityStateLoad {
                state: CapabilitySessionState {
                    codex_home: codex_home.to_path_buf(),
                    thread_id,
                    grants,
                },
                diagnostic: None,
            },
            Err(error) => CapabilityStateLoad {
                state: CapabilitySessionState {
                    codex_home: codex_home.to_path_buf(),
                    thread_id,
                    grants: BTreeMap::new(),
                },
                diagnostic: Some(format!("{}; no grants activated", error.diagnostic)),
            },
        };
    }

    if mode == CapabilityStateLoadMode::Fresh {
        let (initial_state, initial_diagnostic) = match load_initial_grants() {
            Ok(grants) => initial_state(grants),
            Err(error) => {
                let persist_diagnostic = persist_state(&path, &BTreeMap::new())
                    .err()
                    .map(|persist_error| {
                        format!(
                            "; could not clear prior capability sidecar {}: {persist_error}",
                            path.display()
                        )
                    })
                    .unwrap_or_default();
                return CapabilityStateLoad {
                    state: CapabilitySessionState {
                        codex_home: codex_home.to_path_buf(),
                        thread_id,
                        grants: BTreeMap::new(),
                    },
                    diagnostic: Some(format!(
                        "could not load configured initial capability grants: {error}; no grants activated{persist_diagnostic}"
                    )),
                };
            }
        };
        let (grants, persist_diagnostic) = match persist_state(&path, &initial_state) {
            Ok(()) => (initial_state, None),
            Err(error) => (
                BTreeMap::new(),
                Some(format!(
                    "could not persist initial capability snapshots for {}: {error}; no grants activated",
                    path.display()
                )),
            ),
        };
        return CapabilityStateLoad {
            state: CapabilitySessionState {
                codex_home: codex_home.to_path_buf(),
                thread_id,
                grants,
            },
            diagnostic: match (initial_diagnostic, persist_diagnostic) {
                (Some(initial), Some(persist)) => Some(format!("{initial}; {persist}")),
                (Some(diagnostic), None) | (None, Some(diagnostic)) => Some(diagnostic),
                (None, None) => None,
            },
        };
    }

    match read_state(&path) {
        Ok(grants) => CapabilityStateLoad {
            state: CapabilitySessionState {
                codex_home: codex_home.to_path_buf(),
                thread_id,
                grants,
            },
            diagnostic: None,
        },
        Err(error) if !error.replaceable => CapabilityStateLoad {
            state: CapabilitySessionState {
                codex_home: codex_home.to_path_buf(),
                thread_id,
                grants: BTreeMap::new(),
            },
            diagnostic: Some(format!("{}; no grants activated", error.diagnostic)),
        },
        Err(error) => {
            let (initial_state, initial_diagnostic) = match load_initial_grants() {
                Ok(grants) => initial_state(grants),
                Err(initial_error) => {
                    return CapabilityStateLoad {
                        state: CapabilitySessionState {
                            codex_home: codex_home.to_path_buf(),
                            thread_id,
                            grants: BTreeMap::new(),
                        },
                        diagnostic: Some(format!(
                            "{}; could not load configured initial capability grants: {initial_error}; no grants activated",
                            error.diagnostic
                        )),
                    };
                }
            };
            let (grants, persist_diagnostic) = match persist_state(&path, &initial_state) {
                Ok(()) => (initial_state, None),
                Err(persist_error) => (
                    BTreeMap::new(),
                    Some(format!(
                        "could not persist initial capability snapshots for {}: {persist_error}; no grants activated",
                        path.display()
                    )),
                ),
            };
            let fallback_diagnostic = Some(match initial_diagnostic {
                Some(initial_reason) => format!(
                    "{}; using configured initial capability grants ({initial_reason})",
                    error.diagnostic
                ),
                None => format!(
                    "{}; using configured initial capability grants",
                    error.diagnostic
                ),
            });
            CapabilityStateLoad {
                state: CapabilitySessionState {
                    codex_home: codex_home.to_path_buf(),
                    thread_id,
                    grants,
                },
                diagnostic: match (fallback_diagnostic, persist_diagnostic) {
                    (Some(fallback), Some(persist)) => Some(format!("{fallback}; {persist}")),
                    (Some(diagnostic), None) | (None, Some(diagnostic)) => Some(diagnostic),
                    (None, None) => None,
                },
            }
        }
    }
}

/// Reads a parent's snapshots without falling back to initial grants or creating a sidecar.
///
/// Fork callers use this to distinguish a parent with no CapEx state from one with a valid empty
/// grant set, then select only grants represented by the retained branch history.
pub(crate) fn read_existing_capability_grants(
    codex_home: &Path,
    thread_id: ThreadId,
) -> Result<Option<Vec<CapabilityGrantSnapshot>>, String> {
    match read_state(&sidecar_path(codex_home, thread_id)) {
        Ok(grants) => Ok(Some(grants.into_values().collect())),
        Err(error) if error.missing => Ok(None),
        Err(error) => Err(error.diagnostic),
    }
}

/// Atomically persists the exact snapshot selected for a fork, including an empty selection.
pub(crate) fn persist_fork_snapshots(
    codex_home: &Path,
    child_thread_id: ThreadId,
    selected_grants: &[CapabilityGrantSnapshot],
) -> Result<(), CapabilityStateError> {
    let mut grants = BTreeMap::new();
    for grant in selected_grants {
        validate_snapshot(grant)?;
        grants
            .entry(grant.id.clone())
            .or_insert_with(|| grant.clone());
    }
    persist_state(&sidecar_path(codex_home, child_thread_id), &grants)
}

impl CapabilitySessionState {
    /// Returns the accepted capability snapshots in stable ID order.
    pub(crate) fn grants(&self) -> impl Iterator<Item = &CapabilityGrantSnapshot> {
        self.grants.values()
    }

    /// Returns the deduplicated tags captured by the accepted grants.
    pub(crate) fn active_tags(&self) -> BTreeSet<String> {
        self.grants
            .values()
            .flat_map(|grant| grant.tags.iter().cloned())
            .collect()
    }

    /// Adds a capability grant and commits it before updating the in-memory state.
    ///
    /// The return value is `true` only for a newly accepted ID. A repeated ID is idempotent and
    /// retains the first snapshot, even if the caller supplies different content later.
    pub(crate) fn grant(
        &mut self,
        snapshot: CapabilityGrantSnapshot,
    ) -> Result<bool, CapabilityStateError> {
        validate_snapshot(&snapshot)?;
        if self.grants.contains_key(&snapshot.id) {
            return Ok(false);
        }

        let mut next_grants = self.grants.clone();
        next_grants.insert(snapshot.id.clone(), snapshot);
        validate_grants(&next_grants)?;
        persist_state(
            &sidecar_path(&self.codex_home, self.thread_id),
            &next_grants,
        )?;
        self.grants = next_grants;
        Ok(true)
    }

    /// Persists the selected branch-point grants under a fork's thread ID.
    ///
    /// The caller selects grants using the source rollout's turn ordering and cutoff. This
    /// explicit selection supports forks from an earlier turn without copying later grants.
    #[cfg(test)]
    pub(crate) fn persist_snapshot_for_fork(
        &self,
        fork_thread_id: ThreadId,
        selected_grants: &[CapabilityGrantSnapshot],
    ) -> Result<(), CapabilityStateError> {
        persist_fork_snapshots(&self.codex_home, fork_thread_id, selected_grants)
    }
}

fn sidecar_path(codex_home: &Path, thread_id: ThreadId) -> PathBuf {
    codex_home.join("capex").join(format!("{thread_id}.json"))
}

fn initial_state(
    initial_grants: impl IntoIterator<Item = CapabilityGrantSnapshot>,
) -> (BTreeMap<String, CapabilityGrantSnapshot>, Option<String>) {
    let mut grants = BTreeMap::new();
    let mut diagnostics = Vec::new();

    for grant in initial_grants {
        if let Err(error) = validate_snapshot(&grant) {
            diagnostics.push(error.to_string());
            continue;
        }
        grants.entry(grant.id.clone()).or_insert(grant);
    }

    if let Err(error) = validate_grants(&grants) {
        diagnostics.push(error.to_string());
        while validate_grants(&grants).is_err() {
            let Some(last_id) = grants.keys().next_back().cloned() else {
                break;
            };
            grants.remove(&last_id);
        }
    }

    let diagnostic = (!diagnostics.is_empty()).then(|| diagnostics.join("; "));
    (grants, diagnostic)
}

fn validate_snapshot(snapshot: &CapabilityGrantSnapshot) -> Result<(), CapabilityStateError> {
    let id = snapshot.id.as_str();
    let valid_id = !id.is_empty()
        && id.len() <= MAX_ID_BYTES
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
        && !id.starts_with('.')
        && id != "..";
    if !valid_id {
        return Err(CapabilityStateError::InvalidGrant {
            id: snapshot.id.clone(),
            reason: "ID must be a short path-safe name",
        });
    }

    if snapshot.tags.len() > MAX_TAGS_PER_GRANT
        || snapshot.tags.iter().any(|tag| {
            tag.is_empty() || tag.len() > MAX_TAG_BYTES || tag.chars().any(char::is_control)
        })
    {
        return Err(CapabilityStateError::InvalidGrant {
            id: snapshot.id.clone(),
            reason: "tags are empty, too long, or contain control characters",
        });
    }

    if snapshot.instructions.len() > MAX_INSTRUCTION_BYTES {
        return Err(CapabilityStateError::InvalidGrant {
            id: snapshot.id.clone(),
            reason: "instruction snapshot exceeds the per-capability byte limit",
        });
    }

    if snapshot.origin_turn_id.as_ref().is_some_and(|turn_id| {
        turn_id.is_empty() || turn_id.len() > MAX_ID_BYTES || turn_id.chars().any(char::is_control)
    }) {
        return Err(CapabilityStateError::InvalidGrant {
            id: snapshot.id.clone(),
            reason: "origin turn ID is empty, too long, or contains control characters",
        });
    }

    Ok(())
}

fn validate_grants(
    grants: &BTreeMap<String, CapabilityGrantSnapshot>,
) -> Result<(), CapabilityStateError> {
    let total_instruction_bytes = grants
        .values()
        .map(|grant| grant.instructions.len())
        .sum::<usize>();
    if grants.len() > MAX_GRANTS || total_instruction_bytes > MAX_TOTAL_INSTRUCTION_BYTES {
        return Err(CapabilityStateError::StateTooLarge);
    }
    Ok(())
}

fn persist_state(
    path: &Path,
    grants: &BTreeMap<String, CapabilityGrantSnapshot>,
) -> Result<(), CapabilityStateError> {
    validate_grants(grants)?;
    let persisted = PersistedCapabilityState {
        version: FORMAT_VERSION,
        grants: grants.values().cloned().collect(),
    };
    let mut contents = serde_json::to_string(&persisted)?;
    contents.push('\n');
    if contents.len() > MAX_SIDECAR_BYTES {
        return Err(CapabilityStateError::StateTooLarge);
    }

    codex_utils_path::write_atomically(path, &contents)?;
    Ok(())
}

fn read_state(path: &Path) -> Result<BTreeMap<String, CapabilityGrantSnapshot>, SidecarReadError> {
    let mut file = File::open(path).map_err(|error| {
        let missing = error.kind() == std::io::ErrorKind::NotFound;
        SidecarReadError {
            diagnostic: if missing {
                format!("capability sidecar {} is missing", path.display())
            } else {
                format!(
                    "could not read capability sidecar {}: {error}",
                    path.display()
                )
            },
            missing,
            replaceable: missing,
        }
    })?;
    let mut contents = Vec::new();
    file.by_ref()
        .take((MAX_SIDECAR_BYTES + 1) as u64)
        .read_to_end(&mut contents)
        .map_err(|error| SidecarReadError {
            diagnostic: format!(
                "could not read capability sidecar {}: {error}",
                path.display()
            ),
            missing: false,
            replaceable: false,
        })?;
    if contents.len() > MAX_SIDECAR_BYTES {
        return Err(SidecarReadError {
            diagnostic: format!(
                "capability sidecar {} exceeds the size limit",
                path.display()
            ),
            missing: false,
            replaceable: false,
        });
    }

    let persisted: PersistedCapabilityState =
        serde_json::from_slice(&contents).map_err(|error| SidecarReadError {
            diagnostic: format!("capability sidecar {} is invalid: {error}", path.display()),
            missing: false,
            replaceable: true,
        })?;
    if persisted.version != FORMAT_VERSION {
        return Err(SidecarReadError {
            diagnostic: format!(
                "capability sidecar {} has unsupported version {}",
                path.display(),
                persisted.version
            ),
            missing: false,
            replaceable: false,
        });
    }

    let mut grants = BTreeMap::new();
    for grant in persisted.grants {
        validate_snapshot(&grant).map_err(|error| SidecarReadError {
            diagnostic: format!(
                "capability sidecar {} contains an invalid grant: {error}",
                path.display()
            ),
            missing: false,
            replaceable: true,
        })?;
        let id = grant.id.clone();
        if grants.insert(id.clone(), grant).is_some() {
            return Err(SidecarReadError {
                diagnostic: format!(
                    "capability sidecar {} contains duplicate grant ID `{id}`",
                    path.display()
                ),
                missing: false,
                replaceable: true,
            });
        }
    }
    validate_grants(&grants).map_err(|error| SidecarReadError {
        diagnostic: format!(
            "capability sidecar {} exceeds state limits: {error}",
            path.display()
        ),
        missing: false,
        replaceable: true,
    })?;

    Ok(grants)
}

#[cfg(test)]
#[path = "capability_state_tests.rs"]
mod tests;
