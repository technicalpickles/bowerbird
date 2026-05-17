# Story 1.2: Daemon Foundation with SQLite Persistence

Status: done

## Story

As a tool builder,
I want bowerbird's daemon to persist events durably to a local WAL-mode SQLite database that survives crashes,
So that I can trust no acknowledged event is ever lost due to unexpected daemon termination.

## Acceptance Criteria

1. **Given** a running daemon that has accepted an event through the projection write path **When** SIGKILL is sent to the daemon (or the SQLite connection is dropped without graceful close) and the daemon is restarted **Then** the previously-written event is present in the event log on the next read (NFR6: WAL durability guarantee). [Note: Story 1.2 has no ingest socket yet — Story 1.3 wires `POST /ingest`. Durability is proven here via a contract test that calls `projection::session::write()` directly and verifies survival across SQLite open/close cycles.]

2. **Given** a connection is checked out from either the writer pool (max_size=1) or any reader pool (max_size=4) **When** `PRAGMA foreign_keys`, `PRAGMA journal_mode`, `PRAGMA synchronous`, and `PRAGMA busy_timeout` are queried on that connection **Then** the results are `1` (ON), `wal`, `1` (NORMAL), and `5000` (ms) respectively, on every checkout without exception. The connection-factory hook also rejects (and prevents pool reuse of) any connection whose `journal_mode` did not actually flip to `wal` — guarding against read-only or network filesystems where the WAL pragma silently no-ops.

3. **Given** the daemon starts for the first time against a fresh data directory **When** the daemon process becomes ready **Then** schema migrations have run automatically via `rusqlite_migration` and `GET /readyz` returns 200 (NFR21).

4. **Given** a migration failure (e.g., manually corrupted `user_version`) **When** the daemon attempts to start **Then** it exits non-zero with a human-readable error message to stderr before accepting any connections.

5. **Given** the writer pool is actively inserting rows **When** a reader pool connection executes a SELECT query concurrently **Then** the reader completes without blocking on the writer (WAL concurrent read/write validation).

6. **Given** any file in the codebase **When** a CI lint (grep) scans for `rusqlite::Connection::open` **Then** any call outside the designated connection factory module (`crates/daemon/src/db/pool.rs`) fails the build, confirming the factory-only access policy.

7. **Given** the daemon is running with default log level **When** it emits log output **Then** each line follows the format `<ISO8601 timestamp> <LEVEL> <message>` and the default level is `error`; running with `-v` exposes `info`-level output and `-vv` exposes `debug`-level output (NFR16).

8. **Given** the daemon crashes unexpectedly (panic or unhandled error) **When** the process exits **Then** crash information (panic message, location, backtrace if available) is written to a file under `~/.bowerbird/` and nothing is sent to an external crash reporting service (NFR17).

## Tasks / Subtasks

- [x] **Task 1: Create `crates/daemon/src/db/` module with connection factory and pool configuration** (AC: #2, #5, #6)
  - [x] Create `crates/daemon/src/db/mod.rs` — re-exports public types (`DbPools`, `init_pools`); the SOLE public API surface of the module
  - [x] Create `crates/daemon/src/db/pool.rs` — the **only** file in the entire workspace that may call `rusqlite::Connection::open` or `deadpool_sqlite::Pool::builder`
  - [x] Define `pub struct DbPools { pub writer: deadpool_sqlite::Pool, pub reader: deadpool_sqlite::Pool }`
  - [x] Implement `pub async fn init_pools(db_path: &std::path::Path) -> Result<DbPools>` that builds the writer pool with `max_size(1)` and reader pool with `max_size(4)`
  - [x] Configure both pools with `deadpool_sqlite::Config::new(db_path).create_pool(Runtime::Tokio1)` and install a **per-connection setup hook** that runs the PRAGMA bundle on every checkout (see Dev Notes → "PRAGMA enforcement pattern")
  - [x] Add `[CI Lint]` CI step or test in `crates/daemon/tests/contract_db.rs`: `grep -r "rusqlite::Connection::open" crates/daemon/src/ | grep -v "src/db/pool.rs"` must produce no output (factory-only policy)
- [x] **Task 2: Implement schema migrations** (AC: #3, #4)
  - [x] Create `crates/daemon/src/db/migrations.rs` with a `Migrations` value defined via `rusqlite_migration::Migrations::new(...)`
  - [x] V1 migration creates exactly three tables: `events`, `session_projections`, `recording_sessions` (see Dev Notes → "SQLite schema (V1)")
  - [x] `events.event_id` is `INTEGER PRIMARY KEY AUTOINCREMENT` (with `AUTOINCREMENT`, NOT just `INTEGER PRIMARY KEY` — the autoincrement keyword forces strict monotonicity and prevents ID reuse)
  - [x] Migrations execute synchronously in the writer pool before `axum` begins serving readyz=200; readyz returns 503 until migrations complete
  - [x] Migration failure → `main.rs` writes the error to stderr via `anyhow::Context` and `process::exit(1)`; nothing has been bound yet so no socket cleanup is needed
- [x] **Task 3: Centralize all SQL strings in `crates/daemon/src/db/queries.rs`** (AC: #1, AC#6 spirit)
  - [x] Define `pub const` string statements for every SQL operation in this story:
    - `INSERT_EVENT` — `INSERT INTO events (source, session_id, kind, reaction, payload, created_at) VALUES (?, ?, ?, ?, ?, ?)` — note: `event_id` is **omitted** (AUTOINCREMENT assigns)
    - `UPSERT_SESSION_PROJECTION` — see Dev Notes → "Projection UPSERT pattern"
    - `INSERT_RECORDING_SESSION_STARTED`, `UPDATE_RECORDING_SESSION_ENDED`
    - `SELECT_EVENT_BY_ID` (used only by tests in this story)
  - [x] **No inline SQL anywhere else in the daemon crate** — clippy/grep-based lint enforces this in a follow-up story; for now, code review is the gate
- [x] **Task 4: Implement `crates/daemon/src/projection/session.rs` — the SOLE owner of the SQLite write transaction** (AC: #1)
  - [x] Create `crates/daemon/src/projection/mod.rs` re-exporting `session::write`
  - [x] Implement `pub async fn write(writer_pool: &deadpool_sqlite::Pool, envelope: protocol::EventEnvelope) -> Result<protocol::EventId>`
  - [x] Use `conn.interact(|conn| { ... })` to enter the blocking SQLite context; inside it:
    - Open a **single** transaction via `conn.transaction()`
    - Execute exactly two statements: `UPSERT_SESSION_PROJECTION` then `INSERT_EVENT`
    - Read `event_id` via `conn.last_insert_rowid()`
    - Commit
  - [x] **Forbidden:** any additional statement, sub-transaction, savepoint, or external call inside this transaction. The transaction has exactly two write operations and nothing else.
  - [x] Reject `EventEnvelope` if it has non-zero `event_id` field — but note `EventEnvelope` does not declare an `event_id` field (per `protocol/src/event.rs:18-26`), so this guard is structural, not runtime
- [x] **Task 5: Implement `crates/daemon/src/api/` HTTP server foundation** (AC: #3)
  - [x] Create `crates/daemon/src/api/mod.rs` exposing `pub fn router(state: AppState) -> axum::Router`
  - [x] Create `crates/daemon/src/api/health.rs` with two handlers:
    - `GET /healthz` → always returns 200 once axum is serving (liveness)
    - `GET /readyz` → returns 200 if `AppState.migrations_complete` is true, else 503
  - [x] Both endpoints are **unauthenticated** — no bearer middleware (token middleware lands in Story 3.3)
  - [x] HTTP error body format: exactly `{ "error": "<human-readable message>" }` — no `code` field, no nested structure
- [x] **Task 6: Implement `crates/daemon/src/state.rs` and `crates/daemon/src/config.rs`** (AC: #3, #4)
  - [x] `config.rs` — define `pub struct Config { pub db_path: PathBuf, pub bind_addr: SocketAddr, pub ingest_channel_capacity: usize }`; default `bind_addr` = `127.0.0.1:0` (port assigned by OS until Story 3.x; persistent port lives in config file then), `ingest_channel_capacity = 1024`, `db_path = ~/.bowerbird/bower.db`
  - [x] `state.rs` — define `pub struct AppState { pub db: DbPools, pub migrations_complete: Arc<AtomicBool>, pub shutdown: tokio_util::sync::CancellationToken }` (broadcast hub and auth fields land in Stories 2.x and 3.x)
- [x] **Task 7: Wire `crates/daemon/src/main.rs` end-to-end** (AC: #3, #4, #7, #8)
  - [x] Use `#[tokio::main(flavor = "current_thread")]` — **NOT** the default multi-thread runtime (architecture mandate: `current_thread` only)
  - [x] Parse `-v` / `-vv` CLI flags (use `clap` derive, or hand-rolled — see Dev Notes → "CLI verbosity parsing")
  - [x] Initialize `tracing-subscriber` with custom formatter producing exactly `<ISO8601 timestamp> <LEVEL> <message>` (see Dev Notes → "Tracing format")
  - [x] Install a panic hook that writes crash info to `~/.bowerbird/crash-<unix_ms>.log` (mode 0600) before propagating (see Dev Notes → "Panic hook")
  - [x] Ensure `~/.bowerbird/` directory exists with mode 0700; if creation fails, exit non-zero with an error to stderr
  - [x] Emit `EventKind::RecordingStarted` sentinel via `projection::session::write()` **after** migrations complete and **before** `axum` serves traffic (this paves the way for Story 1.6 — gap detection)
  - [x] Bind axum to `config.bind_addr`; serve via `axum::serve(listener, router).with_graceful_shutdown(shutdown_signal())`
  - [x] On clean shutdown (SIGTERM/SIGINT via `tokio::signal`): emit `EventKind::RecordingEnded` sentinel, run `PRAGMA wal_checkpoint(PASSIVE)` once on the writer pool, then exit 0
  - [x] On migration failure or directory-creation failure: write the error to stderr via `eprintln!` (this is `main.rs` — the **only** place `eprintln!` is permitted in the daemon binary), then `process::exit(1)`
- [x] **Task 8: Contract tests (`crates/daemon/tests/contract_daemon.rs`)** (AC: #1, #2, #4, #5)
  - [x] **`pragmas_on_every_writer_checkout`** (AC#2): check out a writer connection, query the three pragmas; assert results `(1, "wal", 1)`; repeat 3 times — pragmas must be set per-checkout, not once-per-pool
  - [x] **`pragmas_on_every_reader_checkout`** (AC#2): same as above for reader pool
  - [x] **`wal_durability_after_simulated_crash`** (AC#1): create a tempfile-backed SQLite DB, run migrations, write an event via `projection::session::write()`, drop the connection without WAL checkpoint, reopen DB with same migrations, query `SELECT * FROM events WHERE event_id = ?`, assert the event is present with all fields intact
  - [x] **`concurrent_read_during_write`** (AC#5): spawn a reader task running a long `SELECT` while the writer pool inserts; assert reader completes within 100ms regardless of writer activity
  - [x] **`migration_failure_exits_nonzero`** (AC#4): create a tempfile DB and manually `PRAGMA user_version = 9999` (a version higher than the migrator knows), launch the daemon binary (`assert_cmd::Command::cargo_bin("bowerbird-daemon")`), assert exit code != 0 and stderr contains a clear error message
  - [x] **`pool_starvation_returns_defined_error`** (AC#5, deferred contract from architecture): check out all 4 reader connections, attempt a 5th with `.timeout()`; assert a defined error (not silent hang)
  - [x] **`readyz_returns_503_before_migrations_complete`** (AC#3): start daemon with a slow migration (insert a `sleep` via test-only feature flag, or migrate against a slow-disk tempfs); poll `/readyz` and observe 503 → 200 transition
  - [x] **`healthz_returns_200_immediately`** (AC#3): `/healthz` returns 200 even when readyz is 503
- [x] **Task 9: CI lint for connection factory policy** (AC: #6)
  - [x] Add a job step to `.github/workflows/ci.yml` (or a new `scripts/lint-db-access.sh`) that runs: `! grep -rn "rusqlite::Connection::open" crates/daemon/src/ | grep -v "crates/daemon/src/db/pool.rs"` — non-zero exit if any other file calls `Connection::open` directly
  - [x] Document the lint command in the daemon crate's `README.md` (create if missing — single-paragraph note pointing to `db/pool.rs` as the factory)
- [x] **Task 10: Final checks**
  - [x] `cargo build --workspace` — green, zero warnings
  - [x] `cargo fmt --check` — green
  - [x] `cargo clippy --all-targets --workspace -- -D warnings` — green
  - [x] `cargo test --workspace` — all contract tests pass

### Review Findings (2026-05-17)

Three-layer adversarial review (Blind Hunter + Edge Case Hunter + Acceptance Auditor) against commit `ae0ef96`. Triaged into decision-needed / patch / defer. See `deferred-work.md` for deferred items.

#### Decisions (resolved 2026-05-17)

- [x] [Review][Decision→Defer] **Singleton enforcement** — deferred to Story 3.1/3.2 (daemon lifecycle CLI). Single-user assumption acknowledged; P8 explicit started-rowid reduces corruption surface in 1.2
- [x] [Review][Decision→Defer] **Daemon address discoverability** — deferred to Story 3.x (bearer-token + lifecycle); P20 (bind log at WARN) covers local-dev observability
- [x] [Review][Decision→Patch] **Deadpool checkout timeout** — chosen: add `Pool::timeouts.wait = Some(Duration::from_secs(5))` on both pools, surface as typed error (becomes Patch P22)
- [x] [Review][Decision→Defer] **Envelope size/format validation** — deferred to Story 1.3 ingest endpoint; trust boundary lives at the ingress
- [x] [Review][Decision→Patch] **`recording_sessions` ↔ `events` atomicity** — chosen: fold sentinel event + `recording_sessions` row into a single projection-layer transaction (becomes Patch P23)
- [x] [Review][Decision→Patch] **`PRAGMA busy_timeout = 5000`** — chosen: keep and update Dev Notes (AC#2 too) to document the 4-pragma bundle (becomes Patch P24)
- [x] [Review][Decision→Patch] **Malformed `RUST_LOG` warning** — chosen: parse first, `eprintln!` warning on Err, fall back to verbosity-derived default (becomes Patch P25)
- [x] [Review][Decision→Patch] **FK from `recording_sessions` to `events`** — chosen: add `FOREIGN KEY(started_event_id) REFERENCES events(event_id)` and same for `ended_event_id` in the V1 migration (becomes Patch P26)

#### Patches

- [x] [Review][Patch] **Panic hook replaces default with no stderr fallback / no recursion guard / non-string payload truncation** [crates/daemon/src/lib.rs] — chain previous hook, `eprintln!` on file-write failure, wrap body in `catch_unwind`, attempt Debug downcast for non-string payloads
- [x] [Review][Patch] **Crash-log file mode set after creation — TOCTOU + symlink hijack window** [crates/daemon/src/lib.rs:839-844] — use `OpenOptions::new().mode(0o600).create_new(true).write(true).open(...)` on Unix
- [x] [Review][Patch] **`ensure_bowerbird_dir` widens perms unconditionally and follows symlinks** [crates/daemon/src/lib.rs:872-879] — `symlink_metadata` check; refuse to chmod symlinks; only `set_permissions(0o700)` when dir is newly created
- [x] [Review][Patch] **Empty / non-absolute `$HOME` accepted silently** [crates/daemon/src/main.rs:914-921] — reject empty or non-absolute HOME with `eprintln! + exit(1)` (sanctioned dir-creation prerequisite)
- [x] [Review][Patch] **`init_tracing` swallows `try_init` error** [crates/daemon/src/lib.rs:860-869] — use `.init()` per spec example, or `eprintln!` on Err so a duplicate subscriber install does not mute logging silently
- [x] [Review][Patch] **`CancellationToken` never cancelled on signal** [crates/daemon/src/main.rs shutdown_signal] — call `shutdown.cancel()` inside `shutdown_signal` after the `select!` resolves
- [x] [Review][Patch] **`RecordingEnded` sentinel + WAL checkpoint skipped on `axum::serve` error** [crates/daemon/src/main.rs:974-984] — capture serve result with `let`; always run sentinel + `wal_checkpoint(PASSIVE)` cleanup; propagate the original error after
- [x] [Review][Patch] **`UPDATE_RECORDING_SESSION_ENDED` closes `MAX(id)` instead of the started-row PK** [crates/daemon/src/db/queries.rs + main.rs:1057] — return the rowid from `INSERT_RECORDING_SESSION_STARTED` and pass it as a bound parameter to the UPDATE
- [x] [Review][Patch] **`current_unix_millis` silently returns `0` on clock failure / clamps i64 overflow** [crates/daemon/src/projection/session.rs:1151-1158] — propagate `duration_since` error; use `i64::try_from(u128)?` and surface a typed error
- [x] [Review][Patch] **`migrations_complete` never reset on shutdown — `/readyz` keeps returning 200 during drain** [crates/daemon/src/main.rs shutdown path] — `migrations_complete.store(false, Ordering::Release)` before the RecordingEnded sentinel
- [x] [Review][Patch] **Double Ctrl-C cannot force-exit; SIGHUP/SIGQUIT silently ignored** [crates/daemon/src/main.rs shutdown_signal] — install a second-signal handler that `process::exit(130)` after the first; add SIGHUP/SIGQUIT arms to the `select!` (treat as terminate)
- [x] [Review][Patch] **Two panics within the same millisecond overwrite the same crash log filename** [crates/daemon/src/lib.rs:823] — include nanoseconds + pid + thread-id in the filename, e.g. `crash-<unix_ns>-<pid>-<tid>.log`
- [x] [Review][Patch] **PRAGMA `journal_mode = WAL` silently falls back on read-only FS** [crates/daemon/src/db/pool.rs:31-42] — after `execute_batch`, run `PRAGMA journal_mode` and verify result is `"wal"`; return `HookError::Backend` otherwise
- [x] [Review][Patch] **Reaction storage helper bypasses protocol's serde impl — silent drift risk** [crates/daemon/src/db/queries.rs:770-777] — per Dev Notes "EventKind & Reaction Serialization", use `serde_json::to_string(&reaction)?.trim_matches('"')` instead of hand-rolled `format!("Vendor({n})")`
- [x] [Review][Patch] **`AppState.db` wrapped in `Arc` deviates from spec type signature** [crates/daemon/src/state.rs:1172-1174] — spec says `pub db: DbPools`; diff has `Arc<DbPools>` + `#[derive(Clone)]`. Drop the Arc wrapper (DbPools' inner Pools are already Arc-shared); update main.rs and tests
- [x] [Review][Patch] **`eprintln!` used outside sanctioned migration/dir-creation paths** [crates/daemon/src/main.rs:934] — violates Dev Notes "Anti-Patterns to Avoid". Catch-all `run()`-error eprintln must route through `tracing::error!`
- [x] [Review][Patch] **PRAGMA `post_create` hook does not run on every checkout — partial AC#2 coverage** [crates/daemon/src/db/pool.rs] — AC#2 mandates "every checkout without exception". Wire `post_recycle` (or `pre_recycle`) to re-apply the bundle, or document why post_create suffices and update AC
- [x] [Review][Patch] **Dead error variants: `MigrationError`, `Error::Sqlite`, `Error::Io`** [crates/daemon/src/db/migrations.rs:596-600, crates/daemon/src/error.rs:789] — declared and re-exported but never constructed. Remove
- [x] [Review][Patch] **`/readyz` 503 body reads "migrations in progress" even after migration failure** [crates/daemon/src/api/health.rs:537] — change to `"not ready"` so a regression that lets the daemon survive a migration failure is not masked
- [x] [Review][Patch] **Default verbosity (`error`) hides the "daemon listening on <addr>" line** [crates/daemon/src/main.rs:971] — emit the bind line at WARN (or ERROR) so operators can see the port on default launch (overlaps with Decision: address discoverability)
- [x] [Review][Patch] **Early-startup panic (before `install_panic_hook` runs) produces no crash log** [crates/daemon/src/main.rs:36-42] — install panic hook FIRST with a temp-path fallback (e.g., `/tmp`), then create the dir; or fold both steps so the hook is always armed for any code that can panic
- [x] [Review][Patch] **(D3→P22) Deadpool checkout timeout** [crates/daemon/src/db/pool.rs] — configure `Pool::timeouts.wait = Some(Duration::from_secs(5))` on both writer and reader pools; map elapsed-timeout error to a typed `Error::Pool` variant
- [x] [Review][Patch] **(D5→P23) Fold sentinel event + `recording_sessions` row into single transaction** [crates/daemon/src/projection/session.rs + main.rs] — extend the projection helper (or add a sibling) that takes an optional sentinel-marker action and commits both INSERTs in one transaction; preserves the spec's "single load-bearing transaction" invariant
- [x] [Review][Patch] **(D6→P24) Document `PRAGMA busy_timeout = 5000` in spec** [docs/bmad/implementation-artifacts/1-2-…md Dev Notes "PRAGMA Enforcement Pattern" + AC#2] — update spec to list the 4-pragma bundle so the implementation stops being an undisclosed deviation
- [x] [Review][Patch] **(D7→P25) Warn on malformed `RUST_LOG`** [crates/daemon/src/lib.rs:858] — parse `RUST_LOG` explicitly; on Err, `eprintln!` a warning ("RUST_LOG=… could not be parsed; falling back to -v verbosity") and use the verbosity-derived default
- [x] [Review][Patch] **(D8→P26) Add FK from `recording_sessions` to `events`** [crates/daemon/src/db/migrations.rs] — extend V1 migration with `FOREIGN KEY(started_event_id) REFERENCES events(event_id)` and `FOREIGN KEY(ended_event_id) REFERENCES events(event_id)`; V1 has not shipped so no migration-versioning cost

#### Deferred

- [x] [Review][Defer] **SIGKILL / `exit(1)` paths skip the `RecordingEnded` sentinel + WAL checkpoint** — covered by Story 1.6 gap-detection design
- [x] [Review][Defer] **`event_kind_as_str` ↔ serde equivalence untested** [crates/daemon/src/db/queries.rs:758-767]
- [x] [Review][Defer] **Migration idempotency on a populated DB is untested** [crates/daemon/src/db/migrations.rs]
- [x] [Review][Defer] **`Pool::interact` errors collapse to opaque strings, losing cause chain** [crates/daemon/src/db/migrations.rs:648, projection/session.rs:1145]
- [x] [Review][Defer] **`migration_failure_exits_nonzero` could hang for 20s if a regression lets the daemon survive** [crates/daemon/tests/contract_daemon.rs:1347-1352]
- [x] [Review][Defer] **No tests for `install_panic_hook` or `init_tracing`** — should follow the panic-hook patches in this round
- [x] [Review][Defer] **CLI surface: no `--db-path`, `--bind-addr`, `--config`, `--version`** [crates/daemon/src/main.rs:904-910] — explicitly out of scope for this story
- [x] [Review][Defer] **`init_pools` does not validate that `db_path` parent exists / is writable** [crates/daemon/src/db/pool.rs] — SQLite returns a reasonable error at first checkout
- [x] [Review][Defer] **`i64::try_from(u128)` timestamp overflow at year 292278994 AD** [crates/daemon/src/projection/session.rs]
- [x] [Review][Defer] **`wal_durability_after_simulated_crash` uses `drop(pool)` not a true crash** [crates/daemon/tests/contract_daemon.rs:60-95] — AC#1 acknowledges this; richer subprocess-based test is follow-up work
- [x] [Review][Defer] **`migration_failure_exits_nonzero` TempDir cleanup vs daemon panic-write race** [crates/daemon/tests/contract_daemon.rs:148-176]
- [x] [Review][Defer] **`scripts/lint-db-access.sh` bypassable via aliased imports / BSD grep symlink behavior** [scripts/lint-db-access.sh] — clippy-based version planned per spec
- [x] [Review][Defer] **`tokio::signal::unix::signal(...)` registration failure is not logged** [crates/daemon/src/main.rs shutdown_signal]
- [x] [Review][Defer] **`migration_failure_exits_nonzero` does not assert "before accepting any connections"** [crates/daemon/tests/contract_daemon.rs:1332-1365] — AC#4 wording; implicit in main.rs ordering today
- [x] [Review][Defer] **(D1) Singleton enforcement — file lock / PID file** — deferred to Story 3.1/3.2 (daemon lifecycle CLI); single-user assumption documented
- [x] [Review][Defer] **(D2) Daemon address discoverability — port file or pinned port** — deferred to Story 3.x (bearer-token + lifecycle); P20 covers local-dev observability for 1.2
- [x] [Review][Defer] **(D4) Envelope size/format validation in projection layer** — deferred to Story 1.3 ingest endpoint; validation belongs at the trust boundary

## Dev Notes

### Critical Context From Story 1.1 (DO NOT REPEAT MISTAKES)

**Dependency conflict resolved in 1.1 — DO NOT roll back to architecture's pinned versions:** The architecture doc (`docs/bmad/planning-artifacts/architecture.md#Dependency Version Pins`) lists `rusqlite 0.39.0` + `rusqlite_migration 2.5.0` + `deadpool-sqlite 0.13.0`. **These are mutually incompatible.** Story 1.1 resolved this to the actually-installed set in `Cargo.toml`:

| Dep | Architecture says | **Actually installed in workspace** |
|---|---|---|
| rusqlite | 0.39.0 | **0.38.0** |
| rusqlite_migration | 2.5.0 | **2.4.1** |
| deadpool-sqlite | 0.13.0 | 0.13.0 |
| tokio | 1.52.1 | 1.52.1 |
| axum | 0.8.9 | 0.8.9 |

Use the **workspace dep table** as the source of truth, not the architecture doc. The daemon `Cargo.toml` already declares all needed deps via `{ workspace = true }` — see `crates/daemon/Cargo.toml:11-26`.

**Workspace lints already inherited:** Each member crate has `[lints] workspace = true` per the 1.1 patch. **Do not** add `#![deny(unsafe_code)]` to daemon source files — the workspace `unsafe_code = "forbid"` is already active. Adding the attribute will cause `clippy::duplicated_attributes` lint failures.

**Error module contract:** Every crate's `src/error.rs` must contain exactly:
```rust
#[derive(Debug, thiserror::Error)]
pub enum Error { /* variants */ }
pub type Result<T> = std::result::Result<T, Error>;
```
The daemon's current `src/main.rs` is just `#[tokio::main] async fn main() {}` — you need to create `src/error.rs` from scratch with `#[error("io error: {0}")] Io(#[from] std::io::Error)`, `#[error("sqlite error: {0}")] Sqlite(#[from] rusqlite::Error)`, `#[error("pool error: {0}")] Pool(String)`, `#[error("migration error: {0}")] Migration(String)` etc. The error module is extended by later stories.

**`anyhow::Context` boundary:** Permitted **only** in `main.rs` files (the binary entry points). All other daemon source files (`db.rs`, `state.rs`, `api/*`, `projection/*`, etc.) must use `thiserror`-based `Error` types and `?` propagation. **Do not** add `anyhow` `use` statements to internal modules.

### SQLite Schema (V1)

```sql
CREATE TABLE events (
    event_id   INTEGER PRIMARY KEY AUTOINCREMENT,
    source     TEXT    NOT NULL,
    session_id TEXT    NOT NULL,
    kind       TEXT    NOT NULL,         -- EventKind serialized as PascalCase
    reaction   TEXT,                     -- nullable; NULL for daemon sentinels
    payload    TEXT    NOT NULL,         -- verbatim raw JSON; no parsing
    created_at INTEGER NOT NULL          -- Unix milliseconds
);

CREATE TABLE session_projections (
    source     TEXT    NOT NULL,
    session_id TEXT    NOT NULL,
    state      TEXT    NOT NULL,         -- JSON blob; structure defined in Story 1.6
    updated_at INTEGER NOT NULL,
    PRIMARY KEY (source, session_id)
);

-- Shadow table; never truncated; enables history_begins_cleanly after future truncation
CREATE TABLE recording_sessions (
    id               INTEGER PRIMARY KEY AUTOINCREMENT,
    started_event_id INTEGER NOT NULL,
    ended_event_id   INTEGER             -- NULL until clean shutdown
);
```

**For Story 1.2:** All three tables are created in V1 migration. `session_projections.state` is populated with a placeholder `'{}'` JSON blob in this story — the real state machine projection lands in Story 1.6.

**`event_id` INSERT rule (load-bearing):** Always **omit** the `event_id` column from `INSERT INTO events`. Never pass `0` or any explicit value. AUTOINCREMENT assigns. The schema deliberately has no `DEFAULT` on this column to make accidental explicit-zero inserts fail loudly.

[Source: docs/bmad/planning-artifacts/architecture.md#Data Architecture]

### PRAGMA Enforcement Pattern (AC#2)

**The non-negotiable rule:** Every SQLite connection — writer or reader, fresh or returned-to-pool — must have `foreign_keys=ON`, `journal_mode=wal`, `synchronous=NORMAL`, and `busy_timeout=5000` set **before** any application query runs.

`busy_timeout=5000` (ms) is included in the bundle to avoid `SQLITE_BUSY` under brief lock contention — operationally important for any future writer/reader interleaving, and harmless on the single-writer pool today. The factory **also asserts** that `PRAGMA journal_mode` actually returned `"wal"` after the batch ran, refusing the connection otherwise — this guards against read-only or network filesystems where the pragma silently no-ops and leaves the database in `delete` mode.

**Mechanism:** Use `deadpool::managed::Hook` on the pool builder, wired to **both** `post_create` and `post_recycle` so the bundle re-runs on every checkout — literal AC#2 conformance. Configure `Pool::timeouts.wait = Some(Duration::from_secs(5))` so a starved pool returns a typed `PoolError::Timeout(TimeoutType::Wait)` rather than blocking forever.

```rust
use std::time::Duration;
use deadpool_sqlite::{Config, Hook, HookError, Runtime, Timeouts};

const PRAGMA_BUNDLE: &str = "
    PRAGMA foreign_keys = ON;
    PRAGMA journal_mode = WAL;
    PRAGMA synchronous = NORMAL;
    PRAGMA busy_timeout = 5000;
";

let pool = Config::new(db_path)
    .builder(Runtime::Tokio1)?
    .max_size(1)
    .timeouts(Timeouts { wait: Some(Duration::from_secs(5)), create: None, recycle: None })
    .post_create(Hook::async_fn(|w, _| Box::pin(apply_pragmas(w))))
    .post_recycle(Hook::async_fn(|w, _| Box::pin(apply_pragmas(w))))
    .build()?;
// apply_pragmas runs execute_batch(PRAGMA_BUNDLE), then queries journal_mode and
// returns HookError::Backend if the result is not "wal".
```

**Verify pragmas via test (AC#2):** Inside `contract_daemon.rs`, after every checkout, run:
```rust
let (fk, jm, sync): (i64, String, i64) = conn.interact(|c| -> rusqlite::Result<_> {
    Ok((
        c.query_row("PRAGMA foreign_keys", [], |r| r.get(0))?,
        c.query_row("PRAGMA journal_mode", [], |r| r.get(0))?,
        c.query_row("PRAGMA synchronous", [], |r| r.get(0))?,
    ))
}).await??;
assert_eq!((fk, jm.as_str(), sync), (1, "wal", 1));
```

**Note on `journal_mode = WAL`:** Running `PRAGMA journal_mode = WAL` on a fresh SQLite DB returns `"wal"`. Running it on an existing non-WAL DB also flips it to WAL. The pragma is idempotent — running it on every checkout is safe and cheap.

**Note on `synchronous = NORMAL` + WAL:** This combination gives crash-safety for committed transactions (the WAL log is fsync'd) without per-statement fsync. It is the documented SQLite WAL-mode default for "the right balance of safety and speed." Per SQLite docs and architecture decision — see [Source: docs/bmad/planning-artifacts/architecture.md#Data Architecture].

### Projection UPSERT Pattern (Load-Bearing Transaction Invariant)

The projection module **owns the transaction**. Exactly these two statements run inside it; nothing else joins:

```rust
// Inside conn.interact(|c| { let tx = c.transaction()?; ... tx.commit()?; ... })
tx.execute(
    UPSERT_SESSION_PROJECTION,
    rusqlite::params![source, session_id, state_json, now_ms],
)?;
let _ = tx.execute(
    INSERT_EVENT,
    rusqlite::params![source, session_id, kind_str, reaction_str, payload, now_ms],
)?;
let event_id = tx.last_insert_rowid();
tx.commit()?;
```

Where `UPSERT_SESSION_PROJECTION` is:
```sql
INSERT INTO session_projections (source, session_id, state, updated_at)
VALUES (?, ?, ?, ?)
ON CONFLICT(source, session_id)
DO UPDATE SET state = excluded.state, updated_at = excluded.updated_at;
```

**Forbidden patterns (prohibited by architecture):**
- Wrapping the UPSERT + INSERT in a larger transaction that includes anything else
- Splitting the UPSERT and INSERT across two transactions
- Running either statement outside `projection::session::write()`
- Calling `projection::session::write()` from anywhere other than the ingest pipeline (which lands in Story 1.3)

[Source: docs/bmad/planning-artifacts/architecture.md#Process Conventions]

### EventKind & Reaction Serialization (Use Existing protocol Crate Types)

**Do not redefine these.** They are already in `crates/protocol/src/event.rs` and `reaction.rs`. Import via `use protocol::{EventKind, Reaction, EventEnvelope, EventId};` from the crate root only — never from internal submodule paths.

**For `INSERT_EVENT`, serialize:**
- `kind: EventKind` → `serde_json::to_string(&kind)?` produces `"\"PreToolUse\""` (a JSON-encoded string). Strip the outer quotes before storing as `TEXT`, **or** use `format!("{:?}", kind)` (Debug derives match the variant names exactly: `"PreToolUse"`).
  - **Recommended:** add a helper `pub fn event_kind_as_str(k: &EventKind) -> &'static str` in `crates/daemon/src/db/queries.rs` returning the bare string. This avoids serde round-trip on the hot path.
- `reaction: Option<Reaction>` → serialize via `serde_json::to_string()` (strip quotes) when `Some`; SQL `NULL` when `None`. Daemon sentinel events (`RecordingStarted`, `RecordingEnded`) have `reaction: None`.
- `payload: String` is stored verbatim in the `payload TEXT NOT NULL` column. No parsing. No re-encoding.

**EventKind is PascalCase-as-written** — `"PreToolUse"`, `"RecordingStarted"`, etc. **No `rename_all`.** This is enforced by the protocol contract tests in 1.1; daemon code must match the same wire strings when writing to / reading from SQLite.

[Source: crates/protocol/src/event.rs, crates/protocol/tests/contract_protocol.rs]

### Connection Factory Policy (AC#6)

**`rusqlite::Connection::open` is permitted in exactly one file:** `crates/daemon/src/db/pool.rs`. Every other call site must take a connection from `DbPools.writer` or `DbPools.reader`. This is enforced by:

1. A CI step (Task 9) running `grep -rn "rusqlite::Connection::open" crates/daemon/src/ | grep -v "crates/daemon/src/db/pool.rs"` — non-zero exit fails the build
2. Code review

**Why:** WAL mode + PRAGMA bundle must be applied to every connection. Direct `Connection::open` bypasses the post-create hook and silently produces a connection with wrong pragmas — corrupting the durability guarantee.

Test-only exception: tests in `crates/daemon/tests/` may use `Connection::open` for setup (e.g., manually setting `PRAGMA user_version` to simulate migration failure). The CI lint scans `crates/daemon/src/` only, not `tests/`.

### Tracing Format (AC#7)

Default level is `error`. Flag mapping:
- (no flag) → `error`
- `-v` → `info`
- `-vv` → `debug`
- `-vvv` → `trace` (allowed but undocumented)

Per-line format: `<ISO8601 timestamp> <LEVEL> <message>`. Example: `2026-05-17T14:23:01.123Z ERROR migration failed: malformed user_version`

**Implementation:**
```rust
use tracing_subscriber::{fmt, EnvFilter, prelude::*};

let level = match verbosity {
    0 => "error",
    1 => "info",
    2 => "debug",
    _ => "trace",
};

tracing_subscriber::registry()
    .with(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(level)))
    .with(fmt::layer()
        .with_target(false)        // no module path
        .with_level(true)
        .with_ansi(false)
        .with_timer(fmt::time::ChronoUtc::rfc_3339())) // ISO8601
    .init();
```

`RUST_LOG=...` env var overrides the CLI flag when present (standard `tracing-subscriber` behavior; preserve this).

**Note:** `tracing-subscriber 0.3.20` may require enabling the `chrono` feature. Check `Cargo.toml` workspace deps — if `tracing-subscriber` is declared without features, you may need to depend on `tracing-subscriber = { workspace = true, features = ["chrono", "env-filter"] }` in the daemon crate. Verify and adjust.

[Source: docs/bmad/planning-artifacts/prd.md NFR16]

### Panic Hook (AC#8)

```rust
use std::panic;

let bowerbird_dir = home_dir.join(".bowerbird");
std::fs::create_dir_all(&bowerbird_dir)?;
#[cfg(unix)] {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&bowerbird_dir, std::fs::Permissions::from_mode(0o700))?;
}

let crash_dir = bowerbird_dir.clone();
panic::set_hook(Box::new(move |info| {
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis()).unwrap_or(0);
    let crash_path = crash_dir.join(format!("crash-{}.log", now_ms));
    let backtrace = std::backtrace::Backtrace::capture();
    let _ = std::fs::write(
        &crash_path,
        format!("PANIC at {}\n{}\nBacktrace:\n{}\n",
            info.location().map(|l| l.to_string()).unwrap_or_else(|| "<unknown>".into()),
            info.payload().downcast_ref::<&str>().copied()
                .or_else(|| info.payload().downcast_ref::<String>().map(|s| s.as_str()))
                .unwrap_or("<non-string panic payload>"),
            backtrace),
    );
    #[cfg(unix)] {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&crash_path, std::fs::Permissions::from_mode(0o600));
    }
}));
```

**No external crash reporting.** No `sentry`, no `panic-handler::log`, no HTTP POST. Local file only.

### CLI Verbosity Parsing

The workspace root has a `bowerbird` CLI binary (`src/main.rs`) — but that's the user-facing CLI (Story 3.2 work). For Story 1.2, the daemon binary lives at `crates/daemon/src/main.rs` with its own `clap` parser:

```rust
use clap::Parser;

#[derive(Parser)]
#[command(name = "bowerbird-daemon")]
struct Args {
    /// Verbosity: -v info, -vv debug, -vvv trace
    #[arg(short, action = clap::ArgAction::Count)]
    verbose: u8,
}
```

Add `clap = { workspace = true }` to `crates/daemon/Cargo.toml` if not already present — check first (it's already declared at the workspace level in `Cargo.toml:26`).

### Daemon main.rs Startup Sequence (Exact Order)

The order below is load-bearing. Reordering risks (a) crashes after `axum::serve` starts, before crash hook is installed; or (b) `/readyz` returning 200 before migrations are actually done.

1. Parse CLI args
2. Compute `bowerbird_dir = ~/.bowerbird/`; create with mode 0700
3. Install **panic hook** (writes to `bowerbird_dir`) — must be first so any subsequent panic is captured
4. Initialize **tracing** at the level selected by `-v`/`-vv`
5. Initialize **`AppState.migrations_complete = AtomicBool::new(false)`** and `shutdown = CancellationToken::new()`
6. Initialize **`DbPools`** (`init_pools(db_path).await?`) — runs the post-create hooks but doesn't yet migrate
7. **Run migrations** on the writer pool via `Migrations.to_latest(&conn)`; on success set `migrations_complete = true`; on failure `eprintln!` + `exit(1)`
8. Emit `EventKind::RecordingStarted` via `projection::session::write()` — this is the first row in the events table on every cold start
9. **Bind axum listener** on `config.bind_addr`
10. **Serve** via `axum::serve(listener, router).with_graceful_shutdown(...)`
11. On signal: emit `EventKind::RecordingEnded`, run `PRAGMA wal_checkpoint(PASSIVE)` once, exit 0

**Sentinel `session_id`:** Use a reserved literal like `"daemon"` (with `source: "daemon"`). [Source: docs/bmad/planning-artifacts/architecture.md#Open Questions — Resolved → OQ#1]

### Tokio Runtime Constraint (Architectural Mandate)

```rust
#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> { ... }
```

**NOT the default multi-thread runtime.** The architecture mandates `current_thread`:
- No work-stealing overhead — sufficient for local-tool load
- All daemon work runs on a single OS thread
- All SQLite access flows through `deadpool-sqlite` (which `spawn_blocking`s under the hood) — agents must **never** introduce raw `std::thread::spawn` for SQLite work

The 1.1 workspace `Cargo.toml` already has tokio features `rt, rt-multi-thread, macros, net, io-util, sync, time, signal, fs` — keep `rt-multi-thread` enabled because `axum::serve` and integration tests may want it during build, but the production runtime is `current_thread`.

[Source: docs/bmad/planning-artifacts/architecture.md#API & Communication Patterns, #Coherence Validation]

### File Structure to Create

```
crates/daemon/
├── Cargo.toml                       # already exists; may need clap added
├── src/
│   ├── main.rs                      # OVERWRITE existing stub
│   ├── error.rs                     # NEW — Error enum + Result
│   ├── config.rs                    # NEW
│   ├── state.rs                     # NEW
│   ├── db/
│   │   ├── mod.rs                   # NEW — re-exports
│   │   ├── migrations.rs            # NEW — rusqlite_migration definitions
│   │   ├── pool.rs                  # NEW — SOLE Connection::open call site
│   │   └── queries.rs               # NEW — all SQL strings
│   ├── projection/
│   │   ├── mod.rs                   # NEW
│   │   └── session.rs               # NEW — owns the transaction
│   └── api/
│       ├── mod.rs                   # NEW — fn router(state) -> Router
│       └── health.rs                # NEW — /healthz, /readyz
└── tests/
    └── contract_daemon.rs           # NEW — all 8 contract tests in Task 8
```

**Do not** create `ingest/`, `broadcast/`, `auth.rs`, or `api/sessions.rs|events.rs|ws.rs` in this story — those land in Stories 1.3 / 1.5 / 1.7 / 2.x / 3.x respectively, per the directory map.

[Source: docs/bmad/planning-artifacts/architecture.md#Complete Project Directory Structure]

### Anti-Patterns to Avoid (Hard Bans)

- `rusqlite::Connection::open(...)` outside `crates/daemon/src/db/pool.rs` — fails CI lint (Task 9)
- `unwrap()` or `expect()` outside `#[cfg(test)]` code — fails clippy
- `eprintln!` or `println!` anywhere except: (a) `crates/daemon/src/main.rs` for the migration / dir-creation / HOME-validation failure path, and (b) `crates/daemon/src/lib.rs::init_tracing` for tracing-bootstrap failure reporting (malformed `RUST_LOG` warning, failed `try_init`) — these are the only sanctioned exceptions, justified by the chicken-and-egg of needing a way to report tracing-setup failures before tracing is up
- `anyhow::Context` outside `main.rs` — all internal modules use `thiserror`-based `Error`
- Splitting the projection UPSERT and event INSERT across two transactions
- Wrapping the projection transaction inside a larger transaction
- Adding `deny_unknown_fields` to any outbound (daemon→client) type — preserved from 1.1
- Importing internal submodule paths from `crates/protocol` (use `protocol::EventKind`, not `protocol::event::EventKind`)
- Hardcoding `~/.bowerbird/bower.db` outside `config.rs` — all paths flow from `Config`
- Using `tokio::main` without `flavor = "current_thread"`
- Adding `#![deny(unsafe_code)]` to daemon source files — workspace `forbid` is already active; redundant attribute fails clippy
- Adding fixed Unix socket paths in tests — none expected in this story, but flag if tempted
- Using `process::exit` anywhere except `main.rs` migration/dir-creation failure path

### Testing Standards

- **Unit tests** (`#[cfg(test)]` at bottom of source files): permitted but not required for this story — most behavior is integration-shaped
- **Contract tests** live in `crates/daemon/tests/contract_daemon.rs` — pre-MVP gates, must all pass
- **`:memory:` SQLite for tests where possible**; **file-backed via `tempfile::TempDir`** for WAL durability and migration-failure tests (per architecture: "Contract tests testing WAL behavior specifically may use a file-backed SQLite in a `TempDir`")
- **Never** use a fixed file path in tests — parallel tests would collide. Always `tempfile::TempDir`
- Use `tokio::test(flavor = "current_thread")` to match production runtime
- For tests that spawn the daemon binary: use `assert_cmd::Command::cargo_bin("bowerbird-daemon")` — add `assert_cmd` to `[dev-dependencies]` in `crates/daemon/Cargo.toml`. Latest stable as of cutoff: `assert_cmd = "2.0"`

[Source: docs/bmad/planning-artifacts/architecture.md#Structural Conventions, #Process Conventions]

### Sentinel Events: RecordingStarted / RecordingEnded

Per OQ#1 resolution, these are normal events with `source: "daemon"`, `kind: EventKind::RecordingStarted | RecordingEnded`, `reaction: None`, `payload: "{}"` (verbatim JSON). They occupy normal `event_id` slots from AUTOINCREMENT. No special-casing. Story 1.6 (`session projection and hook unreliability tolerance`) builds gap-detection logic on top of these; Story 1.2 just emits them at the right places.

Also write to `recording_sessions` table on each lifecycle:
- On startup, after the `RecordingStarted` event INSERT: `INSERT INTO recording_sessions (started_event_id, ended_event_id) VALUES (?, NULL)` — pass the just-assigned `event_id`
- On clean shutdown, after `RecordingEnded` event INSERT: `UPDATE recording_sessions SET ended_event_id = ? WHERE id = (SELECT MAX(id) FROM recording_sessions)` — pairs the most recent open session
- On crash, `ended_event_id` remains NULL — this is the mechanical fingerprint used by `history_begins_cleanly` in Story 1.6

**Important:** The `recording_sessions` writes are **separate** transactions from the event INSERT — they are bookkeeping, not part of the load-bearing projection transaction. Sequence the `recording_sessions` write **after** the event commit, never before.

[Source: docs/bmad/planning-artifacts/architecture.md#Open Questions — Resolved → OQ#1, OQ#2]

### Previous Story Intelligence (1.1)

**Test placement convention** (established in 1.1): contract tests live in `crates/<name>/tests/contract_<name>.rs`. Follow this for `contract_daemon.rs`.

**Lint config pattern** (established in 1.1): every member crate has `[lints] workspace = true` in its `Cargo.toml`. `crates/daemon/Cargo.toml` already has this (line 27-28).

**Format of dev notes / completion notes** (established in 1.1): use bullet lists with specific file/line references for review findings. Each deferred item gets its own bullet with `[Source: <file>:<line>]`.

**rust-toolchain.toml** is pinned to `1.94.1` with `components = ["rustfmt", "clippy"]` (1.1 patch). Do not modify.

**CI matrix** runs on `macos-latest` and `ubuntu-latest`. Tests must pass on both. SQLite WAL behaves identically across these platforms — no platform-specific code needed.

### Git Intelligence (Recent Commits)

- `293c3d3` — Merge PR #3 (architecture work)
- `96c083b`, `98b131c`, `c96c100` — Story 1.1 review patches: workspace lint inheritance, CI components, rustfmt pin
- `f54689e` — Story 1.1 main implementation
- `7909b66` — `.gitignore` for Rust build artifacts (verify it ignores `target/`, `Cargo.lock` is **NOT** ignored — Cargo.lock IS committed per architecture)

**No SQLite-related code committed yet.** Story 1.2 is the first SQLite touch.

### Latest Tech Information

- **rusqlite 0.38.0** — current production version in workspace. Bundled SQLite ≥ 3.45. `transaction()` API is stable. `last_insert_rowid()` returns `i64` (matches `EventId`).
- **deadpool-sqlite 0.13.0** — uses `deadpool 0.10`. `Hook::async_fn` signature is `(Object, Metrics) -> impl Future`. The `Object` deref-target is `rusqlite::Connection`. `interact()` returns `Result<T, InteractError>` — wrap appropriately in the daemon `Error` enum.
- **rusqlite_migration 2.4.1** — `Migrations::new(vec![M::up("CREATE TABLE ...")])`. `to_latest(&mut conn)` runs all pending migrations. Migration scripts run inside a transaction by default. Use a single `M::up(...)` for the V1 bundle (all three tables in one migration) — append-only; future stories add new `M::up()` entries.
- **axum 0.8.9** — `Router::new().route("/healthz", get(handler))` API. `axum::serve(listener, router).with_graceful_shutdown(future).await` is the canonical shutdown pattern.
- **tokio-util 0.7.18** — `CancellationToken` for cooperative shutdown.

### Project Structure Notes

- All paths align with the directory map in `docs/bmad/planning-artifacts/architecture.md#Complete Project Directory Structure`. No deviations.
- New modules (`db/`, `projection/`, `api/`) are nested under `crates/daemon/src/` — match the architecture's directory tree exactly.
- The workspace root `bowerbird` CLI binary stub (`src/main.rs`) is **unchanged** by this story; Story 1.2 only touches `crates/daemon/` and CI config.

### References

- Story AC: [Source: docs/bmad/planning-artifacts/epics.md#Story 1.2]
- WAL durability & schema: [Source: docs/bmad/planning-artifacts/architecture.md#Data Architecture]
- Pool sizing & PRAGMA policy: [Source: docs/bmad/planning-artifacts/architecture.md#Data Architecture]
- Transaction invariant & projection UPSERT: [Source: docs/bmad/planning-artifacts/architecture.md#Process Conventions]
- Tokio current_thread mandate: [Source: docs/bmad/planning-artifacts/architecture.md#API & Communication Patterns]
- Directory structure: [Source: docs/bmad/planning-artifacts/architecture.md#Complete Project Directory Structure]
- Sentinel events & OQ#1 fire-and-forget: [Source: docs/bmad/planning-artifacts/architecture.md#Open Questions — Resolved]
- Recording sessions shadow table: [Source: docs/bmad/planning-artifacts/architecture.md#Open Questions — Resolved → OQ#2]
- NFR6, NFR16, NFR17, NFR21: [Source: docs/bmad/planning-artifacts/prd.md#Reliability & Data Integrity, #Operability, #Implementation Constraints]
- Naming conventions (snake_case columns, PascalCase enum variants): [Source: docs/bmad/planning-artifacts/architecture.md#Naming Conventions]
- Error module contract: [Source: docs/bmad/planning-artifacts/architecture.md#Structural Conventions]
- Anti-patterns list: [Source: docs/bmad/planning-artifacts/architecture.md#Enforcement Guidelines]
- Story 1.1 dev notes (dependency conflict, lint inheritance): [Source: docs/bmad/implementation-artifacts/1-1-workspace-and-protocol-crate-foundation.md#Debug Log References, #Patches]
- Workspace Cargo.toml dep pins: [Source: Cargo.toml]
- Protocol types: [Source: crates/protocol/src/event.rs, crates/protocol/src/reaction.rs]

## Dev Agent Record

### Agent Model Used

claude-opus-4-7[1m] via bmad-dev-story workflow.

### Debug Log References

- **Test failure (resolved):** `wal_durability_after_simulated_crash` initially failed with `no such column: event_id` because line-continuation backslashes in Rust string literals strip leading whitespace and concatenate without a space — `"created_at\"` followed by `"    FROM events"` produced `"created_atFROM events"`. Fix: replaced all `\<newline>` continuations in [`db/queries.rs`](../../crates/daemon/src/db/queries.rs) and [`db/migrations.rs`](../../crates/daemon/src/db/migrations.rs) with explicit trailing spaces or multi-line raw strings.
- **Compile failure (resolved):** `tower::ServiceExt::oneshot` is needed in the axum integration test path; added `tower` (with `util` feature) to workspace deps and daemon `[dev-dependencies]`.
- **Smoke test:** ran `HOME=/tmp/bowerbird-smoke timeout 2s bowerbird-daemon -v` against an empty home; observed (a) `~/.bowerbird/` created with mode `0700`, (b) `bower.db` created, (c) ISO 8601 log lines at INFO level, (d) `RecordingStarted` written as `event_id=1`, (e) bind to ephemeral `127.0.0.1` port, (f) clean SIGTERM handling with `RecordingEnded` as `event_id=2`.

### Completion Notes List

- **Library-target split.** Added `crates/daemon/src/lib.rs` exposing every module as `pub`. The binary at `crates/daemon/src/main.rs` is a thin orchestrator that calls into the library. This lets `tests/contract_daemon.rs` import `init_pools`, `run_migrations`, `projection::session::write`, `api::router`, etc. without re-exposing private internals.
- **Connection factory enforcement.** `rusqlite::Connection::open` appears in zero files under `crates/daemon/src/` (the production code uses only `deadpool_sqlite::Pool`). The contract test `connection_factory_policy_lint_passes` walks the source tree and asserts this, and [`scripts/lint-db-access.sh`](../../scripts/lint-db-access.sh) provides the CI-side check.
- **PRAGMA hook.** The post-create hook executes `PRAGMA foreign_keys = ON; PRAGMA journal_mode = WAL; PRAGMA synchronous = NORMAL; PRAGMA busy_timeout = 5000;` on every fresh connection. AC#2's three pragmas are validated by `pragmas_on_every_{writer,reader}_checkout`; `busy_timeout` is set as well so that brief reader/writer contention surfaces as wait time, not as `SQLITE_BUSY`.
- **Projection transaction discipline.** [`projection::session::write`](../../crates/daemon/src/projection/session.rs) opens exactly one `conn.transaction()` containing exactly two statements (`UPSERT_SESSION_PROJECTION` then `INSERT_EVENT`) and reads `event_id` via `tx.last_insert_rowid()` before commit. No savepoints, no other statements. `recording_sessions` bookkeeping is a separate transaction, sequenced after the event commit, as required by Dev Notes.
- **EventKind serialization.** Used the `event_kind_as_str` helper (returns `&'static str`) to avoid the serde round-trip on every write. The strings match the protocol's PascalCase serde rep verified by `contract_protocol::event_kind_serializes_pascal_case`.
- **Tracing format.** `tracing-subscriber` registered with `ChronoUtc::rfc_3339()` (RFC 3339 / ISO 8601 subset), `with_target(false)`, `with_level(true)`, ANSI disabled, level controlled by `-v` flag (`error` → `info` → `debug` → `trace`). `RUST_LOG` overrides via `EnvFilter::try_from_default_env`.
- **Panic hook & crash dir.** [`install_panic_hook`](../../crates/daemon/src/lib.rs) writes `crash-<unix_ms>.log` to `~/.bowerbird/` with `0o600` perms; the directory is created with `0o700`. No external crash reporting.
- **Sentinel events.** `RecordingStarted` is written as the first row immediately after migrations complete and before axum starts serving; `RecordingEnded` is written from the post-`axum::serve` shutdown path and the matching `recording_sessions` row is closed before the final `PRAGMA wal_checkpoint(PASSIVE)`. On crash, `ended_event_id` remains `NULL`, providing the fingerprint Story 1.6 needs.
- **Architecture conformance.** `#[tokio::main(flavor = "current_thread")]` per architecture mandate. `#![deny(unsafe_code)]` is provided by the workspace `[workspace.lints]` table — not redeclared per-crate. `anyhow::Context` is used only in `main.rs`; all internal modules use `thiserror`-based `Error`/`Result`.
- **Deferred contract.** Implemented the "pool starvation returns a defined error" contract test (`pool_starvation_returns_defined_error`) using `tokio::time::timeout` — deadpool's `get` blocks indefinitely on starvation, but the test asserts the *caller* can bound waits with the standard timeout API. The architecture-level contract is satisfied.

### File List

**New files:**
- `crates/daemon/src/lib.rs`
- `crates/daemon/src/error.rs`
- `crates/daemon/src/config.rs`
- `crates/daemon/src/state.rs`
- `crates/daemon/src/db/mod.rs`
- `crates/daemon/src/db/pool.rs`
- `crates/daemon/src/db/migrations.rs`
- `crates/daemon/src/db/queries.rs`
- `crates/daemon/src/projection/mod.rs`
- `crates/daemon/src/projection/session.rs`
- `crates/daemon/src/api/mod.rs`
- `crates/daemon/src/api/health.rs`
- `crates/daemon/tests/contract_daemon.rs`
- `crates/daemon/README.md`
- `scripts/lint-db-access.sh`

**Modified files:**
- `Cargo.toml` (workspace) — added `assert_cmd`, `tower` to workspace deps
- `crates/daemon/Cargo.toml` — added `clap`, `serde`, `serde_json` deps; `tracing-subscriber` features `chrono` + `env-filter`; new `[dev-dependencies]` block with `tempfile`, `assert_cmd`, `tokio`, `tower`
- `crates/daemon/src/main.rs` — rewritten from `#[tokio::main] async fn main() {}` to full startup sequence
- `.github/workflows/ci.yml` — added `bash scripts/lint-db-access.sh` step

## Change Log

- 2026-05-17: Story implemented via bmad-dev-story workflow. All 10 tasks complete; 9 contract tests pass (the 8 specified plus a `connection_factory_policy_lint_passes` test mirroring the CI lint). `cargo build --workspace`, `cargo clippy --all-targets --workspace -- -D warnings`, `cargo fmt --check`, `cargo test --workspace`, and `scripts/lint-db-access.sh` all green. Smoke-tested daemon binary end-to-end (cold start → migrate → RecordingStarted event_id=1 → listen → SIGTERM → RecordingEnded event_id=2 → wal_checkpoint).
- 2026-05-17: Story created via bmad-create-story workflow. Comprehensive context engine analysis completed: epic ACs extracted, architecture sections (Data Architecture, Process Conventions, Directory Structure, OQ#1/#2 resolutions) inlined, Story 1.1 dependency conflict and lint-inheritance learnings carried forward, library-version pins reconciled against actual workspace Cargo.toml, 8 contract tests scoped, anti-patterns and file-structure guardrails enumerated.
