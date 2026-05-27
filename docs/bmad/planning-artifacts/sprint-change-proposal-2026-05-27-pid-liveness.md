# Sprint Change Proposal — Session-process liveness via PID capture

> **Note (2026-05-27 resequencing):** This proposal references "Story 5.8" throughout. The Epic 5 resequencing of 2026-05-27 (see `sprint-change-proposal-2026-05-27-epic-5-resequencing.md`) renumbered the session-process-liveness story to **Story 5.3**. The proposal text below is preserved verbatim from approval-time; read "Story 5.8" as "Story 5.3" when referring to current Epic 5.

Date: 2026-05-27
Author: pickles (via bmad-correct-course)
Status: Approved 2026-05-27
Related: `sprint-change-proposal-2026-05-27.md` (sibling Epic 5 dogfooding finding), `epics.md` §Story 5.7 / §Story 5.8 (this proposal), `docs/protocol.md:81` (`/sessions` response), `docs/protocol.md:265` (`StateFrame`), `docs/bmad/planning-artifacts/architecture.md:987` (existing `bowerbird.pid` + `kill(pid, 0)` precedent for daemon liveness)

## 1. Issue summary

A second defect surfaced during Epic 5 dogfooding of bowerbird-deck, the Story 5.1 first-party presenter, against a live Claude Code workstation.

### Defect — No signal to distinguish "user walked away" from "Claude Code process is gone"

`crates/protocol/src/state.rs` `SessionState` exposes `current_state` (idle / working / waiting-input) and `last_event_at_ms` but no signal that ties a session row to a live OS process. The 5-minute `STALE_WORKING_MS` read-time fallback (`crates/daemon/src/projection/state.rs:19`) only un-sticks a missing `PostToolUse` / `Stop` (Story 1.6); it does NOT distinguish "agent is genuinely between tools" from "the Claude Code process that owned this session_id exited hours ago."

User-visible effect: bowerbird-deck against a workstation that has accumulated a day's worth of Claude Code starts/stops shows ~30 session rows, most in `WaitingInput` or `Idle`, ages ranging from 30s to 23h. The presenter ribbon cannot filter or sort by liveness because the substrate emits no PID-equivalent fact. The maintainer cannot tell at a glance which sessions are actually live to talk to.

### Why a PID is the right substrate fact

Axiom 1 (the substrate observes; it does not interpret) and Axiom 4 (mechanical facts in the protocol; semantics in the presenter) make the line clean here:

- **PID = mechanical fact.** Each Claude Code hook fires from a real OS process. `getppid()` at shim-invocation time captures the parent identifier; that's a value the daemon can observe.
- **"Is alive" = presenter semantic.** `kill(pid, 0)` returning success/failure is a derived interpretation — the presenter performs it locally and renders accordingly. Same line `architecture.md:987-989` already draws for the *daemon's* own liveness (the `bowerbird.pid` file + `kill(pid, 0)` probe pattern).

The new story extends the existing PID-as-liveness-probe pattern from "is the daemon alive" to "is the Claude Code process backing this session alive," without smuggling a `is_alive: true` flag into the wire (which would fail Axiom 4).

### Resume case

A Claude Code process can `claude --resume <session_id>` an existing session_id from a *new* PID. The mechanical fact stays correct: the new process's PPID overwrites `SessionState.last_pid` on the next hook event for that session_id. Presenters that cached an older PID either revalidate against the latest snapshot (REST or `state.session.*` envelope) or accept staleness — same semantics as `last_event_at_ms`.

### Why this surfaced now

Story 5.1's V1 presenter is the first sustained dogfooding surface for the daemon. The presenter listed all observable sessions, exposed the substrate's gap immediately, and the gap was visible within the first observed Claude Code workday. Without the presenter, the gap would have shipped to v0.1.0 unchanged (same pattern as the sibling `sprint-change-proposal-2026-05-27.md` finding).

### Evidence

- `crates/protocol/src/state.rs:26-31` — `SessionState` has `current_state`, `last_event_kind`, `last_event_at_ms`; no PID field.
- `crates/protocol/src/event.rs:36-42` — `EventEnvelope` has `source`, `session_id`, `kind`, `reaction`, `payload`; no PID field.
- `crates/daemon/src/db/migrations.rs:6-14` — `events` table has no `pid` column.
- `crates/shim/src/main.rs:38-64` — shim sends the payload JSON verbatim; no `getppid()` capture.
- `crates/adapter-claude/src/normalize.rs:76-91` — normalize extracts `session_id` and `tool_name`; no PID extraction path.
- `docs/bmad/planning-artifacts/architecture.md:987-989` — establishes precedent: `bowerbird.pid` + `kill(pid, 0)` is already the canonical "is process alive" pattern for daemon discovery. New story uses the same pattern, one layer down (Claude Code process per session).
- bowerbird-deck observation (Story 5.1 dogfooding, 2026-05-27): 30+ session rows visible, most stale-by-real-life-clock-time; presenter has no way to mark dead-process rows.

## 2. Impact analysis

### Epic impact

- **Epic 1, Epic 2, Epic 3, Epic 4 (closed, retro'd)**: No reopen. The state machine, projection write path, and protocol surface all live forward; new field is additive in protocol-changelog v1.0 → v1.1 line.
- **Epic 5 (in planning, 5.1 in-progress)**: One new story inserted before the v0.1.0 tag story (correctness/signal-quality fixes must precede tagging — same rule the sibling 5.7 insertion observed). Existing Story 5.8 (crates.io + v0.1.0 tag) renumbers to 5.9.

### Story impact

| Story | Action |
|---|---|
| 5.6 (First-time-reader docs pass) | no change |
| 5.7 (Session state projection correctness) | no change |
| **new 5.8 (Session-process liveness via PID capture)** | new story inserted |
| 5.8 → 5.9 (Crates.io namespace + v0.1.0 tag) | renumber only; AC text unchanged except the closing-condition reference "5.1 through 5.7" → "5.1 through 5.8" |

### Artifact conflicts

| Artifact | Touch | Why |
|---|---|---|
| `epics.md` (Story 5.8 insert, 5.8→5.9 renumber) | add + renumber | sequence the fix before v0.1.0 |
| `docs/bmad/implementation-artifacts/sprint-status.yaml` | insert + renumber | match epics.md ordering |
| `architecture.md` §State-machine narrative (≈L1026 FR table row) | extend | add bullet: `last_pid` is carried forward by `transition()` but does not influence `current_state`; presenter computes liveness via `kill(pid, 0)` |
| `architecture.md` §Singletons & discovery (≈L987) | cross-reference | add backlink noting that the *session*-level liveness pattern mirrors the *daemon*-level pattern documented at this line |
| `docs/protocol.md` §`/sessions` response (≈L81) | extend | add `last_pid: number | null` field to `SessionListItem` |
| `docs/protocol.md` §`/sessions/{id}` response (≈L102) | extend | add `last_pid: number | null` field to `SessionDetail.state` |
| `docs/protocol.md` §`StateFrame` (≈L265) | extend | add `last_pid: number | null` field to `StateFrame.state` |
| `docs/protocol.md` §`EventFrame` (locate during impl) | extend | add `pid: number | null` field to `EventFrame` |
| `docs/protocol.md` §Ingest socket contract | extend | document that the shim MAY inject `bowerbird_ppid: number` into the hook payload at send time; daemon adapter extracts and threads it onto `EventEnvelope.pid` |
| `docs/protocol-changelog.md` | add three entries | (a) schema: `SessionState.last_pid`; (b) schema: `EventFrame.pid` / stored `Event.pid` / SQLite `events.pid` column via v2 migration; (c) behavioral: shim PPID injection at ingest time as part of the hook-payload contract |
| `prd.md` | no change | Marcus narrative is about live-state ribbon UX; session liveness is an additive presenter capability, not in the V1 narrative |

### Technical impact

- `crates/protocol/src/state.rs` — `SessionState` gains `last_pid: Option<u32>`. Outbound permissive serde policy (per asymmetric `deny_unknown_fields`) keeps v1.0 presenters compatible; they decode the field as missing/ignored.
- `crates/protocol/src/event.rs` — `EventEnvelope` gains `pid: Option<u32>` (internal ingest-to-projection field, not wire). `Event` (the stored/REST-emitted shape) gains `pid: Option<u32>` so `rebuild_missing_projections` preserves the Story 1.6 AC #5 invariant ("storage layer is a pure function of the event sequence"); without `Event.pid`, a projection rebuild would lose `last_pid` and break that AC.
- `crates/shim/src/main.rs` — after parsing the stdin payload JSON, before sending: call `libc::getppid()` and inject `bowerbird_ppid: <pid>` into the JSON object alongside `hook_kind`. Hot-path cost: one syscall (~hundreds of ns) + one JSON-map insert. Well within the Story 1.5 p99 ≤5ms budget; Story 5.2's chaos-injection sanity work will catch any regression on the shim-bench-gate.
- `crates/shim/Cargo.toml` — adds `libc = { workspace = true }` (already in workspace deps per top-level `Cargo.toml`). No new transitive deps; `libc` is already in the dependency tree via other crates.
- `crates/adapter-claude/src/normalize.rs` — after extracting `session_id`, extract `bowerbird_ppid` via `value.get("bowerbird_ppid").and_then(|v| v.as_u64()).and_then(|n| u32::try_from(n).ok())`. Missing or malformed → `None`. Threaded onto `EventEnvelope.pid`.
- `crates/daemon/src/db/migrations.rs` — appends a `M::up()` for v2: `ALTER TABLE events ADD COLUMN pid INTEGER` (nullable; existing rows have `NULL`; idempotency check covered by Story 5.4's migration-idempotency contract test).
- `crates/daemon/src/db/queries.rs` — `INSERT_EVENT` gains a `pid` bind position; `SELECT_EVENT_KINDS_FOR_SESSION` (used by rebuild) also selects `pid` so rebuild can re-thread it.
- `crates/daemon/src/projection/state.rs::transition()` — signature gains a `pid: Option<u32>` parameter; returned `SessionState` has `last_pid = pid.or(prev.and_then(|p| p.last_pid))` (overwrite-on-Some, carry-forward-on-None). Pure-function and AC #5 invariants preserved.
- `crates/daemon/src/projection/session.rs::write()` — passes `envelope.pid` into `transition()`; binds `envelope.pid` to `INSERT_EVENT`. Atomicity of state-emission + event-INSERT (Story 1.6) is preserved — both writes still inside the same transaction.
- `crates/daemon/src/projection/session.rs::rebuild_missing_projections()` — the rebuild loop reads `(kind, created_at, pid)` from `events` instead of `(kind, created_at)`, and threads `pid` through the per-event `transition()` call so the rebuilt `last_pid` matches what live ingest would have produced.
- `crates/protocol/tests/contract_protocol.rs` — round-trip `SessionState` with `last_pid: Some(_)` and `last_pid: None`; assert v1.0-shaped JSON (no `last_pid` field at all) still deserializes into `SessionState { last_pid: None, .. }` (additive-compat canary).
- `crates/daemon/tests/contract_daemon.rs` — assert that a non-sentinel event with `EventEnvelope.pid = Some(N)` produces a `session_projections.state` row whose deserialized `SessionState.last_pid == Some(N)`, and that a follow-up event with `EventEnvelope.pid = None` preserves the prior `last_pid`.
- `crates/adapter-claude/tests/contract_adapter.rs` — assert normalize round-trips `bowerbird_ppid` from raw JSON into `EventEnvelope.pid`; assert a missing field decodes as `None` without erroring.
- No CI workflow changes; no infrastructure changes; no deployment surface changes.

### Out-of-scope (deliberately deferred)

- **Parent-walk past `sh -c` wrappers.** Claude Code's hook spawn model may wrap the shim in a short-lived `sh` invocation, in which case `getppid()` returns the sh PID — alive for the duration of the hook, dead milliseconds later. v0.1.0 ships the raw PPID and accepts the dogfooding signal. If the dogfooding pattern reveals high false-positive "dead" rates within seconds of the last event, a v0.1.X follow-up adds a one-level parent walk via macOS `proc_pidinfo(PROC_PIDT_BSDINFO)` and Linux `/proc/<pid>/stat`. Tracked as a deferred-work entry, not blocking v0.1.0.
- **PID-recycle resilience via `started_at` capture.** Workstation-scale PID-recycle within the few-minutes-to-hours window deck cares about is empirically rare. Capturing the parent process's `pbi_start_tvsec` (Darwin) / `starttime` (Linux) alongside the PID is the obvious hardening; deferred to a v0.1.X follow-up gated on a real dogfooding false-positive.
- **Cross-host presenter support.** `kill(pid, 0)` lies if the presenter and daemon are on different hosts. V1 deployment model is single-host (`127.0.0.1` bind per project-context.md §HTTP surface); the non-loopback bind is already an open question requiring its own ADR (project-context.md §Open questions). No new constraint introduced here.
- **Per-session auto-prune of dead rows.** The substrate continues to emit dead-PID rows; pruning, sorting, and visibility decisions are presenter-side (Axiom 1).
- **New cookbook entry.** Per the reaction-enum-rule discipline (project-context.md §Substrate-not-actor invariants line 699 — "follows demand; does not anticipate it"), a "detecting dead sessions" cookbook entry waits until two independent presenters demonstrate the need. Deferred-work entry added pointing at the gap.

## 3. Recommended approach

**Selected: Option 1 — Direct Adjustment.** Add one new Story 5.8 in Epic 5; renumber existing 5.8 → 5.9; update planning artifacts and protocol docs in lockstep.

| Option considered | Verdict |
|---|---|
| 1. Direct Adjustment (new story in Epic 5) | ✅ Viable, low risk, medium effort (~1 PR, ~10 file edits, 1 SQLite migration, 4–6 contract test updates) |
| 2. Rollback Story 1.6 + 2.2 | ❌ Not viable — would revert the substrate; the new fact is additive, not a defect requiring rewind |
| 3. PRD MVP review / scope reduction | ❌ Not needed — additive presenter capability, no MVP scope change |
| 4. Skip `Event.pid` migration; only put PID on `SessionState` (cheaper) | ❌ Not viable for v0.1.0 — would erode Story 1.6 AC #5 (storage layer pure function of event sequence) and make `rebuild_missing_projections` return `last_pid = None` for every existing session. AC #5 is load-bearing for the projection-rebuild contract test (Required Contract Tests table in project-context.md). Migration is one column; not worth eroding the invariant. |

**Rationale:** Adding `last_pid` to `SessionState` is the smallest substrate fact that gives presenters a non-semantic way to filter dead sessions. The shim hot path absorbs one syscall + one map insert (well inside budget). The adapter does one field extract. The projection does one parameter thread-through. The migration is one nullable column. v1.0 presenters are unaffected (outbound permissive). The architecture's existing PID-liveness pattern (`bowerbird.pid` + `kill(pid, 0)` for daemon discovery) is extended one layer down for sessions, no new design surface introduced.

Effort: **Medium** (~1.5 days implementation, ~0.5 day docs + contract tests). Risk: **Low** (additive wire, AC #5 preserved by storing pid per-event, no presenter semantics in daemon, no new dependency in shim crate beyond a `libc` line already in the workspace). Timeline impact: **+1 story to Epic 5**, no critical path delay.

## 4. Detailed change proposals

### 4.1 New Story 5.8 in `epics.md`

Insert between current Story 5.7 (Session state projection correctness) and current Story 5.8 (Crates.io namespace + v0.1.0 tag). Renumber existing 5.8 → 5.9.

```
### Story 5.8: Session-process liveness via PID capture

As a presenter author,
I want a mechanical fact tying each session row to the OS process that
last emitted a hook for it,
So that I can distinguish "user walked away" from "Claude Code process
is gone" without smuggling a semantic flag into the substrate, and so my
ribbon UI can filter or sort out tombstone sessions on its own.

Closes the dogfooding finding in
sprint-change-proposal-2026-05-27-pid-liveness.md. Extends the daemon-level
PID-liveness pattern (architecture.md:987-989 `bowerbird.pid` + `kill(pid, 0)`)
one layer down to per-session granularity.

Acceptance Criteria:

Given a Claude Code hook fires and the shim runs
When the shim sends the payload to the daemon's ingest socket
Then the payload JSON includes a `bowerbird_ppid` field whose value is
the integer returned by `libc::getppid()` at shim-invocation time; the
field is injected by the shim, not present in the upstream Claude Code
hook payload; the shim hot-path p99 ≤5ms budget (Story 1.5) is preserved
under the shim-bench-gate

Given the adapter-claude normalize path receives a payload with
`bowerbird_ppid` set
When normalize constructs the EventEnvelope
Then EventEnvelope.pid is Some(<that value>); a payload missing
`bowerbird_ppid` or carrying a non-integer value yields
EventEnvelope.pid = None and is normalized successfully (not a failure
mode)

Given an EventEnvelope with `pid: Some(N)` reaches
projection::session::write
When the projection writes inside its single transaction
Then the events row stores `pid = N`; the upserted session_projections
row's deserialized SessionState carries `last_pid: Some(N)`; the
BroadcastEnvelope::State published after commit carries the same
last_pid; the BroadcastEnvelope::Event likewise carries `pid: Some(N)`

Given a follow-up EventEnvelope for the same (source, session_id) with
`pid: None`
When the projection writes
Then SessionState.last_pid retains the prior Some(N) (carry-forward
semantics); the events row stores `pid = NULL` for that specific event

Given a follow-up EventEnvelope for the same (source, session_id) with
`pid: Some(M)` where M != N (process resumed the session_id under a new
PID)
When the projection writes
Then SessionState.last_pid becomes Some(M) (overwrite-on-Some semantics)

Given a daemon restart with a non-empty events table that has session
rows
When `rebuild_missing_projections` runs
Then for each rebuilt session the reconstructed SessionState.last_pid
matches what live ingest would have produced from the same event
sequence (Story 1.6 AC #5 "storage layer is a pure function of the event
sequence" is preserved); rebuilt rows for sessions whose events all have
`pid IS NULL` have `last_pid: None`

Given GET /sessions and GET /sessions/{id}
When the daemon serializes the response
Then SessionListItem and SessionDetail.state each carry `last_pid` as a
number-or-null field; the read-time stale-Working → Idle fallback
(Story 1.6 current_state_for_read) does NOT alter last_pid; the
sentinel session row (source = "__daemon__") continues to be filtered
out

Given a WS subscriber to state.session.* receives a StateFrame
When the frame is decoded
Then frame.state.last_pid carries the same value as the REST
SessionDetail.state.last_pid would for the same session at the same
moment; snapshot-on-subscribe frames (Story 2.3) likewise carry
last_pid

Given a v1.0 presenter compiled against the pre-Story-5.8 protocol type
When it deserializes a SessionState frame from a Story-5.8+ daemon
Then serde silently ignores the `last_pid` field (asymmetric outbound
permissive policy per project-context.md §Wire format / Story 4.4
catch-all); no decode error, no crash, no protocol-violation close
frame; the additive-compat contract test in contract_protocol.rs
exercises this path

Given the SQLite events schema before Story 5.8 (v1)
When the daemon starts against an existing v1 database
Then migration v2 runs `ALTER TABLE events ADD COLUMN pid INTEGER`;
existing rows have `pid = NULL`; the migration is idempotent (re-running
to_latest is a no-op per Story 5.4's contract test)

Given the protocol surface
When Story 5.8 lands
Then crates/protocol/src/state.rs SessionState gains `last_pid:
Option<u32>`; crates/protocol/src/event.rs EventEnvelope gains `pid:
Option<u32>` (internal type, ingest boundary) AND Event gains `pid:
Option<u32>` (stored/wire type, REST + WS emission); crates/shim/Cargo.toml
adds the workspace libc dep; crates/shim/src/main.rs injects
`bowerbird_ppid` into the payload; crates/adapter-claude/src/normalize.rs
extracts it

Given the doc and contract-test surface
When Story 5.8 lands
Then docs/protocol.md adds last_pid to /sessions, /sessions/{id},
StateFrame, and adds pid to EventFrame; docs/protocol.md §Ingest socket
contract documents the shim's `bowerbird_ppid` injection;
docs/protocol-changelog.md gains three entries (schema:
SessionState.last_pid; schema: Event.pid + events.pid migration v2;
behavioral: shim PPID injection); crates/protocol/tests/contract_protocol.rs
exercises both round-trip and additive-compat; crates/daemon/tests/contract_daemon.rs
exercises projection threading and rebuild AC #5; crates/adapter-claude/tests/contract_adapter.rs
exercises normalize extraction

Given the planning artifacts
When Story 5.8 lands
Then architecture.md adds a bullet under §State-machine narrative noting
`last_pid` is carried by transition() but does not influence
current_state; architecture.md §Singletons & discovery (≈L987) gains a
forward reference noting the same pattern applies one layer down for
sessions

Given documented v0.1.0 caveats
When Story 5.8 lands
Then deferred-work.md records three follow-up entries: (a)
parent-walk past sh -c wrappers via proc_pidinfo / /proc/<pid>/stat;
(b) PID-recycle resilience via started_at capture; (c) cookbook entry
"detecting dead sessions" (gated on the reaction-enum-rule: ship when
two independent presenters demonstrate the need)
```

### 4.2 Existing Story 5.8 → 5.9 renumber

Header change: `### Story 5.8: Crates.io namespace decision and v0.1.0 tag` → `### Story 5.9: Crates.io namespace decision and v0.1.0 tag`.

AC closing condition `Given all Epic 5 stories 5.1 through 5.7 are complete` → `Given all Epic 5 stories 5.1 through 5.8 are complete`. All other AC text unchanged.

### 4.3 `sprint-status.yaml`

```diff
   5-7-session-state-projection-correctness: backlog
-  5-8-crates-io-namespace-and-v0-1-0-tag: backlog
+  5-8-session-process-liveness-pid-capture: backlog
+  5-9-crates-io-namespace-and-v0-1-0-tag: backlog
   epic-5-retrospective: optional
```

Plus an additional `last_updated` line at the top:

```
# last_updated: 2026-05-27 (Story 5.8 session-process-liveness-pid-capture inserted; old 5.8→5.9 per sprint-change-proposal-2026-05-27-pid-liveness.md)
```

### 4.4 `docs/protocol.md` additive edits

Five additive field edits — exact line numbers verified during the story's Dev Notes, not pinned here because Story 5.7 lands before 5.8 and shifts line numbers in the broadcast and EventKind sections.

| Section | Edit |
|---|---|
| `/sessions` response example (≈L81) | add `"last_pid": 12345` (or `null`) field to the example object |
| `/sessions/{id}` response example (≈L102) | add `"last_pid"` field to the `state` sub-object |
| `StateFrame` example (≈L265) | add `"last_pid"` field to the `state` sub-object; update narrative to note it's a mechanical fact, presenter-derives liveness via `kill(pid, 0)` |
| `EventFrame` (locate during impl) | add `"pid": 12345` (or `null`) field to the example |
| §Ingest socket contract | one new bullet: "The shim MAY inject `bowerbird_ppid: <integer>` into the JSON payload before send. The adapter-claude normalize path extracts it into `EventEnvelope.pid`. A missing or non-integer value is normalized as `pid: None` (not a failure)." |

### 4.5 `docs/protocol-changelog.md`

Three new entries under v1.0 → v1.1, placed after the Story 5.7 entries (which themselves are queued for that release):

```
- **type: schema** — `SessionState.last_pid: Option<u32>` field added (Story 5.8). Carries the PID returned by `libc::getppid()` at shim-invocation time for the most recent hook that produced an event for this (source, session_id). The field is a mechanical fact only — the substrate makes no claim about whether the PID is alive. Presenters compute liveness locally via `kill(pid, 0)` (mirroring the daemon-level pattern documented at architecture.md:987-989 for `bowerbird.pid`). Carry-forward semantics: an envelope with `pid: None` preserves the prior `last_pid`; an envelope with `pid: Some(N)` overwrites; a `claude --resume` of an existing session_id under a new PID overwrites on its first hook (this is by design — the latest hook's source PID is the most useful liveness signal). The read-time stale-Working → Idle fallback (Story 1.6) does not interact with `last_pid`. v1.0 presenters decode the field as missing/ignored per the asymmetric outbound-permissive serde policy (project-context.md §Wire format, Story 4.4 catch-all line). Sentinel session rows (`source = "__daemon__"`) continue to be excluded from session-listing queries (Story 1.7). (`Resolves: 5.8`)

- **type: schema** — `Event.pid: Option<u32>` field added and SQLite events table gains a nullable `pid INTEGER` column via migration v2 (Story 5.8). The per-event PID is stored so that `rebuild_missing_projections` can re-thread it through `transition()` and reproduce `SessionState.last_pid` exactly — preserving Story 1.6 AC #5 ("storage layer is a pure function of the event sequence"). Existing v1 rows have `pid = NULL`; the migration is idempotent per Story 5.4's migration-idempotency contract test. `EventFrame` over WS and `GET /sessions/{id}/events` over REST both surface `pid` as a number-or-null field. v1.0 presenters decode the field as missing/ignored. (`Resolves: 5.8`)

- **type: behavioral** — Ingest-socket contract: shim injects `bowerbird_ppid` into the hook payload (Story 5.8). The shim, after parsing the inbound hook JSON, calls `libc::getppid()` and inserts a `bowerbird_ppid: <integer>` field into the payload object alongside the existing `hook_kind` injection (Story 1.5). The adapter-claude normalize path extracts the field into `EventEnvelope.pid`; a missing field, a non-integer value, or a value outside `u32` range is normalized as `pid: None` (not a parse error — pid is opportunistic, not load-bearing). Hot-path cost is one `getppid()` syscall (~hundreds of ns) plus one `serde_json::Map::insert`; the Story 1.5 p99 ≤5ms shim budget is preserved under the Story 5.2 shim-bench-gate. The NDJ framing on `~/.bowerbird/ingest.sock` (ADR-0002) is unchanged. Tools that built custom shims must add the field if they want their sessions to carry liveness PIDs; absence degrades gracefully to `last_pid: None`. (`Resolves: 5.8`)
```

### 4.6 `architecture.md` amendments

Two additive bullets — exact line numbers verified during the story's Dev Notes (Story 5.7 lands before 5.8 and shifts line numbers in the same section).

**At the §State-machine narrative section (≈L1026 FR table):**

> Add a bullet under FR24–FR26 row: "Session liveness: `SessionState.last_pid` carries the PID of the parent process that last fired a hook for this session_id; `transition()` carries it forward (overwrite-on-Some, keep-on-None) but it does NOT influence `current_state`. Presenters compute `is_alive` locally via `kill(pid, 0)` per Axiom 1 — the substrate emits the mechanical fact, the presenter interprets. Mirrors the daemon-level `bowerbird.pid` + `kill(pid, 0)` pattern documented in §Singletons & discovery."

**At §Singletons & discovery (≈L987 after the existing `bowerbird.pid` bullet):**

> Add a forward reference: "The same PID-as-liveness-probe pattern extends one layer down to per-session granularity via `SessionState.last_pid` (Story 5.8). See §State-machine narrative."

### 4.7 `deferred-work.md` follow-up entries

Three new entries added at the end of the file:

```
- 5.8 follow-up: parent-walk past sh -c wrappers via macOS proc_pidinfo(PROC_PIDT_BSDINFO) / Linux /proc/<pid>/stat. v0.1.0 ships raw getppid(); if dogfooding reveals high false-positive "dead" rates because Claude Code wraps the shim in `sh -c`, add a one-level parent walk in the shim. Gated on a real dogfooding signal, not pre-emptively shipped.
- 5.8 follow-up: PID-recycle resilience via started_at capture. Pair last_pid with the parent process's start time (pbi_start_tvsec on Darwin, starttime in /proc/<pid>/stat on Linux) so a recycled PID can be detected as a different process. Gated on a real dogfooding false-positive on workstation timescales (minutes-to-hours).
- 5.8 follow-up: cookbook entry "detecting dead sessions". Demonstrates the local kill(pid, 0) pattern. Gated on the reaction-enum-rule discipline (project-context.md §Substrate-not-actor invariants): ship the cookbook entry when two independent presenters demonstrate the need.
```

## 5. PRD MVP impact

MVP scope unchanged. The Marcus narrative (`prd.md:206`) is about *active*-session ribbon UX (Idle / Working / WaitingInput), not about dead-session filtering. Story 5.8 is an additive presenter capability that improves dogfooding signal quality but does not alter the V1 promise.

## 6. Implementation handoff

**Scope classification: Moderate.**

The change spans planning artifacts (Epic 5 ordering, sprint-status), protocol docs (4 sections in protocol.md, 3 changelog entries), production code (4 crates, 1 SQLite migration), and contract tests (3 test files). It is not Major (no PRD/architecture rewrite, no MVP impact) but it is not Minor either (multi-crate touch + migration + wire-protocol surface change).

**Handoff recipients:**

| Role | Responsibility |
|---|---|
| Product Owner (pickles) | Approve this proposal (Section 5 of bmad-correct-course workflow); insert new Story 5.8 into `epics.md` and renumber existing 5.8 → 5.9; update `sprint-status.yaml`; update `architecture.md` and `docs/protocol.md` cross-references in the same PR |
| Developer agent | Implement the multi-crate change (shim PPID capture, adapter extract, protocol fields, daemon projection threading, v2 migration, contract tests); land in a single PR; document the three deferred-work follow-ups |
| Story automator (Story 5.1 in-progress alongside this) | Create the Story 5.8 implementation-artifacts file (`docs/bmad/implementation-artifacts/5-8-session-process-liveness-pid-capture.md`) once this proposal is approved, following the bmad-create-story flow |

**Success criteria:**

1. `last_pid` visible in `bowerbird-deck` against a real Claude Code workstation within one development cycle.
2. v1.0 presenter binary (built before Story 5.8) connects to a Story-5.8+ daemon without error (additive-compat round-trip contract test green).
3. Shim p99 ≤5ms budget preserved under the shim-bench-gate (Story 5.2's chaos-injection sanity work covers the post-merge regression check).
4. After at least 1 week of dogfooding with Story 5.8 in place, decide whether to promote any of the three deferred-work follow-ups (parent-walk, started_at, cookbook entry) into Story 5.8.X-hotfix work or leave them post-V1.

## 7. Trade-offs and alternatives summary

The alternatives are surfaced inline in §3 (Recommended approach) and §2 (Out-of-scope). The single load-bearing trade-off worth re-stating: storing `pid` per-event with a v2 migration is more work than putting `last_pid` only on `SessionState`, but the per-event storage is what preserves Story 1.6 AC #5 ("storage layer is a pure function of the event sequence"). AC #5 is the contract that makes the projection-rebuild contract test green; eroding it for ~30 lines of saved migration code is not a good trade for a substrate project whose value proposition is durability and replayability.
