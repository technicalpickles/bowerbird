# Test Automation Summary — Story 3.2

Generated 2026-05-24 via `bmad-qa-generate-e2e-tests`. Prior summary covered Story 3.1; superseded here.

## Gap Analysis

Story 3.2 (daemon lifecycle CLI) landed with substantial test coverage already:

- `tests/cli_lifecycle.rs` (existing, 7 tests) — `status_when_no_pid_file_reports_stopped`, `start_then_status_then_stop_round_trip`, `start_when_already_running_is_idempotent`, `stop_when_not_running_is_a_clean_noop`, `start_recovers_from_stale_pid_file`, `help_lists_all_lifecycle_subcommands`, plus the implicit AC #5 coverage from Story 3.1's `tests/cli_install.rs`.
- `crates/daemon/tests/contract_daemon.rs::story_3_2_lifecycle` (existing, 2 tests) — `status_reports_zero_ws_clients_when_no_subscribers` and `status_reports_active_ws_subscriber_count` via `tower::ServiceExt::oneshot` against the axum router. Drives `DaemonStatus.connected_ws_clients` through a real `try_acquire_owned` checkout/release cycle on the `ws_semaphore`.
- `src/commands/status.rs::tests` (in-module) — `format_uptime` boundary tests for the hand-rolled `Duration → "1h 23m 7s"` formatter.
- `src/commands/daemon.rs::tests` (in-module) — `parse_status_code`, `body_after_headers`, `find_subslice` for the hand-rolled HTTP/1.1 client.

Two gaps remained after that baseline:

**Gap A — `bowerbird status` full-block rendering for AC #3 + AC #6.** The existing round-trip test only asserts `stdout.contains("running")` and the pid. Because the daemon mints an ephemeral random bearer token when `$BOWERBIRD_TOKEN` is unset (`crates/daemon/src/api/token.rs:60`) and the CLI reads the same env var (`src/commands/status.rs:74`), running `bowerbird status` from the test always falls into the `print_running_basic` degraded path. The `print_full_status` formatter — the only place that emits `connected ws  : N` and `uptime`, `version`, `protocol`, `last event` — had zero E2E coverage. The daemon contract test proved the JSON shape; nothing proved the CLI rendered it.

**Gap B — `bowerbird status` stale-PID-file path.** The `print_stopped(Some(...))` branch in `status::run` (line 42-45) covers the case where `bowerbird.pid` names a process that has already exited. The start-side stale-PID recovery is covered by `start_recovers_from_stale_pid_file`, but the *status*-side reporting branch had no test.

## Generated Tests

### E2E Tests (new)

- `tests/cli_lifecycle.rs::status_with_shared_token_renders_full_block_including_ws_clients` — AC #3 + AC #6. Sets `BOWERBIRD_TOKEN` on both the spawn and the status invocation so the daemon (which inherits env via `spawn_detached`) and the CLI share a known token. Asserts each labeled row of the full status block — `status        : running`, `pid           :`, `version       :`, `protocol      :`, `uptime        :`, `connected ws  : 0`, `last event    :` — appears in stdout. Asserts the token-unset hint *does not* appear (regression canary for env inheritance / token plumbing). Uses a deferred `cleanup` closure so a failed assertion does not leak a daemon into the test runner.
- `tests/cli_lifecycle.rs::status_with_stale_pid_file_reports_stopped_stale` — AC #3 (stopped path, stale variant). Spawns and reaps `Command::new("true")` to obtain a known-dead PID (same trick used by `start_recovers_from_stale_pid_file`), writes it to `~/.bowerbird/bowerbird.pid`, runs `bowerbird status`, asserts stdout contains `stopped` + `stale pid` + the dead pid integer. Exercises the `print_stopped(Some(...))` branch in `src/commands/status.rs:42-45`.

### Supporting changes

None. Both tests fit inside the existing `tests/cli_lifecycle.rs` helper surface (`bowerbird_cmd_in`, `data_dir`, `read_pid_file`, `wait_for_daemon_up`, `wait_for_pid_dead`, `force_stop`). No new crates, no new helpers, no `Cargo.toml` changes.

## Coverage

| AC | Daemon contract tests | CLI E2E tests |
|---|---|---|
| #1 `start` → `/healthz` 200 within 2s | n/a (Story 1.7 covers `/healthz`) | ✅ existing |
| #2 `stop` → SIGTERM + graceful shutdown | ✅ Story 2.5 | ✅ existing |
| #3 `status` shows version + uptime + liveness; "stopped" when not running | n/a | ✅ existing (stopped + running) + **new** (full block + stale-PID variant) |
| #4 stale-PID → `start` recovers; `/readyz` 200 | ✅ Story 3.1 singleton tests | ✅ existing (start path) |
| #5 install auto-starts daemon | n/a | ✅ Story 3.1's `tests/cli_install.rs` (out of scope for 3.2) |
| #6 `connected_ws_clients` on `GET /status` and `bowerbird status` | ✅ existing (JSON shape via `oneshot`) | ✅ **new** (CLI rendering of `connected ws  : 0`) |
| #7 protocol-surface additions (`DaemonStatus.connected_ws_clients`, doc + deferred-work strike) | ✅ existing (field is used by the contract test); doc/file changes are static review-time | n/a |

## Validation

- [x] `cargo test --workspace -- --test-threads=1` — **288 passed (16 suites, 12.00s)**. Prior baseline was 286 (per `Completion Notes` in `3-2-daemon-lifecycle-cli.md`); the 2 new tests bring the count to 288.
- [x] `cargo clippy --workspace --all-targets -- -D warnings` — **0 issues**.

## Notes

- Both new tests inherit the file-level isolation discipline (per-test `TempDir`, `BOWERBIRD_DATA_DIR` + `BOWERBIRD_DAEMON_BIN` + `BOWERBIRD_CLAUDE_SETTINGS` cleared via the `bowerbird_bin` env-remove block). They cannot touch the developer's real `~/.bowerbird/`.
- The full-block test sets `BOWERBIRD_TOKEN` to a fixed literal (`bb-test-token-3-2`) on both the spawn and the status invocation so the daemon's `load_or_generate` returns the same value the CLI reads — that's the single env-var the daemon and CLI must agree on for the bearer-gated `/status` path to succeed. The test asserts the absence of `$BOWERBIRD_TOKEN` in stdout as a regression canary against the env-inheritance contract.
- Scope kept tight to Story 3.2 per orchestration custom instructions; no unrelated code refactored, no test framework changes, no new crate dependencies. Both tests use only existing helpers from the file.
- `--test-threads=1` retained per Epic 2 retro AI-3 + Story 3.1 retro — the new tests spawn real daemon subprocesses and share TCP-port + PID-file state with the rest of `cli_lifecycle.rs`.
