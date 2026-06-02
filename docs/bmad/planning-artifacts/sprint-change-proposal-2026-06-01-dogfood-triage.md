# Sprint Change Proposal — dogfood triage (reboot supervision, shim diagnostic, cwd wire field, sessions filter)

**Date:** 2026-06-01
**Author:** pickles (via correct-course)
**Status:** Accepted (2026-06-01, @pickles)
**Scope classification:** Moderate (backlog reorganization — 4 new Epic 5 stories, 2 ADRs, sprint-status + epics renumber)
**Related:** `docs/dogfooding-feedback.md` (2026-06-01 entries — the trigger); bean `gt-3cnt` (Ended-graveyard retention, partially addressed); ADR 0004 (`Ended` non-terminal, informs Finding 5); Story 5.3 (`bowerbird_ppid` envelope path, the template for Finding 3)

---

## Section 1: Issue Summary

Two live dogfooding sessions on 2026-06-01 surfaced five distinct findings (`docs/dogfooding-feedback.md`). None is a bug in a shipped acceptance criterion; all are friction the dogfooding-validation-phase exists to harvest. They split into two clusters.

**Cluster A — daemon down after a reboot** (session `ad3eaed4-af27-4bb0-9844-f0e237defbc1`). The workstation rebooted; `bowerbird-daemon` did not come back on its own. For ~90 seconds every Claude Code tool call stacked a `PreToolUse/PostToolUse hook error / Failed with non-blocking status code: No stderr output` in the transcript while the shim failed to connect to a missing `~/.bowerbird/ingest.sock`. Tool calls ran fine (the shim never blocks), but every event in the window was dropped, and the surfaced error named neither bowerbird nor the daemon.

- **Finding 1 — nothing supervises the daemon.** No launchd/pitchfork; recovery was manual. Every event between boot and restart is gone. Not on the no-list, so this is unhandled, not an intentional cut.
- **Finding 2 — the surfaced hook error is alarming and causeless.** The exit-1-on-daemon-down contract is deliberate (`crates/shim/src/error.rs::exit_code`, `main.rs:16`), but the only diagnostic is one line in `~/.bowerbird/shim.log`; the shim never writes stderr, so Claude Code renders its generic no-stderr message on every call for the whole outage.

**Cluster B — presenters can only triage on what the wire carries** (pickletown web `/sessions` triage-radar build, against bowerbird-deck). Building a real triage radar exposed that the wire carries mechanical state but nothing to answer "what needs me, and where." Concrete signal: five `WaitingInput` sessions aged 7m–24m buried below two ~50s `Working` sessions, and a "134 ended hidden" footer.

- **Finding 3 — no per-session cwd/repo.** `SessionState`/`SessionListItem`/`Event` carry no location, so a presenter cannot group or filter by repo — the most natural multi-session triage filter. Not on the no-list.
- **Finding 4 — sessions are only identifiable by an 8-char id hash.** This is Finding 3 from the labeling side: a presenter can only label by data on the wire, and there is none a human recognizes. The persona/role model is an intentional no-list cut; the missing *raw fact* (cwd) is not.
- **Finding 5 — `Ended` never evicts.** `Ended` is non-terminal by design (ADR 0004: `claude --resume` revives it), so the daemon can't delete on death, and `SELECT_NON_SENTINEL_SESSIONS` returns the full history — dumped on every presenter via REST and via the snapshot-on-subscribe burst. Two presenters (deck + web) independently re-implement hide-ended client-side. Tracked as bean `gt-3cnt`.

---

## Section 2: Impact Analysis

**Epic impact.** All five land in **Epic 5 (V1 Release Readiness, in-progress)**. No epic is re-scoped or invalidated; this is exactly the friction Epic 5's dogfooding-validation-phase is designed to convert into `5.X` stories (same machinery that produced 5.2, 5.3, 5.6). All four proposed stories gate the v0.1.0 tag (Story 5.10), so they insert ahead of the release-readiness tail.

**Story impact.**
- Four **new** Epic 5 stories (Findings 1, 2, 3, 5-filter). Finding 4 requires no story — Finding 3 resolves it.
- Existing 5.7–5.10 (release-pipeline, cookbook, reader-docs, v0.1.0 tag) renumber to make room; no shipped story's behavior changes.
- Story 5.1 (deck, in-progress) is the consumer that surfaced all of Cluster B; it benefits but needs no AC change.

**Artifact conflicts.**

| Artifact | Finding | Change |
|----------|---------|--------|
| **New ADR 0006** | 3 | Session working directory (`cwd`) as a mechanical fact on the wire |
| **New ADR 0007** | 1 | Daemon start-on-login supervision via launchd LaunchAgent |
| `crates/protocol/src/state.rs`, `event.rs` | 3 | `cwd: Option<String>` on `SessionState` (number-or-null wire field) |
| `crates/adapter-claude/src/normalize.rs` | 3 | Extract `cwd` from the Claude Code hook payload |
| `crates/daemon/src/db/migrations.rs` | 3 | Schema v3: nullable `cwd` column |
| `crates/daemon/src/projection/session.rs` | 3 | Carry-forward `cwd` (overwrite-on-Some, mirrors `last_pid`) |
| `crates/daemon/src/api/sessions.rs`, `db/queries.rs` | 3, 5 | `cwd` in responses; `?state=`/`?since=`/`?limit=` filter on `/sessions` |
| `crates/daemon/src/api/ws.rs` | 5 | Scope the snapshot-on-subscribe burst by the same filter |
| `crates/shim/src/main.rs`, `error.rs` | 2 | One stderr line on the exit-1 (daemon-down) path |
| `src/commands/install.rs` | 1 | `bowerbird install` writes/removes the LaunchAgent plist |
| `docs/protocol.md`, `docs/protocol-changelog.md` | 3, 5 | `cwd` schema entry; `/sessions` filter behavioral entry |
| `docs/no-list.md` | 5 | Reaffirm "no `gc`" cut covers the retention sweep (filter is in scope) |
| `docs/bmad/implementation-artifacts/deferred-work.md` | 2, 5 | Coalescing follow-up; pagination items folded into the filter story |
| `bean gt-3cnt` | 5 | Filter half resolved; retention sweep remains open |

**Technical impact.**
- **Finding 3 is the only wire-protocol change** (ADR trigger: adds a field + schema migration v3). It follows the `bowerbird_ppid` precedent from Story 5.3 exactly — additive field, carry-forward semantics, `#[serde(other)]`-safe for old presenters. Landing it **before** v0.1.0 avoids the post-tag protocol-compat dance (Story 4.4).
- **Finding 5-filter is additive REST** — new optional query params, `type: behavioral` changelog entry, post-tag-safe but cheap now.
- **Finding 1** is install/lifecycle + an ADR; macOS-only (matches the no-list platform cut). The shim stays a pure thin client (no lazy-spawn — that would violate the shim's "no subprocess on the hot path" discipline).
- **Finding 2** is ~5 lines in the shim's failure path; no hot-path cost (the success path stays stderr-silent per the shim discipline).

**Not in scope (see Section 6):** the Finding 5 retention sweep (no-list "no `gc`" cut), Finding 2 cross-invocation coalescing, and optionally bundling `started_at` (deferred Story 5.3 item) into the cwd story.

---

## Section 3: Recommended Approach

**Direct Adjustment (hybrid), no rollback, no MVP redefinition.** Nothing shipped is wrong; the work is four new Epic 5 stories plus two ADRs, slotted ahead of the release tail. This mirrors how every prior dogfooding finding was handled.

| Finding | Disposition | Effort | Risk |
|---------|-------------|--------|------|
| 1 — supervision | New story + **ADR 0007**; launchd LaunchAgent installed by `bowerbird install` | Medium | Medium (launchd plist correctness, uninstall symmetry) |
| 2 — shim stderr | New **minor** story; stderr line on exit-1, keep exit-1 contract | Low | Low (failure-path only) |
| 3 — cwd wire field | New story + **ADR 0006**; adapter-from-payload, schema v3 | Medium | Low (additive, `ppid` precedent) |
| 4 — label | **No story** — resolved by Finding 3; no-personas cut reaffirmed | — | — |
| 5 — sessions filter | New story; `?state=`/`?since=`/`?limit=` + snapshot scoping | Medium | Low (additive REST) |

**Rationale for the splits:**
- **Why launchd, not shim lazy-spawn (Finding 1):** lazy-spawn puts a subprocess fork on the shim's path, violating "No subprocess on the hot path. No git, no tmux, no anything." launchd keeps the shim a pure thin client and gives crash-restart for free.
- **Why adapter-from-payload, not shim-inject (Finding 3):** Claude Code hook payloads already carry `cwd`; the adapter reads it in `normalize`. No shim change, no new syscall, no hot-path cost — the shim's cwd would be Claude's cwd anyway.
- **Why filter-only, not a retention sweep (Finding 5):** the query filter is mechanical fact-filtering (substrate work) and post-tag-safe; the sweep is managed truncation, which the no-list defers post-V1 ("no `gc`"). Projection growth is slow (one row per session, ~1MB over months); the pain today is the unbounded fetch and connect-time burst, which the filter fixes. `gt-3cnt` keeps the sweep tracked.
- **Why Finding 4 needs no story:** the only new label material that is a *mechanical fact* is cwd (Finding 3). First-prompt would require reading transcript content (Axiom 1 violation); branch is a presenter derivation from cwd. The persona model stays cut.

---

## Section 4: Detailed Change Proposals

Four new stories. Proposed numbering inserts them as **5.7–5.10** and renumbers existing **5.7→5.11, 5.8→5.12, 5.9→5.13, 5.10→5.14**; exact numbers finalize at `bmad-create-story` time. Suggested dogfooding-first order: cwd (protocol, land earliest) → sessions-filter → daemon-supervision → shim-diagnostic.

### 4.1 New Story — "Session working directory on the wire" (Finding 3) + ADR 0006

**Intent:** add `cwd` as a mechanical fact so presenters can group/filter by location (and build a recognizable label, closing Finding 4).

- `crates/protocol/src/state.rs`: `SessionState` gains `cwd: Option<String>` (permissive outbound, number-or-null on the wire).
- `crates/protocol/src/event.rs`: `EventEnvelope` carries `cwd: Option<String>` (mirrors `pid`).
- `crates/adapter-claude/src/normalize.rs`: read `cwd` from the hook payload; absent/non-string → `None`, still normalizes successfully.
- `crates/daemon/src/db/migrations.rs`: schema **v3**, nullable `cwd` column.
- `crates/daemon/src/projection/session.rs`: carry-forward (`Some` overwrites, `None` retains prior) — identical to `last_pid`.
- `crates/daemon/src/api/sessions.rs`: `SessionListItem` and `SessionDetail.state` carry `cwd`.
- `docs/protocol.md`: document `cwd` as a mechanical fact (Axiom 4); explicitly note *repo* is a presenter derivation, not a daemon field.
- `docs/protocol-changelog.md`: one `type: schema` entry (additive `cwd`).
- **ADR 0006** (`docs/decisions/0006-session-cwd-on-the-wire.md`): records cwd-is-a-fact / repo-is-interpretation boundary; `Affects context.md sections: Substrate-not-actor invariants, HTTP surface, Wire format`.

**Rationale:** exact shape of the Story 5.3 `bowerbird_ppid` addition — additive, carry-forward, old-presenter-safe.

### 4.2 New Story — "Server-side session filter" (Finding 5-filter)

- `crates/daemon/src/api/sessions.rs::list`: accept `?state=<active|ended|...>`, `?since=<cursor>`, `?limit=<n>`.
- `crates/daemon/src/db/queries.rs`: filtered variants of `SELECT_NON_SENTINEL_SESSIONS`.
- `crates/daemon/src/api/ws.rs`: scope the snapshot-on-subscribe burst by the same predicate so a new presenter isn't blasted with the full graveyard.
- `docs/protocol.md` + `docs/protocol-changelog.md`: `type: behavioral` entry (filter is additive; default unfiltered preserves current behavior).
- Folds in the deferred-work "No pagination on `GET /sessions`" / "No page-size limit" items. **Partially resolves `gt-3cnt`** (filter half); retention sweep stays open on the bean.

### 4.3 New Story — "Daemon start-on-login supervision" (Finding 1) + ADR 0007

- `src/commands/install.rs`: `bowerbird install` writes a `~/Library/LaunchAgents/<label>.plist` (start on login + `KeepAlive` crash-restart); `bowerbird uninstall` removes it (symmetry tested).
- macOS-only; document the platform scope (matches the no-list Windows/Linux-packaging posture).
- **ADR 0007** (`docs/decisions/0007-daemon-start-on-login.md`): launchd-vs-lazy-spawn decision and the shim-stays-thin rationale; `Affects context.md sections: Durability and chaos`.
- Shim is explicitly **not** changed (no lazy-spawn).

### 4.4 New Story (Minor) — "Shim names the cause on daemon-down" (Finding 2)

- `crates/shim/src/main.rs`: on the exit-1 path, also write one human line to stderr, e.g. `bowerbird: daemon not running, event dropped (see ~/.bowerbird/shim.log)`.
- Keep `Error::Connect → exit 1` (NFR20 contract intact); success path stays stderr-silent.
- Per-call coalescing / exit-0-vs-exit-1 reconsideration → **deferred** (Section 6): the shim is stateless per-invocation, so cross-call rate-limiting needs shared state.

### 4.5 Sprint-status + epics

- `docs/bmad/implementation-artifacts/sprint-status.yaml`: add the four stories (status `backlog`), renumber 5.7–5.10 → 5.11–5.14, add a `last_updated` line referencing this proposal.
- `docs/bmad/planning-artifacts/epics.md`: insert the four story sections under Epic 5 with ACs derived from 4.1–4.4; update the change-log header block.

---

## Section 5: Implementation Handoff

**Scope:** Moderate → Product-Owner/Developer coordination (backlog reorganization), then Developer implementation per story.

**Steps:**
1. Approve this proposal (Section 6 of the checklist).
2. Land **ADR 0006** (cwd) and **ADR 0007** (supervision).
3. Create the four stories via `bmad-create-story`, finalizing numbering and sequencing (suggested: cwd → filter → supervision → shim-diagnostic). Decide at create-story time whether to bundle `started_at` into the cwd story.
4. Update `sprint-status.yaml` + `epics.md` (renumber tail, insert stories).
5. Update `gt-3cnt` (filter half scoped here; sweep remains) and `deferred-work.md` (coalescing follow-up).
6. Implement, dogfood against the live daemon, confirm: cwd renders in deck/web triage; `?state=active` drops the graveyard; reboot brings the daemon back automatically; daemon-down now surfaces a named cause.

**Success criteria:** all four stories `done` before Story 5.14 (v0.1.0 tag); workspace tests + fmt + clippy + changelog gate green; a deck/web snapshot grouped by repo with the Ended graveyard filtered server-side; a reboot with no manual daemon restart and no causeless hook-error wall.

---

## Section 6: Deferred / Follow-on Work (NOT proposed here)

- **Finding 5 retention sweep (`gt-3cnt`, post-V1).** Pruning `Ended` `session_projections` rows is managed truncation — the no-list "no `bowerbird gc`" cut stands. Revisit when projection growth (not event-log growth) actually justifies it; gate any sweep on history-completeness like the reprojection facility in the Story 5.6 deferred-work.
- **Finding 2 cross-invocation coalescing (deferred-work).** Rate-limiting/coalescing the per-call error across an outage needs shared state the stateless shim doesn't have. Defer until the named-cause line proves insufficient in practice.
- **`started_at` on `SessionState` (deferred Story 5.3 item #3).** Same presenter-ergonomics shape as cwd; candidate to bundle into the Finding 3 story at create-story time, or keep separate to hold scope tight. Default: cwd-only.
- **Finding 4 — no separate work.** Resolved by Finding 3; the no-personas / agent-roles cut is reaffirmed. First-prompt as a label is an Axiom 1 violation (reading transcript content); branch is a presenter derivation from cwd.
