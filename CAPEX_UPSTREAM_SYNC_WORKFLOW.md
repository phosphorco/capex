# CapEx upstream sync and release workflow

CapEx is a fork of [OpenAI Codex](https://github.com/openai/codex). `main` is
the primary branch of `phosphorco/capex`; `origin` points to that fork and
`upstream` points to `openai/codex`. The compatibility contract and its known
limits are in [capex-constraints.md](capex-constraints.md); record new findings
in [CAPEX_UPSTREAM_LEARNINGS_LEDGER.md](CAPEX_UPSTREAM_LEARNINGS_LEDGER.md).
Do not treat a successful Git rebase or Cargo build as a compatibility proof.

## Sync source

1. Start with a clean tree. Confirm remotes, the fork's default branch, and
   the commits being integrated:

   ```sh
   git status --short --branch
   git remote -v
   gh repo view phosphorco/capex --json defaultBranchRef
   git fetch --no-tags upstream +refs/heads/main:refs/remotes/upstream/main
   git fetch --no-tags origin +refs/heads/main:refs/remotes/origin/main
   git log --oneline --left-right main...upstream/main
   ```

   Fetch `main` explicitly: an unrestricted fetch of OpenAI's repository
   downloads thousands of unrelated branch and tag refs. Never rebase a dirty
   worktree, discard someone else's changes, or use an unverified local
   `upstream/main` as the base.

2. Make a recoverable local checkpoint before integrating:

   ```sh
   git switch main
   git branch backup/main-before-sync-YYYYMMDD
   ```

   The first publication of `main` was rebased onto upstream. Once `main` is
   public, prefer `git merge upstream/main` for routine syncs: this preserves
   downstream commit IDs and avoids a force push. If a linear rebase is
   deliberately chosen, coordinate with every consumer of `origin/main`,
   inspect the exact remote tip, and use only `--force-with-lease`; do not
   silently rewrite the primary branch. Resolve conflicts in favor of the
   intended CapEx behavior, not just whichever side compiles. Use
   `git rebase --abort` if the integration cannot be completed safely.

3. Review every upstream change touching CapEx's seams: skill discovery,
   selection and explicit invocation; synchronous hooks and their schemas;
   context insertion and compaction; session resume/fork/child history;
   rollout serialization and thread-store indexes; app-server skill APIs;
   CLI dispatch, packaging and version reporting. Compare the resulting
   CapEx delta against upstream, not merely the conflict list:

   ```sh
   git diff --stat upstream/main...main
   git log --oneline upstream/main..main
   ```

4. Run `just fmt` in `codex-rs` after source edits. Run `just test -p <crate>`
   for affected crates, then `just fix -p <crate>` for a large Rust change as
   required by `AGENTS.md`. Do not run `cargo test` directly. Ask before the
   complete `just test` suite when common/core/protocol changed. Review
   snapshots, Cargo/Bazel lockfiles and config schema whenever the upstream
   change calls for them. Record commands, results, and skipped checks in the
   learnings ledger.

5. Exercise actual *two-binary* interoperability on a temporary shared
   `CODEX_HOME`: a pinned, unmodified native Codex binary and the newly built
   CapEx binary. `scripts/capex_binary_compat.py` provides the existing matrix;
   it pins native 0.156.1, so update the pin and expectations when the upstream
   release under test changes. Pass the built binary and its measured SHA-256:

   ```sh
   python3 scripts/capex_binary_compat.py \
     --native-bin /absolute/path/to/native/codex \
     --patched-bin /absolute/path/to/capex/codex \
     --expected-patched-sha256 ACTUAL_SHA256 \
     --patched-revision "$(git rev-parse HEAD)" --keep-temp
   ```

   Retain and inspect the manifest, rollout JSONL, sidecars, request captures
   and shared SQLite indexes. Both runtimes must create, list, resume and
   continue each other's sessions; test compaction in both directions and the
   supported fork cutoff. No CapEx-only rollout JSONL variant or field may be
   introduced without proof that both older and newer native readers and
   writers preserve it. A successful test against one pinned release is not a
   promise about an untested future release. See the exact matrix and gaps in
   [capex-constraints.md](capex-constraints.md).

6. Push only after checking the expected remote tip. For an ordinary
   non-rewriting update:

   ```sh
   git fetch --no-tags origin +refs/heads/main:refs/remotes/origin/main
   git merge-base --is-ancestor origin/main main
   git push origin main
   ```

   Stop if the ancestry check fails. Never push CapEx commits to `upstream`.
   Confirm `origin/main` equals local `main`, and that the fork's GitHub default
   branch is `main`. Keep any legacy `capex` branch only as an explicitly
   identified historical ref; do not let it masquerade as the maintained tip.

## Release gate: upstream-compatible `codex`

Source sync and release are separate decisions. The Rust workspace's
`version = "0.0.0"` is not an upstream release identity. Before advertising a
CapEx release as a drop-in Codex replacement:

- Select and record an exact upstream release tag, native package version and
  binary hash. Build CapEx from the corresponding source baseline and make
  `capex --version` report the matching Codex CLI version in the native
  output format; tools may parse that output. Record an unambiguous CapEx build
  identity separately in the package/release metadata. Verify the advertised
  version against the native binary, not only Cargo metadata.
- Package all required companion binaries/assets and test installation on a
  clean machine. The current local development install is a bare binary with
  a `--no-daemon` launcher workaround; shared-server-only commands therefore
  do **not** make it a full drop-in release.
- Compare native and CapEx CLI command/flag behavior, config/auth/MCP/app-server
  paths, noninteractive `exec` output and exit codes, and shared session/index
  behavior. Run the updated two-binary matrix and targeted regression suites.
  Preserve an unmodified native binary for the cross-runtime test.
- Test both `capex` and a PATH-visible executable named `codex` made from the
  packaged CapEx build. `alias codex=capex` affects an interactive shell only;
  programs that execute `codex` directly do not read that alias. Make the
  opt-in replacement and rollback instructions explicit, and never overwrite
  a user's upstream installation without a deliberate install choice.
- Publish a release only when the observed compatibility scope, unsupported
  features, source/base SHA, native version/hash, CapEx hash and evidence
  location are recorded in the ledger. If any gate fails, keep the build a
  development build and do not claim version parity or general forward
  compatibility.
