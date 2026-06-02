# Story 5.7: Release pipeline end-to-end verification

Status: ready-for-dev

<!-- Note: Validation is optional. Run validate-create-story for quality check before dev-story. -->

## Story

As a release manager,
I want the GitHub Releases pipeline driven to a real tag, producing artifacts that install and run on a fresh machine,
so that v0.1.0 is the second release we cut — not the first.

**Human-in-the-loop, ops-flavored, near-zero net production code.** The release pipeline (`release.yml`, `tarball-smoke-test.sh`, `INSTALL.md`, `release_pipeline_docs.rs`, dual-license files) was **fully built in Story 3.4 but has never been exercised against a real tag** — Story 3.4 validated only via `yamllint` and a local `tar` staging smoke test ([3-4:549](3-4-prebuilt-binary-distribution-and-release-pipeline.md), [epic-4-retro:358](epic-4-retro-2026-05-25.md)). This story is the **first end-to-end smoke**: push `v0.1.0-rc1`, watch the pipeline produce three tarballs, install one on a clean machine, start a real Claude Code session, and confirm the Story 5.1 presenter receives state frames. The only code that may land on `main` is a one-line `cross_version_upgrade.rs` fix and (optionally) a `draft:`-handling tweak in `release.yml`; everything else is tag-push, observation, and a new `docs/release-checklist.md`.

**The maintainer drives the irreversible/external steps** (pushing the tag, the fresh-machine install). The dev agent prepares the code/doc changes, runs the local pre-tag verification, and authors the checklist — it cannot push a public tag or provision a fresh Mac autonomously. Treat the tag-push and fresh-machine ACs as maintainer-executed with the dev agent producing the runbook.

**Closes Epic 4 retro AI-8 (cross-version SKIP) and AI-9 (release-checklist), and exercises the pipeline that folds Epic 3 retro AI-3/AI-4 + Epic 4 retro AI-1..AI-5** ([epics.md:223](../planning-artifacts/epics.md)). Resequenced 5.3 → 5.6 ([sprint-change-proposal-2026-05-27-epic-5-resequencing.md](../planning-artifacts/sprint-change-proposal-2026-05-27-epic-5-resequencing.md)) → 5.7 ([sprint-change-proposal-2026-05-29-idle-prompt-reclassification.md](../planning-artifacts/sprint-change-proposal-2026-05-29-idle-prompt-reclassification.md)); release verification doesn't unblock daily dogfooding, so it sits after the correctness/UX work.

**Scope boundary:** crates.io namespace verification and the final non-prerelease `v0.1.0` tag belong to the **closing story 5.10** (`crates-io-namespace-and-v0-1-0-tag`), not here. Bench-baseline seeding + chaos sanity (Epic 4 retro AI-1/AI-2/AI-3) belong to **Story 5.5**. Do not pull that work forward; this story stops at rc1 + the runbook.

## Acceptance Criteria

1. **Given** the release workflow at `.github/workflows/release.yml` **When** a `v0.1.0-rc1` tag is pushed **Then** the `build` job produces tarballs for `aarch64-apple-darwin`, `x86_64-apple-darwin`, and `x86_64-unknown-linux-gnu`, and the `release` job attaches all three (plus their `.sha256` sidecars) to a GitHub Release for the tag. **Decision required (capture in Dev Agent Record):** `release.yml` currently sets `prerelease: ${{ contains(tag,'-') }}` with **no `draft:` key** ([release.yml:332](../../.github/workflows/release.yml)), so an `-rc1` tag publishes a live *prerelease* immediately. The AC's "draft assets" wording is not what the workflow does today. Either (a) accept "prerelease" as the V1 interpretation and note it, or (b) add `draft: ${{ contains(tag,'-') }}` so rc builds stage as drafts. Record which path was chosen and why.

2. **Given** a fresh macOS arm64 machine (or VM, or a backed-up-and-wiped `~/.bowerbird/` + `~/.claude/settings.json`) **When** the maintainer downloads the `v0.1.0-rc1` `aarch64-apple-darwin` tarball, runs `tar -xz`, follows `INSTALL.md` (incl. the `xattr -d com.apple.quarantine` step), runs `bowerbird install`, and starts a Claude Code session **Then** events appear in `~/.bowerbird/bower.db`, `bowerbird status` shows the daemon running, and the Story 5.1 first-party presenter receives `state.session.*` frames. The exact commands run and observed results are captured in the Dev Agent Record.

3. **Given** the cross-version upgrade contract test `tests/cross_version_upgrade.rs` **When** Story 5.7 lands **Then** its SKIP guard is reconciled with the rc1 tag: the conventional prior-binary path hardcodes `v0.1.0` ([cross_version_upgrade.rs:49-57](../../tests/cross_version_upgrade.rs)) which will never resolve a `v0.1.0-rc1` install. Because **rc1 is the first tag (no prior tag exists), the test correctly stays SKIPPED for rc1 itself** (Epic 4 retro AI-8: the SKIP lifts starting `v0.1.0-rc2`, when rc1 becomes a resolvable prior — [epic-4-retro:281](epic-4-retro-2026-05-25.md)). This AC is satisfied by EITHER: (a) updating the hardcoded path segment to track the rc lineage (`v0.1.0` → the actual prior tag, or rely on the `BOWERBIRD_PRIOR_VERSION_BINARY` env override which already takes precedence), AND/OR (b) documenting in the story that rc1 has no prior so the guard is intentionally still active, with the concrete change rc2 will need. No silent no-op: the resolution must be explicit.

4. **Given** Gatekeeper warnings on first run of unsigned macOS tarball binaries **When** the maintainer follows `INSTALL.md`'s `xattr -d com.apple.quarantine ...` step ([INSTALL.md:19-31](../../INSTALL.md)) **Then** the binaries run successfully; this is documented as the V1-acceptable path and the deferred-work entry for code-signing/notarization ([deferred-work.md:83](deferred-work.md)) **remains open** (cost decision: post-V1, Apple Developer ID $99/yr + notarization roundtrip). Do not close or implement signing.

5. **Given** the rc1 release surfaces a behavioral, install, or release-pipeline issue **When** the maintainer escalates it **Then** a `5.X-hotfix-<topic>` story is created inline (via `bmad-create-story`) and resolved before moving to Story 5.8 — matching the established "dogfooding bugs become ad-hoc 5.X stories" convention ([sprint-status.yaml](sprint-status.yaml) dogfooding-validation-phase note). If rc1 is clean, record "no hotfix needed" in the Dev Agent Record.

6. **Given** the pre-flight steps for cutting a real tag are currently tribal knowledge **When** Story 5.7 lands **Then** `docs/release-checklist.md` exists (Epic 4 retro AI-9 — [epic-4-retro:282](epic-4-retro-2026-05-25.md)) consolidating the ordered pre-tag steps: confirm bench baselines seeded (Story 5.5), `cargo test --workspace -- --test-threads=1` + `fmt --check` + `clippy -D warnings` green, run `scripts/tarball-smoke-test.sh` locally, push rc tag, verify the three jobs, fresh-machine install, then the cross-version SKIP lifts at rc2. The AI-9 tracking entry in the epic-4 retro is struck through with a backlink to this story.

7. **Given** the existing doc-drift guardrail `tests/release_pipeline_docs.rs` **When** any edit touches `release.yml`, `INSTALL.md`, `README.md`, or `ci.yml` **Then** all of its exact-substring assertions still pass (`cargo test --workspace -- --test-threads=1` green). This is a non-regression guard, not new work — see Dev Notes "Brittle pinned strings."

## Tasks / Subtasks

- [ ] **Task 1: Pre-tag local verification (AC: 1, 6, 7)**
  - [ ] Run `cargo test --workspace -- --test-threads=1`, `cargo fmt --check`, `cargo clippy --all-targets --workspace -- -D warnings` — all green. (Serialized run is mandatory; see Dev Notes "Test execution.")
  - [ ] Run `./scripts/tarball-smoke-test.sh v0.1.0-rc1` against locally built binaries; confirm the 10 expected extracted paths and executable bits ([tarball-smoke-test.sh:144-176](../../scripts/tarball-smoke-test.sh)).
  - [ ] Confirm Story 5.5 has seeded `crates/daemon/benches/baselines/{macos,linux}.json` (non-zero p99). If not yet done, note the dependency and surface to the maintainer — do not seed them here.

- [ ] **Task 2: Resolve the draft-vs-prerelease decision (AC: 1)**
  - [ ] Inspect `release.yml:326-338` (`softprops/action-gh-release@v2` block). Decide (a) accept prerelease semantics or (b) add `draft: ${{ contains(steps.tag.outputs.tag, '-') }}`.
  - [ ] If (b): make the one-key edit; re-run `cargo test --test release_pipeline_docs` to confirm no pinned-string regression. Document the choice + rationale in the Dev Agent Record.

- [ ] **Task 3: Reconcile the cross-version SKIP guard (AC: 3)**
  - [ ] Read `tests/cross_version_upgrade.rs:42-85` (the two-layer guard + `resolve_prior_version_binary`). Confirm the hardcoded `target/cross-version-installs/v0.1.0/...` path at line ~53.
  - [ ] Apply resolution (a) and/or (b) from AC #3. Recommended minimal change: leave the env-override path as the CI mechanism (already correct — `release.yml:230-237` sets `BOWERBIRD_PRIOR_VERSION_BINARY`), and update the source comment + hardcoded segment so a human populating the conventional path for an rc lineage doesn't silently mis-resolve. Add a `// rc1 is the first tag; this guard lifts at rc2 (Epic 4 retro AI-8)` note.
  - [ ] `cargo test --test cross_version_upgrade -- --test-threads=1 --nocapture` still passes (SKIPs cleanly for rc1).

- [ ] **Task 4: Author `docs/release-checklist.md` (AC: 6)**
  - [ ] Write the ordered pre-tag runbook (steps in AC #6). Cross-link `INSTALL.md`, `release.yml`, `tarball-smoke-test.sh`, and this story.
  - [ ] Strike through Epic 4 retro AI-9 in `epic-4-retro-2026-05-25.md` §"Action items for V1 release readiness" with a backlink to this story's merge commit (follow the strike-through-not-delete convention used for resolved items).

- [ ] **Task 5: MAINTAINER — push the rc1 tag and verify the pipeline (AC: 1)**
  - [ ] Maintainer pushes `v0.1.0-rc1` (or runs `workflow_dispatch` with `tag: v0.1.0-rc1`). Dev agent provides the exact command in the checklist.
  - [ ] Verify `build` job (3 matrix rows green, artifacts uploaded), `cross-version-test` job (skips — no prior tag), `release` job (Release created, 3 tarballs + 3 `.sha256` attached, prerelease flag set).
  - [ ] Capture run URL + observed artifact list in Dev Agent Record.

- [ ] **Task 6: MAINTAINER — fresh-machine install + presenter smoke (AC: 2, 4)**
  - [ ] On a clean macOS arm64 target (or wiped `~/.bowerbird/` + settings backup): download tarball, `tar -xz`, `xattr -d com.apple.quarantine bin/*`, `bowerbird install`, start a Claude Code session.
  - [ ] Assert: events in `~/.bowerbird/bower.db`, `bowerbird status` shows running daemon, Story 5.1 presenter receives `state.session.*` frames. Capture results.

- [ ] **Task 7: Triage rc1 findings (AC: 5)**
  - [ ] If any issue surfaces, create `5.X-hotfix-<topic>` via `bmad-create-story` and resolve before 5.8. Otherwise record "rc1 clean — no hotfix."

## Dev Notes

### What exists already (do not rebuild)

The pipeline is complete as of Story 3.4 ([3-4-prebuilt-binary-distribution-and-release-pipeline.md](3-4-prebuilt-binary-distribution-and-release-pipeline.md), status `done`):

- **`.github/workflows/release.yml`** (339 lines) — **three** jobs (the header comment says "two-stage" but is stale): `build` (3-target matrix, line 48), `cross-version-test` (line 167), `release` (line 244). Trigger: `push: tags: ['v*.*.*']` + `workflow_dispatch` with required `tag` input ([release.yml:34-42](../../.github/workflows/release.yml)).
  - Targets: `aarch64-apple-darwin` (native macos-latest), `x86_64-apple-darwin` (cross-compiled from ARM runner), `x86_64-unknown-linux-gnu` (pinned **`ubuntu-22.04`** for glibc 2.35 floor — do NOT bump to `ubuntu-latest`, no test catches the silent glibc raise).
  - Two cargo builds per row: workspace `--exclude bowerbird-shim` under default `release`; shim alone under `--profile release-shim`. Both `--locked`.
  - Artifacts: `bowerbird-${TAG}-${target}.tar.gz` + `.tar.gz.sha256`, `if-no-files-found: error`.
  - Release notes are an **inline heredoc** (release.yml:273-323) → `release-notes.md` → `body_path`. There is no separate template file.
- **`scripts/tarball-smoke-test.sh`** — local-only (NOT in CI by design); mirrors release.yml staging+tar and asserts the extracted layout. Invoke `./scripts/tarball-smoke-test.sh [TAG] [TARGET]`.
- **`INSTALL.md`** — already contains the `xattr -d com.apple.quarantine bin/...` Gatekeeper step (lines 19-31) and the full A–G install walkthrough markers.
- **`tests/release_pipeline_docs.rs`** (420 lines) — doc-drift guardrail (see "Brittle pinned strings").
- Dual-license files (`LICENSE`, `LICENSE-MIT`, `LICENSE-APACHE`) at root; each crate `Cargo.toml` declares `license = "MIT OR Apache-2.0"`.

### Files this story touches

| Path | NEW/UPDATE | Change |
| --- | --- | --- |
| `docs/release-checklist.md` | NEW | The pre-tag runbook (AC #6, Epic 4 retro AI-9). |
| `tests/cross_version_upgrade.rs` | UPDATE | Reconcile the hardcoded `v0.1.0` prior-path / SKIP comment with rc lineage (AC #3). One-line + comment. |
| `.github/workflows/release.yml` | UPDATE (maybe) | Optional `draft:` key if Task 2 chooses path (b). |
| `epic-4-retro-2026-05-25.md` | UPDATE | Strike through AI-9 (and AI-8 once rc2 lands, but AI-8 stays open after rc1). |
| `INSTALL.md`, `README.md` | UNCHANGED (expected) | Only touch if rc1 surfaces a gap; any edit MUST preserve all pinned strings. |

### Critical gotchas

- **draft vs prerelease (AC #1).** `release.yml` has no `draft:` key; `-rc1` publishes a live prerelease immediately on tag push. This is a genuine mismatch with the AC's "draft assets" wording — resolve it explicitly (Task 2), don't paper over it.
- **The `v0.1.0` hardcode (AC #3).** `resolve_prior_version_binary()` falls back to `target/cross-version-installs/v0.1.0/bin/bowerbird-daemon`. In CI the `BOWERBIRD_PRIOR_VERSION_BINARY` env override wins (release.yml sets it from `git tag --sort=-v:refname`, which DOES match `v0.1.0-rc1`), so the hardcode only bites a human populating the conventional path. rc1 has no prior tag → test SKIPs correctly; the guard genuinely lifts at rc2.
- **Brittle pinned strings (AC #7).** `tests/release_pipeline_docs.rs` asserts exact substrings: `cargo test --workspace -- --test-threads=1` (in `ci.yml`), `--profile release-shim`, `--exclude bowerbird-shim`, `--locked` (≥2×), the musl phrase `"musl Linux is deferred post-V1"` + `"(NFR9)"` + `"cargo install --git"` (lives inside the release.yml notes heredoc, easy to miss), the three target triples, all tarball staging entries (`bin/`, `adapters/claude`, `tool-reactions.toml`, `LICENSE`/`LICENSE-MIT`/`LICENSE-APACHE`, `README.md`, `INSTALL.md`, `CHANGELOG.md`), and 12 install walkthrough markers across README + INSTALL. Any reflow that splits or rewords these breaks CI.
- **Tarball naming is templated from `${TAG}`** in both the build job and the notes table; `fail_on_unmatched_files: true` means a naming mismatch fails the release job.

### Test execution

Run the workspace suite **serialized**: `cargo test --workspace -- --test-threads=1`. The daemon contract + CLI E2E suites share process-wide state and hang/flake under parallel execution (Epic 2 retro AI-3; codified in `ci.yml`). The intermittent hang is a documented known issue ([sprint-status.yaml](sprint-status.yaml), Story 5.3 close-out). A full root-cause investigation lives at [investigations/test-serialization-investigation.md](investigations/test-serialization-investigation.md) — the real cause is process-global `std::env::set_var` + an irreversible keyring mock in `contract_daemon.rs::story_3_3_auth`, not the four culprits the CI comment names; do not "fix" the flag as part of this story. The `state_plus_event_atomicity_under_sigkill_during_load` SQLite-teardown deadlock was fixed in Epic 4 (explicit drop ordering) — no longer needs `--skip`.

### Process conventions (from Stories 5.4–5.6)

- Multi-pass adversarial code-review is the norm (5.6 ran four passes). Findings tagged `[Review][Decision]` (needs maintainer call) vs `[Review][Patch]` (mechanical), each with an inline "Maintainer decision (pickles, DATE)" resolution.
- **File List drift is a recurring review finding** — keep the Dev Agent Record File List in sync with `git status --porcelain`.
- The changelog gate (`tests/protocol_changelog_gate.rs`) fires ONLY when a PR touches `crates/protocol/src/*.rs`. This story doesn't, so do NOT manufacture a protocol edit to trigger it.
- ADRs live at `docs/decisions/00NN-*.md` (NOT under `docs/bmad/`). Resolved deferred-work entries are struck through with a backlink, never deleted.

### Project Structure Notes

- Story file lives at `docs/bmad/implementation-artifacts/5-7-release-pipeline-end-to-end-verification.md` (matches sprint-status key `5-7-release-pipeline-end-to-end-verification`).
- New `docs/release-checklist.md` sits at the repo `docs/` root alongside `protocol.md`, `quickstart.md`, `decisions/`, per the layout in [architecture.md:768-769](../planning-artifacts/architecture.md).
- No new crate, no protocol change, no SQLite migration. This is an ops/verification + docs story.

### References

- [Source: docs/bmad/planning-artifacts/epics.md#Story 5.7] — story statement + 5 ACs (lines 1157-1185).
- [Source: docs/bmad/implementation-artifacts/3-4-prebuilt-binary-distribution-and-release-pipeline.md] — what the pipeline is + that it was never run against a real tag (3-4:535-549, File List 3-4:553-561).
- [Source: .github/workflows/release.yml] — trigger 34-42, build matrix 50-74, prerelease 332, cross-version env 230-237, notes heredoc 273-323.
- [Source: tests/cross_version_upgrade.rs] — two-layer SKIP guard 63-85, `resolve_prior_version_binary` 42-59 (hardcoded `v0.1.0` ~53).
- [Source: tests/release_pipeline_docs.rs] — pinned-string guardrails (musl 72-101, walkthrough markers 126-159, ci `--test-threads=1` 201-213, triples 271, staging entries 285-308, `--locked` 330).
- [Source: INSTALL.md] — Gatekeeper xattr step 19-31.
- [Source: docs/bmad/implementation-artifacts/deferred-work.md#Story 3.4] — code-signing/notarization entry (line 83), crates.io (85), Windows scope cut (86).
- [Source: docs/bmad/implementation-artifacts/epic-4-retro-2026-05-25.md] — AI-1/2/3 (bench, →5.5), AI-8 (cross-version SKIP, 281), AI-9 (release-checklist, 282), "first end-to-end smoke" narrative (358).
- [Source: docs/bmad/planning-artifacts/architecture.md#Infrastructure & Deployment] — distribution 515-521, supervision 502-505.
- [Source: docs/bmad/implementation-artifacts/investigations/test-serialization-investigation.md] — `--test-threads=1` root cause (do not touch in this story).

## Dev Agent Record

### Agent Model Used

### Debug Log References

### Completion Notes List

### File List

## Change Log

- 2026-06-02: Story created via bmad-create-story. Comprehensive context-engine analysis completed — release pipeline mapped (3 jobs, never run against a real tag), 5 epic ACs expanded to 7 with the draft-vs-prerelease decision, cross-version SKIP reconciliation, Gatekeeper/deferred-signing boundary, hotfix escape hatch, and the AI-9 release-checklist surfaced. Status → ready-for-dev.
