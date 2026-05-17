# Deferred Work

## Deferred from: code review of 1-1-workspace-and-protocol-crate-foundation (2026-05-17)

- **SourceAdapter `&'static str` rigidity** — `AdapterMeta.source` is `&'static str`, preventing runtime-configured or dynamically loaded adapters. Consider a newtype or owned `String` when adapter discovery is implemented. [crates/protocol/src/adapter.rs]
- **Payload String no schema enforcement** — `EventEnvelope.payload` and `Event.payload` are opaque `String` with no validation that content is valid JSON or matches `kind`. Consider per-kind typed payload enums or validation in the adapter layer. [crates/protocol/src/event.rs]
- **Reaction::Vendor error message ambiguity** — `Vendor(65536)` (u16 overflow) and `Vendor(abc)` (non-numeric) produce the same error string. Add distinct error messages distinguishing parse failure vs range overflow. [crates/protocol/src/reaction.rs]
- **SyncFrame ordering not validated** — No guard that `oldest_available_event_id <= latest_event_id`. Add validation in daemon when SyncFrame is constructed. [crates/protocol/src/ws.rs]
- **DroppedFrame invariants not validated** — `count`, `first_dropped_event_id`, `last_dropped_event_id` have no relational guards. Add validation in daemon when DroppedFrame is constructed. [crates/protocol/src/ws.rs]
- **ClientMessage empty topic accepted** — `Subscribe { topic: String }` accepts empty strings. Add non-empty validation in daemon topic routing. [crates/protocol/src/ws.rs]
- **CI matrix omits Windows** — `keyring` has a Windows backend; add a Windows runner when Windows is a supported target. [.github/workflows/ci.yml]

## Deferred from: code review of 1-2-daemon-foundation-with-sqlite-persistence (2026-05-17)

- **SIGKILL / `exit(1)` paths skip the `RecordingEnded` sentinel + WAL checkpoint** — covered by Story 1.6's gap-detection design; revisit when implementing recovery
- **`event_kind_as_str` ↔ serde equivalence untested** — `crates/daemon/src/db/queries.rs` `event_kind_as_str` hand-mirrors `protocol::EventKind` serde; add exhaustive equivalence test so renames stay in sync
- **Migration idempotency on a populated DB is untested** — `crates/daemon/src/db/migrations.rs` `run_migrations` only tested on a fresh tempdir; add a re-run-on-populated test
- **`Pool::interact` errors collapse to opaque strings, losing cause chain** — `crates/daemon/src/db/migrations.rs:648`, `crates/daemon/src/projection/session.rs:1145`; preserve the deadpool error chain for diagnostics
- **`migration_failure_exits_nonzero` could hang for 20s if a regression lets the daemon survive** — `crates/daemon/tests/contract_daemon.rs:1347-1352`; tighten or assert quick exit
- **No tests for `install_panic_hook` or `init_tracing`** — both have non-trivial behavior (file mode, payload downcasting, verbosity mapping). Add after the panic-hook patches land
- **CLI surface: no `--db-path`, `--bind-addr`, `--config`, `--version`** — `crates/daemon/src/main.rs:904-910`; explicitly out of scope for Story 1.2
- **`init_pools` does not validate `db_path` parent exists / is writable** — `crates/daemon/src/db/pool.rs`; SQLite returns a reasonable error at first checkout, so cosmetic
- **`i64::try_from(u128)` timestamp overflow at year 292278994 AD** — `crates/daemon/src/projection/session.rs`; far-future, but the patch in this round will surface it as a typed error so the deferred work is just a test
- **`wal_durability_after_simulated_crash` uses `drop(pool)` not a true subprocess crash** — `crates/daemon/tests/contract_daemon.rs:60-95`; AC#1 acknowledges this. Follow-up: spawn-then-kill subprocess test
- **`migration_failure_exits_nonzero` TempDir cleanup vs daemon panic-write race** — `crates/daemon/tests/contract_daemon.rs:148-176`; narrow but a flaky-test source
- **`scripts/lint-db-access.sh` bypassable via aliased imports / BSD grep symlink behavior** — `scripts/lint-db-access.sh`; spec already calls out a clippy-based replacement as follow-up
- **`tokio::signal::unix::signal(...)` registration failure is not logged** — `crates/daemon/src/main.rs` `shutdown_signal`; on a sandboxed/non-unix env the daemon would silently lose SIGTERM
- **`migration_failure_exits_nonzero` does not assert "before accepting any connections"** — `crates/daemon/tests/contract_daemon.rs:1332-1365`; AC#4 wording. Implicit in main.rs ordering, but a regression would not be caught
- **Singleton enforcement (file lock / PID file)** — nothing prevents two daemon instances binding the same `bower.db`; concurrent migrations would race. Deferred to Story 3.1/3.2 (daemon lifecycle CLI); single-user assumption documented for Story 1.2
- **Daemon address discoverability** — `bind_addr` defaults to ephemeral port `:0`, chosen port only visible in logs. Deferred to Story 3.x (bearer-token + lifecycle); patch P20 (bind log at WARN) covers local-dev observability for 1.2
- **Envelope size/format validation in `projection::session::write`** — no length, NULL-byte, or format guards on `source`/`session_id`/`payload`. Deferred to Story 1.3 ingest endpoint; validation belongs at the HTTP/Unix-socket trust boundary, not at the internal projection layer
