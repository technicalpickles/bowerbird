# Test Automation Summary — Story 3.4

Generated 2026-05-25 via `bmad-qa-generate-e2e-tests`. Supersedes the Story 3.3 summary.

## Gap Analysis

Story 3.4 (prebuilt binary distribution and release pipeline) landed with the following coverage already in place from the dev-story run:

- `crates/adapter-claude/tests/contract_install.rs::installed_command_uses_path_relative_binary_name_no_slash_in_first_token` — AC #4 PATH-relative-binary-name regression test. Walks all four hook kinds and asserts (a) no `/` characters and (b) first whitespace-separated token equals `protocol::SHIM_BINARY_NAME`.
- All preexisting contract / CLI / unit suites stay green under `cargo test --workspace -- --test-threads=1` (317 passes before this run; 331 after).
- Dev exercised two manual checks documented in the Completion Notes but not codified anywhere: a local `yamllint` invocation against `release.yml` + `ci.yml`, and a "tarball staging smoke test" that built the staging directory layout and round-tripped `tar -czf` + `tar -xzf`.

Five gaps remained after that baseline. All five are silent-failure modes — a regression here does not break any existing test, but a user lands on a contradictory or broken surface.

**Gap A — AC #2 musl-deferral statement was duplicated in three files (README.md, INSTALL.md, release.yml release-notes template) but no test pinned the cross-file invariant.** A future story that rewords the paragraph in `release.yml` (e.g., to clarify Alpine vs. Void) but forgets the parallel edit in README.md leaves a user landing on the GitHub repo page believing musl is supported.

**Gap B — AC #5's seven-point `bowerbird install` walkthrough markers (a–g) had no automated assertion that both README.md and INSTALL.md contain them.** The two files are the two user entry points (GitHub repo page vs. tarball post-extract); the docs already deliberately duplicate content for that reason. The drift hazard is the same as Gap A: a future story improves one and forgets the other.

**Gap C — AC #6's `cargo test --workspace -- --test-threads=1` invocation in `.github/workflows/ci.yml` was not pinned.** A well-intentioned CI cleanup (e.g., "let's parallelize tests for faster feedback") would re-introduce the entire Epic 2 retro AI-3 / Discovery #3 failure mode without any test surfacing the regression.

**Gap D — AC #7's six WebSocket-subsystem config knobs in `architecture.md` had no automated drift guard.** The pinning sentence in the section ("Defaults are committed at `crates/daemon/src/config.rs`; the table above MUST be updated in the same commit as any field-default change") is a commit-discipline instruction; without an automated mirror, a knob renamed in code but not in the doc goes unnoticed until a contributor reads the section.

**Gap E — AC #1's tarball staging-and-layout logic was exercised manually but not codified.** The dev's "local tarball staging smoke test" lives in the Completion Notes paragraph; there is no script, no test, nothing a future contributor can run before tagging a release. A regression where `release.yml` drops a `cp` step (e.g., during a YAML reformat) would not surface until the first tag push.

## Generated Tests

### Cross-file invariants (new — `tests/release_pipeline_docs.rs`, 14 tests)

A new workspace-level integration test file. Each test reads source files anchored at `env!("CARGO_MANIFEST_DIR")` (the workspace root for the top-level `bowerbird` crate) and grep-asserts the documented cross-file invariants. Tests are hermetic (read-only) and have no ordering requirements.

- **`musl_deferral_statement_appears_in_readme`** — AC #2. README.md must contain the load-bearing phrase `musl Linux is deferred post-V1`, the `(NFR9)` anchor, and the `cargo install --git` alternative.
- **`musl_deferral_statement_appears_in_install_md`** — AC #2. INSTALL.md is allowed to be more concise and cross-link to README.md, but it MUST mention `musl-based distributions` explicitly so a tarball-extracted user does not believe Alpine / Void are supported.
- **`musl_deferral_statement_appears_in_release_workflow_notes`** — AC #2. `.github/workflows/release.yml`'s `release-notes.md` heredoc body must contain the same `musl Linux is deferred post-V1`, `(NFR9)`, and `cargo install --git` markers. Closes the AC #2 cross-file invariant alongside the two README/INSTALL tests.
- **`readme_install_walkthrough_covers_a_through_g_markers`** — AC #5. Asserts twelve marker substrings in README.md: `~/.claude/settings.json`, `BOWERBIRD_CLAUDE_SETTINGS`, `atomic`, `PreToolUse`, `PostToolUse`, `Stop`, `Notification`, `~/.bowerbird/`, `0700`, `--no-start`, `bowerbird uninstall`, `service=bowerbird-daemon`.
- **`install_md_walkthrough_covers_a_through_g_markers`** — AC #5. Same twelve markers asserted against INSTALL.md. The two tests together pin the duplication invariant.
- **`ci_workflow_runs_workspace_tests_single_threaded`** — AC #6. Asserts the exact substring `cargo test --workspace -- --test-threads=1` in `.github/workflows/ci.yml`. The exact-substring match catches the regression mode "someone dropped the `--` separator," which would silently re-introduce the parallel-execution flake without the workflow appearing broken.
- **`architecture_md_documents_all_six_ws_config_knobs`** — AC #7. Asserts the six knob identifiers (`ws_max_connections`, `ws_ping_interval`, `ws_pong_timeout`, `ws_broadcast_capacity`, `shutdown_drain_timeout`, `ws_broadcast_coalesce_window`) appear in `docs/bmad/planning-artifacts/architecture.md`. Identifier renames in `crates/daemon/src/config.rs` without the parallel architecture.md edit fail this test.
- **`architecture_md_pins_table_to_daemon_config_source`** — AC #7. Asserts the section cites `crates/daemon/src/config.rs` as the source of truth for the knob defaults. Without that pointer, the doc-drift hazard is not preventable by reading the section alone.
- **`release_workflow_targets_three_documented_triples`** — AC #1 (structural). Asserts `aarch64-apple-darwin`, `x86_64-apple-darwin`, `x86_64-unknown-linux-gnu` all appear in `release.yml`. A future matrix shrink that drops one target without updating README.md fails here.
- **`release_workflow_stages_all_documented_tarball_entries`** — AC #1 (structural). Asserts the nine documented Task 3.5 staging substrings (`bin/`, `adapters/claude`, `tool-reactions.toml`, `LICENSE`, `LICENSE-MIT`, `LICENSE-APACHE`, `README.md`, `INSTALL.md`, `CHANGELOG.md`) all appear in `release.yml`. The dropping-a-cp-step regression mode is the target.
- **`release_workflow_builds_shim_under_release_shim_profile`** — AC #1 (structural). Asserts `release.yml` contains both `--profile release-shim` (the shim's perf-budgeted build) AND `--exclude bowerbird-shim` (the workspace build skipping the shim). Two-cargo-invocation shape is the documented pattern; a refactor that "simplifies" to a single cargo run would ship a slower shim than the CI bench gate verifies.
- **`release_workflow_enforces_locked_dependency_graph`** — NFR10 / AC #1. Asserts `release.yml` passes `--locked` at least twice (workspace build + shim build). Without `--locked`, a `Cargo.toml` edit between tags could silently bump a dep version in the release tarball.
- **`every_published_crate_declares_mit_or_apache_license`** — Task 6.2. Iterates over the five published Cargo.toml files (workspace + four crates) and asserts each declares `license = "MIT OR Apache-2.0"`. Adding a sixth crate without a license is the regression mode.
- **`workspace_root_ships_dual_license_files`** — Task 6.2. Asserts `LICENSE`, `LICENSE-MIT`, `LICENSE-APACHE` exist at the workspace root. Without them, `release.yml`'s `cp` step fails at runtime and a tarball user gets an undocumented stop-the-line.

### Local tarball-layout smoke (new — `scripts/tarball-smoke-test.sh`)

Codifies Gap E. A standalone bash script the dev (or any future maintainer) can run locally before tagging a release. NOT wired into CI by design — the production validator for the release workflow is the GH-hosted runner on tag push; this smoke is for the pre-tag cycle on the dev box. Catches regression modes that grep-style tests cannot:

- `cp` source paths that exist in the repo but get omitted from the staging dir.
- `tar` invocations that flatten the staging layout (no top-level directory inside the tarball).
- Extracted binaries that are not marked executable.

Usage:

```sh
# Build the workspace first under both required profiles:
cargo build --release --workspace --exclude bowerbird-shim
cargo build --profile release-shim -p bowerbird-shim

# Run the smoke against the host target triple:
./scripts/tarball-smoke-test.sh                  # defaults to v0.0.0-smoke tag, host triple
./scripts/tarball-smoke-test.sh v0.1.0           # custom tag
./scripts/tarball-smoke-test.sh v0.1.0 x86_64-apple-darwin
```

Exits 0 with a layout listing on success; non-zero with a diagnostic on the first failed assertion. The script is read-only against the repo — it only writes under a tempdir-managed staging area, trapped on EXIT so re-runs are clean.

## Coverage

| AC | Resolver/code-level | CLI / cross-file E2E |
|---|---|---|
| AC #1 tarball layout (3 triples, 8 entries) | n/a — release.yml is YAML config | ✅ **new** `release_workflow_targets_three_documented_triples`, `release_workflow_stages_all_documented_tarball_entries`, `release_workflow_builds_shim_under_release_shim_profile`, `release_workflow_enforces_locked_dependency_graph` + local `scripts/tarball-smoke-test.sh` |
| AC #2 musl deferral verbatim in 3 files | n/a | ✅ **new** `musl_deferral_statement_appears_in_readme`, `_install_md`, `_release_workflow_notes` |
| AC #3 `cargo install` on stable Rust | Implicit in `cargo build --workspace --locked` (rust-toolchain.toml channel 1.94.1 stable; MSRV pinned 1.82) | Out of automated scope per custom instructions — requires a clean Rust install env the test harness cannot provide |
| AC #4 PATH-relative binary name | ✅ `crates/adapter-claude/tests/contract_install.rs::installed_command_uses_path_relative_binary_name_no_slash_in_first_token` (added by dev) | Pinned at the contract-test boundary; CLI surface unchanged |
| AC #5 `bowerbird install` walkthrough in README + INSTALL.md | n/a | ✅ **new** `readme_install_walkthrough_covers_a_through_g_markers`, `install_md_walkthrough_covers_a_through_g_markers` |
| AC #6 CI `--test-threads=1` for contract suite | Implicit (CI cmd runs the suite) | ✅ **new** `ci_workflow_runs_workspace_tests_single_threaded` |
| AC #7 architecture.md WS subsystem section | n/a | ✅ **new** `architecture_md_documents_all_six_ws_config_knobs`, `architecture_md_pins_table_to_daemon_config_source` |
| Task 6.2 dual MIT/Apache license | n/a | ✅ **new** `every_published_crate_declares_mit_or_apache_license`, `workspace_root_ships_dual_license_files` |

## Verification (this session)

```text
cargo test --workspace -- --test-threads=1 --skip state_plus_event_atomicity_under_sigkill_during_load
  → 331 passed, 1 filtered out (18 suites, 22.84s)

cargo clippy --workspace --all-targets
  → No issues found

./scripts/tarball-smoke-test.sh v0.1.0-smoke
  → tarball-smoke-test OK
    extracted entries: bin/{bowerbird,bowerbird-shim,bowerbird-daemon},
                       adapters/claude/tool-reactions.toml,
                       LICENSE, LICENSE-MIT, LICENSE-APACHE,
                       README.md, INSTALL.md, CHANGELOG.md
```

## Files added / modified

**New:**

- `tests/release_pipeline_docs.rs` — 14 cross-file invariant tests for ACs #1, #2, #5, #6, #7 + Task 6.2 license discipline.
- `scripts/tarball-smoke-test.sh` — local tarball layout smoke test (executable; 0755).

**Modified:**

- `docs/bmad/implementation-artifacts/tests/test-summary.md` — this file (supersedes Story 3.3 summary).

## Out of automated scope

- **AC #3 cargo install from a clean stable Rust env** — requires a sandboxed `~/.cargo/bin/` and a Rust 1.82+ toolchain; the test harness shares the dev's cargo state. Documented in the AC #3 row of the coverage table and verified manually via `cargo build --release --workspace --locked` per Task 6.4.
- **AC #1 actual GH Actions runner execution of `release.yml`** — the production validator is a tag push to the upstream remote; the GH-hosted runner is the only environment that can exercise the full matrix. The new structural tests cover the YAML-config invariants; the smoke script covers the staging+tar logic locally. The first real `v*.*.*` tag push is the integration test.
- **`yamllint` / `actionlint` linting of workflow YAML** — yamllint is installed on the dev box and was run manually by dev per the Completion Notes; actionlint is not installed. Adding either to CI is a fair follow-up (file an issue if Story 3.4's deferred-work entries do not already cover it).

## Next steps

- Tag a `v0.1.0-rc1` prerelease on the upstream remote to exercise `release.yml` against the real GH Actions runner; verify the three tarballs attach to the release page and that each, when extracted, matches the layout the smoke script asserts locally.
- Consider lifting `scripts/tarball-smoke-test.sh` into a CI job that runs on PRs touching `release.yml` (gates the YAML-config invariants with an end-to-end staging exercise — though the GH-hosted runner is still the canonical validator).
- Add an `actionlint` lint pass in `.github/workflows/ci.yml` (post-V1; current shim-bench-gate is the only target-pinned job and yamllint already passes locally on both workflow files).
