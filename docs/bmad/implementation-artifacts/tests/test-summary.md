# Test Automation Summary — Story 3.1

Generated 2026-05-24 via `bmad-qa-generate-e2e-tests`. Prior summary covered Story 2.5; superseded here.

## Gap Analysis

Story 3.1's library and singleton-lock surfaces were already well-covered prior to this automation pass:

- `crates/adapter-claude/src/install.rs` in-module tests — JSON merge happy path, idempotency, mixed-group strip, parse/non-object rejection, absolute-path basename match, substring safety (10 tests).
- `crates/adapter-claude/tests/contract_install.rs` — concurrent install, no-tmp-leftover, parse-failure preservation, parent-dir creation, uninstall idempotency, command-shape per kind (7 tests).
- `crates/daemon/src/singleton.rs` in-module tests — fresh acquire, PID written, same-process re-acquire fails, drop releases, stale-PID overwrite, unparseable PID (6 tests).
- `crates/daemon/tests/contract_daemon.rs::story_3_1_singleton` — second-daemon-exits-nonzero with holder PID, lock release on SIGTERM, lock release on SIGKILL (3 tests).

The gap: **zero subprocess (E2E) tests of the actual `bowerbird` CLI binary**. The user-facing entry points (`bowerbird install`, `bowerbird uninstall`) were untested through their real argparse / env-resolution / exit-code surface. Library calls cannot validate clap subcommand wiring or the `--no-start` / `--no-stop` scope-cut flags.

## Generated Tests

### E2E Tests (new)

- `tests/cli_install.rs::install_creates_settings_when_missing` — AC #5: `bowerbird install --settings <missing> --no-start` creates the file and exits 0.
- `tests/cli_install.rs::install_then_uninstall_via_cli_preserves_user_content` — AC #1 + #4: full round-trip via the CLI; theme, editor settings, and a user-authored hook survive both operations.
- `tests/cli_install.rs::install_respects_env_override_for_settings_path` — AC #1: `BOWERBIRD_CLAUDE_SETTINGS` env override is honored end-to-end.
- `tests/cli_install.rs::install_exits_nonzero_on_malformed_settings_json` — AC #1 (negative): non-zero exit + error message references the offending path + original bytes preserved verbatim.
- `tests/cli_install.rs::uninstall_on_missing_settings_is_a_clean_noop` — AC #4: uninstall on missing file exits 0 and does not create the file.
- `tests/cli_install.rs::install_twice_is_idempotent` — AC #1: re-running install leaves settings.json byte-identical (idempotency contract through the CLI).
- `tests/cli_install.rs::help_lists_install_and_uninstall_subcommands` — Surface regression: `bowerbird --help` mentions both subcommands.

### Supporting changes

- `Cargo.toml` (top-level) — added `[dev-dependencies]` block: `assert_cmd`, `tempfile`, `serde_json` (all already workspace-pinned).

## Coverage

| AC | Library tests | Singleton tests | CLI E2E tests |
|---|---|---|---|
| #1 atomic merge + binary name from protocol constant | ✅ | n/a | ✅ (new) |
| #2 concurrent-write retry | ✅ | n/a | n/a (library-sufficient) |
| #3 interruption safety | ✅ | n/a | n/a (library-sufficient) |
| #4 uninstall removes only bowerbird entries | ✅ | n/a | ✅ (new) |
| #5 creates settings.json if missing | ✅ | n/a | ✅ (new) |
| #6 singleton daemon enforcement | n/a | ✅ | n/a (covered by daemon contract) |

Daemon-start (install) and daemon-stop (uninstall) lifecycle paths are deliberately bypassed via `--no-start` / `--no-stop` — the underlying daemon spawn/stop is exercised by `crates/daemon/tests/contract_daemon.rs::story_3_1_singleton` already. Doubling that coverage here would duplicate slow subprocess tests without adding signal.

## Validation

- [x] `cargo test --workspace -- --test-threads=1` — **265 passed (15 suites, 11.22s)**. Prior baseline was 258 tests; the 7 new CLI tests bring the count to 265.
- [x] `cargo clippy --workspace --all-targets -- -D warnings` — **0 issues**.

## Notes

- Tests use `assert_cmd::cargo_bin("bowerbird")` for real subprocess invocation, isolated `TempDir` for each test's HOME and settings path, and explicit `env_remove` for both `BOWERBIRD_CLAUDE_SETTINGS` and `BOWERBIRD_DATA_DIR` so a developer's environment can't leak into a test.
- `--test-threads=1` retained per epic-2 retro AI-3 (real-subprocess fixtures must not race on signal handlers or process state). Suite still completes in ~11s end-to-end.
- Scope tight to Story 3.1 per orchestration custom instructions; no unrelated code refactored.
