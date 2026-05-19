# Story 1.6: Session Projection and Hook Unreliability Tolerance

Status: ready-for-dev

<!-- Note: Validation is optional. Run validate-create-story for quality check before dev-story. -->

## Story

As a tool builder,
I want the daemon to maintain a consistent `current_state` projection per Claude Code session that stays sane even when hook events are dropped,
so that my tools always show meaningful session state rather than getting stuck due to missing hook delivery.

## Acceptance Criteria

1. **Given** a `PreToolUse` event is ingested for a session but the corresponding `PostToolUse` event never arrives (hook dropped) **When** the projection is queried after a defined timeout window **Then** the session's `current_state` is not permanently stuck in `Working` — it falls through to `Idle` (the sane fallback). The threshold is `STALE_WORKING_MS` (compile-time constant in `crates/daemon/src/projection/state.rs`, default `300_000` ms = 5 minutes). The fallback is computed at read time by a pure function `current_state_for_read(stored_state, now_ms)` so the stored projection row remains a faithful function of the event sequence (AC #5 invariant). See Dev Notes "Hook unreliability mitigation — read-time stale check".

2. **Given** an `INSERT INTO events` and its matching `UPSERT INTO session_projections` are committed in a single transaction **When** SIGKILL is sent to the daemon mid-transaction **Then** on daemon restart, every `session_projections` row has at least one matching `events` row for the same `(source, session_id)`, and every `(source, session_id)` in `events` either has a matching projection row or is reachable via the startup rebuild path of Task 6. No half-state exists (formal state+event atomicity contract test — uses a real spawned daemon subprocess + SIGKILL via `nix::sys::signal::kill`, not the `drop(pool)` surrogate from Story 1.2's `wal_durability_after_simulated_crash`).

3. **Given** two sessions with **identical** `session_id` values but **different** `source` values (e.g. `"claude"` and a hypothetical `"codex"`) **When** events are ingested for both **Then** their projections are stored and queried independently with no cross-contamination — `(source, session_id)` is the natural key throughout. The contract test inserts both, asserts `SELECT * FROM session_projections` returns two distinct rows, and that updating one does not mutate the other.

4. **Given** a sequence of mixed `PreToolUse`, `PostToolUse`, `Stop`, and `Notification` events for a session **When** the projection's stored `state` JSON is read **Then** `current_state` reflects the deterministic state derivable from the event sequence per the state machine in Dev Notes "Session state machine (Story 1.6 v1)". Concretely: `PreToolUse → Working`, `PostToolUse → Idle`, `Stop → Idle`, `Notification → WaitingInput`. `RecordingStarted` and `RecordingEnded` sentinel events do not transition `current_state` for non-sentinel sessions (they only affect the daemon's own `__daemon__/__daemon__` row).

5. **Given** the projection rebuild test: the `session_projections` table is deleted from the SQLite file while the daemon is stopped, then the daemon is restarted **When** the daemon finishes startup **Then** every `(source, session_id)` present in `events` has a `session_projections` row whose `state` JSON is **byte-identical** to what would be produced by ingesting the same event sequence forward through `projection::session::write()`. The event log is the source of truth; the projection is a deterministic derivative.

6. **Given** the `crates/protocol` crate **When** Story 1.6 lands **Then** a new public type `SessionCurrentState` (enum: `Idle`, `Working`, `WaitingInput`) and `SessionState` struct (containing `current_state`, `last_event_kind`, `last_event_at_ms`) are exported. Both implement `Serialize` + `Deserialize`; `SessionCurrentState` uses PascalCase wire strings (matching `EventKind` convention); a snapshot test in `crates/protocol/tests/contract_protocol.rs` asserts the wire string for each variant (`"Idle"`, `"Working"`, `"WaitingInput"`) to guard against silent `rename_all` drift; `protocol-changelog.md` has a new entry with `type: schema`.

## Tasks / Subtasks

- [ ] **Task 1: Add `SessionCurrentState` + `SessionState` to `crates/protocol`** (AC: #6)
  - [ ] Create `crates/protocol/src/state.rs` with:
    - `pub enum SessionCurrentState { Idle, Working, WaitingInput }` deriving `Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize`. Do NOT add `#[serde(rename_all = ...)]` — PascalCase-as-written matches the `EventKind` convention (architecture.md:548-551).
    - `pub struct SessionState { pub current_state: SessionCurrentState, pub last_event_kind: protocol::EventKind, pub last_event_at_ms: i64 }` deriving `Debug, Clone, PartialEq, Eq, Serialize, Deserialize`. This is an **outbound** type (daemon → tool) so do NOT add `#[serde(deny_unknown_fields)]` — additive forward-compat per architecture.md:606-608 + `crates/protocol`'s asymmetric serde policy.
  - [ ] Wire the new module: add `mod state;` to `crates/protocol/src/lib.rs` and `pub use state::{SessionCurrentState, SessionState};` to the re-export block (mirroring the existing pattern at `lib.rs:9-17`).
  - [ ] Add a snapshot test to `crates/protocol/tests/contract_protocol.rs` covering all three variants: `assert_eq!(serde_json::to_string(&SessionCurrentState::Idle).unwrap(), "\"Idle\"")`, same for `Working` and `WaitingInput`. This is the canonical guard per architecture.md:711-713 against `rename_all` drift on a new enum.
  - [ ] Add a round-trip snapshot test for `SessionState` covering: serialize a fully-populated struct to JSON, deserialize it back, assert equality. Then deserialize a JSON blob with an **extra unknown field** (`{"current_state":"Idle","last_event_kind":"PreToolUse","last_event_at_ms":1234,"future_field":"ignored"}`) and assert it parses without error — this is the canary for the asymmetric `deny_unknown_fields` policy on the outbound surface.
  - [ ] Add a `docs/protocol-changelog.md` entry under a new `## v1.0 → v1.1` section (or whichever version this lands in; check existing changelog for current header) with `type: schema` and a one-line description: "Added `SessionCurrentState` enum and `SessionState` struct for per-session current-state projection (Story 1.6, FR25)." If `docs/protocol-changelog.md` does not exist yet, create it — the CI gate from Story 4.4 has not landed yet, so a missing changelog is not a build failure today, but the convention is established. The changelog header lives at the top of the file: `# protocol-changelog`.
  - [ ] **Do NOT** add `chrono`, `time`, or any new dep. `i64` Unix millis is the project-wide timestamp convention (architecture.md:593-594).

- [ ] **Task 2: Implement `crates/daemon/src/projection/state.rs`** (AC: #1, #4)
  - [ ] Create the new file. Add `pub mod state;` to `crates/daemon/src/projection/mod.rs`.
  - [ ] Define `STALE_WORKING_MS: i64 = 300_000;` as a module-level `pub(crate) const`. This is the 5-minute fallback window. Keep it as a constant — runtime-configurable thresholds are an explicit out-of-scope (see "Out of scope" in Dev Notes).
  - [ ] Implement `pub(crate) fn transition(prev: Option<&SessionState>, event_kind: EventKind, now_ms: i64) -> SessionState`:
    - **Pure function.** Takes the previous stored state (or `None` for the first event), the incoming event kind, and the wall-clock timestamp; returns the new state to store.
    - Transition table (the substrate's single projection rule):
      | event_kind | next current_state |
      |---|---|
      | `PreToolUse` | `Working` |
      | `PostToolUse` | `Idle` |
      | `Stop` | `Idle` |
      | `Notification` | `WaitingInput` |
      | `RecordingStarted` | (caller skips — sentinel) |
      | `RecordingEnded` | (caller skips — sentinel) |
    - `last_event_kind` and `last_event_at_ms` always reflect the latest event.
    - **Sentinel handling:** if `event_kind` is `RecordingStarted` or `RecordingEnded`, the caller (`projection::session::write_recording_started` / `write_recording_ended`) does NOT call `transition` — those write to the `__daemon__/__daemon__` sentinel row with `EMPTY_PAYLOAD` and do not have a meaningful `current_state`. Defensive guard: if `transition` is somehow called with a sentinel kind, return `prev` unchanged (or default to `Idle` if `prev` is `None`). Document this is a defensive guard, not an expected code path.
  - [ ] Implement `pub fn current_state_for_read(stored: &SessionState, now_ms: i64) -> SessionCurrentState`:
    - **Pure function.** Takes the stored state and the read-time wall-clock; returns the state to surface to the caller.
    - Rule: if `stored.current_state == Working` AND `now_ms - stored.last_event_at_ms > STALE_WORKING_MS`, return `Idle`. Otherwise return `stored.current_state` verbatim.
    - **Do NOT** mutate the stored row at read time. The stale check is a view-function, not a write. Rationale: keeps AC #5 (rebuild byte-identical) clean — the stored projection is a pure function of the event sequence; the staleness is a presenter-facing surface concern.
  - [ ] Add unit tests at the bottom of the file (`#[cfg(test)] mod tests { ... }`):
    - `transition_first_event_pretooluse_yields_working`
    - `transition_pretooluse_then_posttooluse_yields_idle`
    - `transition_notification_yields_waiting_input`
    - `transition_stop_clears_working`
    - `transition_pretooluse_without_posttooluse_keeps_working` (the storage-level state stays Working — the fallback is at read time, not write)
    - `current_state_for_read_returns_working_when_fresh`
    - `current_state_for_read_returns_idle_when_stale` (Working + `now - last_event_at = STALE_WORKING_MS + 1` → Idle)
    - `current_state_for_read_returns_idle_at_exactly_threshold` (boundary: `now - last_event_at = STALE_WORKING_MS` → Working still; strict-greater-than). Document the boundary choice in a one-line comment.
    - `current_state_for_read_does_not_stale_idle` (Idle stays Idle regardless of age — only Working has a stale fallback)
  - [ ] **Do NOT** call `SystemTime::now()` inside `transition` or `current_state_for_read`. Wall-clock is an *input* — pass it as `now_ms`. Lets tests use fixed timestamps without faking time (deterministic-test-discipline per project-context line 642).

- [ ] **Task 3: Wire `transition` into `projection::session::write`** (AC: #2, #3, #4)
  - [ ] Modify `crates/daemon/src/projection/session.rs::write`:
    - Inside the `interact` closure, BEFORE issuing the `UPSERT_SESSION_PROJECTION` and `INSERT_EVENT` statements, perform a read inside the transaction: `SELECT state FROM session_projections WHERE source = ? AND session_id = ?`. Use a new query constant `SELECT_SESSION_PROJECTION_STATE` in `crates/daemon/src/db/queries.rs`.
    - If the row exists, deserialize the `state` TEXT column via `serde_json::from_str::<SessionState>()`. If deserialization fails, log at `error` level and treat as `None` (gracefully tolerate a corrupted state row; the next event will overwrite it).
    - Compute the new state: `let new_state = projection::state::transition(prev.as_ref(), envelope.kind.clone(), now_ms);`
    - Serialize the new state via `serde_json::to_string(&new_state)` (replaces the current `EMPTY_PAYLOAD.to_string()` placeholder at line 45).
    - Pass the serialized JSON to the existing `UPSERT_SESSION_PROJECTION` execute call (the SQL stays the same — only the `state_json` value changes from `"{}"` to the real state).
    - **The two existing statements (UPSERT + INSERT) remain the only writes in the transaction.** The new `SELECT` is a read; it joins the transaction implicitly but does not break the "exactly these two operations" invariant from architecture.md:634-641 (the rule is about *writes*; read-modify-write is the standard projection pattern).
    - **Sentinel skip:** if `envelope.kind` is `RecordingStarted` or `RecordingEnded`, do NOT call `transition` — that codepath is `write_recording_started` / `write_recording_ended`, which write to the `__daemon__/__daemon__` row and do not represent meaningful session state. The `write()` function only receives normalized adapter events, never sentinels (the ingest path doesn't produce sentinels). Add an `assert!(!matches!(envelope.kind, EventKind::RecordingStarted | EventKind::RecordingEnded))` inside `#[cfg(debug_assertions)]` to catch future misuse.
  - [ ] Update the `tracing::instrument` skip list as needed; do NOT log the `state_json` (it's small but per project-context observability axiom keep span fields to `source`, `session_id`, `event_id` — line 499).
  - [ ] **Preserve atomicity.** The whole sequence (SELECT existing state → compute new state → UPSERT projection → INSERT event) must stay inside a single `interact` closure with a single `tx.commit()`. If a panic happens mid-closure, rusqlite's drop impl rolls back the transaction.

- [ ] **Task 4: Update `write_recording_started` / `write_recording_ended` if needed** (AC: #2)
  - [ ] Read both functions in `crates/daemon/src/projection/session.rs` (lines 70-185).
  - [ ] Currently they UPSERT the `__daemon__/__daemon__` projection row with `state_json = "{}"`. Leave that behavior unchanged — sentinels have no meaningful `current_state`, and the `__daemon__/__daemon__` row is excluded from `GET /sessions` (in Story 1.7) by a `WHERE source != '__daemon__'` filter. Add a brief comment to that effect at the top of each function explaining why the projection state for sentinels is intentionally `{}`.
  - [ ] **Do NOT** call `projection::state::transition` from these functions — they don't represent normal session activity.

- [ ] **Task 5: Add `SELECT_SESSION_PROJECTION_STATE` query constant** (AC: #2, #3, #4)
  - [ ] In `crates/daemon/src/db/queries.rs`, add:
    ```rust
    pub const SELECT_SESSION_PROJECTION_STATE: &str =
        "SELECT state FROM session_projections WHERE source = ? AND session_id = ?";
    ```
  - [ ] No new schema migration is needed — the `state` column already exists (Story 1.2's `V1_UP` at `crates/daemon/src/db/migrations.rs:5-29`). The `session_projections.state` column is `TEXT NOT NULL`; we are switching it from the `"{}"` placeholder to a real JSON blob.
  - [ ] All SQL goes through `queries.rs` per architecture.md:798 ("ALL SQL strings live here; no inline SQL elsewhere"). The new SELECT is no exception.

- [ ] **Task 6: Implement projection rebuild on startup** (AC: #5)
  - [ ] Add `pub async fn rebuild_missing_projections(writer_pool: &deadpool_sqlite::Pool) -> Result<usize>` to `crates/daemon/src/projection/session.rs` (returns the count of rebuilt projections; logs each at `info` level).
  - [ ] Implementation:
    1. Inside an `interact` closure with a single transaction:
    2. Query distinct `(source, session_id)` from `events` **excluding** the daemon sentinel pair: `SELECT DISTINCT source, session_id FROM events WHERE source != '__daemon__'`. Add this as `SELECT_DISTINCT_SESSIONS_FROM_EVENTS` in `queries.rs`.
    3. For each `(source, session_id)`, check if it exists in `session_projections` (single SELECT per pair). Use `SELECT_SESSION_PROJECTION_STATE` from Task 5.
    4. If missing: replay events for that pair in ascending `event_id` order via a new query `SELECT_EVENT_KINDS_FOR_SESSION` (`SELECT kind, created_at FROM events WHERE source = ? AND session_id = ? ORDER BY event_id ASC`). Apply `projection::state::transition` fold-style starting from `None`. The final `SessionState` is what gets UPSERTed.
    5. Use the existing `UPSERT_SESSION_PROJECTION` to write the rebuilt state with `updated_at = max(created_at)` from the replayed events.
    6. Commit the transaction. Returning early on any per-session error logs the issue but does not abort the entire rebuild — projection is best-effort; one bad session shouldn't lock the daemon out.
  - [ ] Wire it into `crates/daemon/src/main.rs::run`: call `rebuild_missing_projections(&pools.writer).await` AFTER `run_migrations` succeeds and BEFORE `projection::session::write_recording_started`. Place between lines 78-81. If rebuild returns an error, log it but continue startup — see error-tolerance rationale below.
  - [ ] **Error handling philosophy:** rebuild is a "make-best-effort to converge stored projections with the event log on startup." A rebuild failure is a data-correctness *warning* (the daemon may surface stale-or-missing state for sessions whose projections were never written), not a startup *blocker*. The daemon must still come up to serve REST/WS for sessions whose projections are intact.
  - [ ] Add `event_kind_from_db_str` if needed: the reverse of `event_kind_as_str` for parsing back from the events.kind TEXT column. Implement it in `crates/daemon/src/db/queries.rs` next to `event_kind_as_str` (`pub fn event_kind_from_db_str(s: &str) -> Result<EventKind, String>` returning a parse error on unknown values). Also extend the deferred-work entry "`event_kind_as_str` ↔ serde equivalence untested" — Task 6 introduces the inverse, so add an exhaustive round-trip test: for every `EventKind` variant, assert `event_kind_from_db_str(event_kind_as_str(k)) == Ok(k)`.

- [ ] **Task 7: Contract test — state+event atomicity under SIGKILL (proper subprocess test)** (AC: #2)
  - [ ] Add `crates/daemon/tests/contract_daemon.rs::state_plus_event_atomicity_under_sigkill`. **This supersedes the `drop(pool)` surrogate in `wal_durability_after_simulated_crash`** (line 62-115). Keep that test — it still exercises a useful path — but add the real SIGKILL test alongside.
  - [ ] Strategy:
    1. Use `assert_cmd::Command::cargo_bin("bowerbird-daemon")` to spawn the real daemon binary with `HOME=<temp>` and `BOWERBIRD_INGEST_SOCK=<temp/ingest.sock>` (add this env override to `main.rs` if not yet present; the shim already uses an analogous pattern).
    2. Wait for `~/.bowerbird/ingest.sock` to appear (poll with a 5s budget; if it never appears, fail the test). Do **not** sleep — use `tokio::time::timeout` + a polling loop with `tokio::time::sleep(Duration::from_millis(10))` granularity. This is the one approved use of polling in tests; document it explicitly because project-context line 642 forbids "real sleep() for synchronization" in tests — the alternative here is a non-deterministic race on socket creation.
    3. Open a Unix socket connection to the daemon's ingest socket. Send a single valid NDJ event (`{"session_id":"sess-x","tool_name":"Bash","hook_kind":"PreToolUse"}\n`). Read the `200\n` ACK.
    4. **Immediately** SIGKILL the daemon process. Use `nix::sys::signal::kill(Pid::from_raw(child.id() as i32), Signal::SIGKILL)`. `nix` is already in the workspace via `keyring`'s transitive deps — verify with `cargo tree | grep nix`; if absent, add `nix = { version = "0.30", default-features = false, features = ["signal"] }` to `[dev-dependencies]` in `crates/daemon/Cargo.toml`. Pin version in workspace deps.
    5. Wait for the child to exit (`child.wait()`).
    6. Reopen the SQLite database via a fresh `init_pools` against the same path.
    7. Run `rebuild_missing_projections` (Task 6) so any post-kill rebuild fires.
    8. Assert: every `(source, session_id)` in `events` has a matching `session_projections` row. Use a single JOIN query: `SELECT COUNT(*) FROM (SELECT DISTINCT source, session_id FROM events WHERE source != '__daemon__') e LEFT JOIN session_projections p USING (source, session_id) WHERE p.source IS NULL` — assert this returns 0.
    9. Assert: the `SessionState` JSON for `("claude", "sess-x")` deserializes cleanly and has `current_state == Working` (the PreToolUse landed).
  - [ ] **Why a real subprocess test, not the `drop(pool)` surrogate:** `drop(pool)` exits cleanly through rusqlite's destructor, which runs PRAGMA cleanup. SIGKILL skips destructors entirely — the WAL file is whatever the kernel last flushed. The two test rigs exercise different failure modes; both are valuable. The deferred-work entry from Story 1.2 (line 24-25: "`wal_durability_after_simulated_crash` uses `drop(pool)` not a true subprocess crash") names this specifically. After this story lands, **strike that entry in `docs/bmad/implementation-artifacts/deferred-work.md`** with a backlink to the Story 1.6 commit.

- [ ] **Task 8: Contract test — (source, session_id) collision safety** (AC: #3)
  - [ ] Add `crates/daemon/tests/contract_daemon.rs::source_session_id_collision_safety`.
  - [ ] Strategy:
    1. `fresh_pools()` → fresh DB.
    2. Construct two envelopes with `session_id == "sess-shared"` and `source == "claude"` vs `source == "codex"`.
    3. Write both via `projection::session::write`.
    4. Assert two distinct rows in `session_projections` (one per `(source, session_id)` natural key).
    5. Write a second event for `("claude", "sess-shared")` (e.g. `PostToolUse`) and assert it updates only the claude row; the codex row's `state` and `updated_at` are unchanged. Read both `state` JSON blobs and assert the difference.
    6. Assert event rows in `events` are also segregated by source: `SELECT COUNT(*) FROM events WHERE source = 'claude' AND session_id = 'sess-shared'` = 2; same query with `source = 'codex'` = 1.

- [ ] **Task 9: Contract test — hook unreliability tolerance** (AC: #1, #4)
  - [ ] Add `crates/daemon/tests/contract_daemon.rs::hook_unreliability_tolerance_pretooluse_without_posttooluse`. **Mirrors** the project-context line 593 contract: "Fire `PreToolUse` without a matching `PostToolUse`; assert projection still reaches a sane state (not stuck in `working`)."
  - [ ] Strategy:
    1. `fresh_pools()`.
    2. Write a `PreToolUse` envelope.
    3. Read the stored `SessionState` from `session_projections` and parse JSON. Assert `current_state == Working`, `last_event_kind == PreToolUse`.
    4. Compute `now_ms_late = stored.last_event_at_ms + STALE_WORKING_MS + 1`. Call `projection::state::current_state_for_read(&stored, now_ms_late)`. Assert result is `Idle`.
    5. Also write a `Stop` event (separate sub-case, same test): write `PreToolUse` then `Stop`. Re-read stored state. Assert `current_state == Idle` even WITHOUT staleness fallback — the Stop hook arrived and naturally cleared the Working state.
    6. Assert: the stored state row for the post-Stop case has `current_state: "Idle"` in JSON, byte-for-byte (use a literal string compare so a serde change in `SessionCurrentState` would surface).

- [ ] **Task 10: Contract test — full event-sequence state machine determinism** (AC: #4)
  - [ ] Add `crates/daemon/tests/contract_daemon.rs::state_machine_full_sequence_determinism`.
  - [ ] Strategy: drive a single `(source, session_id)` through `[PreToolUse, PostToolUse, PreToolUse, Notification, PreToolUse, Stop]` and assert the stored `current_state` after each event:
    - After `PreToolUse #1` → `Working`
    - After `PostToolUse` → `Idle`
    - After `PreToolUse #2` → `Working`
    - After `Notification` → `WaitingInput`
    - After `PreToolUse #3` → `Working`
    - After `Stop` → `Idle`
  - [ ] Assert `last_event_kind` always matches the most recent event.
  - [ ] **Determinism property:** consider adding a `proptest` round-trip later if useful (out of scope for this story; project-context line 644 mentions `proptest` for projection determinism — defer to a future hardening pass).

- [ ] **Task 11: Contract test — projection rebuild from event log** (AC: #5)
  - [ ] Add `crates/daemon/tests/contract_daemon.rs::projection_rebuild_from_event_log_is_byte_identical`.
  - [ ] Strategy:
    1. `fresh_pools()`.
    2. Write 5 envelopes for `("claude", "sess-A")` and 3 for `("claude", "sess-B")` via `projection::session::write`.
    3. Read both `state` columns (the "pre-deletion baseline") into local `String`s.
    4. Issue `DELETE FROM session_projections WHERE source != '__daemon__'` — wipe non-sentinel projection rows.
    5. Verify both rows are gone (`SELECT COUNT(*)` = 1, just the sentinel).
    6. Call `rebuild_missing_projections(&pools.writer)`. Assert returned count is 2 (sess-A + sess-B).
    7. Re-read both `state` columns (the "post-rebuild result").
    8. Assert pre-deletion baseline == post-rebuild result **byte-for-byte** for both sessions. This is the "event log is the source of truth" invariant.
  - [ ] **Subtle hazard:** the JSON serialization order of struct fields can affect byte-identity if serde ever changes its emission order. Mitigate by using `serde_json::Value::deep_equal` (or `serde_json::Value` comparison via `==` which sorts by key for objects). Or — preferred — use a literal byte-compare and rely on serde's deterministic field-order emission for struct types. Document the choice in a one-line comment in the test.

- [ ] **Task 12: Sprint hygiene — strike the deferred-work entry from Story 1.2 review** (AC: #2)
  - [ ] Open `docs/bmad/implementation-artifacts/deferred-work.md`.
  - [ ] Find the line: "`wal_durability_after_simulated_crash` uses `drop(pool)` not a true subprocess crash" (line 24 at time of writing).
  - [ ] Strike it (`~~...~~` with a backlink) and add: "**Resolved by Story 1.6 (Task 7):** real subprocess + SIGKILL test in `state_plus_event_atomicity_under_sigkill`." Use the exact same convention as the rest of the file for consistency.
  - [ ] **Do not** strike the line about projection rebuild revisiting on recovery (line 15) — that pointed at Story 1.6's design as the resolution; Story 1.6 implements it. Either strike it with a backlink, or update the entry to acknowledge resolution. Choose strike (delete + backlink) to match the convention used for the `hook_kind` entry that will be struck in Story 1.8.

- [ ] **Task 13: Final checks**
  - [ ] `cargo fmt --check` — green
  - [ ] `cargo clippy --all-targets --workspace -- -D warnings` — green
  - [ ] `cargo test --workspace` — all tests pass. Expected new tests: ~9 protocol-level (state.rs unit + 2 round-trip in contract_protocol.rs) + ~5 daemon contract tests + the strike of the wal_durability surrogate (kept, augmented by SIGKILL).
  - [ ] `cargo build --workspace` — zero warnings (workspace lints already enforce this; double-check).
  - [ ] `grep -rn 'EMPTY_PAYLOAD' crates/daemon/src/projection/` — should ONLY appear in `write_recording_started` and `write_recording_ended` (sentinel rows). The `write` function no longer uses `EMPTY_PAYLOAD` because it now writes real state. If grep shows otherwise, fix.
  - [ ] Manually verify the `__daemon__/__daemon__` row in `session_projections` is still excluded from session-listing queries that Story 1.7 will build. This story does not add the API endpoint, but the schema-level filter premise must hold.

## Dev Notes

### Story scope at a glance

This story does ONE thing in three layers:

1. **Protocol layer:** publish a `SessionCurrentState` enum + `SessionState` struct that presenters can use to decode the daemon's per-session state.
2. **Daemon layer:** replace the `"{}"` placeholder in `projection::session::write` with real state computation (a deterministic state machine fed by event_kind), and add a startup rebuild path so the event log remains the source of truth.
3. **Test layer:** ship 3 of the 10 required contract tests (state+event atomicity under real SIGKILL, hook unreliability tolerance, projection rebuild from event log) plus the `(source, session_id)` collision-safety test.

The story does **NOT** ship a REST `GET /sessions/:id` endpoint (Story 1.7) or a WebSocket `state.session.*` topic (Epic 2). Those consume the projection this story produces.

### Session state machine (Story 1.6 v1)

The state machine is intentionally tiny and pure:

| Incoming event_kind | new current_state |
|---|---|
| `PreToolUse` | `Working` |
| `PostToolUse` | `Idle` |
| `Stop` | `Idle` |
| `Notification` | `WaitingInput` |
| `RecordingStarted` | (caller skips; sentinel-only) |
| `RecordingEnded` | (caller skips; sentinel-only) |

**Why these mappings?**

- `PreToolUse → Working`: matches the bowerbird-lamp use case (PRD line 204): yellow when Claude is mid-tool-call.
- `PostToolUse → Idle`: tool call finished; Claude is between calls. Even if Claude immediately fires another `PreToolUse`, the projection briefly reflects Idle in the storage layer — the next event flips it to Working. Presenters see the transition as a single state update because both events land in close succession; if a presenter wants per-tool-call counts they look at the event stream, not the projection.
- `Stop → Idle`: Claude's `Stop` hook fires at the end of an agent turn. It's the clean way to clear a Working state if `PostToolUse` was dropped. This is the *primary* hook-unreliability mitigation; the time-based stale check is the *fallback* mitigation for sessions where `Stop` is also dropped.
- `Notification → WaitingInput`: Claude's `Notification` hook fires when the agent surfaces a notification to the user (permission prompts, "waiting for input", etc — see `docs/research/02-detailed-inventory.md:193`). Mapping it to `WaitingInput` matches the substrate-not-actor axiom: we observe what Claude tells us happened, we don't infer the *reason*.

**Why is this not a violation of the "exactly one normalization" axiom (project-context line 697)?**

Project-context line 697 says: "Exactly one normalization is applied: tool name → reaction enum." The `current_state` projection is **not** a normalization — it's a projection. Normalization (per the same paragraph) is about *replacing* raw payload fields with derived/canonical values. Projection (per FR25) is about *summarizing* a sequence of events into a current-state view. They're orthogonal concerns. The raw event payload remains verbatim in `events.payload`; the projection is a separate column in a separate table.

If a presenter wants finer granularity than `Idle | Working | WaitingInput`, it reads `events.payload` directly. The substrate provides both surfaces.

### Hook unreliability mitigation — read-time stale check

AC #1 requires that a `PreToolUse` without a matching `PostToolUse` (a dropped hook) does not leave `current_state` permanently stuck in `Working`.

**Design choice: write-once, fall-back-at-read-time.**

- On write: `projection::session::write` calls `transition(prev_state, new_event_kind, now_ms)` and stores the result verbatim. If `PreToolUse` arrives, the stored row is `current_state: Working, last_event_at_ms: <wall_clock>`.
- On read: callers must use `projection::state::current_state_for_read(&stored, now_ms)`. If the stored state is `Working` AND `now_ms - stored.last_event_at_ms > STALE_WORKING_MS` (5 min), the returned value is `Idle`. Stored row is unchanged.

**Why not a background sweeper task?**

- A sweeper would add a third tokio task to daemon main, complicating shutdown (the sentinel-event semantics from `recording_sessions` already require careful ordering — see `main.rs:139-149`).
- A sweeper that periodically rewrites projection rows would change the projection's relationship to the event log: the stored row would no longer be a pure function of the events. AC #5 (rebuild byte-identical) would be harder to satisfy — the rebuilt projection would not match a sweeper-updated one.
- The read-time check has zero ongoing daemon overhead and is trivially testable (pure function).

**Why not an event-driven "next event triggers staleness sweep" pattern?**

- Adds complexity to the hot path (every write becomes a multi-row UPDATE).
- Only fires when activity happens — useless for the bowerbird-lamp case where the user has walked away and wants to see Idle.

**Trade-off acknowledged:** the *stored* state is Working forever (until another event arrives for that session), but the *surfaced* state via REST/WS is Idle after 5 min. A presenter that reads the SQLite file directly will see the stale value. This is acceptable because:
- The README explicitly says SQLite schema is internal (PRD line 397) and tools should use the REST/WS surface.
- The TUI/lamp use cases all go through REST or WS, which use `current_state_for_read`.

**STALE_WORKING_MS = 5 minutes** is a compile-time constant. Runtime configurability is explicitly out of scope — see "Out of scope" below.

### State JSON shape (stored in `session_projections.state`)

```json
{
  "current_state": "Working",
  "last_event_kind": "PreToolUse",
  "last_event_at_ms": 1747574400000
}
```

- Fields ordered as in the Rust struct declaration. Serde preserves struct-field order for serialization (this is documented behavior, not coincidence).
- All field names are snake_case (architecture.md:540).
- All variant strings are PascalCase (architecture.md:548-551).
- `last_event_at_ms` is `i64` Unix milliseconds (architecture.md:593-594).

### Atomicity invariant — what stays, what changes

Architecture.md:634-641 names the transaction invariant:

```rust
// Exactly these two operations; nothing else joins this transaction
conn.execute("INSERT INTO session_projections ... ON CONFLICT DO UPDATE ...", ...)?;
conn.execute("INSERT INTO events ...", ...)?;
```

**Story 1.6 changes:**

- The transaction now contains ONE additional read (`SELECT state FROM session_projections WHERE source = ? AND session_id = ?`) BEFORE the two writes. Reads do not break the invariant — the invariant is about *writes*, not the lack of any other SQL. Document this in a comment above the new SELECT.
- The two writes are unchanged in count, in order, and in the SQL they execute. The only change is the value passed for the `state` column in the UPSERT — from `"{}"` to a real JSON blob.
- The transaction commit is unchanged.

If a future refactor tries to lift the SELECT outside the transaction (read-modify-write across two transactions), it would introduce a race where a concurrent writer interleaves between the read and the UPSERT, and the projection could regress. The single-writer pool (max_size=1) makes this less catastrophic in V1, but the in-transaction SELECT is the right invariant — keep it.

### Subprocess SIGKILL test — operational notes

Task 7 requires a real subprocess kill. Notes for the implementer:

- **Spawn pattern:** `assert_cmd::Command::cargo_bin("bowerbird-daemon")` returns a `Command` configured to use the freshly-built daemon binary. Use `.env_clear().env("HOME", tmp)` to isolate from the user's real `~/.bowerbird/` and `.env("BOWERBIRD_INGEST_SOCK", custom_sock)` for socket path control. If `BOWERBIRD_INGEST_SOCK` is not yet honored by the daemon (it currently uses `Config::with_bowerbird_dir(&home/.bowerbird)`), add the env override to `crates/daemon/src/main.rs::run` mirroring the shim's pattern. The override should fall back to the bowerbird-dir default if unset, exactly like the shim.
- **Daemon binding port discovery:** the daemon binds `127.0.0.1:0` (ephemeral port) and logs it at WARN. The test does not need the TCP port — only the ingest UDS path. Listen for socket creation via `tokio::fs::metadata(&sock_path).await.is_ok()` in a poll loop.
- **SIGKILL via nix:** prefer `nix::sys::signal::kill(Pid::from_raw(child.id() as i32), Signal::SIGKILL)` over `Command::kill()` (which sends SIGTERM by default on Unix — wrong signal). `nix` is the canonical crate; check transitive deps via `cargo tree -p bowerbird-daemon` before adding. If absent, add to workspace deps with `default-features = false, features = ["signal"]` to keep dep surface tight.
- **Post-kill assertions:** reopen the database via `init_pools` against the same path. Run `rebuild_missing_projections`. Then assert no orphan event rows (events without matching projection) and no orphan projection rows (projections without matching events) — the LEFT JOIN query from Task 7 covers the first case; the inverse query covers the second if needed.
- **Race-free wait for socket creation:** the project's deterministic-test discipline (project-context line 642) forbids `sleep()` for synchronization, but the alternative for "wait until the daemon has bound its socket" is genuinely a polling problem — there's no sigchld-style signal for "socket bound." Document the bounded poll loop (max 5s, 10ms granularity) as the one approved exception in this test, with a one-line code comment naming the constraint.

### Projection rebuild — operational notes

Task 6 implements `rebuild_missing_projections`. Notes for the implementer:

- **Why startup, not on-demand?** AC #5 requires that deleting the projection table and restarting the daemon converges. Startup rebuild is the simplest path that satisfies that AC without adding a new CLI subcommand (which would belong in Story 3.2).
- **Why "missing only" and not "always rebuild"?** A full rebuild on every startup would scan the entire event log every time — fine at V1 scale (single-developer workload), but unnecessary work. The "rebuild only what's missing" pattern is both AC-compliant and cheap. Future hardening can add `bowerbird repair-projections` (a manual full rebuild CLI subcommand) — track as deferred work.
- **Order matters:** rebuild runs AFTER `run_migrations` (the schema must exist) and BEFORE `write_recording_started` (the rebuild scans the existing event log; the RecordingStarted sentinel is fine to include or exclude, but we exclude `__daemon__/__daemon__` in the distinct-sessions query for clarity).
- **What if `events` is empty?** The distinct-sessions query returns empty; the rebuild loop runs zero iterations; rebuild returns `Ok(0)`. No-op. Correct.
- **What if some sessions have projections and others don't?** Only the missing ones get rebuilt. Existing projections are not touched. This is important because a partial rebuild after a partial crash should converge, not regress.
- **Concurrent ingest during rebuild:** rebuild happens before `ingest::listener::run_bound` is spawned (`main.rs:112-118`), so no concurrent writes can occur. Document this ordering in a comment at the call site.

### Out of scope

These are deliberate non-targets for this story. **Do not implement.**

- **Runtime-configurable STALE_WORKING_MS.** The 5-minute fallback is a compile-time const. If a user finds 5 min wrong, file an issue; we adjust the const or add config.
- **REST/WebSocket surface for sessions.** Story 1.7 ships `GET /sessions/:id` (REST current state) and Epic 2 ships the `state.session.*` WS topic. Story 1.6 only writes the storage layer; nothing reads it via HTTP yet (except contract tests, which read SQLite directly).
- **Notification → richer states.** The research docs (`docs/research/chat.md:265`) note that `ccam` has a `WAITING_INPUT_PATTERN` regex over Notification payloads — that's interpretation, not projection. V1 treats all Notifications as `WaitingInput`. Presenters wanting finer-grained semantics read the raw payload.
- **`bowerbird repair-projections` CLI subcommand.** Manual full-rebuild is post-V1. Track as deferred work after this story lands.
- **Property-based testing (proptest) for projection determinism.** Project-context line 644 suggests it; defer to a hardening pass. Three example sequences in Task 10 are sufficient for the AC.
- **Per-session event statistics in the projection row.** `event_count`, `first_event_at`, `last_event_at` belong in Story 1.7's `GET /sessions/:id/stats` endpoint, computed via aggregate queries — not stored in the projection row.

### Critical Context from Stories 1.1–1.5 (DO NOT REPEAT MISTAKES)

**Dependency pins** — use the workspace dep table at `Cargo.toml`, never invent versions:

| Dep | Actually installed |
|---|---|
| serde | 1.0.228 |
| serde_json | 1.0.149 |
| thiserror | 2.0.18 |
| tempfile | 3.20.0 |
| assert_cmd | 2.0.17 |
| rusqlite | 0.39.x (bundled) |
| deadpool-sqlite | 0.13.x |
| rusqlite_migration | 2.5.x |
| tokio (current_thread) | as pinned in workspace |

**Workspace lints**: every crate has `[lints] workspace = true` and the workspace has `unsafe_code = "forbid"`. **Do NOT** add `#![deny(unsafe_code)]` or `#![forbid(unsafe_code)]` to any source file — triggers `clippy::duplicated_attributes` as a hard error (Story 1.4 review finding).

**`anyhow` boundary**: permitted only in `main.rs` of binary crates. All daemon-internal modules use `thiserror::Error` types defined in `crates/daemon/src/error.rs`. Story 1.6 adds no new error variants — existing `Error::Pool`, `Error::Sqlite`, `Error::Clock`, `Error::Migration` cover all failure modes.

**No `unwrap()` / `expect()` outside `#[cfg(test)]`**: hard rule, enforced by review (Story 1.4, 1.5 reviews both flagged this). Every Result is mapped to a typed `Error`.

**No `println!` / `eprintln!`**: not just in the shim — anywhere in shipped daemon code. The daemon uses `tracing::*`. Three sanctioned `eprintln!` exceptions exist (verify with grep — they're all in `main.rs` startup-failure paths before tracing is initialized, and in `init_tracing`'s `RUST_LOG` parse-failure path).

**Test fixture patterns**:
- `fresh_pools()` at `crates/daemon/tests/contract_daemon.rs:20-26` is the canonical "give me a clean in-memory-temp-file daemon DB" helper. Use it for every new test.
- `start_ingest_listener` at line 458 is the canonical mock-ingest pattern for tests that need to write to the ingest socket; mirror it if you spawn the daemon binary.
- All test SQLite files live in `tempfile::TempDir` (architecture.md:701).

**SQL discipline**: all SQL strings live in `crates/daemon/src/db/queries.rs`. No inline SQL outside that module (architecture.md:798). Story 1.6 adds three new constants there: `SELECT_SESSION_PROJECTION_STATE`, `SELECT_DISTINCT_SESSIONS_FROM_EVENTS`, `SELECT_EVENT_KINDS_FOR_SESSION`.

**Connection factory rule**: never call `rusqlite::Connection::open` outside `crates/daemon/src/db/pool.rs`. The `scripts/lint-db-access.sh` script enforces this in CI.

**Tracing instrumentation** (architecture.md:661-670): `#[tracing::instrument(skip_all, fields(source, session_id))]` on every async fn crossing a crate boundary. Story 1.6 modifies `projection::session::write` (already instrumented) — keep the existing `skip_all` and `fields(source, session_id)`. Do NOT add `state_json` to the span fields.

**Wire-format snapshot discipline** (architecture.md:711-713): every new wire-format type gets a snapshot assertion in `crates/protocol/tests/contract_protocol.rs`. Story 1.6 adds `SessionCurrentState` and `SessionState` — both need snapshots.

**Additive-only outbound serde** (architecture.md:606-608, 714): no `#[serde(deny_unknown_fields)]` on `SessionState` or any outbound type. The round-trip test in Task 1 covers this canary.

**Single-writer pool**: `pools.writer.max_size = 1` (architecture.md:387). All writes — including the new in-transaction SELECT — go through the writer pool. Do not write to the reader pool. Existing `projection::session::write` already takes `writer_pool: &deadpool_sqlite::Pool`; keep that signature.

**No async sleep for synchronization in tests** (project-context line 642): the one approved exception is Task 7's "wait for ingest socket to appear" poll loop. Document the exception explicitly in a code comment.

### Anti-Patterns To Avoid

- **Reaching for a `chrono` / `time` dep** to compute timestamps. The project uses `SystemTime::now().duration_since(UNIX_EPOCH).as_millis()` already (`projection::session::current_unix_millis` at `session.rs:188-194`). Reuse it. Story 1.5's "ISO8601 without chrono" Dev Notes section explains the rationale (~30-line inline formatter vs ~60 KB dep).
- **Adding a background sweeper task** for stale-Working detection. See "Hook unreliability mitigation" above — rejected design.
- **Mutating the stored projection at read time.** `current_state_for_read` is a pure function. It returns a value; it does not write.
- **Splitting the projection UPSERT and event INSERT across separate transactions.** Architecture.md:720 explicitly forbids this — it's an anti-pattern. The new SELECT joins the same transaction, atop the two writes.
- **Adding fields to `SessionState` that interpret beyond what events provide.** `is_user_attention_needed`, `pending_tool_call_count`, etc. are presenter concerns. The substrate gives raw + minimal projection.
- **`tokio::time::sleep` in tests** for "wait for daemon to be ready." Use polling with `tokio::time::timeout` budget. The deterministic-test discipline is non-negotiable.
- **Using `Command::kill()`** instead of `nix::sys::signal::kill(..., SIGKILL)`. The former sends SIGTERM on Unix, which the daemon catches and gracefully shuts down — exactly the opposite of what the SIGKILL test needs.
- **Adding a new `Result` / `Error` variant** to `crates/daemon/src/error.rs`. The existing variants cover all failure modes for this story; new ones add noise.
- **`#[forbid(unsafe_code)]` / `#[deny(unsafe_code)]`** in source files — workspace already does this; duplicate = `clippy::duplicated_attributes` hard error.
- **Loading or re-deriving the state machine in WS / REST handler code.** When Story 1.7 / Epic 2 land, they call `projection::state::current_state_for_read` — they do not re-implement the transition rules.
- **Rebuilding the projection table on every startup** (instead of "missing only"). Wasteful at scale and risks regressing a working projection if the rebuild is itself buggy. Stick with the "rebuild only what's missing" semantics.
- **Exiting nonzero on rebuild failure.** Rebuild is best-effort; a failure logs an error and continues. The daemon must come up for sessions whose projections are intact.

### ADR-0002 + Story 1.8 Context

[ADR-0002](../../decisions/0002-ingest-wire-framing-and-hook-kind.md) ratified the NDJ wire framing and shim `hook_kind` injection. Story 1.6 is downstream of the ingest path: it works against `EventEnvelope`s that `adapter-claude::normalize` has already produced from raw bytes. No ingest-layer change is needed here.

**Story 1.8 (Tighten daemon `hook_kind` to required)** sits AFTER 1.6/1.7 in the epic-1 sequence (per `sprint-change-proposal-2026-05-18.md` §5). Story 1.8 removes the silent `"PreToolUse"` default at `crates/daemon/src/ingest/handler.rs:53-57`. Story 1.6 does **NOT** depend on 1.8 and does **NOT** modify the ingest handler. If 1.8 lands first by accident, the only impact on 1.6 is that contract tests sending malformed payloads without `hook_kind` would receive `400` instead of a default-`PreToolUse` envelope — fine.

### Project Structure Notes

**Files to be created:**

```
crates/protocol/src/state.rs
  # SessionCurrentState enum + SessionState struct
  # PascalCase wire strings; outbound (permissive) serde

crates/daemon/src/projection/state.rs
  # transition() — pure state-machine function
  # current_state_for_read() — pure read-time stale check
  # STALE_WORKING_MS constant
  # unit tests for all transitions and edge cases

docs/protocol-changelog.md  (if not yet present)
  # type: schema entry for the new types
```

**Files to be modified:**

```
crates/protocol/src/lib.rs
  # add `mod state;` + re-export SessionCurrentState, SessionState

crates/protocol/tests/contract_protocol.rs
  # snapshot tests for SessionCurrentState wire strings
  # round-trip + additive-compat test for SessionState

crates/daemon/src/projection/mod.rs
  # pub mod state;

crates/daemon/src/projection/session.rs
  # write() — replace EMPTY_PAYLOAD placeholder with transition() output
  # add rebuild_missing_projections() function
  # write_recording_started() / write_recording_ended() — add clarifying comments only

crates/daemon/src/db/queries.rs
  # add SELECT_SESSION_PROJECTION_STATE
  # add SELECT_DISTINCT_SESSIONS_FROM_EVENTS
  # add SELECT_EVENT_KINDS_FOR_SESSION
  # add event_kind_from_db_str() helper (reverse of event_kind_as_str)

crates/daemon/src/main.rs
  # call rebuild_missing_projections after run_migrations, before write_recording_started
  # honor BOWERBIRD_INGEST_SOCK env override (if not yet wired) — needed for Task 7's subprocess test

crates/daemon/tests/contract_daemon.rs
  # state_plus_event_atomicity_under_sigkill (Task 7) — supersedes drop(pool) surrogate
  # source_session_id_collision_safety (Task 8)
  # hook_unreliability_tolerance_pretooluse_without_posttooluse (Task 9)
  # state_machine_full_sequence_determinism (Task 10)
  # projection_rebuild_from_event_log_is_byte_identical (Task 11)

crates/daemon/Cargo.toml
  # [dev-dependencies] nix = { workspace = true } if not yet a transitive dep

Cargo.toml (workspace)
  # [workspace.dependencies] nix = { version = "0.30", default-features = false, features = ["signal"] }

docs/bmad/implementation-artifacts/deferred-work.md
  # strike the wal_durability_after_simulated_crash subprocess entry (Task 12)
  # strike the projection-recovery entry from Story 1.2 review (Task 12)
```

**Unchanged** (out of scope per "Out of scope" above):
- `crates/shim/**` — shim doesn't participate in projection
- `crates/adapter-claude/**` — adapter doesn't know about projections; only normalizes
- `crates/daemon/src/ingest/handler.rs` — ingest path is unchanged (Story 1.8 will modify this)
- `crates/daemon/src/api/**` — Story 1.7 adds the REST endpoints that consume the projection

### Source tree alignment with architecture.md:803-805

Architecture lists `crates/daemon/src/projection/` containing `mod.rs` and `session.rs`. Story 1.6 adds `state.rs` as a third file because the state machine is cohesive enough to deserve isolation (pure functions, dedicated unit tests, no DB access). This is a minor additive deviation; document in commit body.

### Git Intelligence (Recent Work Patterns)

Recent commits on `main` show the same feat → review → patches → merge pattern from Stories 1.2 through 1.5:

- `4f10bd0` (PR ?): docs(course-correct): align ingest wire framing with ADR-0002 + add story 1.8
- `369ca17`: ci(story-1.5): per-platform shim bench thresholds (ADR 0003 → Accepted)
- `451dd7a`: docs(adr-0003): record macos-latest shim p99 noise problem (Proposed)
- `51e4a2f`: review(story-1.5): apply code review patches
- `e578a2c`: feat(story-1.5): shim binary with hot-path event delivery
- `59b580a`: feat(story-1.4): Claude Code adapter and event normalization
- `3ebc590`: feat(story-1.3): Unix socket ingest endpoint with NDJ wire protocol
- `ae0ef96`: feat(story-1.2): daemon foundation + SQLite

For this story, expect the dev to land:
1. A `feat(story-1.6): session projection and hook unreliability tolerance` commit
2. A code-review round (Stories 1.1–1.5 each had review rounds; this one is unlikely to be different)
3. A patches commit applying review feedback
4. No new ADRs anticipated — the state-machine design is small enough that the story file is sufficient documentation. If review surfaces a load-bearing decision (e.g. "STALE_WORKING_MS should be runtime-configurable" — currently rejected), that earns an ADR.

### References

- [Source: docs/bmad/planning-artifacts/epics.md#Story-1.6] — original AC text
- [Source: docs/bmad/planning-artifacts/architecture.md#Data-Architecture] — SQLite schema, session_projections table (lines 408-413)
- [Source: docs/bmad/planning-artifacts/architecture.md#Process-Conventions] — transaction invariant (lines 634-641); projection UPSERT pattern (lines 643-648)
- [Source: docs/bmad/planning-artifacts/architecture.md#Architectural-Boundaries] — transaction-boundary ownership in `projection/session.rs` (lines 880-883)
- [Source: docs/bmad/planning-artifacts/architecture.md#Enforcement-Guidelines] — anti-patterns: splitting projection UPSERT and event INSERT (line 720); wire-format snapshot mandate (lines 711-713)
- [Source: docs/bmad/planning-artifacts/architecture.md#Requirements-Coverage] — FR24-FR26 mapped to `projection/session.rs` (lines 964)
- [Source: docs/bmad/planning-artifacts/prd.md#WebSocket-topics] — `state.session.<id>.current_state` topic (line 389); bowerbird-lamp use case (line 204)
- [Source: docs/bmad/planning-artifacts/prd.md#NFR] — NFR22 timestamp column requirement (already in V1 schema)
- [Source: docs/bmad/project-context.md#Substrate-not-actor-invariants] — `(source, session_id)` natural key (line 695); single normalization rule (line 697); hook unreliability acknowledgement (line 702)
- [Source: docs/bmad/project-context.md#Required-contract-tests] — state+event atomicity (line 589); hook unreliability tolerance (line 593); projection rebuild from event log (line 598); (source, session_id) collision safety (line 595)
- [Source: docs/bmad/project-context.md#Deterministic-test-discipline] — no real sleep() (line 642); proptest for projection determinism (line 644); insta snapshots (line 645)
- [Source: docs/bmad/implementation-artifacts/1-5-shim-binary-with-hot-path-event-delivery.md] — Dev Notes pattern; "Critical Context from Stories 1.1–1.4" section format; SIGKILL test caveat from Story 1.2 review (deferred-work line 24-25)
- [Source: docs/bmad/implementation-artifacts/deferred-work.md] — Story 1.2 deferred items resolved by 1.6: SIGKILL surrogate, projection rebuild revisit, optionally: `event_kind_as_str ↔ serde equivalence untested` (covered by `event_kind_from_db_str` round-trip test)
- [Source: docs/decisions/0002-ingest-wire-framing-and-hook-kind.md] — NDJ wire framing + `hook_kind` injection; downstream of Story 1.6's concerns but flagged for context
- [Source: docs/bmad/planning-artifacts/sprint-change-proposal-2026-05-18.md] — sprint order: 1.6 → 1.7 → 1.8 confirmed (§5)
- [Source: crates/daemon/src/projection/session.rs:44] — current `EMPTY_PAYLOAD` placeholder and the inline comment "Story 1.6 populates it" — this story IS the populate
- [Source: crates/daemon/src/db/migrations.rs:5-29] — V1_UP schema; `session_projections` table is already in place, no migration needed
- [Source: crates/daemon/src/db/queries.rs:7-11] — `UPSERT_SESSION_PROJECTION` SQL (reused unchanged)
- [Source: crates/daemon/tests/contract_daemon.rs:62-115] — `wal_durability_after_simulated_crash` (the `drop(pool)` surrogate to be augmented, not replaced, by Task 7's real SIGKILL test)
- [Source: crates/daemon/tests/contract_daemon.rs:125-190] — `state_plus_event_atomicity_rollback` — the rollback-surrogate test; reference for transaction-discipline assertions
- [Source: crates/daemon/tests/contract_daemon.rs:686-...] — `shim_binary_round_trip_to_daemon_ingest` — the canonical "spawn the daemon binary, talk to it via UDS" pattern; mirror in Task 7
- [Source: crates/daemon/src/main.rs:67-173] — daemon `run()` orchestrator; the rebuild call from Task 6 inserts at line ~78 (after `run_migrations`, before `write_recording_started`)
- [Source: crates/protocol/src/event.rs:9-16] — `EventKind` enum; PascalCase wire convention reference for `SessionCurrentState`
- [Source: crates/protocol/src/lib.rs] — module + re-export pattern to mirror for `state` module
- [Source: crates/protocol/tests/contract_protocol.rs] — snapshot test conventions for new wire types

## Change Log

- 2026-05-18: Story created via bmad-create-story workflow. Status `backlog → ready-for-dev`. Six ACs, thirteen tasks, full Dev Notes block; based on epics.md story 1.6 + project-context contract-tests rows + sprint-change-proposal 1.6 sequencing.

## Dev Agent Record

### Agent Model Used

{{agent_model_name_version}}

### Debug Log References

### Completion Notes List

### File List
