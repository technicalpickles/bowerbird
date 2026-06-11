# Sprint Change Proposal — PID supersession (end rolled-over predecessor sessions)

**Date:** 2026-06-11
**Author:** pickles (via correct-course)
**Status:** Accepted (2026-06-11, @pickles)
**Scope classification:** Moderate (one new Epic 5 story + ADR 0009 + sprint-status/epics renumber of the release tail)
**Related:** bean `gt-e9dc` (this trigger, with the verification gate already resolved); bean `gt-043a` (view-side backstop — presenter pid-collapse + staleness, the complement to this daemon fix); ADR 0004 (daemon-observed session liveness — the mechanism this extends); Story 5.3 (`last_pid` carry-forward, the fact this rule keys on); Story 5.2 (`write_if_state_matches` projection-write discipline this reuses)

---

## Section 1: Issue Summary

bowerbird's liveness probe (`crates/daemon/src/projection/liveness.rs`) only ends a session when its `last_pid` is a **dead OS process** (`kill(pid, 0) == ESRCH` → `EndedReason::PidDead`, or `last_pid IS NULL` → `NoPidAtUpgrade`). But a single live `claude` PID hosts **many** session_ids over its lifetime: `/clear`, `claude --resume`, and compaction all roll the session_id while keeping the same process. So every rolled-over predecessor session stays non-`Ended` forever, as long as the newest session keeps that PID alive.

**Live proof (2026-06-11, against the running daemon's `~/.bowerbird/bower.db`):** PID 88706 is one live `claude` process backing **4** bowerbird sessions — 1 current `WaitingInput` plus 3 stale `Idle` predecessors, the oldest last emitting 2026-06-10 21:53 (~13h stale). PID 18484 backed 2; 42 multi-session PIDs exist in the log. Each predecessor renders on every presenter (deck, pickletown web) as a live-looking session that will never age out, because the daemon never observes its PID die.

This is exactly the friction Epic 5's dogfooding-validation-phase exists to convert into a `5.X` story (same machinery that produced 5.2, 5.3, 5.6). Nothing shipped is wrong: the probe's PID-death rule is correct as far as it goes; it just doesn't cover the **one-live-PID-many-sessions** case.

**Verification gate — already resolved (PASS).** The proposed rule (below) is only sound if Task-tool subagents do **not** surface as distinct session_ids sharing the parent PID — otherwise parent and subagent would supersede each other in a ping-pong. Checked against the live events table: PID 6491 hosted session `e0215166`, which fired **42 `Agent` (subagent) dispatches** today, and the **only** session_id ever recorded on PID 6491 is `e0215166`. Zero distinct child sessions. Subagent hooks carry the parent session_id, so supersession cannot ping-pong. (Note: the Task-tool is named `Agent` in current Claude Code; `Task*` tool_names are todo-tracking, not subagents.) The coarse "overlapping session spans on a shared PID" cases in the scan turned out to be the **resume-next-day + PID-reuse** pattern (sequential event bursts, not fine interleaving), which Section 3 handles explicitly.

---

## Section 2: Impact Analysis

**Epic impact.** Lands entirely in **Epic 5 (V1 Release Readiness, in-progress)**. No epic is re-scoped or invalidated. One new story; it gates the v0.1.0 tag, so it inserts ahead of the release-readiness tail.

**Story impact.**

- **One new** Epic 5 story (PID supersession). Slots in as **Story 5.11**.
- Existing **5.11→5.12** (release pipeline), **5.12→5.13** (cookbook), **5.13→5.14** (reader docs), **5.14→5.15** (crates.io + v0.1.0 tag) renumber to make room. No shipped story's behavior changes; the renumbered stories are `ready-for-dev`/`backlog` (no work started).
- Rationale for the slot: this is a **projection-correctness** fix in the family of 5.2 / 5.3 / 5.6, and the sprint-status sequencing principle is "correctness fixes adjacent to dogfooding, then CI/release hardening, then reader-facing polish, then the tag." Correctness lands ahead of the release/docs/tag tail.

**Artifact conflicts.**

| Artifact | Change |
|----------|--------|
| **New ADR 0009** | `docs/decisions/0009-pid-supersession.md` — event-driven supersession of co-PID predecessor sessions; soundness argument (one live PID = one process at any instant), subagent-safety evidence, resume/un-end interaction, and why event-driven (not probe-driven). Extends ADR 0004. |
| `crates/daemon/src/projection/liveness.rs` | Add `EndedReason::PidSuperseded` variant (the enum lives here today; `#[serde(rename_all = "snake_case")]` → wire value `"pid_superseded"`). |
| `crates/daemon/src/projection/session.rs` | **New emission site** (the bean's "ingest/projection write path"): after applying an event for session S′ carrying PID P, emit `SessionEnded { reason: PidSuperseded }` for every **other** non-`Ended` session whose `last_pid == P`, under the same writer-pool txn / `write_if_state_matches` precondition discipline as the probe. |
| `docs/protocol.md` | Extend the `SessionEnded` payload `reason` enumeration (`:291`) to include `pid_superseded`; note it is a mechanical fact (which observation triggered the end), interpretation left to presenters (Axiom 4). |
| `docs/protocol-changelog.md` | One `type: behavioral` entry — additive `reason` value; presenters that treat `reason` as an opaque string (as they should) are unaffected; default outbound surface otherwise unchanged. |
| `docs/bmad/implementation-artifacts/sprint-status.yaml` | Insert Story 5.11 (`backlog`), renumber 5.11–5.14 → 5.12–5.15, add a `last_updated` line referencing this proposal. |
| `docs/bmad/planning-artifacts/epics.md` | Insert the Story 5.11 section under Epic 5 with ACs derived from §4.1; renumber the tail story headers; update the change-log header block. |
| bean `gt-e9dc` | Move toward `in-progress` when the story is created; verification findings already recorded on it. |
| bean `gt-043a` | Unaffected here — the view-side backstop (presenter pid-collapse + staleness) is the complement, explicitly out of scope (Section 6). |

**Technical impact.**

- **New emission site, not the probe.** The existing `EndedReason`s are emitted by the periodic/startup **liveness probe**. This rule is **event-driven**: it fires in the projection write path the instant an event for a session arrives carrying a PID that another non-`Ended` session still claims. The probe is unchanged; this is additive.
- **Soundness.** At any instant a live PID maps to exactly one OS process, and (gate-verified) one process surfaces as exactly one bowerbird session_id at a time. So when S′ emits on PID P, any other non-`Ended` session claiming P is provably stale → safe to end. This is the same observation ADR 0004 already relies on for `PidDead`, applied one step earlier (we don't wait for the OS to reap the PID; the rollover itself is the signal).
- **Resume / un-end is already handled.** `liveness.rs` documents the invariant: "Already `Ended` — do not re-emit. A resume hook event drives the transition out via `projection::session::write`'s normal path." So a predecessor superseded today that later gets `claude --resume`d un-ends through the normal write path, and — correctly — then supersedes whoever currently holds its PID. The story's job is to make this **idempotent and reversible-on-resume** and to cover it with a regression test (A→B supersedes A; resume A → A un-ends and supersedes B). This is an explicit AC, **not** deferred.
- **Additive wire change.** `EndedReason` derives `Serialize` only (outbound; presenters parse the string). Adding `pid_superseded` is backward-compatible for any presenter treating `reason` as opaque (Axiom 4). `type: behavioral` changelog entry, not a schema break.
- **No shim change, no hot-path cost.** Pure daemon-side projection logic; the shim already injects `bowerbird_ppid` (Story 5.3) — the fact this rule keys on is already on the wire.
- **Axiom 4 fit.** `PidSuperseded` is a mechanical fact (which observation ended the session), not an interpretation. "Hide vs dim a superseded session" stays a presenter concern.

---

## Section 3: Recommended Approach

**Direct Adjustment, no rollback, no MVP redefinition.** Nothing shipped is wrong; the work is one new Epic 5 story plus ADR 0009, slotted ahead of the release tail. Mirrors how every prior dogfooding finding was handled (2026-06-01 dogfood-triage).

| Item | Disposition | Effort | Risk |
|------|-------------|--------|------|
| PID supersession | New Story 5.11 + **ADR 0009**; event-driven emission in `projection/session.rs`, `EndedReason::PidSuperseded` | Medium | Low–Medium (txn ordering vs the probe; resume/un-end correctness — both covered by ACs + tests) |

**Rationale for the key choices:**

- **Why event-driven, not a smarter probe.** The probe runs periodically; keying supersession off the **ingest** of the successor's event ends the predecessor immediately on rollover instead of up to one probe-interval later, and it reuses the per-event write path that already owns `last_pid` carry-forward. The probe stays the safety net for genuine PID death.
- **Why a new ADR rather than amending 0004.** 0004 establishes daemon-observed liveness via PID **death**. Supersession is a distinct mechanism (rollover, not death) with its own soundness argument and a verification gate worth recording. One-decision-per-ADR matches the 0005–0008 precedent. ADR 0009 references and extends 0004.
- **Why the resume interaction is an AC, not a deferral.** The live data shows the A→B→A-resumed pattern is real (e.g. PID 4944). The fix must be reversible-on-resume or it would wrongly strand a resumed session as `Ended`. The normal write path already does the un-end; the story just has to prove it and prove supersession is idempotent.

---

## Section 4: Detailed Change Proposals

### 4.1 New Story 5.11 — "Session PID supersession (end rolled-over predecessors)"

**Intent:** when a live `claude` PID rolls from one session_id to the next, end the predecessor immediately instead of leaving it as a stale never-`Ended` session on every presenter.

- `crates/daemon/src/projection/liveness.rs`: add `EndedReason::PidSuperseded` (wire value `"pid_superseded"`).
- `crates/daemon/src/projection/session.rs::write` (or its txn body): after applying an event for session S′ with PID P, find every **other** non-`Ended` session whose `last_pid == P` and emit a synthetic `SessionEnded { reason: PidSuperseded, pid: Some(P), observed_at_ms }` for each — under the same `write_if_state_matches` precondition (current_state + last_pid still match) the probe uses, so it can't race the probe or stomp a concurrent transition. Must never supersede S′ itself.
- `docs/protocol.md` (`:291`): extend the `reason` enumeration with `pid_superseded`; reaffirm reason-is-a-fact (Axiom 4).
- `docs/protocol-changelog.md`: one `type: behavioral` entry (additive reason value, opaque-string-safe).
- **Acceptance criteria (to be finalized at create-story):**
  1. Given live PID P backing session A (non-`Ended`), when an event for a new session B arrives carrying PID P, A transitions to `Ended` with `reason = pid_superseded`; B is unaffected.
  2. Supersession is **idempotent** — re-ingesting B's event does not re-emit a second `SessionEnded` for A (publish-only-on-change, per Story 5.2).
  3. **Reversible on resume** — if A is later resumed (a new event for A arrives on some PID), A un-ends via the normal write path, and A then supersedes whatever non-`Ended` session currently claims that PID.
  4. Supersession respects the `write_if_state_matches` precondition and does not race or duplicate the liveness probe's `PidDead`/`NoPidAtUpgrade` writes.
  5. A Task-tool (`Agent`) subagent dispatched by a session never triggers supersession of its parent (regression guard for the verified gate).
  6. `docs/protocol.md` + changelog updated; workspace tests + fmt + clippy + changelog gate green.

**Rationale:** keys on the `last_pid` fact Story 5.3 already carries; reuses Story 5.2's projection-write discipline; extends ADR 0004's liveness model one step earlier than PID death.

### 4.2 New ADR 0009 — "PID supersession"

`docs/decisions/0009-pid-supersession.md`. Records: the one-live-PID-many-sessions problem; the event-driven supersession rule and its soundness (one live PID = one process = one session at any instant); the subagent-safety verification evidence (PID 6491 / 42 `Agent` dispatches / zero child sessions); the resume/un-end interaction; event-driven vs probe-driven; relationship to ADR 0004. `Affects context.md sections: Durability and chaos, Wire format, Carried mechanical facts on SessionState`.

### 4.3 Sprint-status + epics renumber

- `sprint-status.yaml`: add `5-11-session-pid-supersession: backlog`; renumber `5-11-release-pipeline-end-to-end-verification` → 5.12, `5-12-cookbook-consolidation` → 5.13, `5-13-first-time-reader-docs-pass` → 5.14, `5-14-crates-io-namespace-and-v0-1-0-tag` → 5.15; add a `last_updated` line citing this proposal.
- `epics.md`: insert the Story 5.11 section (ACs from §4.1); renumber the four tail story headers (5.11–5.14 → 5.12–5.15); update the Epic 5 change-log header block.

---

## Section 5: Implementation Handoff

**Scope:** Moderate → backlog reorganization (renumber the release tail), then Developer implementation per story.

**Steps:**
1. Approve this proposal.
2. Land **ADR 0009**.
3. Create Story 5.11 via `bmad-create-story` (the bean `gt-e9dc` body is a near-complete spec, including the resolved verification gate and the resume wrinkle).
4. Update `sprint-status.yaml` + `epics.md` (renumber tail, insert story).
5. Move bean `gt-e9dc` to `in-progress`; leave `gt-043a` (view-side backstop) tracked and out of scope.
6. Implement → dogfood against the live daemon → confirm: rolling a session (`/clear` or resume) immediately ends the predecessor with `reason: pid_superseded`; deck/web no longer show the stale-predecessor pile-up; a resumed predecessor correctly comes back and supersedes the current PID holder.

**Success criteria:** Story 5.11 `done` before Story 5.15 (v0.1.0 tag); workspace tests + fmt + clippy + changelog gate green; the live multi-session-per-PID pile-up (PID 88706's 3 stale predecessors) no longer reproduces; regression tests cover idempotence, resume-reversal, and subagent non-supersession.

---

## Section 6: Deferred / Follow-on Work (NOT proposed here)

- **View-side backstop (`gt-043a`, separate).** Presenter-side PID-collapse + staleness dimming is the complement to this daemon fix and stays its own bean — a presenter should still degrade gracefully if a daemon hasn't shipped supersession yet (old-daemon compatibility). Not in scope; the daemon fix is the substrate-correct primary.
- **A dedicated one-time reconciliation sweep of already-stranded predecessors.** Not needed, and **not** proposed (no-list "no `gc`" posture). The forward rule drains the existing backlog on its own: a predecessor stranded on a *still-live* PID is superseded the next time the current session on that PID emits an event; a predecessor whose PID has *no* remaining live session is caught by the existing ADR 0004 death probe. Between the two the pile-up self-heals without a reconciliation pass. (ADR 0009 §Consequences corrects an earlier framing here that treated the forward rule as not catching already-stranded sessions — it does, as long as their PID still hosts a live session.)
