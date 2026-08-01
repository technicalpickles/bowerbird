//! End-to-end smoke tests for the three cookbook reference entries (Story
//! 4.2; relocated into `docs/cookbook/` by Story 5.13's consolidation).
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

#[test]
fn cookbook_entries_fail_clearly_when_daemon_down() {
    if !node_22_6_available() {
        return;
    }
    // No daemon started.
    let tmp = TempDir::new().expect("tempdir");

    for example in [
        "state-session-fanout",
        "rest-cursor-pagination",
        "dropped-frame-recovery",
    ] {
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
