# Story 5.13: Cookbook consolidation into self-contained directory entries

Status: ready-for-dev

## Story

As the bowerbird maintainer,
I want each cookbook entry to be one self-contained directory under `docs/cookbook/<name>/` containing prose (`README.md`) and runnable code (`src/`, `package.json`, `tsconfig.json`) colocated,
so that the cookbook is the canonical home of the working examples: no duplication, no drift-check, no separate `examples/` surface to navigate.

**Docs/tests/CI reshuffle, zero production Rust code, zero protocol surface.** Story 4.3 shipped `docs/cookbook/` as prose files that copy-paste code out of `examples/`, held in sync by a byte-identity drift-check test. That violates Story 4.2's AC ("automatically reflects the change via include anchors"), Story 4.3's own AC ("inlined via anchor, not copy-pasted"), and project-context §Cookbook discipline ("do not hand-copy snippets, they rot"). Instead of building the deferred inlining mechanism, this story dissolves the problem: `git mv` each example into its cookbook entry (pocketflow pattern: one directory per pattern, README + runnable code colocated), delete the duplicated prose files, delete the drift-check. Closes `deferred-work.md` entry 4 ("Cookbook inlining mechanism", line 104). Full rationale: [sprint-change-proposal-2026-05-26-cookbook-consolidation.md](../planning-artifacts/sprint-change-proposal-2026-05-26-cookbook-consolidation.md).

**Resequenced** 5.5 → 5.7 → 5.8 → 5.12 → **5.13** across four sprint-change proposals (dogfooding-first ordering: reader-facing, not load-bearing for daily use; see the story preamble in [epics.md:1276](../planning-artifacts/epics.md) for the chain). Sequenced before Story 5.14 (first-time-reader docs pass) so 5.14's cross-references target the final directory shape, and before the 5.15 tag so v0.1.0 ships consolidated.

**Scope boundary:** no production Rust code, no `crates/protocol/src` touch (changelog gate will not and should not fire), no new tooling or build step (the whole point is that none is needed). The README/quickstart full rewrite for first-time readers is Story 5.14, not here; this story only retargets paths in reader-facing docs. Historical records (`docs/bmad/**`, `docs/research/**`) keep their `examples/` references untouched.

## Acceptance Criteria

Source: [epics.md:1278-1310](../planning-artifacts/epics.md). Anchors below re-verified against the working tree at `96b996f` (2026-08-01); where the epic text's line numbers or ADR number have drifted, the verified value is used here and the drift is flagged.

1. **Given** the existing `examples/multi-session-router/`, `examples/event-log-viewer/`, and `examples/reconnect-recovery/` directories **When** Story 5.13 lands **Then** they have been `git mv`'d to `docs/cookbook/state-session-fanout/`, `docs/cookbook/rest-cursor-pagination/`, and `docs/cookbook/dropped-frame-recovery/` respectively (directory names are the pattern names, not the example names); the `examples/` directory no longer exists at the repo root; `cargo build --workspace`, the workspace test suite (via `scripts/test.sh`), and the TypeScript smoke tests all pass against the new paths.

2. **Given** the three standalone cookbook prose files (`docs/cookbook/state-session-fanout.md`, `docs/cookbook/rest-cursor-pagination.md`, `docs/cookbook/dropped-frame-recovery.md`) **When** Story 5.13 lands **Then** they have been deleted; their Problem / Approach / Variants content has been folded into the per-entry `docs/cookbook/<name>/README.md` files alongside the existing per-example README content.

3. **Given** each new `docs/cookbook/<name>/README.md` **When** a reader opens it **Then** the README contains no embedded TypeScript code blocks, only prose sections (*What this is*, *Run it*, *How it works*, *How to apply it*, *Files* with relative links to `src/index.ts` and any sidecar code files); code is read by opening `src/index.ts` directly, matching the pocketflow cookbook pattern.

4. **Given** the `// cookbook-begin:<name>` / `// cookbook-end:<name>` comment markers in each `src/index.ts` **When** Story 5.13 lands **Then** the markers have been deleted; the anchor-presence test `each_example_source_carries_cookbook_anchors` has been deleted (it lives in [tests/cli_examples_drift.rs:102](../../tests/cli_examples_drift.rs), not `cli_examples.rs` as the epic text guessed; the epic's "or its current equivalent" clause covers this); the drift-check test `cookbook_include_directives_match_example_anchors` ([tests/cli_docs_drift.rs:227](../../tests/cli_docs_drift.rs)) has been deleted along with its now-dead helpers (`find_directives`, `next_fenced_block_body`, `extract_anchored_region`, `cookbook_consumer_files`).

5. **Given** the smoke-test crate `tests/cli_examples.rs` and CI workflow at `.github/workflows/ci.yml` **When** Story 5.13 lands **Then** all `examples/` path references have been retargeted to `docs/cookbook/` (the smoke's `examples_dir()` at [cli_examples.rs:191-192](../../tests/cli_examples.rs) plus the per-example names it joins, which change to the pattern names per the mapping table in Dev Notes); the `typecheck-examples` job's shell loop over `examples/*/` at [ci.yml:71](../../.github/workflows/ci.yml) retargets to `docs/cookbook/*/`.

6. **Given** the planning and project-context artifacts **When** Story 5.13 lands **Then** `prd.md` (327, 445, 448-450), `architecture.md` (57, 761-829 directory diagram, 926, 957-960, 972, 1039, 1088), and `project-context.md` (203, 244-262 §Repository layout, 323, 534-547 §Cookbook discipline, 584-586, 694-702, 911) have been updated to reflect the single-directory shape (verified anchors; the epic text's `architecture.md:915, 946` and `project-context.md:242-258, 524-545` had drifted); `deferred-work.md:104` is struck through with a backlink to this story's merge commit; path-retarget edits applied to `deferred-work.md` entries at lines 101, 102, 105, 106, 107 per the epic AC, plus line 103 (also references `examples/` paths; discovered during verification, same mechanical retarget).

7. **Given** the project's update protocol (project-context.md L77: every merged ADR includes an `Affects context.md sections:` field) **When** Story 5.13 lands **Then** the consolidation ADR has been authored at `docs/decisions/0010-cookbook-consolidation.md` documenting the decision, considered alternatives (mdBook `{{#include}}`, hand-rolled preprocessor, pocketflow pattern), the chosen path, and `Affects context.md sections: Repository layout, Cookbook discipline`. **Number drift:** the epic text says ADR 0008, but 0008 (server-side session filter, Story 5.8) and 0009 (PID supersession, Story 5.11) were claimed after that text was written; the epic's own escape clause ("claim the next free ADR number then") applies, and 0010 is the next free number (verified against `docs/decisions/` 2026-08-01).

8. **Given** reader-facing docs **When** Story 5.13 lands **Then** `README.md` (lines 7-8, 162-166), `docs/quickstart.md` (lines 10, 19, 45), and `docs/presenter-authoring.md` (lines 5, 210, 240, 285, 305, 327-331) have all `examples/` path references retargeted to `docs/cookbook/<name>/`, and links to the deleted `examples/README.md` and deleted standalone cookbook `.md` files retargeted to their new homes.

## Tasks / Subtasks

Task headers are stable slugs (cite these in commits, not ordinals).

- [ ] **`move-entries` (AC: 1)**
  - [ ] `git mv examples/multi-session-router docs/cookbook/state-session-fanout && git mv examples/event-log-viewer docs/cookbook/rest-cursor-pagination && git mv examples/reconnect-recovery docs/cookbook/dropped-frame-recovery` (mapping table in Dev Notes; `package-lock.json` moves with each, `npm ci` depends on it).
  - [ ] `examples/.gitignore` (if present) moves its rules into the moved dirs or a `docs/cookbook/.gitignore`; `examples/README.md` is handled by `rewrite-cookbook-index` below; after both, `examples/` must not exist.
  - [ ] Sanity: `cd docs/cookbook/state-session-fanout && npm ci && npm run typecheck` (repeat for the other two).

- [ ] **`fold-entry-readmes` (AC: 2, 3, 4)**
  - [ ] For each entry, rewrite `docs/cookbook/<name>/README.md` merging the old standalone `docs/cookbook/<name>.md` prose (Problem / Approach / Variants; the Code section drops, its content already lives in `src/index.ts`) with the old per-example README content (run instructions, file inventory). Target sections in order: *What this is*, *Run it*, *How it works*, *How to apply it*, *Files*. No fenced TypeScript blocks anywhere in these READMEs.
  - [ ] Delete `docs/cookbook/state-session-fanout.md`, `docs/cookbook/rest-cursor-pagination.md`, `docs/cookbook/dropped-frame-recovery.md`.
  - [ ] Delete the `// cookbook-begin:<name>` / `// cookbook-end:<name>` marker lines from each `src/index.ts` (markers only; code unchanged).

- [ ] **`rewrite-cookbook-index` (AC: 2, 8)**
  - [ ] Rewrite `docs/cookbook/README.md` as the cookbook index: fold in `examples/README.md`'s content (Node 22.6 floor, why TypeScript, the no-SDK decision pointer, quick-run block), replace the pairing table with a one-directory-per-pattern table, drop the drift-check sentence (that test is deleted).
  - [ ] `git rm examples/README.md`.

- [ ] **`retarget-tests` (AC: 1, 4, 5)**
  - [ ] `tests/cli_examples.rs`: `examples_dir()` → `docs/cookbook`; the smoke functions join example names, which become pattern names (mapping table); update the stale path in the comment at line ~528. Do NOT rename this file (see gotchas).
  - [ ] `tests/cli_examples_drift.rs`: delete `each_example_source_carries_cookbook_anchors`; retarget `each_example_has_required_files` and `each_example_package_json_declares_node_22_6_engine` to the new dirs/names; rework `architecture_md_describes_examples_as_typescript_not_cargo` to assert the new architecture.md shape (mentions `docs/cookbook/*/package.json`, no Cargo/Rust example shape); delete or retarget `examples_readme_reconciliation_note_present` (its target file is deleted; if the reconciliation note survives into `docs/cookbook/README.md`, retarget, else delete); retarget `examples_not_in_root_cargo_toml_members` to assert `docs/cookbook` is not in `[workspace] members` (the invariant survives: cookbook dirs are a Node zone, not a Cargo zone). Update the file's doc comment.
  - [ ] `tests/cli_docs_drift.rs`: delete `cookbook_include_directives_match_example_anchors` + dead helpers; update `REQUIRED_COOKBOOK_ENTRIES` from the three `.md` paths to the three `docs/cookbook/<name>/README.md` paths; replace `every_cookbook_entry_has_canonical_four_sections` (Problem/Approach/Code/Variants) with the new-shape guard: the five README sections in order (*What this is*, *Run it*, *How it works*, *How to apply it*, *Files*) and no ` ```ts` / ` ```typescript` fenced blocks (this keeps AC 3 honest the same way the old test kept the four-section shape honest).

- [ ] **`retarget-ci` (AC: 5)**
  - [ ] `ci.yml:71`: `for d in examples/*/;` → `for d in docs/cookbook/*/;` (glob matches only the entry dirs; `README.md` at the cookbook root is not a dir). Keep the `typecheck-examples` job id (branch protection does not pin it, verified 2026-08-01, but there is no reason to churn it); updating the display name to mention the cookbook is fine.

- [ ] **`update-planning-artifacts` (AC: 6)**
  - [ ] `prd.md:327` (tech-stack table row), `445` (docs deliverable table row), `448-450` (§Code Examples paragraph): reword per proposal §4.2 (colocated prose + code; the coupling invariant dissolves because there is no second surface to sync).
  - [ ] `architecture.md`: directory diagram at 761-829 (drop the `examples/` subtree at ~784, expand `docs/cookbook/` at ~809 to show one representative entry: README.md, package.json, tsconfig.json, src/index.ts; update the workspace-manifest comment at 772); prose retargets at 57, 926, 957-960, 972, 1039, 1088.
  - [ ] `project-context.md`: 203 (§Example presenters location), 244-262 §Repository layout diagram + "Why this shape", 323 (smoke bullet), 534-547 §Cookbook discipline major rewrite (the entry IS the source of truth; README explains, `src/` runs; no inlining mechanism needed; keep the reference-by-function-name rule for cross-doc references; the reader-path diagram just below stays as-is), 584-586 (test-pyramid table rows), 694-702 (§keeping examples honest), 911 (checklist bullet). Grep `examples` in this file after editing; only historical/quoted mentions may remain.
  - [ ] `deferred-work.md`: strike entry 4 (line 104) with backlink to this story's merge commit (strike-through convention, never delete); mechanical `examples/` → `docs/cookbook/` path retargets in entries at lines 101, 102, 103, 105, 106, 107 (bracketed file-path tails and inline paths; the prose stays historical).

- [ ] **`author-adr` (AC: 7)**
  - [ ] Write `docs/decisions/0010-cookbook-consolidation.md`: Status Accepted (date of merge); context (4.3 shipped duplicate-with-drift-check against its own AC; inlining mechanism deferred and never chosen); decision (single directory per pattern); alternatives considered (mdBook `{{#include}}`: adds a build dependency to solve a problem that only exists because there are two directories; hand-rolled preprocessor: same, plus bespoke tooling debt; pocketflow pattern: proven, zero tooling, dissolves the question); trade-off (docs/ becomes mixed prose+code at a reader-facing surface, already true of docs/bmad/; aesthetic loss, concrete gain); `Affects context.md sections: Repository layout, Cookbook discipline`. Content sketch: proposal §4.5.

- [ ] **`retarget-reader-docs` (AC: 8)**
  - [ ] `README.md:7-8` and `162-166`: point at `docs/cookbook/`; the `examples/README.md` link at 166 retargets to `docs/cookbook/README.md`.
  - [ ] `docs/quickstart.md:10` (the `examples/README.md` link), `19` (the run command path), `45` (the cookbook/examples pairing sentence, which simplifies to one surface).
  - [ ] `docs/presenter-authoring.md:5, 210, 240, 285, 305, 327-331`: retarget paths; links to deleted standalone `.md` entries become links to `docs/cookbook/<name>/` (line 285's "pinned via a cookbook-include directive that fails CI on drift" sentence must be rewritten, that mechanism is gone).
  - [ ] Final sweep: `grep -rn "examples/" README.md INSTALL.md docs/ --include="*.md" | grep -v "docs/bmad" | grep -v "docs/research"` returns only intentional survivors (expected: none).

- [ ] **`verify` (AC: 1, all)**
  - [ ] `scripts/test.sh` (never raw `cargo test`; parallel is the default), `cargo fmt --check`, `cargo clippy --all-targets --workspace -- -D warnings` all green. The suite includes the retargeted smoke (`cli_examples.rs` spawns node against the new paths) and the release-docs pinned-string guard (`release_pipeline_docs.rs`), which must stay green after the README/ci.yml edits.
  - [ ] `git diff | grep $'^+.*\u2014'` is empty (no emdashes in added lines; generated prose reliably reintroduces them; the \u2014 escape keeps this file itself sweep-clean).
  - [ ] File List in Dev Agent Record matches `git status --porcelain` (recurring review finding).

## Dev Notes

### Current shape (verified at `96b996f`, 2026-08-01)

- `examples/{multi-session-router,event-log-viewer,reconnect-recovery}/`: each has `src/index.ts`, `package.json`, `package-lock.json`, `tsconfig.json`, `README.md` (44-58 lines); `reconnect-recovery` additionally has `tests/recover.test.ts`. Plus `examples/README.md` (the index).
- `docs/cookbook/`: `README.md` (11-line index) + three standalone prose files (78, 80, 107 lines) each shaped Problem / Approach / Code / Variants, with the Code section a fenced block byte-checked against the example's anchor region.
- The only cross-directory relative import is `dropped-frame-recovery`'s test importing `../src/index.ts`, which is depth-invariant under the move. No example references `fixtures/` or any path outside its own directory (verified by grep). The moves are mechanically safe.

### Name mapping (memorize; the dirs are renamed, not just moved)

| Old example dir | New cookbook dir | Old anchor/entry name |
|---|---|---|
| `examples/multi-session-router/` | `docs/cookbook/state-session-fanout/` | `state-session-fanout` |
| `examples/event-log-viewer/` | `docs/cookbook/rest-cursor-pagination/` | `rest-cursor-pagination` |
| `examples/reconnect-recovery/` | `docs/cookbook/dropped-frame-recovery/` | `dropped-frame-recovery` |

### Critical gotchas

- **ADR number is 0010, not 0008.** The epic AC text predates ADR 0008 (Story 5.8) and 0009 (Story 5.11). Its own escape clause resolves this; do not create a second 0008.
- **`each_example_source_carries_cookbook_anchors` lives in `tests/cli_examples_drift.rs:102`**, not `cli_examples.rs`. The epic AC named the wrong file with an "or its current equivalent" hedge.
- **Do not rename `tests/cli_examples.rs`.** `tests/release_pipeline_docs.rs:184-200` references it by name in its release.yml Node-setup assertions. Renaming the file breaks that guard; only its internal paths change.
- **`release_pipeline_docs.rs` pins exact substrings in `README.md`, `INSTALL.md`, and `ci.yml`** (12 install-walkthrough markers across README + INSTALL, several ci.yml assertions; see Story 5.12's Dev Notes "Brittle pinned strings"). The README line 162-166 edits and the ci.yml loop edit are near those pins. Run `scripts/test.sh --test release_pipeline_docs` after each of those edits, not just at the end. INSTALL.md itself has zero `examples/` references (verified); do not edit it.
- **Branch protection** (verified via `gh api` 2026-08-01) requires only `ci (ubuntu-latest)`, `ci (macos-latest)`, and the two shim bench gate rows. `typecheck-examples` is not a required check, so retargeting it cannot strand a required status, but keep the job id stable anyway.
- **`examples/README.md` is link-targeted** by `docs/quickstart.md:10` and `README.md:166`. Deleting it without retargeting those links leaves dead links that no test catches.
- **The four-section cookbook test must not simply be deleted.** `every_cookbook_entry_has_canonical_four_sections` and `required_docs_exist` (via `REQUIRED_COOKBOOK_ENTRIES`) currently guard the old shape; replace them with the new-shape equivalents (five README sections, no fenced TS) so AC 3 stays machine-enforced. Deleting the guard without a successor is the kind of coverage regression review flags.
- **Do not sweep historical artifacts.** `docs/bmad/**` (story files, retros, proposals, sprint-status history) and `docs/research/**` legitimately reference `examples/` as records of what was true. Only living surfaces change (the AC 6/8 lists are exhaustive).
- **No protocol touch.** Nothing here goes near `crates/protocol/src`; do not manufacture a protocol-changelog entry.
- **Emdash discipline.** Folded/rewritten prose must not carry emdashes into added lines even where the source text had them; the pre-commit sweep in `verify` is the gate.

### Test execution

Run the workspace suite via `scripts/test.sh`, never raw `cargo test` (project rule, `CLAUDE.md`; a second concurrent cargo-test process in this worktree is the confirmed hang trigger). The suite is parallel by default and includes `cli_examples.rs`, which spawns a real daemon plus `node --experimental-strip-types` against the entry sources; Node 22.6+ must be on PATH. `npm ci && npm run typecheck` per entry mirrors the CI `typecheck-examples` job.

### Process conventions (carried from Stories 5.4-5.12)

- ADRs live at `docs/decisions/00NN-*.md`, not under `docs/bmad/`. Resolved deferred-work entries are struck through with a backlink, never deleted.
- File List drift is a recurring review finding: keep it synced with `git status --porcelain`.
- Multi-pass adversarial review is the norm; findings tagged `[Review][Decision]` vs `[Review][Patch]` with inline maintainer resolutions.
- The changelog gate fires only on `crates/protocol/src/*.rs` changes; this story does not touch it.

### Project Structure Notes

- Story file: `docs/bmad/implementation-artifacts/5-13-cookbook-consolidation.md` (matches sprint-status key `5-13-cookbook-consolidation`).
- After this story, `docs/cookbook/` is the single reference-example surface: an index `README.md` plus three self-contained entry directories, shape-identical to `pocketflow/cookbook/<entry>/`. Root `Cargo.toml` `[workspace] members = ["crates/*"]` is untouched (cookbook dirs are a Node zone; the drift test keeps asserting the exclusion, retargeted).
- No new crate, no migration, no bench impact. CI gets marginally faster (one drift-check test gone).

### References

- [Source: docs/bmad/planning-artifacts/epics.md#Story 5.13] statement + 8 ACs (lines 1270-1310).
- [Source: docs/bmad/planning-artifacts/sprint-change-proposal-2026-05-26-cookbook-consolidation.md] full rationale, per-artifact change table (§2), README section shape + ADR sketch (§4).
- [Source: tests/cli_examples_drift.rs] six guard functions, anchors test at 102, workspace-members invariant at 162.
- [Source: tests/cli_docs_drift.rs] `REQUIRED_COOKBOOK_ENTRIES` 37-41, four-section test 85-110, drift-check 227+, helpers 126-225.
- [Source: tests/cli_examples.rs] `examples_dir()` 191-192, entry spawn 206, stale comment 528.
- [Source: .github/workflows/ci.yml] `typecheck-examples` job 46-73, loop at 71.
- [Source: tests/release_pipeline_docs.rs] pinned strings referencing cli_examples.rs 184-200; walkthrough markers (see 5-12 Dev Notes).
- [Source: docs/bmad/project-context.md#Cookbook discipline] 534-547 (the section this story rewrites); §Repository layout 244-262.
- [Source: docs/bmad/implementation-artifacts/deferred-work.md] entry 4 at 104 (closes); path retargets 101-103, 105-107.
- [Source: docs/bmad/implementation-artifacts/5-12-release-pipeline-end-to-end-verification.md] previous story: test-execution discipline, pinned-string gotchas, process conventions.

## Dev Agent Record

### Agent Model Used

### Debug Log References

### Completion Notes List

### File List

## Change Log

- 2026-08-01: Story created via bmad-create-story. Ultimate context engine analysis completed: all epic ACs re-verified against the working tree at `96b996f`; drifted anchors corrected (ADR 0008 → 0010, `cli_examples.rs` → `cli_examples_drift.rs` for the anchors test, architecture/project-context line numbers); deferred-work line 103 added to the retarget list; new-shape guard test specified as the successor to the four-section test.
