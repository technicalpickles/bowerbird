# Test Automation Summary — Story 4.2

Generated 2026-05-25 via `bmad-qa-generate-e2e-tests`. Supersedes the Story 4.1 summary.

## Baseline coverage already in place

Story 4.2 (three TypeScript reference examples — `multi-session-router`, `event-log-viewer`, `reconnect-recovery`) landed with substantial test coverage from the dev-story pass — 12 dedicated tests before this QA run:

- **`tests/cli_examples.rs`** (4 tests) — `multi_session_router_routes_state_frames_for_both_fixture_sessions` (AC #1), `event_log_viewer_paginates_session_history_and_renders_tool_calls` (AC #2), `reconnect_recovery_recovers_after_close_frame_and_resumes` (AC #3 Close branch), `examples_fail_clearly_when_daemon_down` (daemon-down error path for all three). Each orchestrates a real `bowerbird-daemon` subprocess plus a Node subprocess running the example's `src/index.ts`, then asserts the canonical stdout/stderr shape.
- **`tests/cli_examples_drift.rs`** (6 tests) — `each_example_has_required_files`, `each_example_package_json_declares_node_22_6_engine`, `each_example_source_carries_cookbook_anchors`, `architecture_md_describes_examples_as_typescript_not_cargo`, `examples_readme_reconciliation_note_present`, `examples_not_in_root_cargo_toml_members`. Hermetic doc-drift guardrails per Epic 3 retro agreement A7 — no daemon, no Node, fast.
- **`examples/reconnect-recovery/tests/recover.test.ts`** (2 tests) — `recover fetches missed events and updates the cursor`, `recover returns 0 when no events past the cursor`. Node-built-in `--test` runner covering the AC #3 `Dropped` branch as a compiled assertion against an in-process fake daemon (no real lag burst required).

All 12 baseline tests pass under `cargo test --workspace -- --test-threads=1` (Rust) and `npm test` (Node).

## Gap analysis

Four gaps remained after that baseline. Each is a silent-failure mode — a regression here does not break any existing test, but a behavior the AC or story task promises stops being enforced.

**Gap A — Multi-session-router stderr `new session: ...` lines unasserted.** Story 4.2 Task 7.4 explicitly says: *"assert the example logged `new session: claude/session-alpha` and `new session: claude/session-beta` to stderr."* The baseline smoke test pipes stderr but never reads it. AC #1's stderr-side observable ("treating a previously-unseen `(source, session_id)` as a 'new session appeared' event and logging it on stderr") was therefore unenforced — a refactor that quietly dropped the stderr log would still pass the existing test.

**Gap B — Event-log-viewer default session id never exercised.** The example defaults to `session-alpha` when `process.argv[2]` is undefined (`src/index.ts:139`). The baseline smoke test always passes `"session-alpha"` explicitly, so the default-arg branch is dead code under test. AC #2's "CLI arg: a session id, default `session-alpha`" was a documented contract with no enforcement.

**Gap C — Event-log-viewer behavior for unknown session ids unpinned.** The example's `if (res.status === 404)` handler at `src/index.ts:100-103` anticipates a 404 the daemon does not actually return — the daemon serves `GET /sessions/<unknown-id>/events?since=0` with HTTP 200 and an empty events array. The semantic contract a presenter relies on ("ask for any session id and get a renderable response") was undocumented. A future daemon change that flipped to actual 404s would change observable presenter behavior with no test catching the regression.

**Gap D — Recover()'s gap-detection and multi-session cursor-advancement branches uncovered.** The baseline Node tests covered only the happy path (events past the cursor) and the no-op case (cursor past all events). Two structurally distinct branches in `examples/reconnect-recovery/src/index.ts` had no coverage: (1) the gap-warning branch at lines 167-172 fires when `cursor.lastEventId < oldest_available_event_id - 1`, and (2) the cross-session cursor-advancement loop at lines 146-186 maintains a *global* `cursor.lastEventId` across multiple sessions. Either branch could regress silently against the existing test suite.

## Generated tests

### Rust smoke tests — `tests/cli_examples.rs`

Two new test functions; one existing test gained a stderr-side assertion.

1. **`multi_session_router_routes_state_frames_for_both_fixture_sessions`** — *Modified.* Added a background stderr-drainer thread (same shape as the existing `reconnect_recovery` test) so stderr lines survive the test run. After the child exits, the test asserts the drained stderr contains both `new session: claude/session-alpha` and `new session: claude/session-beta`. Closes Gap A.

2. **`event_log_viewer_defaults_to_session_alpha_when_no_arg`** — *New.* Spawns the event-log-viewer with no CLI args. Asserts exit 0 and stdout containing 6 lines (session-alpha's bundled-fixture event count). The default-arg branch is now exercised. Closes Gap B.

3. **`event_log_viewer_renders_empty_for_unknown_session`** — *New.* Spawns the event-log-viewer with `definitely-not-a-real-session`. Asserts exit 0 with empty stdout — pins the daemon's *current* "200 OK + empty events" contract for unknown session ids. Includes an inline discovery note pointing at the dead-code 404 handler in the example, so a future daemon API tightening surfaces as a test failure rather than a silent presenter behavior change. Closes Gap C.

### Node `--test` tests — `examples/reconnect-recovery/tests/recover.test.ts`

Two new test cases.

4. **`recover handles an unrecoverable gap (cursor predates oldest_available)`** — *New.* Fake daemon serves events with `event_id` starting at 10; cursor starts at 1. Shims `process.stderr.write` to capture writes; asserts the `gap unrecoverable for session rotated-session` warning fires AND the recovered count is 2 AND the cursor advances to 11. Restores `process.stderr.write` on exit. Closes Gap D (gap-detection half).

5. **`recover advances cursor to the max across multiple sessions`** — *New.* Fake daemon serves two sessions with non-overlapping event_id ranges (session-one: 1, 2; session-two: 3, 4). Asserts `recover()` returns 4 (events across both sessions) AND `cursor.lastEventId` ends at the global max (4), not per-session. Closes Gap D (cross-session half).

## Verification

```sh
cargo test --test cli_examples -- --test-threads=1
# Result: 6 passed in 5.72s (4 baseline + 2 new)

cargo test --test cli_examples_drift -- --test-threads=1
# Result: 6 passed (no changes to drift suite)

cd examples/reconnect-recovery && node --experimental-strip-types --test 'tests/**/*.test.ts'
# Result: 4 passed (2 baseline + 2 new)

cargo fmt --all -- --check
# Result: clean

cargo clippy --workspace --all-targets -- -D warnings
# Result: 0 warnings
```

## Coverage tally

| Surface | Before QA | After QA |
|---|---|---|
| Rust smoke tests (`tests/cli_examples.rs`) | 4 | 6 |
| Rust drift tests (`tests/cli_examples_drift.rs`) | 6 | 6 |
| Node `--test` cases (`recover.test.ts`) | 2 | 4 |
| **Total dedicated tests** | **12** | **16** |

## ACs traced

- **AC #1 — multi-session-router** — fully covered. Snapshot fan-out, both sessions surfacing as map entries, stdout JSON shape, stderr new-session lines, Close-frame exit 0.
- **AC #2 — event-log-viewer** — fully covered. Cursor-pagination loop, tab-separated render shape, default session id, unknown-session graceful render, daemon-down failure, kind/tool/reaction sequence per bundled fixture.
- **AC #3 — reconnect-recovery** — fully covered. Close-branch via `bowerbird stop` + restart + replay; Dropped-branch via `recover()` unit tests including happy-path, no-op-past-cursor, gap-detection, and multi-session cursor advancement; idle-timeout clean exit.
- **AC #4 — CI smoke for all three examples** — covered by `tests/cli_examples.rs` running under `cargo test --workspace -- --test-threads=1` with `actions/setup-node@v4` pinning Node 22.6 in CI.
- **AC #5 — cookbook anchors present** — covered by `tests/cli_examples_drift.rs::each_example_source_carries_cookbook_anchors`.

## Next steps

- Run the augmented suite in CI on the next PR push; the Node-side glob (`'tests/**/*.test.ts'` in `package.json`) is Node-version-portable so the CI runner's Node 22.6 pin and contributor environments on Node 23/24 both work.
- The discovery note in `event_log_viewer_renders_empty_for_unknown_session` should be revisited if the daemon's REST surface ever returns actual HTTP 404s for unknown session ids — the dead-code 404 handler in `event-log-viewer/src/index.ts` would then become live, and the test should flip to asserting the error-exit shape instead of the empty-render shape.
- The `process.stderr.write` shim in the recover gap-detection test is restored on exit but adds a small surface-area risk: a future test inserted between save and restore could see logs vanish. Keeping the shim block tightly scoped to the single test is the discipline the file already follows; revisit if more tests need stderr capture.
