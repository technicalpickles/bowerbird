# 0009. PID supersession: end rolled-over predecessor sessions on the successor's first event

Date: 2026-06-11
Status: Accepted
Deciders: @pickles
Related: ADR-0004 (`docs/decisions/0004-daemon-observed-session-liveness.md` — **extends** it: 0004 ends a session when its `last_pid` is a *dead* OS process; this ADR ends it when that PID has *rolled over* to a newer session, one step earlier than death); ADR-0005 (`idle_prompt` reclassification — no conflict); sprint-change-proposal-2026-06-11-pid-supersession.md (the trigger); Story 5.3 (`docs/bmad/implementation-artifacts/5-3-session-process-liveness-pid-capture.md` — `last_pid` capture, the mechanical fact this rule keys on); Story 5.2 (the `write_if_state_matches` projection-write discipline this reuses); Story 5.11 (the implementation); bean `gt-e9dc` (the trigger + the verification gate, resolved); bean `gt-043a` (the view-side backstop — presenter pid-collapse + staleness, the complement to this daemon fix)
Implementation: `crates/daemon/src/projection/liveness.rs` (new `EndedReason::PidSuperseded` variant — the enum lives here), `crates/daemon/src/projection/session.rs` (new event-driven supersession in the projection write path)
Affects context.md sections: `Durability and chaos`; `Wire format: JSON via serde`; `Substrate-not-actor invariants`

## Context

ADR 0004 gave the daemon a way to end a session it has stopped observing: a background liveness probe ends any session whose `last_pid` is a dead OS process (`kill(pid, 0) == ESRCH` → `EndedReason::PidDead`, or `last_pid IS NULL` on a pre-Story-5.3 row → `NoPidAtUpgrade`). That closes the "terminal was closed mid-turn, no `Stop` fired" gap.

It does not close a second, structurally different gap surfaced during 2026-06-11 dogfooding: **one live `claude` PID hosts many session_ids over its lifetime.** `/clear`, `claude --resume`, and compaction all roll the session_id forward while keeping the same OS process. So a session that has been rolled past is no longer the one the user is in — but its `last_pid` is still a *live* process (the newest session is keeping it alive). The probe's PID-death rule never fires for it. It sits non-`Ended` indefinitely, rendering on every presenter as a live-looking session that will never age out.

**Live proof (2026-06-11, against the running daemon's `~/.bowerbird/bower.db`):** PID 88706 was one live `claude` process backing **4** bowerbird sessions — 1 current `WaitingInput` plus 3 stale `Idle` predecessors, the oldest last emitting 2026-06-10 21:53 (~13h stale). 42 multi-session PIDs existed in the event log. Each predecessor is the same friction the deck's 48-ghost problem (ADR 0004 §Context) was, but invisible to the PID-death probe because the PID is still alive.

The reframe that resolves it parallels ADR 0004's: *a session_id rolling to a successor on the same PID is a mechanical fact — the successor's event literally carries that PID. Observing the rollover is equivalent in nature to observing PID death. The daemon can end the predecessor the instant it sees the successor emit, instead of waiting for the OS to eventually reap the PID.*

This was only safe to do after settling one question (see §"The subagent gate" below): does a Task-tool subagent surface as a *distinct* session_id sharing the parent's PID? If it did, parent and subagent would supersede each other on every event — a ping-pong. The gate was checked against live data and **passed**; that evidence is load-bearing for this decision.

## Decision

**The daemon ends a predecessor session the moment its PID is observed to have rolled to a successor.** Specifically:

### 1. The supersession rule (mechanical fact, event-driven)

When an ingested event for session **S′** carries PID **P**, any **other** non-`Ended` session whose `last_pid == P` is superseded → the daemon emits `SessionEnded { reason: pid_superseded, pid: Some(P), observed_at_ms }` for it and drives its projection `→ Ended`.

Sound because, at any instant, a live PID maps to exactly one OS process, and (gate-verified) one `claude` process surfaces as exactly one bowerbird session_id at a time. So when S′ emits on P, any other non-`Ended` session still claiming P is provably stale.

### 2. New `EndedReason::PidSuperseded`

`EndedReason` is a daemon-internal enum (`crates/daemon/src/projection/liveness.rs`), `#[serde(rename_all = "snake_case")]`, serialized into the `SessionEnded` payload's `reason` field — so the wire value is `"pid_superseded"`. This is the third reason alongside `pid_dead` and `no_pid_at_upgrade`. Presenters that treat `reason` as an opaque string (as they should, per Axiom 4) are unaffected; the addition is wire-compatible.

### 3. It lives in the projection write path, not the probe

The existing `EndedReason`s are emitted by the periodic/startup liveness **probe**. This rule is **event-driven**: it fires in `crates/daemon/src/projection/session.rs`'s write path, after applying S′'s event, under the **same `write_if_state_matches` precondition** (current_state + last_pid still match) and the same writer-pool transaction that Story 5.2 established. That precondition is what keeps it from racing or double-emitting against the probe's `PidDead`/`NoPidAtUpgrade` writes.

### 4. Idempotent and reversible-on-resume

`Ended` is non-terminal (ADR 0004 §1): a `SessionEnded` is an observation, not a verdict.

- **Idempotent:** re-ingesting S′'s event does not re-emit a second `SessionEnded` for the predecessor — the publish-only-on-change rule (Story 5.2) suppresses it because `current_state` is already `Ended`.
- **Reversible-on-resume:** if a superseded predecessor A is later `claude --resume`d, A's new event un-ends it through the normal write path (the path ADR 0004 §1 already relies on), and A — now the live session on its PID — correctly supersedes whatever non-`Ended` session currently claims that PID. The A→B→A-resumed sequence observed in the live data (e.g. PID 4944) lands in the right place: whoever emitted most recently on the PID is the survivor.

### 5. Never supersede the emitter

The rule supersedes *other* sessions claiming P, never S′ itself.

## The subagent gate

The rule is only sound if a subagent does **not** surface as a distinct session_id sharing the parent's PID — otherwise the parent's events would supersede the subagent's session and vice-versa, a ping-pong that ends both repeatedly.

Checked against the live events table before deciding: PID 6491 hosted session `e0215166`, which fired **42 `Agent` (subagent) dispatches** on 2026-06-11, and the **only** session_id ever recorded on PID 6491 is `e0215166`. Zero distinct child sessions. Subagent hooks carry the *parent's* session_id; the subagent does not register as its own co-PID session. (Note: the Task-tool is named `Agent` in current Claude Code; `Task*` tool_names are todo-tracking, not subagents.) The "overlapping session spans on a shared PID" cases that a coarse min/max scan flags turned out to be the resume-next-day + PID-reuse pattern (sequential event bursts, not fine interleaving), which §4 handles.

This evidence is a load-bearing premise. The implementation carries a regression guard (Story 5.11 AC) asserting a subagent dispatch never supersedes its parent, so a future Claude Code change to the subagent model fails loudly rather than silently re-introducing the ping-pong.

## Why this is consistent with Axioms 1 and 4

Axiom 1: *"the substrate observes; it does not interpret."* Axiom 4: *"mechanical facts in the protocol; semantics in the presenter."*

Observing that a PID has rolled to a new session is a mechanical fact — the successor's event literally carries the PID; comparing it against the projection's `last_pid` is a lookup, not a judgment. It is the same *kind* of observation as `kill(pid, 0)` returning `ESRCH` (ADR 0004), one step earlier in time. `reason: pid_superseded` records *which observation* ended the session; it is a fact, not an instruction. The semantic — "hide a superseded session, or dim it, or show its final state" — stays entirely in the presenter. No `superseded_by` lineage, no generation counter, no "this one matters more" ranking enters the substrate (see Alternatives).

## Alternatives considered

- **Extend the probe to detect rollover (probe-only, no write-path change).** The probe runs on a 5s cadence; the successor's event is the *exact* moment the rollover becomes observable, and the write path already owns `last_pid` carry-forward, so doing it there is both more timely and cheaper (no per-tick scan of every projection). The probe stays the safety net for genuine PID *death* (when no live session remains on the PID). Rejected as the primary mechanism; retained as the complement.

- **A `superseded_by` / `generation` field on the projection.** Richer session lineage (which session replaced which, in what order). Rejected: that is presenter-side interpretation of session ancestry. The substrate's job is only to stop calling a stale session live; it does not owe presenters a genealogy. Axiom 1.

- **A one-time reconciliation sweep of already-stranded predecessors on upgrade** (mirroring ADR 0004's `no_pid_at_upgrade` cleanup). Rejected as unnecessary: the forward rule drains the existing backlog on its own (see Consequences), and a dedicated sweep edges toward the `bowerbird gc` the no-list defers post-V1.

- **Key supersession on `transcript_path` instead of PID.** `transcript_path` is per-session (each session_id has its own `.jsonl`), so it is never shared across the very sessions we need to relate. PID is the shared fact, already on the wire since Story 5.3. PID is the correct key.

- **Emit `Stop` for the predecessor instead of `SessionEnded`.** Conflates "agent finished its turn cleanly" with "this session was rolled past" — different presenter affordances. A distinct `SessionEnded { reason: pid_superseded }` is honest about what was observed. Rejected (same reasoning as ADR 0004's rejection of `Stop`-driven death detection).

## Consequences

- **`protocol-changelog.md` adds one entry:** `type: behavioral` — the `SessionEnded` payload `reason` enumeration gains `pid_superseded`. Additive and opaque-string-safe; presenters that switch on `reason` only need to handle the new value if they want to render it distinctly, and old presenters treating it as a string are unaffected.

- **`docs/protocol.md`** updates the `SessionEnded` payload description (the `reason` enumeration at the `SessionEnded` entry) to list `pid_superseded` alongside `pid_dead`/`no_pid_at_upgrade`, framed as a mechanical fact.

- **The live pile-up largely self-heals, forward, with no sweep.** A predecessor stranded on a *still-live* PID is superseded the next time the current session on that PID ingests an event (PID 88706's 3 predecessors end as soon as the live session emits again). Only a predecessor whose PID has *no* remaining live session (the live session itself ended, or the PID died) waits — and that case is exactly what the ADR 0004 death probe already drains. Between the two, the backlog clears without a dedicated reconciliation pass. (This is more complete than the trigger proposal's §6 deferral note anticipated, which framed the forward rule as not catching already-stranded sessions; it does, as long as their PID still has a live session.)

- **New emission site coupled to the probe by precondition.** `session.rs`'s supersession write and `liveness.rs`'s probe write both end sessions; they coexist safely only because both go through `write_if_state_matches` (Story 5.2). Any future change to one must preserve that the other can't observe a half-applied transition.

- **`bowerbird-deck` / pickletown web simplify their staleness handling.** Stale predecessors stop arriving as live rows; presenters lean on `Ended` (filterable since Story 5.8 / ADR 0008) rather than re-deriving "this PID looks rolled-over" client-side. The view-side backstop (`gt-043a`) remains worthwhile for old-daemon compatibility but is no longer the primary defense.

## Revisit when

- **A second adapter lands whose session-lifecycle signal isn't a PID.** Like ADR 0004's probe, supersession is PID-based; a browser/WebSocket-only agent with no PID needs its own rollover signal feeding the same `SessionEnded` event. The session-level wire vocabulary absorbs this; the daemon-side rule does not.

- **Claude Code changes how subagents surface.** If a future release registers a subagent as a *distinct* session_id sharing the parent PID, the §"subagent gate" premise breaks and supersession would ping-pong. The Story 5.11 regression guard is designed to fail in that case; the fix would be a parent/child discriminator (e.g. honoring `bowerbird_ppid` or a subagent marker) before superseding.

- **Session-id rollover stops keeping the PID stable.** If `claude --resume` ever spawns a fresh process with a new PID for the same session_id, supersession naturally no-ops on that path (different PID) and the death probe handles the abandoned predecessor — no code change needed, but the assumption "rollover keeps the PID" is worth re-checking against new Claude Code releases.
