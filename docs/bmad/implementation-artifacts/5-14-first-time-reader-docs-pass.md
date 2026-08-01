# Story 5.14: First-time-reader docs pass

Status: ready-for-dev

## Story

As a developer who has never seen bowerbird before,
I want the README and quickstart to answer "what is this, why would I care, how do I try it in five minutes" before I bounce,
so that the V1 audience (other developers reachable via the Claude Code community) can decide bowerbird is worth their attention.

**Prose-and-ordering pass over the reader funnel, zero production code, zero protocol surface.** Story 5.13 consolidated the cookbook and retargeted every path; this story is the fresh-eyes rewrite it was sequenced to enable: README motivation-first, quickstart timed against the five-minute claim, presenter-authoring's audience switch named, plus the staleness the 5.13 review and a funnel survey flagged for exactly this pass. Sequenced before Story 5.15 so v0.1.0 ships with docs a stranger can land on.

**Scope boundary:** the reader funnel only (`README.md`, `docs/quickstart.md`, `docs/presenter-authoring.md` first paragraph, the three staleness fixes listed in `staleness-sweep`). No INSTALL.md/README a-g consolidation (test-pinned in both files, see gotchas; explicitly out). No new configuration-reference doc (deferred, see `verify` task's followup bullet). Historical artifacts (`docs/bmad/**` other than prd.md's one stale list, `docs/research/**`) stay untouched.

## Acceptance Criteria

Source: [epics.md:1312-1340](../planning-artifacts/epics.md). Anchors re-verified against the working tree at `cd6e536` (2026-08-01); drift from the epic text is flagged inline.

1. **Given** the current `README.md` **When** a first-time reader (someone who has not read `docs/`, `project-context.md`, or any planning artifact) opens it **Then** within the first screen (~25 lines, before any scroll) they learn: what bowerbird is (one sentence), why it exists (one sentence), and what they can do in five minutes (call to action linking to `docs/quickstart.md`).

2. **Given** the current `docs/quickstart.md` **When** the first-time reader follows it on a fresh machine with neither Claude Code nor bowerbird installed **Then** they complete the quickstart (install bowerbird, run `bowerbird replay`, run one reference example, see live state output) in under five minutes wall-clock. Verification proxy (a literal fresh machine is not reproducible in CI): every command in the doc appears in the order a reader runs them, each with its expected output stated; the command sequence is exercised end to end by the existing smoke (`tests/cli_examples.rs` covers replay + entry execution) plus one manual timed walkthrough recorded in the Dev Agent Record.

3. **Given** the docs path Quickstart → presenter-authoring → protocol → cookbook (PRD §Documentation Requirements, [prd.md:436-446](../planning-artifacts/prd.md)) **When** the first-time reader graduates from Quickstart and reaches `docs/presenter-authoring.md` **Then** the first paragraph names the audience switch ("you've seen it work; now you're going to build something") rather than starting directly in technical detail. The AC's second clause (cookbook cross-references target `docs/cookbook/<name>/` directories, not pre-consolidation `.md` files) was **already satisfied by Story 5.13**; this story verifies it stands (grep, no rework expected).

4. **Given** the README in its current state mentions install before motivation **When** Story 5.14 lands **Then** motivation precedes install, and the "Status: V1 in development" framing is removed. **Epic-text adaptation (disclosed):** the epic's replacement string, "Status: v0.1.0 — first stable release", contains an emdash (banned in added lines, house rule; see 5.13 Completion Note 6 for the precedent) and describes a tag that does not exist until Story 5.15. This story ships an emdash-free, stage-accurate status line (e.g. `Status: v0.1.0 release candidate` with a link to the rc3 release), and adds the final-wording flip to `docs/release-checklist.md` so Story 5.15 lands it at tag time.

5. **Given** the Story 5.14 PR **When** review runs **Then** the review explicitly invokes the `bmad-editorial-review-prose` and `bmad-editorial-review-structure` skills against `README.md` and `docs/quickstart.md`, and the priority-1 findings are addressed in the same PR. (Review-time requirement; record the skill invocations and finding dispositions in this file's Review Findings section.)

## Tasks / Subtasks

Task headers are stable slugs (cite these in commits, not ordinals).

- [ ] **`readme-rewrite` (AC: 1, 4)**
  - [ ] Restructure `README.md` (199 lines today) so the first screen carries: one-sentence what (local daemon that captures Claude Code session events and re-broadcasts them as a typed real-time stream), one-sentence why (your agent sessions are observable by any tool you care to write, not locked in one UI), and the five-minute CTA linking `docs/quickstart.md`. Motivation section moves ABOVE the Install section.
  - [ ] Replace the "Status: V1 in development" framing with the emdash-free, pre-tag status line per AC 4.
  - [ ] Preserve every pinned string byte-for-byte while moving sections (the full pin list is in Dev Notes §Test-pin map; the a-g install walkthrough markers, the musl/NFR9 sentence, `cargo install --git`, and the `](docs/quickstart.md)` / `](docs/protocol.md)` link forms all live in README).
  - [ ] Run `scripts/test.sh --test release_pipeline_docs --test cli_docs_drift` immediately after the README edit, not just at the end.
- [ ] **`quickstart-pass` (AC: 2)**
  - [ ] Resolve the install-step tension first: the AC (and PRD table row 1) counts "install bowerbird" inside the five minutes, but today's quickstart starts at `bowerbird start` with install as a prerequisite link. Either inline the one-command tarball install as step 0 (preferred if it fits the pins; the install one-liner lives in README/INSTALL and is not quickstart-pinned), or keep the prerequisite link and state explicitly that the five-minute clock includes it. Disclose the choice in Completion Notes.
  - [ ] Reread `docs/quickstart.md` (45 lines) as a stranger: every step states what the reader should see before the next step; prerequisites name versions (Node >= 22.6 is test-pinned as the literal `22.6`); the success moment ends with the forward-pointer to `docs/presenter-authoring.md` (PRD §Documentation Requirements table row 1; the pointer string is test-pinned).
  - [ ] Keep all pinned strings (five command names, `BOWERBIRD_TOKEN`, `--experimental-strip-types`, the three troubleshooting phrases; full list in Dev Notes).
  - [ ] One manual timed walkthrough of the full sequence on this machine (install step simulated from the already-built workspace binaries; the timing claim covers the reader's active steps, not compile time). Record wall-clock in the Dev Agent Record. If over five minutes, cut steps or tighten prose until it fits, or document precisely why the claim needs adjusting and adjust the README CTA wording to match.
- [ ] **`presenter-authoring-audience-switch` (AC: 3)**
  - [ ] Rewrite the first paragraph of `docs/presenter-authoring.md` (340 lines; only the opening changes) to name the audience switch: the reader has seen the quickstart work and is now building their own presenter. Do not touch the six test-pinned section headings.
  - [ ] Verify (grep) all cookbook cross-references already target `docs/cookbook/<name>/` per 5.13; fix any straggler found, expect none.
- [ ] **`staleness-sweep` (AC: 1-4 rationale: a first-time reader trusts docs that are true)**
  - [ ] `docs/bmad/planning-artifacts/prd.md:452-457`: rename the "V1 reference examples" list entries to the shipped pattern names with their directories (`state-session-fanout`, `rest-cursor-pagination`, `dropped-frame-recovery` under `docs/cookbook/<name>/`), keeping each entry's one-line pattern description. Closes deferred-work.md §"code review of story-5-13" entry 2 (strike it with the house `**Resolved by Story 5.14:**` form).
  - [ ] `docs/protocol.md:445`: the sentence "Story 4.4 will land the mechanical contract test suite ... until then the discipline is documented + reviewer-enforced" describes shipped behavior in future tense (the suite exists: `tests/protocol_v1_compat.rs`, `tests/contract_test_inventory.rs`). Rewrite in present tense naming the enforcing tests.
  - [ ] `docs/no-list.md:11`: the "**No distro packaging.**" entry claims Homebrew is part of the V1 distribution surface; no formula/tap/workflow step exists. Keep the lead phrase `No distro packaging` byte-for-byte (all 13 lead phrases are test-pinned verbatim); rewrite only the explanation sentence to match reality (prebuilt tarball + `cargo install --git`; Homebrew is not shipped).
- [ ] **`release-checklist-status-flip-note` (AC: 4)**
  - [ ] Add a step to `docs/release-checklist.md` (near its existing hardcoded-v0.1.0 self-notes at lines 166-174): at tag time, flip README's status line to the final released wording (emdash-free form of the epic's "first stable release" framing). This hands the deferred half of AC 4 to Story 5.15 explicitly instead of by memory.
- [ ] **`editorial-review` (AC: 5)** — review-time task, not dev-time
  - [ ] When this story reaches review, invoke `bmad-editorial-review-prose` and `bmad-editorial-review-structure` against `README.md` and `docs/quickstart.md`; triage findings; address every priority-1 finding in the same PR; record dispositions in Review Findings.
- [ ] **`verify` (AC: all)**
  - [ ] `scripts/test.sh` (never raw `cargo test`), `cargo fmt --check`, `cargo clippy --all-targets --workspace -- -D warnings` all green. The suite carries the doc guards: `release_pipeline_docs.rs` (README/INSTALL pins), `cli_docs_drift.rs` (existence, quickstart pins, presenter-authoring headings, no-list lead phrases, link resolution across the whole funnel), `cli_examples.rs` (the quickstart command path, live).
  - [ ] `git diff | grep $'^+.*—'` is empty (no emdashes in added lines; rewritten reader prose is exactly where they creep in; the — escape keeps this file itself sweep-clean).
  - [ ] Final grep sweeps: no `V1 in development` anywhere reader-facing; no `Story 4.4 will` in docs/; prd.md carries the pattern names.
  - [ ] File a followup (taskwarrior) for the two survey items deliberately left out of scope: a public configuration reference for `config.toml` (schema currently only in architecture.md, outside the funnel), and the README/INSTALL a-g contract redundancy (consolidation blocked by dual test pins; decide post-tag whether it is worth the churn).
  - [ ] File List in Dev Agent Record matches `git status --porcelain` (recurring review finding).

## Dev Notes

### The funnel today (verified at `cd6e536`, 2026-08-01)

README.md (199 lines, hub) → docs/quickstart.md (45 lines, five commands: start / replay / auth token / run entry / stop; no Claude Code required by design) → docs/presenter-authoring.md (340 lines) → docs/protocol.md (452 lines, reference) → docs/cookbook/ (index + three five-section entries). INSTALL.md (209 lines) is the tarball-bundled parallel entry point. docs/no-list.md carries the 13 scope cuts. All internal links in the funnel currently resolve (survey verified; `quickstart_internal_links_resolve` enforces a superset). There are no stale `examples/` references left in the funnel; 5.13 cleaned them.

### Test-pin map (the landmines; every one breaks the suite if reworded without updating the test in the same commit)

- **`tests/release_pipeline_docs.rs`** pins in README.md: "musl Linux is deferred post-V1", "(NFR9)", "cargo install --git", the a-g install-walkthrough markers (`~/.claude/settings.json`, `BOWERBIRD_CLAUDE_SETTINGS`, "atomic", `PreToolUse`/`PostToolUse`/`Stop`/`Notification`, `~/.bowerbird/`, `0700`, `--no-start`, `bowerbird uninstall`, `service=bowerbird-daemon`), link forms `](docs/quickstart.md)` and `](docs/protocol.md)`, and the ABSENCE of "in flight under Story 4.3". Same walkthrough markers pinned in INSTALL.md (do not edit INSTALL.md; it is out of scope and doubly pinned).
- **`tests/cli_docs_drift.rs`** pins in docs/quickstart.md: `bowerbird start`, `bowerbird replay`, `bowerbird auth token`, `BOWERBIRD_TOKEN`, `--experimental-strip-types`, `bowerbird stop`, `22.6`, troubleshooting phrases "should now see", `{event:"state"`, "scrolling on stdout", forward-pointers `docs/presenter-authoring.md`, `docs/protocol.md`, `docs/cookbook/`. In docs/presenter-authoring.md: the six `##` section headings in order (The substrate model … Fetching a REST snapshot), the seven ServerMessage variant names, six topic-grammar strings, `Bearer`, `server.json`. In docs/no-list.md: the 13 lead phrases verbatim. Plus link resolution across README, INSTALL, quickstart, presenter-authoring, protocol, no-list, and the four cookbook READMEs.
- **Reordering is safe, rewording pins is not.** AC 1/4's restructure moves sections; the pins are substring checks, so moving a pinned sentence intact is fine. The dangerous edit is "improving" a pinned sentence's wording.
- **Cookbook entry READMEs** (if touched at all, not expected): exactly five `##` headings in order, prose-only fences (bare or `sh` only).
- The prd.md fix is NOT test-pinned (nothing reads prd.md's example list); it drifted silently for that exact reason. The fix is safe but unguarded; cite the deferred-work strike in the commit.

### Critical gotchas

- **Emdash discipline vs the epic's own AC text.** The epic's replacement status string carries an emdash. Do not ship it verbatim; the AC-adaptation in AC 4 is the disclosed resolution (5.13 Completion Note 6 is the precedent for adapting emdash-bearing house forms). Every added line must survive `git diff | grep '^+.*—'` empty.
- **The tag does not exist yet.** 5.14 lands before 5.15. Nothing in the README may claim v0.1.0 is released; the status line says release candidate, and the release-checklist note hands the flip to 5.15.
- **Link scanner limitation (deferred-work, 5.13 review entry 1):** `tests/cli_docs_drift.rs::check_link_target` mishandles CommonMark link titles (`[x](path "title")`) and angle-bracket destinations (`[x](<path>)`). Do not introduce either form in rewritten prose; plain `[text](path)` only. (Fixing the scanner is that deferred entry's business, not this story's.)
- **No protocol touch.** Nothing here goes near `crates/protocol/src`; do not manufacture a protocol-changelog entry. docs/protocol.md's staleness fix is prose about tests, not wire format.
- **Do not edit INSTALL.md.** Its a-g markers are pinned in parallel with README's; the redundancy consolidation is explicitly deferred (verify task files the followup).
- **quickstart's five-minute claim is a wall-clock promise to a stranger.** The timed walkthrough is the honesty check; if the claim fails, fix the doc, not the claim.

### Test execution

Run the workspace suite via `scripts/test.sh`, never raw `cargo test` (project rule, `CLAUDE.md`). Since 2026-08-01 the harness is safe to run with the maintainer's live daemon loaded (`BOWERBIRD_LAUNCH_AGENT_LABEL` isolation seam, PR #40); no daemon babysitting needed. The suite includes `cli_examples.rs`, which spawns a real daemon plus `node --experimental-strip-types` against the cookbook entries; Node 22.6+ must be on PATH.

### Previous story intelligence (5.13, done 2026-08-01)

- 5.13's review added guards this story inherits: the link checker now covers README.md and INSTALL.md; cookbook fence-language allowlist is bare/`sh` only; index-table link forms are pinned. Its Review Findings list is the shape reviews take here (three-layer Opus review, patch/defer triage, all patches same-PR).
- 5.13 Completion Note 6 is the emdash-adaptation precedent AC 4 leans on.
- Recurring review findings to pre-empt: File List drift vs `git status --porcelain`; undisclosed task-text substitutions (disclose every adaptation in Completion Notes, as AC 4 already does in-spec).
- Process: ADRs at `docs/decisions/00NN-*.md` (none expected here); deferred-work entries struck with `**Resolved by Story 5.14:**` + story reference, never deleted.

### Git intelligence

Recent main history is the test-harness/CI hardening PR #40 (label-isolation seam, stop sweep fix, bench ENOTCONN tolerance, bench-job caching, cross-target check) plus 5.13's consolidation. Docs-relevant residue: scripts/test.sh is now safe on a daemon-loaded machine (above), CI's macOS row got ~20s slower (cross-target check), and `docs/superpowers/plans/` now exists (not reader-facing; ignore).

### Web research

Not applicable: docs-only story, no library or API surface. The only version-sensitive claim in the funnel is the Node 22.6 floor, which is test-pinned and unchanged.

### Project Structure Notes

- Story file: `docs/bmad/implementation-artifacts/5-14-first-time-reader-docs-pass.md` (matches sprint-status key `5-14-first-time-reader-docs-pass`).
- Files touched: `README.md`, `docs/quickstart.md`, `docs/presenter-authoring.md` (first paragraph), `docs/no-list.md` (one explanation sentence), `docs/protocol.md` (one sentence), `docs/release-checklist.md` (one step), `docs/bmad/planning-artifacts/prd.md` (one list), `docs/bmad/implementation-artifacts/deferred-work.md` (one strike), this story file, sprint-status.yaml.
- No new files, no moves, no test-file changes expected (pins are preserved, not reworded; if a pin must change, the test updates in the same commit with the reason in Completion Notes).

### References

- [Source: docs/bmad/planning-artifacts/epics.md#Story 5.14] statement + 5 ACs (lines 1312-1340).
- [Source: docs/bmad/planning-artifacts/prd.md#Documentation Requirements] reader path + quickstart contract (436-446); §Code Examples stale list (448-458).
- [Source: docs/bmad/implementation-artifacts/deferred-work.md#code review of story-5-13] entry 1 (link-scanner limitation, avoid the forms), entry 2 (prd stale names, closed here).
- [Source: docs/bmad/implementation-artifacts/5-13-cookbook-consolidation.md] previous story: test-pin discipline, emdash precedent (Completion Note 6), review shape, five-section guard.
- [Source: tests/release_pipeline_docs.rs] README/INSTALL/release.yml pinned strings.
- [Source: tests/cli_docs_drift.rs] quickstart/presenter-authoring/protocol/no-list pins, link checker, cookbook shape guards.
- [Source: docs/bmad/project-context.md#Documentation discipline] reader-path and doc-shape conventions.
- Funnel survey 2026-08-01 (session artifact): reading-path inventory, staleness findings, test-pin map, friction list; findings folded into tasks above.

## Dev Agent Record

### Agent Model Used

### Debug Log References

### Completion Notes List

### File List
