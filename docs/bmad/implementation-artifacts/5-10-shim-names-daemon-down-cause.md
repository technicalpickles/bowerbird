# Story 5.10: Shim names the cause on daemon-down

Status: review

<!-- Note: Validation is optional. Run validate-create-story for quality check before dev-story. -->

## Story

As a Claude Code user,
I want the shim to print one human-readable line naming the cause when it exits non-zero (daemon down, bad input),
so that a daemon outage doesn't render as Claude Code's generic causeless `No stderr output` hook error on every tool call.

## Context

Dogfooding session `ad3eaed4` (2026-06-01) rebooted; `bowerbird-daemon` did not come back. For ~90 seconds every Pre/PostToolUse hook stacked this in the transcript:

```
PreToolUse:Bash hook error    Failed with non-blocking status code: No stderr output
PostToolUse:Bash hook error   Failed with non-blocking status code: No stderr output
```

The shim's exit-code contract is deliberate and correct: `Error::Connect` (daemon unreachable) → **exit 1** ("surface a real failure"); mid-write / daemon-answered errors → **exit 0** (fire-and-forget, NFR20); **exit 2 forbidden** (it blocks the tool call). The shim logs the cause to `~/.bowerbird/shim.log` (`main.rs:28`) but **never writes stderr**. Claude Code, on a non-zero hook exit with empty stderr, renders the generic `No stderr output` message — which names neither bowerbird nor the daemon, and repeats on every call for the whole outage. "Noisy enough to alarm, mute enough to be useless" (`docs/dogfooding-feedback.md` Finding 2).

This story makes the exit-1 surface **name its cause**. It is the smallest of the four dogfood-triage stories (Finding 2 — minor). The exit-1/exit-0 contract is unchanged.

## Acceptance Criteria

1. **AC1 — daemon-down names the cause on stderr.** When the shim fails to connect to the ingest socket (`Error::Connect`, daemon unreachable), it writes exactly one line to **stderr** naming the cause: `bowerbird: daemon not running, event dropped (see <log-path>)` when the failure was recorded to the file log, where `<log-path>` is the resolved shim-log path (default `~/.bowerbird/shim.log`, or `BOWERBIRD_SHIM_LOG` when set). When the file log append itself fails (the path is unwritable, a directory, ...), the `(see <log-path>)` pointer is **omitted** and the line is exactly `bowerbird: daemon not running, event dropped` — Claude is never sent to a file that was not written (pass-1 F2). When the append succeeds, the existing ERROR line in the file log is still written; either way the exit code stays **1** (NFR20 contract intact — `Error::Connect → exit 1`).

2. **AC2 — every exit-1 variant names a cause; exit-0 stays stderr-silent.** Every error whose `exit_code()` is `1` (the ERROR-level class: `Stdin`, `StdinEmpty`, `StdinNotJsonObject`, `StdinTooLarge`, `StdinJson`, `Connect`, `LogIo`, `BadArgs`, `NoHome`) emits one stderr line of the form `bowerbird: <cause> (see <log-path>)` when the file log append succeeded, or the pointer-less `bowerbird: <cause>` when it did not (and unconditionally pointer-less on the pre-run no-log-path branch — AC4). Every error whose `exit_code()` is `0` (the WARN-level class: `SocketIo`, `BadResponse`, `Backpressure`, `Backpressure503`, `DaemonError400`) writes **nothing** to stderr — the daemon is up and answering, fire-and-forget per NFR20, so Claude must see success, not a warning.

3. **AC3 — the success path stays stderr-silent and allocation-free.** On a `200` response the shim writes nothing to stdout or stderr and creates no log file (unchanged). The stderr-emit logic lives **only** in the `Err(_)` arm of `main` (and the pre-`run` log-path-resolution failure branch); the `run()` success path and `socket::send` happy path are untouched, so the hot-path budget and `shim/benches/hot_path.rs` are unaffected.

4. **AC4 — pre-run failures are not causeless either.** When `resolve_log_path()` fails (no `HOME`, no `BOWERBIRD_SHIM_LOG` — the branch at `main.rs:18-21` that currently `exit(1)`s silently), the shim writes one stderr line naming the cause (e.g. `bowerbird: HOME not set, cannot record event`) before exiting 1. This line omits the `(see <log-path>)` pointer because no log path could be resolved.

5. **AC5 — a stderr write that itself fails is swallowed.** Writing to stderr never panics and never changes the exit code; a failed stderr write is ignored exactly as a failed log-append already is (`main.rs:28`). The shim's `#![deny(unsafe_code)]` / no-`unwrap`-on-error-path discipline holds.

6. **AC6 — regression coverage.** The contract suite proves: (a) connect-refused now emits the named stderr line *and* still exits non-zero with the file log intact; (b) the `200` and `503` paths leave stderr empty; (c) a unit test asserts the stderr-hint partition matches the exit-code partition (`Some` iff `exit_code() == 1`, `None` iff `exit_code() == 0`) for every variant — mirroring the existing `exit_code_never_2` / `level_matches_exit_code` table tests so a future variant cannot be added without a deliberate hint decision.

## Tasks / Subtasks

- [x] **Task 1 — add the per-variant stderr hint to `Error` (AC1, AC2, AC5)**
  - [x] In `crates/shim/src/error.rs`, add `pub fn stderr_hint(&self) -> Option<&'static str>`. Return `Some(<cause>)` for every exit-1 variant, `None` for every exit-0 variant. Keep the partition in lockstep with `exit_code()` — match the same arms in the same order so reviewers can diff them side by side.
  - [x] Cause strings (all `&'static str`, no allocation):
    - `Connect { .. }` → `"daemon not running, event dropped"`
    - `Stdin(_)` → `"could not read hook payload from stdin"`
    - `StdinEmpty` → `"empty hook payload"`
    - `StdinNotJsonObject` → `"hook payload was not a JSON object"`
    - `StdinTooLarge { .. }` → `"hook payload exceeds size cap"`
    - `StdinJson(_)` → `"hook payload was not valid JSON"`
    - `LogIo(_)` → `"could not write shim log"`
    - `BadArgs(_)` → `"invalid shim arguments"`
    - `NoHome` → `"HOME not set, cannot record event"`
    - all exit-0 variants → `None`
  - [x] Document on the method that the WARN/exit-0 class returns `None` *by contract* (NFR20: daemon up and answering → Claude must see success).

- [x] **Task 2 — emit the stderr line in `main`'s failure arm (AC1, AC3, AC4, AC5)**
  - [x] In `crates/shim/src/main.rs`, in the `Err(e)` arm of `match run(&log_path)`, after the existing `log::append(...)` call, write the stderr line when `e.stderr_hint()` is `Some`:
    ```rust
    if let Some(hint) = e.stderr_hint() {
        let _ = writeln!(io::stderr(), "bowerbird: {hint} (see {})", log_path.display());
    }
    ```
    Use `let _ =` to swallow write errors (AC5); add `use std::io::Write;` (the trait is needed for `writeln!` on `io::stderr()`). **Done** via `use std::io::{self, Read, Write};` (`self` brings the `io` module into scope for `io::stderr()`).
  - [x] In the `resolve_log_path()` failure branch (`main.rs:18-21`), before `std::process::exit(1)`, write `bowerbird: HOME not set, cannot record event` to stderr (no `(see ...)` suffix — no log path resolved). Swallow the write error.
  - [x] Do **not** touch `run()`'s success path or `socket.rs`. Confirm the `Ok(()) => exit(0)` arm emits nothing.

- [x] **Task 3 — unit test the hint partition (AC2, AC6)**
  - [x] In `error.rs`'s `#[cfg(test)] mod tests`, add `stderr_hint_matches_exit_code` iterating `sample_variants()`: assert `e.stderr_hint().is_some() == (e.exit_code() == 1)` and `e.stderr_hint().is_none() == (e.exit_code() == 0)` for every variant. This is the canary against a new variant getting a hint (or no hint) by accident.
  - [x] Assert `Error::Connect { .. }.stderr_hint() == Some("daemon not running, event dropped")` explicitly (pins the dogfood-relevant wording) — `connect_hint_names_the_daemon_down_cause`.

- [x] **Task 4 — contract tests for the surfaced stderr (AC1, AC2, AC3, AC6)**
  - [x] In `crates/shim/tests/contract_shim.rs`, extend or add a sibling to `shim_exit_nonzero_on_connection_refused`: assert `out.stderr` is non-empty, contains `bowerbird:`, contains `daemon not running`, and contains the resolved log path; the file log still contains `ERROR` and the socket path; exit code is non-zero and `!= 2`. (The harness already routes `BOWERBIRD_SHIM_LOG` via `run_shim_with_env`, so the log path in the stderr line is the test's temp log.) **Done** — extended `shim_exit_nonzero_on_connection_refused` in place.
  - [x] Add an assertion (extend `shim_exit_0_on_200` / `shim_silent_on_success`) that `out.stderr` is **empty** on the `200` path (already asserted in `shim_exit_0_on_200` — keep it green). **Kept green** (pre-existing assertion).
  - [x] Add an assertion to `shim_exit_0_on_503_with_warning_log` (and/or `shim_exit_0_on_400_from_daemon`) that `out.stderr` is **empty** on the exit-0 daemon-answered paths — the NFR20 regression guard that the new behavior does not leak into exit-0. **Done on both.**

- [x] **Task 5 — record the deferred follow-up (proposal §6)**
  - [x] Append a deferred-work entry to `docs/bmad/implementation-artifacts/deferred-work.md` for **cross-invocation coalescing / rate-limiting** of the per-call error across a multi-call outage, and the **exit-0-vs-exit-1 reconsideration** once daemon-down is distinguishable from a genuine shim bug. Note why it is deferred: the shim is stateless per invocation, so cross-call rate-limiting needs shared state it does not have (proposal §6, `sprint-change-proposal-2026-06-01-dogfood-triage.md`).

- [x] **Task 6 — gates (all ACs)**
  - [x] `cargo test -p bowerbird-shim` green (unit + contract) — 24 passed (6 unit + 18 contract).
  - [x] `cargo fmt --check` and `cargo clippy --all-targets --workspace -- -D warnings` green.
  - [x] Confirm **no** `crates/protocol/src` change → **no** `docs/protocol-changelog.md` entry and the changelog gate stays green (this story touches only the shim + docs). Verified `git status` shows only `crates/shim/*` + `docs/*`.
  - [x] `shim_source_has_no_async` stays green (no Tokio sneaks in); `cargo build --release` of the shim still succeeds under the `release-shim` profile — built clean.

### Review Findings

- [x] [Review][Patch] Sprint status YAML is invalid because the active `last_updated` value is an unquoted scalar containing `bowerbird: <cause>` [docs/bmad/implementation-artifacts/sprint-status.yaml:111] — The active `last_updated:` line includes `"bowerbird: <cause> (see <log>)"` inside an unquoted YAML plain scalar. `:` followed by a space is parsed as a mapping delimiter, so status automation cannot load the file; verified locally with Ruby Psych: `mapping values are not allowed in this context at line 111 column 138`. Fix by quoting the active `last_updated` string or converting it to a folded scalar (`last_updated: >-`) and keep future colon-bearing snippets inside a quoted/folded value. While touching this file, sync `5-10-shim-names-daemon-down-cause` to `in-progress` so sprint tracking matches this unresolved review state.
  - **Resolved:** The active `last_updated` value is now wrapped in a single-quoted YAML scalar, so its embedded colons no longer parse as mapping delimiters. Verified with `Psych.parse_file` (no syntax error) — the earlier `mapping values are not allowed in this context at line 111 column 138` is gone. (The remaining `Psych::DisallowedClass: Date` under `safe_load` is unrelated: the pre-existing `generated:`/`last_updated:` date scalars; consumers load with `permitted_classes: [Date]`, which loads the file cleanly.) Sprint tracking is now consistent because the findings were resolved in this same session: both the story Status and `development_status[5-10]` are `review` (rather than parking at `in-progress`).

- [x] [Review][Patch] The stderr hint can point at a log file even when the log append failed [crates/shim/src/main.rs:33, crates/shim/src/main.rs:40] — `main` currently ignores `log::append(&log_path, ...)` and then unconditionally emits `(see <log_path>)` for every exit-1 hint. If the append failed because the log path is a directory, unwritable, or otherwise unusable, Claude will send the user to a diagnostic file that was not written; the `Error::LogIo(_)` hint is especially contradictory because it says the log could not be written while still adding `(see <log>)`. Capture the append result and omit or change the pointer when the log write failed (or make the hint structured with an `include_log_path` flag). Add a contract test using an unwritable/directory `BOWERBIRD_SHIM_LOG` path.
  - **Resolved:** `main` now binds `let log_written = log::append(...).is_ok()` and only appends `(see <log>)` when `log_written`; otherwise it emits the pointer-less `bowerbird: <cause>` line. New contract test `shim_omits_log_pointer_when_log_append_fails` points `BOWERBIRD_SHIM_LOG` at a directory (open(2) → EISDIR), asserts exit 1 / not 2 and exact stderr `bowerbird: daemon not running, event dropped\n` (no pointer).

- [x] [Review][Patch] AC4's pre-run no-HOME/no-log stderr branch has no contract coverage [crates/shim/src/main.rs:20, crates/shim/tests/contract_shim.rs:70] — The new `resolve_log_path()` failure branch writes `bowerbird: HOME not set, cannot record event` before exiting 1, but the contract helper always sets `BOWERBIRD_SHIM_LOG`, so the branch is never exercised. Add a subprocess contract test that runs `bowerbird-shim` with both `HOME` and `BOWERBIRD_SHIM_LOG` removed, asserts exit code 1 / not 2, empty stdout, and exact pointer-less stderr `bowerbird: HOME not set, cannot record event\n`.
  - **Resolved:** New contract test `shim_names_cause_when_no_home_and_no_log_path` spawns the shim with `env_remove("HOME")` + `env_remove("BOWERBIRD_SHIM_LOG")`, asserting exit 1 / not 2, empty stdout, and exact stderr `bowerbird: HOME not set, cannot record event\n`.

- [x] [Review][Patch] The connect-refused stderr regression test allows extra lines and wording drift [crates/shim/tests/contract_shim.rs:203] — AC1 requires exactly one line of the form `bowerbird: daemon not running, event dropped (see <log-path>)`, but the test only checks that stderr is non-empty and contains `bowerbird:`, `daemon not running`, and the log path. A regression that prints duplicate lines or drops `event dropped` would still pass. Assert exact stderr with `format!("bowerbird: daemon not running, event dropped (see {})\n", log.display())`.
  - **Resolved:** `shim_exit_nonzero_on_connection_refused` now asserts exact equality: `assert_eq!(stderr, format!("bowerbird: daemon not running, event dropped (see {})\n", log.display()))`. (The temp log path has no control chars, so the sanitized pointer equals `log.display()`.)

- [x] [Review][Patch] An env-provided log path can break the one-line stderr contract [crates/shim/src/main.rs:43, crates/shim/src/main.rs:123] — `resolve_log_path()` accepts `BOWERBIRD_SHIM_LOG` unchanged and `main` writes `log_path.display()` directly into stderr. A path containing a newline or control character can turn the promised one-line hook message into multiple lines or misleading text. Escape or sanitize the path before embedding it in stderr (for example, a lossy string with `escape_default()` or an equivalent one-line display helper), and add a regression with a newline-containing `BOWERBIRD_SHIM_LOG`.
  - **Resolved:** Added `fn one_line_path(&Path) -> String` that escapes every `char::is_control()` byte (newline, CR, ANSI ESC, ...) via `escape_default` while keeping printable Unicode verbatim; `main` embeds `one_line_path(&log_path)` instead of `log_path.display()`. Unit tests `one_line_path_passes_normal_paths_through` / `_escapes_control_chars` / `_keeps_non_ascii_verbatim` pin the behavior; contract test `shim_stderr_stays_one_line_with_newline_in_log_path` proves a newline in `BOWERBIRD_SHIM_LOG` yields exactly one stderr line with the newline escaped to `\n`.

### Review Findings — Pass 2 (2026-06-08)

bmad-code-review pass 2 reviewed the cumulative story footprint (both 5.10 commits) across three layers (Blind Hunter, Edge Case Hunter, Acceptance Auditor). Outcome: 3 patch findings, 0 decision-needed, 0 deferred, 5 dismissed as noise. The dismissed items (verified against the project): the EISDIR test premise is correct (`log.rs` uses `.append(true).create(true)`, so open on a directory returns `Err`); std `io::stderr()` is unbuffered so the missing flush before `exit` is harmless; `to_string_lossy` U+FFFD replacement is acceptable best-effort and cannot break the line; the connect-refused test's `log.display()` assertion is safe because temp paths carry no control chars (and would fail loudly, not silently).

- [x] [Review][Patch] `one_line_path` escapes `is_control()` only, so the F5 one-line guarantee still breaks on Unicode line separators [crates/shim/src/main.rs:69-80] — Raised independently by the Blind Hunter and Edge Case Hunter (and verified with `rustc`): U+2028 (LINE SEPARATOR), U+2029 (PARAGRAPH SEPARATOR), and the bidi override U+202E all report `char::is_control() == false`, so they pass through `one_line_path` verbatim. A `BOWERBIRD_SHIM_LOG` carrying U+2028/U+2029 (legal UTF-8 bytes in a unix filename) can still split the stderr hint into multiple lines in Unicode-aware renderers — exactly the AC1 "exactly one line" property the pass-1 F5 fix was meant to guarantee. The fix is incomplete, not wrong: extend the escape guard to also catch the line/paragraph separators (and, cheaply, the bidi formatting/override chars that can spoof Claude's transcript), and add U+2028 coverage to the `one_line_path_escapes_control_chars` unit test + the newline contract test.
  - **Resolved (pass 3 — superseded by the pass-3 byte-aware rewrite below):** `one_line_path` now sanitizes the raw unix path bytes (`OsStrExt::as_bytes()` + `utf8_chunks()`), escaping U+2028/U+2029 and the bidi controls (LRM/RLM/ALM, U+202A–U+202E, U+2066–U+2069) via a new `needs_escape` predicate. New unit test `one_line_path_escapes_unicode_separators_and_bidi` pins U+2028/U+2029/U+202E escaping.

- [x] [Review][Patch] Pre-run no-HOME stderr branch hardcodes the cause string instead of deriving it from the `Error`, duplicating the `NoHome` hint and bypassing the partition canary [crates/shim/src/main.rs:22-25] — Raised by the Blind Hunter and Acceptance Auditor. The pre-run `resolve_log_path()` failure arm writes the literal `"bowerbird: HOME not set, cannot record event"`, which duplicates `Error::NoHome.stderr_hint()` with no shared source, so a reword drifts them apart silently. It also means `stderr_hint_matches_exit_code` (the canary that sells "no variant ships without a deliberate hint decision") never guards the code that actually runs on the no-HOME path, and the hardcoded string becomes a mis-report if `resolve_log_path` ever grows a non-`NoHome` failure mode. Fix: route the arm through the returned `Error` — `Err(e) => { if let Some(h) = e.stderr_hint() { let _ = writeln!(io::stderr(), "bowerbird: {h}"); } std::process::exit(e.exit_code()); }`. Behavior is byte-identical today (NoHome → same string, exit 1) but now single-sourced and canary-guarded.
  - **Resolved (pass 3):** the `resolve_log_path()` failure arm now binds `Err(e)` and emits `e.stderr_hint()` / exits `e.exit_code()` exactly as proposed — single-sourced with `Error::NoHome` and guarded by `stderr_hint_matches_exit_code`. Behavior is byte-identical (`shim_names_cause_when_no_home_and_no_log_path` still asserts exact `bowerbird: HOME not set, cannot record event\n` / exit 1).

- [x] [Review][Patch] AC1/AC2 still describe an unconditional `(see <log-path>)` pointer, but F2 made it conditional [docs/bmad/implementation-artifacts/5-10-shim-names-daemon-down-cause.md:28,30] — Raised by the Acceptance Auditor. The pass-1 F2 fix correctly drops the `(see <log>)` suffix when the log append fails, so the live stderr line is `bowerbird: <cause>` in that case. AC1 and AC2 were never amended and still read as if the pointer is always present. The code is more correct than the spec; sync AC1/AC2 (or add a one-line note) so the acceptance criteria match the agreed conditional-pointer behavior.
  - **Resolved (pass 3):** AC1 and AC2 now spell out the conditional pointer (`(see <log-path>)` only when the file-log append succeeded; pointer-less `bowerbird: <cause>` otherwise), and the Completion Notes were synced to match.

### Review Findings — Pass 3 (2026-06-08)

bmad-code-review pass 3 reviewed the cumulative Story 5.10 footprint (`d7ab6bf^..HEAD` for `crates/shim/src/{error.rs,main.rs}`, `crates/shim/tests/contract_shim.rs`, and the story/docs touched by 5.10) across the required three layers: Blind Hunter, Edge Case Hunter, and Acceptance Auditor. Outcome after local triage: 4 patch findings, 0 decision-needed, 0 deferred, 10 dismissed as noise or out-of-scope. The dismissed items were: stderr flush before `std::process::exit` (std stderr handle is not buffered in the relevant way); `/dev/null`/special-file `BOWERBIRD_SHIM_LOG` pointers (user-directed override, not the story contract); `SocketIo` staying stderr-silent (explicit NFR20 exit-0 policy); exact wording assertions being too strict (AC1 makes the wording contractual); `sample_variants()` theoretical incompleteness (enum matches remain exhaustive and this is not a current miss); escaped path ambiguity (`\n` vs literal backslash-n is acceptable for a diagnostic hint); the non-ASCII test literal (intentional coverage); and helper/env fallback speculation not supported by the current resolver.

- [x] [Review][Patch] `one_line_path` is still not a faithful one-line renderer for every valid Unix log path [crates/shim/src/main.rs:70] — Raised by Blind Hunter, Edge Case Hunter, and Acceptance Auditor. Pass 2 already caught the `char::is_control()` gap for U+2028 LINE SEPARATOR, U+2029 PARAGRAPH SEPARATOR, and bidi formatting/override chars such as U+202E. The Edge Case layer adds a second concrete gap: `path.to_string_lossy()` replaces non-UTF-8 Unix filename bytes with U+FFFD, so the stderr hint can point at a path the user cannot actually open even though `log::append` wrote to the real byte path. Fix the renderer as a path-byte-aware sanitizer: on Unix, prefer `std::os::unix::ffi::OsStrExt::as_bytes()` and escape bytes/chars that can break the single-line transcript or spoof display (C0/DEL controls, UTF-8 line/paragraph separators, bidi format controls, invalid UTF-8 bytes as `\xNN`). Update the existing tests to cover U+2028/U+2029 and an invalid-UTF-8 filename; make the contract test assert the exact sanitized pointer, not only `contains("\\n")`.
  - **Resolved:** `one_line_path` rewritten to iterate the raw path bytes via `path.as_os_str().as_bytes().utf8_chunks()`: each valid `char` is escaped via `escape_default()` when `needs_escape(c)` is true (the new predicate catches `is_control()` **plus** U+2028/U+2029 and the bidi controls LRM/RLM/ALM, U+202A–U+202E, U+2066–U+2069), and each invalid (non-UTF-8) byte is rendered as `\xNN` instead of a lossy U+FFFD. New unit tests `one_line_path_escapes_unicode_separators_and_bidi` and `one_line_path_renders_invalid_utf8_bytes` pin both gaps; the existing `_escapes_control_chars` / `_keeps_non_ascii_verbatim` / `_passes_normal_paths_through` tests stay green. The contract test `shim_stderr_stays_one_line_with_newline_in_log_path` now asserts the **exact** sanitized pointer `bowerbird: daemon not running, event dropped (see <tmp>/foo\nbar.log)\n` rather than only `contains("\\n")`.

- [x] [Review][Patch] Pre-run no-HOME branch bypasses the `Error::stderr_hint()` contract [crates/shim/src/main.rs:22] — Reconfirmed by Blind Hunter and Acceptance Auditor. The `resolve_log_path()` failure arm hardcodes `bowerbird: HOME not set, cannot record event` and exits literal `1`, duplicating `Error::NoHome.stderr_hint()` in `crates/shim/src/error.rs:124`. That bypasses the `stderr_hint_matches_exit_code` canary and will silently drift if the NoHome wording or `resolve_log_path()` error surface changes. Route the returned error through the same machinery used by the normal failure arm: `Err(e) => { if let Some(hint) = e.stderr_hint() { let _ = writeln!(io::stderr(), "bowerbird: {hint}"); } std::process::exit(e.exit_code()); }`. Behavior is byte-identical today, but the cause and exit code become single-sourced.
  - **Resolved:** the failure arm now binds `Err(e)` and writes `e.stderr_hint()` / exits `e.exit_code()` verbatim as proposed. The hardcoded string is gone; `Error::NoHome` is the single source, now guarded by `stderr_hint_matches_exit_code`. `shim_names_cause_when_no_home_and_no_log_path` confirms byte-identical behavior (exact `bowerbird: HOME not set, cannot record event\n`, exit 1, empty stdout).

- [x] [Review][Patch] AC1/AC2 and completion notes still describe an unconditional `(see <log-path>)` pointer [docs/bmad/implementation-artifacts/5-10-shim-names-daemon-down-cause.md:28] — Reconfirmed by Acceptance Auditor. The pass-1 F2 resolution intentionally made the pointer conditional on `log::append` succeeding (`crates/shim/src/main.rs:37-58`) and added `shim_omits_log_pointer_when_log_append_fails`, but AC1/AC2 still say every exit-1 stderr line is `bowerbird: <cause> (see <log-path>)`. The Completion Notes and Change Log also still summarize the behavior as if the pointer is always present. Sync the story text to the implemented contract: include `(see <log-path>)` only when the file log was actually written; otherwise emit exactly `bowerbird: <cause>`.
  - **Resolved:** AC1 and AC2 now describe the conditional pointer explicitly (`(see <log-path>)` only when the file-log append succeeded; pointer-less `bowerbird: <cause>` when it failed, and unconditionally pointer-less on the pre-run no-log-path branch). The Completion Notes bullet for `main`'s `Err` arm was synced; this Change Log records the conditional behavior.

- [x] [Review][Patch] Persistent docs still state the old file-only/no-stderr shim failure contract [docs/bmad/planning-artifacts/prd.md:468] — Raised by Acceptance Auditor and verified locally. Story 5.10 intentionally creates a scoped exception for exit-1 failures, but the persistent docs still contradict it: PRD FR5 says the shim logs failures "without writing to stdout or stderr" (`docs/bmad/planning-artifacts/prd.md:468`), PRD Journey 4 still says "never stdout/stderr" (`docs/bmad/planning-artifacts/prd.md:258`), architecture still says "no stdout/stderr on any path" and "failure log only" (`docs/bmad/planning-artifacts/architecture.md:34`, `docs/bmad/planning-artifacts/architecture.md:658`), and project-context's shim hot-path rule says failure logging goes to a file, not stdout/stderr (`docs/bmad/project-context.md:348`) despite the Observability section already allowing the success-path-only exception (`docs/bmad/project-context.md:222`). Update the PRD, architecture, and project-context to preserve the real invariant: success and exit-0 daemon-answered paths stay stdout/stderr-silent; exit-1 shim failures emit one cause line to stderr, with the file log pointer only when the log append succeeded.
  - **Resolved:** updated PRD FR5 (`prd.md:468`) and Journey 4 (`prd.md:258`), architecture FR1–FR5 summary (`architecture.md:34`) and the shim hot-path rules (`architecture.md:657`), and project-context's Observability shim exception (`project-context.md:222`) + shim hot-path discipline rule (`project-context.md:348`). All now state the same invariant: success and exit-0 daemon-answered paths stay stdout/stderr-silent; exit-1 failures emit exactly one `bowerbird: <cause>` stderr line, with the `(see <log-path>)` pointer only when the log append succeeded.

### Open Decision — log path in stderr + strict one-line guarantee (raised after pass 3, 2026-06-08)

**Why this is here.** Three review passes converged on the same helper from different angles: F5 (pass 1) escaped control chars, pass 2 added Unicode line/paragraph separators + bidi controls, pass 3 added non-UTF-8 byte rendering. Each fix was "incomplete, not wrong." That escalation pattern is the reviews circling a design choice that was never decided: **the exit-1 stderr line interpolates an environment-controlled value (`BOWERBIRD_SHIM_LOG`) into Claude's transcript, and asserts an "exactly one line" property — neither the threat model nor the one-line requirement was ever grounded.** All of `one_line_path` / `needs_escape` and ~4 of the shim's tests exist only to make that interpolation safe.

**What we now know (grounded, not assumed).** Investigated Claude Code's actual hook-error rendering via binary spelunking (`claude 2.1.168`). On a non-zero non-blocking hook exit the message is built as:

```js
stderr: `Failed with non-blocking status code: ${TH.stderr.trim() || "No stderr output"}`
```

`TH.stderr` is the hook process's full stderr; it is only `.trim()`med. There is **no** first-line extraction, `.slice()`, or truncation at any of the three hook-stderr render sites. So:

- **The empty-stderr fix is correct and necessary.** `… || "No stderr output"` is exactly the causeless message Finding 2 reported; emitting one named line fixes it. (Not in question.)
- **Claude renders multi-line stderr faithfully** — it does not collapse, truncate, or first-line it. A newline in the log path produces an *ugly multi-line* hook error, not a *broken* one.
- Therefore **"exactly one line" is a UX/aesthetic property, not a correctness one.** Control-char / newline / separator escaping prevents an ugly multi-line blob repeating per tool call; it is not preventing a parse break.
- The **bidi-control** escaping is the one item defending an actual integrity property (transcript spoofing via visual reordering) — real but low severity, and only reachable by someone who set their own `BOWERBIRD_SHIM_LOG` to a pathological value.

**The decision to make (next review/dev cycle, or a follow-up story):**

1. **Does the line need the variable path at all?** The threat model for every path-escaping finding is "a hostile/malformed `BOWERBIRD_SHIM_LOG`" — but the person who set that env var is the person reading the transcript. If the path is dropped in favor of a fixed pointer (e.g. `bowerbird: daemon not running, event dropped (see your shim log)`), the entire `one_line_path` / `needs_escape` helper and its tests delete themselves, and the real path stays where it is already safe (the file log + docs). Cost: a user who customized `BOWERBIRD_SHIM_LOG` doesn't see *which* file in the one-liner.
2. **If the path stays, is the strict one-line guarantee worth the byte-level sanitizer,** now that we know multi-line only costs aesthetics? A cheaper `replace control/newline → space`, best-effort, may be enough — keeping only the bidi escaping as the deliberate anti-spoofing measure.

**Recommendation:** option 1 (fixed pointer, drop the variable path) is the smallest faithful design and erases the recurring finding class; keep a one-line bidi/control note for the fixed-string case (trivially satisfied). This is a contract/spec choice, so it belongs to whoever owns the next pass — flagged, not silently changed. Related to the exit-0-vs-exit-1 reframing in the **Saved question** below (both ask "is the surfaced-failure contract right?").

## Dev Notes

### What this changes (and what it must NOT)

- **One concept:** the shim's failure path gains a stderr voice. Nothing else moves. The exit-code contract (`error.rs::exit_code`) is **unchanged** — `Connect` stays exit 1, the daemon-answered class stays exit 0, exit 2 stays forbidden. NFR20 (`docs/bmad/planning-artifacts/prd.md:566`) is intact.
- **Why stderr is the right surface here, despite the project rule.** `project-context.md` §Observability says "Logging on failure goes to a file, not stdout/stderr ... anything on stdout/stderr risks polluting Claude's experience." That rule protects the *success* path. This story is a scoped, deliberate exception for the *failure* path only: on exit-1 Claude **already** surfaces a hook error — the choice is between Claude's causeless `No stderr output` and a line that names bowerbird. The success path stays silent, so the rule's intent (don't pollute normal operation) is preserved.
- **Hot path is untouched.** All new code is in `main`'s `Err` arm and the pre-`run` log-path branch. `run()` success, `socket::send`'s 200 path, and the bench (`crates/shim/benches/hot_path.rs`) see no change. Hints are `&'static str` (zero allocation); the only allocation is the `writeln!` format on the failure path, which is already an exiting/failing path.

### Current state of the files being modified (read before editing)

- **`crates/shim/src/main.rs`** — `main()` (lines 13-32): resolves `log_path` (silent `exit(1)` if unresolvable, line 18-21), runs `run()`, and on `Err(e)` does `log::append(&log_path, e.level(), &e.to_string())` then `exit(e.exit_code())`. The log-append error is already swallowed with `let _ =` (line 28) — mirror that pattern for the stderr write. `run()` (34-74) does the work; its `Ok` path returns `Ok(())` → `exit(0)`, no output. **Do not** add output to `run()`.
- **`crates/shim/src/error.rs`** — `Error` enum (5-56), `exit_code()` (66-87, the exit-1 vs exit-0 partition), `level()` (90-96, ERROR for exit-1 / WARN for exit-0). The `#[cfg(test)] mod tests` (99-151) has `sample_variants()` (covers all 14 variants), `exit_code_never_2`, `level_matches_exit_code` — model `stderr_hint_matches_exit_code` on these. `stderr_hint()` is the third method in the family (`exit_code` → `level` → `stderr_hint`); keep the match arms aligned with `exit_code()`.
- **`crates/shim/src/log.rs`** — `append()` writes one timestamped `{ts} {level} {message}\n` line, chmod 0600. Unchanged by this story; the file log keeps its current shape. The stderr line is *additional*, not a replacement.
- **`crates/shim/src/socket.rs`** — `send()` maps `UnixStream::connect` failure to `Error::Connect { path, source }` (line 19-22). That is the daemon-down error this story headlines. Unchanged.
- **`crates/shim/tests/contract_shim.rs`** — `run_shim_with_env` (70) spawns the shim via `assert_cmd::Command` with `BOWERBIRD_INGEST_SOCK` + `BOWERBIRD_SHIM_LOG` set to temp paths and returns `out` with `.status`, `.stdout`, `.stderr`. `shim_exit_nonzero_on_connection_refused` (169-200) already asserts non-zero exit + ERROR file log + socket path in the log; extend it (or add a sibling) to assert the stderr line. `shim_exit_0_on_200` (112-142) already asserts empty stderr on success — keep it.

### Decision: stderr for all exit-1 variants, not just `Connect`

The dogfood example message is daemon-specific, but the epics stub and `dogfooding-feedback.md` both frame the fix as "on the **exit-1 path**" — every exit-1 case is equally causeless in Claude today (a `BadArgs` install bug renders the same useless `No stderr output`). The `exit_code()`/`level()` machinery already partitions ERROR (exit-1) from WARN (exit-0) for free, so covering the whole ERROR class is barely more code than `Connect`-only and removes "causeless" everywhere it can occur. The exit-0 (WARN) class stays silent by contract because the daemon is up and answering (NFR20) — surfacing a warning there would regress fire-and-forget. See **saved question** below for the one residual judgment call.

### Project structure / conventions

- Aligns with the existing shim module layout (`error.rs` owns error→behavior mappings; `main.rs` owns process-exit orchestration). No new files, no new modules.
- Error discipline: `thiserror` only in the shim, small fixed enum, no `anyhow`, `#![deny(unsafe_code)]`, no `unwrap`/`expect` on the per-event path (`project-context.md` §Shim hot-path discipline). The stderr write uses `let _ =`, never `unwrap`.
- No new dependencies. `std::io::Write` is already available; `io::stderr()` is std.

### References

- [Source: docs/bmad/planning-artifacts/epics.md#Story 5.10] — story stub + scope (line 1223-1229).
- [Source: docs/bmad/planning-artifacts/sprint-change-proposal-2026-06-01-dogfood-triage.md#4.4] — Finding 2 change proposal (exit-1 stderr line, keep `Connect → exit 1`, coalescing deferred §6).
- [Source: docs/dogfooding-feedback.md#Finding 2 — the surfaced hook error is alarming and causeless] — the originating friction, transcript sample, and fix direction (lines 63-90).
- [Source: docs/bmad/planning-artifacts/prd.md#NFR20] — shim exit-code contract (non-zero on unreachable, 0 on mid-write/backpressure) (line 566).
- [Source: docs/bmad/project-context.md#Observability: tracing] — "Logging on failure goes to a file, not stdout/stderr"; this story's scoped failure-path exception.
- [Source: docs/bmad/project-context.md#Shim hot-path discipline] — no allocation/subprocess/async on success path; failure path is exempt but still no-`unwrap`.
- [Source: crates/shim/src/main.rs] — `main` error arm (line 23-31), log-path-resolution branch (18-21).
- [Source: crates/shim/src/error.rs] — `exit_code()` (66-87), `level()` (90-96), test table (99-151).
- [Source: crates/shim/src/socket.rs] — `Error::Connect` construction (19-22).
- [Source: crates/shim/tests/contract_shim.rs] — `run_shim_with_env` (70), connect-refused test (169-200), success test (112-142).
- Precedent stories (additive shim changes, carry-forward/preserve patterns): Story 5.3 (`bowerbird_ppid` injection), Story 5.7 (`shim_preserves_cwd_field_verbatim`). Neither changed the failure path; this story is the first to give the shim a stderr voice.

### Saved question (does not block implementation)

The proposal §6 leaves open whether a daemon-down outage spanning dozens of calls should ultimately be **exit 0 + stderr** (so Claude sees no hook error, just an informational stderr line) rather than the current **exit 1 + stderr**, once daemon-down is distinguishable from a genuine shim bug. This story keeps **exit 1** (NFR20 contract intact, smallest change). If you'd prefer the exit-0 reframing, that's a contract change and belongs in its own story with an NFR20 amendment — flagged here, deferred per §6.

## Dev Agent Record

### Agent Model Used

claude-opus-4-8 (1M context) — bmad-dev-story workflow, 2026-06-06.

### Debug Log References

- `cargo test -p bowerbird-shim` → 24 passed (6 unit in `error.rs`, 18 contract in `contract_shim.rs`).
- `cargo fmt --check` → clean (one initial wrap of the `writeln!` in `main.rs`, auto-formatted).
- `cargo clippy --all-targets --workspace -- -D warnings` → no issues.
- `cargo test -p bowerbird-shim shim_source_has_no_async` → passed (no Tokio/async leaked in).
- `cargo build --profile release-shim -p bowerbird-shim` → finished clean.
- Full workspace `cargo test --workspace` → 184 daemon-contract passed; the **single** failure is the pre-existing, documented, waived `story_2_4_dropped::lag_invalidates_snapshot_coverage_resubscribe_resnapshots` daemon wall-clock flake (`docs/research/test-isolation-bowerbird-findings.md §Symptom B`), which runs entirely against daemon code this story does **not** touch (`git status` shows only `crates/shim/*` + `docs/*`). Independent of Story 5.10 — same waiver as all 7 Story 5.9 passes.

**Code-review pass 1 resolution (2026-06-06):**
- `cargo test -p bowerbird-shim` → 30 passed (9 unit in `error.rs`/`main.rs` + 21 contract in `contract_shim.rs`); up from 24 (added 3 `one_line_path` unit tests + 3 new contract tests).
- `cargo fmt --check` → clean; `cargo clippy --all-targets -p bowerbird-shim -- -D warnings` → no issues.
- `cargo build --profile release-shim -p bowerbird-shim` → finished clean.
- `ruby -ryaml -rdate` `Psych.parse_file(sprint-status.yaml)` → no syntax error; `YAML.load_file(..., permitted_classes:[Date])` loads cleanly (the prior `mapping values are not allowed in this context at line 111 column 138` is resolved).

**Code-review pass 2+3 resolution (2026-06-08):**
- `cargo test -p bowerbird-shim` → 32 passed (11 unit in `error.rs`/`main.rs` + 21 contract in `contract_shim.rs`); up from 30 (added 2 `one_line_path` unit tests: `_escapes_unicode_separators_and_bidi`, `_renders_invalid_utf8_bytes`).
- `cargo fmt -p bowerbird-shim --check` → clean; `cargo clippy --all-targets -p bowerbird-shim -- -D warnings` → no issues (fixed a `clippy::doc_lazy_continuation` on the rewritten `one_line_path` doc comment).
- `cargo build --profile release-shim -p bowerbird-shim` → finished clean.
- Scope check: `git status` shows only `crates/shim/{src/main.rs,tests/contract_shim.rs}` + `docs/*` (no `crates/protocol` → no `docs/protocol-changelog.md` entry).

### Completion Notes List

- Added `Error::stderr_hint(&self) -> Option<&'static str>` as the third method in the `exit_code` → `level` → `stderr_hint` family, with arms in the same order as `exit_code()` for side-by-side review. Every exit-1 (ERROR) variant returns `Some(<cause>)`; every exit-0 (WARN) variant returns `None` by contract (NFR20). All cause strings are `&'static str` (zero allocation).
- `main`'s `Err(e)` arm emits a stderr line when `stderr_hint()` is `Some`, after the existing `log::append`: `bowerbird: {hint} (see {log_path})` **only when the append succeeded**, otherwise the pointer-less `bowerbird: {hint}` (pass-1 F2 — Claude is never sent to a file that was not written). The pre-`run` `resolve_log_path()` failure branch emits the pointer-less `bowerbird: {hint}` for the returned `Error` (`NoHome` → `HOME not set, cannot record event`) — no log path resolved, and routed through `Error::stderr_hint()`/`exit_code()` rather than a hardcoded string so the cause is single-sourced and canary-guarded (pass-3). Both writes use `let _ = writeln!(...)` so a failed stderr write is swallowed exactly like the log append (AC5); never panics, never changes the exit code.
- The exit-code contract is **unchanged**: `Connect` stays exit 1, the daemon-answered class stays exit 0, exit 2 stays forbidden. `run()`'s success path and `socket.rs` are untouched, so the hot path and `shim/benches/hot_path.rs` are unaffected (AC3). On a 200 the shim still writes nothing and creates no log file.
- New unit tests: `stderr_hint_matches_exit_code` (canary that the hint partition tracks the exit-code partition for every variant) and `connect_hint_names_the_daemon_down_cause` (pins the dogfood wording).
- New/extended contract tests: `shim_exit_nonzero_on_connection_refused` now asserts the named stderr line (bowerbird-prefixed, names "daemon not running", points at the resolved log path) alongside the unchanged file-log + exit-code assertions; `shim_exit_0_on_503_with_warning_log` and `shim_exit_0_on_400_from_daemon` now assert stderr stays **empty** (NFR20 regression guard); `shim_exit_0_on_200` keeps its pre-existing empty-stderr assertion.
- Deferred-work entry added for cross-invocation coalescing/rate-limiting and the exit-0-vs-exit-1 reconsideration (proposal §6) — the stateless shim has no shared state for cross-call rate-limiting.
- No `crates/protocol/src` change → no `docs/protocol-changelog.md` entry, changelog gate stays green.

**Code-review pass 1 resolution (2026-06-06):** All five patch findings resolved (see Review Findings for per-item resolution notes). Two behavior changes in `main`: (1) the `(see <log>)` pointer is omitted when the log append fails, so Claude is never sent to a file that was not written; (2) the log path is sanitized through a new `one_line_path()` helper that escapes control characters, so a newline/control byte in `BOWERBIRD_SHIM_LOG` can never break the one-line hook message. The exit-code contract and the exit-1/exit-0 stderr partition are unchanged. Test coverage grew from 24 → 30 (3 unit + 3 contract). `sprint-status.yaml`'s active `last_updated` is now a quoted scalar (valid YAML).

**Code-review pass 2+3 resolution (2026-06-08):** All pass-2 (3) and pass-3 (4) patch findings resolved (pass-3 reconfirms and supersedes pass-2; see Review Findings for per-item notes). Two code changes in `main.rs`, both behavior-preserving for legitimate paths: (1) `one_line_path()` is now a path-byte-aware sanitizer — it walks the raw unix path bytes (`OsStrExt::as_bytes()` + `utf8_chunks()`), escapes `is_control()` chars **plus** the Unicode line/paragraph separators (U+2028/U+2029) and bidi controls (LRM/RLM/ALM, U+202A–U+202E, U+2066–U+2069) via the new `needs_escape` predicate, and renders non-UTF-8 bytes as `\xNN` instead of a lossy U+FFFD, so the stderr pointer reflects the real byte path and can never split the one-line hook message; (2) the pre-run `resolve_log_path()` failure arm now routes through `Error::stderr_hint()` / `exit_code()` instead of a hardcoded string, single-sourcing the cause with `Error::NoHome` and bringing it under the `stderr_hint_matches_exit_code` canary (byte-identical behavior). Docs synced: AC1/AC2 + Completion Notes describe the conditional `(see <log>)` pointer; PRD FR5 + Journey 4 + traceability row, architecture FR1–FR5 summary + shim hot-path rules, and project-context Observability + hot-path discipline now state the scoped exit-1 stderr exception. Test coverage grew 30 → 32 (2 new `one_line_path` unit tests); the newline contract test now asserts the exact sanitized pointer. The exit-code contract and the exit-1/exit-0 stderr partition are unchanged.

### File List

- `crates/shim/src/error.rs` (modified) — added `stderr_hint()` + 2 unit tests.
- `crates/shim/src/main.rs` (modified) — `use std::io::{self, Read, Write}`; stderr line in `Err` arm + `resolve_log_path()` failure branch. Pass-1: capture `log::append` result and drop the `(see <log>)` pointer when it fails; `one_line_path()` helper sanitizes the log path; `#[cfg(test)] mod tests` with 3 `one_line_path` unit tests. Pass-3: `one_line_path()` rewritten as a byte-aware sanitizer (`OsStrExt::as_bytes()` + `utf8_chunks()`) with the new `needs_escape()` predicate (Unicode separators + bidi controls) and `\xNN` rendering for non-UTF-8 bytes; pre-run failure arm routed through `Error::stderr_hint()`/`exit_code()`; 2 new `one_line_path` unit tests.
- `crates/shim/tests/contract_shim.rs` (modified) — stderr assertions on connect-refused (named line) + 503/400 (empty, NFR20 guard). Pass-1: connect-refused assertion tightened to exact-match; added `shim_omits_log_pointer_when_log_append_fails`, `shim_names_cause_when_no_home_and_no_log_path`, `shim_stderr_stays_one_line_with_newline_in_log_path`. Pass-3: `shim_stderr_stays_one_line_with_newline_in_log_path` tightened to assert the exact sanitized pointer.
- `docs/bmad/implementation-artifacts/deferred-work.md` (modified) — Story 5.10 deferred-work section.
- `docs/bmad/implementation-artifacts/sprint-status.yaml` (modified) — status bookkeeping.
- `docs/bmad/implementation-artifacts/5-10-shim-names-daemon-down-cause.md` (modified) — this story file.
- `docs/bmad/planning-artifacts/prd.md` (modified, pass-3) — FR5, Journey 4, and the requirements-traceability row synced to the exit-1 stderr exception.
- `docs/bmad/planning-artifacts/architecture.md` (modified, pass-3) — FR1–FR5 summary and shim hot-path rules synced to the exit-1 stderr exception.
- `docs/bmad/project-context.md` (modified, pass-3) — Observability shim-logging exception and shim hot-path discipline rule synced to the exit-1 stderr exception.

## Change Log

- 2026-06-06: Shim failure path gains a stderr voice. New `Error::stderr_hint()` partitions causes to the exit-1 (ERROR) class; `main` emits one `bowerbird: <cause> (see <log>)` line on the exit-1 path (and a pointer-less line on pre-run log-path-resolution failure). Exit-0 (WARN, daemon-answered) class stays stderr-silent per NFR20. Exit-code contract unchanged; success path and hot path untouched. 4 new/extended tests (2 unit + 2 contract). Deferred-work entry recorded for §6 follow-ups. (Story 5.10)
- 2026-06-06: Code review pass 1 (`bmad-code-review`) documented five unresolved patch findings in the Review Findings section: invalid active `sprint-status.yaml` due unquoted `bowerbird: <cause>` text, misleading `(see <log>)` when log append fails, missing AC4 pre-run no-HOME contract coverage, too-loose connect-refused stderr assertion, and unescaped env-provided log path breaking the one-line stderr contract. Status: review → in-progress.
- 2026-06-06: Resolved all five code-review pass-1 findings. `sprint-status.yaml` active `last_updated` is now single-quoted (valid YAML again). `main` only emits the `(see <log>)` pointer when the log append succeeded, and routes the log path through a new `one_line_path()` helper that escapes control characters so the stderr hint can never become multi-line. Added 3 unit tests (`one_line_path*`) and 3 contract tests (`shim_omits_log_pointer_when_log_append_fails`, `shim_names_cause_when_no_home_and_no_log_path`, `shim_stderr_stays_one_line_with_newline_in_log_path`); tightened `shim_exit_nonzero_on_connection_refused` to exact-match. `cargo test -p bowerbird-shim` 30 passed (9 unit + 21 contract); fmt + clippy + release-shim build green. Status: in-progress → review.
- 2026-06-08: Code review pass 3 (`bmad-code-review`) documented four unresolved patch findings: `one_line_path` still misses Unicode separators/bidi controls and loses non-UTF-8 Unix path bytes; the pre-run NoHome branch hardcodes the stderr cause instead of routing through `Error::stderr_hint()` / `exit_code()`; AC1/AC2 plus completion notes still describe an unconditional log pointer; persistent PRD/architecture/project-context docs still state the old file-only/no-stderr failure contract. `cargo test -p bowerbird-shim` 30 passed; `sprint-status.yaml` loads with Ruby YAML. Status: review → in-progress.
- 2026-06-08: Resolved all pass-2 (3) and pass-3 (4) code-review findings. `one_line_path()` is now a byte-aware sanitizer that escapes Unicode line/paragraph separators (U+2028/U+2029) and bidi controls and renders non-UTF-8 path bytes as `\xNN` (was `is_control()`-only over `to_string_lossy`), so the one-line stderr pointer is faithful for every valid unix path. The pre-run no-HOME branch now routes through `Error::stderr_hint()`/`exit_code()` (byte-identical, single-sourced, canary-guarded). AC1/AC2, Completion Notes, and the persistent docs (PRD FR5 + Journey 4 + traceability row, architecture FR1–FR5 summary + shim hot-path rules, project-context Observability + hot-path discipline) were synced to the conditional-pointer / scoped exit-1 stderr contract. Added 2 unit tests (`one_line_path_escapes_unicode_separators_and_bidi`, `one_line_path_renders_invalid_utf8_bytes`) and tightened `shim_stderr_stays_one_line_with_newline_in_log_path` to an exact sanitized-pointer assertion. `cargo test -p bowerbird-shim` 32 passed (11 unit + 21 contract); fmt + clippy + release-shim build green. Exit-code contract and exit-1/exit-0 stderr partition unchanged. Status: in-progress → review.
