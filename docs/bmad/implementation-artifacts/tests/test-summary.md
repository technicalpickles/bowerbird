# Test Automation Summary — Story 4.1

Generated 2026-05-25 via `bmad-qa-generate-e2e-tests`. Supersedes the Story 3.4 summary.

## Baseline coverage already in place

Story 4.1 (`bowerbird replay` and `bowerbird export` commands) landed with extensive test coverage from the dev-story pass — 20 dedicated tests before this QA run:

- **`crates/daemon/tests/contract_daemon.rs::story_4_1_replay`** (6 tests) — `replay_forwards_events_through_broadcast_path`, `replay_emits_state_frames_for_each_session`, `replay_continues_on_per_line_parse_error`, `replay_rejects_sentinel_kinds`, `replay_requires_bearer`, `replay_dropped_event_id_and_created_at_are_reassigned`. Each maps to a specific AC and covers the daemon-side wiring of `POST /replay` through the `ingest_tx` → writer task → broadcast hub path.
- **`tests/cli_replay.rs`** (5 tests) — `replay_with_explicit_file_forwards_events`, `replay_with_no_argument_uses_bundled_fixture`, `replay_continues_after_invalid_lines`, `replay_fails_clearly_when_daemon_down`, `replay_fails_with_401_when_token_wrong`. End-to-end CLI exercise of `bowerbird replay`.
- **`tests/cli_export.rs`** (4 tests) — `export_writes_jsonl_of_session_events_to_stdout`, `export_writes_to_file_when_output_flag_given`, `export_returns_session_not_found_for_unknown_id`, `export_round_trips_through_replay`. End-to-end CLI exercise of `bowerbird export` including the load-bearing round-trip invariant.
- **`tests/cli_replay_fixture.rs`** (5 tests) — `bundled_fixture_is_valid_jsonl`, `bundled_fixture_spans_at_least_two_sessions`, `architecture_md_lists_replay_and_export_as_shipped`, `protocol_changelog_documents_post_replay_endpoint`, `cli_help_lists_replay_and_export`. Hermetic doc-drift guardrails per Epic 3 retro agreement A7.
- **Unit tests in `src/commands/replay.rs`** (2 tests) — `count_fixture_shape_skips_blank_and_comment_lines`, `count_fixture_shape_counts_distinct_sessions`. Cover the CLI-side preamble computation.

All 20 baseline tests pass under `cargo test --workspace -- --test-threads=1`.

## Gap Analysis

Four gaps remained after that baseline. Each is a silent-failure mode — a regression here does not break any existing test, but a behavior the AC promises stops being enforced.

**Gap A — Comment + blank line handling in `POST /replay` was implemented but unpinned.** AC #1 says "every non-blank, non-`#`-prefixed line deserializes as a `protocol::Event`". The daemon code at `crates/daemon/src/api/replay.rs:69-71` honors this by skipping such lines, but no test asserts that a `#`-comment line does not become a `parse_error` entry. A future refactor that, say, switches the body parsing to a stricter line iterator that treats `#` as JSON-invalid would silently break fixture authors who put explanatory headers in their JSONL files. The trailing-newline edge case (universal in JSONL) is implicitly exercised by the existing fixture but not pinned at the contract level.

**Gap B — Asymmetric transport-failure coverage between replay and export.** `replay_fails_clearly_when_daemon_down` asserts the "daemon stopped" stderr for `bowerbird replay`, but no equivalent existed for `bowerbird export`. The two commands share the same `commands::daemon::read_server_info` + token-resolver chain, but they have separate `Unreachable` arms in their own `run` functions. Symmetric coverage means a future change that swaps one command's error path doesn't silently regress the other.

**Gap C — Asymmetric 401 coverage between replay and export.** Same shape as Gap B: `replay_fails_with_401_when_token_wrong` exists, but `bowerbird export`'s 401 path (which routes through `http_get_session_detail`, not `http_get_events`, because the pre-check fires first) was untested. A user with a stale `~/.bowerbird/config.toml` or expired `BOWERBIRD_TOKEN` should get the same clear "check your token" message from either command; the only way to keep that promise stable is to test both paths.

**Gap D — Story Task 4.5's truncate-on-overwrite contract for `-o <path>` was undocumented in tests.** The task spec explicitly says "open with `OpenOptions::new().create(true).truncate(true)` (a re-export overwrites)". Implementation uses `File::create`, which truncates by default — but `File::create` and `OpenOptions::new().write(true).open()` (which appends if the file already exists) differ by one method call. A well-intentioned refactor to "use OpenOptions for clarity" without `.truncate(true)` would silently break the documented overwrite semantics.

## Generated Tests

### Daemon contract — `crates/daemon/tests/contract_daemon.rs::story_4_1_replay` (2 new tests)

- **`replay_skips_blank_and_comment_lines`** — Closes Gap A. Body is a mix of `#`-prefixed comments, blank lines, two real event lines, an inline comment between them, and a trailing newline. Asserts `replayed_count == 2` and `parse_errors.is_empty()`. The trailing-newline assertion is the load-bearing piece: `body.split(|&b| b == b'\n')` produces a final empty slice that the handler must skip rather than treat as an empty-string parse error.
- **`replay_with_only_comments_replays_zero_events`** — Closes Gap A's smallest case. Body is exclusively `#`-comment and blank lines. Asserts `200 OK` with `replayed_count: 0, parse_errors: []`. Pins the "empty effective body is not an error" contract — a future "should we 400 on empty replay requests?" refactor trips this test.

### CLI export E2E — `tests/cli_export.rs` (3 new tests)

- **`export_fails_clearly_when_daemon_down`** — Closes Gap B. Mirrors `replay_fails_clearly_when_daemon_down` exactly: no daemon started, `bowerbird export any-session-id`, assert `cannot reach daemon` on stderr and non-zero exit.
- **`export_fails_with_401_when_token_wrong`** — Closes Gap C. Starts the daemon with the test token, then runs `bowerbird export` under `BOWERBIRD_TOKEN=wrong-token`. Asserts `daemon rejected bearer token` on stderr and non-zero exit. Verifies the export's `http_get_session_detail` pre-check fires the 401 path correctly.
- **`export_overwrites_existing_output_file`** — Closes Gap D. Pre-seeds the output file with sentinel garbage (`STALE GARBAGE LINE THAT IS NOT JSONL\n`) that would fail JSONL parsing, then runs `bowerbird export session-alpha -o <pre-seeded-file>`. Asserts the garbage is gone and every non-empty line parses as `protocol::Event`. Catches the regression mode "OpenOptions::open without .truncate(true)".

## Coverage

| AC | Coverage source |
| --- | --- |
| AC #1 (replay forwards through broadcast) | `replay_forwards_events_through_broadcast_path` + `replay_continues_on_per_line_parse_error` + new `replay_skips_blank_and_comment_lines` + new `replay_with_only_comments_replays_zero_events` + 3 CLI replay tests |
| AC #2 (export to JSONL) | `export_writes_jsonl_of_session_events_to_stdout` + `export_writes_to_file_when_output_flag_given` + `export_returns_session_not_found_for_unknown_id` + `export_round_trips_through_replay` + 3 new export-failure-path tests |
| AC #3 (bundled fixture) | `replay_with_no_argument_uses_bundled_fixture` + `bundled_fixture_is_valid_jsonl` + 2 `count_fixture_shape` unit tests |
| AC #4 (multi-session fan-out) | `replay_emits_state_frames_for_each_session` + `bundled_fixture_spans_at_least_two_sessions` |
| AC #5 (no timing preservation) | `replay_dropped_event_id_and_created_at_are_reassigned` |
| AC #6 (architecture.md updates) | `architecture_md_lists_replay_and_export_as_shipped` |
| AC #7 (protocol-changelog) | `protocol_changelog_documents_post_replay_endpoint` |

| File | Tests before | Tests after |
| --- | --- | --- |
| `crates/daemon/tests/contract_daemon.rs::story_4_1_replay` | 6 | 8 |
| `tests/cli_replay.rs` | 5 | 5 |
| `tests/cli_export.rs` | 4 | 7 |
| `tests/cli_replay_fixture.rs` | 5 | 5 |
| **Story 4.1 total** | **20** | **25** |

## Verification

```sh
cargo fmt --all -- --check                                          # clean
cargo clippy --workspace --all-targets -- -D warnings               # 0 warnings
cargo test -p bowerbird-daemon --test contract_daemon \
  story_4_1_replay -- --test-threads=1                              # 8 passed
cargo test --test cli_replay --test cli_export --test cli_replay_fixture \
  -- --test-threads=1                                               # 17 passed (3 suites)
```

## Next Steps

- All 25 Story 4.1 tests pass; the 5 new tests close the gaps identified above without introducing new infrastructure.
- No new dependencies. The daemon contract tests reuse the existing `wired_state` helper and `auth_post` builder; the CLI export tests reuse the existing `bowerbird_cmd_in` + `start_daemon` + `stop_daemon` helpers.
- Future Story 4.2 (reference examples) will exercise the bundled fixture from another consumer — the existing `bundled_fixture_is_valid_jsonl` doc-drift guardrail catches the cross-consumer drift hazard without needing a separate test in this story.
- The `replay_channel_full_returns_parse_error` case (the `TrySendError::Full` arm in `crates/daemon/src/api/replay.rs:115`) remains untested — it is hard to trigger deterministically without artificially blocking the writer task. Leaving it deferred; the runtime guard exists, but a regression there is low-probability and self-evident (the "channel full" message appears in parse_errors).
