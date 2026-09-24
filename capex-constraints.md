# CapEx build constraints

## Post-delivery adversarial review (resolved for v1)

- Spawned children now scrub inherited CapEx developer guidance *before child
  rollout persistence*, including `Compacted.replacement_history`; non-root
  cold resume also scrubs an older or native-authored contaminated history in
  memory without rewriting shared JSONL. Focused first-request, persisted-prefix,
  and cold-resume tests pass. Existing contaminated JSONL remains on disk, so
  native Codex can still read its original text; CapEx removes it on each child
  resume rather than migrating upstream-owned records.
- Synchronous hook grants now trigger a post-hook dependency refresh before
  the first model request. A regression asserts the newly eligible skill's MCP
  tool is present on that request.
- Core explicit `$name` resolution now filters before name ambiguity counts,
  so a hidden duplicate cannot block a visible skill. A gated-only explicit
  reference emits an unavailable warning. Mode-off selection retains the old
  resolver, and unit plus request-level regressions pass.
- The experimental shadow selector refilters after merging raw host skills;
  hidden entries are removed before candidate selection and its metrics.
- A reviewer found ordinary copied forks may omit the existing logical cutoff
  metadata. This is upstream-pre-existing and has no demonstrated CapEx grant
  mis-selection: CapEx seeds from selected history, not this metadata. Legacy
  rollouts may lack absolute ordinals, so do not fabricate a cutoff. The
  prepared paginated fork converted to copied persistence retains an exact
  cutoff and remains the tested compatibility path.

The original pinned binary matrix did not cover child inheritance or
hook-granted MCP dependencies. Those paths now have focused request-level tests;
the rebuilt binary passed a fresh native/CapEx shared-storage matrix. The
ordinary copied-fork lineage limit remains documented, not treated as a new v1
grant/JSONL compatibility defect.

This file records requirements and repository rules that must survive handoff.
The intended behavior and acceptance map live in `capex-v1.md`; task evidence and
progress live in `capex.ledger.jsonl` through `capex.plan.pkl`.
For ongoing source synchronization, version parity, and release checks, see
`CAPEX_UPSTREAM_SYNC_WORKFLOW.md` and `CAPEX_UPSTREAM_LEARNINGS_LEDGER.md`.
The original implementation and pinned binary proof were based on
`8e17909b27875b76b1e9a883a604ed24e969d609`. On 2026-09-24 the fork's
`main` was rebased onto upstream `16b20547f6b1feecbf41b89057d704ce1c293ab0`;
that newer tree needs its own test results before the earlier proof can be
claimed for it. The `codex-rs` workspace version `0.0.0` and a raw-binary
`--no-daemon` launcher are development artifacts, not a version-matched,
fully packaged drop-in Codex release.

## User requirements

1. Native upstream Codex and CapEx share the **same session storage**. Each must
   be able to open and continue sessions written by the other, including after
   either runtime has compacted a session.
2. Do not change session JSONL unless the exact change is proved **forwards and
   backwards compatible**. CapEx capability grants therefore use a separate
   per-thread sidecar. No new rollout item variant or field is permitted for
   v1. Ordinary existing-shape `ResponseItem::Message` lines may carry
   model-visible capability context, but their cross-version readability and
   rewriting must be tested. The sidecar must be optional for native Codex.
3. A hook grant must reveal the corresponding capability instructions and newly
   eligible skill metadata before the next model request. Automatic compaction
   in the middle of a turn must restore effective availability immediately.
4. Build with Luna xhigh worker agents. Give each worker a bounded file
   ownership area and preserve concurrent edits.
5. Keep this file current as the build reveals compatibility limits or new
   decisions. Do not treat a sidecar unit test as proof of shared-session
   interoperability.

## Existing source and repository contract

- Original implementation base: `openai/codex` main at
  `8e17909b27875b76b1e9a883a604ed24e969d609`. The first fork-primary
  `main` rebase uses `16b20547f6b1feecbf41b89057d704ce1c293ab0`; consult
  the sync ledger for later bases and verification status.
- Root `AGENTS.md` governs code and tests. Every new model-visible fragment
  must be a bounded `ContextualUserFragment` under `codex-rs/core/src/context`;
  no item may exceed 10K tokens and context must grow incrementally.
- Preserve existing skill disablement, plugin disablement, and explicit skill
  invocation behavior under CapEx filtering. Mode off retains native behavior.
- `RolloutItemWire` in `codex-rs/history/src/rollout_payload.rs` is a closed
  tagged enum. Older-reader behavior for an unknown variant is not proved;
  adding a CapEx variant would risk parse failure or grant loss and is excluded.
- Run `just fmt` under `codex-rs`; use `just test -p <crate>` for targeted tests
  and `just fix -p <crate>` for a large Rust change. Root `AGENTS.md` says to
  ask before the full `just test` suite after common/core/protocol changes.
- No dependency change unless its Cargo and Bazel lock updates are both made.
- Build storage has fluctuated sharply (below 400 MiB during test builds, then
  about 12 GiB free). Generated `codex-rs/target` artifacts were removed with
  `cargo clean` after failed builds; monitor free space and do not delete source
  or user data to make room.

## Verification gate

Use temporary shared session storage to run this sequence in both directions:
native starts -> CapEx resumes and grants -> native resumes and continues ->
CapEx resumes; then repeat across native and CapEx compaction. Inspect session
JSONL bytes/schema for unsupported changes and assert effective CapEx grants
come only from the sidecar. Also verify missing/corrupt sidecar recovery, fork
cutoff, duplicate grants, tag exclusion, disabled skills, and immediate hook
reveal using outbound model request assertions.

The binary-level compatibility matrix must use a pinned installed native Codex
binary (currently `codex-cli 0.156.1`, SHA-256
`0196e89fe5a7598f816ee54232c3d7c26d75e502ab5cfe2c9240e81d90f7255a`)
and a built CapEx binary, never two
instances of the patched source. Give both the same temporary `CODEX_HOME` and
session ID, with an isolated mock model endpoint:

This can prove interoperability for the pinned upstream version, not all future
native releases. Keep the JSONL schema unchanged and rerun the matrix when the
native binary changes; do not label a source-level plausibility argument as
forward-compatibility proof.

| Case | Required observation |
| --- | --- |
| Native origin -> CapEx -> native -> CapEx | Each resumes and adds a turn; CapEx grant survives native continuation. |
| CapEx origin -> native -> CapEx | Native reads and continues the CapEx-written developer message; CapEx restores exactly the sidecar grants. |
| Native compacts a CapEx session | CapEx next request reintroduces exactly one active instruction block and current skill catalog. |
| CapEx compacts a native-origin session after an active grant | Native can still resume; CapEx's immediate next request has exactly one active instruction block, and repeated resume/compaction has no duplicates. |
| Fork cutoff | Core tests cover before/after a completed dynamic-grant turn and before the first turn with initial grants, including an explicit empty snapshot. The binary matrix covers one CapEx prepared/reference-backed fork converted to a self-contained child: native cold-resumes and continues it, then CapEx does the same from shared storage, with copied JSONL, existing logical-cutoff metadata, sidecar, and index inspected. Other historical cutoffs, especially after native compaction, remain unsupported unless demonstrated exact. |

For each case, compare the pre/post rollout JSONL record types and payload
fields, confirm neither binary reports parse or state-index errors while listing
as well as resuming the same thread, inspect shared state DB/index behavior,
and assert the immediate next outbound model request. Close each process before
the other opens the shared `CODEX_HOME`.
Merely deserializing a `ResponseItem` with current Rust types is not sufficient.

Do not mark the compatibility guard in `capex.plan.pkl` complete on code review
alone; it requires observed cross-runtime behavior.

## Review findings still requiring proof or resolution

- The app-server `skills/list` RPC is cwd-based and has no thread identity, so
  its picker cannot show each thread's CapEx-filtered catalog without an
  additional design. Core invocation and model-visible catalogs must still
  reject gated skills independently of that UI.
- Fork cutoff cannot be inferred merely by loading the parent's latest
  sidecar. Core tests now demonstrate the grant-to-history cutoff for selected
  root fork paths, including a prepared/reference-backed fork. Do not claim
  native-compacted historical-fork fidelity or cross-binary fork support until
  each has its own observed proof.
- A reference-backed prepared fork must not scrub only its in-memory history:
  the original inherited-item count can then drain beyond the shorter vector,
  while a cold resume can replay the unsanitized parent history. In CapEx mode,
  such a fork needs a durable sanitized child history (or must fail closed),
  with focused prepared-fork and cold-resume tests. A copied replacement must
  also preserve the original fork cutoff in the existing SessionMeta ordinal
  field; setting history_base to None must not silently erase lineage. Copied-
  fork tests alone do not discharge this requirement.
- Initial capability snapshots must be persisted on fresh sessions so a
  resume does not silently read a changed `CAPABILITY.md` definition.
- Run an actual old-native / patched-CapEx round trip on temporary shared
  storage before claiming the rollout JSONL is cross-runtime compatible. The
  gate must also check shared indexes/state DBs and locking, not just JSONL.

## Current implementation status (not acceptance evidence)

- Sidecar writes are atomic and bounded. Fresh/clear replace previous grants;
  valid resume reads the snapshot without re-reading capability definitions.
  Missing/corrupt replaceable state uses a persisted initial fallback; a
  failed write activates no initial grants.
- Root forks select parent snapshots only when the retained fork history has
  the harness-classified `capex.instructions` developer item for each dynamic
  ID; a dynamic grant also requires its originating `TurnComplete` boundary in
  the retained prefix, since a prompt hook can write its marker before the user
  message later excluded by a cutoff. Initial grants need no history marker:
  they are effective from session creation, including before the first turn. A
  fork also strips inherited CapEx instruction messages before rebuilding from
  its selected child sidecar; otherwise a retained pre-user marker could expose
  a dynamic capability despite a correctly empty child snapshot. The
  prepared-fork integration test and pinned binary child round trip now verify
  that scrub for the selected cutoff. A missing parent sidecar is read without
  creating one; a valid parent sidecar
  produces one atomic child snapshot, even for an empty selection. Other
  historical cutoffs, especially after native compaction, remain unproved.
- A fork from a native-only parent with no CapEx sidecar starts from configured
  initial capabilities. A corrupt/unreadable parent sidecar or failed child
  snapshot write uses fail-closed child loading and activates no new grants.
  Resuming a fork with a missing child sidecar also fails closed, so a prior
  failed seed cannot later turn into configured initial grants. Focused review
  found this source logic consistent, but suite integration tests for a
  native-only parent fork and failed seed followed by child resume are missing.
- After adversarial fixes, `codex-skills` passes 56/56,
  `codex-skills-extension` 180/180, and the focused `codex-core` CapEx tests
  pass 15/15 unit plus 8/8 integration with `RUST_MIN_STACK=8388608` and one
  test thread. The existing prepared-fork integration overflows the default
  test-thread stack in a filtered run but passes with 8 MiB. Earlier unchanged
  suites passed: `codex-hooks` 187/187 and `codex-thread-store` 254/254;
  `cargo check -p codex-app-server --tests` passes after this review round.
  The first core fork and binary runs found real or harness defects; their
  corrective history is in `capex.ledger.jsonl`.
- A freshly rebuilt patched CLI has SHA-256
  `6c4ca249e45ce776cbb62a2f1edbe6d1bc7671dffe93aaf1891563520f62721e`.
  The pinned native 0.156.1 SHA-256 is
  `0196e89fe5a7598f816ee54232c3d7c26d75e502ab5cfe2c9240e81d90f7255a`.
  The tightened two-binary matrix passed again after the review fixes on one
  temporary shared `CODEX_HOME`: both session origins, synchronous hook grant, native and CapEx
  compaction with immediate follow-up, sidecar preservation, shared SQLite
  indexes, and a CapEx prepared fork copied without history_base at cutoff 27
  that native cold-resumed/continued before CapEx resumed it. The final
  retained manifest, rollout JSONL, sidecars, request captures, and DBs are at
  `/var/folders/59/3p94sjsj1vb3vd0czhyr554m0000gn/T/capex-binary-compat-cstua_yq`.
  The harness checked both child requests, the copied rollout and sidecar, and
  shared parent/child index projections; an independent read-only
  `PRAGMA integrity_check` returned `ok` for all six SQLite databases. This supports the pinned,
  serial shared-storage compatibility Guard. Other fork boundaries, future
  native versions, simultaneous writer/locking safety, and binary tagged
  skill-catalog exposure are outside this matrix.
- The new core regression for hook grant plus tagged skill across mid-turn
  automatic compaction now passes, including its immediate continuation
  request. The reference-backed fork fix also passes its focused prepared-fork
  and paginated cold-resume regression (1/1), with no physical history_base
  and a preserved logical cutoff. The pinned binary fork round trip above now
  observes native and CapEx cold continuation of that child. Other historical
  fork cutoffs and the per-thread app-server skill picker remain outside proof.
- An ordinary developer `ResponseItem` with a string content kind is now
  observed readable and continuable by native 0.156.1 in the pinned matrix.
  Native compaction may drop its classification and capability message.
  Same-thread CapEx recovery must rely on the sidecar, not that message.
  Exact developer text is used only to avoid duplicate guidance if native
  drops the classification;
  it never authorizes a grant or fork inheritance. Historical forks after
  native compaction may lack the classified marker needed for cutoff selection;
  they can lose grants conservatively and cannot yet claim full fork fidelity.
