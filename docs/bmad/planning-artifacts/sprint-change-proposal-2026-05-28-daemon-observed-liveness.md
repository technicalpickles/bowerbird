# Sprint Change Proposal — Daemon-observed session liveness + typed-notification WaitingInput

Date: 2026-05-28
Author: pickles (via bmad-correct-course)
Status: Approved 2026-05-28
Related: `sprint-change-proposal-2026-05-27-pid-liveness.md` (this proposal amends Story 5.3's design while keeping its `last_pid` capture); `docs/decisions/0004-daemon-observed-session-liveness.md` (the ADR this proposal operationalizes); Story 5.1 `docs/bmad/implementation-artifacts/5-1-first-party-presenter-tool.md` (dogfooding source); Story 5.2 `docs/bmad/implementation-artifacts/5-2-session-state-projection-correctness.md` (whose PostToolUse "preserve prior" rule this amendment refines); `docs/protocol.md` (`SessionCurrentState`, `EventKind`); `crates/daemon/src/projection/state.rs` (transition function); `crates/adapter-claude/src/normalize.rs` (notification_type extraction)

## 1. Issue summary

Story 5.3 (Session-process liveness via PID capture, `backlog`) was approved 2026-05-27 with a presenter-side `kill(pid, 0)` liveness recipe. During Story 5.1 dogfooding of `bowerbird-deck` against a live Claude Code workstation on 2026-05-27 → 2026-05-28, two further findings compounded on the original PID-liveness gap and forced a reframe of the substrate-vs-presenter line:

### Finding 1 — `Notification → WaitingInput` is interpretation in disguise

Story 1.6 collapses every `Notification` hook event to `current_state = WaitingInput` (`crates/daemon/src/projection/state.rs:54`), discarding the typed `notification_type` field that Claude Code populates on the payload (six documented values per the Claude Code hooks doc: `permission_prompt`, `idle_prompt`, `auth_success`, `elicitation_dialog`, `elicitation_response`, `elicitation_complete`). In the dogfooding corpus (358 Notification events):

- 60% (214) carry `notification_type: idle_prompt`
- 40% (142) carry `notification_type: permission_prompt`
- 0% of the other four documented types appeared

Mapping all six types to a single `WaitingInput` state forces presenters into a one-bit conclusion that is wrong half the time — `idle_prompt` is the *least* actionable of the input-required types (just a reminder), but the substrate treats it identically to `permission_prompt` (a genuine block on agent progress). The substrate is being more opinionated than Claude is. Discussion captured in the design conversation 2026-05-27 → 2026-05-28; conclusion in ADR 0004 §1, Finding 3.

### Finding 2 — Presenter-side `kill()` breaks cross-machine deployment and forces work-per-presenter

Story 5.3 as approved asks every presenter to implement its own `kill(pid, 0)` loop. Two problems:

- **Cross-machine consumers cannot do this.** A presenter on machine B watching a daemon on machine A cannot `kill()` machine A's PIDs — they're different process namespaces. The PRD's loopback-bind constraint (single-host V1) papers over this but doesn't resolve it.
- **N presenters means N liveness loops.** The same syscall runs in every presenter, with no shared cache, no shared event-log entry, no postmortem record of when the session ended.

Recognizing these as a single problem: observing process death is a **mechanical fact**, equivalent in nature to observing a hook firing. The daemon already emits other mechanical observations (hook events from the ingest socket; `RecordingStarted`/`RecordingEnded` sentinels from its own lifecycle). Process-death is a third such observation, just per-session. Discussion in ADR 0004 §"Why this is consistent with Axioms 1 and 4 (refined)."

### Combined effect on bowerbird-deck

The deck against the maintainer's accumulated workstation history (`~/.bowerbird/bower.db` 2026-05-28): 48 sessions stuck at `WaitingInput`, all >10 min stale, 38 between 1-24h, 8 >24h. None are actually waiting for input — they're sessions whose terminals closed without firing `Stop`, frozen on the last `Notification` they emitted. Without typed-`notification_type` rules AND daemon-observed death, no presenter (this one or any future one) can reach a useful state-of-the-world view.

### Evidence

- `crates/daemon/src/projection/state.rs:54` — `EventKind::Notification → SessionCurrentState::WaitingInput` (blind collapse; no notification_type inspection)
- `crates/daemon/src/projection/state.rs:233` — test `current_state_for_read_does_not_stale_waiting_input` (no stale fallback for WaitingInput)
- `crates/adapter-claude/src/normalize.rs:73` — adapter recognizes `"Notification"` hook_kind but does NOT extract `notification_type` from payload
- `docs/bmad/implementation-artifacts/1-6-session-projection-and-hook-unreliability-tolerance.md:314` — Story 1.6 explicitly punted finer-grained semantics to presenters; the punt has not aged well now that a real presenter exists
- `~/.bowerbird/bower.db` Notification corpus (2026-05-28): 60% `idle_prompt`, 40% `permission_prompt`, 0% of the other four documented types
- `~/.bowerbird/bower.db` stuck-session corpus (2026-05-28): 48 sessions stuck on Notification with last_event_at ages of 1-24h+
- Claude Code hooks doc: `notification_type` enumerates six values (`permission_prompt`, `idle_prompt`, `auth_success`, `elicitation_dialog`, `elicitation_response`, `elicitation_complete`)
- MCP 2025-06-18 elicitation spec: clarifies elicitation_dialog/response/complete lifecycle

## 2. Impact analysis

### Epic impact

- **Epic 1, Epic 2, Epic 3, Epic 4 (closed, retro'd):** no reopen.
- **Epic 5 (in-progress; 5.1 in-progress, 5.2 done, 5.3 backlog):** Story 5.3 amends in-place. No story renumbering. No new story inserted. No epic resequencing.

### Story impact

| Story | Action |
|---|---|
| 5.1 (First-party presenter tool) | no change to AC; the amendment unblocks the rest of Task 5 (dogfooding window) because the deck becomes useful |
| 5.2 (Session state projection correctness) | done — the PostToolUse → "preserve prior" rule introduced in 5.2 is *refined* (not reverted) by this amendment to → "Working unconditionally"; flagged in `protocol-changelog.md` as a behavioral entry |
| **5.3 (amended)** | scope expands: daemon-side liveness probe + `SessionEnded` event + `Ended` state + notification_type-driven WaitingInput rules + PostToolUse → Working refinement; AC count grows from 13 to ~19 |
| 5.4, 5.5, 5.6, 5.7, 5.8, 5.9 | no change |

### Artifact conflicts

| Artifact | Touch | Why |
|---|---|---|
| `docs/bmad/planning-artifacts/epics.md` §Story 5.3 | rewrite ACs in place | scope expands per §4.1 below |
| `docs/bmad/implementation-artifacts/sprint-status.yaml` | add `last_updated` line; no entry shape changes | record the amendment |
| `docs/decisions/0004-daemon-observed-session-liveness.md` | already written | this proposal operationalizes it |
| `docs/bmad/planning-artifacts/architecture.md` §State-machine narrative (≈L1026 FR table row) | extend | add bullet covering daemon-side probe + `Ended` state + notification_type-driven WaitingInput; supersedes the bullet planned by the original 5.3 |
| `docs/protocol.md` §`SessionCurrentState` (≈L282) | extend | add `Ended` variant, note it is non-terminal (revivable by next hook event); document `notification_type`-driven rules |
| `docs/protocol.md` §`EventKind` table (≈L348) | extend | add `SessionEnded` row (daemon-emitted, not a Claude hook) |
| `docs/protocol.md` §`StateFrame` (≈L265) and §`/sessions` response (≈L81) | extend | add `last_pid: number | null` field (unchanged from original 5.3); update narrative to note presenter-side `kill()` is NOT required |
| `docs/protocol.md` §`EventFrame` (locate during impl) | extend | add `pid: number | null` field (unchanged from original 5.3); document `SessionEnded` payload shape |
| `docs/protocol.md` §Ingest socket contract | extend | document shim's `bowerbird_ppid` injection (unchanged from original 5.3) |
| `docs/protocol-changelog.md` | add five entries (revising three originally planned) | see §4.5 below |
| `docs/presenter-authoring.md` | add | new section on `Ended` rendering, "no `kill()` in presenters" guidance, recommended treatment (hide vs dim) |
| `docs/bmad/implementation-artifacts/deferred-work.md` | revise three entries | retain the parent-walk and started_at follow-ups; remove the "cookbook entry for detecting dead sessions" follow-up (no longer needed — presenters don't write the dead-detection code) |
| `prd.md` | no change | Marcus narrative unchanged; this is a substrate correctness improvement |

### Technical impact

Net of the original 5.3 + this amendment:

**Retained from original 5.3** (no change to scope):

- `crates/protocol/src/state.rs` — `SessionState` gains `last_pid: Option<u32>`.
- `crates/protocol/src/event.rs` — `EventEnvelope` gains `pid: Option<u32>` (internal); `Event` gains `pid: Option<u32>` (stored/wire).
- `crates/shim/src/main.rs` — `libc::getppid()` call + `bowerbird_ppid` injection into payload before send.
- `crates/shim/Cargo.toml` — adds `libc = { workspace = true }`.
- `crates/adapter-claude/src/normalize.rs` — extracts `bowerbird_ppid` → `EventEnvelope.pid`.
- `crates/daemon/src/db/migrations.rs` — appends migration v2 (`ALTER TABLE events ADD COLUMN pid INTEGER`).
- `crates/daemon/src/db/queries.rs` — `INSERT_EVENT` and projection-rebuild queries gain `pid` bind position.
- `crates/daemon/src/projection/session.rs::write()` and `::rebuild_missing_projections()` — thread `pid` through.
- Story 1.6 AC #5 ("storage layer is a pure function of the event sequence") preserved via per-event PID storage.

**Added by this amendment:**

- `crates/protocol/src/state.rs` — `SessionCurrentState` gains `Ended` variant (alongside existing `Idle | Working | WaitingInput | Unknown`). Wire string `"Ended"`. Outbound permissive policy: v1.0 presenters using `#[serde(other)]` catch-all decode it as `Unknown`.
- `crates/protocol/src/event.rs` — `EventKind` gains `SessionEnded` variant (alongside existing hook kinds + `RecordingStarted` + `RecordingEnded`). Daemon-emitted, broadcast on `events.*` topic. Decode-only `Unknown` catch-all applies.
- `crates/protocol/src/event.rs` — new typed enum `NotificationType { PermissionPrompt, IdlePrompt, AuthSuccess, ElicitationDialog, ElicitationResponse, ElicitationComplete, Unknown }` (or per Rust convention; PascalCase wire strings matching Claude's snake_case via `#[serde(rename = "permission_prompt")]` etc.). `EventEnvelope` gains `notification_type: Option<NotificationType>`.
- `crates/adapter-claude/src/normalize.rs` — when `hook_kind == "Notification"`, extract `notification_type` from payload via `value.get("notification_type").and_then(|v| v.as_str())`; map known strings to enum variants, unknown → `NotificationType::Unknown`, missing → `None`. Thread onto `EventEnvelope.notification_type`.
- `crates/daemon/src/projection/state.rs::transition()` — signature gains `notification_type: Option<NotificationType>` parameter; rules per §4.3 below; existing `EventKind` arm for `Notification` replaced with the typed-field rules; `PostToolUse` arm changed from "preserve prior" to "→ Working" (refines Story 5.2).
- `crates/daemon/src/projection/state.rs` — `transition` learns new event kind `EventKind::SessionEnded → Ended`.
- `crates/daemon/src/projection/liveness.rs` (new module) — periodic probe task. Uses `tokio::time::interval` with 5-second cadence and `MissedTickBehavior::Skip`. Per-iteration: query `session_projections` for rows where `current_state != 'Ended'`; for each, if `last_pid IS NULL OR libc::kill(last_pid as i32, 0) != 0` (errno = ESRCH), write a `SessionEnded` event via the normal projection write path. Payload: `{"reason": <"no_pid_at_upgrade"|"pid_dead">, "pid": <last_pid or null>, "observed_at_ms": <epoch_ms>}`.
- `crates/daemon/src/main.rs` — new startup sequence: (1) `run_migrations`; (2) `rebuild_missing_projections`; (3) `probe_once` (synchronous, before WS); (4) spawn periodic probe task; (5) spawn ingest listener; (6) spawn WS server. Step 3 and 4 share the same `probe_iteration` function — single source of truth.
- `crates/daemon/src/api/ws.rs` — `SessionEnded` events are broadcast on `events.*` like any hook event. Sentinel filter (Story 1.6 — excludes `__daemon__/__daemon__`) is unchanged; `SessionEnded` carries the real `(source, session_id)`, so it's not affected by the sentinel filter.
- `crates/protocol/tests/contract_protocol.rs` — round-trip `SessionCurrentState::Ended`, `EventKind::SessionEnded`, `NotificationType` variants; additive-compat tests that v1.0 presenters decode unknown variants as `Unknown`.
- `crates/daemon/tests/contract_daemon.rs` — projection transitions for the new notification_type rules; PostToolUse → Working unconditionally; liveness probe emits `SessionEnded` for `last_pid IS NULL` and `last_pid IS NOT NULL AND kill() != 0`; resume case (Ended + UserPromptSubmit → Working with new last_pid); probe overlap protection (slow tick doesn't queue).
- `crates/adapter-claude/tests/contract_adapter.rs` — normalize extracts all six `notification_type` values into the typed enum; unknown values → `NotificationType::Unknown`; missing field → `None`.
- No CI workflow changes (the new `liveness.rs` module is covered by existing `cargo test` matrix and existing benches; the probe task does not change the shim-bench-gate or the daemon-bench-gate).

### Out-of-scope (deliberately deferred)

Retained from original 5.3:

- **Parent-walk past `sh -c` wrappers** — defer to v0.1.X based on dogfooding signal.
- **PID-recycle resilience via `started_at` capture** — defer to v0.1.X based on dogfooding signal.

Added by this amendment:

- **`STALE_WORKING_MS` retirement.** ADR 0004 notes the 5-minute Working-decay rule predates daemon-observed liveness and is likely redundant once the probe lands. Retiring it cleanly is independent of this story (it would require asserting no remaining contract tests rely on the fallback). Deferred-work entry tracks this; not blocking this story.
- **`STALE_WAITING_INPUT_MS` substrate fallback.** Considered during the design; explicitly NOT added. Liveness handles dead-session cleanup; a time-based decay rule would re-introduce the kind of substrate-side interpretation the new design eliminates. ADR 0004 §"5. No `STALE_WAITING_INPUT_MS` substrate fallback."
- **Typed `notification_type` exposed on the projection / wire StateFrame.** The substrate maps `notification_type` to `current_state` via the transition rules and discards the raw value (it remains in `events.payload` for archaeology). Presenters that want richer rendering subscribe to the events stream too. Decision is in line with the maintainer's "I don't like having latest_notification on the projection — I want a slight interpolation of it" preference (design conversation 2026-05-28).
- **Per-session auto-prune of `Ended` rows in `session_projections`.** Rows stay in the table after transitioning to `Ended`. The deck filters them; future presenters can choose. A separate pruning story can land later if DB size becomes a real problem.
- **Cross-host liveness model.** Mirroring the original 5.3's note: V1 deployment is single-host. The new design DOES work better than the original for hypothetical cross-host setups (the daemon broadcasts the SessionEnded event, presenters don't have to call kill() locally), but doesn't formally close the cross-host bind decision.

## 3. Recommended approach

**Selected: Option 1 — Direct Adjustment (amend Story 5.3 in place).**

| Option considered | Verdict |
|---|---|
| 1. Direct Adjustment: amend Story 5.3 ACs in place, no renumbering | ✅ **Selected.** Low risk (5.3 is `backlog`; no implementation to revise). Single PR can land the full design. AC count grows from 13 to ~19, complexity stays Moderate. |
| 2. Split into two stories (amended 5.3 for liveness; new 5.3.5 for notification_type + PostToolUse) | ❌ Not preferred. The two pieces are tightly coupled: the notification_type fix is operationally meaningless without `Ended` (you still get WaitingInput accumulation), and the deck only becomes useful when both land. Single story is easier to review as a coherent design. |
| 3. Supersede original 5.3 with a new story (renumber later stories) | ❌ Not viable. 5.3 hasn't started; in-place amendment is the lower-cost path. Supersession adds bureaucracy without benefit. |
| 4. Rollback Story 5.2's PostToolUse "preserve prior" rule | ❌ Not viable. The 5.2 rule was correct in spirit (the agent is alive between tool calls); this amendment *refines* it (→ Working unconditionally) rather than rolls it back. Captured as a behavioral changelog entry, not a story rollback. |

**Rationale:** Story 5.3 is the right vehicle because (a) it's already scoped around session-liveness, (b) it hasn't started, and (c) the notification_type changes share the same ADR (0004) motivation and the same dogfooding-trigger pathway. Bundling avoids the test-environment problem where Story 5.3's daemon-side probe would land first and emit `SessionEnded` events that the older blind-WaitingInput projection rules would misinterpret in dev/CI fixtures.

Effort: **Medium** (~2 days implementation, ~0.5 day docs + contract tests; +~0.5 day over the original 5.3 estimate). Risk: **Low** (additive wire, AC #5 preserved, no presenter semantics in daemon, no new shim deps beyond a `libc` line already in the workspace, no new ports/sockets). Timeline impact: **+0 stories to Epic 5** (in-place amendment).

## 4. Detailed change proposals

### 4.1 Amend `epics.md` §Story 5.3 ACs

Replace the existing 13 ACs (lines 984–1040) with the revised set below. Story title and stakeholder framing stay; the narrative paragraph is updated to reflect the broader scope.

**Updated story title and narrative (lines 976–982):**

```
### Story 5.3: Daemon-observed session liveness + typed-notification WaitingInput

As a presenter author,
I want the substrate to observe process death and emit a mechanical SessionEnded event,
And I want WaitingInput to reflect Claude's typed notification_type field rather than collapse every Notification into one bucket,
So that my ribbon UI can render an accurate per-session state without doing its own liveness syscalls or its own payload regex.

Closes two dogfooding findings from Story 5.1: (1) accumulating WaitingInput ghosts from blind Notification → WaitingInput mapping; (2) no mechanical signal for "session process is gone." Operationalizes ADR 0004 (Daemon-observed session liveness). Refines Story 5.2's PostToolUse "preserve prior" rule to "→ Working" so a session in WaitingInput correctly transitions to Working when a tool completes. Resequenced from 5.8 → 5.3 by `sprint-change-proposal-2026-05-27-epic-5-resequencing.md` (dogfooding-first ordering); design amended in this proposal (`sprint-change-proposal-2026-05-28-daemon-observed-liveness.md`).
```

**Revised AC list (replaces lines 984–1040):**

```
**Acceptance Criteria:**

**Given** a Claude Code hook fires and the shim runs
**When** the shim sends the payload to the daemon's ingest socket
**Then** the payload JSON includes a `bowerbird_ppid` field whose value is the integer returned by `libc::getppid()` at shim-invocation time; the field is injected by the shim, not present in the upstream Claude Code hook payload; the shim hot-path p99 ≤5ms budget (Story 1.5) is preserved under the shim-bench-gate

**Given** the `adapter-claude` normalize path receives a payload with `bowerbird_ppid` set
**When** normalize constructs the `EventEnvelope`
**Then** `EventEnvelope.pid` is `Some(<that value>)`; a payload missing `bowerbird_ppid` or carrying a non-integer value yields `EventEnvelope.pid = None` and is normalized successfully (not a failure mode)

**Given** the `adapter-claude` normalize path receives a payload with `hook_kind = Notification` and a `notification_type` field
**When** normalize constructs the `EventEnvelope`
**Then** `EventEnvelope.notification_type` is `Some(NotificationType::X)` for known values (`permission_prompt`, `idle_prompt`, `auth_success`, `elicitation_dialog`, `elicitation_response`, `elicitation_complete`); an unrecognized value yields `Some(NotificationType::Unknown)`; a missing field yields `None`; the event is normalized successfully in all three cases

**Given** an `EventEnvelope` with `pid: Some(N)` reaches `projection::session::write`
**When** the projection writes inside its single transaction
**Then** the `events` row stores `pid = N`; the upserted `session_projections` row's deserialized `SessionState` carries `last_pid: Some(N)`; the `BroadcastEnvelope::State` published after commit carries the same `last_pid`; the `BroadcastEnvelope::Event` likewise carries `pid: Some(N)`

**Given** a follow-up `EventEnvelope` for the same `(source, session_id)` with `pid: None`
**When** the projection writes
**Then** `SessionState.last_pid` retains the prior `Some(N)` (carry-forward semantics); the `events` row stores `pid = NULL` for that specific event

**Given** a follow-up `EventEnvelope` for the same `(source, session_id)` with `pid: Some(M)` where `M != N`
**When** the projection writes
**Then** `SessionState.last_pid` becomes `Some(M)` (overwrite-on-Some semantics)

**Given** an `EventEnvelope` for `hook_kind = Notification` with `notification_type` in `{PermissionPrompt, IdlePrompt, ElicitationDialog}`
**When** the projection's `transition` function runs
**Then** the resulting `current_state` is `WaitingInput`; the prior state is irrelevant

**Given** an `EventEnvelope` for `hook_kind = Notification` with `notification_type` in `{AuthSuccess, ElicitationResponse, ElicitationComplete}` OR `notification_type = Unknown` OR `notification_type = None`
**When** the projection's `transition` function runs
**Then** the resulting `current_state` preserves the prior state (no transition)

**Given** an `EventEnvelope` for `hook_kind = PostToolUse`
**When** the projection's `transition` function runs
**Then** the resulting `current_state` is `Working` unconditionally (refines Story 5.2's "preserve prior" rule — flagged as a behavioral changelog entry); `last_event_kind` and `last_event_at_ms` update normally

**Given** the daemon completes `run_migrations` and `rebuild_missing_projections` at startup
**When** the daemon proceeds to accept connections
**Then** one synchronous iteration of the liveness probe has run before the WS server binds — for each `session_projections` row where `last_pid IS NULL` OR `libc::kill(last_pid as i32, 0) != 0` (errno = ESRCH), a `SessionEnded` event is written via the normal `projection::session::write` path; the projection row transitions to `current_state = Ended`; the events row carries `source = <row's source>`, `session_id = <row's session_id>`, `kind = SessionEnded`, `payload = {"reason": "no_pid_at_upgrade"|"pid_dead", "pid": <last_pid or null>, "observed_at_ms": <epoch_ms>}`

**Given** the daemon is running steady-state with the WS server up
**When** the periodic liveness probe task wakes (5-second cadence via `tokio::time::interval` with `MissedTickBehavior::Skip`)
**Then** the same per-row logic from the startup iteration runs; `SessionEnded` events are written and broadcast on `events.*`; resulting state transitions are broadcast on `state.session.*`; an in-flight probe iteration that takes longer than the tick interval does NOT queue (next tick skipped)

**Given** a `session_projections` row in `current_state = Ended`
**When** a subsequent hook `EventEnvelope` arrives for the same `(source, session_id)` (e.g. from `claude --resume`)
**Then** `transition` runs normally: `UserPromptSubmit`/`PreToolUse`/`PostToolUse → Working`; `Stop → Idle`; `Notification` with input-required type → `WaitingInput`; `last_pid` updates from the new envelope's PID via overwrite-on-Some semantics; the row exits `Ended`

**Given** a daemon restart with a non-empty `events` table that includes `SessionEnded` events
**When** `rebuild_missing_projections` runs
**Then** for each rebuilt session the reconstructed `SessionState.last_pid` AND `current_state` match what live ingest would have produced from the same event sequence (Story 1.6 AC #5 "storage layer is a pure function of the event sequence" is preserved); `SessionEnded` events in the log drive transitions to `Ended` during rebuild exactly as they did during live ingest

**Given** `GET /sessions` and `GET /sessions/{id}`
**When** the daemon serializes the response
**Then** `SessionListItem` and `SessionDetail.state` each carry `last_pid` as a number-or-null field; `SessionCurrentState` includes the new `Ended` variant in `current_state` for rows where the liveness probe observed death; the read-time stale-`Working` → `Idle` fallback (Story 1.6 `current_state_for_read`) does NOT alter `last_pid` and does NOT interfere with `Ended` (which passes through unchanged); the sentinel session row (`source = "__daemon__"`) continues to be filtered out

**Given** a WS subscriber to `state.session.*` receives a `StateFrame`
**When** the frame is decoded
**Then** `frame.state.last_pid` carries the same value as the REST `SessionDetail.state.last_pid` would for the same session at the same moment; snapshot-on-subscribe frames (Story 2.3) likewise carry `last_pid`; transitions to `Ended` (driven by the liveness probe) broadcast a `StateFrame` per the Story 5.2 transitions-only policy

**Given** a WS subscriber to `events.*` receives an `EventFrame`
**When** the frame is decoded for a `SessionEnded` event
**Then** the frame carries `kind = "SessionEnded"`, the real `source` and `session_id` of the session that ended, and a payload object with `reason`, `pid` (number or null), and `observed_at_ms`

**Given** a v1.0 presenter compiled against the pre-Story-5.3 protocol type
**When** it deserializes a `SessionState` frame, a `StateFrame`, or an `EventFrame` from a Story-5.3+ daemon
**Then** serde silently ignores the `last_pid` field; the `Ended` `SessionCurrentState` variant decodes to `Unknown` via the Story 4.4 `#[serde(other)]` catch-all; the `SessionEnded` `EventKind` decodes to `Unknown` via the same catch-all; no decode error, no crash, no protocol-violation close frame; additive-compat contract tests in `contract_protocol.rs` exercise each path

**Given** the SQLite `events` schema before Story 5.3 (v1)
**When** the daemon starts against an existing v1 database
**Then** migration v2 runs `ALTER TABLE events ADD COLUMN pid INTEGER`; existing rows have `pid = NULL`; the migration is idempotent (re-running `to_latest` is a no-op per Story 5.4's migration-idempotency contract test)

**Given** the protocol surface
**When** Story 5.3 lands
**Then** `crates/protocol/src/state.rs` `SessionState` gains `last_pid: Option<u32>` AND `SessionCurrentState` gains the `Ended` variant; `crates/protocol/src/event.rs` `EventEnvelope` gains `pid: Option<u32>` (internal) AND `notification_type: Option<NotificationType>` (internal), `EventKind` gains the `SessionEnded` variant, a new `NotificationType` enum is added with six variants + `Unknown`, and stored `Event` gains `pid: Option<u32>`; `crates/shim/Cargo.toml` adds the workspace `libc` dep; `crates/shim/src/main.rs` injects `bowerbird_ppid`; `crates/adapter-claude/src/normalize.rs` extracts both `bowerbird_ppid` and `notification_type`; a new module `crates/daemon/src/projection/liveness.rs` houses the probe loop

**Given** the doc and contract-test surface
**When** Story 5.3 lands
**Then** `docs/protocol.md` adds `last_pid` to `/sessions`, `/sessions/{id}`, and `StateFrame`, adds `pid` to `EventFrame`, adds the `Ended` variant to the `SessionCurrentState` enum docs, adds the `SessionEnded` row to the `EventKind` table, documents the typed `notification_type` rules in the `Notification` event description, and documents the shim's `bowerbird_ppid` injection in §Ingest socket contract; `docs/protocol-changelog.md` gains five entries (see §4.5 of this proposal); `docs/presenter-authoring.md` gains a section on `Ended` rendering and "no `kill()` in presenters" guidance; `crates/protocol/tests/contract_protocol.rs` exercises round-trip + additive-compat for all new variants; `crates/daemon/tests/contract_daemon.rs` exercises projection threading, the typed-notification rules, the PostToolUse refinement, the liveness probe (both startup and periodic), and rebuild AC #5; `crates/adapter-claude/tests/contract_adapter.rs` exercises `notification_type` extraction for all six known values + unknown + missing

**Given** the planning artifacts
**When** Story 5.3 lands
**Then** `architecture.md` §State-machine narrative gets the updated bullet covering daemon-side probe + `Ended` + notification_type-driven WaitingInput; `architecture.md` §Singletons & discovery retains the forward reference noting the same PID-as-liveness pattern applies one layer down for sessions

**Given** documented v0.1.0 caveats
**When** Story 5.3 lands
**Then** `deferred-work.md` retains the parent-walk and `started_at` follow-up entries (unchanged from original 5.3 scope); removes the original 5.3's "cookbook entry for detecting dead sessions" entry (no longer needed — the daemon emits the event; presenters consume it); adds a new entry tracking `STALE_WORKING_MS` retirement as a separate follow-up
```

### 4.2 `sprint-status.yaml` update

Add a `last_updated` line at the top documenting the amendment; no entry-shape changes (Story 5.3 remains `backlog`).

```
# last_updated: 2026-05-28 (Story 5.3 ACs amended for daemon-observed liveness + typed-notification WaitingInput per sprint-change-proposal-2026-05-28-daemon-observed-liveness.md and ADR 0004; story remains backlog)
```

### 4.3 Transition table (canonical reference for the AC text)

The substrate's new state machine:

```
Hook events:
  UserPromptSubmit                              → Working
  PreToolUse                                    → Working
  PostToolUse                                   → Working    (refines Story 5.2's "preserve prior")
  Stop                                          → Idle

Notification (with notification_type extracted by adapter-claude):
  permission_prompt | idle_prompt | elicitation_dialog
                                                → WaitingInput
  auth_success | elicitation_response | elicitation_complete
                                                → preserve prior
  Unknown | None                                → preserve prior

Daemon-observed:
  SessionEnded                                  → Ended

Sentinels (unchanged from Story 1.6):
  RecordingStarted | RecordingEnded             → do not affect per-session current_state
```

### 4.4 `docs/protocol.md` additive edits

| Section | Edit |
|---|---|
| `SessionCurrentState` enum (≈L282) | Add `Ended` variant. Update narrative: "`Ended` is daemon-observed (not hook-driven). It indicates the session's `last_pid` is no longer a live OS process. It is **not terminal** — a session can transition out of `Ended` on the next hook event (typically a `UserPromptSubmit` from `claude --resume`)." Document that the typed `notification_type` field drives WaitingInput (see EventKind §Notification). |
| `EventKind` table (≈L348) | Add row: `\| SessionEnded \| no \| Daemon-observed: liveness probe detected that the session's `last_pid` is no longer alive. Daemon-emitted, not from a Claude hook. Per-session (`source = <real>`, `session_id = <real>`). \|`. Update `Notification` row narrative to point at the new `notification_type` documentation. |
| `/sessions` response example (≈L81) | Add `"last_pid": 12345` (or `null`) field to the example object. |
| `/sessions/{id}` response example (≈L102) | Add `"last_pid"` field to the `state` sub-object. |
| `StateFrame` example (≈L265) | Add `"last_pid"` field to the `state` sub-object; update narrative to note it's a mechanical fact, but unlike the original Story 5.3 design, presenters do NOT need to call `kill(pid, 0)` — they receive a `SessionEnded` event when the daemon observes death. |
| `EventFrame` (locate during impl) | Add `"pid": 12345` (or `null`) field to the example. Document `SessionEnded` payload shape: `{"reason": "no_pid_at_upgrade" \| "pid_dead", "pid": <number or null>, "observed_at_ms": <epoch_ms>}`. |
| §Ingest socket contract | One new bullet: "The shim MAY inject `bowerbird_ppid: <integer>` into the JSON payload before send. The adapter-claude normalize path extracts it into `EventEnvelope.pid`. A missing or non-integer value is normalized as `pid: None` (not a failure)." Add a second bullet covering `notification_type`: "When `hook_kind = Notification`, the adapter-claude normalize path also extracts the upstream `notification_type` field into `EventEnvelope.notification_type` as a typed `NotificationType` enum (six known variants plus `Unknown` for forward-compatibility plus `None` for missing). The projection uses this typed value to drive the WaitingInput state transition; see `SessionCurrentState` §Ended for the rules." |

### 4.5 `docs/protocol-changelog.md` entries

Five entries under v1.0 → v1.1, placed after the Story 5.2 entries:

```
- **type: schema** — `SessionState.last_pid: Option<u32>` field added (Story 5.3). Carries the PID returned by `libc::getppid()` at shim-invocation time for the most recent hook that produced an event for this `(source, session_id)`. The field is a mechanical fact — the substrate makes no claim about whether the PID is alive at any moment after observation. Carry-forward semantics: an envelope with `pid: None` preserves the prior `last_pid`; an envelope with `pid: Some(N)` overwrites; a `claude --resume` of an existing `session_id` under a new PID overwrites on its first hook. The daemon's liveness probe uses `last_pid` as input to `kill(pid, 0)` — see the `EventKind::SessionEnded` entry below. v1.0 presenters decode the field as missing/ignored per the asymmetric outbound-permissive serde policy. Sentinel session rows (`source = "__daemon__"`) continue to be excluded from session-listing queries. (`Resolves: 5.3`)

- **type: schema** — `Event.pid: Option<u32>` field added and SQLite events table gains a nullable `pid INTEGER` column via migration v2 (Story 5.3). Per-event PID is stored so `rebuild_missing_projections` can reproduce `SessionState.last_pid` exactly — preserving Story 1.6 AC #5. Existing v1 rows have `pid = NULL`. The migration is idempotent per Story 5.4. `EventFrame` over WS and `GET /sessions/{id}/events` over REST both surface `pid` as a number-or-null field. v1.0 presenters decode the field as missing/ignored. (`Resolves: 5.3`)

- **type: schema** — `SessionCurrentState::Ended` variant added; `EventKind::SessionEnded` variant added; `NotificationType` enum added (Story 5.3). The substrate now emits per-session `SessionEnded` events when its liveness probe observes that `last_pid` is no longer reachable (`libc::kill(pid, 0) != 0`) OR a row has `last_pid IS NULL` at probe time (legacy/upgrade case). `SessionEnded` events drive a projection transition to `current_state = Ended`. `Ended` is **non-terminal** — the next hook event for that session_id (UserPromptSubmit / PreToolUse / PostToolUse → Working, Stop → Idle, Notification with input-required `notification_type` → WaitingInput) transitions out of Ended normally; `last_pid` overwrites on Some. `NotificationType` enumerates the six Claude Code values (`permission_prompt`, `idle_prompt`, `auth_success`, `elicitation_dialog`, `elicitation_response`, `elicitation_complete`) plus `Unknown` for forward-compat plus `None` for missing; the adapter extracts it from the payload and the projection uses it to decide WaitingInput vs preserve-prior (see the behavioral entry below). v1.0 presenters decode `Ended` and `SessionEnded` as `Unknown` via the Story 4.4 catch-all. (`Resolves: 5.3`)

- **type: behavioral** — `Notification → WaitingInput` mapping refined to be `notification_type`-aware (Story 5.3, supersedes Story 1.6's blind collapse). `permission_prompt`, `idle_prompt`, and `elicitation_dialog` map to WaitingInput. `auth_success`, `elicitation_response`, `elicitation_complete`, unknown future types, and missing-field cases preserve the prior `current_state`. This sharpens — does not break — the v1.0 wire shape: v1.0 presenters that subscribed to `state.session.*` will see fewer false-positive WaitingInput state transitions (idle_prompt re-fires no longer flip a Working session to WaitingInput; the rule is now "only set WaitingInput when Claude tells us via a typed input-required notification_type"). (`Resolves: 5.3`)

- **type: behavioral** — `PostToolUse → Working` (refines Story 5.2's `PostToolUse → preserve prior` rule). Story 5.2 introduced "preserve prior" to capture the semantic "the agent is alive between tool calls" — but a session in `WaitingInput` whose tool call completes (because the user resolved an elicitation_dialog mid-tool) would stay `WaitingInput`. The corrected rule says what 5.2 actually meant: tool activity = agent is in active state. v1.0 presenters that subscribed to `state.session.*` will see Working transitions from former-WaitingInput-and-PostToolUse paths that previously stayed cyan. (`Resolves: 5.3, refines: 5.2`)

- **type: behavioral** — Ingest-socket contract: shim injects `bowerbird_ppid` and adapter extracts `notification_type` (Story 5.3). The shim, after parsing the inbound hook JSON, calls `libc::getppid()` and inserts a `bowerbird_ppid: <integer>` field into the payload object alongside the existing `hook_kind` injection. The adapter-claude normalize path extracts `bowerbird_ppid` into `EventEnvelope.pid` AND extracts the upstream `notification_type` field (when present, i.e. on Notification events) into `EventEnvelope.notification_type` as a typed `NotificationType` enum. Missing or malformed values degrade gracefully (None). Hot-path cost is one `getppid()` syscall, one `serde_json::Map::insert`, and one additional `value.get().and_then()` chain; the Story 1.5 p99 ≤5ms shim budget is preserved under the Story 5.5 shim-bench-gate. NDJ framing on `~/.bowerbird/ingest.sock` (ADR-0002) is unchanged. (`Resolves: 5.3`)
```

### 4.6 `architecture.md` amendments

**At §State-machine narrative (≈L1026 FR table):**

> Replace the original-5.3 bullet plan with: "Session liveness and observed death: `SessionState.last_pid` carries the PID of the parent process that last fired a hook for this session_id; `transition()` carries it forward (overwrite-on-Some, keep-on-None) but it does NOT influence `current_state`. A background liveness probe task in the daemon runs at 5s cadence; when it observes `last_pid` is unreachable (`libc::kill(pid, 0) != 0`) OR a row has `last_pid IS NULL` at probe time, it emits a `SessionEnded` event with `source = <row's source>` and `session_id = <row's session_id>`. The projection's `transition()` maps `EventKind::SessionEnded → SessionCurrentState::Ended`. `Ended` is non-terminal: the next hook event for that session_id transitions out normally (resume case). Presenters do NOT call `kill(pid, 0)` themselves — they consume `SessionEnded` like any hook event. WaitingInput is `notification_type`-aware: `permission_prompt`, `idle_prompt`, and `elicitation_dialog` map to WaitingInput; the three transient types (`auth_success`, `elicitation_response`, `elicitation_complete`) and unknown future types preserve prior state. PostToolUse → Working unconditionally (refining Story 5.2). See ADR 0004 (`docs/decisions/0004-daemon-observed-session-liveness.md`) for the rationale."

**At §Singletons & discovery (≈L987 after the existing `bowerbird.pid` bullet):**

> Forward reference unchanged from original 5.3 plan: "The same PID-as-liveness-probe pattern extends one layer down to per-session granularity via `SessionState.last_pid` (Story 5.3). The session-level probe runs inside the daemon (see §State-machine narrative), not in presenters. See ADR 0004 for the rationale."

### 4.7 `docs/presenter-authoring.md` additions

Add a new subsection under the state-subscription guidance:

```
### Rendering `Ended` sessions

`SessionCurrentState::Ended` is daemon-observed: the substrate's liveness probe noticed the session's `last_pid` is no longer reachable. Some recommendations:

- **Default render: hide.** A session in `Ended` cannot be switched to; the row is noise in a "what should I do next?" UI.
- **Alternative: dim/strike-through.** Useful if you want a brief "X just ended" awareness window. Combine with a hide-after-N-seconds rule to avoid clutter.
- **Do NOT call `kill(pid, 0)` in your presenter.** The daemon already does this and emits `SessionEnded` events. Presenter-side kill() loops are duplicate work (and break for cross-host consumers — see ADR 0004).

`Ended` is **not terminal.** The next hook event for that session_id (e.g. from `claude --resume`) transitions the session out of Ended via the normal state machine. Your presenter should treat the next `state.session.<id>` envelope after an `Ended` as a resumption, not a brand-new session — though the simplest implementation is to treat it as a new row, which is fine for v1 presenters.

### Rendering `WaitingInput` sessions

`WaitingInput` is set when Claude emits a `Notification` event with `notification_type` in `{permission_prompt, idle_prompt, elicitation_dialog}`. The typed value is preserved verbatim in `events.payload` (subscribe to `events.<source>.<session_id>` to read it) but is NOT exposed on the projection / `state.session.*` topic. Presenters that want richer rendering (e.g. "this row needs your permission" vs "this row is just idle") subscribe to events too. Presenters that just want a single "switch here" affordance can render off `current_state` alone.
```

### 4.8 `deferred-work.md` follow-up entries

Revise the three entries originally planned by Story 5.3:

```
- 5.3 follow-up: parent-walk past sh -c wrappers via macOS proc_pidinfo(PROC_PIDT_BSDINFO) / Linux /proc/<pid>/stat. v0.1.0 ships raw getppid(); if dogfooding reveals high false-positive "dead" rates because Claude Code wraps the shim in `sh -c`, add a one-level parent walk in the shim. Gated on a real dogfooding signal, not pre-emptively shipped.

- 5.3 follow-up: PID-recycle resilience via started_at capture. Pair last_pid with the parent process's start time (pbi_start_tvsec on Darwin, starttime in /proc/<pid>/stat on Linux) so a recycled PID can be detected as a different process. Daemon-side liveness probe is the primary defense; started_at is the secondary defense for long-running daemons on PID-busy machines. Gated on a real dogfooding false-positive on workstation timescales.

- 5.3 follow-up: STALE_WORKING_MS retirement. The 5-minute Working-decay rule (Story 1.6) predates daemon-observed liveness. Once Story 5.3 lands, the rule is likely redundant for its original purpose (catching dropped Stop hooks — those sessions get SessionEnded'd eventually anyway). Retire after asserting no contract tests rely on the fallback and confirming via dogfooding that no real Working sessions sit longer than 5 min on healthy workloads. Tracked separately so the retirement decision is not bundled with the substrate-side liveness work.
```

Remove the original 5.3's "cookbook entry: detecting dead sessions" follow-up. No longer applicable — the substrate emits the SessionEnded event; presenters don't need a cookbook for `kill()`.

## 5. PRD MVP impact

MVP scope unchanged. The Marcus narrative (`prd.md:206`) is about active-session ribbon UX; this amendment improves the accuracy of that ribbon (fewer false-positive WaitingInput rows, correct dead-session filtering) and removes presenter-side complexity, but does not alter the V1 promise.

## 6. Implementation handoff

**Scope classification: Moderate.**

Multi-crate touch (protocol + shim + adapter-claude + daemon), one SQLite migration (v2), one new daemon module (`liveness.rs`), three contract test files updated, four doc files updated, plus a behavioral refinement to a recently-shipped story (5.2's PostToolUse rule). Complexity is comparable to the original 5.3 scope (Moderate); the addition of the typed-notification rules, the PostToolUse refinement, and the liveness probe loop is incremental over the original.

**Handoff recipients:**

| Role | Responsibility |
|---|---|
| Product Owner (pickles) | Approve this proposal (Section 5 of bmad-correct-course workflow); rewrite `epics.md` §Story 5.3 ACs per §4.1; update `sprint-status.yaml` per §4.2; update `architecture.md`, `docs/protocol.md`, `docs/presenter-authoring.md`, `docs/protocol-changelog.md`, `docs/bmad/implementation-artifacts/deferred-work.md` per §4.4–4.8 in the same PR as the implementation |
| Developer agent | Implement the multi-crate change (shim PPID + notification_type ingest, adapter extract, protocol type additions, daemon projection threading + new transition rules, liveness probe module, v2 migration, contract tests); land in a single PR |
| Story automator (separately scheduled) | Create the Story 5.3 implementation-artifacts file (`docs/bmad/implementation-artifacts/5-3-daemon-observed-session-liveness.md`) once this proposal is approved, following the bmad-create-story flow; the file should reference both this proposal and ADR 0004 |

**Success criteria:**

1. `bowerbird-deck` against the maintainer's accumulated workstation history shows zero (or near-zero) stale `WaitingInput` rows after first boot post-upgrade; the ~48-row ghost problem dissolves automatically as the eager startup probe emits `SessionEnded` events for the no-PID legacy rows.
2. Live Claude Code sessions in active use show accurate state transitions: `idle_prompt` notifications no longer flip a Working session to WaitingInput; permission_prompts correctly highlight rows that need action; sessions ended by closing the terminal disappear from the deck within 5 seconds.
3. v1.0 presenter binary (built before Story 5.3) connects to a Story-5.3+ daemon without error: additive-compat round-trip contract tests green for all new variants (`Ended` SessionCurrentState, `SessionEnded` EventKind, all NotificationType values).
4. Shim p99 ≤5ms budget preserved under the shim-bench-gate; daemon p99 ≤100ms hook-to-presenter budget preserved under the daemon-bench-gate.
5. Story 1.6 AC #5 ("storage layer is a pure function of the event sequence") preserved: deleting `session_projections` and restarting the daemon reproduces byte-identical state, including all `Ended` transitions from the SessionEnded events in the log.
6. After at least 1 week of dogfooding with Story 5.3 in place, decide whether to promote any of the three deferred-work follow-ups (parent-walk, started_at, STALE_WORKING_MS retirement) into Story 5.3.X-hotfix work or leave them post-V1.

## 7. Trade-offs and alternatives summary

Three load-bearing trade-offs are surfaced inline; re-stated here:

1. **Daemon-side liveness probe adds a background tokio task** (Story 1.6 explicitly avoided this for STALE_WORKING_MS decay; the cost/benefit here is different — the probe drives presenter-visible events, not just an internal decay). ADR 0004 §"Why this is consistent with Axioms 1 and 4 (refined)" and §"Alternatives considered" cover the reasoning in detail.

2. **`notification_type` is consumed by the projection but not exposed on the wire StateFrame.** The substrate maps it to `current_state` and discards the typed value (it stays in `events.payload`). Presenters that want richer rendering must subscribe to events too. This matches the maintainer's stated preference ("no latest_notification on the projection; slight interpolation"). The alternative — an orthogonal `pending_input` field on the projection — is rejected in ADR 0004 §"Alternatives considered."

3. **PostToolUse refinement (→ Working unconditionally) is a behavioral change to a recently-shipped story.** Captured as a `type: behavioral` changelog entry, not a story rollback. The change is monotonically more correct than Story 5.2's "preserve prior" (it correctly clears WaitingInput; preserves Working when prior was Working — same end state for the original 5.2 motivating case; handles resume cleanly). No revert is needed.

The single load-bearing trade-off that's NOT covered elsewhere: bundling the typed-notification + PostToolUse + liveness changes into one Story 5.3 amendment vs splitting into two stories. Bundling chosen because the two pieces are tightly coupled (WaitingInput fix is operationally meaningless without Ended) and the deck only becomes useful when both land. Single-PR review benefits also significant. Splitting was considered and explicitly rejected in §3 above.
