# Story 1.5: Shim Binary with Hot-Path Event Delivery

Status: ready-for-dev

## Story

As a Claude Code user,
I want the bowerbird shim to capture and forward hook events to the daemon in under 5ms at p99,
So that bowerbird is invisible during normal coding sessions and never causes Claude Code to feel slow.

## Acceptance Criteria

1. **Given** the shim compiled with the `release-shim` profile (`panic=abort`, `lto=fat`, `codegen-units=1`, `opt-level=z`, `strip=true`) **When** Criterion runs `crates/shim/benches/hot_path.rs` against a warm-cache daemon connection on both `macos-latest` and `ubuntu-latest` CI runners **Then** p99 latency is ≤5ms per platform (baselines committed as files, per-platform — not averaged); a p99 regression >15% from the committed baseline fails CI.

2. **Given** a successful shim event delivery (daemon responds `200\n`) **When** the shim exits **Then** it exits with code 0 and has written nothing to stdout or stderr.

3. **Given** the daemon is unreachable (`ECONNREFUSED` or socket path does not exist) **When** the shim attempts delivery **Then** it appends one timestamped error line to `~/.bowerbird/shim.log` and exits non-zero.

4. **Given** a `503\n` backpressure response from the daemon **When** the shim receives it **Then** it appends one timestamped warning line to `~/.bowerbird/shim.log` and exits 0 (fire-and-forget per NFR5).

5. **Given** `~/.bowerbird/shim.log` is created for the first time **When** its file mode is inspected **Then** it is `0600`, regardless of the calling process's umask (NFR15).

6. **Given** the shim source code in `crates/shim/src/**` **When** searched for the literal substrings `tokio`, `async fn`, or `.await` **Then** none are found — the shim contains no async runtime, only synchronous I/O.

## Tasks / Subtasks

- [ ] **Task 1: Add shim dependencies and bench scaffolding** (AC: #1, #6)
  - [ ] Add `serde_json = { workspace = true }` to `crates/shim/Cargo.toml` `[dependencies]`. **Note:** `serde_json` is already pinned at `1.0.149` in the workspace `[workspace.dependencies]` table (root `Cargo.toml`); just reference it with `workspace = true`, do NOT redefine the version.
  - [ ] Add `[dev-dependencies]` to `crates/shim/Cargo.toml`: `criterion = { workspace = true }`, `tempfile = { workspace = true }`, `assert_cmd = { workspace = true }`, `serde_json = { workspace = true }`. `tempfile` and `assert_cmd` are already in workspace deps (Story 1.2 added them); only `criterion` is new.
  - [ ] Add `criterion = "0.5"` to `[workspace.dependencies]` in root `Cargo.toml` (used only as dev-dep by shim; keeps version pinned alongside others)
  - [ ] Add `[[bench]] name = "hot_path" harness = false` to `crates/shim/Cargo.toml` so Criterion owns the bench main
  - [ ] Create empty `crates/shim/benches/hot_path.rs` placeholder so `cargo build` resolves the bench target — fill in Task 6
  - [ ] Do NOT add `tokio`, `axum`, `reqwest`, `ureq`, `hyper`, `tracing`, `log`, or any async/HTTP framework. The shim ships with stdlib + serde_json + thiserror only.

- [ ] **Task 2: Implement `crates/shim/src/error.rs`** (AC: #2, #3, #4)
  - [ ] Define `pub enum Error` with `thiserror::Error` variants covering: `Stdin(std::io::Error)`, `StdinEmpty`, `StdinNotJsonObject`, `StdinJson(serde_json::Error)`, `Connect(std::io::Error)`, `SocketIo(std::io::Error)`, `LogIo(std::io::Error)`, `BadResponse(String)`, `Backpressure(String)`, `Backpressure503`, `DaemonError400(String)`
  - [ ] Define `pub type Result<T> = std::result::Result<T, Error>;`
  - [ ] Add `pub fn exit_code(&self) -> i32` method on `Error`: returns `1` for `Connect`/`Stdin`/`StdinEmpty`/`StdinNotJsonObject`/`StdinJson`/`LogIo` (daemon-unreachable and bad-input class), returns `0` for `Backpressure503`/`SocketIo`/`BadResponse`/`DaemonError400` (mid-write / daemon-responding-with-error class — fire-and-forget per NFR20)
  - [ ] Add `pub fn level(&self) -> &'static str` method returning `"ERROR"` for exit-1 variants and `"WARN"` for exit-0 variants — used by `main.rs` to choose the log line level
  - [ ] **CRITICAL:** Never return `2` from any branch. Exit code 2 blocks Claude tool calls (architecture.md:615-616). Add a `#[cfg(test)]` unit test `exit_code_never_2` that iterates over a sample of each variant and asserts `e.exit_code() != 2` — belt-and-suspenders against future variants being added without thought.
  - [ ] No `#[derive(Debug)]` blocker — keep crate compileable under workspace lints. Do NOT add `#![deny(unsafe_code)]` (already enforced by `[workspace.lints.rust] unsafe_code = "forbid"`; duplicate attribute triggers `clippy::duplicated_attributes`).

- [ ] **Task 3: Implement `crates/shim/src/socket.rs` — sync UDS write path** (AC: #1, #2, #3, #4)
  - [ ] `use std::os::unix::net::UnixStream;` + `std::io::{Read, Write, BufRead, BufReader};` + `std::time::Duration;` + `std::path::Path;`
  - [ ] `pub(crate) fn send(sock_path: &Path, wire_bytes: &[u8]) -> crate::Result<Response>`:
    - [ ] `let mut stream = UnixStream::connect(sock_path).map_err(map_connect_err)?;` where `map_connect_err` distinguishes `ErrorKind::NotFound` and `ErrorKind::ConnectionRefused` → `Error::Connect` (both → exit 1) from any other I/O error → also `Error::Connect`
    - [ ] `stream.set_write_timeout(Some(Duration::from_millis(2)))?;` and `stream.set_read_timeout(Some(Duration::from_millis(3)))?;` — keeps total budget under 5ms. Map errors to `Error::SocketIo`
    - [ ] `stream.write_all(wire_bytes).map_err(Error::SocketIo)?;` — `wire_bytes` already includes the trailing `\n`
    - [ ] **No** `stream.flush()` — `write_all` on `UnixStream` is unbuffered; flush is a no-op that adds a syscall
    - [ ] `let mut reader = BufReader::with_capacity(64, stream);` (small fixed buffer — response is ≤512 bytes)
    - [ ] `let mut line = String::with_capacity(64);` then `reader.read_line(&mut line).map_err(Error::SocketIo)?;`
    - [ ] **Trim the trailing newline before matching:** `let trimmed = line.trim_end_matches('\n').trim_end_matches('\r');` — `read_line` retains the `\n`. Match on `trimmed`, not `line`.
    - [ ] Return one of: `Response::Ok` for `trimmed == "200"`; `Response::Backpressure` for `trimmed == "503"`; `Response::DaemonError(reason)` when `trimmed.starts_with("400 ")` (capture `trimmed["400 ".len()..]` as the reason); `Err(Error::BadResponse(line))` for anything else (including empty line / EOF / unexpected status)
  - [ ] `pub(crate) enum Response { Ok, Backpressure, DaemonError(String) }`
  - [ ] **No `unwrap()` / `expect()` anywhere outside `#[cfg(test)]`** — every Result is mapped to a typed `Error`

- [ ] **Task 4: Implement `crates/shim/src/log.rs` — failure log with mode 0600** (AC: #5)
  - [ ] `use std::fs::{OpenOptions, set_permissions, Permissions};` + `use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};` + `use std::path::Path;` + `use std::io::Write;` + `use std::time::{SystemTime, UNIX_EPOCH};`
  - [ ] `pub(crate) fn append(log_path: &Path, level: &str, message: &str) -> crate::Result<()>`:
    - [ ] Resolve `log_path.parent()` and `std::fs::create_dir_all(parent)` if it does not exist; map errors to `Error::LogIo`. The parent dir mode is intentionally not forced — we only constrain the log file itself.
    - [ ] Open with `OpenOptions::new().create(true).append(true).mode(0o600).open(log_path)` — `OpenOptionsExt::mode(0o600)` is the **target** mode passed to `open(2)`, but the kernel applies `mode & !umask`. To force 0o600 regardless of umask, **call `set_permissions(log_path, Permissions::from_mode(0o600))` immediately after the file is created/opened**. This is the same "chmod-after-create" pattern that Story 1.3 used for the ingest socket (architecture.md:684-689; `crates/daemon/src/ingest/listener.rs:set_permissions` call).
    - [ ] Format the line as `<ISO8601 UTC ms-precision> <LEVEL> <message>\n` per NFR16. **Exact format:** `YYYY-MM-DDTHH:MM:SS.sssZ` (millisecond precision, UTC, e.g. `2026-05-17T14:30:45.123Z`). Use a tiny inline ISO8601 formatter (no `chrono` dep) — compute seconds-since-epoch + millis from `SystemTime::now()` via Howard Hinnant's `civil_from_days` algorithm. OR use `time = "0.3"` if added to workspace. **Preferred:** keep the dep surface minimal and inline a UTC formatter (~30 lines). Either path is acceptable; see Dev Notes "ISO8601 without chrono".
    - [ ] `file.write_all(line.as_bytes()).map_err(Error::LogIo)?;` — the write itself can `Err` (ENOSPC); per AC #3 we still need to *try* to log, but if logging fails on a missing-daemon path the shim still exits 1 — both error sources collapse to exit 1, which is the right behavior.
  - [ ] **NEVER** call `println!`, `eprintln!`, `tracing::*`, `log::*`, or write to fd 1/2 anywhere in this module or any sibling module (architecture.md:626-627).

- [ ] **Task 5: Implement `crates/shim/src/main.rs`** (AC: #2, #3, #4, #6)
  - [ ] Top of file: `mod error; mod log; mod socket;`
  - [ ] `use error::{Error, Result};`
  - [ ] Parse a single CLI arg `--hook-kind <KIND>` where `<KIND>` is one of `PreToolUse`, `PostToolUse`, `Stop`, `Notification`. **Use stdlib `std::env::args()` — DO NOT add `clap`** (heavyweight; would push p99 over budget). A 20-line manual loop is fine; on missing/invalid arg, log error and exit 1.
  - [ ] **Resolve `$HOME` early and fail loudly if absent:** read `std::env::var_os("HOME")` into a `Option<OsString>`. If `None` or empty AND neither `BOWERBIRD_INGEST_SOCK` nor `BOWERBIRD_SHIM_LOG` is set, exit 1 immediately — there is nowhere to log to. If env vars override both paths, missing HOME is fine. The shim's contract requires log_path resolvable BEFORE attempting connect, since connect-failures need to log.
  - [ ] `let sock_path: PathBuf = std::env::var_os("BOWERBIRD_INGEST_SOCK").map(PathBuf::from).unwrap_or_else(|| Path::new(&home).join(".bowerbird/ingest.sock"));` — env override exists for tests; production uses `$HOME/.bowerbird/ingest.sock` per architecture.md:434.
  - [ ] `let log_path: PathBuf = std::env::var_os("BOWERBIRD_SHIM_LOG").map(PathBuf::from).unwrap_or_else(|| Path::new(&home).join(".bowerbird/shim.log"));` — also overridable via `BOWERBIRD_SHIM_LOG` for tests.
  - [ ] Read stdin to `Vec<u8>` with a 1 MiB cap (`std::io::Read::take(1 << 20)` against stdin lock). Empty stdin → `Error::StdinEmpty` → exit 1, log "stdin empty".
  - [ ] Parse as `serde_json::Value`; if not an Object → `Error::StdinNotJsonObject`.
  - [ ] **Inject `hook_kind`**: as `Value::Object`, call `.insert("hook_kind".to_string(), Value::String(hook_kind.to_string()))`. `serde_json::Map::insert` overwrites any existing entry — the CLI arg wins. Claude Code's payload may use `hook_event_name` for the same concept — leave that field intact, do NOT remove. Concrete example:

    ```text
    Stdin:   {"session_id":"s1","tool_name":"Bash","hook_event_name":"PreToolUse"}
    CLI:     --hook-kind PreToolUse
    Output:  {"session_id":"s1","tool_name":"Bash","hook_event_name":"PreToolUse","hook_kind":"PreToolUse"}
    ```

    See Dev Notes "Why the shim injects hook_kind" for the architectural reasoning.
  - [ ] Serialize back to bytes via `serde_json::to_vec(&value)`; append `b'\n'`. This `Vec<u8>` is the wire payload.
  - [ ] Call `socket::send(&sock_path, &wire)`:
    - `Ok(Response::Ok)` → return `Ok(())`
    - `Ok(Response::Backpressure)` → log warning + return `Err(Error::Backpressure503)` (exit_code=0)
    - `Ok(Response::DaemonError(reason))` → log warning + return `Err(Error::DaemonError400(reason))` (exit_code=0)
    - `Err(Error::Connect(_))` → log error + propagate (exit_code=1)
    - `Err(Error::SocketIo(_))` → log warning + propagate (exit_code=0 per NFR20: mid-write errors are fire-and-forget)
  - [ ] `fn main()` body: `match run() { Ok(()) => std::process::exit(0), Err(e) => { let _ = log::append(&log_path, e.level(), &e.to_string()); std::process::exit(e.exit_code()); } }` — note the swallowed log error: we already failed, and crashing the shim makes things worse than silently dropping the log line.
  - [ ] **NEVER** `unwrap()` or `expect()` outside `#[cfg(test)]`. **NEVER** `println!`/`eprintln!` on any path.
  - [ ] **NEVER** import or use `tokio` / `async` / `.await` (AC #6 gate will grep for these literal strings).

- [ ] **Task 6: Implement Criterion benchmark `crates/shim/benches/hot_path.rs`** (AC: #1)
  - [ ] `use criterion::{criterion_group, criterion_main, Criterion, black_box};`
  - [ ] Fixture: a tiny stdlib-only mock UDS server that accepts one connection, reads one `\n`-terminated JSON line, writes back `b"200\n"`, closes the connection — looped in a background `std::thread::spawn`. **Do NOT use tokio in the bench** — keep the bench's dep tree shallow so the measurement isn't dominated by runtime startup. The bench is allowed to use `tokio` if needed (it's a dev-dep boundary, AC #6 only restricts `crates/shim/src/`), but it is much faster and clearer with raw stdlib.
  - [ ] Pre-warm: open the socket, accept a noop connection to ensure inode is in the dentry cache. Then run `criterion::bench_function("uds_post_ingest", |b| b.iter(|| { … })`.
  - [ ] Inside the loop: spawn the shim binary via `assert_cmd::Command::cargo_bin("bowerbird-shim")` with `--hook-kind PreToolUse`, env `BOWERBIRD_INGEST_SOCK=<temp sock>`, `BOWERBIRD_SHIM_LOG=<temp log>`, stdin = a small fixture JSON. Measure end-to-end wall time per invocation.
  - [ ] **Per-platform baselines — seeding procedure:**
    1. Locally on the dev's machine, after Tasks 1–5 are implemented, run `cargo bench -p bowerbird-shim --profile release-shim -- --save-baseline initial uds_post_ingest`. Criterion writes `target/criterion/uds_post_ingest/initial/estimates.json`.
    2. Copy that file to `crates/shim/benches/baselines/<host-platform>.json` (`macos.json` if on macOS, `linux.json` if on Linux) and commit it. This seeds the dev's local platform.
    3. CI's first run on the *other* platform will fail the bench-gate (no baseline to compare against). At that point, copy the artifact CI produces (`target/criterion/uds_post_ingest/new/estimates.json`) from the workflow's artifact uploads into the missing baseline file and commit. The bench-gate step (Task 9) must upload `target/criterion/**` as an artifact so this is possible.
    4. Once both `macos.json` and `linux.json` exist, all subsequent CI runs use `--load-baseline` against them and gate at +15% regression.
    The criterion JSON shape is documented at <https://bheisler.github.io/criterion.rs/book/user_guide/cli_output.html>. CI selects the right baseline by `cfg!(target_os = "macos")` vs `cfg!(target_os = "linux")` in the bench file.
  - [ ] `criterion_group!(benches, …); criterion_main!(benches);`

- [ ] **Task 7: Contract tests `crates/shim/tests/contract_shim.rs`** (AC: #2, #3, #4, #5, #6)
  - [ ] Helper `fn start_mock_ingest(tmp: &TempDir, response: &'static [u8]) -> PathBuf` — stdlib UDS listener thread that returns `response` to whatever it reads. Returns the bound socket path.
  - [ ] Helper `fn run_shim(sock: &Path, log: &Path, hook_kind: &str, stdin: &[u8]) -> assert_cmd::assert::Assert` — wraps `assert_cmd::Command::cargo_bin("bowerbird-shim")`.
  - [ ] **`shim_exit_0_on_200`** (AC#2): start mock returning `b"200\n"`, run shim → assert exit 0, stdout empty, stderr empty.
  - [ ] **`shim_silent_on_success`** (AC#2): same as above, plus assert the log file does not exist (the shim must not create the log on the success path).
  - [ ] **`shim_exit_nonzero_on_connection_refused`** (AC#3): socket path points to a non-existent file → assert exit code ≠ 0 AND ≠ 2, log file contains one timestamped ERROR line referencing the socket path.
  - [ ] **`shim_exit_0_on_503_with_warning_log`** (AC#4): mock returns `b"503\n"` → assert exit 0, log file contains exactly one timestamped WARN line.
  - [ ] **`shim_log_mode_is_0600_with_permissive_umask`** (AC#5): **preferred pattern — shell out via `assert_cmd` to avoid adding a new dep:** spawn `sh -c 'umask 0022 && exec "$0" --hook-kind PreToolUse' <shim-path>` so the child inherits umask `0o022`. Trigger a connection-refused path (no mock listener), assert `std::fs::metadata(log).permissions().mode() & 0o777 == 0o600`. The `umask=0o022` case is the smoking gun: if the shim relied on `OpenOptionsExt::mode()` alone, the resulting file mode would still be `0o600` (since 0o022 doesn't strip owner bits) — but if a future regression set `mode(0o644)` instead, the test would catch it as `0o644` instead of the required `0o600`. Add a second variant `umask=0o077` (strips group+other entirely) to verify the chmod-after-create doesn't accidentally widen permissions when umask is restrictive — should still be exactly `0o600`.
  - [ ] **`shim_source_has_no_async`** (AC#6): walk `crates/shim/src/**/*.rs` and assert none contain `tokio` (case-sensitive), `async fn`, or `.await`. Read each file via `std::fs::read_to_string` and `assert!(!s.contains("tokio"))` etc. This test must NOT scan `tests/`, `benches/`, or `Cargo.toml`.
  - [ ] **`shim_respects_env_var_sock_path`**: set `BOWERBIRD_INGEST_SOCK` to a custom temp path (NOT under `$HOME/.bowerbird`), start the mock listener on that exact path, run the shim, assert it connects there. This proves the env override actually takes effect (instead of the test passing only because both paths happen to resolve the same way).
  - [ ] **`shim_respects_env_var_log_path`**: set `BOWERBIRD_SHIM_LOG` to a custom temp path, trigger a connection-refused path, assert the log line lands at the env-specified path and NOT at `$HOME/.bowerbird/shim.log`.
  - [ ] **`shim_wire_payload_is_valid_ndjson`**: start a mock that captures all bytes received, run the shim with a known stdin, assert the captured bytes split on `\n` produce exactly one non-empty line that parses as a JSON object, AND the JSON object equals the expected merge of (stdin JSON) ∪ (`"hook_kind": <flag-value>`).
  - [ ] **`shim_injects_hook_kind`**: start a mock that captures the request line, run shim with `--hook-kind PreToolUse` and stdin `{"session_id":"s1","tool_name":"Bash"}`, assert the captured request contains `"hook_kind":"PreToolUse"` and the original `session_id` and `tool_name` survive verbatim.
  - [ ] **`shim_preserves_existing_hook_event_name_field`**: stdin already has `"hook_event_name":"PreToolUse"`; assert the captured request still contains `hook_event_name` (verbatim preservation) AND `hook_kind` (shim's injection). Both coexist.
  - [ ] **`shim_exit_0_on_400_from_daemon`**: mock returns `b"400 invalid JSON: …\n"` → exit 0, log file contains one WARN line. (Fire-and-forget; daemon's complaint is logged but does not block Claude.)

- [ ] **Task 8: End-to-end test against the real daemon ingest path** (AC: #2, #3, #4)
  - [ ] Add `crates/shim/tests/e2e_against_daemon.rs` using `daemon = { path = "../daemon" }` as a dev-dep, OR — preferred — extend the existing `start_ingest_listener` helper in `crates/daemon/tests/contract_daemon.rs` with **one new test** `shim_binary_round_trip_to_daemon_ingest` that:
    - [ ] Starts the daemon's real `ingest::listener::run_bound` against a temp socket
    - [ ] Invokes the shim binary via `assert_cmd::Command::cargo_bin("bowerbird-shim")` with `--hook-kind PreToolUse`, stdin = a fixture matching `crates/adapter-claude/tests/fixtures/pre_tool_use_bash.json`
    - [ ] Asserts: shim exits 0, the receiver channel yields one `EventEnvelope` with `kind == EventKind::PreToolUse`, `source == "claude"`, `session_id == "test-session-abc123"`
  - [ ] **Choose ONE location** (preferring `crates/daemon/tests/contract_daemon.rs` so we don't introduce a daemon dev-dep on shim). Document the choice in Dev Notes.

- [ ] **Task 9: CI wiring for per-platform bench gate** (AC: #1)
  - [ ] Update `.github/workflows/ci.yml`: add a step on both `macos-latest` and `ubuntu-latest` runners that runs `cargo bench --profile release-shim -p bowerbird-shim -- --save-baseline ci-current` and then `cargo bench --profile release-shim -p bowerbird-shim -- --load-baseline ci-current --baseline platform-committed --noplot` and fails if Criterion reports a regression >15% on the `uds_post_ingest` benchmark.
  - [ ] Commit baseline files under `crates/shim/benches/baselines/macos.json` and `…/linux.json` after the first green run on each platform — the dev should run the bench locally to seed the baselines, then update them after CI's first run to match the hosted-runner numbers (these are the authoritative baselines).
  - [ ] Document the baseline-refresh procedure in `crates/shim/benches/README.md` (short, ~20 lines): when to refresh (architectural changes that legitimately move the p99), how to refresh (PR with new baseline JSON + justification in commit body), and the >15% threshold rationale.
  - [ ] **Acknowledge:** if the first green CI run shows p99 > 5ms on either platform, do NOT silently raise the threshold. Per PRD line 181, the right response is an ADR documenting the real number. File `docs/decisions/0002-shim-p99-budget.md` with the measured number, root-cause analysis, and either a tightened implementation or a justified budget revision.

- [ ] **Task 10: Final checks**
  - [ ] `cargo build --workspace` — green, zero warnings
  - [ ] `cargo build -p bowerbird-shim --profile release-shim` — green; measure with `ls -l target/release-shim/bowerbird-shim` and log the size in the PR description. **Sanity threshold:** if the stripped binary exceeds **3 MB**, investigate immediately via `cargo tree -p bowerbird-shim` for unexpected deps (most common offender: accidentally pulling in `tokio` via a misconfigured workspace dep with `default-features = true`). The expected size is a few hundred KB given the stdlib + serde_json + thiserror dep tree.
  - [ ] `cargo fmt --check` — green
  - [ ] `cargo clippy --all-targets --workspace -- -D warnings` — green
  - [ ] `cargo test --workspace` — all tests pass including new shim contract tests
  - [ ] `cargo bench -p bowerbird-shim` — produces a baseline locally; commit the result under `crates/shim/benches/baselines/<platform>.json` (or wait for CI to seed; either works for the first run)
  - [ ] Run `grep -rn 'tokio\|async fn\|\.await' crates/shim/src/` — must produce zero matches (this is what AC #6's contract test automates)

## Dev Notes

### Wire Protocol (DO NOT BE MISLED BY THE ARCHITECTURE TEXT)

**Authoritative reference: [ADR 0002](../../decisions/0002-ingest-wire-framing-and-hook-kind.md)** — formalizes both the NDJ wire framing and the `hook_kind` injection model described below. Read it first if any of the architecture/PRD text below seems to contradict the daemon's actual behavior.

**The architecture document and PRD both say "POST /ingest via HTTP/1.1 over the Unix domain socket"** (architecture.md:984-985 mark wire framing as "TBD at implementation time"; PRD line 365). **Story 1.3 resolved this as newline-delimited JSON, NOT HTTP**, and ADR 0002 ratifies the choice. The shipped daemon at `crates/daemon/src/ingest/handler.rs` reads a single `\n`-terminated line as a JSON object and writes `200\n`, `503\n`, or `400 <reason>\n` back.

**Wire contract (the source of truth — verified against `crates/daemon/src/ingest/handler.rs`):**
- **Request:** `<valid JSON object>\n` — ONE object, terminated by ONE LF
- **Response (success):** `200\n`
- **Response (backpressure):** `503\n`
- **Response (malformed/normalize-error):** `400 <reason>\n` where `<reason>` is single-line, ≤512 chars
- **Connection lifecycle:** one connection per event; daemon closes after writing the response

If you find yourself reaching for `hyper`, `reqwest`, `ureq`, or hand-rolling HTTP/1.1 headers — **stop**. That's the architecture doc misleading you. The actual protocol is one line in, one line out.

### Why the shim injects `hook_kind`

The daemon handler (`crates/daemon/src/ingest/handler.rs:63-66`) reads `hook_kind` from the top-level JSON object and passes it to `adapter_claude::normalize()`. Story 1.4's deferred work explicitly flags this: *"Revisit when shim guarantees `hook_kind` in every payload — at that point, missing `hook_kind` should be a 400, not a silent default"* (`deferred-work.md` line 37). **That moment is now.**

Claude Code's own hook stdin payload uses `hook_event_name` (per `docs/research/09-multi-agent-support.md:47`), not `hook_kind`. Three options were considered:

1. Shim parses stdin and adds a `hook_kind` field (chosen)
2. Daemon handler also looks at `hook_event_name` (would couple the daemon to Claude Code's schema; rejected — that's adapter-claude's job)
3. Shim renames `hook_event_name` → `hook_kind` (loses the original; tools building on the raw payload column would break)

**The shim adds `hook_kind` as a transport-routing label** without touching anything else — this is *not* the "normalization" Axiom 1 forbids (which is interpreting application-level concepts like reactions or session state). It's a single field added so the daemon can dispatch to the right adapter codepath. The original `hook_event_name` (if present) is preserved verbatim in the payload.

The hook_kind value comes from the `--hook-kind` CLI arg, which the future `bowerbird install` command (Story 3.1) will write into `~/.claude/settings.json` per hook event entry.

### Exit Code Matrix (load-bearing)

| Scenario | Exit Code | Log Behavior | Source |
|---|---|---|---|
| Daemon returned `200\n` | 0 | None (no log line) | AC #2 |
| Daemon returned `503\n` | 0 | One WARN line | AC #4 |
| Daemon returned `400 <reason>\n` | 0 | One WARN line | NFR20 (mid-stream daemon-side errors are fire-and-forget) |
| Connect failed (ECONNREFUSED / ENOENT) | 1 | One ERROR line | AC #3, NFR20 |
| Socket write error mid-stream | 0 | One WARN line | NFR20 ("exits 0 on mid-write errors") |
| Socket response was not a recognized status line | 0 | One WARN line | Defensive: malformed daemon response is fire-and-forget |
| Stdin empty / not a JSON object / not parseable | 1 | One ERROR line | Input validation failure |
| Missing or invalid `--hook-kind` arg | 1 | One ERROR line | Input validation failure |
| **Any path** | **NEVER 2** | — | architecture.md:615-616 (exit 2 blocks Claude tool calls — forbidden) |

**Why is 503 fire-and-forget but Connect-failure is exit 1?** 503 means the daemon is running and overloaded; the event is lost but recovery is automatic (next event will likely succeed). Connect-failure means the daemon is *down* — the user needs to know bowerbird is unavailable so they can restart it. The asymmetric exit code surfaces the second case to Claude Code as a hook failure (which the user sees), while keeping the first invisible.

### File Mode 0600 — The Umask Trap

`std::fs::OpenOptions::new().mode(0o600).create(true).open(path)` does **not** guarantee mode 0o600. The kernel computes the resulting mode as `requested_mode & !umask`. With a permissive umask like `0o022`, you get `0o600 & !0o022 = 0o600` (still 0o600 — `0o022` only strips group/other write bits, which 0o600 doesn't request anyway). **But with a typical default umask of `0o022`, the resulting file is 0o600 anyway.** The trap is umasks that strip owner-read or where the kernel implementation differs.

**The safe pattern, identical to Story 1.3's socket-permissions fix:**
1. Open with `OpenOptions::new().mode(0o600).create(true).append(true).open(path)`
2. Immediately call `std::fs::set_permissions(path, Permissions::from_mode(0o600))` — this calls `chmod(2)` which **ignores umask**

See `crates/daemon/src/ingest/listener.rs` for the reference implementation (Story 1.3 Task 3, chmod-after-bind pattern).

### Sync I/O Constraints

- **Stdin:** `std::io::stdin().lock().take(1 << 20).read_to_end(&mut buf)` — locked handle, 1 MiB cap
- **Socket:** `std::os::unix::net::UnixStream::connect()` + `write_all()` + `BufReader::read_line()`. **All synchronous.** No tokio. No async.
- **Log file:** `std::fs::OpenOptions` with `mode(0o600)`, `.append(true)`, `.create(true)`
- **No allocations on the success path:** Best-effort, not enforced at compile time. Serde inherently allocates during `to_vec`. The Criterion bench is the gate (architecture.md:622-624).
- **AC #6 grep gate** scans `crates/shim/src/**/*.rs` for the literal substrings `tokio`, `async fn`, `.await`. Even a doc comment containing these will fail. Use `runtime`, `synchronous fn`, or rephrase if you need to reference them.

### ISO8601 Without `chrono`

To avoid pulling in the `chrono` (~100 KB) or `time` (~60 KB) crate just for log timestamps:

```rust
use std::time::{SystemTime, UNIX_EPOCH};

fn iso8601_utc_now() -> String {
    let dur = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
    let total_secs = dur.as_secs();
    let millis = dur.subsec_millis();
    // Days-since-epoch / civil-from-days algorithm (Howard Hinnant)
    let days = (total_secs / 86_400) as i64;
    let secs_of_day = total_secs % 86_400;
    let (y, m, d) = civil_from_days(days);
    let hh = (secs_of_day / 3600) as u32;
    let mm = ((secs_of_day % 3600) / 60) as u32;
    let ss = (secs_of_day % 60) as u32;
    format!("{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z", y, m, d, hh, mm, ss, millis)
}
```

The `civil_from_days` helper is ~10 lines (Hinnant's `civil_from_days` algorithm). Total formatter is ~30 lines, zero deps, microsecond-fast. The single `unwrap_or_default()` is acceptable because `SystemTime::now()` returning before `UNIX_EPOCH` requires a system clock that's 56 years off — that's a clock bug, not an error case worth modeling. Even then, `unwrap_or_default()` gives us a duration of zero seconds, yielding `1970-01-01T00:00:00.000Z` — wrong but not a panic.

If 30 lines of date math is more than you want to write, the alternative is `time = "0.3"` added to workspace deps. Document the choice in your commit message.

### Critical Context from Stories 1.1–1.4 (DO NOT REPEAT MISTAKES)

**Dependency pins** — use the workspace dep table at `Cargo.toml`, not the architecture doc:

| Dep | Actually installed |
|---|---|
| serde | 1.0.228 |
| serde_json | 1.0.149 |
| thiserror | 2.0.18 |
| tempfile | 3.20.0 |
| assert_cmd | 2.0.17 |

**Workspace lints**: every crate has `[lints] workspace = true` and the workspace has `unsafe_code = "forbid"`. **Do NOT** add `#![deny(unsafe_code)]` or `#![forbid(unsafe_code)]` to any source file — it triggers `clippy::duplicated_attributes` as a hard error (Story 1.4 review finding).

**`anyhow` boundary**: permitted only in `main.rs` of binary crates. The shim's `main.rs` MAY use `anyhow::Context` if convenient — but everything else in the shim is `thiserror`-only.

**No `unwrap()` / `expect()` outside `#[cfg(test)]`**: hard rule, enforced by review. Every Result is mapped to a typed `Error`.

**No `println!` / `eprintln!`**: not just in the shim — anywhere in shipped code. The daemon uses `tracing::*`. The shim uses **nothing** on the success path and writes only to `~/.bowerbird/shim.log` on the failure path.

**Test fixture pattern**: Story 1.3's `start_ingest_listener` helper in `crates/daemon/tests/contract_daemon.rs` is the canonical mock-ingest pattern. The shim's contract tests should mirror that style: spawn a stdlib UDS listener thread, capture writes for assertion, return the bound socket path.

**Stub `hook_kind` default**: the daemon currently defaults missing `hook_kind` to `"PreToolUse"` (`crates/daemon/src/ingest/handler.rs:63-66`). This story does NOT need to change that default — but the dev should note for future work that once the shim ships and is the only ingest client, missing `hook_kind` should become a 400. That's a separate change, not Story 1.5's scope.

**Workspace dep additions:** `serde_json` is already in workspace deps. `criterion` is NOT — add it. `tempfile` and `assert_cmd` are already pinned in workspace deps (Story 1.2 added them).

### Anti-Patterns To Avoid

- **Reaching for an HTTP client** — the wire is NDJ, not HTTP. `reqwest`, `ureq`, `hyper`, hand-rolled HTTP/1.1 are all wrong. Use raw `UnixStream`.
- **Adding `tokio` to the shim** — instant AC #6 failure. `tokio` is for the daemon; the shim runs synchronously.
- **`stream.flush()` on `UnixStream`** — no-op that adds a syscall. `write_all` already wrote everything.
- **`clap`** — too heavy for a 5ms hot path. Stdlib `args()` is fine for one flag.
- **`chrono` / `time`** — only for timestamps in the log line. Inline a 30-line ISO8601 formatter, or accept the `time` crate cost (already lighter than `chrono`).
- **`tracing` / `log` macros** — banned in the shim source tree. Plain file I/O only.
- **Exit code 2** — blocks Claude tool calls. Forbidden architecture-wide.
- **Re-serializing the entire payload** when only injecting one field — actually unavoidable with `serde_json::Value`, so just do it. The "no allocation" rule is best-effort and gated by the bench, not a compile-time guarantee.
- **`#[forbid(unsafe_code)]` / `#[deny(unsafe_code)]`** in source files — workspace already does this; duplicate = `clippy::duplicated_attributes` hard error.
- **`unwrap()` / `expect()` outside `#[cfg(test)]`** — every Result is typed.
- **Cleaning up `~/.bowerbird/shim.log`** between runs / rotating it — V1 leaves it as an unbounded append-only file. Rotation is a post-V1 concern; do not write rotation code in this story.
- **Reading the daemon's HTTP response body or headers** — the response is one line. Read one line. Stop.

### Project Structure Notes

**Files to be created:**
```
crates/shim/src/
├── main.rs          # arg parse, stdin read, hook_kind inject, dispatch, exit
├── error.rs         # thiserror Error enum + Result + exit_code() method
├── socket.rs        # synchronous UnixStream connect/write/read with timeouts
└── log.rs           # append timestamped line to shim.log with mode 0600

crates/shim/benches/
├── hot_path.rs      # criterion benchmark for p99 ≤ 5ms gate
├── baselines/
│   ├── macos.json   # committed per-platform baseline (seeded on first green CI run)
│   └── linux.json
└── README.md        # baseline refresh procedure

crates/shim/tests/
└── contract_shim.rs # exit-code, silence-on-success, log-mode-0600, no-async ACs
```

**Files to be modified:**
```
Cargo.toml                              # add criterion to [workspace.dependencies]
crates/shim/Cargo.toml                  # add serde_json dep; dev-deps; [[bench]] target
crates/daemon/tests/contract_daemon.rs  # add shim_binary_round_trip_to_daemon_ingest e2e test (preferred location per Task 8)
.github/workflows/ci.yml                # add per-platform bench gate step (Task 9)
```

**Unchanged** (the shim does not modify these, per Story 1.4 review's "no scope creep" discipline):
- `crates/protocol/**` — already exposes everything the shim needs
- `crates/daemon/src/**` — wire protocol is already correct from Story 1.3; daemon handler's default-to-`PreToolUse` is fine for now
- `crates/adapter-claude/**` — adapter is daemon-side; shim doesn't import or use it

**Source tree alignment with architecture.md:771-778:** Architecture lists `crates/shim/src/{main.rs, error.rs, socket.rs}` — we add `log.rs` as a fourth file because the failure-log concern is cohesive enough to deserve isolation. This is a minor deviation; document in commit body.

### Git Intelligence (Recent Work Patterns)

Recent commits on `main` show a consistent pattern: feat commit + later fix commits applying review patches, then merge.

- `6ca00d1` (PR #13): Story 1.4 merged — adapter and normalization
- `ffe4a15`: review patches applied for 1.4
- `59b580a`: feat(story-1.4) — adapter-claude impl
- `3ebc590`: feat(story-1.3) — Unix socket ingest endpoint with NDJ wire protocol ← **THE WIRE PROTOCOL SOURCE OF TRUTH**
- `ae0ef96`: feat(story-1.2) — daemon foundation + SQLite

For this story, expect the dev to land:
1. A feat commit implementing the shim
2. A code-review run that surfaces patches (Story 1.1, 1.2, 1.3, 1.4 each had review rounds)
3. The first CI run on a real macOS/Linux runner — this seeds the baseline files

### References

- [Source: docs/decisions/0002-ingest-wire-framing-and-hook-kind.md] — **authoritative** wire-framing + `hook_kind` decisions that supersede contradictory PRD/architecture text
- [Source: docs/bmad/planning-artifacts/epics.md#Story-1.5] — original AC text
- [Source: docs/bmad/planning-artifacts/architecture.md#OQ#1-Shim-when-daemon-down] — fire-and-forget design (lines 123-138)
- [Source: docs/bmad/planning-artifacts/architecture.md#Process-Conventions] — exit code semantics (lines 610-627)
- [Source: docs/bmad/planning-artifacts/architecture.md#Project-Structure] — shim source tree (lines 771-778)
- [Source: docs/bmad/planning-artifacts/architecture.md#Compiler-and-Toolchain] — `release-shim` profile (already in `Cargo.toml`)
- [Source: docs/bmad/planning-artifacts/prd.md#API-Surface] — daemon `/ingest` semantics (line 365; note the protocol is NDJ not HTTP — see Wire Protocol section above)
- [Source: docs/bmad/planning-artifacts/prd.md#Risks] — shim performance budget rationale (line 181: "If the number can't be met cleanly, the right response is an ADR")
- [Source: docs/bmad/implementation-artifacts/1-3-unix-socket-ingest-endpoint.md] — established NDJ wire protocol, chmod-after-bind pattern, `start_ingest_listener` test helper
- [Source: docs/bmad/implementation-artifacts/1-4-claude-code-adapter-and-event-normalization.md] — adapter contract, `hook_kind` field expectation, validate_config at startup pattern
- [Source: docs/bmad/implementation-artifacts/deferred-work.md] — line 37: `hook_kind` should become required once shim ships
- [Source: docs/bmad/project-context.md#Axiom-1] — "The substrate observes; it does not interpret" — clarifies why hook_kind injection is a transport-routing concern, not interpretation
- [Source: crates/daemon/src/ingest/handler.rs:43-110] — current daemon handler behavior (the source of truth for the wire protocol)
- [Source: crates/daemon/src/ingest/listener.rs] — chmod-after-bind pattern (mirror for shim.log mode 0600)
- [Source: crates/adapter-claude/tests/fixtures/pre_tool_use_bash.json] — payload shape that the shim's stdin will resemble
- [Source: Cargo.toml] — `release-shim` profile (lines 34-40), workspace deps pinned

## Dev Agent Record

### Agent Model Used

_To be filled in by dev agent_

### Debug Log References

### Completion Notes List

### File List

### Review Findings

_Code review run: 2026-05-18 against branch `claude/bmad-dev-story-1.5-cwAPx` vs `main`. Layers: Blind Hunter, Edge Case Hunter, Acceptance Auditor (all completed). 4 decision-needed, 6 patch, 9 deferred, 24 dismissed as noise._

**Decision Needed**

- [ ] [Review][Decision] CI bench gate measures `mean`, not `p99` as AC #1 requires — `.github/workflows/ci.yml:67-71` reads `data["mean"]["point_estimate"]` from Criterion's `change/estimates.json` and labels the failure "p99 regressed". Criterion's default JSON exposes mean/median/std_dev, not p99. Either (a) accept gating on mean and amend AC #1 wording, (b) post-process raw sample CSV to compute p99, or (c) configure Criterion to emit percentile estimates. Needs your call before patching.
- [ ] [Review][Decision] Bench gate is unarmed: no baselines committed + soft-fail on missing baseline — `.github/workflows/ci.yml:76-85` exits 0 when no baseline file is present, and the diff commits no `linux.json` / `macos.json` despite the spec mandating per-platform baselines. AC #1 says "fails CI" on regression; today the gate cannot fail. Confirm whether the soft-fail-until-seeded approach (documented in Task 9 followups) is acceptable, or whether the first green CI artifacts should be committed now to arm the gate.
- [ ] [Review][Decision] Silent stdin truncation at 1 MiB — `crates/shim/src/main.rs:1188-1194` uses `take(MAX_STDIN_BYTES)` and silently discards bytes beyond 1 MiB. If truncation lands at a syntactically-valid JSON object boundary, the shim forwards an incomplete payload. Spec Task 5 said "1 MiB cap" but did not specify overflow behavior. Choose: silently truncate (status quo), explicit `StdinTooLarge` error → exit 1, or peek-and-reject when `read` would exceed cap.
- [ ] [Review][Decision] Baseline seeding copies only `estimates.json` — `.github/workflows/ci.yml:42-47` copies a single file into the criterion baseline dir, but Criterion's `--load-baseline` typically expects `sample.json`, `tukey.json`, etc. too. If the load silently fails, the gate prints "first run on this platform" and exits 0. Verify Criterion 0.5.x tolerates a directory containing only `estimates.json`; if not, the gate is broken from the first PR.

**Patch**

- [ ] [Review][Patch] CI `cargo bench | tee` masks failure (no `pipefail`) [`.github/workflows/ci.yml:52-58`] — without `set -o pipefail` or `${PIPESTATUS[0]}` check, a non-zero exit from `cargo bench` is swallowed by `tee`'s success and the gate silently passes.
- [ ] [Review][Patch] Symlink TOCTOU in `log::append` — use fd-based `file.set_permissions(...)` [`crates/shim/src/log.rs:978-988`] — `OpenOptions::open` + path-based `set_permissions(path, ...)` both follow symlinks. A local actor can swap `~/.bowerbird/shim.log` for a symlink to e.g. `~/.ssh/authorized_keys` between open and chmod. Call `file.set_permissions(Permissions::from_mode(0o600))` on the open `File` to chmod via the fd.
- [ ] [Review][Patch] AC #5 umask test doesn't exercise chmod-after-create [`crates/shim/tests/contract_shim.rs:1485-1531`] — `umask=0o022` and `0o077` both yield `0o600` via `OpenOptionsExt::mode(0o600)` alone (no chmod needed). To prove the chmod-after-create is doing the work, add a `umask=0o277` variant that strips owner-read — without `set_permissions`, the file would land at `0o400`.
- [ ] [Review][Patch] AC #3 test doesn't assert socket path in ERROR log line [`crates/shim/tests/contract_shim.rs:1447-1453`] — spec Task 7 line 113 requires the log line to reference the socket path; test currently only checks for the literal "ERROR" and newline count. Strengthen with `assert!(log_contents.contains(bogus_sock.to_str().unwrap()))`.
- [ ] [Review][Patch] Story doc File List contradicts the diff [`docs/bmad/implementation-artifacts/1-5-shim-binary-with-hot-path-event-delivery.md:2056`] — File List claims `crates/shim/benches/baselines/linux.json` was added, but Task 9 followup commentary says the locally-seeded baseline was removed and the diff contains no such file. Fix the File List to match reality.
- [ ] [Review][Patch] Bench README references wrong baseline directory name [`crates/shim/benches/README.md` + `.github/workflows/ci.yml:80`] — README/CI message points to `target/criterion/uds_post_ingest/new/estimates.json`, but `--save-baseline ci-current` writes to `…/ci-current/estimates.json`. Correct the path so a dev seeding baselines on first green run isn't sent to a missing dir.

**Deferred**

- [x] [Review][Defer] Aggressive 3 ms read timeout can drop events under CI load [`crates/shim/src/socket.rs:1226-1228`] — deferred, spec-mandated (NFR20 fire-and-forget); track for production observability.
- [x] [Review][Defer] Concurrent shim invocations: log lines >PIPE_BUF (4 KiB) can interleave [`crates/shim/src/log.rs:978-993`] — deferred, edge case for very long error messages; not Story 1.5-blocking.
- [x] [Review][Defer] Test mock `captured` buffer accumulates across connections [`crates/shim/tests/contract_shim.rs:1289-1312`] — deferred, test-infra fragility; not currently buggy but invites misuse.
- [x] [Review][Defer] `civil_from_days` double-corrects negative day counts [`crates/shim/src/log.rs:1016`] — deferred, unreachable from `SystemTime::now()`; defensive bug only.
- [x] [Review][Defer] `Error::Backpressure(String)` is permanently dead [`crates/shim/src/error.rs:847-850`] — deferred, spec explicitly authorized the variant for future `503 <reason>\n` wire protocol; revisit when daemon adds reason field.
- [x] [Review][Defer] Multiple `--hook-kind` flags silently last-wins [`crates/shim/src/main.rs:1131-1157`] — deferred, no current invoker emits duplicates; harden if observed in the wild.
- [x] [Review][Defer] Stdin EPIPE during read mapped to exit 1 (could produce spurious logs during Claude shutdown) [`crates/shim/src/main.rs`] — deferred, race-only, not load-bearing.
- [x] [Review][Defer] Mock listener thread not joined on drop [`crates/shim/tests/contract_shim.rs:1323-1327`, `crates/shim/benches/hot_path.rs:766-770`] — deferred, flake risk only; no correctness bug.
- [x] [Review][Defer] E2E test 2-second envelope-receipt timeout may flake on cold builders [`crates/daemon/tests/contract_daemon.rs:566`] — deferred; pre-built `cargo_bin` covers it today.
