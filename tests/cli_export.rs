//! End-to-end tests for `bowerbird export` (Story 4.1 AC #2).
//!
//! These tests spawn real `bowerbird-daemon` subprocesses and run a replay
//! to populate the DB, then exercise `bowerbird export` against the
//! resulting sessions. Parallel-safe: per-test TempDir data dirs and
//! ephemeral daemon ports.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use assert_cmd::Command;
use protocol::Event;
use tempfile::TempDir;

const EXPORT_TEST_TOKEN: &str = "export-test-token";

/// taskwarrior 2e9cfda3: an isolated label so `stop`/`start` launchd probes
/// address a service that never exists, instead of the developer's real
/// agent. Real `launchctl print` on this label exits 113 ("Could not find"),
/// which the CLI classifies NotLoaded, falling to the pid-file path.
const TEST_LAUNCH_AGENT_LABEL: &str = "com.technicalpickles.bowerbird.test-isolation";

fn bowerbird_bin() -> Command {
    let mut cmd = Command::cargo_bin("bowerbird").expect("bowerbird binary built");
    cmd.env_remove("BOWERBIRD_CLAUDE_SETTINGS");
    cmd.env_remove("BOWERBIRD_DATA_DIR");
    cmd.env_remove("BOWERBIRD_DAEMON_BIN");
    cmd.env_remove("BOWERBIRD_INGEST_SOCK");
    cmd.env("BOWERBIRD_TOKEN", EXPORT_TEST_TOKEN);
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

/// Replay the bundled fixture against the running daemon (best-effort
/// success check).
fn replay_bundled_fixture(tmp: &TempDir) {
    bowerbird_cmd_in(tmp).arg("replay").assert().success();
}

/// AC #2: `bowerbird export <session-id>` writes JSONL of `Event` records
/// to stdout. Each line parses as a `protocol::Event`.
#[test]
fn export_writes_jsonl_of_session_events_to_stdout() {
    let tmp = TempDir::new().expect("tempdir");
    start_daemon(&tmp);

    // Prime the DB by replaying the bundled fixture (12 events / 2 sessions).
    replay_bundled_fixture(&tmp);

    // Give the writer task a moment to drain.
    std::thread::sleep(Duration::from_millis(500));

    let assertion = bowerbird_cmd_in(&tmp)
        .arg("export")
        .arg("session-alpha")
        .assert();
    let assertion = assertion.success();
    let stdout = String::from_utf8_lossy(&assertion.get_output().stdout).into_owned();

    // Each line should parse as a protocol::Event.
    let mut event_count = 0usize;
    for line in stdout.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let event: Event = serde_json::from_str(line).unwrap_or_else(|e| {
            stop_daemon(&tmp);
            panic!("line did not parse as Event: {e}; line: {line}");
        });
        assert_eq!(event.session_id, "session-alpha");
        event_count += 1;
    }

    // The bundled fixture has 6 events for session-alpha.
    if event_count < 1 {
        stop_daemon(&tmp);
        panic!("expected ≥1 events for session-alpha; got {event_count}\nstdout:\n{stdout}");
    }

    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr).into_owned();
    if !stderr.contains("exported ") {
        stop_daemon(&tmp);
        panic!("expected `exported ...` on stderr; got:\n{stderr}");
    }

    stop_daemon(&tmp);
}

/// AC #2: `-o <path>` directs output to a file; stdout stays clean.
#[test]
fn export_writes_to_file_when_output_flag_given() {
    let tmp = TempDir::new().expect("tempdir");
    start_daemon(&tmp);
    replay_bundled_fixture(&tmp);
    std::thread::sleep(Duration::from_millis(500));

    let out_path = tmp.path().join("export-alpha.jsonl");
    let assertion = bowerbird_cmd_in(&tmp)
        .arg("export")
        .arg("session-alpha")
        .arg("-o")
        .arg(&out_path)
        .assert();
    let _ = assertion.success();

    let body = std::fs::read_to_string(&out_path).expect("read output file");
    let mut count = 0usize;
    for line in body.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let _event: Event = serde_json::from_str(line).unwrap_or_else(|e| {
            stop_daemon(&tmp);
            panic!("line did not parse as Event: {e}; line: {line}");
        });
        count += 1;
    }
    if count < 1 {
        stop_daemon(&tmp);
        panic!("output file empty");
    }

    stop_daemon(&tmp);
}

/// AC #2: unknown session id surfaces a clear `session <id> not found`
/// message on stderr and exits non-zero.
#[test]
fn export_returns_session_not_found_for_unknown_id() {
    let tmp = TempDir::new().expect("tempdir");
    start_daemon(&tmp);

    let assertion = bowerbird_cmd_in(&tmp)
        .arg("export")
        .arg("bogus-session-id-that-does-not-exist")
        .assert();
    let assertion = assertion.failure();
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr).into_owned();
    if !stderr.contains("session bogus-session-id-that-does-not-exist not found") {
        stop_daemon(&tmp);
        panic!("expected `session ... not found`; got:\n{stderr}");
    }

    stop_daemon(&tmp);
}

/// AC #2 (transport failure): `bowerbird export` against a stopped daemon
/// exits non-zero with a clear stderr message. Parity with the matching
/// `replay_fails_clearly_when_daemon_down` test.
#[test]
fn export_fails_clearly_when_daemon_down() {
    let tmp = TempDir::new().expect("tempdir");
    // No daemon started.

    let assertion = bowerbird_cmd_in(&tmp)
        .arg("export")
        .arg("any-session-id")
        .assert()
        .failure();
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr).into_owned();
    assert!(
        stderr.contains("cannot reach daemon"),
        "expected `cannot reach daemon`; got:\n{stderr}"
    );
}

/// AC #2 (401): `bowerbird export` with a wrong bearer token returns the
/// bearer-rejected stderr and exits non-zero. Parity with the matching
/// `replay_fails_with_401_when_token_wrong` test.
#[test]
fn export_fails_with_401_when_token_wrong() {
    let tmp = TempDir::new().expect("tempdir");
    start_daemon(&tmp);

    let assertion = bowerbird_cmd_in(&tmp)
        .env("BOWERBIRD_TOKEN", "wrong-token")
        .arg("export")
        .arg("any-session-id")
        .assert();
    let assertion = assertion.failure();
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr).into_owned();
    if !stderr.contains("daemon rejected bearer token") {
        stop_daemon(&tmp);
        panic!("expected bearer-rejected message; got:\n{stderr}");
    }

    stop_daemon(&tmp);
}

/// Story Task 4.5: `bowerbird export -o <path>` opens the output with
/// `create(true).truncate(true)`, so a re-export overwrites rather than
/// appending. Pre-seed the output file with sentinel bytes that would NOT
/// parse as JSONL and assert they are gone after the export.
#[test]
fn export_overwrites_existing_output_file() {
    let tmp = TempDir::new().expect("tempdir");
    start_daemon(&tmp);
    replay_bundled_fixture(&tmp);
    std::thread::sleep(Duration::from_millis(500));

    let out_path = tmp.path().join("overwrite-me.jsonl");
    // Pre-seed with content that would clearly break parsing if appended-to
    // rather than overwritten.
    std::fs::write(&out_path, "STALE GARBAGE LINE THAT IS NOT JSONL\n").expect("pre-seed");

    let assertion = bowerbird_cmd_in(&tmp)
        .arg("export")
        .arg("session-alpha")
        .arg("-o")
        .arg(&out_path)
        .assert();
    let _ = assertion.success();

    let body = std::fs::read_to_string(&out_path).expect("read output file");
    if body.contains("STALE GARBAGE") {
        stop_daemon(&tmp);
        panic!(
            "export did not truncate the existing file; \
             stale content still present:\n{body}"
        );
    }

    // Every non-empty line should parse as a protocol::Event.
    for line in body.lines() {
        if line.trim().is_empty() {
            continue;
        }
        if serde_json::from_str::<Event>(line).is_err() {
            stop_daemon(&tmp);
            panic!("post-overwrite line did not parse as Event:\n{line}");
        }
    }

    stop_daemon(&tmp);
}

/// Round-trip: export's output IS replay's input. Replay the bundled
/// fixture → export session-alpha → re-replay the exported file → re-export
/// → the second export should have ≥ as many events as the first (the
/// second replay duplicates the events; the events row is append-only).
///
/// The strong invariant the AC requires is shape compatibility: the export
/// output parses as `Event` records and the replay accepts it. The
/// "differ only by reassigned event_id + created_at" check is structural
/// (we compare the `kind`/`payload`/`session_id`/`source` tuples).
#[test]
fn export_round_trips_through_replay() {
    let tmp = TempDir::new().expect("tempdir");
    start_daemon(&tmp);

    // 1. Prime with bundled fixture.
    replay_bundled_fixture(&tmp);
    std::thread::sleep(Duration::from_millis(500));

    // 2. Export session-alpha.
    let export_path = tmp.path().join("round-trip-alpha.jsonl");
    bowerbird_cmd_in(&tmp)
        .arg("export")
        .arg("session-alpha")
        .arg("-o")
        .arg(&export_path)
        .assert()
        .success();
    let first_body = std::fs::read_to_string(&export_path).expect("read exported jsonl");
    let first_events: Vec<Event> = first_body
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| {
            serde_json::from_str::<Event>(l)
                .unwrap_or_else(|e| panic!("first export line did not parse: {e}; line: {l}"))
        })
        .collect();
    assert!(
        !first_events.is_empty(),
        "expected non-empty first export from session-alpha"
    );
    let first_shapes: Vec<(String, String, String)> = first_events
        .iter()
        .map(|e| (e.source.clone(), e.session_id.clone(), e.payload.clone()))
        .collect();

    // 3. Replay the exported file (events get fresh event_ids).
    bowerbird_cmd_in(&tmp)
        .arg("replay")
        .arg(&export_path)
        .assert()
        .success();
    std::thread::sleep(Duration::from_millis(500));

    // 4. Re-export. The export should now contain ≥ first_events.len()
    //    events (the original primed events plus the just-replayed copies).
    let second_path = tmp.path().join("round-trip-alpha-2.jsonl");
    bowerbird_cmd_in(&tmp)
        .arg("export")
        .arg("session-alpha")
        .arg("-o")
        .arg(&second_path)
        .assert()
        .success();
    let second_body = std::fs::read_to_string(&second_path).expect("read second export");
    let second_events: Vec<Event> = second_body
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str::<Event>(l).expect("second export line"))
        .collect();
    if second_events.len() < first_events.len() * 2 {
        stop_daemon(&tmp);
        panic!(
            "second export {} < first {}*2; round-trip did not replay events",
            second_events.len(),
            first_events.len()
        );
    }

    // The first N shapes of the second export should match the first
    // export's shapes (chronological); the next N are the round-tripped
    // copies of those same shapes.
    let first_n_shapes: Vec<(String, String, String)> = second_events
        .iter()
        .take(first_events.len())
        .map(|e| (e.source.clone(), e.session_id.clone(), e.payload.clone()))
        .collect();
    assert_eq!(
        first_n_shapes, first_shapes,
        "second export's first N shapes should match the first export's shapes",
    );

    stop_daemon(&tmp);
}
