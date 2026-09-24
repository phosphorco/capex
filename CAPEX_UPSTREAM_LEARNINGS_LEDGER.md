# CapEx upstream learnings ledger

This is an append-oriented record of decisions and observed behavior for
maintaining the fork. The operative constraints and binary test matrix live in
[capex-constraints.md](capex-constraints.md); the repeatable process lives in
[CAPEX_UPSTREAM_SYNC_WORKFLOW.md](CAPEX_UPSTREAM_SYNC_WORKFLOW.md). Do not replace
evidence with a broad claim of "Codex-compatible."

## Compatibility vocabulary and release expectations

- **Backward readability:** CapEx can list, open, resume and continue sessions
  written by the supported native Codex version, including its compactions.
- **Forward readability for the tested pair:** that same unmodified native
  Codex can list, open, resume and continue CapEx-written sessions, including
  after CapEx compaction. Its existing tools and indexes must still work.
- **Shared-storage safety:** both use the same `CODEX_HOME` and session files,
  not a copied or translated archive. Preserve rollout JSONL shapes, shared
  index behavior, and native-owned data. CapEx grants live in an optional
  per-session sidecar; native Codex must not need to understand it. Do not
  modify JSONL schema without explicit proof in both directions.
- **Future upstream compatibility is a hypothesis until retested.** A test
  against native 0.156.1 proves only that pair and its exercised paths. A new
  upstream release can change parsing, compaction, thread indexes, CLI or
  packaging. Re-pin and rerun the binary matrix before making a claim about
  that release. Unknown future versions cannot be certified in advance.
- **Drop-in tool compatibility is broader than session readability.** Match
  the corresponding native release version, provide a complete package,
  preserve CLI/config/auth/MCP/app-server and `exec` contracts, and test a
  PATH-resolved `codex` executable. A shell alias alone cannot serve tools
  that launch subprocesses. The current raw-binary `--no-daemon` development
  install is not yet this release product.

## Entries

### 2026-09-24 — fork remotes and first `main` rebase

- **Observed:** `phosphorco/capex` was a fork of `openai/codex`, defaulting to
  `capex`; locally only `upstream` existed. The old fork `main` was at
  `869b5527cd`, and OpenAI `main` advanced to `16b20547f6`. The two CapEx
  commits rebased cleanly onto that upstream tip. A local `origin` was added
  for the fork and a `main` branch created from the rebased result.
- **Learning:** `origin/main` means the *fork's* `main` after this remote
  topology is established; upstream source is `upstream/main`. Be explicit
  about both names in commands and reviews. An unrestricted upstream fetch
  brought thousands of unrelated refs; use a `main`-only, no-tags fetch during
  normal syncs.
- **Evidence/status:** Git graph and remote tips observed locally during this
  sync. Record the final pushed commit, default-branch switch and test results
  in a follow-up entry below; a clean rebase alone establishes none of the
  compatibility gates.

### 2026-09-24 — shared-session compatibility baseline

- **Observed:** The earlier pinned native 0.156.1 / CapEx binary matrix passed
  serial cross-runtime create/resume/continue, native and CapEx compaction,
  sidecar preservation, shared SQLite index checks, and one prepared copied
  fork cutoff. See exact binaries, hashes, path and limitations in
  [capex-constraints.md](capex-constraints.md).
- **Learning:** Store capability state outside rollout JSONL. An existing-shape
  developer message may carry instructions but cannot authorize grants; native
  compaction can drop its classification. CapEx must rebuild effective guidance
  from the sidecar after resume/compaction, without duplicating it. A test
  using two patched binaries would miss old-native parse failures.
- **Limit:** Other historical fork cutoffs, especially after native
  compaction, concurrent writers, and thread-specific app-server skill-picker
  display have not been established. The prior binary proof does not cover
  the newly rebased source or a later native version.

### 2026-09-24 — capability selection and hook timing

- **Observed:** Earlier review exposed three independent leaks: child history
  could inherit CapEx guidance, hidden duplicate skill names could distort
  explicit `$name` resolution, and a synchronous grant could reach the model
  before the newly eligible skill's MCP dependency refreshed. Focused
  regressions were added; details are in [capex-constraints.md](capex-constraints.md).
- **Learning:** Recheck the immediate outbound request, not just internal
  catalogs or sidecar state. Grants must be persisted and folded before the
  next request; compaction, child creation, explicit invocation and plugin
  paths must all agree on the same effective selection. A model-written
  summary is not a source of authority for capability grants.

### 2026-09-24 — local launcher is not a release package

- **Observed:** A copied development `codex` binary failed interactive startup
  because it lacked the complete local package for the background server.
  The local `capex` launcher now selects `--no-daemon` for interactive use.
- **Learning:** A CLI that opens in a TUI is not necessarily a drop-in Codex
  distribution. Shared-server commands, executable naming for subprocesses,
  package assets and `--version` parity must be checked separately. Current
  workspace Cargo metadata reports `0.0.0`; a release must derive and verify
  its identity from a selected upstream release, not merely patch that string.

### 2026-09-24 — verification of the first rebased tree (`16b20547f6`)

- **Observed:** `just fmt` succeeded using `just` 1.58.0; the active shim
  resolved to 1.15.0, which cannot parse this repository's `working-directory`
  setting. `just test -p codex-core -E 'test(capex)'` passed 23/23. The four
  affected supporting crates (`codex-hooks`, `codex-skills`,
  `codex-skills-extension`, `codex-thread-store`) passed 677/677 tests.
  `cargo build -p codex-cli` succeeded, producing SHA-256
  `ea593a962568f9bb715c43d89251d544489dcbda5bed40cb77d64fe11e41d381`.
- **Cross-runtime evidence:** The pinned native 0.156.1 / rebased CapEx matrix
  passed both origins, continuation, compaction in both directions, sidecar and
  SQLite checks, and the supported copied-fork cutoff. Its retained evidence is
  `/var/folders/59/3p94sjsj1vb3vd0czhyr554m0000gn/T/capex-binary-compat-9pxdj81s`.
  This proves the exercised paths for that exact binary pair, not all future
  native releases or a fully packaged CLI.
- **Broad-check caveat:** The entire `codex-core` crate test run failed across
  multiple suites, including code-mode expectations, missing
  `test_stdio_server`, optional MCP startup timing, and plugin tests. The
  CapEx-filtered rerun passed. Do not report the complete core crate as green
  or infer that every broad failure is unrelated without a clean upstream
  comparison. Generated `.snap.new` files from the failed run were not
  accepted and were removed.

### 2026-09-24 — final pre-publication upstream tip (`b19cebecc0`)

- **Observed:** OpenAI `main` added `49862b62be` and `b19cebecc0` while the
  first verification ran. Neither commit changed a file in the CapEx patch.
  The three local CapEx commits rebased cleanly onto upstream
  `b19cebecc0169097bda7539af03c886e03bdeafe`.
- **Focused checks:** `just test -p codex-core -E 'test(capex)'` passed 23/23;
  `cargo build -p codex-cli` passed. The resulting CLI SHA-256 is
  `51cbb553103458ed9ffff696355e9f1e97db20022ac5ef30012e42438a216df2`.
  The supporting-crate 677/677 result belongs to the immediately preceding
  upstream tip; those two new commits did not touch the four supporting
  crates. The broad `codex-core` failures remain unresolved and were not
  rerun for this final tip.
- **Cross-runtime evidence:** The pinned native 0.156.1 / final-tip CapEx
  matrix passed the same serial shared-session paths, including two-way
  compaction and the supported copied fork. Evidence is retained at
  `/var/folders/59/3p94sjsj1vb3vd0czhyr554m0000gn/T/capex-binary-compat-7ix5d27s`.
  This is a source-sync validation, **not** a package/version-parity release.

## New-entry template

```text
### YYYY-MM-DD — short title
- Upstream source/tag and native version/hash:
- CapEx commit and binary hash:
- Change/decision and why:
- Commands/evidence location and pass/fail results:
- Backward and forward paths actually exercised:
- Remaining uncertainty, owner, and next verification:
```
