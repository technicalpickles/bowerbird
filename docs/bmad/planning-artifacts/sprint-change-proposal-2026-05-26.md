# Sprint Change Proposal — Epic 5: V1 Release Readiness (dogfooding → presenter → hardening)

Date: 2026-05-26
Author: pickles (via bmad-correct-course)
Status: Draft — pending §6 sign-off
Related: epic-4-retro-2026-05-25.md (AI-1..AI-6), `docs/bmad/implementation-artifacts/deferred-work.md`

## 1. Issue summary

All four MVP epics are `done` in `docs/bmad/implementation-artifacts/sprint-status.yaml`. The PRD's stated V1 success gate is experiential:

> V1 success gate: pickles can build and iterate on local example tools against live Claude Code sessions, with multiple tools running simultaneously, without any instrumentation changes between experiments.

That gate has not been exercised yet. The reference examples in `examples/` pass smoke tests under `bowerbird replay`, but no one — including pickles — has run bowerbird against a live Claude Code session for sustained daily use. The Epic 4 retrospective names this explicitly:

> The "next epic" question is genuinely empty — this is the right time to shift focus to V1 release prep, not new feature work. [...] None of these justify a "Epic 5" framing; they're pre-tag checklist items.

This proposal argues the opposite: a coherent Epic 5 framing is the right shape, because release-prep is not just a checklist — it's three sequenced concerns that depend on each other.

1. **Dogfooding.** Install on the maintainer's main machine and run for sustained use. Validates the PRD success gate. This is the truth-seeking step that has been deferred since the project started.
2. **Presenter.** A first-party tool the maintainer will actually use daily, living in a sibling repo (Axiom 1: substrate observes, presenter interprets — interpretation does not belong in `crates/`). Without this, "dogfooding" has nothing to look at; with it, dogfooding becomes a real signal source.
3. **Public-release hardening.** CI gates converted from aspirational to load-bearing (Epic 4 retro AI-1..AI-3), release pipeline tested end-to-end (Epic 4 retro AI-5 + cross-version upgrade), and the docs optimized for the first-time reader who has never seen bowerbird before.

Dogfooding has to come before hardening because hardening designed without real-use signal optimizes the wrong things. The presenter has to come before sustained dogfooding because without it, "what would I actually look at" has no answer beyond running an example periodically.

### Why this is an Epic, not a punch list

The Epic 4 retro recommended absorbing this work as "pre-tag checklist items." That framing under-counts the work. The deferred-work backlog alone names:

- `bowerbird install` auto-copy of `tool-reactions.toml` (Epic 3 retro AI-4 + Story 3.4 deferred)
- `CatchPanicLayer` middleware not yet wired (Story 2.1 deferred)
- `/sessions/{id}/events` 404 alignment for unknown sessions (Story 4.1 deferred)
- Typecheck CI lane for examples (Story 4.2 deferred)
- Migration idempotency test on populated DB (Story 1.2 deferred)
- Daemon-bench baselines unseeded (Epic 4 retro AI-1)
- Bench gates never exercised in failure mode (Epic 4 retro AI-2, AI-3)
- Cross-version upgrade test SKIP guard load-bearing on first v0.1.0 tag (Epic 4 retro)
- Crates.io namespace verification (Epic 3 retro AI-3 / Epic 4 retro AI-5)

Six stories' worth of concrete work, plus the docs first-time-reader pass, plus a presenter, plus dogfooding. The story-driven cadence (CS → DS → CR) is the right shape for that.

### Evidence

- `docs/bmad/implementation-artifacts/sprint-status.yaml` — all four epics `done`
- `docs/bmad/implementation-artifacts/epic-4-retro-2026-05-25.md` lines 178, 248-260, 274-279 — V1 release pre-flight scope named, six action items enumerated
- `docs/bmad/implementation-artifacts/deferred-work.md` — twelve+ release-relevant items across stories 1.2, 2.1, 3.4, 4.1, 4.2
- `docs/bmad/planning-artifacts/prd.md` lines 73-76 — V1 success gate is dogfooding-based and has not been run
- `.github/workflows/{ci,release}.yml` — exist, but the release pipeline has never produced a real-tag release

## 2. Impact analysis

### Epic impact

- **Epic 1 (capture/REST)** — closed; unaffected except for one deferred item from Story 1.2 (migration idempotency on populated DB) folded into Epic 5 Story 5.4.
- **Epic 2 (WS streaming)** — closed; one deferred item (`CatchPanicLayer`) folded into Story 5.4. Bench-gate seeding (AI-1/AI-2) lands in Story 5.2.
- **Epic 3 (install/lifecycle/auth/release)** — closed; AI-4 (auto-copy `tool-reactions.toml`) folded into Story 5.4; AI-3 (crates.io namespace verification) folded into Story 5.6.
- **Epic 4 (DX/replay/docs/protocol-compat)** — closed; retro AI-1..AI-5 fold into Epic 5 as concrete stories.
- **Epic 5 (new)** — V1 Release Readiness: 6 stories, sequenced after a dogfooding-validation phase marker.

### Artifact impact

| Artifact | Change |
|---|---|
| PRD | 2 amendments: (a) "MVP Strategy & Philosophy" section gains a "Phase 2: Release Readiness" subsection naming the dogfooding-presenter-hardening sequence; (b) "Post-MVP Features" loses the "first external tool author" gate framing in favor of "first dogfooded tool author = pickles; first external comes after v0.1.0 tag." Neither amendment changes any FR/NFR. |
| Epics | New Epic 5 block at the bottom of `epics.md` with 6 stories. New FR/NFR coverage rows are minimal — Epic 5 work is primarily hardening of existing FRs, plus one new presenter tool that is explicitly *outside* the bowerbird crates (sibling repo, no new FR). |
| Architecture | 1 amendment: under `### Frontend Architecture` (currently "N/A — headless"), add a "Companion Projects" note pointing at the sibling-repo presenter and clarifying that interpretation/presentation does not enter `crates/`. |
| Sprint status | New Epic 5 block with 6 stories at `backlog`, plus a `dogfooding-validation-phase` comment marker between Epic 4 and Epic 5. |
| Deferred work | Six entries get folded into Epic 5 stories with backlinks; no entries deleted (the resolution sites are still load-bearing for traceability). |
| project-context.md | No changes. All new work is consistent with the existing axioms; no new ADRs triggered by this proposal alone. |

### Technical impact

The technical change surface is concentrated in two places:

1. **bowerbird repo:** 5 stories of in-repo hardening work (5.2 bench seeding, 5.3 release pipeline E2E, 5.4 install UX + middleware, 5.5 docs pass, 5.6 crates.io + v0.1.0 tag). All consistent with the existing architecture.
2. **Sibling repo (new):** Story 5.1 produces a new repo (`bowerbird-statusbar` or similar — naming decision deferred to the story). This is a presenter, not a substrate; per Axiom 1 it does not modify any bowerbird crate, only consumes the WebSocket + REST API as a third-party tool would.

No protocol changes. No ADRs triggered by this proposal alone. One potential protocol-changelog entry from Story 5.4's `/sessions/{id}/events` 404 alignment (deferred from Story 4.1; behavioral change with `type: behavioral` entry).

### Dogfooding bug capture

Bugs found during the dogfooding phase between Story 5.1 (presenter shipped) and Story 5.6 (v0.1.0 tag) are handled ad-hoc:

- Trivial fixes: standalone PRs against `main`, no story file. Documented in commit messages and `protocol-changelog.md` if user-facing.
- Non-trivial fixes: a new `5.X-hotfix-<topic>` story is created inline, slots into the sprint between the next two planned stories, and is reviewed via the normal CS → DS → CR cycle.

This is the same pattern Epic 2/3/4 used for retro carryover folds; nothing new about it.

## 3. Recommended approach

**Selected: Option 1 — Direct adjustment.** A new Epic 5 in `epics.md`, with a `dogfooding-validation-phase` marker in `sprint-status.yaml` before the Epic 5 stories. PRD gets a small "Phase 2: Release Readiness" paragraph. Architecture gets a brief "Companion Projects" note.

Rationale considering:

- **Effort and timeline impact** — Bounded. 6 stories, most small-to-medium. The longest is Story 5.1 (presenter); the rest are concentrated cleanup.
- **Technical risk and complexity** — Low. No protocol changes proposed; existing axioms unchanged; all in-repo work is hardening of already-shipped code paths.
- **Sequencing risk** — Real but managed. Dogfooding before hardening is the right order; the presenter (5.1) is the only forward-dependency on dogfooding having a useful surface to observe. If 5.1 reveals that "just look at the JSON" is sufficient for the maintainer's purposes, 5.1's scope can shrink mid-flight.
- **Long-term sustainability and maintainability** — Improves. Converts every "aspirational" CI gate to load-bearing, closes 6 deferred-work entries, ships the protocol-changelog enforcement against a real cross-version test.
- **Stakeholder expectations and business value** — Aligned with the PRD's stated success gate. The PRD has always said V1 = "pickles uses it for real"; this is the proposal that finally executes that.

### Alternatives considered

- **Option A — Treat as pre-tag checklist items per Epic 4 retro recommendation.** Tempting because the work is partially enumerated already. Rejected because (a) presenter scope is non-trivial and deserves story-level shape, (b) docs first-time-reader optimization is real work that doesn't fit a checklist, (c) dogfooding-driven bug fixes need a sprint cadence to land cleanly, (d) the story framework's CR (code review) step is genuinely valuable for the install UX and middleware changes in 5.4.
- **Option B — Two epics, hardening + docs separately.** Rejected because docs work and hardening work are interleaved — Story 5.5 (docs) consumes the results of 5.3 (release pipeline E2E) and 5.4 (install UX) to write accurate first-time-reader instructions. Splitting them would require either out-of-order docs (stale within weeks) or artificial sequencing.
- **Option C — Skip presenter; use existing examples.** Rejected by the maintainer's stated preference. The deeper rationale: the three examples in `examples/` are pattern demonstrations, not daily-use tools. Dogfooding against pattern demonstrations is testing the substrate's instructional clarity, not its production fitness. A real first-party presenter is what generates the bug signal that 5.2-5.6 should be informed by.
- **Option D — Defer dogfooding until v0.1.0-rc1 lands.** Rejected because rc1 → tag is the *wrong* phase to discover that the substrate is hard to use daily. Dogfooding wants to land *before* rc1 so its bug signal informs rc1's gates.

## 4. Detailed change proposals

### 4.1 PRD — `docs/bmad/planning-artifacts/prd.md`, append to `### MVP Strategy & Philosophy` (after line 146)

**Insert this paragraph as the new closing paragraph of MVP Strategy & Philosophy:**

```
**Phase 2 — Release Readiness (Epic 5):** V1 ships in two beats. Phase 1 (Epics
1–4, complete) produced the capture + streaming + install + replay substrate.
Phase 2 validates and hardens it: the maintainer installs bowerbird on their
main machine, builds a first-party presenter in a sibling repository, uses it
daily, and harvests the friction. Bugs found in that loop become hotfix stories
folded into the Epic 5 cadence; the planned Epic 5 stories then convert the CI
gates from aspirational to load-bearing, exercise the release pipeline
end-to-end, polish the install UX, and rewrite the README and quickstart for
the first-time reader who has not been in the room while bowerbird was built.
The v0.1.0 tag is the closing event of Phase 2.
```

### 4.2 PRD — small clarification to "Post-MVP Features" (lines 168-175)

**No textual change required.** The "first external tool author" gate stays as-is; Phase 2 above already clarifies that the dogfooded tool author = pickles is internal to V1, not the same signal.

### 4.3 Architecture — `docs/bmad/planning-artifacts/architecture.md` line 493 (`### Frontend Architecture`)

**Before:**

```
### Frontend Architecture

N/A. bowerbird has no UI; presenters are external consumers of the WebSocket
and REST surfaces.
```

**After:**

```
### Frontend Architecture

N/A in this repository. bowerbird has no UI; presenters are external
consumers of the WebSocket and REST surfaces.

**Companion projects (out of scope for `crates/`).** A first-party presenter
shipped alongside V1 lives in a sibling repository, not in this crate
workspace. Per Axiom 1 (the substrate observes; it does not interpret),
interpretation belongs in a presenter, and a presenter is structurally a
*consumer* of bowerbird — not a component of it. Sibling-repo conventions
(naming, license, install path) are documented in the presenter repo itself,
not here. See Epic 5 Story 5.1 for the V1 first-party presenter.
```

### 4.4 Epics — append new Epic 5 block to `docs/bmad/planning-artifacts/epics.md`

Insert after the closing of Story 4.4 (current end of file). New block:

```markdown
---

## Epic 5: V1 Release Readiness

The maintainer installs bowerbird on their main machine, builds a first-party
presenter in a sibling repository, runs it daily against live Claude Code
sessions, and harvests the friction. The planned stories below convert the CI
gates from aspirational to load-bearing, exercise the release pipeline
end-to-end against a real tag, polish the install UX, and rewrite the README +
quickstart for a first-time reader. Closing event: v0.1.0 tagged on GitHub
Releases.

**FRs covered:** primarily hardening of FRs already covered by Epics 1–4. No
new FRs introduced.
**NFRs covered:** strengthens NFR1, NFR2 (bench gates load-bearing); NFR19
(protocol stability, cross-version upgrade test load-bearing).

### Story 5.1: First-party presenter tool (sibling repository)

As the bowerbird maintainer,
I want a real presenter tool I can use daily against live Claude Code sessions,
So that dogfooding has a useful surface to observe — not just JSON in a
terminal — and the friction I find informs the rest of Epic 5.

**Acceptance Criteria:**

**Given** a sibling repository (naming decision finalized during story creation; candidate names include `bowerbird-statusbar`, `bowerbird-deck`)
**When** I run the presenter against a locally running `bowerbird` daemon connected to a live Claude Code session
**Then** the presenter surfaces session state (idle / working / waiting-on-input) and recent tool-use activity in a form the maintainer finds useful for daily work — exact UI form (terminal TUI, menu bar, web UI, etc.) decided during story creation

**Given** the presenter is installed on the maintainer's main machine
**When** the maintainer codes with Claude Code for at least 5 working days
**Then** the presenter is the maintainer's actual signal source for "is Claude doing something" — used in preference to alt-tabbing to the terminal

**Given** the presenter is in a sibling repository, not in `crates/` or `examples/`
**When** a reader of the bowerbird repository looks at architecture.md §Frontend Architecture
**Then** they find a backlink to the presenter's repository, with a one-sentence justification that interpretation does not belong in the substrate

**Given** the presenter consumes the WebSocket and REST API
**When** any aspect of consumption is awkward (auth flow, snapshot-on-connect, dropped-frame handling, reconnect behavior)
**Then** the awkwardness is captured as a `5.X-hotfix-<topic>` story or as a deferred-work entry against bowerbird, *not* worked around silently in the presenter

**Given** the presenter codebase
**When** the maintainer reaches a "this is the V1 presenter" milestone (subjective)
**Then** a README in the sibling repo names: required bowerbird version, how to install, how to run, and the one cookbook pattern from `docs/cookbook/` the presenter most directly demonstrates

### Story 5.2: Bench gates converted to load-bearing

As a release manager,
I want every committed CI bench gate to fail loudly when a real regression
lands, so that the bench infrastructure is producing signal — not just running.

Closes Epic 4 retro AI-1, AI-2, AI-3 (this retrospective's Action Items table).

**Acceptance Criteria:**

**Given** `crates/daemon/benches/baselines/macos.json` and `linux.json` currently contain placeholder zero values
**When** Story 5.2 lands
**Then** both files contain non-zero p99 values per shape (solo, fanout3, burst, steady) sourced from the most recent green CI run on `main` (or the Story 5.2 PR's CI run if it's green); the bench gate `daemon-bench-gate` exercises the regression check without auto-skipping any shape

**Given** the daemon-bench gate has never been exercised in failure mode
**When** Story 5.2 lands
**Then** the Dev Agent Record documents two chaos-injection sanity PRs (one macOS, one Linux) that injected `tokio::time::sleep(50ms)` between `tx.commit()` and `broadcaster.publish` in `crates/daemon/src/projection/session.rs::write`, verified CI's daemon-bench-gate failed on the burst-shape p99 regression, and were reverted before merge

**Given** the shim hot-path bench gate has never been exercised in failure mode (Story 4.4 Task 4.3 deferred)
**When** Story 5.2 lands
**Then** the Dev Agent Record documents two chaos-injection sanity PRs (one per platform) that injected `std::thread::sleep(Duration::from_millis(2))` into the shim's hot path, verified CI's shim-bench-gate failed, and were reverted before merge

**Given** the work is paperwork-flavored (no production code changes after the chaos PRs are reverted)
**When** Story 5.2 closes
**Then** the deferred-work entries naming AI-1/AI-2/AI-3 are struck through with a backlink to this story's merge commit

### Story 5.3: Release pipeline end-to-end verification

As a release manager,
I want the GitHub Releases pipeline driven to a real (non-prerelease) tag,
producing artifacts that install and run on a fresh machine,
So that v0.1.0 is the second release we cut — not the first.

**Acceptance Criteria:**

**Given** the release workflow at `.github/workflows/release.yml`
**When** a `v0.1.0-rc1` tag is pushed
**Then** the workflow produces tarballs for `aarch64-apple-darwin`, `x86_64-apple-darwin`, and `x86_64-unknown-linux-gnu`, attached to the GitHub Release as draft assets

**Given** a fresh macOS arm64 machine (or VM, or wiped `~/.bowerbird/` and `~/.claude/settings.json` backup-and-restore)
**When** the maintainer downloads the `v0.1.0-rc1` tarball, runs `tar -xz`, then `bowerbird install`, and starts a Claude Code session
**Then** events appear in `~/.bowerbird/bower.db`, the daemon is running, and the first-party presenter from Story 5.1 receives state frames

**Given** the cross-version upgrade contract test `cross_version_upgrade.rs`
**When** Story 5.3 lands
**Then** its SKIP guard (currently load-bearing on the absence of a real prior tag) is removed or asserts against `v0.1.0-rc1`'s data directory, depending on which boundary is tested

**Given** Gatekeeper warnings on first run of unsigned macOS tarball binaries
**When** the maintainer follows `INSTALL.md`'s `xattr -d com.apple.quarantine ...` step
**Then** the binary runs successfully; this is documented as the V1-acceptable path and the deferred-work entry for code-signing/notarization remains open (cost decision: post-V1)

**Given** the rc1 release surfaces a behavioral, install, or release-pipeline issue
**When** the maintainer escalates it
**Then** a `5.X-hotfix-<topic>` story is created inline before moving to Story 5.4

### Story 5.4: Install UX polish and middleware closure

As a first-time user,
I want `bowerbird install` to leave my system in a fully working state without manual file shuffling,
And as a release manager,
I want the missing-on-purpose middleware (`CatchPanicLayer`) wired before V1
exposes the daemon to a wider audience.

Folds in five deferred-work entries; no new design surface.

**Acceptance Criteria:**

**Given** a user runs `bowerbird install` from a freshly extracted prebuilt tarball
**When** the install completes
**Then** `~/.bowerbird/adapters/claude/tool-reactions.toml` is present, seeded from the bundled file (Epic 3 retro AI-4 / Story 3.4 deferred-work entry "bowerbird install auto-copies tool-reactions.toml"); if the file already exists with user modifications, it is left untouched and a warning is logged

**Given** an HTTP handler panics inside the daemon
**When** the panic happens
**Then** `CatchPanicLayer` (Story 2.1 deferred-work entry) returns a structured `500` JSON response and the daemon continues serving other requests, rather than the panic bubbling to axum's default close-the-connection path

**Given** the TypeScript reference examples under `examples/`
**When** CI runs against a PR
**Then** a new `Typecheck examples` job runs `tsc --noEmit` against each example (Story 4.2 deferred-work entry "Typecheck CI lane for examples"); type errors fail the build

**Given** a populated SQLite database with prior-version schema
**When** the daemon starts and `run_migrations` runs against it
**Then** a migration-idempotency contract test verifies a second `run_migrations` call against the now-migrated DB is a no-op (Story 1.2 deferred-work entry "Migration idempotency on a populated DB is untested")

**Given** a request to `GET /sessions/{id}/events` for a session_id that has never existed
**When** the daemon processes it
**Then** the response is `404 Not Found` rather than `200 {events: [], cursor: None, ...}` (Story 4.1 deferred-work entry "/sessions/{id}/events 404 for unknown sessions"); a `type: behavioral` entry lands in `docs/protocol-changelog.md` documenting the alignment; `bowerbird export` drops its pre-check round trip

### Story 5.5: First-time-reader docs pass

As a developer who has never seen bowerbird before,
I want the README and quickstart to answer "what is this, why would I care,
how do I try it in five minutes" before I bounce,
So that the V1 audience (other developers reachable via the Claude Code
community) can decide bowerbird is worth their attention.

**Acceptance Criteria:**

**Given** the current `README.md`
**When** a first-time reader (defined as someone who has not read `docs/`, `project-context.md`, or any planning artifact) opens it
**Then** within the first screen they learn: what bowerbird is (one sentence), why it exists (one sentence), and what they can do in five minutes (call to action linking to `docs/quickstart.md`)

**Given** the current `docs/quickstart.md`
**When** the first-time reader follows it on a fresh machine with neither Claude Code nor bowerbird installed
**Then** they complete the quickstart (install bowerbird, run `bowerbird replay`, run one reference example, see live state output) in under five minutes wall-clock

**Given** the docs path Quickstart → presenter-authoring → protocol → cookbook (PRD §Documentation Requirements line 436)
**When** the first-time reader graduates from Quickstart and reaches `docs/presenter-authoring.md`
**Then** the first paragraph names the audience switch ("you've seen it work; now you're going to build something") rather than starting directly in technical detail

**Given** the README in its current state mentions install before motivation
**When** Story 5.5 lands
**Then** motivation precedes install; the "Status: V1 in development" framing is removed in favor of "Status: v0.1.0 — first stable release" once Story 5.6 tags it

**Given** the Story 5.5 PR
**When** review runs
**Then** the review explicitly invokes the `bmad-editorial-review-prose` and `bmad-editorial-review-structure` skills against `README.md` and `docs/quickstart.md`, and the priority-1 findings are addressed in the same PR

### Story 5.6: Crates.io namespace decision and v0.1.0 tag

As the project owner,
I want a deliberate decision on crates.io publishing,
And the v0.1.0 tag pushed, so V1 is shipped.

Closes Epic 3 retro AI-3 / Epic 4 retro AI-5.

**Acceptance Criteria:**

**Given** `cargo search bowerbird`
**When** Story 5.6 is started
**Then** the namespace availability is documented (available / squatted / taken-by-related-project); if available, the four workspace crates are published with `description`, `repository`, `keywords`, `categories`, and `[package.metadata.docs.rs]` blocks added to each `Cargo.toml`; if not available, an ADR documents the renaming decision or the decision to publish under a different namespace

**Given** all Epic 5 stories 5.1 through 5.5 are complete and any hotfix stories are merged
**When** the maintainer tags `v0.1.0`
**Then** the release workflow runs end-to-end producing artifacts; the GitHub Release is published (not draft); release notes name the V1 scope, the dogfooding signal that motivated the tag, and the contract-test summary

**Given** the v0.1.0 tag exists
**When** the maintainer reads `docs/bmad/implementation-artifacts/deferred-work.md`
**Then** every entry referenced in this Epic 5 (Story 5.2 AI-1/AI-2/AI-3, Story 5.4's five entries, Story 5.6's AI-3/AI-5) is struck through with a backlink to its closing story's merge commit

**Given** the v0.1.0 release notes
**When** a first-time reader (Story 5.5's audience) finds them
**Then** they include the install one-liner, a link to Quickstart, and an honest statement of "what works today and what doesn't" (the deferred-work entries that remain — code-signing, second-adapter, etc.)
```

### 4.5 Sprint status — `docs/bmad/implementation-artifacts/sprint-status.yaml`

Append after the Epic 4 block (after line 80, `epic-4-retrospective: done`):

```yaml

  # Dogfooding validation phase (no formal stories; bugs become 5.X-hotfix stories ad-hoc)
  # Begins after Story 5.1 (presenter) ships; runs in parallel with Stories 5.2–5.6.
  dogfooding-validation-phase: in-progress  # transitions to 'done' on v0.1.0 tag

  # Epic 5: V1 Release Readiness (dogfooding → presenter → hardening)
  epic-5: backlog
  5-1-first-party-presenter-tool: backlog
  5-2-bench-gates-load-bearing: backlog
  5-3-release-pipeline-end-to-end-verification: backlog
  5-4-install-ux-polish-and-middleware-closure: backlog
  5-5-first-time-reader-docs-pass: backlog
  5-6-crates-io-namespace-and-v0-1-0-tag: backlog
  epic-5-retrospective: optional
```

And update the comment header `# last_updated:` to reflect 2026-05-26 with the Epic 5 addition.

### 4.6 Deferred-work — `docs/bmad/implementation-artifacts/deferred-work.md`

No file edits in this proposal. Each Epic 5 story strikes its consumed entries on close (per the existing convention). The proposal binds them by reference, not by pre-emptive strike.

## 5. Implementation handoff

**Scope classification: Moderate.** The proposal introduces a new epic (planning surface), but the work itself is concentrated, low-architectural-risk, and almost entirely consumes already-named deferred items.

| Recipient | Responsibility |
|---|---|
| `pickles` | (a) Approve this proposal in §6. (b) Apply the §4 edits to PRD, architecture, epics, sprint-status. (c) Decide the sibling-repo name for Story 5.1 (or punt that decision into Story 5.1's CS step). |
| `bmad-create-story` (per-story, in the order shipped) | Generate the per-story implementation file for each of 5.1 → 5.6 from the epic block when each becomes next in cadence. |
| `bmad-dev-story` (downstream of each CS) | Implement each story. Story 5.1 produces a sibling repo; the rest produce in-repo work. |
| `bmad-code-review` (per story) | Standard CR cycle. Note: Story 5.5 explicitly invokes `bmad-editorial-review-prose` and `bmad-editorial-review-structure` as part of its CR. |
| `bmad-retrospective` (Epic 5 close) | Optional per `epic-5-retrospective: optional`; recommended because Epic 5 is the V1-ship cycle and lessons will be load-bearing for any post-V1 work. |

### Sequencing

Stories run in numeric order with one explicit exception: **Story 5.1 must complete before sustained dogfooding begins** (otherwise dogfooding has no surface). Stories 5.2–5.4 can run in parallel with each other if capacity allows, but each requires its own CS/DS/CR cycle. Story 5.5 (docs) is best run *after* 5.3 and 5.4 because it needs accurate install/release content. Story 5.6 is the closing event and must be last.

### Success criteria

- Epic 5 ships v0.1.0 tagged on GitHub Releases.
- All six AI items from the Epic 4 retro Action Items table are closed (with backlinks).
- All six deferred-work entries named in this proposal are struck through (with backlinks).
- The maintainer is using bowerbird daily for real work — measured by "did I alt-tab to the terminal to check on Claude this week, or did I look at the presenter."
- First-time reader can complete the Quickstart in under five minutes on a fresh machine.
- CI's daemon-bench-gate and shim-bench-gate have both been exercised in failure mode at least once (chaos-injection sanity PRs documented).

## 6. Acknowledgements

- The Epic 4 retrospective (`epic-4-retro-2026-05-25.md`) named V1-release-prep as the right next scope and enumerated 6 of the 6 action items folded into Story 5.2 and Story 5.6.
- The deferred-work registry's discipline — never deleting entries, always striking with backlinks — is what made it possible to source Epic 5's stories from existing artifacts rather than inventing scope.
- The PRD's V1 success gate (line 76: "pickles can build and iterate on local example tools against live Claude Code sessions") has been load-bearing through 4 epics of work; Epic 5 is finally the proposal that executes against it.
- Axiom 1 (substrate observes; does not interpret) is the load-bearing constraint that puts the Story 5.1 presenter in a sibling repository rather than in `crates/`.
