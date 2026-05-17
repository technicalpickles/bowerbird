# Story 1.2: Daemon foundation with SQLite persistence

Status: ready-for-dev

<!-- Validation is optional. Run validate-create-story for a quality check before dev-story. -->

## Story

As a tool builder,
I want bowerbird's daemon to persist events durably to a local WAL-mode SQLite database that survives crashes,
so that I can trust no acknowledged event is ever lost due to unexpected daemon termination.

## Acceptance Criteria

1. **Given** a running daemon that has accepted an event via the internal `projection::session::write()` function (the load-bearing atomic transaction owner — `POST /ingest` arrives in Story 1.3), **When** SIGKILL is sent to the daemon and the daemon is restarted, **Then** the event row and its corresponding `session_projections` row are both present in the event log (NFR6 — WAL durability guarantee; this is contract test #4 — state+event atomicity).

2. **Given** a connection is checked out from either the writer pool (`max_size=1`) or any reader pool (`max_size=4`), **When** `PRAGMA foreign_keys`, `PRAGMA journal_mode`, and `PRAGMA synchronous` are queried on that connection, **Then** the results are `1` (ON), `'wal'`, and `1` (NORMAL) respectively, on every checkout without exception (contract test #2 — PRAGMA invariants on every connection).

3. **Given** the daemon starts for the first time against a fresh data directory, **When** the daemon process reaches readiness, **Then** schema migrations have run automatically via `rusqlite_migration`, the three tables (`events`, `session_projections`, `recording_sessions`) exist with the exact schema defined in the architecture, and `GET /readyz` returns `200` (NFR21).

4. **Given** a migration failure (e.g., `user_version` ahead of bundled migrations, or a corrupt schema), **When** the daemon attempts to start, **Then** it exits non-zero with a human-readable error message to stderr **before** accepting any connections (no half-started state).

5. **Given** the writer pool is actively inserting rows, **When** a reader pool connection executes a `SELECT` query concurrently, **Then** the reader completes without blocking on the writer (WAL concurrent read/write validation; reader sees a consistent snapshot).

6. **Given** any file in the codebase, **When** CI scans for `rusqlite::Connection::open` (any call form), **Then** any call outside the designated connection factory module (`crates/daemon/src/db/pool.rs`) fails the build (contract test #3 — connection factory enforcement; lint self-test also asserts the lint itself fires on a fixture violation).

7. **Given** the daemon is running at default log level, **When** it emits log output, **Then** each line follows the format `<ISO8601 timestamp> <LEVEL> <message>`, the default level is `error`, `-v` enables `info` output, and `-vv` enables `debug` output (NFR16).

8. **Given** the daemon panics or exits via an unhandled error, **When** the process terminates, **Then** a crash report file (containing the panic message and backtrace if available) is written under `~/.bowerbird/` (e.g., `~/.bowerbird/crash-<unix-ms>.log`), and nothing is sent to any external crash reporting service (NFR17).

9. **Given** `GET /healthz` is requested without an `Authorization` header, **When** the daemon's process is up and the request handler runs, **Then** the response is `200 {"status":"ok"}` — this endpoint reflects process liveness only (DB state is `/readyz`'s job).

## Tasks / Subtasks

- [ ] **Task 1: Daemon error type + config module** (AC: #3, #4)
  - [ ] Create `crates/daemon/src/error.rs` with `pub enum Error` (thiserror) covering: `Io`, `Db(rusqlite::Error)`, `Pool(deadpool_sqlite::PoolError)`, `Migration(rusqlite_migration::Error)`, `Bind(std::io::Error)`, `Config(String)`, `TaskPanic(String)`. Add `pub type Result<T> = std::result::Result<T, Error>;`
  - [ ] Create `crates/daemon/src/config.rs` with `pub struct Config` fields: `data_dir: PathBuf` (default `~/.bowerbird/`), `ingest_socket_path: PathBuf` (default `~/.bowerbird/ingest.sock` — bind in 1.3, path computed now), `tcp_addr: SocketAddr` (default `127.0.0.1:0` — port chosen later; for 1.2 a fixed loopback port is acceptable), `writer_pool_max: usize = 1`, `reader_pool_max: usize = 4`, `ingest_channel_capacity: usize = 1024`. Resolve `~` via `std::env::var("HOME")`; fall back to error on missing HOME.
  - [ ] Re-export `Error`/`Result` from `crates/daemon/src/main.rs` or a `lib.rs` if the binary is restructured; keep crate-internal access via `crate::error::*`.

- [ ] **Task 2: Connection factory + dual pool topology** (AC: #2, #5, #6)
  - [ ] Create `crates/daemon/src/db/mod.rs` with `pub use pool::{DbPools, build_pools};` and `pub use migrations::run_migrations;`
  - [ ] Create `crates/daemon/src/db/pool.rs` as the **sole** module containing `rusqlite::Connection::open*` calls. Build factory `fn open_connection(path: &Path) -> Result<rusqlite::Connection>` that:
    - Opens the connection
    - Immediately runs `PRAGMA journal_mode = WAL;`, `PRAGMA synchronous = NORMAL;`, `PRAGMA foreign_keys = ON;`, `PRAGMA busy_timeout = 5000;`
    - Returns the connection (PRAGMAs are now baked into every connection produced by the factory)
  - [ ] Implement `pub struct DbPools { pub writer: deadpool_sqlite::Pool, pub readers: deadpool_sqlite::Pool }`
  - [ ] Implement `pub async fn build_pools(cfg: &Config) -> Result<DbPools>`:
    - Writer pool: `max_size = 1`
    - Readers pool: `max_size = 4`
    - **Both pools** use a `post_create` (or equivalent deadpool hook) that calls the connection factory above; never pass `Connection::open` results to deadpool by any other path.
    - Pool starvation: rely on deadpool's default timeout behavior; ensure a `Pool` error surfaces (not a hang).
  - [ ] Implement `pub async fn checkout_writer(pools: &DbPools) -> Result<deadpool_sqlite::Object>` and the reader equivalent. Both must execute a probe `PRAGMA foreign_keys; PRAGMA journal_mode; PRAGMA synchronous;` inside an `interact` block in a contract test to assert (1, "wal", 1) on every checkout.
  - [ ] Add a `# Safety` / `# Invariants` doc-comment on `db::pool` noting "this module is the sole permitted caller of `rusqlite::Connection::open*`; the CI lint depends on it."

- [ ] **Task 3: Schema migrations via rusqlite_migration** (AC: #3, #4)
  - [ ] Create `crates/daemon/src/db/migrations.rs` containing all migration SQL inline (no external `.sql` files in this story — keep migrations atomic with the binary).
  - [ ] Define migration `M0001_initial_schema` with the schema verbatim from architecture (see Dev Notes — Schema below).
  - [ ] Implement `pub async fn run_migrations(pools: &DbPools) -> Result<()>` that acquires the writer connection and runs `Migrations::new(vec![...]).to_latest(&mut conn)`.
  - [ ] On migration error, propagate via `Result` so the startup sequence in `main.rs` can convert it to `eprintln!("migration failed: {}", err); std::process::exit(1);` **before** any TCP bind occurs.
  - [ ] Add a contract test that constructs `Migrations::new(...)`, calls `validate()` (rusqlite_migration's built-in dry-run consistency check), and asserts `Ok(())`. This catches migration SQL syntax errors at test time, not on user machines.

- [ ] **Task 4: queries.rs centralization (initial scope)** (AC: #1, partial #6)
  - [ ] Create `crates/daemon/src/db/queries.rs` as a `pub(crate)` module of `&'static str` constants for **every** SQL string used in the daemon. Initial entries (more added in later stories):
    - `INSERT_EVENT` — INSERT into events; **omits the `event_id` column** (AUTOINCREMENT assigns it). Columns: `(source, session_id, kind, reaction, payload, created_at)`. RETURNING `event_id` so the caller has the assigned id without a separate `SELECT last_insert_rowid()` round trip.
    - `UPSERT_PROJECTION` — see architecture's exact UPSERT in Dev Notes.
  - [ ] **Forbidden** anywhere outside `queries.rs`: inline SQL string literals containing `INSERT`, `UPDATE`, `DELETE`, `SELECT`. Add a CI grep gate (see Task 8) that fails on inline SQL outside `db/queries.rs` and `db/migrations.rs`.

- [ ] **Task 5: Projection module — atomic transaction owner** (AC: #1)
  - [ ] Create `crates/daemon/src/projection/mod.rs` re-exporting `session::write`.
  - [ ] Create `crates/daemon/src/projection/session.rs` with `pub async fn write(pools: &DbPools, envelope: EventEnvelope) -> Result<EventId>`. Behavior:
    - Acquire writer pool connection (single-writer enforced by pool topology).
    - Begin a transaction.
    - Execute the projection UPSERT (computing the new state JSON; for Story 1.2 the projected state is a placeholder JSON blob like `{"last_kind": "PreToolUse", "updated_at": <ts>}` — state machine semantics arrive in Story 1.6).
    - Execute the event INSERT — `event_id` column **omitted** from the column list; capture the assigned `event_id` via `RETURNING event_id`.
    - Commit.
    - Return the assigned `EventId`.
  - [ ] **Invariant (load-bearing):** the transaction contains exactly these two statements. No additional inserts, no broader wrapping transaction. Violating this breaks contract test #4. See Dev Notes — Transaction Invariant.
  - [ ] `#[tracing::instrument(skip_all, fields(source = %envelope.source, session_id = %envelope.session_id))]` on `write()`. `skip_all` is mandatory — it prevents the raw `payload` (which may contain user-sensitive Claude tool I/O) from being logged into spans.

- [ ] **Task 6: Health endpoints (axum router skeleton)** (AC: #3, #9)
  - [ ] Create `crates/daemon/src/api/mod.rs` re-exporting `health::router`.
  - [ ] Create `crates/daemon/src/api/health.rs`:
    - `GET /healthz` — unauthenticated. Returns `Json(json!({"status": "ok"}))` with `200`. Handler does **not** touch the DB.
    - `GET /readyz` — unauthenticated. Acquires a reader-pool connection, runs `SELECT 1 FROM events WHERE 1=0 LIMIT 1;` (proves the schema exists and the pool is reachable without scanning data). Returns `200` on success or `503 Json(json!({"error": "<reason>"}))` on failure.
  - [ ] `health::router(state: Arc<AppState>) -> axum::Router` returns a `Router` mounted at `/`.
  - [ ] Build `AppState { db: DbPools, shutdown: CancellationToken }` in `crates/daemon/src/state.rs`. Auth/broadcaster fields are placeholders for later stories — leave the struct extensible but include only fields relevant to 1.2 to avoid YAGNI cruft.

- [ ] **Task 7: Daemon startup sequence + logging + crash handler** (AC: #3, #4, #7, #8)
  - [ ] Rewrite `crates/daemon/src/main.rs` as the startup orchestrator. Sequence (order is load-bearing):
    1. Parse CLI args via `clap::Parser` (flags: `-v`, `-vv`). No subcommands in this story.
    2. Install panic hook (see crash handler bullet below) **before** anything else can panic.
    3. Initialize `tracing_subscriber` with the resolved level filter (see logging bullet).
    4. Resolve `Config` (Task 1).
    5. Ensure `data_dir` exists (`tokio::fs::create_dir_all`).
    6. Build pools (Task 2). On error: `eprintln!` and `exit(1)`.
    7. Run migrations (Task 3). On error: `eprintln!` and `exit(1)`.
    8. Build `AppState`.
    9. Build axum app (`health::router(state.clone())`).
    10. Bind TCP listener on `127.0.0.1` (port from config — for this story a fixed loopback port like `127.0.0.1:38121` is fine; configurable port arrives in Story 3.2).
    11. Spawn `axum::serve(...).with_graceful_shutdown(shutdown_signal(state.shutdown.clone()))`.
    12. On shutdown: drain pools (`pools.writer.close(); pools.readers.close();`), log "shutdown complete" at info, exit 0.
  - [ ] **Logging setup** (`tracing_subscriber`):
    - Time formatter: `tracing_subscriber::fmt::time::ChronoUtc::rfc_3339()` — produces ISO8601 (RFC3339 is the ISO8601 profile we need; matches the architecture's "ISO8601 timestamp" requirement).
    - Format: `<ts> <LEVEL> <message>` — use the default `fmt::layer()` with `.with_timer(...)`, `.with_target(false)`, `.with_level(true)` (verify the exact output matches the format spec; adjust formatter if needed).
    - Level filter: `error` by default; `-v` → `info`; `-vv` → `debug`. Use `EnvFilter::new(<level>)`; honor `RUST_LOG` only for development (the user-facing contract is the CLI flags).
  - [ ] **Crash handler:**
    - Install via `std::panic::set_hook(Box::new(|info| { ... }))`.
    - Hook writes `~/.bowerbird/crash-<unix-ms>.log` with: timestamp, panic message, location (file:line:column if present), and best-effort backtrace via `std::backtrace::Backtrace::capture()` (or `force_capture()` with `RUST_BACKTRACE=1`; capture the env decision inside the hook).
    - File mode `0o600` on creation (use `OpenOptions::new().create_new(true).mode(0o600).write(true)`).
    - Hook **must not panic** — any failure to write the crash file falls through silently (the original panic is what matters; double-panicking destroys the diagnostic).
    - No HTTP calls, no external reporting (NFR17).
  - [ ] **Signal handling:** `shutdown_signal()` selects on `tokio::signal::ctrl_c()` and `tokio::signal::unix::signal(SignalKind::terminate())`. On either, trigger `CancellationToken::cancel()` so spawned tasks can drain.

- [ ] **Task 8: CI lint — connection factory enforcement + inline SQL ban** (AC: #6)
  - [ ] Add a CI step (in `.github/workflows/ci.yml`) that runs the connection-factory grep:
    ```bash
    if git grep -nE 'rusqlite::Connection::open' -- '*.rs' \
        | grep -v -E '^crates/daemon/src/db/pool\.rs:'; then
      echo "rusqlite::Connection::open found outside crates/daemon/src/db/pool.rs"
      exit 1
    fi
    ```
  - [ ] Add a CI step for inline SQL ban (heuristic — false-positive-aware):
    ```bash
    if git grep -nE '"(SELECT |INSERT INTO|UPDATE |DELETE FROM)' -- 'crates/daemon/src/**/*.rs' \
        | grep -v -E '^crates/daemon/src/db/(queries|migrations)\.rs:'; then
      echo "inline SQL found outside crates/daemon/src/db/{queries,migrations}.rs"
      exit 1
    fi
    ```
  - [ ] Add a lint self-test (contract test): create `crates/daemon/tests/fixtures/lint_violation.rs.txt` containing a line `let _ = rusqlite::Connection::open("/tmp/foo");`. The CI script must include this fixture in its scan and the test asserts the script produces a non-zero exit code when run against the fixture. (Lint self-test prevents the lint from rotting silently — per architecture's lint discipline.)

- [ ] **Task 9: Contract tests** (AC: #1, #2, #3, #4, #5)
  - [ ] Create `crates/daemon/tests/contract_daemon.rs` (file-backed SQLite in `tempfile::TempDir` — `:memory:` is forbidden per architecture; WAL semantics require a real file).
  - [ ] **Test: PRAGMA invariants on every checkout** — acquire 1 writer + 4 reader connections, on each run `PRAGMA foreign_keys`, `PRAGMA journal_mode`, `PRAGMA synchronous`, assert `(1, "wal", 1)`.
  - [ ] **Test: migrations apply cleanly on fresh DB** — build pools against a brand-new tempdir, call `run_migrations`, then SELECT against `events`, `session_projections`, `recording_sessions` to assert each table exists and matches the column list.
  - [ ] **Test: migration failure surfaces non-zero** — manually set `user_version` to a future value via raw rusqlite open, then attempt `run_migrations`, assert `Err`.
  - [ ] **Test: WAL concurrent reader/writer** — spawn writer task inserting 100 rows in a loop, spawn reader task SELECT-counting concurrently, assert reader never blocks beyond `busy_timeout` and final counts are consistent.
  - [ ] **Test: state+event atomicity (contract #4 — SIGKILL surrogate)** — for this story, simulate via an explicit `tx.rollback()` mid-transaction and assert no half-state. The full SIGKILL-process test is appropriate later when the daemon has a spawnable harness; document the gap in Dev Notes.
  - [ ] **Test: `/healthz` returns 200 unauthenticated** — using `tower::ServiceExt::oneshot` against the router (no port bind needed).
  - [ ] **Test: `/readyz` returns 503 before migrations, 200 after** — drive the router state through both phases.
  - [ ] **Test: connection factory enforcement lint self-test** — see Task 8.

- [ ] **Task 10: Verify everything passes** (AC: all)
  - [ ] `cargo fmt --check` — green
  - [ ] `cargo clippy --all-targets --workspace -- -D warnings` — green
  - [ ] `cargo test --workspace` — all new contract tests pass
  - [ ] `cargo build --workspace` — green
  - [ ] CI lint scripts (Task 8) — both pass on this branch (no violations)
  - [ ] Manually start `cargo run -p bowerbird-daemon`, hit `curl http://127.0.0.1:<port>/healthz` and `/readyz`, observe ISO8601-prefixed log lines, send SIGTERM, observe clean exit 0.

## Dev Notes

### Relevant architecture patterns and constraints (load-bearing for this story)

- **Dual-pool topology with single writer.** `writer.max_size=1` + `readers.max_size=4`. The pool topology enforces single-writer; never rely on SQLite's lock alone. `deadpool-sqlite` 0.13.0 wraps `spawn_blocking` internally — never call SQLite operations on the main async thread directly. [Source: architecture.md#Data Architecture; project-context.md#Daemon → SQLite]
- **Connection factory is the only path to a `Connection`.** Architecture: "Connection factory is the only public path to a `Connection`. Module-private constructor, no raw `rusqlite::Connection::open` calls outside it. CI lint (`grep`/clippy) forbids the raw call." [Source: project-context.md#Daemon → SQLite]
- **PRAGMA invariants set on every connection checkout.** `journal_mode=WAL`, `synchronous=NORMAL`, `foreign_keys=ON`, `busy_timeout=5000`. `foreign_keys` is the canary — NOT on by default in SQLite. A test asserts `PRAGMA foreign_keys = 1` on every checkout. [Source: project-context.md#Daemon → SQLite; architecture.md#Decision Priority Analysis]
- **`rusqlite_migration` from day one.** "Hand-rolled `PRAGMA user_version` is fine for v1 but rusqlite_migration is one Cargo.toml line and adopting it now is the right call." Migrations are auto-applied on startup; migration failure is fatal (NFR21). Append-only — no destructive migrations in V1. [Source: project-context.md#Storage: SQLite via rusqlite; architecture.md#Schema Migrations]
- **Transaction invariant (load-bearing correctness rule).** From architecture, verbatim:
  > Exactly these two operations; nothing else joins this transaction
  > ```rust
  > conn.execute("INSERT INTO session_projections ... ON CONFLICT DO UPDATE ...", ...)?;
  > conn.execute("INSERT INTO events ...", ...)?;
  > ```
  > The projection UPSERT and event INSERT are the only operations in the transaction. A broader wrapping transaction is a prohibited pattern.
  [Source: architecture.md#Process Conventions]
- **`event_id` is AUTOINCREMENT — always omit from INSERT column list.** Never pass `0` or any explicit value. The schema has no `DEFAULT` on this column to prevent accidental explicit-zero inserts. Use `RETURNING event_id`. [Source: architecture.md#Process Conventions]
- **`(source, session_id)` is the natural key for sessions** — Claude session IDs can collide with future sources. Every projection key, every query, every log line uses both. [Source: project-context.md#Substrate-not-actor invariants]
- **Timestamps are Unix milliseconds as `i64` everywhere** on the wire and in the schema. No RFC3339 strings in columns, no seconds, no microseconds. The exception is *log line timestamps*, which are ISO8601 per NFR16. [Source: architecture.md#Wire Format Conventions]
- **WAL checkpoint policy.** PASSIVE checkpoint on clean daemon shutdown only. No periodic checkpointing in V1. SQLite's automatic WAL threshold handles routine operation. [Source: architecture.md#Process Conventions]
- **No `unwrap()` / `expect()` outside `#[cfg(test)]` code.** Hard rule. Setup-time `.expect()` in `main` is acceptable when the alternative is uglier and the alternative is unambiguous failure. [Source: architecture.md#Enforcement Guidelines; project-context.md#Daemon style]
- **`anyhow::Context` permitted only in `main.rs`.** All internal modules (`db/*`, `projection/*`, `api/*`, `config.rs`, `state.rs`) use `thiserror::Error` exclusively. [Source: architecture.md#Process Conventions]
- **`#[tracing::instrument(skip_all, fields(...))]` on every async fn crossing a crate boundary** and on hot paths. `skip_all` is the default — payloads, DB handles, and sensitive data never appear in traces. Specific fields opted in via `fields(...)` only. [Source: architecture.md#Process Conventions]
- **`unsafe_code = "forbid"`** is already set at workspace level. Don't override it in any crate.

### SQLite schema (exact — copy verbatim)

[Source: architecture.md#Data Architecture]

```sql
CREATE TABLE events (
    event_id   INTEGER PRIMARY KEY AUTOINCREMENT,
    source     TEXT    NOT NULL,
    session_id TEXT    NOT NULL,
    kind       TEXT    NOT NULL,          -- EventKind serialized as string ("PreToolUse", "PostToolUse", ...)
    reaction   TEXT,                       -- Reaction variant string; NULL for daemon sentinels
    payload    TEXT    NOT NULL,           -- verbatim raw JSON; no information loss
    created_at INTEGER NOT NULL            -- Unix timestamp ms
);

CREATE TABLE session_projections (
    source     TEXT    NOT NULL,
    session_id TEXT    NOT NULL,
    state      TEXT    NOT NULL,           -- JSON blob of projected session state
    updated_at INTEGER NOT NULL,
    PRIMARY KEY (source, session_id)
);

-- Shadow table; never truncated; enables history_begins_cleanly post-truncation
CREATE TABLE recording_sessions (
    id               INTEGER PRIMARY KEY AUTOINCREMENT,
    started_event_id INTEGER NOT NULL,
    ended_event_id   INTEGER                -- NULL until clean shutdown
);
```

**No `CREATE INDEX` in this story.** Query patterns settle in Story 1.7 (REST query API); adding speculative indexes here violates "no speculative optimization." [Source: architecture.md#NFR2]

### Projection UPSERT pattern (exact)

[Source: architecture.md#Process Conventions]

```sql
INSERT INTO session_projections (source, session_id, state, updated_at)
VALUES (?, ?, ?, ?)
ON CONFLICT(source, session_id)
DO UPDATE SET state = excluded.state, updated_at = excluded.updated_at;
```

### Event INSERT pattern (exact)

```sql
INSERT INTO events (source, session_id, kind, reaction, payload, created_at)
VALUES (?, ?, ?, ?, ?, ?)
RETURNING event_id;
```

Bound parameter order matches the column order. `kind` is the `EventKind` enum **serialized to PascalCase string** (e.g., `"PreToolUse"`) — this is the variant name as serde produces it (no `rename_all`, see Story 1.1 protocol contract). `reaction` is `Option<String>`; `None` binds as SQL NULL.

### Files being updated (read these before changing them)

- `crates/daemon/src/main.rs` — currently `#[tokio::main] async fn main() {}`. Replace with the startup orchestrator (Task 7). The existing tokio runtime declaration is fine; build on it.
- `crates/daemon/Cargo.toml` — currently declares all needed dependencies (tokio, axum, rusqlite, deadpool-sqlite, rusqlite_migration, tracing, tracing-subscriber, anyhow, uuid, tokio-util, tokio-stream, thiserror). **Already wired — do not bump versions.** You may need to add `chrono` (workspace dep — not currently pinned) for ISO8601 formatting; alternatively use `tracing_subscriber::fmt::time::SystemTime` with `time = "0.3"` and the `formatting` feature. Verify which is already transitively available before adding a new dep; if you must add one, it goes in `[workspace.dependencies]` per the Cargo.toml structure from Story 1.1.
- `crates/daemon/Cargo.toml` `[lints] workspace = true` — preserve this. Story 1.1's review fixed it; do not remove.
- `.github/workflows/ci.yml` — currently runs `cargo fmt --check`, `cargo clippy`, `cargo test --workspace` on macOS-latest + ubuntu-latest. Add the connection factory lint and inline SQL lint steps (Task 8). Do NOT modify or remove existing steps. Preserve `components = ["rustfmt", "clippy"]` in `rust-toolchain.toml`.
- `Cargo.toml` (workspace root) — `rusqlite` is pinned to `0.38.0` (not 0.39.0 as the architecture states). Story 1.1's debug log records this: `rusqlite_migration 2.5.0` requires `rusqlite ^0.39` and `deadpool-sqlite 0.13.0` requires `rusqlite ^0.38` — the conflict was resolved by pinning the consistent set: **`rusqlite 0.38.0`, `deadpool-sqlite 0.13.0`, `rusqlite_migration 2.4.1`**. **Do not "fix" this back to 0.39 — it will break the build.**

### Required new module structure under `crates/daemon/src/`

```
crates/daemon/src/
├── main.rs           # startup orchestrator (Task 7)
├── error.rs          # Error enum + Result alias (Task 1)
├── config.rs         # Config struct (Task 1)
├── state.rs          # AppState (Task 6) — extensible but only db+shutdown for this story
├── db/
│   ├── mod.rs        # re-exports
│   ├── pool.rs       # SOLE caller of rusqlite::Connection::open* (Task 2)
│   ├── migrations.rs # rusqlite_migration definitions (Task 3)
│   └── queries.rs    # all SQL string constants (Task 4)
├── projection/
│   ├── mod.rs        # re-exports
│   └── session.rs    # session::write() — SOLE transaction owner (Task 5)
└── api/
    ├── mod.rs        # re-exports
    └── health.rs     # /healthz, /readyz handlers (Task 6)
```

This matches `architecture.md#Complete Project Directory Structure` exactly. `ingest/`, `broadcast/`, `api/auth.rs`, `api/token.rs`, `api/sessions.rs`, `api/events.rs`, `api/ws.rs` are intentionally absent — they arrive in later stories. Do **not** scaffold empty stubs for them in this story.

### Logging format — exact requirement

NFR16: `<ISO8601 timestamp> <LEVEL> <message>`. Example:
```
2026-05-17T14:32:11.482Z INFO daemon started; bound 127.0.0.1:38121
2026-05-17T14:32:11.500Z ERROR migration failed: schema_version_mismatch
```

`tracing_subscriber::fmt` produces this by default with `.with_timer(ChronoUtc::rfc_3339())` and `.with_target(false)`. Verify in a quick `cargo run` that the format matches before claiming AC#7 done. **Don't ship structured JSON logging in this story** — NFR16: "structured JSON logging deferred to V2."

### Testing standards summary

[Source: project-context.md#Testing rules; architecture.md#Structural Conventions]

- **File placement:** integration tests in `crates/daemon/tests/`; contract tests prefixed `contract_` (e.g., `contract_daemon.rs`).
- **SQLite test fixtures:** `tempfile::TempDir` with file-backed SQLite. **`:memory:` is explicitly forbidden** — it doesn't exercise WAL, and WAL guarantees are part of what we're testing.
- **No real `sleep()` for synchronization.** Use `tokio::test(start_paused = true)` + `tokio::time::advance` for time-dependent assertions.
- **`unwrap()` / `expect()` is acceptable in test code** (`#[cfg(test)]` and `tests/` files) — production discipline does not extend to tests.
- **Snapshot tests not required for this story** — they're for wire-format types (Story 1.1 territory). DB tests assert behavior, not byte-shape.

### Anti-patterns to avoid (this story specifically)

- ❌ Calling `rusqlite::Connection::open*` anywhere outside `crates/daemon/src/db/pool.rs` (breaks Task 8 lint)
- ❌ Inline SQL strings (`"SELECT ...""`, `"INSERT ..."`, `"UPDATE ..."`, `"DELETE ..."`) anywhere outside `db/queries.rs` and `db/migrations.rs`
- ❌ Adding any SQL operation to the projection-write transaction beyond the two specified statements (breaks the load-bearing invariant)
- ❌ Passing an explicit `event_id` value (including `0`) to the events INSERT
- ❌ `deny_unknown_fields` on any outbound type (REST or WS) — daemon→client is permissive forever
- ❌ Using `:memory:` SQLite in tests (WAL semantics aren't exercised)
- ❌ Holding a writer connection across an `.await` point on a non-DB future (deadpool will block; let the connection drop before awaiting unrelated work)
- ❌ `eprintln!` / `println!` anywhere except (a) the `eprintln!` in `main.rs` for pre-`tracing` startup errors and (b) the crash handler. All other output goes through `tracing`.
- ❌ Adding `chrono` or any other timestamp lib without verifying it's not already pulled in transitively. Prefer `tracing-subscriber`'s built-in time formatter.
- ❌ Adding indexes to the schema — Story 1.7 territory.
- ❌ Scaffolding `ingest/`, `broadcast/`, or auth modules — out of scope.
- ❌ Catching panics with `std::panic::catch_unwind` *instead of* a panic hook — the hook approach gives crash logs without breaking the panic-on-error behavior elsewhere. axum's `CatchPanicLayer` arrives in Story 2.1 (WS).

### Previous story intelligence (from Story 1.1)

[Source: docs/bmad/implementation-artifacts/1-1-workspace-and-protocol-crate-foundation.md]

**Patterns established in 1.1 that this story must respect:**
- Workspace lints inherited via `[lints] workspace = true` in every member crate. The daemon already has this — preserve.
- `#![deny(unsafe_code)]` is enforced workspace-wide via `unsafe_code = "forbid"`. Per the 1.1 review, redundant `#![deny(unsafe_code)]` crate attributes were removed — do **not** add them back.
- `rust-toolchain.toml` pins `channel = "stable"` at version `1.94.1` with `components = ["rustfmt", "clippy"]`. Do not bump.
- The CI matrix runs on macOS-latest + ubuntu-latest. New CI steps (Task 8) must be cross-platform — use POSIX shell only; macOS bash 3.2 and Ubuntu bash 5 disagree on bashisms. Run `shellcheck` mentally before committing.
- Protocol crate's `Cargo.lock` is committed (139 packages). Any new dependency added in this story must update `Cargo.lock` in the same commit.
- `EventEnvelope` does **not** derive `Serialize`/`Deserialize` (per 1.1 review). The daemon serializes `Event` (post-storage) for wire output, never `EventEnvelope`. For this story, `projection::session::write()` accepts `EventEnvelope` directly — no JSON round-trip required.

**Deferred items from 1.1 that are still deferred (do NOT pick up in this story):**
- `EventEnvelope.payload` schema enforcement — still opaque `String` in 1.2.
- `Reaction::Vendor` error message ambiguity — out of scope.
- `SyncFrame` / `DroppedFrame` invariant validation — Story 2.x territory.
- Windows CI — explicit scope cut.

[Source: docs/bmad/implementation-artifacts/deferred-work.md]

### Git intelligence — recent commits relevant to this story

```
293c3d3 Merge pull request #3 ... (architecture finalization)
98b131c fix(story-1.1): apply code review patches
c96c100 chore(review): add code review findings for story 1.1
f54689e feat(story-1.1): scaffold Rust workspace and implement protocol crate
```

Patterns to follow from `f54689e` (Story 1.1's main implementation):
- Cargo workspace dependency pins are the source of truth — never override versions in member-crate `Cargo.toml`. Use `{ workspace = true }` for every shared dep.
- `[lints] workspace = true` in every member crate.
- Contract tests as `crates/<name>/tests/contract_<name>.rs`.

### Latest tech specifics

- **`rusqlite_migration` 2.4.1** (pinned in workspace) — API: `Migrations::new(vec![M::up("SQL")])`. Use `.to_latest(&mut conn)` to apply. The `M::up_with_hook(...)` variant is available if a migration needs Rust-side logic (not needed for M0001). `.validate()` exists and is the test-time consistency check.
- **`deadpool-sqlite` 0.13.0** — exposes `Pool::builder(Manager::from_config(...)).max_size(n).build()`. The `interact(|conn| { ... })` method runs a closure on a `spawn_blocking` thread with the `&mut Connection`. Errors return `InteractError` which wraps `JoinError` — surface it through your `Error::TaskPanic` variant.
- **`axum` 0.8.9** — `Router::new().route("/healthz", get(healthz_handler)).with_state(state)`. `State<Arc<AppState>>` extractor. For `oneshot` testing: `app.oneshot(Request::builder().uri("/healthz").body(Body::empty()).unwrap()).await`.
- **`tracing-subscriber` 0.3.20** — `fmt::Subscriber::builder().with_env_filter(EnvFilter::new("info")).with_timer(fmt::time::ChronoUtc::rfc_3339()).finish()`. If `ChronoUtc` is not directly available, the `time` crate with its `formatting` feature is the alternative (already-pinned tokio brings in `time` transitively; verify before adding to workspace deps).
- **`tokio-util` 0.7.18** — `CancellationToken::new()` for shutdown coordination; `.cancelled().await` in tasks.

### Open questions saved for end of story (do NOT block on these)

These are noted for awareness; the dev agent should pick reasonable defaults and proceed:

1. **TCP port for the daemon's REST/WS surface.** Architecture says `127.0.0.1:<port>`. Story 3.2 introduces lifecycle CLI; for Story 1.2 a hardcoded port (e.g., `127.0.0.1:38121`) is acceptable. Make it configurable via `Config.tcp_addr` so 3.2 can flip it to dynamic later without API churn.
2. **Exact crash file naming.** `~/.bowerbird/crash-<unix-ms>.log` is a reasonable default. Architecture says "crash information ... is written to a file under `~/.bowerbird/`" without specifying the name.
3. **Should `/readyz` probe via `interact()` (writer pool) or a reader connection?** Reader is correct — readiness should not contend with the writer queue. Confirmed: use reader pool.

### Project Structure Notes

- All new modules align exactly with `architecture.md#Complete Project Directory Structure`. No deviations.
- The daemon's binary name is `bowerbird-daemon` (set in `crates/daemon/Cargo.toml`). When Story 3.2 wires the top-level `bowerbird` CLI to spawn this binary, the PATH lookup expects this exact name. Do not rename.
- The `bowerbird` (root) CLI binary is currently a stub — leave it alone in this story.
- `crates/daemon/src/lib.rs` does **not** exist and should not be created in this story. The daemon is a binary crate; tests reach into modules via `mod` declarations in `main.rs` exposed `pub(crate)` plus `tests/` accessing via the binary's `tests::` path through `#[path = "..."]` if needed. Simpler: put genuinely shared test helpers in `tests/common/mod.rs` and have contract tests use `mod common;`.

### References

- Story AC source: [Source: docs/bmad/planning-artifacts/epics.md#Story 1.2: Daemon foundation with SQLite persistence]
- SQLite schema + dual-pool topology: [Source: docs/bmad/planning-artifacts/architecture.md#Data Architecture]
- Transaction invariant + event INSERT discipline: [Source: docs/bmad/planning-artifacts/architecture.md#Process Conventions]
- Connection factory enforcement: [Source: docs/bmad/project-context.md#Daemon → SQLite]
- PRAGMA invariants list: [Source: docs/bmad/project-context.md#Daemon → SQLite]
- Logging format (NFR16): [Source: docs/bmad/planning-artifacts/epics.md#NonFunctional Requirements (NFR16)]
- Crash file location (NFR17): [Source: docs/bmad/planning-artifacts/epics.md#NonFunctional Requirements (NFR17)]
- Auto-migration + fatal failure (NFR21): [Source: docs/bmad/planning-artifacts/epics.md#NonFunctional Requirements (NFR21)]
- Daemon readiness within 2s (NFR3): [Source: docs/bmad/planning-artifacts/epics.md#NonFunctional Requirements (NFR3)]
- WAL durability (NFR6): [Source: docs/bmad/planning-artifacts/epics.md#NonFunctional Requirements (NFR6)]
- Project directory structure: [Source: docs/bmad/planning-artifacts/architecture.md#Complete Project Directory Structure]
- Anti-patterns list: [Source: docs/bmad/planning-artifacts/architecture.md#Enforcement Guidelines]
- Substrate-not-actor invariants: [Source: docs/bmad/project-context.md#Substrate-not-actor invariants]
- Required framework infrastructure (AppState shape, CancellationToken, graceful shutdown): [Source: docs/bmad/project-context.md#Required framework infrastructure]
- Required contract tests catalog: [Source: docs/bmad/project-context.md#Required contract tests]
- Story 1.1 patterns + deferred items: [Source: docs/bmad/implementation-artifacts/1-1-workspace-and-protocol-crate-foundation.md; docs/bmad/implementation-artifacts/deferred-work.md]
- Rust dependency pin reality (rusqlite 0.38.0 chain): [Source: docs/bmad/implementation-artifacts/1-1-workspace-and-protocol-crate-foundation.md#Debug Log References]

## Dev Agent Record

### Agent Model Used

{{agent_model_name_version}}

### Debug Log References

### Completion Notes List

### File List
