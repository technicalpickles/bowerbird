# Story 5.3: Daemon-observed session liveness + typed-notification WaitingInput

Status: review

<!-- Note: Validation is optional. Run validate-create-story for quality check before dev-story. -->

## Story

As a presenter author,
I want the substrate to observe process death and emit a mechanical `SessionEnded` event, and I want `WaitingInput` to reflect Claude's typed `notification_type` field rather than collapse every `Notification` into one bucket,
so that my ribbon UI can render an accurate per-session state without doing its own liveness syscalls, without doing its own payload regex on `notification_type`, and without breaking when the presenter and daemon are on different machines.

**Closes two Story 5.1 dogfooding findings** against `bowerbird-deck`:
1. ~48 sessions stuck at `WaitingInput`, none actually waiting — terminals closed without firing `Stop`, frozen on the last `Notification`.
2. No mechanical signal for "session process is gone" — every presenter would have to call `kill(pid, 0)` itself.

**Operationalizes ADR 0004** (`docs/decisions/0004-daemon-observed-session-liveness.md`). **Refines Story 5.2's** `PostToolUse → preserve prior` rule to `PostToolUse → Working unconditionally` (a session in `WaitingInput` whose tool call completes mid-elicitation now correctly transitions back to `Working`). Resequenced 5.8 → 5.3 by `sprint-change-proposal-2026-05-27-epic-5-resequencing.md`; design amended in `sprint-change-proposal-2026-05-28-daemon-observed-liveness.md` (the canonical spec for this story).

**Important — epics.md is stale.** `docs/bmad/planning-artifacts/epics.md` §"Story 5.3" (lines 976–1040) still carries the ORIGINAL 13 ACs from the pre-amendment PID-only design. The amended ACs (below) come from `sprint-change-proposal-2026-05-28-daemon-observed-liveness.md` §4.1 and supersede the epics.md text in full. Task 14 rewrites epics.md in the same PR.

## Acceptance Criteria

1. **Given** a Claude Code hook fires and the shim runs **When** the shim sends the payload to the daemon's ingest socket **Then** the payload JSON includes a `bowerbird_ppid` field whose value is the integer returned by `libc::getppid()` at shim-invocation time; the field is injected by the shim, not present in the upstream Claude Code hook payload; the shim hot-path p99 ≤5ms budget (Story 1.5) is preserved under the shim-bench-gate.

2. **Given** the `adapter-claude` normalize path receives a payload with `bowerbird_ppid` set **When** normalize constructs the `EventEnvelope` **Then** `EventEnvelope.pid` is `Some(<that value>)`; a payload missing `bowerbird_ppid` or carrying a non-integer value yields `EventEnvelope.pid = None` and is normalized successfully (not a failure mode).

3. **Given** the `adapter-claude` normalize path receives a payload with `hook_kind = Notification` and a `notification_type` field **When** normalize constructs the `EventEnvelope` **Then** `EventEnvelope.notification_type` is `Some(NotificationType::X)` for known values (`permission_prompt`, `idle_prompt`, `auth_success`, `elicitation_dialog`, `elicitation_response`, `elicitation_complete`); an unrecognized value yields `Some(NotificationType::Unknown)`; a missing field yields `None`; the event is normalized successfully in all three cases.

4. **Given** an `EventEnvelope` with `pid: Some(N)` reaches `projection::session::write` **When** the projection writes inside its single transaction **Then** the `events` row stores `pid = N`; the upserted `session_projections` row's deserialized `SessionState` carries `last_pid: Some(N)`; the `BroadcastEnvelope::State` published after commit (if gated through per Story 5.2) carries the same `last_pid`; the `BroadcastEnvelope::Event` likewise carries `pid: Some(N)`.

5. **Given** a follow-up `EventEnvelope` for the same `(source, session_id)` with `pid: None` **When** the projection writes **Then** `SessionState.last_pid` retains the prior `Some(N)` (carry-forward semantics); the `events` row stores `pid = NULL` for that specific event.

6. **Given** a follow-up `EventEnvelope` for the same `(source, session_id)` with `pid: Some(M)` where `M != N` **When** the projection writes **Then** `SessionState.last_pid` becomes `Some(M)` (overwrite-on-Some semantics).

7. **Given** an `EventEnvelope` for `hook_kind = Notification` with `notification_type` in `{PermissionPrompt, IdlePrompt, ElicitationDialog}` **When** the projection's `transition` function runs **Then** the resulting `current_state` is `WaitingInput`; the prior state is irrelevant.

8. **Given** an `EventEnvelope` for `hook_kind = Notification` with `notification_type` in `{AuthSuccess, ElicitationResponse, ElicitationComplete}` OR `notification_type = Unknown` OR `notification_type = None` **When** the projection's `transition` function runs **Then** the resulting `current_state` preserves the prior state (no transition).

9. **Given** an `EventEnvelope` for `hook_kind = PostToolUse` **When** the projection's `transition` function runs **Then** the resulting `current_state` is `Working` unconditionally (refines Story 5.2's "preserve prior" rule — flagged as a `type: behavioral` changelog entry); `last_event_kind` and `last_event_at_ms` update normally.

10. **Given** the daemon completes `run_migrations` and `rebuild_missing_projections` at startup **When** the daemon proceeds to accept connections **Then** one synchronous iteration of the liveness probe has run before the WS server binds — for each `session_projections` row where `last_pid IS NULL` OR `libc::kill(last_pid as i32, 0) != 0` (errno = ESRCH), a `SessionEnded` event is written via the normal `projection::session::write` path; the projection row transitions to `current_state = Ended`; the events row carries `source = <row's source>`, `session_id = <row's session_id>`, `kind = SessionEnded`, `payload = {"reason": "no_pid_at_upgrade"|"pid_dead", "pid": <last_pid or null>, "observed_at_ms": <epoch_ms>}`.

11. **Given** the daemon is running steady-state with the WS server up **When** the periodic liveness probe task wakes (5-second cadence via `tokio::time::interval` with `MissedTickBehavior::Skip`) **Then** the same per-row logic from the startup iteration runs; `SessionEnded` events are written and broadcast on `events.*`; resulting state transitions are broadcast on `state.session.*`; an in-flight probe iteration that takes longer than the tick interval does NOT queue (next tick skipped).

12. **Given** a `session_projections` row in `current_state = Ended` **When** a subsequent hook `EventEnvelope` arrives for the same `(source, session_id)` (e.g. from `claude --resume`) **Then** `transition` runs normally: `UserPromptSubmit`/`PreToolUse`/`PostToolUse → Working`; `Stop → Idle`; `Notification` with input-required `notification_type` → `WaitingInput`; `last_pid` updates from the new envelope's PID via overwrite-on-Some semantics; the row exits `Ended`.

13. **Given** a daemon restart with a non-empty `events` table that includes `SessionEnded` events **When** `rebuild_missing_projections` runs **Then** for each rebuilt session the reconstructed `SessionState.last_pid` AND `current_state` match what live ingest would have produced from the same event sequence (Story 1.6 AC #5 "storage layer is a pure function of the event sequence" is preserved); `SessionEnded` events in the log drive transitions to `Ended` during rebuild exactly as they did during live ingest.

14. **Given** `GET /sessions` and `GET /sessions/{id}` **When** the daemon serializes the response **Then** `SessionListItem` and `SessionDetail.state` each carry `last_pid` as a number-or-null field; `SessionCurrentState` includes the new `Ended` variant in `current_state` for rows where the liveness probe observed death; the read-time stale-`Working` → `Idle` fallback (Story 1.6 `current_state_for_read`) does NOT alter `last_pid` and does NOT interfere with `Ended` (which passes through unchanged); the sentinel session row (`source = "__daemon__"`) continues to be filtered out.

15. **Given** a WS subscriber to `state.session.*` receives a `StateFrame` **When** the frame is decoded **Then** `frame.state.last_pid` carries the same value as the REST `SessionDetail.state.last_pid` would for the same session at the same moment; snapshot-on-subscribe frames (Story 2.3) likewise carry `last_pid`; transitions to `Ended` (driven by the liveness probe) broadcast a `StateFrame` per the Story 5.2 transitions-only policy.

16. **Given** a WS subscriber to `events.*` receives an `EventFrame` **When** the frame is decoded for a `SessionEnded` event **Then** the frame carries `kind = "SessionEnded"`, the real `source` and `session_id` of the session that ended, and a payload object with `reason`, `pid` (number or null), and `observed_at_ms`.

17. **Given** a v1.0 presenter compiled against the pre-Story-5.3 protocol type **When** it deserializes a `SessionState` frame, a `StateFrame`, or an `EventFrame` from a Story-5.3+ daemon **Then** serde silently ignores the `last_pid` field; the `Ended` `SessionCurrentState` variant decodes to `Unknown` via the Story 4.4 `#[serde(other)]` catch-all; the `SessionEnded` `EventKind` decodes to `Unknown` via the same catch-all; no decode error, no crash, no protocol-violation close frame; additive-compat contract tests in `contract_protocol.rs` exercise each path.

18. **Given** the SQLite `events` schema before Story 5.3 (v1) **When** the daemon starts against an existing v1 database **Then** migration v2 runs `ALTER TABLE events ADD COLUMN pid INTEGER`; existing rows have `pid = NULL`; the migration is idempotent (re-running `to_latest` is a no-op per Story 5.4's migration-idempotency contract test).

19. **Given** the protocol surface **When** Story 5.3 lands **Then** `crates/protocol/src/state.rs` `SessionState` gains `last_pid: Option<u32>` AND `SessionCurrentState` gains the `Ended` variant; `crates/protocol/src/event.rs` `EventEnvelope` gains `pid: Option<u32>` (internal) AND `notification_type: Option<NotificationType>` (internal), `EventKind` gains the `SessionEnded` variant, a new `NotificationType` enum is added with six known variants + `Unknown`, and stored `Event` gains `pid: Option<u32>`; `crates/shim/Cargo.toml` adds the workspace `libc` dep; `crates/shim/src/main.rs` injects `bowerbird_ppid`; `crates/adapter-claude/src/normalize.rs` extracts both `bowerbird_ppid` and `notification_type`; a new module `crates/daemon/src/projection/liveness.rs` houses the probe loop.

## Tasks / Subtasks

- [x] **Task 1: Add `NotificationType` enum + `last_pid`/`pid`/`notification_type` fields to the protocol crate** (AC: #4, #5, #6, #17, #19)
  - [x] Edit `crates/protocol/src/event.rs`. Add a new enum `NotificationType` with variants `PermissionPrompt`, `IdlePrompt`, `AuthSuccess`, `ElicitationDialog`, `ElicitationResponse`, `ElicitationComplete`, plus `Unknown` last with `#[serde(other)]`. Use `#[serde(rename = "permission_prompt")]` etc. on each known variant so the wire form is Claude Code's snake_case while the Rust identifier stays PascalCase. The `Unknown` catch-all matches the Story 4.4 / Epic 2 retro AI-4 additive-compat pattern.
  - [x] In the same file, add `EventKind::SessionEnded` BEFORE `RecordingStarted` (chronological-ish lifecycle ordering: hooks → daemon-observed → sentinels → catch-all). `Unknown` stays last with `#[serde(other)]`.
  - [x] Extend `EventEnvelope` with `pub pid: Option<u32>` AND `pub notification_type: Option<NotificationType>` (internal pre-storage type — these fields are NOT directly serialized to the wire; the adapter populates them and the projection threads them).
  - [x] Extend stored `Event` with `pub pid: Option<u32>` (this IS on the wire — REST `GET /sessions/{id}/events`, WS `EventFrame.event`). Do NOT add `notification_type` to stored `Event`; per ADR 0004 §3 + sprint-change-proposal-2026-05-28 §2 "Out-of-scope: Typed `notification_type` exposed on the projection / wire StateFrame," the typed value stays in `events.payload` for archaeology.
  - [x] Edit `crates/protocol/src/state.rs`. Add `SessionCurrentState::Ended` BEFORE `Unknown`. Extend `SessionState` with `pub last_pid: Option<u32>`. The `Ended` variant's doc comment must call out that it is **non-terminal** (per ADR 0004 §1) — the next hook event transitions out via normal `transition()` rules.
  - [x] Run `cargo build -p protocol`. No new dependencies; no new derive.

- [x] **Task 2: Shim injects `bowerbird_ppid`** (AC: #1, #19)
  - [x] Edit `crates/shim/Cargo.toml`. Add `libc = { workspace = true }`. The workspace `libc` dep already exists (used elsewhere). Confirm with `cargo build -p bowerbird-shim`.
  - [x] Edit `crates/shim/src/main.rs`. After the existing `obj.insert("hook_kind", ...)` call (line 51–54) and BEFORE the `serde_json::to_vec(&value)` serialize step, inject `obj.insert("bowerbird_ppid".to_string(), serde_json::Value::Number(serde_json::Number::from(unsafe { libc::getppid() })))`. The `unsafe` block is unavoidable (`libc::getppid()` is `unsafe extern "C"`); document it inline with a one-line comment explaining "getppid is signal-safe and cannot fail." This is the ONLY `unsafe` block in the shim; `#![deny(unsafe_code)]` is NOT at the shim crate root today (verify) — if it is, the inline `#[allow(unsafe_code)]` lives on the line, not the crate.
  - [x] **Hot-path discipline (Story 1.5 reminder):** no allocations beyond what `serde_json::Number::from` already does (zero — `Number` wraps an enum); no async runtime; one syscall added. The bench gate (Story 5.5) is the actual proof, but be conscious of the budget while writing.
  - [x] Add a test in `crates/shim/tests/contract_shim.rs`: invoke the shim binary with `--hook-kind PreToolUse` against a mock ingest socket; capture the framed payload; assert `bowerbird_ppid` is present in the JSON object and is a positive integer that matches the test runner's PID (since the shim's parent IS the test runner). For a Notification hook test, also assert `notification_type` from the input survives verbatim into the framed payload (the shim doesn't extract it — that's the adapter's job — but it shouldn't strip or rename it either).

- [x] **Task 3: Adapter extracts `bowerbird_ppid` and `notification_type`** (AC: #2, #3, #19)
  - [x] Edit `crates/adapter-claude/src/normalize.rs::normalize`. After the existing `hook_kind` match block (lines 68–75) and AFTER `session_id` extraction, extract `pid` from the JSON value: `let pid = value.get("bowerbird_ppid").and_then(|v| v.as_u64()).and_then(|n| u32::try_from(n).ok())`. A missing field, a non-integer value, or an out-of-range value all yield `pid = None` without failing normalization. Add a NEW arm in the match block — when `kind == EventKind::Notification`, extract `notification_type` via `value.get("notification_type").and_then(|v| v.as_str()).map(|s| match s { "permission_prompt" => NotificationType::PermissionPrompt, "idle_prompt" => NotificationType::IdlePrompt, "auth_success" => NotificationType::AuthSuccess, "elicitation_dialog" => NotificationType::ElicitationDialog, "elicitation_response" => NotificationType::ElicitationResponse, "elicitation_complete" => NotificationType::ElicitationComplete, _ => NotificationType::Unknown, })`. For non-Notification kinds, `notification_type = None`.
  - [x] Thread both `pid` and `notification_type` onto the `EventEnvelope` construction at the end of `normalize`.
  - [x] **Out-of-scope:** the shim does NOT extract `notification_type` (the shim doesn't peek into the payload's semantic fields — only injects `hook_kind` and `bowerbird_ppid`). The adapter is the right boundary.
  - [x] Add tests in `crates/adapter-claude/tests/contract_adapter.rs`:
    - `normalize_extracts_pid_when_bowerbird_ppid_set` — payload with `bowerbird_ppid: 12345` → `envelope.pid == Some(12345)`.
    - `normalize_extracts_pid_none_when_missing` — payload without the field → `envelope.pid == None`.
    - `normalize_extracts_pid_none_when_non_integer` — payload with `bowerbird_ppid: "string"` → `envelope.pid == None`, no error.
    - `normalize_extracts_notification_type_for_six_known_values` — table-driven test over all six known strings.
    - `normalize_extracts_notification_type_unknown_for_future_value` — payload with `notification_type: "future_type"` → `envelope.notification_type == Some(NotificationType::Unknown)`.
    - `normalize_extracts_notification_type_none_when_missing` — payload without the field → `envelope.notification_type == None`.
    - `normalize_does_not_extract_notification_type_for_non_notification_kinds` — `PreToolUse` payload with a stray `notification_type` field → `envelope.notification_type == None`.

- [x] **Task 4: Migration v2 — add `events.pid` column** (AC: #18)
  - [x] Edit `crates/daemon/src/db/migrations.rs`. Add a `V2_UP` constant: `"ALTER TABLE events ADD COLUMN pid INTEGER"`. Append to the `migrations()` vec: `M::up(V2_UP)`. `rusqlite_migration` records the applied version in `user_version`; idempotency falls out for free.
  - [x] Add a unit test in `crates/daemon/src/db/migrations.rs` (or extend an existing one): apply migrations to an in-memory connection twice, assert the second `to_latest` is a no-op (zero rows affected by re-running). Story 5.4's migration-idempotency contract test (referenced in AC #18) is the broader gate but will not exist yet when this story lands; the unit test in this PR is the bridge.
  - [x] Update `crates/daemon/src/db/queries.rs::INSERT_EVENT`: extend column list and placeholder list to include `pid`. New SQL: `"INSERT INTO events (source, session_id, kind, reaction, payload, created_at, pid) VALUES (?, ?, ?, ?, ?, ?, ?)"`.
  - [x] Update `crates/daemon/src/db/queries.rs::SELECT_EVENT_BY_ID`, `SELECT_EVENTS_FOR_SESSION_SINCE` to include `pid` in their select column lists. Any helper that reconstructs an `Event` row needs to pull and pass `pid`.
  - [x] Update `crates/daemon/src/db/queries.rs::SELECT_EVENT_KINDS_FOR_SESSION` if it's used by the rebuild path AND the rebuild needs `pid` (it does — see Task 6 AC #13). Either extend it to `SELECT kind, created_at, pid` or use a separate query for rebuild that includes `pid`.

- [x] **Task 5: Thread `pid` and `notification_type` through `projection::session::write`** (AC: #4, #5, #6, #7, #8, #9, #15, #16)
  - [x] Edit `crates/daemon/src/projection/state.rs::transition`. Signature gains a `notification_type: Option<NotificationType>` parameter (positional, after `event_kind`). Existing arms unchanged EXCEPT:
    - `EventKind::PostToolUse`: change from `prev.map(|s| s.current_state).unwrap_or(SessionCurrentState::Working)` (Story 5.2's "preserve prior") to `SessionCurrentState::Working` unconditionally (AC #9). Update the doc comment lines 44–49 to reflect the refinement and cross-reference ADR 0004 §4.
    - `EventKind::Notification`: replace the blind `SessionCurrentState::WaitingInput` with a `match notification_type` block per AC #7/#8:
      ```rust
      EventKind::Notification => match notification_type {
          Some(NotificationType::PermissionPrompt)
          | Some(NotificationType::IdlePrompt)
          | Some(NotificationType::ElicitationDialog) => SessionCurrentState::WaitingInput,
          Some(NotificationType::AuthSuccess)
          | Some(NotificationType::ElicitationResponse)
          | Some(NotificationType::ElicitationComplete)
          | Some(NotificationType::Unknown)
          | None => {
              return prev.cloned().unwrap_or(SessionState {
                  current_state: SessionCurrentState::Idle,
                  last_event_kind: event_kind,
                  last_event_at_ms: now_ms,
                  last_pid: None,
              });
          }
      },
      ```
      The "preserve prior" branch needs the same `return prev.cloned().unwrap_or(...)` shape as the defensive `RecordingStarted | RecordingEnded | Unknown` arm, so `last_event_kind`/`last_event_at_ms` still update — wait, that doesn't match. Re-read the defensive arm: it returns prev UNCHANGED. The "preserve prior" branch for Notification should update `last_event_kind` and `last_event_at_ms` (the event still happened) but keep `current_state`. Use the same shape as Story 5.2's PostToolUse arm (before this story changes it): `prev.map(|s| s.current_state).unwrap_or(SessionCurrentState::Idle)` and let the `SessionState { current_state, last_event_kind, last_event_at_ms, last_pid }` construction at the bottom of `transition` carry the new event fields.
    - Add a new arm for `EventKind::SessionEnded`: `EventKind::SessionEnded => SessionCurrentState::Ended`.
    - Add `last_pid` to the final `SessionState` construction (and to the `prev.cloned().unwrap_or(SessionState { ... })` constructions in the defensive branches). The new field needs a value source — see next subtask.
  - [x] `transition` ALSO needs to know the incoming envelope's `pid` so it can carry-forward / overwrite per AC #5 and #6. Add a `pid: Option<u32>` parameter to `transition` (positional, after `notification_type`). At the bottom `SessionState` construction, set `last_pid: pid.or(prev.and_then(|s| s.last_pid))` — carry-forward semantics: an envelope with `pid: Some(M)` overwrites; an envelope with `pid: None` preserves the prior `last_pid`. For the defensive `RecordingStarted | RecordingEnded | Unknown` arm and the Notification "preserve prior" branch, use the same carry-forward expression.
  - [x] Edit `crates/daemon/src/projection/session.rs::write`. The closure already extracts `kind` from the envelope (line 78). Add `let notification_type = envelope.notification_type` and `let pid = envelope.pid` above the closure (line 78ish). Pass both into the `transition` call inside the closure (line 145). Pass `pid` into the SQL `INSERT_EVENT` `params!` (line 162–172) — add it as the last positional after `now_ms`. Pass `pid` into the `Event { ... }` construction (line 195–203) — set `pid` to the envelope's `pid`. Story 5.2's `prev_raw_current_state` / `prev_read_current_state` gating logic for `BroadcastEnvelope::State` stays unchanged — the new `Ended` transitions naturally flow through it (`Ended != Working` → publish).
  - [x] Update existing `transition` unit tests in `state.rs` to pass `notification_type: None` and `pid: None` where they previously didn't have those args. The five existing tests for Notification (`transition_notification_yields_waiting_input`, etc.) need to be split into per-notification_type cases — see Task 9.

- [x] **Task 6: Update `rebuild_missing_projections` to thread `pid` and `notification_type`** (AC: #13)
  - [x] Edit `crates/daemon/src/projection/session.rs::rebuild_missing_projections` (lines 370–470). The current loop reads `(kind_str, created_at)` from `SELECT_EVENT_KINDS_FOR_SESSION`. Extend the SELECT to also pull `pid` AND `payload` (the latter is needed to extract `notification_type` from Notification rows during rebuild).
  - [x] For each event row in the rebuild loop, if `kind == EventKind::Notification`, parse `payload` as `serde_json::Value` and extract `notification_type` via the same logic as the adapter (Task 3) — share a helper if practical, or duplicate the match. Pass the extracted `Option<NotificationType>` into `transition`.
  - [x] Pass `pid: Option<u32>` (parsed from the new `events.pid` column) into `transition`. Rows written before migration v2 have `pid = NULL`, which carries forward as `last_pid: None` — the eager startup probe (Task 7) then emits `SessionEnded` for those rows with `reason: "no_pid_at_upgrade"`.
  - [x] Add a contract test in `crates/daemon/tests/contract_daemon.rs::rebuild_preserves_last_pid_and_ended`: insert event rows with mixed `pid` (some `NULL`, some integers) and `SessionEnded` markers; delete `session_projections`; restart; assert reconstructed `SessionState.last_pid` matches the last non-NULL pid in the event sequence AND `current_state == Ended` for sessions ended in the log.

- [x] **Task 7: New module `crates/daemon/src/projection/liveness.rs` + probe task** (AC: #10, #11)
  - [x] Create the file. Module structure:
    ```rust
    use std::time::Duration;
    use deadpool_sqlite::Pool;
    use tokio_util::sync::CancellationToken;
    use crate::broadcast::BroadcastHub;

    pub(crate) const PROBE_CADENCE: Duration = Duration::from_secs(5);

    /// One probe iteration: scan `session_projections` for rows where
    /// `current_state != 'Ended'`, check liveness via `kill(pid, 0)`, emit
    /// `SessionEnded` events for dead-or-no-PID rows.
    pub(crate) async fn probe_once(writer_pool: &Pool, broadcaster: &BroadcastHub) -> Result<usize, Error> { ... }

    /// Long-running probe loop. Calls `probe_once` on a 5s interval with
    /// `MissedTickBehavior::Skip` so slow iterations don't queue.
    pub(crate) async fn run(writer_pool: Pool, broadcaster: Arc<BroadcastHub>, shutdown: CancellationToken) { ... }
    ```
  - [x] `probe_once` SQL: `SELECT source, session_id, state FROM session_projections WHERE source != '__daemon__'`. Deserialize each `state` blob, inspect `last_pid` and `current_state`, skip rows where `current_state == Ended` (already done). For each candidate: check `kill(pid, 0)` (or `last_pid IS NULL`). For dead-or-no-PID rows, synthesize an `EventEnvelope { source, session_id, kind: EventKind::SessionEnded, reaction: None, payload: <JSON>, pid: <last_pid>, notification_type: None }` and call `projection::session::write` — this reuses the existing transactional write + broadcast path. **Do NOT bypass `write`** — that would break the "exactly two writes per event" invariant from architecture.md §634-641.
  - [x] **`kill(pid, 0)` wrapper.** Use `libc::kill(pid as i32, 0)` directly (the `daemon` crate already has `libc` available via the workspace). Check the return value: `0` = alive, `-1` = check `errno`. `ESRCH` (process does not exist) is the "dead" signal; `EPERM` (we don't have permission) is treated as "alive" (defensive — if we can't tell, don't kill the session). Wrap this in a helper `is_pid_alive(pid: u32) -> bool` with inline docs.
  - [x] **Payload shape** per AC #10: `{"reason": "no_pid_at_upgrade"|"pid_dead", "pid": <last_pid as number or null>, "observed_at_ms": <epoch_ms>}`. Serialize via `serde_json`. The shape lives in the daemon (it's emitted-by-daemon, not consumed-by-daemon), so a private struct in `liveness.rs` with `Serialize` derive is fine — does NOT need to be in the protocol crate.
  - [x] **`MissedTickBehavior::Skip`** rationale (per ADR 0004 §"Cadence shorter/longer than 5s"): if probe_once takes longer than 5s (large session_projections table), we want the next tick skipped, not queued. `tokio::time::interval` defaults to `Burst` which DOES queue. Explicit `interval.set_missed_tick_behavior(MissedTickBehavior::Skip)`.
  - [x] **Shutdown discipline.** The `run` task selects on `shutdown.cancelled()` and the interval tick. On shutdown, exit cleanly (do not finish an in-flight iteration mid-way through writing `SessionEnded` events — the writer pool will already have started closing). The graceful-shutdown sequence in `main.rs` cancels `shutdown_requested` BEFORE waiting on tasks, so this is naturally observed.

- [x] **Task 8: Wire the probe task into daemon startup** (AC: #10, #11)
  - [x] Edit `crates/daemon/src/main.rs`. The current sequence (lines 156–212): `init_pools → run_migrations → rebuild_missing_projections → write_recording_started → adapter setup → ingest writer task → ingest listener task → WS server`. INSERT one synchronous `liveness::probe_once(&pools.writer, &broadcaster).await` call AFTER `rebuild_missing_projections` and BEFORE `write_recording_started`. The broadcaster doesn't exist yet at that point in the current code (it's constructed at line 206), so move `let broadcaster = Arc::new(BroadcastHub::new(...))` UP to before the probe call. **Order matters:** broadcaster construction does not block on anything (it's just a hub object), so moving it up is safe.
  - [x] Spawn the periodic probe task BEFORE `tokio::net::TcpListener::bind` (so it's running once axum starts serving): `let liveness_task = tokio::spawn(liveness::run(pools.writer.clone(), broadcaster.clone(), shutdown_requested.clone()))`. Add the task's `JoinHandle` to the shutdown wait list (alongside `ingest_writer_task` and `ingest_listener_task`).
  - [x] **Critical AC #10 detail:** the synchronous probe runs BEFORE the WS server binds the listener. The relevant line is `let listener = tokio::net::TcpListener::bind(...).await?`. The probe-once call must happen before that line. Even with the periodic task running, the synchronous startup probe guarantees presenters connecting at t=0 see the post-cleanup state in their snapshot.
  - [x] On shutdown, await `liveness_task` after the ingest tasks (alongside `ingest_writer_task.await`). The probe respects `shutdown_requested.cancelled()` per Task 7.

- [x] **Task 9: Update daemon contract tests for transition rules** (AC: #7, #8, #9, #10, #11, #12, #13)
  - [x] Edit `crates/daemon/src/projection/state.rs` unit tests. Replace `transition_notification_yields_waiting_input` with a parameterized test over all six known `NotificationType` values + `Unknown` + `None`:
    - Three values (`PermissionPrompt`, `IdlePrompt`, `ElicitationDialog`) → `WaitingInput`.
    - Three values (`AuthSuccess`, `ElicitationResponse`, `ElicitationComplete`) → preserve prior.
    - `Unknown` → preserve prior.
    - `None` → preserve prior.
  - [x] Rewrite `transition_posttooluse_preserves_working` to `transition_posttooluse_yields_working_unconditionally` — assert `PostToolUse` with prev=`Idle` returns `Working`; with prev=`WaitingInput` returns `Working`; with prev=`Ended` returns `Working`. Delete `transition_posttooluse_without_prev_defaults_to_working` (subsumed by the new test).
  - [x] Add `transition_session_ended_yields_ended` — `transition(prev, EventKind::SessionEnded, now)` returns `Ended`.
  - [x] Add `transition_from_ended_resumes_on_hook_event` — prev=`Ended`, then `UserPromptSubmit` returns `Working`; prev=`Ended`, then `Stop` returns `Idle`; prev=`Ended`, then `Notification(PermissionPrompt)` returns `WaitingInput`.
  - [x] Add `transition_carry_forward_last_pid` — prev with `last_pid: Some(100)`, new envelope with `pid: None` → new state's `last_pid == Some(100)`. prev with `last_pid: Some(100)`, new envelope with `pid: Some(200)` → new state's `last_pid == Some(200)`. prev=None, new envelope with `pid: Some(100)` → new state's `last_pid == Some(100)`.
  - [x] Edit `crates/daemon/tests/contract_daemon.rs::state_machine_full_sequence_determinism`. Update the `cases` array to encode the new table: `PostToolUse → Working` (was `Working` already under Story 5.2, no change there); add `Notification + PermissionPrompt → WaitingInput`; add `Notification + AuthSuccess → preserve prior`; add `SessionEnded → Ended`. Also update any other test in `contract_daemon.rs` that constructs a `Notification` envelope — it now needs a `notification_type` value, and the existing tests assumed blind `→ WaitingInput`. Grep for `EventKind::Notification` and audit each call site.
  - [x] Add `state_broadcast_publishes_for_session_ended` — spawn hermetic daemon, write some events that put a session in `Working`, force-write a `SessionEnded` event via the probe (or via direct `projection::session::write` with a `SessionEnded` envelope), assert subscribers receive one `EventFrame` for `SessionEnded` AND one `StateFrame` with `state.current_state == Ended`.
  - [x] Add `liveness_probe_emits_session_ended_for_no_pid_at_upgrade` — pre-populate `session_projections` with a row whose `state` JSON has `last_pid: None`, run `liveness::probe_once`, assert a `SessionEnded` event row was written with `payload.reason == "no_pid_at_upgrade"` and the projection row transitioned to `Ended`.
  - [x] Add `liveness_probe_emits_session_ended_for_dead_pid` — pre-populate with `last_pid: Some(99999)` (a PID that's almost certainly not running; the test should pick a PID it knows is dead — spawn a short-lived subprocess, capture its PID, await its exit, then use that PID). Run probe, assert `SessionEnded` with `reason: "pid_dead"`.
  - [x] Add `liveness_probe_skips_alive_pid` — pre-populate with `last_pid: Some(<test runner's PID>)` (via `std::process::id()`), run probe, assert NO `SessionEnded` event was written, projection unchanged.
  - [x] Add `session_ended_then_resume_exits_ended` — write events putting session in `Ended`, then ingest a `UserPromptSubmit` for the same `(source, session_id)`, assert projection transitions to `Working` (AC #12).
  - [x] Add `liveness_probe_missed_tick_does_not_queue` — synthesize a slow-running `probe_once` (e.g. populate 1000 rows requiring kill() each), advance virtual time past two ticks, assert the second tick was skipped (not queued). Use `tokio::test(start_paused = true)` + `tokio::time::advance` per project-context.md §"Deterministic test discipline."

- [x] **Task 10: Update REST API serialization** (AC: #14)
  - [x] Edit `crates/daemon/src/api/sessions.rs`. The current `SessionListItem` push at line 89 doesn't carry `last_pid` directly — it pulls from `SessionState` projection blob. Since `SessionState` now contains `last_pid` (Task 1), it's automatically serialized when `SessionDetail` includes `state: SessionState`. Verify: `SessionListItem` does NOT directly carry `last_pid` per the current shape (it has `current_state`, `last_event_kind`, `last_event_at_ms`, `updated_at` flattened from `SessionState`). Decide whether `SessionListItem` should grow `pub last_pid: Option<u32>` for parity with `SessionDetail.state.last_pid`. **Recommended:** yes — presenters listing sessions to render the deck need `last_pid` per row, and a flattened field is cheaper than a nested struct. Add it to `crates/protocol/src/rest.rs::SessionListItem` and populate it in `sessions::list` from the deserialized state.
  - [x] Verify that `current_state_for_read` (Story 1.6) correctly passes `Ended` through unchanged (it does — the function only special-cases stale `Working`, all other states pass through verbatim). Add a unit test in `state.rs::tests`: `current_state_for_read_does_not_stale_ended`.
  - [x] Verify the `__daemon__` sentinel filter on `SELECT_NON_SENTINEL_SESSIONS` (`crates/daemon/src/db/queries.rs:40-43`) is unchanged — `SessionEnded` events carry the real `source` and `session_id`, so the sentinel filter is irrelevant for them.

- [x] **Task 11: Update protocol crate contract tests** (AC: #17)
  - [x] Edit `crates/protocol/tests/contract_protocol.rs`. Add `SessionEnded` and all six `NotificationType` variants + `Unknown` to existing PascalCase / snake_case round-trip assertions. Verify `NotificationType` wire strings are snake_case (`"permission_prompt"` etc.) — this is the one place in the protocol where serde rename is used, so a regression test is critical.
  - [x] Add `additive_compat_ended_session_current_state_decodes_as_unknown` — JSON `{"current_state":"Ended","last_event_kind":"Stop","last_event_at_ms":0,"last_pid":null}` deserialized through a LOCAL mock copy of `SessionCurrentState` that has only `Idle | Working | WaitingInput | #[serde(other)] Unknown` (no `Ended`) decodes the `"Ended"` string as `Unknown`. Same pattern as Story 5.2's `pre_story_5_2_presenter_decodes_user_prompt_submit_as_unknown` test.
  - [x] Add `additive_compat_session_ended_event_kind_decodes_as_unknown` — full event JSON `{"event_id":1,"source":"claude","session_id":"x","kind":"SessionEnded","reaction":null,"payload":"{}","created_at":0,"pid":null}` deserialized through a LOCAL mock copy of `EventKind` (legacy variants + `#[serde(other)] Unknown`, no `SessionEnded`) decodes as `Unknown`.
  - [x] Add `additive_compat_last_pid_is_ignored_by_v1_consumer` — JSON with `last_pid: 12345` decoded through a LOCAL mock `SessionState`-shaped struct that lacks the `last_pid` field decodes successfully (no `deny_unknown_fields` — confirming outbound permissive policy per project-context.md §Wire format).
  - [x] Same for `pid` on `Event`: `additive_compat_pid_is_ignored_by_v1_consumer`.

- [x] **Task 12: Documentation updates** (AC: #19, support for #14, #15, #16, #17)
  - [x] Edit `docs/protocol.md`. Per sprint-change-proposal-2026-05-28 §4.4:
    - §`SessionCurrentState` (≈L282): add `Ended` variant. Add narrative: "`Ended` is daemon-observed (not hook-driven). It indicates the session's `last_pid` is no longer a live OS process. It is **not terminal** — a session can transition out of `Ended` on the next hook event (typically a `UserPromptSubmit` from `claude --resume`)."
    - §`EventKind` table (≈L348): add `SessionEnded` row. Update `Notification` row narrative to point at the new `notification_type`-driven `WaitingInput` rules.
    - §`/sessions` response (≈L81): add `"last_pid": 12345` (or `null`) to the example object.
    - §`/sessions/{id}` response (≈L102): add `"last_pid"` to the `state` sub-object.
    - §`StateFrame` (≈L265): add `"last_pid"` to the `state` sub-object; note that presenters do NOT need to call `kill(pid, 0)` — they receive `SessionEnded` events.
    - §`EventFrame` (locate during impl): add `"pid": 12345` (or `null`); document `SessionEnded` payload shape: `{"reason": "no_pid_at_upgrade" | "pid_dead", "pid": <number or null>, "observed_at_ms": <epoch_ms>}`.
    - §Ingest socket contract: add the `bowerbird_ppid` injection note AND the `notification_type` extraction note (two bullets, from sprint-change-proposal §4.4's narrative).
  - [x] Edit `docs/protocol-changelog.md`. Add SIX new entries under v1.0 → v1.1, AFTER the Story 5.2 entries, in this order (full text in sprint-change-proposal-2026-05-28 §4.5; reproduce verbatim — the entries are pre-written):
    1. `type: schema` — `SessionState.last_pid: Option<u32>` field added.
    2. `type: schema` — `Event.pid: Option<u32>` field + SQLite events table migration v2.
    3. `type: schema` — `SessionCurrentState::Ended` + `EventKind::SessionEnded` + `NotificationType` enum added.
    4. `type: behavioral` — `Notification → WaitingInput` mapping refined to be `notification_type`-aware.
    5. `type: behavioral` — `PostToolUse → Working` refinement of Story 5.2's "preserve prior."
    6. `type: behavioral` — Ingest-socket contract: shim injects `bowerbird_ppid`, adapter extracts `notification_type`.
  - [x] Edit `docs/presenter-authoring.md` (per sprint-change-proposal §4.7). Add a new subsection "Rendering `Ended` sessions" (default: hide; alternative: dim/strike-through; explicitly DO NOT call `kill(pid, 0)` in presenters). Add another subsection "Rendering `WaitingInput` sessions" noting that the typed `notification_type` stays in `events.payload` for richer rendering — presenters can subscribe to events too if they want to distinguish `permission_prompt` from `idle_prompt`.
  - [x] Edit `docs/bmad/planning-artifacts/architecture.md` (per sprint-change-proposal §4.6). Update §State-machine narrative (≈L1026 FR table row) with the new bullet covering daemon-side probe + `Ended` + notification_type-driven WaitingInput. Add a forward reference at §Singletons & discovery (≈L987) — the same PID-as-liveness-probe pattern applies one layer down for sessions; the session-level probe runs inside the daemon.

- [x] **Task 13: Update `deferred-work.md`** (per sprint-change-proposal §4.8)
  - [x] Retain (with revised text) the parent-walk and `started_at` follow-up entries — see sprint-change-proposal §4.8 for verbatim text.
  - [x] REMOVE the original 5.3's "cookbook entry: detecting dead sessions" entry (no longer needed — the daemon emits the event, presenters consume it).
  - [x] ADD a new entry: `STALE_WORKING_MS` retirement, tracked as a separate follow-up per ADR 0004 §5.

- [x] **Task 14: Rewrite `epics.md` §Story 5.3 ACs in place** (per sprint-change-proposal §4.1)
  - [x] Edit `docs/bmad/planning-artifacts/epics.md` lines 976–1040. Replace the OLD 13-AC story body with the NEW story body from sprint-change-proposal-2026-05-28 §4.1. Updated narrative paragraph + 19 revised ACs.
  - [x] **Do NOT renumber** subsequent stories (5.4 → 5.9 stay as-is). Only the §"Story 5.3" section content changes.
  - [x] The story title changes too: "Session-process liveness via PID capture" → "Daemon-observed session liveness + typed-notification WaitingInput". Update the H3 heading at line 976.

- [x] **Task 15: Update `sprint-status.yaml`** (AC: implicit)
  - [x] Story key `5-3-session-process-liveness-pid-capture` stays unchanged (per sprint-change-proposal §4.2 — "no entry-shape changes"). Filename of this story file also stays `5-3-session-process-liveness-pid-capture.md` (Story Automator naming is a separate cleanup; bundling it with this PR creates renames that complicate diff review).
  - [x] When the story moves to `in-progress`: update entry value, add a `last_updated` line.
  - [x] When the story moves to `review` (post `dev-story`): update entry, add `last_updated`.
  - [x] When the story moves to `done` (post `code-review`): update entry, add `last_updated`.

- [x] **Task 16: Satisfy the protocol-changelog gate** (AC: implicit)
  - [x] Story 5.2 introduced the gate (`tests/protocol_changelog_gate.rs`). Story 5.3 touches `crates/protocol/src/event.rs` and `crates/protocol/src/state.rs`, so the gate fires. The six new changelog entries (Task 12) provide ample `+`-prefixed `type:` lines — gate passes automatically. No extra work, but verify locally with `BOWERBIRD_CHANGELOG_GATE_BASE=origin/main` set.

- [x] **Task 17: Full workspace test suite serialized** (AC: all)
  - [x] `cargo test --workspace -- --test-threads=1`. Serialized per Epic 2 retro AI-3.
  - [x] `cargo fmt --check` — workspace-wide.
  - [x] `cargo clippy --all-targets --workspace -- -D warnings` — workspace-wide.
  - [x] **Plan to spend disproportionate time on contract test audit.** The `Notification`-without-`notification_type` change ripples through many existing tests that construct `EventEnvelope { kind: EventKind::Notification, ... }` and expect a `WaitingInput` transition. Each call site now needs `notification_type: Some(NotificationType::PermissionPrompt)` (or one of the WaitingInput-triggering values) to keep the test's intent intact. Grep for `EventKind::Notification` and `kind: Notification` across `crates/daemon/tests/` AND `crates/daemon/src/`.

- [x] **Task 18: Manual smoke against running daemon** (AC: #1, #2, #3, #10, #11, #16)
  - [x] After all tests pass, build a release binary, `bowerbird uninstall && bowerbird install`. Restart any running daemon. Confirm `~/.bowerbird/bower.db` migrates cleanly (existing v1 DB → v2 column added; existing rows have `pid = NULL`).
  - [x] Restart the daemon against the maintainer's accumulated workstation history (the ~48 stale `WaitingInput` row corpus mentioned in sprint-change-proposal-2026-05-28 §1). The eager startup probe should emit ~48 `SessionEnded` events. Verify via `sqlite3 ~/.bowerbird/bower.db 'SELECT count(*) FROM events WHERE kind = "SessionEnded"'` — count should match the legacy-row count.
  - [x] Subscribe `bowerbird-deck` (or `wscat` to `state.session.*`); confirm `Ended` rows are now `Ended`, not `WaitingInput`.
  - [x] Start a live Claude Code session; trigger a `permission_prompt` Notification (e.g., a Bash tool that requires confirmation). Confirm the deck shows that session as `WaitingInput`. Resolve the prompt; confirm transition through `Working` → `Idle` on `Stop`.
  - [x] Close the Claude Code terminal mid-session. Wait ≤5s. Confirm the session disappears from the deck (or transitions to `Ended` if the deck renders `Ended` rows as dim).
  - [x] **Out of scope:** formal load testing of the probe loop. The probe touches O(n) rows where n = active sessions; 5s cadence with `MissedTickBehavior::Skip` is comfortable headroom. If dogfooding surfaces probe-cost issues, that's a follow-up.

## Dev Notes

### Why this story exists (the user-visible defect, again)

The `bowerbird-deck` against the maintainer's workstation: 48 sessions stuck at `WaitingInput`, all >10 min stale, 38 in the 1-24h range, 8 over 24h. **None** are actually waiting for the maintainer's input — they're sessions whose terminals closed without firing `Stop`, frozen on the last `Notification` they emitted (overwhelmingly `idle_prompt`, which is a "yo I'm idle" beep, not a "give me a permission" prompt).

Story 5.1 surfaced this. The original Story 5.3 (PID capture + presenter-side `kill(pid, 0)`) was approved 2026-05-27. Story 5.1 dogfooding continued. Two further findings forced the amendment:

1. **Cross-machine deployment.** Presenters on machine B watching a daemon on machine A can't `kill()` machine A's PIDs. The current single-host bind hides this, but it breaks the V1 substrate's "WS+JSON to anything" story.
2. **Notification semantics are richer than `WaitingInput`.** Claude's `notification_type` enumerates six values; the substrate threw all of them away and substituted a one-bit conclusion (`WaitingInput`) that was wrong half the time.

The reframe (ADR 0004): **observing process death is a mechanical fact, equivalent in nature to observing a hook firing.** The semantic ("should I render this session?") stays in the presenter; the *observation* belongs in the substrate.

### What "preserve prior" means in the new Notification arm

The Notification arm now branches on `notification_type`:

- **Three WaitingInput types** (`PermissionPrompt`, `IdlePrompt`, `ElicitationDialog`): construct a new `SessionState` with `current_state: WaitingInput`, `last_event_kind: Notification`, `last_event_at_ms: now_ms`, `last_pid: <carry-forward>`.
- **Three transient types** (`AuthSuccess`, `ElicitationResponse`, `ElicitationComplete`) + `Unknown` + `None`: construct a new `SessionState` with `current_state: prev.map(|s| s.current_state).unwrap_or(Idle)`, `last_event_kind: Notification`, `last_event_at_ms: now_ms`, `last_pid: <carry-forward>`. The new `last_event_kind`/`last_event_at_ms` MUST update — the event happened, REST/snapshot readers expect freshness — only `current_state` is preserved.

This is NOT the same as the defensive `RecordingStarted | RecordingEnded | Unknown` arm (which returns `prev.cloned().unwrap_or(...)` — i.e., prev UNCHANGED including its `last_event_kind`). Use the standard bottom-of-function `SessionState { ... }` construction for the Notification preserve-prior branch.

### The `last_pid` carry-forward / overwrite-on-Some pattern

The semantics:

| Prev `last_pid` | New envelope `pid` | New `last_pid` |
|---|---|---|
| `None` | `None` | `None` |
| `None` | `Some(N)` | `Some(N)` |
| `Some(P)` | `None` | `Some(P)` (carry-forward) |
| `Some(P)` | `Some(N)` | `Some(N)` (overwrite, even if P == N) |

One expression: `pid.or(prev.and_then(|s| s.last_pid))`. The `pid` argument is `Option<u32>` from the envelope; `prev` is `Option<&SessionState>` from the closure. `pid.or(...)` takes `pid` if it's `Some`, otherwise falls back to the prior. Use this in EVERY arm of `transition` that constructs a `SessionState` — the defensive arms, the standard arm, and the Notification preserve-prior branch.

### Liveness probe cost analysis (back-of-envelope)

- Per row: one `serde_json::from_str` (state blob), one `libc::kill(pid, 0)` syscall (~microseconds), zero allocations beyond what serde needs.
- For 1000 sessions: ~1ms total syscall cost + serde overhead. Well below the 5s tick interval.
- For 10,000 sessions: maybe 10-50ms. Still well below threshold. If we ever hit 100k sessions on a workstation, the cadence is the wrong knob — that's a "shard the table" problem.

The probe runs in a single `spawn_blocking` (or async, since `kill` is cheap enough we don't need to offload). Verify with strace / instruments / `tokio-console` during dogfooding that probe iterations are sub-100ms; if not, route through `spawn_blocking`.

### Why `MissedTickBehavior::Skip` matters

`tokio::time::interval` defaults to `Burst` — missed ticks queue. If `probe_once` takes 7s on a slow iteration, the next tick fires immediately, then the next 5s later (no recovery time). Under sustained slowness, the queue grows and the probe starves the runtime.

`Skip` says "if you missed a tick because the body was slow, just skip it and resume from the next scheduled tick." This is the right policy for a periodic maintenance task — we don't NEED every tick to fire exactly on schedule, we just need to probe roughly every 5s.

ADR 0004 §"Cadence shorter than 5s" considered 1s and rejected it (marginal value not worth 5× per-tick work). 5s is the chosen sweet spot. Don't change it without a new ADR.

### The protocol changelog gate is still alive

Story 5.2 introduced the gate (`tests/protocol_changelog_gate.rs`). It fires when any file under `crates/protocol/src/*.rs` changes in the PR diff against `origin/main`. Story 5.3 touches `event.rs` AND `state.rs`, so the gate fires. The six new changelog entries (Task 12) provide 6+ `+`-prefixed `type:` lines, so the gate passes — but if any of those entries get accidentally collapsed to a single line during PR review, the gate might pass barely. Run `cargo test --workspace -- protocol_changelog_gate` locally with `BOWERBIRD_CHANGELOG_GATE_BASE=origin/main` before pushing.

### The `notification_type` is consumed but not exposed on the wire StateFrame

Per ADR 0004 §3 + sprint-change-proposal-2026-05-28 §2 "Out-of-scope," the typed `notification_type` value drives the projection transition and is then **discarded from the projection**. It remains in `events.payload` (verbatim, never stripped) for presenters that want richer rendering — they subscribe to `events.<source>.<session_id>` and parse it themselves.

The reasoning (maintainer's design conversation 2026-05-28): "I don't like having latest_notification on the projection. I want a slight interpolation of it." The typed-field-driven `current_state` IS the slight interpolation. Adding a parallel `pending_input` field on the projection would force every presenter to compose two axes; rejected.

If you find yourself adding `notification_type` to `SessionState`, `StateFrame`, or `SessionDetail`, stop — that's a scope creep. The right discussion is whether to surface a richer signal, and that's an ADR + sprint-change-proposal conversation, not an implementation choice.

### Why the synchronous startup probe is non-optional

If the daemon just spawned the periodic task and accepted connections immediately, a presenter connecting at t=0.5s would receive a snapshot of `session_projections` BEFORE the first probe iteration emitted any `SessionEnded` events. The snapshot would show 48 ghost `WaitingInput` rows. Five seconds later, the first probe iteration emits 48 `SessionEnded` state-transition envelopes — the presenter sees them, but only if it's still connected. Cold-connect presenters at t=0 to t=5s see stale state.

The synchronous startup probe closes that gap: by the time the WS server is bound and accepting connections, the projection is already correct. The 48 ghost rows are gone in the snapshot. This matters MOST for the upgrade case (lots of legacy `last_pid IS NULL` rows) but applies equally to steady-state (any dead session whose PID died between daemon-down and daemon-up).

The cost: startup blocks on one probe iteration. Per the cost analysis above, ≤100ms for any reasonable session count. Acceptable.

### Watch the variant ordering in `EventKind` and `SessionCurrentState`

Per project convention (Story 5.2 Dev Notes, restated): `Unknown` MUST stay last with `#[serde(other)]`. The other variants can be in any order, but project preference is chronological lifecycle: hooks first (`UserPromptSubmit`, `PreToolUse`, `PostToolUse`, `Stop`, `Notification`), then daemon-observed (`SessionEnded`), then sentinels (`RecordingStarted`, `RecordingEnded`), then `Unknown`. Slot `SessionEnded` accordingly.

For `SessionCurrentState`: existing order is `Idle | Working | WaitingInput | Unknown`. Slot `Ended` between `WaitingInput` and `Unknown` — order isn't load-bearing for serde (the wire string IS the variant identifier), but human readers look for chronology.

### Files this story touches (read each carefully)

Story 5.2's File List had 7 source files + 5 test files + 2 doc files. Story 5.3 is bigger:

- **8 source files modified:** protocol/event.rs, protocol/state.rs, shim/main.rs, shim/Cargo.toml, adapter-claude/normalize.rs, daemon/projection/state.rs, daemon/projection/session.rs, daemon/main.rs, daemon/db/migrations.rs, daemon/db/queries.rs, daemon/api/sessions.rs, protocol/rest.rs.
- **1 source file created:** daemon/projection/liveness.rs.
- **5 test files modified:** protocol/tests/contract_protocol.rs, daemon/tests/contract_daemon.rs, adapter-claude/tests/contract_adapter.rs, shim/tests/contract_shim.rs. (No contract_install.rs change needed — the hook kinds list isn't touched by this story.)
- **4 doc files modified:** protocol.md, protocol-changelog.md, presenter-authoring.md, architecture.md.
- **3 planning artifacts modified:** epics.md (§5.3 rewrite), deferred-work.md, sprint-status.yaml.

Plan for the dev session to be 2-3 days end-to-end. Most of that time is contract test audit (every existing test that constructs a Notification envelope needs an updated `notification_type`).

### Previous story intelligence (Story 5.2 — done)

Story 5.2 closed the over-broadcasting and PostToolUse-flap defects with three substrate changes: `transition()` PostToolUse arm preserves prev (THIS STORY REFINES THAT TO `→ Working`); `write()` gates `State` publish on transition; `UserPromptSubmit` hook wired through ingest. Six review findings caught in patch — the most relevant to Story 5.3:

- **Shim CLI parse boundary.** Story 5.2 review finding #1 was that `bowerbird install` wrote `--hook-kind UserPromptSubmit` but the shim's `parse_hook_kind` didn't accept it. Story 5.3 doesn't add hook kinds (the shim accepts all five Story 5.2 kinds), so this trap isn't repeating — but the lesson is: **whenever you wire a new code path end-to-end (shim → adapter → daemon → projection → broadcaster → presenter), test the actual binary, not just unit tests.** The Story 5.2 `shim_user_prompt_submit_round_trip_persists_working` contract test is the model — copy its shape for `shim_injects_bowerbird_ppid_into_payload` (Task 2).
- **State publish gating + stale-Working interaction.** Story 5.2 review finding #2 was that the broadcast gating compared raw stored prev to raw stored new, missing the case where the read-facing prev was different (stale-`Working` → snapshot says `Idle`; new event arrives → stored `Working` to stored `Working`; subscriber stuck on `Idle`). The fix was to compare BOTH raw and read-facing prev. **Story 5.3 inherits this gating logic** — make sure new `Ended` transitions exercise it. The probe-emitted `SessionEnded → Ended` transition is naturally `Working → Ended` or `WaitingInput → Ended` or `Idle → Ended` — all three differ from `Ended` itself, so the `State` envelope publishes. But add a test for the `Ended → Working` resume case: the prev is `Ended`, the new is `Working`, the publish must fire.
- **Contract test landscape is bigger than it looks.** Story 5.2 audited 8 collateral tests that relied on the old `PostToolUse → Idle` rule. Story 5.3's audit is similar: every test that constructs `EventEnvelope { kind: Notification, ... }` needs a `notification_type` value, AND every test that constructs `EventEnvelope { kind: PostToolUse, ... }` and expects a specific `current_state` outcome needs to be reread under the new "→ Working unconditionally" rule. Grep both. Plan time for it.
- **Forward-compat tests need full event JSON shape, not just the bare kind string.** Story 5.2 review finding #4: the original additive-compat test deserialized only `"UserPromptSubmit"` as a string into a legacy enum, which didn't prove a v1.0 presenter could parse the full event payload. Fixed to construct a full event JSON object. **Apply the same shape to Story 5.3's additive-compat tests:** the legacy presenter must deserialize a full `Event` JSON with `kind: "SessionEnded"` AND `pid: 12345`, not just the bare `"SessionEnded"` string.

### What to do about the original 13 ACs in epics.md

epics.md §"Story 5.3" lines 976–1040 carry the OLD 13-AC story body. They're not internally inconsistent — they're a coherent older design — but they don't match what this story actually ships. Task 14 rewrites the section in place.

**Don't read the old ACs as the spec.** The story file you're reading IS the spec (ACs above). The old ACs in epics.md are historical until Task 14 rewrites them. The sprint-change-proposal-2026-05-28 §4.1 is the canonical source for the new AC text; Task 14 is a verbatim transcription of that.

### Watch the `EventEnvelope` field count grow

This story adds TWO new fields to `EventEnvelope` (`pid`, `notification_type`). Every constructor in the codebase (test fixtures, mock builders, the ingest writer, replay) needs updating. Grep for `EventEnvelope {` and `EventEnvelope::new`. If a test still constructs it with old field positionals after this story lands, the build fails — a forcing function for the audit.

Consider whether to introduce an `EventEnvelope::builder()` pattern to make future additions less expensive. **Recommended:** NOT in this story (scope creep), but flag it as a follow-up if the constructor count is painful. Story 5.2 added one field (`source` was already there; nothing new added to envelope itself) — Story 5.3 adds two. If a future story adds a third or fourth, the builder pays off.

### Project Structure Notes

- `crates/protocol/src/event.rs` — UPDATE — `EventKind` gains `SessionEnded`; `EventEnvelope` gains `pid` + `notification_type`; `Event` gains `pid`; new `NotificationType` enum.
- `crates/protocol/src/state.rs` — UPDATE — `SessionCurrentState` gains `Ended`; `SessionState` gains `last_pid`.
- `crates/protocol/src/rest.rs` — UPDATE — `SessionListItem` gains `last_pid` (flattened from state per the recommended approach in Task 10).
- `crates/shim/Cargo.toml` — UPDATE — add `libc = { workspace = true }`.
- `crates/shim/src/main.rs` — UPDATE — inject `bowerbird_ppid` via `libc::getppid()` after the existing `hook_kind` injection.
- `crates/adapter-claude/src/normalize.rs` — UPDATE — extract `bowerbird_ppid` → `EventEnvelope.pid`; extract `notification_type` → `EventEnvelope.notification_type` (Notification only).
- `crates/daemon/src/db/migrations.rs` — UPDATE — append `V2_UP = ALTER TABLE events ADD COLUMN pid INTEGER`.
- `crates/daemon/src/db/queries.rs` — UPDATE — extend INSERT_EVENT + SELECT_EVENT_BY_ID + SELECT_EVENTS_FOR_SESSION_SINCE + SELECT_EVENT_KINDS_FOR_SESSION column lists.
- `crates/daemon/src/projection/state.rs` — UPDATE — `transition` signature gains `notification_type`, `pid`; PostToolUse → Working; Notification → typed-field rules; new SessionEnded arm; last_pid carry-forward.
- `crates/daemon/src/projection/session.rs` — UPDATE — `write` threads `pid` and `notification_type` through closure, INSERT_EVENT params!, transition() call, Event construction; `rebuild_missing_projections` also threads them.
- `crates/daemon/src/projection/liveness.rs` — NEW — probe_once() + run() loop module.
- `crates/daemon/src/projection/mod.rs` — UPDATE — re-export the new module.
- `crates/daemon/src/main.rs` — UPDATE — move BroadcastHub construction up; insert sync probe_once() after rebuild_missing_projections; spawn periodic liveness task; thread its JoinHandle through graceful shutdown.
- `crates/daemon/src/api/sessions.rs` — UPDATE — populate `SessionListItem.last_pid` from the deserialized state.
- `crates/protocol/tests/contract_protocol.rs` — UPDATE — round-trip + PascalCase / snake_case assertions for new variants; three additive-compat tests.
- `crates/daemon/tests/contract_daemon.rs` — UPDATE — `state_machine_full_sequence_determinism` rewrite; audit all `EventKind::Notification` / `EventKind::PostToolUse` call sites; add probe-related tests; add resume-from-Ended test.
- `crates/adapter-claude/tests/contract_adapter.rs` — UPDATE — pid extraction tests + notification_type extraction tests (seven new test functions).
- `crates/shim/tests/contract_shim.rs` — UPDATE — assert `bowerbird_ppid` is present in shim-framed payload.
- `docs/protocol.md` — UPDATE — additive edits per sprint-change-proposal §4.4.
- `docs/protocol-changelog.md` — UPDATE — six new entries per sprint-change-proposal §4.5.
- `docs/presenter-authoring.md` — UPDATE — new subsections per sprint-change-proposal §4.7.
- `docs/bmad/planning-artifacts/architecture.md` — UPDATE — §State-machine narrative + §Singletons & discovery per sprint-change-proposal §4.6.
- `docs/bmad/planning-artifacts/epics.md` — UPDATE — rewrite §Story 5.3 (lines 976–1040) per sprint-change-proposal §4.1.
- `docs/bmad/implementation-artifacts/deferred-work.md` — UPDATE — revise three entries per sprint-change-proposal §4.8.
- `docs/bmad/implementation-artifacts/sprint-status.yaml` — UPDATE — status transitions.

**Files explicitly NOT updated:**
- `docs/bmad/planning-artifacts/prd.md` — no change (Marcus narrative unchanged per sprint-change-proposal §5).
- `crates/adapter-claude/src/install.rs` — no change (hook kinds list is unchanged; Story 5.2 added `UserPromptSubmit`, Story 5.3 doesn't add another).
- `crates/adapter-claude/tests/contract_install.rs` — no change (same reason).
- `crates/daemon/src/broadcast/hub.rs` — no change (the `MIN_CAPACITY = 2` floor still accommodates the Event+State pair pattern; SessionEnded events use the same pattern).
- `crates/daemon/src/api/ws.rs` — no change (`SessionEnded` events broadcast on `events.*` like any hook event; the sentinel filter excluding `__daemon__/__daemon__` doesn't affect them because they carry real source+session_id).

### Testing Standards

Per project-context.md §"Required contract tests" (lines 580–602):

- This story adds entries to that table (none of these are formally added in this PR — defer to Story 5.5's "load-bearing sweep" or the Epic 5 retro):
  - "Daemon-observed liveness probe: SessionEnded emitted for dead/no-PID rows; resume case exits Ended."
  - "Typed-notification WaitingInput rules: three input-required types transition; three transient types preserve prior."
  - "`last_pid` carry-forward + overwrite-on-Some across `transition` and `rebuild_missing_projections`."
- The story preserves Story 1.6 AC #5 ("storage layer is a pure function of the event sequence") — the new fields are stored per-event AND reconstructed during rebuild. Test in Task 6.
- Deterministic test discipline (project-context.md §642-646): NO `sleep()` in the new probe tests. Use `tokio::test(start_paused = true)` + `tokio::time::advance` for the `MissedTickBehavior::Skip` test (Task 9).
- The forward-compat tests (Task 11) follow Story 4.4's pattern: a LOCAL mock enum/struct that lacks the new variants/fields, deserializing a full JSON shape from a Story-5.3+ daemon.

### References

- `docs/decisions/0004-daemon-observed-session-liveness.md` — the ADR. **Read first.** The "Why this is consistent with Axioms 1 and 4" section is the rationale for the daemon-observed approach over the original presenter-side `kill()`.
- `docs/bmad/planning-artifacts/sprint-change-proposal-2026-05-28-daemon-observed-liveness.md` — the canonical spec for THIS story. AC text in §4.1; doc edits in §4.4–4.8.
- `docs/bmad/planning-artifacts/sprint-change-proposal-2026-05-27-pid-liveness.md` — the ORIGINAL Story 5.3 proposal (PID capture + presenter-side `kill()`). Partially superseded by ADR 0004; the `last_pid` capture survives.
- `docs/bmad/planning-artifacts/sprint-change-proposal-2026-05-27-epic-5-resequencing.md` — why this story is 5.3 not 5.8.
- `docs/bmad/planning-artifacts/epics.md:976-1040` — OLD Story 5.3 ACs (Task 14 rewrites in place).
- `docs/bmad/implementation-artifacts/5-2-session-state-projection-correctness.md` — Story 5.2 (done). The `PostToolUse → Working` refinement in AC #9 of THIS story changes one of its rules; the broadcast gating (`prev_raw_current_state` / `prev_read_current_state`) it introduced is inherited unchanged.
- `docs/bmad/implementation-artifacts/5-1-first-party-presenter-tool.md` — Story 5.1 (in-progress). The two dogfooding findings this story closes originated here.
- `docs/bmad/implementation-artifacts/1-6-session-projection-and-hook-unreliability-tolerance.md` — Story 1.6. The original state machine + `STALE_WORKING_MS` fallback. The fallback is RETAINED (ADR 0004 §5) but flagged for retirement in a future story.
- `docs/bmad/implementation-artifacts/4-4-protocol-compatibility-guarantee-and-contract-test-suite.md` — Story 4.4. The `#[serde(other)] Unknown` catch-all that makes AC #17 work.
- `docs/protocol.md` — current protocol surface. Reference for the doc edits in Task 12.
- `docs/protocol-changelog.md` — six new entries land here in Task 12.
- `docs/bmad/project-context.md` — §"Project axioms" (Axiom 1: substrate observes; Axiom 4: mechanical facts in protocol). Read before any close call about whether a new field belongs in the substrate.
- `crates/protocol/src/event.rs` — current `EventEnvelope` + `EventKind` + `Event`. The protocol-changelog gate fires on changes here.
- `crates/protocol/src/state.rs` — current `SessionState` + `SessionCurrentState`.
- `crates/protocol/src/rest.rs:30-45` — `SessionListItem` shape; gains `last_pid`.
- `crates/protocol/src/ws.rs:122-127` — `StateFrame` shape; `state` field gains `last_pid` via `SessionState`.
- `crates/shim/src/main.rs:46-58` — where to inject `bowerbird_ppid`.
- `crates/adapter-claude/src/normalize.rs:52-103` — `normalize` function; extract `bowerbird_ppid` + `notification_type` here.
- `crates/daemon/src/projection/state.rs:36-80` — `transition` function; major rewrite (Task 5).
- `crates/daemon/src/projection/session.rs:46-231` — `write` function; thread new fields (Task 5).
- `crates/daemon/src/projection/session.rs:370-470` — `rebuild_missing_projections`; thread new fields (Task 6).
- `crates/daemon/src/db/migrations.rs` — append migration v2.
- `crates/daemon/src/db/queries.rs:3-31` — INSERT_EVENT + SELECT queries; extend column lists.
- `crates/daemon/src/main.rs:156-212` — startup sequence; insert sync probe + spawn periodic task.
- `crates/daemon/tests/contract_daemon.rs` — extensive audit + new tests.
- `tests/protocol_changelog_gate.rs` — the gate. Six new changelog entries satisfy it.

## Dev Agent Record

### Agent Model Used

claude-opus-4-7 (1M context)

### Debug Log References

- Initial daemon-on-startup deadlock surfaced via `tests/cli_examples::reconnect_recovery_recovers_after_close_frame_and_resumes`: my first cut of `liveness::probe_once` held a `writer_pool.get()` connection across the per-row `projection::session::write(&writer_pool, ...)` calls, deadlocking the second daemon startup against the max_size=1 writer pool. Fixed by scoping the read-side checkout in a block so it drops before the loop runs (`crates/daemon/src/projection/liveness.rs::probe_once`).
- The workspace lint `unsafe_code = "forbid"` was incompatible with the shim's `libc::getppid()` call (`#[allow(unsafe_code)]` cannot override `forbid`). Per project-context.md §"Crate-wide invariants" the spec calls for `deny`, not `forbid` — implementation was stricter than spec. Downgraded to `deny` and applied a single inline `#[allow(unsafe_code)]` on the `getppid` line.
- The connection-factory lint test (`tests/contract_daemon::connection_factory_policy_lint_passes`) flagged the new `rusqlite::Connection::open_in_memory()` call in the migration v2 idempotency test. Refined the lint substring from `Connection::open` to `Connection::open(` (with open paren) so `open_in_memory()` passes — the lint's intent is to block file-backed connection opens that bypass the PRAGMA factory; in-memory test DBs are fine.

### Completion Notes List

- **Story complete: 19 ACs satisfied, 18 tasks executed.** Daemon-observed liveness via 5-second `kill(pid, 0)` probe + synchronous startup probe before WS bind; typed-`notification_type`-driven `WaitingInput`; `PostToolUse → Working` unconditionally; `Ended` state non-terminal (a `claude --resume` transitions back out).
- **Workspace tests green serialized: zero failures** across the full suite (`cargo test --workspace -- --test-threads=1`). 142 daemon contract tests pass; 27 protocol contract tests pass; 17 shim contract tests pass; 24 adapter contract tests pass. Six new `story_5_3_liveness::*` tests cover the probe behavior end-to-end.
- **`cargo fmt --check` clean, `cargo clippy --all-targets --workspace -- -D warnings` clean.**
- **Protocol-changelog gate satisfied:** six new `+`-prefixed entries (three `type: schema`, three `type: behavioral`) cover every protocol-crate touch.
- **Hermetic end-to-end smoke validated:** shim injects `bowerbird_ppid=<shell pid>`; adapter normalize extracts it; projection writes `last_pid=85457` to `session_projections`; notification with `permission_prompt` transitions to `WaitingInput`; daemon restart preserves state and migration v2 applied cleanly (`PRAGMA user_version = 2`). The dead-pid probe path is covered by contract test `liveness_probe_emits_session_ended_for_dead_pid` (spawn-and-reap pattern).
- **Deferred to follow-up:** parent-walk past wrappers, PID-recycle resilience via `started_at`, probe cost telemetry, cross-machine probe semantics, typed-notification on the wire StateFrame (the last is explicitly rejected per maintainer design conversation — NOT scope creep). Captured in `docs/bmad/implementation-artifacts/deferred-work.md` §"Deferred from: Story 5.3."
- **NOT touched:** `crates/adapter-claude/src/install.rs` (hook list unchanged from Story 5.2), `crates/daemon/src/broadcast/hub.rs` production code (test helpers only), `crates/daemon/src/api/ws.rs` (sentinel filter unaffected — SessionEnded carries real source+session_id), PRD (Marcus narrative unchanged per sprint-change-proposal §5).

### File List

**Source files modified:**

- `Cargo.toml` — workspace lint `unsafe_code` from `"forbid"` to `"deny"`; enables shim's inline `#[allow(unsafe_code)]` for `libc::getppid()`.
- `crates/protocol/src/event.rs` — `NotificationType` enum added; `EventKind::SessionEnded` added; `EventEnvelope` gains `pid` + `notification_type`; stored `Event` gains `pid`.
- `crates/protocol/src/state.rs` — `SessionCurrentState::Ended` added; `SessionState` gains `last_pid`.
- `crates/protocol/src/rest.rs` — `SessionListItem` gains `last_pid` for parity with `SessionDetail`.
- `crates/protocol/src/lib.rs` — re-exports `NotificationType`.
- `crates/shim/Cargo.toml` — adds workspace `libc` dep.
- `crates/shim/src/main.rs` — injects `bowerbird_ppid` via `libc::getppid()` (single `#[allow(unsafe_code)]` block).
- `crates/adapter-claude/src/normalize.rs` — extracts `bowerbird_ppid` → `EventEnvelope.pid`; extracts `notification_type` → `EventEnvelope.notification_type` (Notification kind only).
- `crates/daemon/Cargo.toml` — adds workspace `libc` dep.
- `crates/daemon/src/db/migrations.rs` — migration v2: `ALTER TABLE events ADD COLUMN pid INTEGER`; idempotency unit test.
- `crates/daemon/src/db/queries.rs` — `INSERT_EVENT` extended with `pid`; `SELECT_EVENT_BY_ID`, `SELECT_EVENTS_FOR_SESSION_SINCE` extended with `pid`; `SELECT_EVENT_KINDS_FOR_SESSION` extended with `pid + payload` for rebuild path.
- `crates/daemon/src/projection/state.rs` — `transition()` signature gains `notification_type` and `pid`; `PostToolUse → Working` unconditional; `Notification` branches on typed `notification_type`; `SessionEnded → Ended`; `last_pid` carry-forward applied to every arm.
- `crates/daemon/src/projection/session.rs` — `write` threads `pid` + `notification_type` through closure, INSERT params, transition call, Event construction; sentinel writers pass `None::<u32>` for pid; `rebuild_missing_projections` reads pid + payload and re-parses notification_type during rebuild.
- `crates/daemon/src/projection/mod.rs` — re-exports the new `liveness` module.
- `crates/daemon/src/main.rs` — broadcaster construction moved up; synchronous `liveness::probe_once` after `rebuild_missing_projections` and BEFORE `tokio::net::TcpListener::bind`; periodic `liveness::run` task spawned; shutdown wait list extended.
- `crates/daemon/src/api/sessions.rs` — populates `SessionListItem.last_pid` and `SessionDetail.state.last_pid` from the deserialized state.
- `crates/daemon/src/api/events.rs` — extended `EventRow` tuple and `Event` construction to thread `pid`.
- `crates/daemon/src/api/replay.rs` — replayed envelopes carry `pid` forward and re-parse `notification_type` from payload.
- `crates/daemon/src/broadcast/event.rs` — test helpers updated for new fields (`#[cfg(test)]` only).
- `crates/daemon/src/broadcast/hub.rs` — test helpers updated for new fields (`#[cfg(test)]` only).
- `crates/daemon/src/projection/snapshot.rs` — test helpers updated for new fields (`#[cfg(test)]` only).

**Source files created:**

- `crates/daemon/src/projection/liveness.rs` — new module: `is_pid_alive(pid)` helper via `libc::kill(pid, 0)` with `EPERM = alive` defensive treatment; `probe_once(writer_pool, broadcaster) -> Result<usize>` per-iteration scan; `run(writer_pool, broadcaster, shutdown)` long-running loop with `tokio::time::interval(5s)` + `MissedTickBehavior::Skip`. Critical: read-side connection scoped in a block so it drops before per-row `write()` calls (avoids writer-pool deadlock).

**Test files modified:**

- `crates/protocol/tests/contract_protocol.rs` — round-trip tests for `SessionEnded` / `Ended` / six known `NotificationType` variants; three additive-compat tests proving v1.0 presenters decode the new variants as `Unknown` via `#[serde(other)]` catch-all; two field-additive-compat tests proving `last_pid` and `pid` are silently dropped by legacy decoders.
- `crates/daemon/tests/contract_daemon.rs` — bulk patch of every `EventEnvelope` constructor with `pid: None, notification_type: None`; `state_machine_full_sequence_determinism` rewritten to use the new `envelope_for_notification` helper for typed notifications; `connection_factory_policy_lint_passes` lint refined for `open(` specificity; six new `story_5_3_liveness::*` tests covering probe behavior (no_pid_at_upgrade, dead pid, alive pid, already-Ended skip, resume from Ended, rebuild preserves last_pid and Ended).
- `crates/adapter-claude/tests/contract_adapter.rs` — seven new tests covering pid extraction (set, missing, non-integer, negative) and notification_type extraction (six known values, future-value-as-Unknown, missing, non-Notification kinds).
- `crates/shim/tests/contract_shim.rs` — `shim_injects_bowerbird_ppid_into_payload` asserts `bowerbird_ppid` is the test runner's PID; `shim_preserves_notification_type_field_verbatim` asserts the shim preserves `notification_type` without extracting it (adapter's job).

**Doc files modified:**

- `docs/protocol.md` — `/sessions`, `/sessions/{id}`, `EventFrame`, `StateFrame` examples gain `last_pid` / `pid`; `SessionCurrentState` and `EventKind` tables list new variants; `SessionEnded` payload shape documented; ingest socket contract gains `bowerbird_ppid` injection + `notification_type` extraction bullets.
- `docs/protocol-changelog.md` — six new entries under v1.0 → v1.1 (three schema, three behavioral) — all marked `(Resolves: 5.3)`.
- `docs/presenter-authoring.md` — new "Rendering `Ended` sessions" and "Rendering `WaitingInput` sessions" subsections; `state` frame example updated.
- `docs/bmad/planning-artifacts/architecture.md` — FR24–FR26 row updated; NFR coverage updates `unsafe_code` from `forbid` to `deny` with rationale.

**Planning artifacts modified:**

- `docs/bmad/planning-artifacts/epics.md` — §Story 5.3 (lines 976–1040) rewritten in place with the 19 amended ACs from `sprint-change-proposal-2026-05-28-daemon-observed-liveness.md`; title changed from "Session-process liveness via PID capture" to "Daemon-observed session liveness + typed-notification WaitingInput."
- `docs/bmad/implementation-artifacts/deferred-work.md` — new "Deferred from: Story 5.3" section with six follow-up entries (STALE_WORKING_MS retirement, parent-walk, started_at capture, probe cost telemetry, cross-machine probe, typed-notification on wire).
- `docs/bmad/implementation-artifacts/sprint-status.yaml` — Story 5.3 status transitions logged in header comments; entry moved ready-for-dev → in-progress → review (review applied at completion).

### Change Log

- 2026-05-28 — Story 5.3 implemented: daemon-observed session liveness (5s probe + `kill(pid, 0)`) + typed-notification `WaitingInput` (`notification_type`-aware) + `PostToolUse → Working` refinement. Closes two Story 5.1 dogfooding findings against `bowerbird-deck`. Six new protocol-changelog entries; nineteen ACs satisfied; eighteen tasks completed. Workspace tests green serialized; `cargo fmt --check` clean; `cargo clippy -D warnings` clean. Status → review.
