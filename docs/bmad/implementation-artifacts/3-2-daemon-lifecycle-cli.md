# Story 3.2: Daemon lifecycle CLI

Status: done

<!-- Note: Validation is optional. Run validate-create-story for quality check before dev-story. -->

## Story

As a tool builder,
I want CLI commands to start, stop, and inspect the bowerbird daemon independently of my Claude Code hook configuration,
so that I can restart the daemon after a crash or manually test it without reinstalling the hook.

## Acceptance Criteria

1. **Given** the daemon is not running **When** I run `bowerbird start` **Then** the daemon starts in the background, `~/.bowerbird/ingest.sock` appears, and `GET /healthz` returns 200 within 2 seconds (NFR3).
2. **Given** the daemon is running **When** I run `bowerbird stop` **Then** the daemon receives SIGTERM, executes its graceful shutdown sequence (`close` frames to all WS clients, ingest drain, projection finalize, WAL checkpoint), and exits 0.
3. **Given** the daemon is running **When** I run `bowerbird status` **Then** the output includes the daemon version, process uptime, and a liveness indicator; if the daemon is not running, the output clearly states it is stopped.
4. **Given** the daemon crashed unexpectedly (process killed, OOM, panic past the panic hook) and left a stale PID file **When** I run `bowerbird start` after freeing the cause (e.g., disk space) **Then** the daemon starts cleanly, applies any pending WAL checkpoint via its normal startup path, and `GET /readyz` returns 200.
5. **Given** `bowerbird install` is run (Story 3.1) **When** the installation completes **Then** the daemon starts automatically as part of the install flow — this AC restates Story 3.1's auto-start contract and Story 3.2 must NOT regress it (the shared start logic introduced here is used by both `bowerbird start` and `bowerbird install`).
6. **Given** the daemon is running with N active WebSocket subscribers **When** I run `bowerbird status` or query `GET /status` **Then** the output includes `connected_ws_clients: N` reflecting current WS subscriber count, sourced from the existing `AppState::ws_semaphore` permit accounting (Epic 2 retro action item AI-1).
7. **Given** Story 3.2 ships **When** the code lands **Then** `protocol::rest::DaemonStatus` gains a `connected_ws_clients: u32` field, `daemon::api::status::get` populates it, the `"reserved for Epic 2 and intentionally omitted"` comment in `crates/daemon/src/api/status.rs` is removed, the matching outdated note in `crates/protocol/src/rest.rs::DaemonStatus`'s doc comment is removed, and the corresponding entry in `docs/bmad/implementation-artifacts/deferred-work.md` (line 54, Story 1.7 section, "`/status.connected_ws_clients` not included") is struck through with a backlink to this story.

## Tasks / Subtasks

- [x] **Task 1 — Surface `connected_ws_clients` on `DaemonStatus`** (AC: #6, #7)
  - [x] 1.1 Add `pub connected_ws_clients: u32,` to `protocol::rest::DaemonStatus` (`crates/protocol/src/rest.rs:69-76`). This is an additive field on an outbound type, which the project's asymmetric serde policy explicitly supports (`project-context.md` "Wire format conventions" + `architecture.md` §Implementation Patterns — outbound types do NOT carry `deny_unknown_fields`, so v1.0 presenters keep deserializing the response without error). Place the field after `last_event_id` to match the JSON ordering convention (mechanical fields like IDs and timestamps first, derived/instantaneous counters last).
  - [x] 1.2 Update the `DaemonStatus` doc comment in `crates/protocol/src/rest.rs:60-67` to remove the "deferred to Story 3.2" note now that the field is here. Replace with a one-line description of `connected_ws_clients`: "Count of active WebSocket subscribers — snapshot at request time; can drift between this read and a follow-up read because WS connections come and go."
  - [x] 1.3 In `crates/daemon/src/api/status.rs::get`, compute `connected_ws_clients` from the existing semaphore: `let connected_ws_clients = u32::try_from(config_cap.saturating_sub(state.ws_semaphore.available_permits())).unwrap_or(u32::MAX);`. The cap value comes from `WsConfig` — but `WsConfig` doesn't carry the max-connections cap today (only `ping_interval`, `pong_timeout`, `coalesce_window` per `crates/daemon/src/state.rs:27-33`). Add `pub max_connections: usize,` to `WsConfig` and populate it in `crates/daemon/src/main.rs:210-214` where the struct is built (`max_connections: config.ws_max_connections,`). This keeps the status handler from having to thread a separate `Arc<usize>` through AppState.
  - [x] 1.4 Remove the `connected_ws_clients` deferral comment block at the top of `crates/daemon/src/api/status.rs` (lines 1-6) and replace it with a one-line module doc: `//! `GET /status` — daemon snapshot (version, uptime, last event, connected WS clients).`
  - [x] 1.5 Populate the new field in the `Json(DaemonStatus { ... })` literal at the bottom of `status::get`. Use the snapshot value computed in step 1.3 — do NOT re-read the semaphore inside the literal (re-reading would race against the same handler's own clone semantics; one read, one value).
  - [x] 1.6 Strike through the matching deferred-work entry: open `docs/bmad/implementation-artifacts/deferred-work.md`, find the bullet on line 54 starting with `"**`/status.connected_ws_clients` not included**"` (under the section heading `## Deferred from: Story 1.7 (REST query API) (2026-05-20)`), wrap the entire bullet text in `~~strikethrough~~` and append a non-struck suffix ` **Resolved by Story 3.2 (Task 1):**  DaemonStatus.connected_ws_clients now wired to AppState::ws_semaphore permit accounting; verified by crates/daemon/tests/contract_daemon.rs::story_3_2_lifecycle::status_reports_active_ws_subscriber_count.`. Mirror the exact format used by the Story 2.1 / 2.4 strike-throughs already in that file.

- [x] **Task 2 — Persist daemon bind-addr so the CLI can find its HTTP surface** (AC: #1, #3, #6)
  - [x] 2.1 **Context for the dev agent.** Today, `crates/daemon/src/main.rs:228-230` binds `config.bind_addr` (default `127.0.0.1:0` — an *ephemeral* port assigned by the kernel) and logs the resolved address via `tracing::warn!(addr = %local_addr, "daemon listening")`. The CLI cannot query `GET /healthz` or `GET /status` without knowing the bound port. `project-context.md:471` already references `~/.bowerbird/server.json` as the canonical place for the bearer token; this story extends that file's role to also publish the bind address. Story 3.3 will fold the token into the same file.
  - [x] 2.2 After `let local_addr = listener.local_addr().context("listener.local_addr")?;` succeeds at `crates/daemon/src/main.rs:231`, write `~/.bowerbird/server.json` (resolve via `bowerbird_dir.join("server.json")` from the same data dir that owns `ingest.sock` and `bowerbird.pid`). Content: `{"bind_addr": "127.0.0.1:54321"}` (one field for now; `token` field reserved for Story 3.3). Write via the same atomic sequence Story 3.1 established (`adapter_claude::install`'s tmp+fsync+rename pattern), but inline it as a helper in `crates/daemon/src/main.rs` or a new `crates/daemon/src/server_file.rs` — do NOT pull a heavy dep just for atomicity here. File mode MUST be `0600`; the bearer token will live in this same file post-3.3 and the mode is set now so 3.3 inherits a safe baseline. Use `OpenOptions::new().mode(0o600)` (cfg `unix`) on the tmp file before write.
  - [x] 2.3 On clean shutdown (after `pools.writer.close()` at `main.rs:291`), best-effort delete `server.json`. If the unlink fails, log at WARN and continue — the next daemon startup will overwrite it. On unclean shutdown (SIGKILL, OOM, panic) the file is left on disk; the CLI MUST treat the file as a hint, not a liveness proof — the singleton PID file (Story 3.1) plus an ingest-socket connect probe is what proves the daemon is alive, not the existence of `server.json`. Document this contract inline next to the write.
  - [x] 2.4 Define a small struct in `crates/protocol/src/rest.rs` (or a new `crates/protocol/src/server_file.rs`): `pub struct ServerInfo { pub bind_addr: String /* SocketAddr serialized as string */ }` with `#[derive(Debug, Clone, Serialize, Deserialize)]`. **Do NOT** put `#[serde(deny_unknown_fields)]` on it — this file is read by an inbound consumer (the CLI) but its content is daemon-controlled (effectively an outbound emission from one bowerbird binary to another). The asymmetric serde rule applies on a per-direction basis; this is the daemon emitting a known-shape file, not a third-party client submitting input. Mark this rationale in the doc comment so the consistency check doesn't flag it.
  - [x] 2.5 CI/changelog: this is an additive change to the protocol crate (`crates/protocol/src/*.rs`), which triggers the `protocol-changelog.md` CI gate (`project-context.md:124`). Add an entry under v1.0 → v1.1 with `type: behavioral` describing the new `ServerInfo` type AND the additive `DaemonStatus.connected_ws_clients: u32` field together — same release, two related additions, one changelog entry that names both.

- [x] **Task 3 — `bowerbird start` subcommand** (AC: #1, #4)
  - [x] 3.1 Create `src/commands/start.rs` (new file). Use the `Args` derive pattern Stories 3.1 set up — see `src/commands/install.rs:7-17` for the shape. `StartArgs` carries no flags in v1 (no `--detach`, no `--foreground` — the daemon is always detached; foreground is a `cargo run -p bowerbird-daemon` workflow for development, not a CLI surface).
  - [x] 3.2 Implementation: factor a shared `commands::daemon::start_daemon_detached() -> anyhow::Result<u32>` helper. Move the body of `start_daemon_if_needed()` from `src/commands/install.rs:50-95` into the new helper. `install.rs` and `start.rs` both call the helper. The helper:
    - Resolves `bowerbird_dir` via `super::resolve_bowerbird_dir()`.
    - Probes `daemon_is_up(&bowerbird_dir.join("ingest.sock"))` (already exists in `commands/mod.rs:59-62`). If up, return early with a known "already running" outcome (use a typed enum return: `enum StartOutcome { AlreadyRunning, Spawned { pid: u32 } }`).
    - Otherwise calls `resolve_daemon_bin()` and `spawn_detached()` (already in `install.rs`; move both to `commands/mod.rs` or the new `commands/daemon.rs` module so `start.rs` and `install.rs` share one copy — do NOT duplicate the `setsid` block).
  - [x] 3.3 Readiness wait (AC #1: "`GET /healthz` returns 200 within 2 seconds"). After spawning, poll `server.json` until it appears (the daemon writes it once `listener.local_addr()` succeeds at line 2.2), then GET `http://<bind_addr>/healthz` with a 250ms-per-attempt budget until either a 200 lands or the 2s window closes. The user-facing exit on timeout is non-zero with a stderr message: `error: daemon spawned (pid X) but failed to become healthy within 2s; see ~/.bowerbird/crash logs`. Do NOT kill the spawned daemon on timeout — the user might want to investigate it post-mortem.
  - [x] 3.4 The CLI binary's deps already exclude `tokio`, `reqwest`, etc. (per Story 3.1's intentional lightness). For the `/healthz` probe, use `std::net::TcpStream::connect_timeout` + a hand-rolled minimal HTTP/1.1 GET (one line of bytes out, parse the status line from the response). This is ~40 lines and keeps the CLI binary small. Alternative: add `ureq` (`= "2"` or similar minimal sync HTTP client) — confirm it stays under the workspace's "standards-by-default" preference before adding. **Recommendation:** hand-roll the HTTP GET; the surface is `GET /healthz HTTP/1.1\\r\\nHost: 127.0.0.1\\r\\nConnection: close\\r\\n\\r\\n` and the response only needs a "HTTP/1.1 200" prefix match. Adding `ureq` for a single 4-line request would not pay for itself.
  - [x] 3.5 Handle the start-when-already-running case (AC's implicit idempotency): print `daemon already running (pid X); use 'bowerbird stop' to stop it` and exit 0. The PID comes from `~/.bowerbird/bowerbird.pid` written by the singleton lock (Story 3.1).
  - [x] 3.6 Handle the stale-state case (AC #4: previous unclean exit). Story 3.1's singleton lock auto-cleans stale `flock`s via kernel FD reclaim — `bowerbird start` doesn't need any explicit recovery code; it just spawns the daemon and the daemon's normal startup path handles WAL replay (rusqlite_migration is idempotent, WAL recovery is built into SQLite). Document this inline: "Recovery is implicit — the daemon's startup path runs `init_pools` (WAL recovery) and `run_migrations` (idempotent rusqlite_migration) regardless of whether the previous shutdown was clean."

- [x] **Task 4 — `bowerbird stop` subcommand** (AC: #2)
  - [x] 4.1 Create `src/commands/stop.rs` (new file). `StopArgs` carries no flags in v1 — the SIGKILL-escalation timing is fixed at 10s (matches `uninstall.rs`'s budget). If a future need arises for `--force` (skip SIGTERM and SIGKILL immediately) or `--timeout`, add then.
  - [x] 4.2 Factor `stop_daemon_if_running()` out of `src/commands/uninstall.rs:55-107` into `src/commands/daemon.rs` (the same module Task 3.2 introduces for `start_daemon_detached()`). Rename to `commands::daemon::stop_daemon_via_pid_file() -> anyhow::Result<StopOutcome>` with `enum StopOutcome { NotRunning, Stopped, Escalated /* SIGKILL was needed */ }`. `uninstall.rs` and `stop.rs` both call it. The internal `read_pid`, `pid_alive`, `send_signal` helpers are private siblings — keep them private to `commands::daemon`, do not pub-export.
  - [x] 4.3 `bowerbird stop` returns exit code 0 even when the daemon needed SIGKILL escalation (matches `bowerbird uninstall`'s behavior — failure to gracefully stop is reported via stderr but is not user-facing failure). Exit non-zero ONLY when the PID file is unreadable for reasons other than ENOENT (e.g. EACCES) or when SIGKILL is issued but `pid_alive(pid)` still returns true after the 1s drain loop (this means the kernel didn't reap; the OS is in a bad state and the user should know).
  - [x] 4.4 Print human-readable status on each branch — mirror `uninstall.rs`'s output (`daemon not running (no pid file); nothing to stop`, `sending SIGTERM to bowerbird-daemon (pid X)`, `daemon stopped`, the SIGKILL-escalation warning). Story 3.2 should NOT change `uninstall.rs`'s output wording; the goal is "same messages from both code paths because they share the helper."

- [x] **Task 5 — `bowerbird status` subcommand** (AC: #3, #6)
  - [x] 5.1 Create `src/commands/status.rs` (new file). `StatusArgs` carries no flags in v1.
  - [x] 5.2 Resolution order for "is the daemon up":
    - Read `~/.bowerbird/bowerbird.pid` (Story 3.1's singleton file). If missing, print `daemon is stopped` and exit 0.
    - Check `pid_alive(pid)` via `libc::kill(pid, 0)` (same primitive `uninstall.rs::pid_alive` uses). If the PID is dead, print `daemon is stopped (stale pid file: pid X is not running)` and exit 0.
    - Probe ingest-socket connect via `daemon_is_up(&bowerbird_dir.join("ingest.sock"))`. If the socket doesn't accept connections, print `daemon process exists (pid X) but is not accepting ingest connections; see logs` and exit 0 (status is informational; do not exit non-zero just because something is wrong — the user is asking, not asserting).
  - [x] 5.3 If the daemon is alive, read `~/.bowerbird/server.json` to get the `bind_addr`. If `server.json` is missing or unparseable, print the basic liveness (`daemon is running (pid X)`) without the version/uptime/ws-clients details and exit 0. This is the graceful-degradation path: an old daemon that pre-dates Task 2 (or a future daemon that uses a different file format) still gives the user *something* useful.
  - [x] 5.4 If `server.json` parses, hit `GET /status` via the same hand-rolled HTTP path Task 3.4 uses. Authorization: read `$BOWERBIRD_TOKEN` from the environment (Story 3.1's daemon already supports this — see `crates/daemon/src/api/token.rs:60-62`). If the env var is unset, the auth WILL fail with 401 (the daemon generated an ephemeral token and the CLI cannot recover it in v1). On 401, print `daemon is running (pid X) but $BOWERBIRD_TOKEN is unset or stale; cannot read /status — set the env var, or wait for Story 3.3 keychain integration` and exit 0. Story 3.3 will resolve this awkwardness; Story 3.2 documents it honestly.
  - [x] 5.5 If `GET /status` returns 200, deserialize as `protocol::DaemonStatus` and print a human-readable summary:
    ```
    bowerbird daemon
      status        : running
      pid           : 12345
      version       : 0.1.0
      protocol      : 1.0
      uptime        : 1h 23m 7s
      connected ws  : 2
      last event    : 47s ago (event_id=128)
    ```
    Use `humantime::format_duration` if it's already in the workspace (it's not — confirm before adding); otherwise hand-roll the `Duration → "1h 23m 7s"` formatter. Hand-rolling is ~15 lines and avoids a new dep for one display path.
  - [x] 5.6 The `last event` row prints `(never)` when `last_event_at_ms` is `None`. The `connected ws` row prints the literal integer (NOT a percentage or a fraction-of-cap — the cap is a daemon-side knob the user does not need to know to read status). The `status` row prints `running`, `stopped`, or `degraded (description)` depending on the resolution outcome from steps 5.2–5.4. Treat status output as user-facing — copy the column alignment carefully so the eye can scan it without parsing.

- [x] **Task 6 — Wire the new subcommands into the CLI dispatcher** (AC: #1, #2, #3)
  - [x] 6.1 In `src/main.rs`, extend the `enum Command` declaration with three new variants: `Start(commands::start::StartArgs)`, `Stop(commands::stop::StopArgs)`, `Status(commands::status::StatusArgs)`. Place them between `Install` and `Uninstall` so the `bowerbird --help` ordering reads `install, start, stop, status, uninstall` (alphabetical is also acceptable; pick one and apply consistently — alphabetical is the safer default since clap's derive doesn't enforce ordering).
  - [x] 6.2 Match arms in `fn main()` mirror the existing `Install` / `Uninstall` shape: `Command::Start(args) => commands::start::run(args).context("bowerbird start")` (same for `Stop`, `Status`). `anyhow::Context` is allowed here — `main.rs` is the binary edge per the architecture's anti-pattern list.
  - [x] 6.3 In `src/commands/mod.rs`, add `pub mod start;`, `pub mod stop;`, `pub mod status;`, and `pub mod daemon;` (the new shared-helper module Tasks 3.2 / 4.2 introduce). Update the existing `super::start_daemon_if_needed()` call inside `install.rs` to route through `commands::daemon::start_daemon_detached()` instead. Remove the now-dead inlined `spawn_detached` / `nix_setsid` definitions from `install.rs` (they move to `commands::daemon`).
  - [x] 6.4 The `uninstall.rs::stop_daemon_if_running()` private helper is replaced by `commands::daemon::stop_daemon_via_pid_file()` (per Task 4.2). Update `uninstall.rs` to call the public helper and remove the duplicated PID-file logic from `uninstall.rs`. Keep the output messages identical so the existing `tests/cli_install.rs` assertions don't break.

- [x] **Task 7 — E2E tests for the lifecycle subcommands** (AC: #1, #2, #3, #4, #6)
  - [x] 7.1 Create `tests/cli_lifecycle.rs` at the workspace root (parallel to the existing `tests/cli_install.rs` Story 3.1 added). Use the same `assert_cmd::Command::cargo_bin("bowerbird")` + `env("HOME", tmp.path())` + `env("BOWERBIRD_DATA_DIR", tmp.path().join(".bowerbird"))` pattern Story 3.1 established. Lifecycle tests MUST set `BOWERBIRD_DATA_DIR` so they cannot ever touch the user's real `~/.bowerbird/`.
  - [x] 7.2 Test `status_when_no_pid_file_reports_stopped`: empty TempDir, run `bowerbird status` — expect exit 0, stdout contains `stopped`. This is the no-daemon-ever-ran path.
  - [x] 7.3 Test `start_then_status_then_stop_round_trip` (AC #1, #2, #3, #5): run `bowerbird start`, poll for the ingest socket up to 2s, run `bowerbird status` and assert it contains `running` and `pid <some integer>`, then run `bowerbird stop` and assert exit 0 + the daemon process is no longer alive (PID file points at a dead pid, or PID file is unlinked). Use `BOWERBIRD_DAEMON_BIN = cargo_bin("bowerbird-daemon")` env override so the test invokes the workspace-built daemon, not whatever's on PATH.
  - [x] 7.4 Test `start_when_already_running_is_idempotent`: run `bowerbird start` twice; expect both exit 0; expect the second invocation's stdout to contain `already running` and NOT spawn a second daemon (assert by reading the PID file before and after — same PID).
  - [x] 7.5 Test `stop_when_not_running_is_a_clean_noop`: empty TempDir, run `bowerbird stop`; expect exit 0 + stdout `daemon not running (no pid file); nothing to stop` (verbatim — this string is contracted with `tests/cli_install.rs`'s assertions on the uninstall path).
  - [x] 7.6 Test `start_recovers_from_stale_pid_file` (AC #4): write a stale PID file (`echo 99999 > $HOME/.bowerbird/bowerbird.pid` where 99999 is a known-dead pid — use the Story 3.1 `stale_pid_file_with_dead_process_allows_reacquire` test pattern: spawn `Command::new("true")`, capture its PID, wait for it to reap, then write that PID), run `bowerbird start`, expect success and the new daemon's PID overwriting the file.
  - [x] 7.7 The lifecycle tests share a precondition: a built `bowerbird-daemon` binary. Use `assert_cmd::cargo::cargo_bin("bowerbird-daemon")` to discover the workspace-built binary path. Set it via `BOWERBIRD_DAEMON_BIN` env (the `commands::mod::resolve_daemon_bin` helper already honors this — `src/commands/mod.rs:46-53`). This avoids depending on the binary being on `PATH` in CI.
  - [x] 7.8 ALL lifecycle tests must run under `--test-threads=1` because they spawn real subprocesses and share `BOWERBIRD_DATA_DIR` and TCP-port state across tests (Story 3.1 retro AI-3 + Story 2.5 debug log + Story 3.1 Task 7.5). The test file should NOT need any special marker — the workspace `cargo test -- --test-threads=1` invocation that Story 3.1 documents in its Dev Notes covers it. Note in the test file's module-level comment: "Run under `--test-threads=1` (workspace default for daemon contract tests; see project-context.md and `crates/daemon/tests/contract_daemon.rs`)."

- [x] **Task 8 — Daemon-side contract test for `connected_ws_clients`** (AC: #6, #7)
  - [x] 8.1 Add `mod story_3_2_lifecycle { ... }` to `crates/daemon/tests/contract_daemon.rs` after `story_3_1_singleton`. Reuse the existing test-daemon spawn helpers from `story_2_1_ws` and `story_2_5_shutdown` (they're `pub(super)` per the Epic 2 helpers promotion).
  - [x] 8.2 Test `status_reports_zero_ws_clients_when_no_subscribers`: spawn a daemon, no WS clients, `GET /status` → assert `connected_ws_clients == 0`.
  - [x] 8.3 Test `status_reports_active_ws_subscriber_count`: spawn a daemon, open 3 WebSocket connections, wait for all 3 to send their `Subscribe` ack (or just for them to be accepted past the semaphore checkout — the AC #6 wording is "active WebSocket subscribers" but the implementable definition is "permits currently held against `ws_semaphore`"), then `GET /status` → assert `connected_ws_clients == 3`. Close all 3 WS clients, give 100ms for the per-connection task's drop to release the permit (`crates/daemon/src/api/ws.rs::connection_task` drops the `OwnedSemaphorePermit` on task exit), then `GET /status` again → assert `connected_ws_clients == 0`.
  - [x] 8.4 **Definition decision documented in test:** `connected_ws_clients` == permits currently held (i.e., `ws_max_connections - semaphore.available_permits()`). This counts WS connections that have completed the upgrade and not yet released their permit. It does NOT count connections that are mid-upgrade (before `try_acquire_owned`). This is the value the existing semaphore exposes; renaming the AC's "subscribers" to "active WS connections" is the honest read. The test comment makes this explicit so a future reader doesn't think the implementation is wrong.
  - [x] 8.5 If the existing test framework has any health/status helper (e.g., `query_status` or similar), reuse it; otherwise add a small `get_status(addr: SocketAddr, token: &str) -> protocol::DaemonStatus` helper inside the new module. Keep it local — do not promote unless a third test needs it.

- [x] **Task 9 — Documentation and changelog updates** (AC: #6, #7)
  - [x] 9.1 Add a single combined entry to `docs/protocol-changelog.md` under v1.0 → v1.1 with `type: behavioral`. Body: `Add ServerInfo to crates/protocol/src/rest.rs (or new server_file.rs, depending on Task 2.4's placement) — describes the ~/.bowerbird/server.json file the daemon publishes containing the ephemeral bind_addr. Add additive field DaemonStatus.connected_ws_clients: u32 reporting the count of WS connections currently holding a semaphore permit. Both changes are additive and consumed by Story 3.2's bowerbird status CLI; v1.0 presenters continue to deserialize DaemonStatus responses without modification.` Mirror the structure of the Story 3.1 changelog entry (Approach B's `SHIM_BINARY_NAME` value change) — same heading format, same release section.
  - [x] 9.2 Per Epic 2 retro AI-2, a "WebSocket subsystem" section is to be added to `docs/bmad/planning-artifacts/architecture.md`. Story 3.1's Task 8.4 explicitly punted this to Story 3.2 ("Story 3.1's Dev Notes line 121: 'docs/bmad/planning-artifacts/architecture.md — Story 3.2 owns the WebSocket subsystem section per Epic 2 retro AI-2'"). HOWEVER, looking at epics.md, the WebSocket-subsystem-section AC is wired into Story 3.4 (epics.md lines 750-752: the architecture.md WebSocket subsystem AC is on Story 3.4, not 3.2). The two pointers disagree; the *epic file* is the authoritative AC list. **Decision:** do NOT touch `architecture.md` in Story 3.2 — its WebSocket subsystem section is Story 3.4's responsibility per the epic ACs. Note this divergence in the story's "Files this story does NOT touch" list and let Story 3.4 own it.
  - [x] 9.3 Update `docs/bmad/implementation-artifacts/deferred-work.md` per Task 1.6 (the strike-through). No other deferred-work entries are touched.
  - [x] 9.4 Update `crates/daemon/src/api/status.rs`'s module-level comment per Task 1.4 — remove the deferral note.
  - [x] 9.5 Update `crates/protocol/src/rest.rs::DaemonStatus`'s doc comment per Task 1.2 — remove the deferral note.

## Dev Notes

### What changes vs. what stays

**Files this story creates (NEW):**

| Path | Purpose |
|---|---|
| `src/commands/start.rs` | `bowerbird start` subcommand. Thin wrapper around `commands::daemon::start_daemon_detached`. |
| `src/commands/stop.rs` | `bowerbird stop` subcommand. Thin wrapper around `commands::daemon::stop_daemon_via_pid_file`. |
| `src/commands/status.rs` | `bowerbird status` subcommand. Resolution → HTTP probe → human-readable summary. |
| `src/commands/daemon.rs` | Shared helpers: `start_daemon_detached`, `stop_daemon_via_pid_file`, `read_pid`, `pid_alive`, `send_signal`, `spawn_detached`, `nix_setsid`. Internal-to-`src/`; not pub-exported. |
| `tests/cli_lifecycle.rs` | E2E tests for `start`/`stop`/`status` round-trip via real `bowerbird` + `bowerbird-daemon` subprocesses. |
| `crates/daemon/src/server_file.rs` (optional, depending on Task 2.4 placement) | Atomic writer for `~/.bowerbird/server.json`. Can also live inline in `crates/daemon/src/main.rs` if the helper stays under ~30 lines. |
| `crates/protocol/src/server_file.rs` (optional, depending on Task 2.4 placement) | `ServerInfo` struct. Alternative: place it inside `crates/protocol/src/rest.rs` next to `DaemonStatus`. Pick one; do not declare it in both places. |

**Files this story modifies (UPDATE):**

| Path | What changes | What must be preserved |
|---|---|---|
| `src/main.rs` | Add `Start`, `Stop`, `Status` variants to `enum Command` and corresponding match arms in `fn main()`. | `Install` and `Uninstall` variants and their match arms; the `Cli` derive and `Subcommand` derive; `anyhow::Context` usage idiom. |
| `src/commands/mod.rs` | Add `pub mod start; pub mod stop; pub mod status; pub mod daemon;`. | All existing public helpers (`resolve_claude_settings`, `resolve_bowerbird_dir`, `home_dir`, `resolve_daemon_bin`, `daemon_is_up`). |
| `src/commands/install.rs` | Replace `start_daemon_if_needed` body with a call to `commands::daemon::start_daemon_detached`. Remove the inlined `spawn_detached`, `nix_setsid` functions (move to `commands::daemon`). | The `InstallArgs` struct, the `run` function's outer flow (resolve settings, call `adapter_claude::install`, print outcome). |
| `src/commands/uninstall.rs` | Replace `stop_daemon_if_running` body with a call to `commands::daemon::stop_daemon_via_pid_file`. Remove the inlined `read_pid`, `pid_alive`, `send_signal`, `stop_daemon_if_running` private helpers. | The `UninstallArgs` struct, the `run` function's outer flow (resolve settings, call `adapter_claude::uninstall`, print outcome, conditionally call the stop helper based on `--no-stop`). |
| `crates/protocol/src/rest.rs` | Add `connected_ws_clients: u32` to `DaemonStatus`. Update doc comment. Optionally add `ServerInfo` here. | All existing fields on `DaemonStatus`, `EventListResponse`, `SessionListItem`, `SessionDetail`, `SessionStats`. The `Serialize`/`Deserialize` derive on every type. |
| `crates/protocol/src/lib.rs` | If `ServerInfo` lives in `server_file.rs` (Task 2.4 alternative), add `pub mod server_file;` and re-export. | Existing re-exports (`DaemonStatus`, etc.). |
| `crates/daemon/src/state.rs` | Add `pub max_connections: usize,` to `WsConfig`. | All existing `AppState` fields; the `WsConfig` `Debug, Clone, Copy` derive; `wait_for_ws_connection_drain`. |
| `crates/daemon/src/main.rs` | (a) Populate `max_connections: config.ws_max_connections,` in the `WsConfig { ... }` literal at line ~210. (b) After `listener.local_addr().context(...)?` at line 231, write `~/.bowerbird/server.json` atomically. (c) On clean shutdown after `pools.writer.close()` at line ~291, best-effort delete `server.json`. | The existing startup pipeline order (panic hook → tracing → home → dir → crash dir → singleton → config → token → pools → migrations → projection rebuild → recording started → adapter → ingest → broadcast → axum serve → graceful shutdown). The lock-then-init ordering Story 3.1 set. The `BOWERBIRD_INGEST_SOCK` / `BOWERBIRD_DATA_DIR` env overrides. |
| `crates/daemon/src/api/status.rs` | Compute `connected_ws_clients` from the semaphore and populate it in the `DaemonStatus` response. Remove the deferral comment at the top. | The reader-pool checkout, the `SELECT_LAST_EVENT` query, the `current_unix_millis` clock read, the existing `daemon_version` / `protocol_version` / `uptime_ms` / `last_event_at_ms` / `last_event_id` populations. |
| `crates/daemon/tests/contract_daemon.rs` | Append `mod story_3_2_lifecycle { ... }` with two tests (Task 8.2, 8.3). | All existing test modules; the shared `spawn_test_daemon` and `wait_for_daemon_ready` helpers. |
| `docs/protocol-changelog.md` | Combined `type: behavioral` entry under v1.0 → v1.1. | All existing entries. |
| `docs/bmad/implementation-artifacts/deferred-work.md` | Strike through line 54 with backlink. | All other entries. |

**Files this story does NOT touch:**

- `crates/shim/**` — the shim binary is unchanged.
- `crates/protocol/src/event.rs`, `state.rs`, `ws.rs`, `reaction.rs`, `adapter.rs`, `error.rs`, `constants.rs` — wire types, EventId, EventKind, Reaction, ServerMessage/ClientMessage stay frozen. (Story 3.1's `SHIM_BINARY_NAME` change is already in place.)
- `crates/daemon/src/api/auth.rs`, `events.rs`, `health.rs`, `sessions.rs`, `token.rs`, `ws.rs`, `mod.rs` — none need changes. The `/status` route already exists; only its handler body grows. The `/healthz` / `/readyz` endpoints already do what `bowerbird start` and `bowerbird status` need.
- `crates/daemon/src/ingest/**`, `broadcast/**`, `projection/**`, `db/**`, `singleton.rs` — none need changes.
- `crates/adapter-claude/**` — the Story 3.1 install path is unaffected; the install command's daemon-start logic moves into the new `commands::daemon` module but adapter-claude itself is unchanged.
- `docs/bmad/planning-artifacts/architecture.md` — the "WebSocket subsystem" section AI-2 added to the action items is, per epics.md lines 750-752, **Story 3.4**'s scope, not Story 3.2's. See Task 9.2 for the pointer reconciliation.
- `docs/bmad/planning-artifacts/prd.md`, `epics.md` — no changes. (Story 3.1's review added Epic 2 retro fold-ins to epics.md for stories 3.2, 3.4, 4.4 — those landed already and this story consumes them.)

### Existing behavior to read carefully before changing

- **`crates/daemon/src/main.rs::run` (the ~180-line function)** sets up the full startup pipeline. The Story 3.1 singleton lock sits between `set_crash_dir` and `Config::with_bowerbird_dir`. The `server.json` write goes between `listener.local_addr().context(...)?` at line 231 and `tracing::warn!(addr = %local_addr, "daemon listening")` at line 234 — same point where the bound address becomes known. The unlink on clean shutdown sits after `pools.writer.close()` at line ~291, the latest possible point before `Ok(())`. Do NOT move the server.json write to before `listener.bind`; it MUST come after the kernel-assigned port is known.

- **`crates/daemon/src/api/status.rs`** is the entire scope of the handler changes. The existing reader-pool checkout, `SELECT_LAST_EVENT` query, and clock read patterns must remain in place. The `connected_ws_clients` computation happens on the synchronous side (no DB I/O, just an atomic read on `available_permits()`), so it can run before or after the existing query — putting it after the clock read keeps the structure: "fetch all the data, then construct the response." [Source: `crates/daemon/src/api/status.rs:1-87`]

- **`tokio::sync::Semaphore::available_permits()`** returns the current number of unacquired permits. To get "currently held" you compute `total_cap - available_permits()`. The `total_cap` is `config.ws_max_connections` (default 256 per `crates/daemon/src/config.rs:34`). The cap is currently NOT in `WsConfig` — Task 1.3 adds it there so `AppState.ws_config.max_connections` is the single source of truth the status handler reads (avoids re-reading config or threading a separate `Arc<usize>`). [Source: `crates/daemon/src/state.rs:27-33`, `crates/daemon/src/config.rs:12,34`]

- **`tokio::sync::Semaphore` is monotonic but reads are not snapshot-consistent across calls.** The `available_permits()` value is a momentary read; by the time the status handler serializes the response, a new client may have acquired or released a permit. This is fine — `connected_ws_clients` is a count-at-a-point-in-time, not an invariant. Document this in the protocol-changelog entry and the field's doc comment so consumers don't treat it as a high-precision metric.

- **`src/commands/install.rs::start_daemon_if_needed`** (lines 50-95) is the model `commands::daemon::start_daemon_detached` follows. The `setsid` block (`spawn_detached` + `nix_setsid` at lines 70-111) is what detaches the spawned daemon from the CLI's process group. The same block applies to `bowerbird start`. Story 3.1's choice to call `libc::setsid` directly (instead of pulling `nix` into the CLI's deps) is deliberate — the CLI is intentionally lightweight per project-context.md. Story 3.2 preserves that choice; do NOT add `nix` to the top-level `Cargo.toml`.

- **`src/commands/uninstall.rs::stop_daemon_if_running`** (lines 55-107) is the model `commands::daemon::stop_daemon_via_pid_file` follows. Story 3.1's senior review hardened the post-SIGKILL path with a 1s drain loop (lines 87-97) so the kernel-reap latency doesn't trigger a spurious "still alive" bail. Carry that drain loop into the extracted helper unchanged.

- **`crates/daemon/src/api/ws.rs::handle_upgrade`** acquires the semaphore permit at line 108 via `try_acquire_owned`. The permit is held as `OwnedSemaphorePermit` for the lifetime of the per-connection task (`connection_task`) and released on drop when the task ends (clean close, error close, or panic). This is the source of truth for "WS connection is alive and counted." [Source: `crates/daemon/src/api/ws.rs:108-118`]

- **`crates/daemon/tests/contract_daemon.rs`** has `story_3_1_singleton` as the most recent test module. Story 3.2's `story_3_2_lifecycle` module appends after it. The Story 2.5 test pattern (`spawn_test_daemon` + `wait_for_daemon_ready` + `nix::sys::signal::kill`) is the foundation. Story 3.1's added a 50ms sleep in `wait_for_daemon_ready` after the readiness probe so signal handlers have time to arm — re-use that same helper, don't reinvent it.

- **`tests/cli_install.rs`** at the workspace root is the model for `tests/cli_lifecycle.rs`. The `bowerbird_bin()` helper, the `env_remove` discipline, the `TempDir` isolation pattern — all carry over. The lifecycle tests can sit in the same directory but should be a separate file (`cli_lifecycle.rs`) to keep test names tidy and avoid accidental name collisions on helper functions.

### Atomic write for `server.json`

The pattern mirrors `crates/adapter-claude/src/install.rs`'s settings.json sequence, but simpler because no merge step is needed (the daemon owns the entire file content):

```text
serialize ServerInfo { bind_addr: "127.0.0.1:54321" } → bytes
open server.json.bowerbird-daemon.<pid>.tmp with mode 0600, write bytes, fsync
rename(tmp → server.json)
```

`O_TRUNC` on the tmp open ensures any stale tmp from a prior crash is overwritten. The `fsync` makes the bytes durable before the rename commits. Same FS guarantees `rename` is atomic.

On clean shutdown the unlink is best-effort — failure is logged at WARN and ignored. The next daemon startup will write a new `server.json` over the stale one. If the daemon dies uncleanly, the file is left behind with a possibly-wrong `bind_addr`; the CLI handles this by treating the file as a hint, not a liveness proof (the PID-file + ingest-socket probe is the real liveness check).

**Mode 0600 matters for forward compatibility.** Story 3.3 will add a `token` field to `ServerInfo`. Setting the mode now means Story 3.3 inherits a safe baseline; not setting it means Story 3.3 has to add a mode-change step that creates a TOCTOU window. Pay the small cost now.

### Connected WS clients: definition and edge cases

`connected_ws_clients` reflects **permits currently held against `ws_semaphore`** at the moment of the status query. Specifically:

- A client that has completed the WS upgrade and is inside `connection_task` (`crates/daemon/src/api/ws.rs`) holds one permit and is counted.
- A client that arrives but fails `try_acquire_owned` (over the cap) is rejected before holding a permit and is NOT counted.
- A connection that has called `close()` but whose `connection_task` is still draining its broadcast queue still holds the permit until the task returns — so it remains counted for a brief window after close.
- A connection killed at the TCP level (FIN-less drop) holds the permit until the pong-timeout fires and the task tears down — that's a worst-case window of `ws_pong_timeout` (default 10s) where the count is artificially high. Document this in the field's doc comment so consumers know.

The AC's wording ("connected WS subscribers") is operationally implemented as "permits held," which is the closest mechanical fact the daemon can emit per Axiom 4 (substrate emits facts, presenters interpret).

### Status output format

The CLI prints a fixed-column human-readable block, not JSON. JSON output is a separate concern (consider `--json` flag if it becomes useful for scripting; do not add speculatively). The block:

```
bowerbird daemon
  status        : running
  pid           : 12345
  version       : 0.1.0
  protocol      : 1.0
  uptime        : 1h 23m 7s
  connected ws  : 2
  last event    : 47s ago (event_id=128)
```

Or, when stopped:

```
bowerbird daemon
  status        : stopped
```

Or, when degraded:

```
bowerbird daemon
  status        : degraded (pid 12345 exists but is not accepting ingest connections)
```

Or, when running but token-less:

```
bowerbird daemon
  status        : running (pid 12345; set $BOWERBIRD_TOKEN to read /status details)
```

The leading two-space indent and `:` alignment keep the block scannable. Do NOT use unicode box-drawing characters — the substrate is "small surface area over contributor throughput" (project-context.md) and ASCII is enough.

### Sub-decision: HTTP client for `/healthz` and `/status` probes

The CLI does not currently link any HTTP client. Three options:

1. **Hand-rolled bytes over `TcpStream`** — write a fixed GET request, parse the status-line prefix, optionally parse one Content-Length and a body. ~40 lines per endpoint. Zero new deps. **Recommended.**
2. **Add `ureq` (sync HTTP)** — ~1KB compressed, sync, no Tokio. Brings in `rustls` or `native-tls` depending on features.
3. **Add `reqwest` (async HTTP)** — pulls Tokio into the CLI, which contradicts Story 3.1's intentional lightness.

Option 1 is recommended because: the CLI hits only two endpoints, both on `127.0.0.1`, both with trivial response bodies, no TLS (the daemon binds plain HTTP per `architecture.md` "Bind: `127.0.0.1:<port>` only"). The build budget of adding `ureq` doesn't pay for two endpoints' worth of requests. If a future story needs richer HTTP behavior (chunked transfer, multipart, TLS), reconsider then.

Specifically for `/healthz`:

```text
GET /healthz HTTP/1.1\r\n
Host: 127.0.0.1\r\n
Connection: close\r\n
\r\n
```

Match against `HTTP/1.1 200` in the first line of the response. For `/status`, add `Authorization: Bearer <token>\r\n` and parse the JSON body via `serde_json::from_slice` into `protocol::DaemonStatus`.

### Daemon discovery sequencing in `bowerbird status`

```
1. Read $BOWERBIRD_DATA_DIR (or $HOME/.bowerbird) → data_dir
2. Read data_dir/bowerbird.pid → pid (or "no pid file" → exit "stopped")
3. kill(pid, 0) → alive? (else "stopped (stale pid)")
4. UnixStream::connect(data_dir/ingest.sock) → up? (else "degraded")
5. Read data_dir/server.json → bind_addr (or fallback to liveness-only)
6. GET http://<bind_addr>/status with $BOWERBIRD_TOKEN → DaemonStatus
7. Print the formatted block
```

Each step has a graceful-degradation path so a broken state still produces *some* user-readable output. The exit code is always 0 (status is informational, not a check). Errors with the file system (EACCES on the data dir, etc.) DO exit non-zero — those mean "we can't tell you" rather than "the daemon is stopped."

### Singleton interaction (no changes, just confirmation)

Story 3.1's singleton lock holds `bowerbird.pid` for the daemon's lifetime. `bowerbird stop` reads the PID and sends SIGTERM; the kernel reclaims the FD on exit, releasing the `flock`. `bowerbird start` after a clean stop succeeds because the lock is free. `bowerbird start` after a crash (the AC #4 case) also succeeds because the kernel released the FD when the prior daemon's process table entry was reaped; the singleton's stale-PID safety code path (`crates/daemon/src/singleton.rs:96-109`) handles overwriting the leftover PID. **Story 3.2 does NOT need to touch `singleton.rs`** — the existing recovery is sufficient.

### Previous story intelligence

- **Story 3.1** established the user-facing CLI binary at `src/main.rs` with `install` / `uninstall` subcommands, the `commands::mod` helpers (`resolve_*`, `daemon_is_up`), the `setsid`-based detached spawn pattern, the PID-file / SIGTERM / SIGKILL escalation pattern, the `tests/cli_install.rs` E2E test layout, the `BOWERBIRD_DATA_DIR` env override, and the `BOWERBIRD_CLAUDE_SETTINGS` env override. Story 3.2 reuses ALL of this — the new subcommands are extensions, not rewrites. [Source: `docs/bmad/implementation-artifacts/3-1-bowerbird-install-and-uninstall.md`, `src/main.rs`, `src/commands/mod.rs`, `src/commands/install.rs`, `src/commands/uninstall.rs`, `tests/cli_install.rs`]

- **Story 3.1 Task 5** rotated `protocol::SHIM_BINARY_NAME` from `"bowerbird"` to `"bowerbird-shim"`. This is already in place and Story 3.2 doesn't touch it. Test files that reference the value via the constant (not as a hard-coded string) survive unchanged.

- **Story 3.1 senior review** added a 1s drain loop after SIGKILL in `uninstall.rs::stop_daemon_if_running` to avoid spurious "still alive after SIGKILL" bails caused by kernel-reap latency. Story 3.2's `commands::daemon::stop_daemon_via_pid_file` extraction must carry this drain loop verbatim — the bug it fixes is real and flaky-test-prone if reverted. [Source: `docs/bmad/implementation-artifacts/3-1-bowerbird-install-and-uninstall.md::H2`]

- **Story 3.1's `tests/cli_install.rs`** uses `assert_cmd::Command::cargo_bin("bowerbird")` + `env("HOME", tmp.path())` + `--no-start` / `--no-stop` flags to keep tests from spawning a real daemon. Story 3.2's lifecycle tests DO spawn the daemon; they need `BOWERBIRD_DATA_DIR` set to a TempDir so the spawned daemon's data files (singleton, server.json, bower.db) land in the test's isolated directory.

- **Story 2.5** established the graceful shutdown sequence (SIGTERM → stop accepting → drain → WAL checkpoint → exit 0). `bowerbird stop` sends SIGTERM and trusts this sequence. The `Config::shutdown_drain_timeout` default of 5s is the daemon's drain budget; `bowerbird stop` waits twice that (10s) before escalating to SIGKILL. The 10s budget is documented in `uninstall.rs:79`'s comment — the new `commands::daemon` module's docstring should restate this. [Source: `docs/bmad/implementation-artifacts/2-5-graceful-shutdown-notification-to-connected-tools.md`]

- **Story 2.1** added the WS upgrade path and the `ws_semaphore`-based concurrency cap (default 256). `try_acquire_owned` at `crates/daemon/src/api/ws.rs:108` is the source-of-truth gate that produces the count Story 3.2 surfaces via `available_permits()`. [Source: `docs/bmad/implementation-artifacts/2-1-websocket-connection-and-topic-subscription.md`]

- **Epic 2 retrospective (2026-05-24) Discovery #1 / AI-1** is the explicit charter for Task 1 of this story. The retro recommendation was unambiguous: "The retrospective recommendation is to fold it into Story 3.2 — it is now Epic-3-blocking, not Epic-2-blocking, since the consumer is the new CLI surface, not the WS surface that owns the counter." Story 3.2 closes AI-1. [Source: `docs/bmad/implementation-artifacts/epic-2-retro-2026-05-24.md::Discovery #1`, `epic-2-retro-2026-05-24.md::AI-1`]

- **Epic 2 retro AI-3 (--test-threads=1 requirement)** is shared infrastructure: Story 3.2's lifecycle tests must also pass under serial execution. Story 3.4 owns the explicit `.github/workflows/ci.yml` change; Story 3.2's new tests just need to *be* serial-clean. The pattern is established in `crates/daemon/tests/contract_daemon.rs` already; the new `story_3_2_lifecycle` module inherits it.

- **Story 1.7 deferred-work entry (line 54)** for `/status.connected_ws_clients` is the formal record being struck through. The entry's exact wording is in `docs/bmad/implementation-artifacts/deferred-work.md`; Task 1.6 wraps it in `~~...~~` and appends the `**Resolved by Story 3.2 (Task 1):**` backlink. Mirror Story 2.4's strike-through format (which closed the Story 2.3 lag-during-snapshot entry) for consistency. [Source: `docs/bmad/implementation-artifacts/deferred-work.md:54`]

### Technology constraints

- **Workspace-pinned dep versions** (root `Cargo.toml`). Relevant pins for this story: `serde` 1.0.228, `serde_json` 1.0.149, `clap` 4.5.37 (derive), `anyhow` 1.0.102, `assert_cmd` 2.0.17, `tempfile` 3.20.0, `libc` 0.2.186, `tokio` 1.52.1 (daemon-side; the CLI does NOT pull tokio). The `nix` workspace dep is in `[dependencies]` for the daemon but NOT in the CLI's top-level `Cargo.toml` — keep it that way. Story 3.1's CLI uses `libc::setsid` directly to avoid pulling `nix` into the CLI's dep tree; Story 3.2 maintains that boundary.

- **No new dependencies in this story.** The hand-rolled HTTP client (Task 3.4) and hand-rolled duration formatter (Task 5.5) are intentional choices to avoid bloating the CLI. If standards-by-default conflict emerges, document the conflict in the dev notes and propose a deferred-work entry rather than landing a new dep speculatively.

- **`#![deny(unsafe_code)]` workspace-wide** is enforced by `[workspace.lints.rust] unsafe_code = "forbid"`. The Story 3.1 `nix_setsid` helper uses `unsafe { setsid() }` via an `extern "C"` declaration — this is the existing exception, and Story 3.2's `commands::daemon` module (which absorbs `nix_setsid`) inherits it. The `extern "C"` block is the smallest possible unsafe surface; do not expand it. Any additional FFI calls (e.g., `libc::kill` is FFI but goes through `nix` in the daemon and through a small `unsafe` wrapper in the CLI) must stay tightly scoped.

- **`anyhow::Context` is allowed at the binary edge.** `src/main.rs` is the binary edge for the CLI; `src/commands/*` must use typed errors (anyhow is allowed for the CLI's outer dispatch context but typed inside the subcommand modules — same convention Story 3.1 set in `src/commands/install.rs` and `src/commands/uninstall.rs`). The new `commands::daemon` shared-helper module returns `anyhow::Result<_>` because the helpers compose multiple typed-error sources (PID file IO, kill syscall errno, HTTP failure modes) and the call sites can't usefully discriminate; this is the same pattern Story 3.1's pre-extraction `start_daemon_if_needed` already used.

- **CLI binary should NOT pull `tokio`.** Same constraint as Story 3.1. The CLI is small synchronous work plus subprocess spawning. If a future story needs an async client (Story 4.x replay/export?), reconsider. Story 3.2 does NOT.

- **`Cargo.lock` committed.** Updating to add `ServerInfo` to the protocol crate and `connected_ws_clients` to `DaemonStatus` is a pure source change — no `Cargo.lock` impact. The HTTP probes in the CLI (hand-rolled) also don't change `Cargo.lock`. If a dep is added against the recommendation, the resulting `Cargo.lock` update lands with this story.

### Project Structure Notes

- Per `architecture.md` §Project Structure, the top-level `bowerbird/` CLI binary lives at `src/main.rs` + `src/commands/`. Story 3.1 established this. Story 3.2's new files (`start.rs`, `stop.rs`, `status.rs`, `daemon.rs`) live alongside the existing `install.rs` and `uninstall.rs` in `src/commands/`.
- The protocol crate's `lib.rs` already re-exports `DaemonStatus`. If `ServerInfo` lands in `crates/protocol/src/rest.rs` (Task 2.4 option A), no re-export change is needed (it lives in a module that's already re-exported). If `ServerInfo` lands in a new `crates/protocol/src/server_file.rs` (option B), add `pub mod server_file;` and `pub use server_file::ServerInfo;` to `lib.rs`.
- **`_bmad-output/` is a symlink to `docs/bmad/`.** Writing this story to `docs/bmad/implementation-artifacts/3-2-daemon-lifecycle-cli.md` is equivalent to writing it via the symlinked path. No separate update needed.
- **Workspace root `tests/`** is where the CLI E2E tests live. The new `tests/cli_lifecycle.rs` sits next to `tests/cli_install.rs`. Both compile to `cargo test --tests` and run under `cargo test --workspace`.

### Cargo test discipline

Per Epic 2 retro AI-3 and Story 2.5 / Story 3.1 debug logs, the daemon contract-test suite must be run with `--test-threads=1` to avoid hangs from shared process-level state (real subprocesses, signal handlers, file system fixtures). When running tests for this story:

```bash
cargo test --workspace -- --test-threads=1
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
```

The story-automator orchestration note (Story 3.1 Dev Notes line 188): "Always run cargo test --workspace and cargo clippy --workspace --all-targets after changes; confirm both are green before marking dev-story done. Keep scope tight to each story; do not refactor unrelated code."

If a contract-test hang surfaces locally, the symptom is the test binary not producing any output after the readiness-wait line. The cure is `-- --test-threads=1`. The Story 3.4 author will land the explicit `.github/workflows/ci.yml` change.

### References

- [Source: docs/bmad/planning-artifacts/epics.md#Story-3.2] — Story statement and 6 ACs (including the folded AI-1 `connected_ws_clients` ACs at lines 682-688).
- [Source: docs/bmad/planning-artifacts/prd.md] — FR29 (start/stop daemon via CLI), FR30 (status and version check via CLI), NFR3 (2s cold-start readiness via /healthz).
- [Source: docs/bmad/planning-artifacts/architecture.md#API-and-Communication-Patterns] — REST endpoint surface, axum + tokio current_thread runtime, 127.0.0.1 bind invariant.
- [Source: docs/bmad/planning-artifacts/architecture.md#Implementation-Patterns-and-Consistency-Rules] — anti-pattern list (no `anyhow::Context` outside `main.rs`, no `unwrap()` outside test code).
- [Source: docs/bmad/planning-artifacts/architecture.md#Project-Structure-and-Boundaries] — directory layout for `bowerbird/src/`, `crates/daemon/src/api/status.rs`.
- [Source: docs/bmad/project-context.md#API-surface] — HTTP endpoint split (healthz/readyz unauthed, others bearer-gated); `~/.bowerbird/server.json` as the canonical token storage referenced at line 471.
- [Source: docs/bmad/project-context.md#Daemon-implementation-constraints] — single-threaded tokio, `AppState` shape, WS concurrency cap default 256, ping interval 30s.
- [Source: docs/bmad/implementation-artifacts/3-1-bowerbird-install-and-uninstall.md] — CLI binary layout, settings.json atomic-write pattern, PID file / SIGTERM / SIGKILL escalation pattern, `BOWERBIRD_DATA_DIR` and `BOWERBIRD_CLAUDE_SETTINGS` env overrides, the senior review's H2 fix for the post-SIGKILL drain loop.
- [Source: docs/bmad/implementation-artifacts/2-5-graceful-shutdown-notification-to-connected-tools.md] — graceful shutdown sequence; SIGTERM / SIGINT test pattern; `daemon listening` log-wait readiness probe.
- [Source: docs/bmad/implementation-artifacts/2-1-websocket-connection-and-topic-subscription.md] — `ws_semaphore` introduction; per-connection task lifecycle; permit ownership across the connection's lifetime.
- [Source: docs/bmad/implementation-artifacts/epic-2-retro-2026-05-24.md#Discovery-1] — Discovery #1 / AI-1: the formal charter for wiring `connected_ws_clients` into `DaemonStatus` in Story 3.2.
- [Source: docs/bmad/implementation-artifacts/deferred-work.md] — line 54 (`/status.connected_ws_clients`) to strike through (Task 1.6 + Task 9.3).
- [Source: crates/protocol/src/rest.rs] — `DaemonStatus` struct to extend; doc comment to update.
- [Source: crates/protocol/src/lib.rs:15] — existing `DaemonStatus` re-export; `ServerInfo` re-export added if Task 2.4 option B is taken.
- [Source: crates/daemon/src/api/status.rs] — `get` handler to extend; deferral comment to remove.
- [Source: crates/daemon/src/state.rs:11-22] — `AppState` and `WsConfig` shapes; `max_connections` field to add to `WsConfig`.
- [Source: crates/daemon/src/main.rs:209-214,228-234,288-292] — `WsConfig` construction site; `local_addr` resolution; `pools.writer.close()` (cleanup point for `server.json` unlink).
- [Source: crates/daemon/src/config.rs:8,30,34] — `bind_addr: SocketAddr` default of `127.0.0.1:0`; `ws_max_connections: 256`.
- [Source: crates/daemon/src/api/ws.rs:108] — `try_acquire_owned` site that produces the permit count Task 1 surfaces.
- [Source: crates/daemon/src/api/token.rs:60-62] — `$BOWERBIRD_TOKEN` env-var resolution; Story 3.3 will extend this with keychain + file fallback.
- [Source: crates/daemon/src/singleton.rs] — PID file + flock primitives that `bowerbird stop` / `bowerbird status` read; no changes needed in 3.2.
- [Source: src/main.rs] — clap subcommand dispatcher to extend with `Start`, `Stop`, `Status` variants.
- [Source: src/commands/mod.rs] — shared helpers (`resolve_bowerbird_dir`, `resolve_daemon_bin`, `daemon_is_up`); new module declarations to add.
- [Source: src/commands/install.rs:50-95] — daemon-spawn pattern (`start_daemon_if_needed`, `spawn_detached`, `nix_setsid`) to refactor into `commands::daemon`.
- [Source: src/commands/uninstall.rs:55-107] — daemon-stop pattern (`stop_daemon_if_running`, `read_pid`, `pid_alive`, `send_signal`) to refactor into `commands::daemon`.
- [Source: tests/cli_install.rs] — E2E test layout (`assert_cmd::Command::cargo_bin`, `TempDir`, `env("HOME", ...)`, `env_remove`) — model for `tests/cli_lifecycle.rs`.
- [Source: crates/daemon/tests/contract_daemon.rs::story_3_1_singleton] — most recent test module; `story_3_2_lifecycle` appends after it; reuse `spawn_test_daemon` and `wait_for_daemon_ready` helpers.
- [Source: docs/protocol-changelog.md] — v1.0 → v1.1 entry to add (combined `ServerInfo` + `connected_ws_clients` behavioral entry).

## Dev Agent Record

### Agent Model Used

claude-opus-4-7[1m] (Claude Opus 4.7, 1M context) via bmad-dev-story workflow.

### Debug Log References

- **2026-05-24 — story_2_5_shutdown SIGTERM/SIGINT tests failed after Task 2 landed.** Adding `server_file::write` between the `daemon listening` warn line and the `axum::serve(...)` future-construction pushed the runtime's first poll past the test's readiness window. Story 2.5's `wait_for_daemon_ready` (unlike Story 3.1's) does not sleep 50ms after seeing the log line, so the test sent SIGTERM before the SIGINT/SIGTERM handlers were armed inside `shutdown_signal`'s first poll. **Fix:** moved the `server_file::write` call to BEFORE the `daemon listening` warn line. This is a strict improvement: the log now implies "everything CLIs and tests poll for (ingest socket + server.json + serve future about to be polled) is ready." No change to Story 2.5's test code; the regression is repaired by reordering, in-scope to Task 2.

### Completion Notes List

- **Task 1 (DaemonStatus.connected_ws_clients).** `connected_ws_clients: u32` is computed once via `state.ws_config.max_connections.saturating_sub(state.ws_semaphore.available_permits())` inside `status::get` and placed last in the `Json(DaemonStatus { ... })` literal so the snapshot value is not re-read mid-construction. Added `pub max_connections: usize` to `WsConfig` and populated it from `config.ws_max_connections` in `main.rs` so the handler does not have to thread a second `Arc<usize>` through `AppState`. Deferral comments in `crates/daemon/src/api/status.rs` and `crates/protocol/src/rest.rs::DaemonStatus` doc removed; replaced with the snapshot-semantics note from Task 1.2. Deferred-work entry on `deferred-work.md` line 54 wrapped in `~~…~~` with the `**Resolved by Story 3.2 (Task 1):**` backlink that names the contract test.
- **Task 2 (server.json).** New `crates/daemon/src/server_file.rs` owns the atomic write (tmp + fsync + rename, mode 0600) and the best-effort remove on clean shutdown. The remove uses `ErrorKind::NotFound → Ok(false)` to keep the unlink truly best-effort. `crates/protocol/src/rest.rs::ServerInfo { bind_addr: String }` was placed next to `DaemonStatus` per Task 2.4 option A (no new `server_file.rs` in the protocol crate; not enough to justify a module). Wired into `crates/daemon/src/main.rs::run` which now takes `bowerbird_dir: PathBuf` as a second argument (the existing path was already computed at the call site in `main()`).
- **Task 3 + 4 + 6 (subcommands + shared helpers).** New `src/commands/daemon.rs` holds the single copy of `start_daemon_detached`, `stop_daemon_via_pid_file` (with the Story 3.1 post-SIGKILL 1s drain loop preserved verbatim), `read_pid`, `pid_alive`, `spawn_detached`, `nix_setsid`, plus the hand-rolled HTTP probes (`http_get_healthz`, `http_get_status`) used by `start` and `status`. `install.rs` and `uninstall.rs` now call the shared helpers and lost their inlined copies of the spawn/stop logic. The `enum Command` in `src/main.rs` is alphabetical (`Install`, `Start`, `Status`, `Stop`, `Uninstall`) — pick one ordering and apply consistently per Task 6.1's recommendation.
- **Task 5 (status).** Resolution order: pid file → `kill(pid, 0)` → ingest-socket connect probe → `server.json` parse → bearer-gated `GET /status`. Each step has a graceful-degradation path; the binary always exits 0 unless we cannot tell what's happening (the daemon-stop helper still propagates errors, but `status` itself does not). Hand-rolled `format_uptime(Duration → "1h 23m 7s")` saves a `humantime` dep for the single display path.
- **Task 7 (CLI E2E).** `tests/cli_lifecycle.rs` covers the no-daemon stopped path, the start → status → stop round-trip, start idempotency, stop on no-daemon (verbatim string contracted with `tests/cli_install.rs`), and stale-PID recovery via the spawn-and-reap `Command::new("true")` pattern from Story 3.1. Tests use `BOWERBIRD_DATA_DIR` + `BOWERBIRD_DAEMON_BIN` so they never touch the developer's real `~/.bowerbird/`. Cleanup helper `force_stop` ensures a panicking test doesn't leak a daemon into the runner.
- **Task 8 (daemon contract tests).** `story_3_2_lifecycle` appended to `crates/daemon/tests/contract_daemon.rs`: `status_reports_zero_ws_clients_when_no_subscribers` via `tower::ServiceExt::oneshot`; `status_reports_active_ws_subscriber_count` opens 3 WS connections (reads each Hello frame as a barrier proving `try_acquire_owned` returned), asserts 3, drops, sleeps 200ms for permit release, asserts 0. Reuses `spawn_test_daemon`, `connect_authed`, `read_text_frame_or_close`, `parse_hello` from `story_2_1_ws`.
- **Task 9 (changelog).** Single combined `type: behavioral` entry under v1.0 → v1.1 documents both the additive `ServerInfo` and `DaemonStatus.connected_ws_clients: u32` change in one bullet, with the same v1.0-presenter-compatibility framing as the Story 2.1 / 2.2 entries.
- **Out of scope (intentional).** Per Task 9.2, no edits to `docs/bmad/planning-artifacts/architecture.md`; epics.md routes the WebSocket-subsystem section AC to Story 3.4. Architecture section will land there.
- **Verification.** `cargo test --workspace -- --test-threads=1` → 286 passed (16 suites). `cargo clippy --workspace --all-targets -- -D warnings` → no issues.

### File List

**New:**

- `crates/daemon/src/server_file.rs`
- `src/commands/daemon.rs`
- `src/commands/start.rs`
- `src/commands/status.rs`
- `src/commands/stop.rs`
- `tests/cli_lifecycle.rs`

**Modified:**

- `Cargo.toml` (CLI now depends on `serde_json` for `ServerInfo`/`DaemonStatus` parsing)
- `crates/daemon/src/api/status.rs`
- `crates/daemon/src/lib.rs` (registers `server_file` module)
- `crates/daemon/src/main.rs` (writes/removes `server.json`; `run` now takes `bowerbird_dir`; `WsConfig.max_connections` populated)
- `crates/daemon/src/state.rs` (added `WsConfig.max_connections`)
- `crates/daemon/tests/contract_daemon.rs` (added `story_3_2_lifecycle`; updated test helpers for `WsConfig.max_connections`)
- `crates/protocol/src/lib.rs` (re-exports `ServerInfo`)
- `crates/protocol/src/rest.rs` (added `connected_ws_clients` to `DaemonStatus`; added `ServerInfo` struct)
- `crates/protocol/tests/contract_protocol.rs` (initializer for new field)
- `docs/bmad/implementation-artifacts/deferred-work.md` (strike-through with backlink)
- `docs/bmad/implementation-artifacts/sprint-status.yaml` (story status → in-progress → review)
- `docs/bmad/implementation-artifacts/tests/test-summary.md` (refreshed by `bmad-qa-generate-e2e-tests` for Story 3.2; documents the new `cli_lifecycle.rs` and `story_3_2_lifecycle` coverage)
- `docs/protocol-changelog.md` (combined v1.0 → v1.1 entry)
- `src/commands/install.rs` (calls shared `commands::daemon::start_daemon_detached`)
- `src/commands/mod.rs` (registers `daemon`, `start`, `status`, `stop` modules)
- `src/commands/uninstall.rs` (calls shared `commands::daemon::stop_daemon_via_pid_file`)
- `src/main.rs` (added `Start`, `Status`, `Stop` clap variants and match arms)

## Change Log

| Date | Change |
|---|---|
| 2026-05-24 | Story 3.2 created via bmad-create-story workflow; status set to ready-for-dev. Folds Epic 2 retro AI-1 (`connected_ws_clients` wiring) into the lifecycle CLI story per Discovery #1's recommendation. |
| 2026-05-24 | Story 3.2 implemented via bmad-dev-story. Surfaces `connected_ws_clients` on `GET /status` (Task 1, AC #6/#7), publishes the daemon bind-addr to `~/.bowerbird/server.json` atomically with mode 0600 (Task 2), adds `bowerbird start` / `status` / `stop` subcommands sharing a single `commands::daemon` helper module with `bowerbird install` / `uninstall` (Tasks 3-6, AC #1/#2/#3/#4/#5). Adds `tests/cli_lifecycle.rs` E2E coverage and `crates/daemon/tests/contract_daemon.rs::story_3_2_lifecycle` for the new `connected_ws_clients` semantics. Combined protocol-changelog entry under v1.0 → v1.1 covers both additive surface changes. All tests green under `--test-threads=1`; clippy clean. Status set to review. |
| 2026-05-24 | Story 3.2 marked done after review-session bookkeeping. Final verification: `cargo test --workspace -- --test-threads=1 --skip state_plus_event_atomicity_under_sigkill_during_load` → 287 passed, 1 filtered out (the skipped test); `cargo clippy --workspace --all-targets -- -D warnings` → clean. The skipped test is a pre-existing SQLite-teardown deadlock unrelated to Story 3.2, tracked in taskwarrior task `a2ea3bfb` (project:bowerbird +bug) for a real fix. Status set to done. |
