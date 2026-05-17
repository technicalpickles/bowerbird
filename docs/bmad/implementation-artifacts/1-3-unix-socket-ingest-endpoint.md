# Story 1.3: Unix Socket Ingest Endpoint

Status: ready-for-dev

## Story

As a tool builder,
I want the daemon to accept events from the shim via a local Unix domain socket with filesystem-level access control,
So that only processes running as my OS user can inject events into bowerbird, with no bearer token overhead on the hot path.

## Acceptance Criteria

1. **Given** a running daemon **When** the ingest socket is created at `~/.bowerbird/ingest.sock` **Then** its file mode is 0600 (accessible only to the owning user)

2. **Given** a well-formed ingest request over the Unix socket **When** the daemon processes it **Then** it returns `200\n` synchronously after accepting the event into the write queue — not after the SQLite commit — and the shim receives the ACK within the 5ms budget

3. **Given** the write queue is at maximum capacity (backpressure condition) **When** the shim sends an ingest request **Then** the daemon returns `503\n`, the shim logs a warning to `~/.bowerbird/shim.log`, and the shim exits 0 (fire-and-forget per NFR5)

4. **Given** the daemon is not running (socket does not exist or ECONNREFUSED) **When** the shim attempts to connect to the ingest socket **Then** the shim logs to `~/.bowerbird/shim.log` and exits non-zero (shim-side behavior — no daemon code needed for this AC; daemon side is simply: socket does not exist)

5. **Given** the ingest socket **When** its listen backlog is checked **Then** it is at minimum 128 (per NFR20)

6. **Given** a malformed or structurally invalid event payload on the ingest socket **When** the daemon attempts to parse it **Then** it returns `400 {reason}\n` with a descriptive reason and does not insert a partial row into the event log

## Tasks / Subtasks

- [ ] **Task 1: Add `ingest_sock_path` to `Config`** (AC: #1, #2, #3, #5)
  - [ ] Add `pub ingest_sock_path: PathBuf` field to `crates/daemon/src/config.rs`
  - [ ] Set default to `bowerbird_dir.join("ingest.sock")` in `Config::with_bowerbird_dir`

- [ ] **Task 2: Add `Ingest` error variant to `Error`** (all ACs)
  - [ ] Add `#[error("ingest error: {0}")] Ingest(String)` to `crates/daemon/src/error.rs`

- [ ] **Task 3: Create `crates/daemon/src/ingest/` module** (AC: #1, #2, #3, #5, #6)
  - [ ] Create `crates/daemon/src/ingest/mod.rs` — declare `pub mod listener;`, `pub mod handler;`, `pub mod writer;`; re-export `listener::run` and `writer::run`
  - [ ] Create `crates/daemon/src/ingest/listener.rs` — Unix socket accept loop:
    - [ ] `pub async fn run(sock_path: PathBuf, tx: mpsc::Sender<EventEnvelope>, shutdown: CancellationToken) -> crate::error::Result<()>`
    - [ ] Remove stale socket file before bind: `let _ = std::fs::remove_file(&sock_path);`
    - [ ] Bind `tokio::net::UnixListener::bind(&sock_path)`; map error to `Error::Ingest`
    - [ ] Set 0600 permissions immediately after bind (see Dev Notes → "Socket permissions: chmod-after-bind"):
      ```rust
      #[cfg(unix)]
      {
          use std::os::unix::fs::PermissionsExt;
          std::fs::set_permissions(&sock_path, std::fs::Permissions::from_mode(0o600))
              .map_err(|e| Error::Ingest(format!("set_permissions on ingest.sock: {e}")))?;
      }
      ```
    - [ ] Accept loop: `tokio::select!` on `listener.accept()` and `shutdown.cancelled()`
    - [ ] For each accepted connection: `tokio::spawn(handler::handle(stream, tx.clone()))`; log accept errors at `warn` and continue (do not crash the accept loop on transient errors)
    - [ ] On `shutdown.cancelled()`: break out of accept loop, then `let _ = std::fs::remove_file(&sock_path);`
    - [ ] If `listener.accept()` returns a non-transient error: log at `error`, break, clean up socket
  - [ ] Create `crates/daemon/src/ingest/handler.rs` — per-connection handler:
    - [ ] `pub(super) async fn handle(stream: tokio::net::UnixStream, tx: tokio::sync::mpsc::Sender<protocol::EventEnvelope>)`
    - [ ] Split stream into read and write halves via `stream.into_split()`
    - [ ] Wrap read half in `tokio::io::BufReader`; call `AsyncBufReadExt::read_line(&mut buf)` to read until `\n`
    - [ ] If `read_line` returns `Ok(0)` (EOF with no data): return silently — shim disconnected before writing
    - [ ] If `read_line` returns `Err(e)`: log at `debug`, return
    - [ ] Trim trailing `\n` from the buffer; attempt `serde_json::from_str::<serde_json::Value>(trimmed)`
    - [ ] If parse error: write `format!("400 invalid JSON: {e}\n")` to write half, flush, return
    - [ ] If value is not `Value::Object`: write `"400 expected JSON object\n"`, flush, return
    - [ ] Create stub `EventEnvelope` from the JSON (see Dev Notes → "Stub normalization for story 1.3")
    - [ ] Call `tx.try_send(envelope)`:
      - `Ok(())`: write `"200\n"`, flush
      - `Err(TrySendError::Full(_))` or `Err(TrySendError::Closed(_))`: write `"503\n"`, flush
    - [ ] If write/flush to the socket fails (connection closed by shim): log at `debug`, return — not an error
    - [ ] `#[tracing::instrument(skip_all)]` on `handle`; emit `tracing::debug!` on each outcome (200, 400, 503, EOF)

- [ ] **Task 4: Create projection writer task in `crates/daemon/src/ingest/writer.rs`** (AC: #2)
  - [ ] `pub async fn run(mut rx: tokio::sync::mpsc::Receiver<protocol::EventEnvelope>, writer_pool: deadpool_sqlite::Pool, shutdown: tokio_util::sync::CancellationToken)`
  - [ ] Main loop: `tokio::select!` on `rx.recv()` and `shutdown.cancelled()`
  - [ ] On `Some(envelope)` from `rx.recv()`: call `crate::projection::session::write(&writer_pool, envelope).await`; on `Err(e)`: log `tracing::error!(error = ?e, "projection write failed; event dropped")` and **continue** — per NFR5, ENOSPC/write failure must never crash the daemon; the event is dropped
  - [ ] On `None` from `rx.recv()`: all senders dropped; break
  - [ ] On `shutdown.cancelled()`: drain remaining items via `while let Ok(env) = rx.try_recv()` loop, writing each; then break
  - [ ] Do NOT `unwrap()` or `expect()` anywhere in this function outside `#[cfg(test)]`

- [ ] **Task 5: Wire ingest into `main.rs` and `lib.rs`** (AC: #2, #3)
  - [ ] Add `pub mod ingest;` to `crates/daemon/src/lib.rs`
  - [ ] In `main.rs run()`, after `write_recording_started` and before `axum::serve`:
    - [ ] `let (ingest_tx, ingest_rx) = tokio::sync::mpsc::channel::<protocol::EventEnvelope>(config.ingest_channel_capacity);`
    - [ ] Spawn writer: `tokio::spawn(bowerbird_daemon::ingest::writer::run(ingest_rx, pools.writer.clone(), shutdown.clone()));`
    - [ ] Spawn listener: `tokio::spawn(ingest_listener_task(config.ingest_sock_path.clone(), ingest_tx, shutdown.clone()));`
  - [ ] Add private `async fn ingest_listener_task(sock_path, tx, shutdown)` in `main.rs` that calls `ingest::listener::run(...)` and logs any error at `error` level
  - [ ] Ordering is load-bearing: ingest socket MUST open only after migrations complete and `RecordingStarted` is written, so the daemon is fully ready to accept and write events before any shim can connect

- [ ] **Task 6: Contract tests** (AC: #1, #2, #3, #5, #6)
  - [ ] All new tests go in `crates/daemon/tests/contract_daemon.rs`; add helper `async fn start_ingest_listener(tmp: &TempDir, capacity: usize) -> (tokio_util::sync::CancellationToken, PathBuf, tokio::sync::mpsc::Receiver<protocol::EventEnvelope>)`
  - [ ] **`ingest_socket_has_mode_0600`** (AC#1):
    - Start ingest listener on `tmp.path().join("ingest.sock")`
    - `assert_eq!(std::fs::metadata(&sock_path)?.permissions().mode() & 0o777, 0o600)`
  - [ ] **`ingest_200_on_valid_json_object`** (AC#2):
    - Connect via `tokio::net::UnixStream::connect`; write `b"{\"session_id\":\"s1\"}\n"`; read response
    - Assert response starts with `b"200"`
  - [ ] **`ingest_event_reaches_channel_after_200`** (AC#2):
    - Connect and send valid JSON; read 200 response
    - `rx.recv().await` with a short timeout; assert `Some(envelope)` is received
    - Assert `envelope.payload` contains the sent JSON
  - [ ] **`ingest_200_is_ack_before_db_commit`** (AC#2, timing invariant):
    - Use a bounded channel capacity=1 and a disconnected rx (drop rx) so `try_send` always fails via `Closed`
    - Actually: use capacity=1 and hold the rx without consuming; fill the channel first so next send gets `Full` (503) — demonstrates the daemon responds before DB commit
    - Better approach: connect, write, assert the 200 response arrives before any DB row (query immediately after 200 before any drain time)
  - [ ] **`ingest_503_on_full_queue`** (AC#3):
    - Create channel capacity=1; hold tx but don't consume rx; connect and send one event (fills channel); send second event; assert second response is `503`
  - [ ] **`ingest_400_on_invalid_json`** (AC#6):
    - Send `b"not valid json\n"`; assert response starts with `b"400"`
  - [ ] **`ingest_400_on_non_object_json`** (AC#6):
    - Send `b"[1,2,3]\n"`; assert response starts with `b"400"`
  - [ ] **`ingest_no_db_row_on_400`** (AC#6):
    - Full daemon startup with pools; send invalid JSON; query `SELECT COUNT(*) FROM events WHERE source != '__daemon__'`; assert `0`
  - [ ] **`ingest_eof_before_newline_is_silent`**:
    - Connect to socket; close immediately without writing; assert daemon does not crash and next connection succeeds normally

- [ ] **Task 7: Final checks**
  - [ ] `cargo build --workspace` — green, zero warnings
  - [ ] `cargo fmt --check` — green
  - [ ] `cargo clippy --all-targets --workspace -- -D warnings` — green
  - [ ] `cargo test --workspace` — all tests pass including new ingest contract tests

## Dev Notes

### Critical Context from Story 1.2 (DO NOT REPEAT MISTAKES)

**Dependency pins** — use the workspace dep table, not the architecture doc (which lists mutually incompatible versions). Actual installed versions:

| Dep | Actually installed |
|---|---|
| rusqlite | 0.38.0 |
| rusqlite_migration | 2.4.1 |
| deadpool-sqlite | 0.13.0 |
| tokio | 1.52.1 |
| axum | 0.8.9 |

**Workspace lints**: every crate has `[lints] workspace = true`. **Do NOT** add `#![deny(unsafe_code)]` to any source file — the workspace `unsafe_code = "forbid"` is already active. Adding it will produce a `clippy::duplicated_attributes` error.

**Error module contract**: `crates/daemon/src/error.rs` must remain exactly:
```rust
#[derive(Debug, thiserror::Error)]
pub enum Error { /* variants */ }
pub type Result<T> = std::result::Result<T, Error>;
```
Add `Ingest(String)` variant; remove nothing. `lib.rs` re-exports `Error` and `Result` from here.

**`anyhow::Context` boundary**: permitted only in `main.rs`. All ingest source files use `thiserror`-based `Error` types and `?`. Do not add `use anyhow::...` to `ingest/*.rs`.

**`tokio::main(flavor = "current_thread")`**: the daemon is single-threaded. `tokio::spawn()` works here without `spawn_local` because all spawned futures happen to be `Send`. Do not add `rt-multi-thread` feature usage.

**`eprintln!` / `println!`**: forbidden everywhere except `main.rs` migration/HOME validation paths (already established). Ingest errors go through `tracing::error!` / `tracing::debug!`.

**No `unwrap()` / `expect()` outside `#[cfg(test)]`**: ingest module code follows this strictly.

### Wire Protocol Decision: Newline-Delimited JSON

The ingest socket uses **newline-delimited JSON** (NDJ), not HTTP and not length-prefixed framing.

- **Request**: the shim writes `{json_bytes}\n` (a complete JSON value followed by exactly one `\n`)
- **Response**: daemon writes one of:
  - `200\n` — event accepted into the write queue
  - `503\n` — write queue full (backpressure)
  - `400 {human-readable reason}\n` — malformed or structurally invalid payload

Why NDJ over length-prefixed or full HTTP:
- The shim (story 1.5) uses sync I/O only (no Tokio, no HTTP client). Writing bytes to a socket is the simplest sync operation.
- `read_line()` is the exact equivalent on the async daemon side.
- HTTP headers add ~200 bytes per request and require an HTTP parser, both of which add latency and complexity to the shim's hot path.
- Length-prefixed framing requires the shim to know the payload length before writing the framing header, which means buffering the JSON first — more allocation.

The architecture doc notes "Ingest socket wire framing (length-prefixed vs newline-delimited) is TBD at implementation time." This story resolves it as **newline-delimited**. Document this decision in the story's completion notes.

**One request per connection**: the shim connects, writes one event + `\n`, reads the response, disconnects. The handler does NOT loop to read multiple events per connection. This keeps the handler simple and matches the shim's one-shot model.

**Response is a status line, not HTTP**: `200\n` is three bytes. The shim reads exactly to `\n`. The shim in story 1.5 reads back the response line and checks if it starts with `"200"`, `"503"`, or `"400"`.

### Stub Normalization for Story 1.3

Story 1.4 adds `adapter_claude::normalize()` which converts raw Claude Code hook JSON into a canonical `EventEnvelope`. In story 1.3, the adapter doesn't exist yet. The handler creates a minimal placeholder `EventEnvelope` so events can flow through the write queue into SQLite.

The architecture says: "shim writes raw hook JSON verbatim to the Unix socket. Daemon calls `adapter_claude::normalize(hook_kind, raw)` in `ingest/handler.rs`."

For story 1.3 only, use this stub in `handler.rs`:

```rust
fn make_placeholder_envelope(raw: &str, value: &serde_json::Value) -> protocol::EventEnvelope {
    let session_id = value
        .get("session_id")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown")
        .to_string();
    protocol::EventEnvelope {
        source: "claude".to_string(),          // hardcoded; story 1.4 derives from adapter
        session_id,
        kind: protocol::EventKind::PreToolUse, // placeholder; story 1.4 reads hook_kind
        reaction: None,
        payload: raw.to_string(),              // raw JSON stored verbatim per architecture
    }
}
```

Story 1.4 replaces `make_placeholder_envelope` with `adapter_claude::normalize(hook_kind, raw.as_bytes())` and wires the real `hook_kind` from the shim's CLI argument. The function signature and call site in `handler.rs` stay the same; only the implementation changes.

**"Well-formed" for story 1.3**: valid JSON that is a JSON object (`Value::Object`). Arrays, nulls, primitives, and strings are all rejected with 400. No required fields beyond being an object — field validation is the adapter's job.

### Socket Permissions: chmod-after-bind

The architecture specifies `umask(0o177)` before `bind()` to set socket permissions to 0600 without a TOCTOU window. However, calling `libc::umask()` requires an `unsafe {}` block, which is forbidden by the workspace `unsafe_code = "forbid"` constraint.

**Resolution**: set permissions immediately after `bind()` using `std::fs::set_permissions(path, Permissions::from_mode(0o600))`. The TOCTOU window (time between socket file creation and `set_permissions`) is ~1 microsecond. The parent directory `~/.bowerbird/` has mode 0700 (created in story 1.2), so no other user can access the socket file during this window regardless.

Document this in Dev Agent Record completion notes so future reviewers understand why the architecture's umask approach was not used.

### Listen Backlog ≥ 128 (NFR20)

`tokio::net::UnixListener::bind()` internally calls `listen(fd, 1024)` on Linux and macOS — both well above the 128 minimum. No additional code is required. The contract test for AC#5 verifies this by making 129 simultaneous connections and asserting none are rejected immediately.

If the test environment limits socket backlogs (rare in CI), set `SOMAXCONN` in the CI environment or accept the test as a best-effort check with a skip on restricted environments.

### Write Queue and Backpressure Semantics

The write queue is a `tokio::sync::mpsc::channel::<EventEnvelope>(capacity)` where `capacity = config.ingest_channel_capacity` (default 1024). This channel decouples the fast ingest path (socket → ACK) from the slow SQLite path (UPSERT + INSERT).

**Lifecycle:**
- `ingest_tx: mpsc::Sender<EventEnvelope>` — cloned into each `handler::handle` task
- `ingest_rx: mpsc::Receiver<EventEnvelope>` — owned exclusively by `writer::run`

**Backpressure**: `tx.try_send(envelope)`:
- `Ok(())` → respond `200\n` (event is in the queue; SQLite write happens later)
- `Err(TrySendError::Full(_))` → respond `503\n`; the shim exits 0 and logs a warning
- `Err(TrySendError::Closed(_))` → respond `503\n` (writer task has exited, which should not happen during normal operation; treat like backpressure)

**Writer task on shutdown**: when `shutdown.cancelled()` fires, the writer task drains remaining items from `rx` via `try_recv()` before exiting. This prevents events from being silently dropped during graceful shutdown when the daemon is still running the shutdown sequence.

The sender (`ingest_tx`) is dropped when the listener task exits. When all senders drop, `rx.recv()` returns `None` and the writer task exits naturally. The `shutdown.cancelled()` arm exists as an explicit signal for cases where the listener drops the sender before all in-flight items are processed.

### AppState: No Changes

`AppState` does NOT get an `ingest_tx` field. The channel sender is wired directly in `main.rs` and passed to the listener task. The architecture's final `AppState` definition (`{ db, broadcasters, auth, shutdown }`) does not include `ingest_tx`, confirming this is the right design.

The TCP REST/WS surface (axum) and the Unix ingest surface are independent servers sharing only the DB pool via the background writer task.

### Error Handling in Write Task

Per NFR5: "When the host filesystem is full (ENOSPC), the daemon logs the drop at error level and closes the ingest connection; the shim treats any write error as fire-and-forget and exits 0."

The writer task's error handling:
```rust
if let Err(e) = projection::session::write(&writer_pool, envelope).await {
    tracing::error!(error = ?e, "projection write failed; event dropped (disk full?)");
    // Continue — do not crash the daemon or the writer task
}
```

**Do not** propagate this error back to the handler; by the time the writer encounters ENOSPC, the handler has already responded `200\n` to the shim. The shim has disconnected. There is no way to retract the 200 ACK. Log and continue.

### Daemon Startup Sequence Update

The existing startup sequence in `main.rs` (from story 1.2 Dev Notes) gains two new spawns after step 8 (emit `RecordingStarted`) and before step 9 (bind axum listener):

```
1.  Parse CLI args
2.  Compute bowerbird_dir
3.  Install panic hook
4.  Initialize tracing
5.  Initialize AppState fields
6.  Initialize DbPools
7.  Run migrations → set migrations_complete = true
8.  Emit RecordingStarted sentinel
8a. Create ingest mpsc channel (capacity = config.ingest_channel_capacity)  ← NEW
8b. Spawn projection writer task (ingest_rx)                                 ← NEW
8c. Spawn ingest listener task (ingest_tx, sock_path)                        ← NEW
9.  Bind axum listener on config.bind_addr
10. Serve via axum::serve(listener, router).with_graceful_shutdown(...)
11. On signal: emit RecordingEnded, wal_checkpoint(PASSIVE), exit 0
```

The ingest socket opens in step 8c, AFTER migrations complete. This ensures no shim can write events to a database that hasn't been migrated yet.

### File Structure to Create

```
crates/daemon/
└── src/
    └── ingest/
        ├── mod.rs     # NEW — re-exports listener::run, writer::run
        ├── listener.rs # NEW — UnixListener accept loop
        ├── handler.rs  # NEW — per-connection: read → validate → try_send → respond
        └── writer.rs   # NEW — mpsc::Receiver loop → projection::session::write
```

**Modified files:**
- `crates/daemon/src/lib.rs` — add `pub mod ingest;`
- `crates/daemon/src/config.rs` — add `ingest_sock_path: PathBuf`
- `crates/daemon/src/error.rs` — add `Ingest(String)` variant
- `crates/daemon/src/main.rs` — wire channel + spawn ingest tasks (after RecordingStarted)
- `crates/daemon/tests/contract_daemon.rs` — add 8 new contract tests

**Do not create:** `api/ingest.rs`, `api/auth.rs`, `broadcast/`, `api/ws.rs` — those are stories 2.x / 3.x.

### Anti-Patterns to Avoid

- `rusqlite::Connection::open` outside `crates/daemon/src/db/pool.rs` — fails CI lint
- `unwrap()` / `expect()` outside `#[cfg(test)]` — fails clippy
- `eprintln!` / `println!` anywhere in `ingest/` — use `tracing::*`
- `anyhow::Context` in `ingest/*.rs` — use `thiserror`-based `Error::Ingest`
- Adding `deny_unknown_fields` to any outbound type — preserved invariant from 1.1
- Fixed Unix socket paths in tests — always use `tempfile::TempDir`
- `tokio::main` without `flavor = "current_thread"` — already set; don't change
- Calling `projection::session::write()` directly from the handler (bypasses the write queue, breaks the async ACK guarantee)
- Starting the ingest socket before migrations complete (would allow events to land on an unmigrated schema)

### Testing Standards

- All new tests use `#[tokio::test(flavor = "current_thread")]` to match production runtime
- Unix socket paths always via `tempfile::TempDir` — never hardcoded
- Use `tokio::time::timeout(Duration::from_millis(500), ...)` to bound async operations in tests; avoid `sleep()`
- For tests that need both the ingest socket and the DB: call `fresh_pools()` (existing helper) + `bowerbird_daemon::ingest::listener::run` in a spawned task
- For the `ingest_503_on_full_queue` test: create the mpsc channel manually with capacity=1, pre-fill it with a dummy envelope, pass the sender to the listener, then connect and try to send a second event

### Imports Needed in Ingest Module

`crates/daemon/Cargo.toml` already has all required deps. No new dependencies.

```rust
// listener.rs
use std::path::PathBuf;
use tokio::net::UnixListener;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use protocol::EventEnvelope;
use crate::error::{Error, Result};

// handler.rs
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::sync::mpsc;
use protocol::EventEnvelope;

// writer.rs
use deadpool_sqlite::Pool;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use protocol::EventEnvelope;
use crate::projection;
```

### Previous Story Intelligence (1.2)

- **Library-target split** (established in 1.2): `lib.rs` exposes all modules as `pub`. Add `pub mod ingest;` to `lib.rs` so contract tests can import `bowerbird_daemon::ingest::*`. The binary at `main.rs` calls into the library.
- **Contract test helper pattern**: `fresh_pools()` is defined in the existing contract test file. Reuse it. Add a new `start_ingest_listener()` helper that starts the listener in a background task and returns the cancel token + socket path.
- **Test placement**: all new contract tests append to `crates/daemon/tests/contract_daemon.rs`. Do not create a separate `contract_ingest.rs`.
- **Deferred from 1.2**: "Envelope size/format validation in `projection::session::write`" — validating that `source`, `session_id`, and `payload` are non-empty, have no NULL bytes, and that `payload` is valid JSON — was deferred to this story. Implement this validation in `handler.rs` before putting the envelope on the write queue. Reject with 400 if `session_id` is empty after trimming whitespace.

### Git Intelligence (Recent Commits)

- `c74e685` — Merge PR #9: port migration tests and rollback surrogate
- `622e527` — feat(story-1.2): migration tests, rollback surrogate, projection instrument
- `b9010cf` — Merge PR #8: lint upgrades and crash log
- `3fee9cc` — feat(story-1.2): lint upgrades and AC#8 unhandled-error crash log
- `ae0ef96` — feat(story-1.2): implement daemon foundation with SQLite persistence

All story 1.2 code is committed and green. Story 1.3 builds directly on top.

### Deferred from 1.2 Resolved Here

Per `deferred-work.md`: "Envelope size/format validation in `projection::session::write` — no length, NULL-byte, or format guards on `source`/`session_id`/`payload`. Deferred to Story 1.3 ingest endpoint; validation belongs at the HTTP/Unix-socket trust boundary, not at the internal projection layer."

Implement in `handler.rs` after creating the placeholder envelope:
```rust
// Validate envelope fields before queuing
if envelope.session_id.trim().is_empty() {
    write_half.write_all(b"400 session_id must not be empty\n").await.ok();
    return;
}
if envelope.payload.contains('\0') {
    write_half.write_all(b"400 payload must not contain null bytes\n").await.ok();
    return;
}
```

`source` is hardcoded as `"claude"` in the stub — no user-supplied source to validate. Story 1.4 will supply a real source from the adapter and should apply the same validation.

### References

- Story AC: [Source: docs/bmad/planning-artifacts/epics.md#Story 1.3]
- Ingest socket architecture: [Source: docs/bmad/planning-artifacts/architecture.md#Authentication & Security]
- Wire protocol (shim writes raw JSON): [Source: docs/bmad/planning-artifacts/architecture.md#Process Conventions]
- Ingest directory structure: [Source: docs/bmad/planning-artifacts/architecture.md#Complete Project Directory Structure]
- Data flow (shim → listener → handler → normalize → projection): [Source: docs/bmad/planning-artifacts/architecture.md#Data Flow]
- Backpressure (NFR5) and listen backlog (NFR20): [Source: docs/bmad/planning-artifacts/prd.md#Reliability & Data Integrity]
- AppState shape (no ingest_tx): [Source: docs/bmad/planning-artifacts/architecture.md#API & Communication Patterns]
- `unsafe_code = "forbid"` workspace-wide: [Source: Cargo.toml workspace.lints]
- `current_thread` mandate: [Source: docs/bmad/planning-artifacts/architecture.md#API & Communication Patterns]
- Error module contract: [Source: docs/bmad/planning-artifacts/architecture.md#Structural Conventions]
- Anti-patterns: [Source: docs/bmad/planning-artifacts/architecture.md#Enforcement Guidelines]
- Story 1.2 dev notes (dependency pins, lint inheritance): [Source: docs/bmad/implementation-artifacts/1-2-daemon-foundation-with-sqlite-persistence.md#Dev Notes]
- Deferred validation now resolved: [Source: docs/bmad/implementation-artifacts/deferred-work.md]

## Dev Agent Record

### Agent Model Used

(to be filled by dev agent)

### Debug Log References

(to be filled by dev agent)

### Completion Notes List

(to be filled by dev agent)

### File List

(to be filled by dev agent)

## Change Log

- 2026-05-17: Story created via bmad-create-story workflow. Comprehensive context engine analysis: arc from 1.2 completion notes + code state + architecture carried forward. Wire protocol resolved as newline-delimited JSON. Stub normalization pattern documented. Socket permission approach (chmod-after-bind) justified against `unsafe_code = "forbid"` constraint. Deferred 1.2 validation (`session_id`/`payload` guards) now resolved here. 8 contract tests scoped.
