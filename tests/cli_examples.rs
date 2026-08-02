//! End-to-end smoke tests for the cookbook reference entries (Story 4.2;
//! relocated into `docs/cookbook/` by Story 5.13's consolidation; joined by
//! `session-glance` in Story 6-session-glance).
//!
//! Each test orchestrates a real daemon subprocess + a Node subprocess
//! running one of the TypeScript cookbook entries, then asserts the entry's
//! canonical stdout/stderr shape. Mirrors the `tests/cli_replay.rs` shape.
//! Parallel-safe: each daemon binds an ephemeral port the entry reads
//! from its own server.json, and all state is TempDir-scoped per test.
//!
//! The FILE name stays `cli_examples.rs` (pinned by
//! `tests/release_pipeline_docs.rs`); internal helpers use cookbook-entry
//! naming where it aids the reader.
//!
//! Tests gracefully skip when Node 22.6+ is unavailable. CI's `ubuntu-latest`
//! and `macos-latest` runners ship Node 22+ natively; the skip path covers
//! contributors on stale local environments.

use std::path::PathBuf;
use std::process::{Child, Stdio};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use assert_cmd::Command;
use tempfile::TempDir;

const EXAMPLES_TEST_TOKEN: &str = "examples-smoke-token";

/// taskwarrior 2e9cfda3: an isolated label so `stop`/`start` launchd probes
/// address a service that never exists, instead of the developer's real
/// agent. Real `launchctl print` on this label exits 113 ("Could not find"),
/// which the CLI classifies NotLoaded, falling to the pid-file path.
const TEST_LAUNCH_AGENT_LABEL: &str = "com.technicalpickles.bowerbird.test-isolation";

// ---------------------------------------------------------------------------
// Daemon orchestration helpers — mirror tests/cli_replay.rs.
// ---------------------------------------------------------------------------

fn bowerbird_bin() -> Command {
    let mut cmd = Command::cargo_bin("bowerbird").expect("bowerbird binary built");
    cmd.env_remove("BOWERBIRD_CLAUDE_SETTINGS");
    cmd.env_remove("BOWERBIRD_DATA_DIR");
    cmd.env_remove("BOWERBIRD_DAEMON_BIN");
    cmd.env_remove("BOWERBIRD_INGEST_SOCK");
    cmd.env("BOWERBIRD_TOKEN", EXAMPLES_TEST_TOKEN);
    cmd.env("BOWERBIRD_KEYRING_BACKEND", "disable");
    cmd.env("BOWERBIRD_LAUNCH_AGENT_LABEL", TEST_LAUNCH_AGENT_LABEL);
    cmd
}

fn data_dir(tmp: &TempDir) -> PathBuf {
    tmp.path().join(".bowerbird")
}

fn daemon_bin() -> PathBuf {
    assert_cmd::cargo::cargo_bin("bowerbird-daemon")
}

fn bowerbird_cmd_in(tmp: &TempDir) -> Command {
    let mut cmd = bowerbird_bin();
    cmd.env("HOME", tmp.path())
        .env("BOWERBIRD_DATA_DIR", data_dir(tmp))
        .env("BOWERBIRD_DAEMON_BIN", daemon_bin());
    cmd
}

fn read_pid_file(tmp: &TempDir) -> Option<i32> {
    let path = data_dir(tmp).join("bowerbird.pid");
    let s = std::fs::read_to_string(&path).ok()?;
    s.trim().parse::<i32>().ok().filter(|p| *p > 0)
}

fn wait_for_daemon_up(tmp: &TempDir, deadline: Instant) -> bool {
    use std::os::unix::net::UnixStream;
    let sock = data_dir(tmp).join("ingest.sock");
    while Instant::now() < deadline {
        if UnixStream::connect(&sock).is_ok() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    false
}

fn wait_for_pid_dead(pid: i32, deadline: Instant) -> bool {
    while Instant::now() < deadline {
        let r = unsafe { libc::kill(pid, 0) };
        if r != 0 {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    false
}

fn force_stop(tmp: &TempDir) {
    if let Some(pid) = read_pid_file(tmp) {
        unsafe {
            libc::kill(pid, libc::SIGTERM);
        }
        let _ = wait_for_pid_dead(pid, Instant::now() + Duration::from_secs(5));
        if unsafe { libc::kill(pid, 0) } == 0 {
            unsafe {
                libc::kill(pid, libc::SIGKILL);
            }
        }
    }
}

fn start_daemon(tmp: &TempDir) {
    bowerbird_cmd_in(tmp).arg("start").assert().success();
    if !wait_for_daemon_up(tmp, Instant::now() + Duration::from_secs(3)) {
        force_stop(tmp);
        panic!("daemon did not come up within 3s after `bowerbird start`");
    }
}

fn stop_daemon(tmp: &TempDir) {
    if let Some(pid) = read_pid_file(tmp) {
        let _ = bowerbird_cmd_in(tmp).arg("stop").assert();
        let _ = wait_for_pid_dead(pid, Instant::now() + Duration::from_secs(5));
    }
}

// ---------------------------------------------------------------------------
// Node binary discovery + version gate.
// ---------------------------------------------------------------------------

fn node_bin() -> Option<&'static PathBuf> {
    static CACHE: OnceLock<Option<PathBuf>> = OnceLock::new();
    CACHE
        .get_or_init(|| {
            if let Ok(p) = std::env::var("BOWERBIRD_NODE_BIN") {
                let path = PathBuf::from(p);
                if path.is_file() {
                    return Some(path);
                }
            }
            // `which node` — first stdout line is the binary path.
            let out = std::process::Command::new("which")
                .arg("node")
                .output()
                .ok()?;
            if !out.status.success() {
                return None;
            }
            let s = String::from_utf8_lossy(&out.stdout);
            let line = s.lines().next()?.trim();
            if line.is_empty() {
                return None;
            }
            let path = PathBuf::from(line);
            if path.is_file() {
                Some(path)
            } else {
                None
            }
        })
        .as_ref()
}

/// Returns `Some((major, minor))` for the currently-discovered Node, or
/// `None` if the binary isn't found or the version string is unparseable.
fn node_version() -> Option<(u32, u32)> {
    let bin = node_bin()?;
    let out = std::process::Command::new(bin)
        .arg("--version")
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout);
    let line = s.lines().next()?.trim();
    // Expect "vMAJOR.MINOR.PATCH" — strip the v, take the first two dotted parts.
    let stripped = line.strip_prefix('v')?;
    let mut parts = stripped.split('.');
    let major: u32 = parts.next()?.parse().ok()?;
    let minor: u32 = parts.next()?.parse().ok()?;
    Some((major, minor))
}

/// Returns true when the discovered Node is 22.6+ (or 23+). Returns false
/// (with a stderr message) if Node is missing or too old. Tests call this
/// to skip cleanly on contributor environments without Node 22.6.
fn node_22_6_available() -> bool {
    let Some((major, minor)) = node_version() else {
        eprintln!(
            "SKIP: Node not found or version unparseable. Install Node 22.6+ from \
             https://nodejs.org/ or set BOWERBIRD_NODE_BIN to a node 22.6+ binary path."
        );
        return false;
    };
    if major < 22 || (major == 22 && minor < 6) {
        eprintln!(
            "SKIP: Node v{major}.{minor} found but Node 22.6+ is required \
             for --experimental-strip-types."
        );
        return false;
    }
    true
}

fn cookbook_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("docs/cookbook")
}

/// Spawn a Node subprocess running a cookbook entry. The child's stdout and
/// stderr are piped so the test can read them line-by-line; the daemon's
/// bind_addr lives in `<tmp>/.bowerbird/server.json`, which the entry reads
/// via `homedir()`-relative path resolution.
fn spawn_example(
    tmp: &TempDir,
    example_name: &str,
    extra_args: &[&str],
    extra_env: &[(&str, &str)],
) -> Child {
    let node = node_bin().expect("node binary already gated by node_22_6_available");
    let entry = cookbook_dir().join(example_name).join("src/index.ts");
    let mut cmd = std::process::Command::new(node);
    cmd.arg("--experimental-strip-types")
        .arg(&entry)
        .args(extra_args)
        .env("HOME", tmp.path())
        .env("BOWERBIRD_TOKEN", EXAMPLES_TEST_TOKEN)
        .env_remove("BOWERBIRD_DATA_DIR")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (k, v) in extra_env {
        cmd.env(k, v);
    }
    cmd.spawn()
        .unwrap_or_else(|e| panic!("failed to spawn node for {example_name}: {e}"))
}

/// Read the child's stdout until the predicate returns true on a line, OR
/// the deadline expires. Returns the collected lines on success, panics on
/// timeout.
fn read_stdout_until<F: FnMut(&str) -> bool>(
    child: &mut Child,
    deadline: Instant,
    mut done: F,
    label: &str,
) -> Vec<String> {
    use std::io::{BufRead, BufReader};
    let stdout = child
        .stdout
        .take()
        .unwrap_or_else(|| panic!("{label}: stdout not piped"));
    let mut reader = BufReader::new(stdout);
    let mut lines = Vec::new();
    let mut line = String::new();
    while Instant::now() < deadline {
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) => return lines, // EOF
            Ok(_) => {
                let trimmed = line.trim_end().to_string();
                if done(&trimmed) {
                    lines.push(trimmed);
                    return lines;
                }
                lines.push(trimmed);
            }
            Err(_) => break,
        }
    }
    panic!(
        "{label}: timed out before predicate matched; collected lines:\n{}",
        lines.join("\n")
    );
}

fn dump_child_diagnostics(label: &str, child: &mut Child) -> String {
    use std::io::Read;
    let mut stderr = String::new();
    if let Some(mut s) = child.stderr.take() {
        let _ = s.read_to_string(&mut stderr);
    }
    let mut stdout = String::new();
    if let Some(mut s) = child.stdout.take() {
        let _ = s.read_to_string(&mut stdout);
    }
    format!("--- {label} stdout ---\n{stdout}\n--- {label} stderr ---\n{stderr}\n")
}

// ---------------------------------------------------------------------------
// AC #1: state-session-fanout routes state.session.* for both fixture sessions.
// ---------------------------------------------------------------------------

#[test]
fn state_session_fanout_routes_state_frames_for_both_fixture_sessions() {
    if !node_22_6_available() {
        return;
    }
    let tmp = TempDir::new().expect("tempdir");
    start_daemon(&tmp);

    // Replay first so the snapshot-on-subscribe behavior delivers state
    // frames for both fixture sessions on connect.
    bowerbird_cmd_in(&tmp).arg("replay").assert().success();

    let mut child = spawn_example(&tmp, "state-session-fanout", &[], &[]);

    // Drain stderr in a background thread. Two reasons:
    //   1) Story 4.2 Task 7.4 requires asserting the example logs
    //      `new session: claude/session-alpha` and `claude/session-beta`
    //      on stderr — those lines must survive the test run.
    //   2) Without an active reader, a long-running example could fill the
    //      stderr pipe buffer and block on its next stderr write.
    use std::io::{BufRead, BufReader};
    use std::sync::{Arc, Mutex};
    use std::thread;

    let stderr = child
        .stderr
        .take()
        .expect("state-session-fanout: stderr not piped");
    let stderr_buf = Arc::new(Mutex::new(String::new()));
    let stderr_buf_thread = Arc::clone(&stderr_buf);
    let stderr_drainer = thread::spawn(move || {
        let mut reader = BufReader::new(stderr);
        let mut line = String::new();
        loop {
            line.clear();
            match reader.read_line(&mut line) {
                Ok(0) => return,
                Ok(_) => {
                    if let Ok(mut buf) = stderr_buf_thread.lock() {
                        buf.push_str(&line);
                    }
                }
                Err(_) => return,
            }
        }
    });

    // Read stdout until we see both session-alpha and session-beta in the
    // canonical JSON-per-update shape. Each line is `{"event":"state",...}`.
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut saw_alpha = false;
    let mut saw_beta = false;
    let lines = read_stdout_until(
        &mut child,
        deadline,
        |line| {
            if line.contains("\"session_id\":\"session-alpha\"") {
                saw_alpha = true;
            }
            if line.contains("\"session_id\":\"session-beta\"") {
                saw_beta = true;
            }
            saw_alpha && saw_beta
        },
        "state-session-fanout",
    );

    // Trigger graceful close so the example exits 0.
    stop_daemon(&tmp);
    let status = child.wait().expect("state-session-fanout subprocess wait");
    let _ = stderr_drainer.join();
    let drained_stderr = stderr_buf
        .lock()
        .map(|g| g.clone())
        .unwrap_or_else(|_| String::new());

    if !status.success() {
        force_stop(&tmp);
        panic!(
            "example exited non-zero: {status:?}\nstdout:\n{}\nstderr:\n{drained_stderr}",
            lines.join("\n")
        );
    }

    assert!(saw_alpha, "expected session-alpha in stdout");
    assert!(saw_beta, "expected session-beta in stdout");

    // Story 4.2 Task 7.4 — assert the new-session-discovery log lines hit
    // stderr for both fixture sessions. This is the stderr-side observable
    // for AC #1's "treating a previously-unseen `(source, session_id)` as
    // a 'new session appeared' event and logging it on stderr" requirement.
    assert!(
        drained_stderr.contains("new session: claude/session-alpha"),
        "expected stderr to contain `new session: claude/session-alpha`; got:\n{drained_stderr}"
    );
    assert!(
        drained_stderr.contains("new session: claude/session-beta"),
        "expected stderr to contain `new session: claude/session-beta`; got:\n{drained_stderr}"
    );

    force_stop(&tmp);
}

// ---------------------------------------------------------------------------
// AC #2: rest-cursor-pagination paginates session history and renders tool calls.
// ---------------------------------------------------------------------------

#[test]
fn rest_cursor_pagination_paginates_session_history_and_renders_tool_calls() {
    if !node_22_6_available() {
        return;
    }
    let tmp = TempDir::new().expect("tempdir");
    start_daemon(&tmp);
    bowerbird_cmd_in(&tmp).arg("replay").assert().success();

    let mut child = spawn_example(&tmp, "rest-cursor-pagination", &["session-alpha"], &[]);

    let status = child
        .wait()
        .expect("rest-cursor-pagination subprocess wait");
    if !status.success() {
        let diag = dump_child_diagnostics("rest-cursor-pagination", &mut child);
        force_stop(&tmp);
        panic!("rest-cursor-pagination exited non-zero: {status:?}\n{diag}");
    }

    // Collect stdout now that the subprocess has exited cleanly.
    use std::io::Read;
    let mut out = String::new();
    if let Some(mut s) = child.stdout.take() {
        let _ = s.read_to_string(&mut out);
    }
    let lines: Vec<&str> = out.lines().collect();

    // The bundled fixture has 6 events for session-alpha
    // (event_ids 1, 3, 5, 7, 9, 11). The example renders one tab-separated
    // line per event.
    let session_alpha_event_count = 6;
    if lines.len() != session_alpha_event_count {
        force_stop(&tmp);
        panic!(
            "expected {session_alpha_event_count} lines, got {}: {:?}",
            lines.len(),
            lines
        );
    }
    for line in &lines {
        let cols: Vec<&str> = line.split('\t').collect();
        if cols.len() != 4 {
            force_stop(&tmp);
            panic!("expected 4 tab-separated columns; got line: {line}");
        }
        // First column parses as an integer event_id.
        if cols[0].parse::<i64>().is_err() {
            force_stop(&tmp);
            panic!("first column should be a numeric event_id; got: {line}");
        }
    }
    // Spot-check the (kind, tool, reaction) sequence — assert on content
    // rather than on specific event_ids, because the daemon's startup
    // RecordingStarted sentinel takes event_id=1, shifting the replay
    // events by 1 (session-alpha lands on event_ids 2, 4, 6, 8, 10, 12).
    // The kind/tool/reaction shape is the stable contract.
    let expected_tail: Vec<(&str, &str, &str)> = vec![
        ("PreToolUse", "Read", "Continue"),
        ("PostToolUse", "Read", "Continue"),
        ("PreToolUse", "Edit", "Continue"),
        ("PostToolUse", "Edit", "Continue"),
        ("Notification", "-", "-"),
        ("Stop", "-", "-"),
    ];
    for (i, (kind, tool, reaction)) in expected_tail.iter().enumerate() {
        let line = lines
            .get(i)
            .unwrap_or_else(|| panic!("missing line {i}; collected:\n{}", lines.join("\n")));
        let cols: Vec<&str> = line.split('\t').collect();
        assert_eq!(
            cols.get(1).copied().unwrap_or(""),
            *kind,
            "line {i} kind mismatch: {line}"
        );
        assert_eq!(
            cols.get(2).copied().unwrap_or(""),
            *tool,
            "line {i} tool mismatch: {line}"
        );
        assert_eq!(
            cols.get(3).copied().unwrap_or(""),
            *reaction,
            "line {i} reaction mismatch: {line}"
        );
    }

    stop_daemon(&tmp);
    force_stop(&tmp);
}

// ---------------------------------------------------------------------------
// AC #2: rest-cursor-pagination defaults to session-alpha when no CLI arg given.
// Exercises the `process.argv[2] ?? "session-alpha"` default in rest-cursor-pagination
// (src/index.ts). The primary smoke test always passes `session-alpha`
// explicitly, so the default-arg branch would otherwise be untested.
// ---------------------------------------------------------------------------

#[test]
fn rest_cursor_pagination_defaults_to_session_alpha_when_no_arg() {
    if !node_22_6_available() {
        return;
    }
    let tmp = TempDir::new().expect("tempdir");
    start_daemon(&tmp);
    bowerbird_cmd_in(&tmp).arg("replay").assert().success();

    // No CLI args — example should pick session-alpha by default.
    let mut child = spawn_example(&tmp, "rest-cursor-pagination", &[], &[]);

    let status = child.wait().expect("rest-cursor-pagination (default) wait");
    if !status.success() {
        let diag = dump_child_diagnostics("rest-cursor-pagination-default", &mut child);
        force_stop(&tmp);
        panic!("rest-cursor-pagination exited non-zero with default arg: {status:?}\n{diag}");
    }

    use std::io::Read;
    let mut out = String::new();
    if let Some(mut s) = child.stdout.take() {
        let _ = s.read_to_string(&mut out);
    }
    let lines: Vec<&str> = out.lines().collect();
    // session-alpha has 6 events in the bundled fixture; the default-arg
    // path must reach the same render shape as the explicit-arg path.
    assert_eq!(
        lines.len(),
        6,
        "default arg should render session-alpha's 6 events; got {} lines:\n{out}",
        lines.len()
    );

    stop_daemon(&tmp);
    force_stop(&tmp);
}

// ---------------------------------------------------------------------------
// AC #2: rest-cursor-pagination renders gracefully when the requested session has
// no events. The daemon returns HTTP 200 with an empty `events` array and a
// null `cursor` for unknown sessions (not HTTP 404), so the example's loop
// exits its first iteration without printing any rows and returns exit 0.
//
// Story 5.4 update: `GET /sessions/{id}/events` now returns `404 Not Found`
// for an id with no `session_projections` row — see protocol-changelog.md
// v1.0 → v1.1. The example's `if (res.status === 404)` branch in
// `docs/cookbook/rest-cursor-pagination/src/index.ts` (previously structurally
// dead against the 200-empty quirk) is now load-bearing. This test pins the
// 404 contract: the example exits non-zero with a `session ... not found`
// stderr instead of silently rendering an empty stream.
// ---------------------------------------------------------------------------

#[test]
fn rest_cursor_pagination_surfaces_404_for_unknown_session() {
    if !node_22_6_available() {
        return;
    }
    let tmp = TempDir::new().expect("tempdir");
    start_daemon(&tmp);
    bowerbird_cmd_in(&tmp).arg("replay").assert().success();

    let mut child = spawn_example(
        &tmp,
        "rest-cursor-pagination",
        &["definitely-not-a-real-session"],
        &[],
    );

    let status = child.wait().expect("rest-cursor-pagination (unknown) wait");
    let diag = dump_child_diagnostics("rest-cursor-pagination-unknown", &mut child);

    // Tear the daemon down BEFORE asserting so a failing assertion can't leave
    // the test daemon running (Story 5.4 review). Both calls are the standard
    // idempotent teardown used by the success paths in this file.
    stop_daemon(&tmp);
    force_stop(&tmp);

    assert!(
        !status.success(),
        "rest-cursor-pagination should exit non-zero when the daemon returns 404 \
         for an unknown session; got: {status:?}\n{diag}"
    );
    assert!(
        diag.contains("session definitely-not-a-real-session not found"),
        "expected 'session ... not found' on stderr per the 404 branch; got:\n{diag}"
    );
}

// ---------------------------------------------------------------------------
// AC #3: dropped-frame-recovery handles Close and recovers via REST catch-up.
// ---------------------------------------------------------------------------

#[test]
fn dropped_frame_recovery_recovers_after_close_frame_and_resumes() {
    if !node_22_6_available() {
        return;
    }
    let tmp = TempDir::new().expect("tempdir");
    start_daemon(&tmp);
    bowerbird_cmd_in(&tmp).arg("replay").assert().success();

    // BOWERBIRD_EXAMPLE_MAX_IDLE_MS=2000 makes the example exit cleanly
    // 2s after the last activity (WS frame or successful recover()),
    // provided at least one recover() has succeeded. The smoke relies on
    // this bounded exit shape rather than engineering an explicit "you're
    // done" message into the example.
    let mut child = spawn_example(
        &tmp,
        "dropped-frame-recovery",
        &[],
        &[("BOWERBIRD_EXAMPLE_MAX_IDLE_MS", "2000")],
    );

    // Drain stderr continuously in a background thread to avoid pipe
    // deadlock — the example logs frequently (subscribe, recover diagnostics,
    // reconnect attempts), and if we only read stderr looking for one marker
    // and then stop, the pipe buffer fills and the example blocks on its
    // next stderr write, never reaching the stdout print of `recovered`.
    use std::io::{BufRead, BufReader};
    use std::sync::{Arc, Mutex};
    use std::thread;

    let stderr = child
        .stderr
        .take()
        .expect("dropped-frame-recovery: stderr not piped");
    let stderr_buf = Arc::new(Mutex::new(String::new()));
    let stderr_buf_thread = Arc::clone(&stderr_buf);
    let stderr_drainer = thread::spawn(move || {
        let mut reader = BufReader::new(stderr);
        let mut line = String::new();
        loop {
            line.clear();
            match reader.read_line(&mut line) {
                Ok(0) => return,
                Ok(_) => {
                    if let Ok(mut buf) = stderr_buf_thread.lock() {
                        buf.push_str(&line);
                    }
                }
                Err(_) => return,
            }
        }
    });

    // Give the example time to connect + subscribe. 500ms is generous;
    // a fast machine sees `ws open` within ~50ms of spawn. We don't
    // assert on the marker here because the stderr drainer thread owns
    // the read side; correctness comes from the recovered-JSON assertion
    // below.
    std::thread::sleep(Duration::from_millis(500));

    // Trigger the Close branch via `bowerbird stop`. The daemon emits
    // Close, then tears down both WS and REST listeners. The example's
    // first `recover()` fails (`fetch failed`) — by design, because the
    // realistic recovery scenario is "daemon restarted, presenter
    // reconnects." So we restart the daemon and replay; the example
    // re-reads `server.json` (the daemon's discovery anchor; Story 3.2)
    // on each recover() attempt, picks up the new ephemeral bind addr,
    // and the recovery succeeds against the new daemon.
    stop_daemon(&tmp);

    // Brief gap to let the example's first recover() fail. The new
    // daemon will bind a different ephemeral port; the example re-reads
    // server.json on each recover() retry.
    std::thread::sleep(Duration::from_millis(300));
    start_daemon(&tmp);
    bowerbird_cmd_in(&tmp).arg("replay").assert().success();

    // Drain stdout until we see the recovered JSON OR the deadline expires.
    let stdout = child
        .stdout
        .take()
        .expect("dropped-frame-recovery: stdout not piped");
    let mut sreader = BufReader::new(stdout);
    let mut stdout_lines = Vec::new();
    let mut saw_recovered = false;
    let recover_deadline = Instant::now() + Duration::from_secs(15);
    let mut line = String::new();
    while Instant::now() < recover_deadline {
        line.clear();
        match sreader.read_line(&mut line) {
            Ok(0) => break,
            Ok(_) => {
                let trimmed = line.trim_end().to_string();
                if trimmed.contains("\"event\":\"recovered\"") {
                    saw_recovered = true;
                    stdout_lines.push(trimmed);
                    break;
                }
                stdout_lines.push(trimmed);
            }
            Err(_) => break,
        }
    }

    // Wait for the example to exit (via BOWERBIRD_EXAMPLE_MAX_IDLE_MS).
    let status = child.wait().expect("dropped-frame-recovery wait");
    let _ = stderr_drainer.join();
    let drained_stderr = stderr_buf
        .lock()
        .map(|g| g.clone())
        .unwrap_or_else(|_| String::new());

    if !saw_recovered {
        force_stop(&tmp);
        panic!(
            "dropped-frame-recovery did not print recovered JSON within 15s; stdout:\n{}\nstderr:\n{drained_stderr}",
            stdout_lines.join("\n")
        );
    }
    if !status.success() {
        force_stop(&tmp);
        panic!(
            "dropped-frame-recovery exited non-zero: {status:?}; stdout:\n{}\nstderr:\n{drained_stderr}",
            stdout_lines.join("\n")
        );
    }

    force_stop(&tmp);
}

// ---------------------------------------------------------------------------
// AC #1, #2, #3: examples fail clearly when the daemon is down.
// ---------------------------------------------------------------------------

/// Every cookbook entry directory, derived from the `docs/cookbook/*/` glob
/// exactly as `tests/cli_docs_drift.rs` and `tests/cli_examples_drift.rs` do
/// (Story 6-session-glance, AC 4): a subdir with no `package.json` is not an
/// entry and is skipped, mirroring CI's own filter.
///
/// The daemon-down contract below is cookbook-wide, so the loop is derived
/// rather than listed. A hardcoded list here would have exactly the failure
/// mode AC 4 is about: a new entry gets typechecked and smoked but silently
/// skips the "fails clearly with no daemon" assertion.
fn cookbook_entry_dirs() -> Vec<String> {
    let mut dirs: Vec<String> = std::fs::read_dir(cookbook_dir())
        .expect("read_dir docs/cookbook")
        .filter_map(|entry| {
            let entry = entry.expect("dir entry");
            if !entry.file_type().expect("file type").is_dir() {
                return None;
            }
            if !entry.path().join("package.json").is_file() {
                return None;
            }
            Some(entry.file_name().to_string_lossy().into_owned())
        })
        .collect();
    dirs.sort();
    // A13 positive companion: an empty derivation would make the loop below
    // assert nothing while still reporting green.
    assert!(
        dirs.len() >= 3 && dirs.iter().any(|d| d == "session-glance"),
        "docs/cookbook/*/ derivation looks wrong: {dirs:?}"
    );
    dirs
}

#[test]
fn cookbook_entries_fail_clearly_when_daemon_down() {
    if !node_22_6_available() {
        return;
    }
    // No daemon started, so `server.json` is MISSING. That is daemon-down
    // failure mode (a). Mode (b), `server.json` present but stale and the
    // connection refused, is a different code path and is covered by
    // `session_glance_names_the_address_when_server_json_is_stale`.
    let tmp = TempDir::new().expect("tempdir");

    for example in cookbook_entry_dirs() {
        let example = example.as_str();
        let mut child = spawn_example(&tmp, example, &[], &[]);
        let status = child.wait().expect("daemon-down subprocess wait");
        assert!(
            !status.success(),
            "{example}: expected non-zero exit with no daemon"
        );
        use std::io::Read;
        let mut stderr = String::new();
        if let Some(mut s) = child.stderr.take() {
            let _ = s.read_to_string(&mut stderr);
        }
        assert!(
            stderr.contains("server.json"),
            "{example}: stderr should mention server.json on daemon-down; got:\n{stderr}"
        );
    }
}

// ---------------------------------------------------------------------------
// Story 6-session-glance: the one-shot glance entry.
//
// The bundled `bowerbird replay` fixture cannot drive these assertions: every
// one of its sessions ends on `Stop` (so they all land on `Idle`), none carry
// a `cwd`, and none carry a `pid`, which means the liveness probe would mark
// them all `Ended` on its next 5s tick (`projection/liveness.rs`:
// `last_pid IS NULL` -> `no_pid_at_upgrade`). So these tests replay their own
// fixture: distinct LIVE pids so the probe leaves the rows alone (distinct
// because a shared pid triggers Story 5.11 supersession), real directory
// trees so the repo derivation has something to walk, and one deliberately
// `Ended` session so "non-Ended only" is an assertion rather than an accident.
// ---------------------------------------------------------------------------

/// Hang detector for the glance polls, not a latency assertion.
const GLANCE_HANG_GUARD: Duration = Duration::from_secs(30);

struct GlanceFixture {
    /// Path to the JSONL file `bowerbird replay` consumes.
    path: PathBuf,
    /// Basename of the ordinary repository the derivation should find by
    /// walking up from a subdirectory.
    repo: String,
    /// Basename of the git-worktree directory (its `.git` is a FILE, so the
    /// derivation must stop there rather than walking to the outer repo).
    worktree: String,
}

/// Build the replay fixture plus the directory tree its `cwd` values point at.
///
/// `live_pids` must be three DISTINCT pids of processes that are alive: the
/// liveness probe ends a session whose `last_pid` is null or dead, and the
/// event-driven supersession path ends the predecessor when two sessions
/// claim the same pid.
fn write_glance_fixture(tmp: &TempDir, live_pids: [u32; 3]) -> GlanceFixture {
    let root = tmp.path().join("work");
    let repo = root.join("bowerbird-fixture-repo");
    let repo_subdir = repo.join("crates").join("daemon");
    // A worktree nested INSIDE the repo, so a derivation that tested
    // `.git`.isDirectory() would walk past it and report the outer repo.
    let worktree = repo.join("worktrees").join("wt-feature-branch");
    std::fs::create_dir_all(repo.join(".git")).expect("mkdir repo/.git");
    std::fs::create_dir_all(&repo_subdir).expect("mkdir repo subdir");
    std::fs::create_dir_all(&worktree).expect("mkdir worktree");
    std::fs::write(
        worktree.join(".git"),
        "gitdir: /elsewhere/.git/worktrees/wt-feature-branch\n",
    )
    .expect("write worktree .git file");

    let line = |event_id: i64,
                session_id: &str,
                kind: &str,
                payload: serde_json::Value,
                pid: Option<u32>,
                cwd: Option<&std::path::Path>| {
        serde_json::json!({
            "event_id": event_id,
            "source": "claude",
            "session_id": session_id,
            "kind": kind,
            "reaction": serde_json::Value::Null,
            // `payload` rides the wire as a JSON *string*, verbatim.
            "payload": payload.to_string(),
            "created_at": 1_700_000_000_000i64,
            "pid": pid,
            "cwd": cwd.map(|p| p.to_string_lossy().into_owned()),
        })
        .to_string()
    };

    let body = [
        // PreToolUse -> Working. cwd is BELOW the repo root, so rule 2 has to
        // walk up to find `.git`.
        line(
            1,
            "sess-alpha",
            "PreToolUse",
            serde_json::json!({ "tool_name": "Read" }),
            Some(live_pids[0]),
            Some(&repo_subdir),
        ),
        // Notification(permission_prompt) -> WaitingInput. cwd IS the
        // worktree, whose `.git` is a file.
        line(
            2,
            "sess-beta",
            "Notification",
            serde_json::json!({ "notification_type": "permission_prompt" }),
            Some(live_pids[1]),
            Some(&worktree),
        ),
        // Stop -> Idle, with no cwd at all: the `(unknown repo)` bucket.
        line(
            3,
            "sess-gamma",
            "Stop",
            serde_json::json!({}),
            Some(live_pids[2]),
            None,
        ),
        // SessionEnded -> Ended, which the default filter must exclude. No
        // pid needed: the probe skips rows that are already Ended.
        line(
            4,
            "sess-delta",
            "SessionEnded",
            serde_json::json!({ "reason": "pid_dead" }),
            None,
            Some(&repo),
        ),
    ]
    .join("\n");

    let path = tmp.path().join("glance-fixture.jsonl");
    std::fs::write(&path, format!("{body}\n")).expect("write glance fixture");
    GlanceFixture {
        path,
        repo: repo
            .file_name()
            .expect("repo basename")
            .to_string_lossy()
            .into_owned(),
        worktree: worktree
            .file_name()
            .expect("worktree basename")
            .to_string_lossy()
            .into_owned(),
    }
}

/// Three distinct pids that are certainly alive: this test process, the
/// daemon it just started, and pid 1 (init/launchd, which `kill(1, 0)`
/// reports as EPERM and the probe therefore treats as alive).
fn distinct_live_pids(tmp: &TempDir) -> [u32; 3] {
    let self_pid = std::process::id();
    let daemon_pid = read_pid_file(tmp).expect("daemon pid file") as u32;
    assert_ne!(
        self_pid, daemon_pid,
        "the fixture needs distinct pids; a shared pid triggers Story 5.11 supersession"
    );
    [self_pid, daemon_pid, 1]
}

/// Run session-glance to completion. Returns `(success, stdout, stderr)`.
fn run_glance(tmp: &TempDir, args: &[&str]) -> (bool, String, String) {
    use std::io::Read;
    let mut child = spawn_example(tmp, "session-glance", args, &[]);
    let status = child.wait().expect("session-glance wait");
    let mut stdout = String::new();
    if let Some(mut s) = child.stdout.take() {
        let _ = s.read_to_string(&mut stdout);
    }
    let mut stderr = String::new();
    if let Some(mut s) = child.stderr.take() {
        let _ = s.read_to_string(&mut stderr);
    }
    (status.success(), stdout, stderr)
}

/// Poll `--count` until it reports `expected`, or the hang guard expires.
///
/// `POST /replay` returns once the envelopes are queued on the ingest
/// channel, not once the projection has committed, so the rows land shortly
/// after `bowerbird replay` exits. This is a probe fence on an observable
/// (the count the entry itself reports), not a sleep-to-synchronize.
fn wait_for_glance_count(tmp: &TempDir, expected: usize) {
    let deadline = Instant::now() + GLANCE_HANG_GUARD;
    let mut last = String::new();
    while Instant::now() < deadline {
        let (ok, stdout, stderr) = run_glance(tmp, &["--count"]);
        last = format!("ok={ok} stdout={stdout:?} stderr={stderr:?}");
        if let Some(n) = ok.then(|| stdout.trim().parse::<usize>().ok()).flatten() {
            if n == expected {
                return;
            }
            // Fail fast rather than burning the hang guard: the count only
            // rises as rows commit, so OVERSHOOTING it can never be fixed by
            // waiting. The fixture has exactly `expected` non-Ended sessions,
            // so a higher count means the `?state=` filter is letting the
            // Ended one through.
            if n > expected {
                force_stop(tmp);
                panic!(
                    "session-glance --count reported {n}, more than the {expected} \
                     non-Ended sessions the fixture defines. The ?state= filter is \
                     not excluding Ended."
                );
            }
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    force_stop(tmp);
    panic!("session-glance --count never reached {expected}; last attempt: {last}");
}

#[test]
fn session_glance_groups_live_sessions_by_repo_with_ages() {
    if !node_22_6_available() {
        return;
    }
    let tmp = TempDir::new().expect("tempdir");
    start_daemon(&tmp);
    let fixture = write_glance_fixture(&tmp, distinct_live_pids(&tmp));
    bowerbird_cmd_in(&tmp)
        .arg("replay")
        .arg(&fixture.path)
        .assert()
        .success();
    wait_for_glance_count(&tmp, 3);

    let (ok, stdout, stderr) = run_glance(&tmp, &[]);

    // Positive companion for the "Ended is excluded" assertion below: ask for
    // Ended explicitly and confirm sess-delta really is in the daemon and
    // really is Ended. Without this, its absence from the default output
    // could just mean the fixture row never landed.
    let (ended_ok, ended_stdout, ended_stderr) = run_glance(&tmp, &["--state=ended"]);

    stop_daemon(&tmp);
    force_stop(&tmp);

    assert!(ok, "session-glance exited non-zero; stderr:\n{stderr}");
    assert!(
        ended_ok,
        "session-glance --state=ended exited non-zero; stderr:\n{ended_stderr}"
    );
    assert!(
        ended_stdout.contains("claude/sess-delta") && ended_stdout.contains("Ended"),
        "precondition: sess-delta must exist in the Ended state; got:\n{ended_stdout}"
    );

    // AC 1: grouped by repository, derived presenter-side from `cwd`.
    let lines: Vec<&str> = stdout.lines().collect();
    for heading in [
        fixture.repo.as_str(),
        fixture.worktree.as_str(),
        "(unknown repo)",
    ] {
        assert!(
            lines.contains(&heading),
            "expected a `{heading}` repo heading; got:\n{stdout}"
        );
    }
    // The worktree's `.git` is a FILE. Grouping it under the outer repo would
    // mean the derivation tested isDirectory() and walked past it.
    let worktree_idx = lines
        .iter()
        .position(|l| *l == fixture.worktree)
        .expect("worktree heading");
    assert!(
        lines[worktree_idx + 1].contains("claude/sess-beta"),
        "sess-beta must sit under the worktree heading; got:\n{stdout}"
    );

    // AC 1: each session carries its state and an age.
    for (session, state) in [
        ("sess-alpha", "Working"),
        ("sess-beta", "WaitingInput"),
        ("sess-gamma", "Idle"),
    ] {
        let line = lines
            .iter()
            .find(|l| l.contains(&format!("claude/{session}")))
            .unwrap_or_else(|| panic!("no line for {session}; got:\n{stdout}"));
        assert!(
            line.starts_with("  "),
            "session lines are indented under their repo heading; got: {line:?}"
        );
        assert!(
            line.contains(state),
            "{session} should render `{state}` (PascalCase, verbatim from the \
             wire); got: {line:?}"
        );
        let age = line.rsplit("  ").next().unwrap_or("");
        assert!(
            age.ends_with('s') || age.ends_with('m') || age.ends_with('h'),
            "{session} should render an age from started_at, not a raw \
             timestamp; got: {line:?}"
        );
        assert!(
            !line.contains("NaN") && !line.contains("1970"),
            "age must never render NaN or a 1970 timestamp; got: {line:?}"
        );
    }

    // AC 1: every NON-ENDED session, which means the Ended one is absent.
    assert!(
        !stdout.contains("sess-delta"),
        "sess-delta is Ended and must not appear in the default glance; got:\n{stdout}"
    );
    assert!(
        !stdout.contains("Ended"),
        "the default filter is the four non-Ended tokens; got:\n{stdout}"
    );
}

#[test]
fn session_glance_machine_modes_pin_the_output_contract() {
    if !node_22_6_available() {
        return;
    }
    let tmp = TempDir::new().expect("tempdir");
    start_daemon(&tmp);
    let fixture = write_glance_fixture(&tmp, distinct_live_pids(&tmp));
    bowerbird_cmd_in(&tmp)
        .arg("replay")
        .arg(&fixture.path)
        .assert()
        .success();
    wait_for_glance_count(&tmp, 3);

    let count = run_glance(&tmp, &["--count"]);
    let blocked = run_glance(&tmp, &["--count", "--state=waitinginput"]);
    let json = run_glance(&tmp, &["--format=json"]);
    let bad_flag = run_glance(&tmp, &["--fromat=json"]);
    let bad_state = run_glance(&tmp, &["--state=running"]);

    stop_daemon(&tmp);
    force_stop(&tmp);

    // AC 2: `--count` is a single integer on stdout and nothing else. This is
    // the entirety of the tmux status line's data path, so it is asserted as
    // a contract, not spot-checked.
    assert!(count.0, "--count exited non-zero; stderr:\n{}", count.2);
    assert_eq!(
        count.1.lines().count(),
        1,
        "--count must print exactly one line; got:\n{}",
        count.1
    );
    assert_eq!(
        count
            .1
            .trim()
            .parse::<usize>()
            .expect("--count is an integer"),
        3
    );
    assert!(
        blocked.0 && blocked.1.trim() == "1",
        "--count --state=waitinginput should report the one blocked session; \
         got stdout={:?} stderr={:?}",
        blocked.1,
        blocked.2
    );

    // AC 2: `--format=json` is NDJSON, one object per session, fixed field set.
    assert!(json.0, "--format=json exited non-zero; stderr:\n{}", json.2);
    let rows: Vec<serde_json::Value> = json
        .1
        .lines()
        .map(|l| serde_json::from_str(l).unwrap_or_else(|e| panic!("NDJSON line {l:?}: {e}")))
        .collect();
    assert_eq!(rows.len(), 3, "one object per session; got:\n{}", json.1);
    const CONTRACT_FIELDS: &[&str] = &[
        "repo",
        "source",
        "session_id",
        "current_state",
        "age",
        "age_seconds",
        "started_at",
        "cwd",
    ];
    for row in &rows {
        let obj = row.as_object().expect("NDJSON row is an object");
        let mut keys: Vec<&str> = obj.keys().map(|k| k.as_str()).collect();
        keys.sort();
        let mut expected: Vec<&str> = CONTRACT_FIELDS.to_vec();
        expected.sort();
        assert_eq!(
            keys, expected,
            "the --format=json field set is the documented contract \
             (README.md 'Run it'); adding or removing a key breaks every \
             consumer. Row: {row}"
        );
    }
    // The repo derivation is present in machine mode too, and the null-cwd
    // session is bucketed rather than dropped.
    let repos: Vec<&str> = rows
        .iter()
        .map(|r| r["repo"].as_str().expect("repo is a string"))
        .collect();
    assert!(
        repos.contains(&"(unknown repo)"),
        "a null cwd must land in the named bucket, not be dropped; got {repos:?}"
    );
    assert!(
        repos.contains(&fixture.worktree.as_str()),
        "the worktree must group by its own basename; got {repos:?}"
    );

    // AC 2 / `machine-output-contract`: bad input is a one-line failure that
    // names the input and the accepted set, never a stack trace.
    for (label, (ok, stdout, stderr), needle, accepted) in [
        ("--fromat=json", bad_flag, "--fromat=json", "--count"),
        (
            "--state=running",
            bad_state,
            "running",
            "idle, working, waitinginput, ended, unknown",
        ),
    ] {
        assert!(!ok, "{label}: expected a non-zero exit; stdout:\n{stdout}");
        assert_eq!(
            stderr.lines().count(),
            1,
            "{label}: expected exactly one stderr line, not a stack trace; got:\n{stderr}"
        );
        assert!(
            stderr.contains(needle),
            "{label}: stderr must name the bad input; got:\n{stderr}"
        );
        assert!(
            stderr.contains(accepted),
            "{label}: stderr must list the accepted set; got:\n{stderr}"
        );
    }
}

/// Daemon-down failure mode (b), and the CI-side counterpart of AC 6's
/// provoked adversity ("daemon stopped mid-day").
///
/// `cookbook_entries_fail_clearly_when_daemon_down` covers mode (a) only: no
/// daemon ever ran, so `server.json` is missing and the read fails. The
/// interesting case is different. The daemon removes `server.json` on a CLEAN
/// shutdown, so a crash, an OOM kill, or a `kill -9` leaves the file behind
/// pointing at an address nothing is listening on. Node reports that as a
/// bare `TypeError: fetch failed`, which names neither the address nor the
/// fix and is precisely the stack-trace-shaped failure AC 6 forbids.
#[test]
fn session_glance_names_the_address_when_server_json_is_stale() {
    if !node_22_6_available() {
        return;
    }
    let tmp = TempDir::new().expect("tempdir");
    start_daemon(&tmp);

    let server_json = data_dir(&tmp).join("server.json");
    let before = std::fs::read_to_string(&server_json).expect("server.json while daemon is up");
    let bind_addr = serde_json::from_str::<serde_json::Value>(&before).expect("server.json parses")
        ["bind_addr"]
        .as_str()
        .expect("bind_addr is a string")
        .to_string();

    // SIGKILL, not `bowerbird stop`: a graceful stop deletes server.json and
    // would put us back on mode (a).
    let pid = read_pid_file(&tmp).expect("daemon pid file");
    #[allow(unsafe_code)]
    unsafe {
        libc::kill(pid, libc::SIGKILL);
    }
    assert!(
        wait_for_pid_dead(pid, Instant::now() + GLANCE_HANG_GUARD),
        "daemon did not die after SIGKILL"
    );

    // A13 positive companion: the assertions below are only meaningful if the
    // precondition actually fired, i.e. the file really did survive the kill.
    // If a future change made shutdown remove it here too, this test would
    // otherwise keep passing while silently testing mode (a) again.
    let after = std::fs::read_to_string(&server_json)
        .expect("server.json must SURVIVE an unclean daemon death; that is mode (b)");
    assert_eq!(
        after, before,
        "the stale server.json should still point at the dead daemon's address"
    );

    let (ok, stdout, stderr) = run_glance(&tmp, &[]);
    force_stop(&tmp);

    assert!(!ok, "expected a non-zero exit; stdout:\n{stdout}");
    assert_eq!(
        stderr.lines().count(),
        1,
        "expected exactly one stderr line, never a stack trace; got:\n{stderr}"
    );
    for needle in [
        "cannot reach the daemon",
        &format!("http://{bind_addr}"),
        "server.json",
        "bowerbird start",
    ] {
        assert!(
            stderr.contains(needle),
            "stale-server.json message must contain {needle:?}; got:\n{stderr}"
        );
    }
    for banned in ["TypeError", "fetch failed", "    at "] {
        assert!(
            !stderr.contains(banned),
            "AC 6 forbids the raw Node failure shape; stderr contains {banned:?}:\n{stderr}"
        );
    }
    // And it must NOT be the mode-(a) message: the file is right there.
    assert!(
        !stderr.contains("cannot read"),
        "mode (b) must not be reported as a missing server.json; got:\n{stderr}"
    );
}
