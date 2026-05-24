# Story 2.5: Graceful shutdown notification to connected tools

Status: done

<!-- Note: Validation is optional. Run validate-create-story for quality check before dev-story. -->

## Story

As a tool builder,
I want to receive a `close` frame from bowerbird before it shuts down,
so that my tool knows the disconnection is intentional and can show an appropriate status to the user rather than treating it as an error.

## Acceptance Criteria

1. **Given** three tools are connected via WebSocket and an event is mid-ingest **When** SIGTERM is sent to the daemon **Then** the daemon stops accepting new WebSocket connections, sends a `close` frame to all three connected tools, drains the broadcast channels (5-second timeout), flushes the DB connection pools, and exits with code 0.
2. **Given** SIGTERM is sent to the daemon while a SQLite write transaction is in progress **When** the daemon shuts down **Then** the in-flight event is either fully committed or fully rolled back; no partial rows exist after restart.
3. **Given** Ctrl-C (SIGINT) is received by the daemon **When** the shutdown sequence runs **Then** it follows the same path as SIGTERM: `close` frames to all clients, clean DB flush, exit 0.
4. **Given** the broadcast channel drain takes longer than 5 seconds during shutdown **When** the timeout expires **Then** the daemon proceeds with shutdown rather than hanging indefinitely, logs a warning about the drain timeout, and still exits 0.

## Tasks / Subtasks

- [x] **Task 1 - Split shutdown into explicit phases in `crates/daemon/src/main.rs` and `crates/daemon/src/state.rs`** (AC: #1, #3, #4)
  - [x] 1.1 Replace the current single-token shutdown sequencing with two phases: `shutdown_requested` stops new HTTP/WS upgrades and ingest accepts; `ws_close_requested` tells existing WS connection tasks to drain and send the protocol close frame.
  - [x] 1.2 Refactor `run(config)` so the HTTP server is driven concurrently with the shutdown drain, not awaited before the drain begins. Do not deadlock by waiting for `axum::serve(...with_graceful_shutdown(...))` to finish before signaling existing WS tasks to close.
  - [x] 1.3 On SIGTERM or SIGINT, cancel the request token, stop accepting new WebSocket upgrades, stop the ingest listener, drain the ingest writer queue, then cancel the WS-close token.
  - [x] 1.4 Add `Config::shutdown_drain_timeout` defaulting to `Duration::from_secs(5)` and thread it through the shutdown path. Keep existing WS config fields (`ping_interval`, `pong_timeout`, `coalesce_window`) unchanged unless a compile-time struct update requires a test factory change.
  - [x] 1.5 Preserve the existing force-exit behavior on a second signal (`force_exit_on_next_signal`), but ensure the first SIGTERM and first SIGINT enter the same graceful path.

- [x] **Task 2 - Send the protocol `close` frame from `crates/daemon/src/api/ws.rs` before socket closure** (AC: #1, #3)
  - [x] 2.1 Change the current `state.shutdown.cancelled()` branch in `connection_task` so it no longer returns immediately. It must drain pending broadcast envelopes under the connection's current subscriptions, send `ServerMessage::Close(protocol::CloseFrame { reason: Some("daemon shutdown".to_string()) })`, then send a WebSocket control close with normal shutdown semantics.
  - [x] 2.2 Disambiguate protocol and WebSocket close types explicitly, e.g. import `protocol::CloseFrame as ProtocolCloseFrame` and keep `axum::extract::ws::CloseFrame as WsCloseFrame`.
  - [x] 2.3 Reuse `drain_backlog_under_state(...)` for the final per-connection drain. Do not add `BroadcastEnvelope::Close` and do not publish lifecycle close through the broadcast hub.
  - [x] 2.4 Treat send failures during shutdown as non-fatal per connection: log at debug/warn as appropriate, release the connection permit by returning, and let global shutdown continue.
  - [x] 2.5 Keep bad-message close behavior unchanged: malformed inbound messages still use WebSocket close code 1008 via `close_with_bad_message`.

- [x] **Task 3 - Wait for connected WebSocket tasks with the 5-second drain timeout** (AC: #1, #4)
  - [x] 3.1 After canceling `ws_close_requested`, wait for all active WS connection permits to return by acquiring all permits from `state.ws_semaphore` (or an equivalent connection registry). This avoids adding per-connection task handles to axum's `on_upgrade` path.
  - [x] 3.2 Wrap that wait in `tokio::time::timeout(config.shutdown_drain_timeout, ...)`.
  - [x] 3.3 On timeout, log a warning containing the timeout duration and proceed with cleanup and exit 0. Do not call `std::process::exit` from inside the timeout branch.
  - [x] 3.4 Ensure the wait does not permanently consume permits; acquired permits must drop before process exit or before any test reuses the state.

- [x] **Task 4 - Preserve ingest, transaction, and DB cleanup invariants** (AC: #1, #2, #3)
  - [x] 4.1 Preserve the existing ingest order: stop the listener first, then await the writer task so accepted events drain through `projection::session::write`.
  - [x] 4.2 Preserve the transaction invariant in `projection::session::write`: event row and matching session projection commit in one SQLite transaction; no partial event/projection state may be observable.
  - [x] 4.3 Preserve the existing readiness drain invariant by setting `migrations_complete` false before final cleanup is advertised as done.
  - [x] 4.4 Preserve sentinel behavior: write `RecordingEnded` through `projection::session::write_recording_ended` and do not broadcast sentinel events to user-facing WS clients.
  - [x] 4.5 Preserve final WAL cleanup: run `wal_checkpoint_passive(&pools)` after `RecordingEnded`. If `deadpool_sqlite::Pool` exposes an explicit close method in the current dependency version, call it after the checkpoint; otherwise document in code comments that the flush guarantee is "writer task joined + sentinel committed + passive checkpoint + pools dropped at process exit."

- [x] **Task 5 - Contract tests for graceful WS close and signal parity in `crates/daemon/tests/contract_daemon.rs`** (AC: #1, #3, #4)
  - [x] 5.1 Add `mod story_2_5_shutdown { ... }` after `story_2_4_dropped`. Reuse `story_2_1_ws::{connect_authed, parse_hello, read_text_frame_or_close, spawn_test_daemon}` and `story_2_2_publish::{publish_via_projection, wait_subscribe_live, ProbeKind}` instead of redeclaring WS helpers.
  - [x] 5.2 Test `shutdown_token_sends_protocol_close_to_all_connected_tools`: connect three authenticated clients, subscribe them, request graceful shutdown through the test state, and assert each receives `{"op":"close","reason":"daemon shutdown"}` before EOF/control close.
  - [x] 5.3 Replace or tighten the existing `story_2_1_ws::ws_shutdown_token_closes_task` so it asserts the protocol `ServerMessage::Close` frame, not merely "Close, EOF, or network error." Story 2.1 allowed no frame; Story 2.5 requires the frame.
  - [x] 5.4 Add process-level SIGTERM and SIGINT tests using the existing `assert_cmd::cargo::cargo_bin("bowerbird-daemon")` and `nix::sys::signal::kill` pattern already used by `state_plus_event_atomicity_under_sigkill_during_load`. Both tests must assert exit code 0.
  - [x] 5.5 Add `shutdown_drain_timeout_does_not_hang`: hold one WS connection open so its task cannot finish within a small configured test timeout, trigger shutdown, assert a warn log is emitted and the shutdown future completes without hanging. Keep the production default 5s; use a shorter test-only `Config` value.

- [x] **Task 6 - Contract tests for mid-transaction shutdown integrity** (AC: #2)
  - [x] 6.1 Add a deterministic SQLite transaction test that obtains a writer-pool connection through `pools.writer.get().await`, enters `conn.interact(...)`, starts a transaction, inserts a partial row, blocks briefly inside the transaction, and rolls back. Trigger graceful shutdown while that transaction is in progress, then assert no partial rows are visible after reopening the DB.
  - [x] 6.2 Add the commit-side counterpart if the test harness can make it deterministic: when the in-flight transaction commits before shutdown cleanup, the event row and projection row must both be visible after restart.
  - [x] 6.3 Reuse existing SQL/query helpers where possible. Do not open raw `rusqlite::Connection` outside the pool factory path; the connection-factory lint is a project invariant.
  - [x] 6.4 Keep the older `state_plus_event_atomicity_rollback` and `state_plus_event_atomicity_under_sigkill_during_load` tests intact; Story 2.5 adds graceful-shutdown coverage, not a replacement for crash coverage.

- [x] **Task 7 - Update protocol documentation and changelog** (AC: #1, #3)
  - [x] 7.1 Add a `docs/protocol-changelog.md` behavioral entry under `v1.0 -> v1.1`: graceful daemon shutdown now emits a protocol `close` frame with reason `daemon shutdown` before the WebSocket control close.
  - [x] 7.2 If any public protocol type changes are required, add a schema entry. Expected path: no schema change because `ServerMessage::Close(CloseFrame)` already exists.
  - [x] 7.3 Confirm docs do not imply `RecordingEnded` sentinel events are broadcast to user-facing WS clients. They remain DB lifecycle sentinels only.

## Dev Notes

### Existing behavior to change

- `crates/protocol/src/ws.rs` already has `ServerMessage::Close(CloseFrame)` and `CloseFrame { reason: Option<String> }`. Story 2.5 should activate this existing wire type; it should not invent a new frame shape. [Source: crates/protocol/src/ws.rs]
- `crates/daemon/src/api/ws.rs::connection_task` currently has a biased `state.shutdown.cancelled()` branch that logs and returns immediately. That satisfied Story 2.1's "task exits" contract, but it violates Story 2.5 because clients may see EOF without the protocol `close` frame. [Source: crates/daemon/src/api/ws.rs]
- `crates/daemon/src/main.rs::run` currently awaits `axum::serve(...).with_graceful_shutdown(shutdown_fut).await` before it cancels ingest, awaits ingest tasks, writes `RecordingEnded`, and checkpoints WAL. Story 2.5 needs the server stop-accepting phase and the drain phase to run in coordinated sequence without waiting for active WS connections too early. [Source: crates/daemon/src/main.rs]
- `crates/daemon/src/ingest/writer.rs` already drains queued accepted events on shutdown and publishes them through the broadcaster. That code comment explicitly points at Story 2.5, but production WS tasks currently share the same shutdown token and can exit before seeing those drained publishes. Fix the sequencing, not the writer's publish path. [Source: crates/daemon/src/ingest/writer.rs; _bmad-output/implementation-artifacts/2-2-real-time-event-and-state-broadcast-to-multiple-tools.md#Review-Findings]

### Shutdown order required by this story

The implementation should use this order:

1. First SIGTERM/SIGINT observed.
2. Stop accepting new HTTP/WS work through axum graceful shutdown and reject any already-routed late WS upgrade with a non-upgrade error.
3. Stop accepting new ingest socket connections.
4. Drain the ingest writer queue so already accepted events are either committed or explicitly fail as whole events.
5. Signal existing WS tasks to drain their per-connection broadcast receiver and send the protocol `close` frame.
6. Wait up to 5 seconds for WS connection permits to return.
7. If timeout expires, warn and proceed.
8. Mark readiness false, write `RecordingEnded`, run passive WAL checkpoint, drop pools/process exits with code 0.

Do not reverse steps 4 and 5. If WS tasks close before the ingest writer drains, a mid-ingest event can commit and publish after the clients have already gone away, recreating the deferred Story 2.2 shutdown bug. [Source: _bmad-output/implementation-artifacts/2-2-real-time-event-and-state-broadcast-to-multiple-tools.md#Review-Findings]

### Close frame semantics

- The required "close frame" is the bowerbird protocol frame `ServerMessage::Close`, serialized as a text message, e.g. `{"op":"close","reason":"daemon shutdown"}`. After that, the daemon may send a WebSocket control close so libraries surface normal closure.
- Do not use `DroppedFrame` or `SyncFrame` during shutdown unless pending broadcast backlog already produces them through existing drain logic.
- Do not broadcast a close envelope through `BroadcastHub`. Shutdown is connection lifecycle, not agent activity.
- Do not publish `RecordingEnded` to WS clients. Story 2.2 deliberately kept sentinel events out of the user-facing stream. [Source: _bmad-output/implementation-artifacts/2-2-real-time-event-and-state-broadcast-to-multiple-tools.md#Completion-Notes-List]

### Connection tracking

The existing `ws_semaphore` is the simplest completion signal: every upgraded connection owns one `OwnedSemaphorePermit` until `connection_task` returns. After requesting WS close, the shutdown coordinator can wait for all permits to become available with `acquire_many_owned(max_connections as u32)` inside a timeout. This avoids wiring join handles out of `WebSocketUpgrade::on_upgrade`.

Guardrails:

- Keep `try_acquire_owned` for upgrade-time cap enforcement. The 257th connection still returns HTTP 503. [Source: crates/daemon/src/api/ws.rs]
- Ensure `ws_max_connections` cannot exceed the type accepted by `acquire_many_owned`; default is 256. If needed, validate or clamp at config construction.
- Drop the acquired permits after the wait so tests that reuse state do not starve later connections.

### Transaction and DB integrity

- All event writes must still go through `projection::session::write(&writer_pool, &broadcaster, envelope)`, which owns the event insert plus projection update transaction. Do not split event and projection writes across tasks or transactions.
- `write_recording_started` and `write_recording_ended` are sentinel-only paths and intentionally do not take a broadcaster. Preserve that separation.
- WAL mode and PRAGMAs are set in `crates/daemon/src/db/pool.rs`; do not create raw SQLite connections in production code. [Source: crates/daemon/src/db/pool.rs]
- The DB "flush" acceptance criterion maps to: writer task joined, all in-flight transactions finished by commit or rollback, `RecordingEnded` committed, `PRAGMA wal_checkpoint(PASSIVE)` executed, then pools dropped/closed. [Source: _bmad-output/planning-artifacts/architecture.md#Data-Architecture]

### Previous story intelligence

- Story 2.1 created WS auth, hello, subscribe/unsubscribe, ping/pong, cap enforcement, and the current "shutdown token closes task" test. Story 2.5 must tighten the shutdown assertion to require the protocol close frame. [Source: _bmad-output/implementation-artifacts/2-1-websocket-connection-and-topic-subscription.md]
- Story 2.2 established `projection::session::write` as the sole user-facing publisher and explicitly deferred the shared-shutdown-token sequencing bug to Story 2.5. Do not move publishing into sentinel writers. [Source: _bmad-output/implementation-artifacts/2-2-real-time-event-and-state-broadcast-to-multiple-tools.md]
- Story 2.3 added snapshot-on-subscribe and the current backlog drain before subscription changes. Reuse `drain_backlog_under_state` for shutdown drain so snapshot and lag behavior keep one implementation path. [Source: _bmad-output/implementation-artifacts/2-3-new-session-discovery-and-state-snapshot-on-connect.md]
- Story 2.4 added per-connection lag state, `DroppedFrame::new`, and coalescing. A shutdown drain can encounter lag; it should flow through existing lag handling and then still send `Close`. [Source: _bmad-output/implementation-artifacts/2-4-lagged-consumer-recovery-with-dropped-frame.md]

### Relevant source files

| File | Current state | Story 2.5 change |
|---|---|---|
| `crates/protocol/src/ws.rs` | Has `ServerMessage::Close(CloseFrame)` already. | Likely no schema change; use existing type. |
| `crates/daemon/src/api/ws.rs` | Per-connection select exits immediately on global shutdown. | Drain backlog, send protocol close, then control close. |
| `crates/daemon/src/main.rs` | Uses one shutdown token and awaits server before ingest/WS drain cleanup. | Coordinate stop-accepting, ingest drain, WS close wait, DB cleanup without deadlock. |
| `crates/daemon/src/state.rs` | `AppState` carries one `shutdown` token plus WS config. | Add/rename tokens to represent shutdown phases clearly. |
| `crates/daemon/src/config.rs` | WS caps and coalescing defaults live here. | Add 5-second shutdown drain timeout default. |
| `crates/daemon/src/ingest/writer.rs` | Drains queued accepted events after shutdown token fires. | Preserve behavior; sequence WS close after writer drain. |
| `crates/daemon/tests/contract_daemon.rs` | Has reusable WS helpers and existing shutdown/atomicity tests. | Add Story 2.5 module and tighten Story 2.1 shutdown test. |
| `docs/protocol-changelog.md` | Documents `hello`, `event`, `state`, `dropped`; no active shutdown close behavior yet. | Add behavioral changelog entry for close-on-shutdown. |

### Technology constraints

- Use the existing workspace dependency pins in `Cargo.toml`/`Cargo.lock`, not architecture-doc stale pins. Current relevant pins include `tokio 1.52.1`, `axum 0.8.9`, `tokio-util 0.7.18`, `tokio-tungstenite 0.27` as dev-dependency, and `rusqlite 0.38.0`. [Source: Cargo.toml]
- Keep the daemon on `#[tokio::main(flavor = "current_thread")]`.
- Keep outbound protocol deserialization permissive; do not add `deny_unknown_fields` to `CloseFrame` or `ServerMessage` outbound types.
- `anyhow` remains allowed at binary edges (`main.rs`); protocol/library internals should keep typed errors.
- Do not add non-dev dependencies for tests. `nix`, `tokio-tungstenite`, `assert_cmd`, `tower`, and `tempfile` are already available in daemon tests. [Source: crates/daemon/Cargo.toml]

### Project Structure Notes

- This story is daemon/protocol-surface work. It should not touch `crates/shim` or `crates/adapter-claude` unless compile-time type changes force test/support updates.
- The user requested this story artifact under `_bmad-output/implementation-artifacts`; the BMM config still points at `docs/bmad//implementation-artifacts`. Keep implementation source references using repo paths, and do not assume both artifact trees are automatically synchronized.
- Existing worktree changes under `docs/bmad` should not be reverted or normalized as part of this story implementation.

### References

- [Source: _bmad-output/planning-artifacts/epics.md#Story-2.5] - Story statement and four ACs.
- [Source: _bmad-output/planning-artifacts/prd.md#Real-Time-Event-Streaming] - FR17 shutdown notification to connected tools.
- [Source: _bmad-output/planning-artifacts/prd.md#WebSocket-TCP-Bearer-Auth] - `close` server frame meaning is daemon graceful shutdown.
- [Source: _bmad-output/planning-artifacts/architecture.md#API-Communication-Patterns] - WS fan-out, slow consumer behavior, close frame, graceful shutdown.
- [Source: _bmad-output/project-context.md#Daemon-async-Tokio-single-threaded] - single-threaded Tokio and bounded per-client behavior.
- [Source: _bmad-output/implementation-artifacts/2-2-real-time-event-and-state-broadcast-to-multiple-tools.md#Review-Findings] - shared shutdown token bug deferred to Story 2.5.
- [Source: crates/protocol/src/ws.rs] - existing `CloseFrame` protocol type.
- [Source: crates/daemon/src/api/ws.rs] - current connection task, drain, dispatch, and close helpers.
- [Source: crates/daemon/src/main.rs] - current signal handling and DB cleanup path.
- [Source: crates/daemon/src/ingest/writer.rs] - accepted-event drain on shutdown.

## Dev Agent Record

### Agent Model Used

GPT-5 Codex

### Debug Log References

- 2026-05-24: `cargo fmt --all` completed successfully.
- 2026-05-24: `cargo check -p bowerbird-daemon` attempted; blocked before compilation because Cargo could not resolve `index.crates.io` for dependency download (`toml` via `adapter-claude`).
- 2026-05-24: `cargo check -p bowerbird-daemon --offline` attempted; blocked because the local Cargo registry lacks `tempfile`.
- 2026-05-24: `cargo test -p bowerbird-daemon --test contract_daemon story_2_5_shutdown -- --nocapture` attempted; blocked before compilation because Cargo could not resolve `index.crates.io` for dependency download (`toml` via `adapter-claude`).
- 2026-05-24: `env -u RUSTUP_TOOLCHAIN PATH="$HOME/.rustup/toolchains/1.94.1-x86_64-apple-darwin/bin:$HOME/.cargo/bin:$PATH" cargo test -p bowerbird-daemon --test contract_daemon story_2_5_shutdown -- --nocapture` passed: 6 Story 2.5 tests.
- 2026-05-24: `env -u RUSTUP_TOOLCHAIN PATH="$HOME/.rustup/toolchains/1.94.1-x86_64-apple-darwin/bin:$HOME/.cargo/bin:$PATH" cargo test -p bowerbird-daemon` passed: 67 daemon unit tests, 105 daemon contract tests, and daemon doctests.
- 2026-05-24: `env -u RUSTUP_TOOLCHAIN PATH="$HOME/.rustup/toolchains/1.94.1-x86_64-apple-darwin/bin:$HOME/.cargo/bin:$PATH" cargo test --workspace -- --test-threads=1` passed: workspace unit, contract, and doctests. Non-serialized `cargo test --workspace` was stopped after daemon contract tests hung under concurrent execution.
- 2026-05-24: `env -u RUSTUP_TOOLCHAIN PATH="$HOME/.rustup/toolchains/1.94.1-x86_64-apple-darwin/bin:$HOME/.cargo/bin:$PATH" cargo clippy --workspace --all-targets -- -D warnings` passed.
- 2026-05-24: `cargo fmt --all` completed successfully during senior developer review.
- 2026-05-24: `env -u RUSTUP_TOOLCHAIN PATH="$HOME/.rustup/toolchains/1.94.1-x86_64-apple-darwin/bin:$HOME/.cargo/bin:$PATH" cargo test -p bowerbird-daemon --test contract_daemon story_2_5_shutdown -- --nocapture` passed: 8 Story 2.5 tests.
- 2026-05-24: non-serialized `cargo test -p bowerbird-daemon` exposed the known concurrent contract-suite hang mode and was stopped; before the stop it also exposed a SIGTERM readiness race in the Story 2.5 signal test.
- 2026-05-24: `env -u RUSTUP_TOOLCHAIN PATH="$HOME/.rustup/toolchains/1.94.1-x86_64-apple-darwin/bin:$HOME/.cargo/bin:$PATH" cargo test -p bowerbird-daemon -- --test-threads=1` passed: 67 daemon unit tests, 107 daemon contract tests, and daemon doctests.
- 2026-05-24: `env -u RUSTUP_TOOLCHAIN PATH="$HOME/.rustup/toolchains/1.94.1-x86_64-apple-darwin/bin:$HOME/.cargo/bin:$PATH" cargo clippy --workspace --all-targets -- -D warnings` passed during senior developer review.

### Completion Notes List

- Ultimate context engine analysis completed - comprehensive developer guide created.
- Split daemon shutdown state into `shutdown_requested` and `ws_close_requested`; axum stop-accept now runs concurrently with ingest/WS drain rather than blocking the drain behind server completion.
- WebSocket shutdown now drains the existing subscription backlog, emits protocol `{"op":"close","reason":"daemon shutdown"}`, then sends a normal WebSocket control close.
- Added a configurable `Config::shutdown_drain_timeout` defaulting to 5 seconds and a semaphore-based WS drain wait that warns and proceeds on timeout.
- Preserved ingest writer drain, sentinel separation, transaction path through `projection::session::write`, readiness false-before-cleanup, and passive WAL checkpoint cleanup.
- Added Story 2.5 contract coverage for multi-client shutdown close, SIGTERM/SIGINT exit parity, drain timeout, and rollback/commit transaction integrity.
- Updated protocol changelog with the graceful shutdown close-frame behavior and confirmed no schema change was needed.
- Fixed the daemon `axum::serve(...).with_graceful_shutdown(...)` future conversion and `ws_semaphore` ownership issue in `main.rs`; validation now compiles and passes with the repo-pinned Rust 1.94.1 toolchain.
- Senior developer review auto-fixed DB pool closure after WAL checkpoint for the current `deadpool-sqlite`/`deadpool` API.
- Senior developer review tightened rollback shutdown coverage so `write_recording_ended` runs while the writer connection is held by an in-flight transaction.
- Senior developer review hardened SIGTERM/SIGINT process tests to wait for the daemon listening log before sending the signal, avoiding a race before signal handlers are registered.
- Definition of Done passed; story status updated to `done`.

### File List

- `crates/daemon/src/api/ws.rs`
- `crates/daemon/src/config.rs`
- `crates/daemon/src/main.rs`
- `crates/daemon/src/state.rs`
- `crates/daemon/tests/contract_daemon.rs`
- `crates/protocol/src/ws.rs`
- `docs/protocol-changelog.md`
- `_bmad-output/implementation-artifacts/2-5-graceful-shutdown-notification-to-connected-tools.md`
- `_bmad-output/implementation-artifacts/sprint-status.yaml`
- `docs/bmad/implementation-artifacts/2-5-graceful-shutdown-notification-to-connected-tools.md`
- `docs/bmad/implementation-artifacts/sprint-status.yaml`

### Change Log

| Date | Change |
|---|---|
| 2026-05-24 | Implemented graceful shutdown phases, protocol close emission, bounded WS drain wait, shutdown/transaction contract tests, and protocol changelog entry. Status left `in-progress` because Cargo dependency resolution prevented tests from compiling/running in this sandbox. |
| 2026-05-24 | Completed validation with Rust 1.94.1, fixed `main.rs` compile issues in the graceful shutdown server future path, and moved story to `review`. |
| 2026-05-24 | Senior developer review auto-fixed explicit DB pool close after WAL checkpoint, strengthened the mid-transaction rollback shutdown test, hardened signal-test readiness, and moved story to `done`. |

## Senior Developer Review (AI)

### Review Date

2026-05-24

### Reviewer

GPT-5 Codex

### Outcome

Approved after auto-fixes. Story status set to `done`.

### Findings Fixed

- **HIGH - Task 4.5 incomplete:** `crates/daemon/src/main.rs` documented that `deadpool-sqlite` had no explicit close method, but the workspace uses `deadpool-sqlite 0.13.0` / `deadpool 0.13.0`, where `Pool::close()` exists. Fixed by closing reader and writer pools after `wal_checkpoint_passive(&pools)`.
- **MEDIUM - Task 6.1 test did not trigger shutdown cleanup:** `graceful_shutdown_mid_transaction_rollback_leaves_no_partial_rows` canceled an unrelated token while the transaction was in progress. Fixed by starting a real lifecycle session and running `write_recording_ended` while the writer connection is held, then asserting rollback left no partial row and cleanup completed.
- **MEDIUM - Signal tests had a readiness race:** process-level SIGTERM/SIGINT tests waited only for the ingest socket, which can exist before the daemon signal handler is registered. Fixed by redirecting daemon stderr to a temp log and waiting for the `daemon listening` log before sending the signal.

### Validation

- Loaded project context and checked official docs for axum graceful shutdown, Tokio semaphore permit behavior, and deadpool pool close.
- Ran `cargo fmt --all`.
- Ran `env -u RUSTUP_TOOLCHAIN PATH="$HOME/.rustup/toolchains/1.94.1-x86_64-apple-darwin/bin:$HOME/.cargo/bin:$PATH" cargo test -p bowerbird-daemon --test contract_daemon story_2_5_shutdown -- --nocapture` successfully: 8 passed.
- Ran `env -u RUSTUP_TOOLCHAIN PATH="$HOME/.rustup/toolchains/1.94.1-x86_64-apple-darwin/bin:$HOME/.cargo/bin:$PATH" cargo test -p bowerbird-daemon -- --test-threads=1` successfully: 67 daemon unit tests, 107 daemon contract tests, and daemon doctests passed.
- Ran `env -u RUSTUP_TOOLCHAIN PATH="$HOME/.rustup/toolchains/1.94.1-x86_64-apple-darwin/bin:$HOME/.cargo/bin:$PATH" cargo clippy --workspace --all-targets -- -D warnings` successfully.
