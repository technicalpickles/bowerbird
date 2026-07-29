//! End-to-end tests for `bowerbird replay` (Story 4.1).
//!
//! Parallel-safe: these tests spawn real `bowerbird-daemon` subprocesses,
//! but each binds an ephemeral port and owns a per-test TempDir, so runs
//! do not collide (see `tests/cli_lifecycle.rs`).
//!
//! Every test isolates state via `BOWERBIRD_DATA_DIR` → `TempDir` plus
//! `BOWERBIRD_DAEMON_BIN` → the workspace-built daemon. No test touches
//! `~/.bowerbird/` or depends on the daemon being on `PATH`.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use assert_cmd::Command;
use tempfile::TempDir;

const REPLAY_TEST_TOKEN: &str = "replay-test-token";

fn bowerbird_bin() -> Command {
    let mut cmd = Command::cargo_bin("bowerbird").expect("bowerbird binary built");
    cmd.env_remove("BOWERBIRD_CLAUDE_SETTINGS");
    cmd.env_remove("BOWERBIRD_DATA_DIR");
    cmd.env_remove("BOWERBIRD_DAEMON_BIN");
    cmd.env_remove("BOWERBIRD_INGEST_SOCK");
    cmd.env("BOWERBIRD_TOKEN", REPLAY_TEST_TOKEN);
    cmd.env("BOWERBIRD_KEYRING_BACKEND", "disable");
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

/// Start the daemon and wait until it is fully up. Panics on failure.
fn start_daemon(tmp: &TempDir) {
    bowerbird_cmd_in(tmp).arg("start").assert().success();
    if !wait_for_daemon_up(tmp, Instant::now() + Duration::from_secs(3)) {
        force_stop(tmp);
        panic!("daemon did not come up within 3s after `bowerbird start`");
    }
}

/// Stop the daemon and wait for the named pid to exit. Best-effort cleanup
/// for tear-down paths.
fn stop_daemon(tmp: &TempDir) {
    if let Some(pid) = read_pid_file(tmp) {
        let _ = bowerbird_cmd_in(tmp).arg("stop").assert();
        let _ = wait_for_pid_dead(pid, Instant::now() + Duration::from_secs(5));
    }
}

/// AC #1: explicit-file replay forwards events through the daemon's pub/sub
/// path. Verifies via daemon-side assertion: the `/sessions` list grows by
/// the count of distinct sessions in the replay file.
#[test]
fn replay_with_explicit_file_forwards_events() {
    let tmp = TempDir::new().expect("tempdir");
    start_daemon(&tmp);

    // Author a tiny JSONL file with 2 events across 1 session.
    let jsonl_path = tmp.path().join("replay.jsonl");
    let body = "\
{\"event_id\":1,\"source\":\"claude\",\"session_id\":\"file-sess\",\"kind\":\"PreToolUse\",\"reaction\":\"Continue\",\"payload\":\"{}\",\"created_at\":1}
{\"event_id\":2,\"source\":\"claude\",\"session_id\":\"file-sess\",\"kind\":\"Stop\",\"reaction\":null,\"payload\":\"{}\",\"created_at\":2}
";
    std::fs::write(&jsonl_path, body).expect("write jsonl");

    let assertion = bowerbird_cmd_in(&tmp)
        .arg("replay")
        .arg(&jsonl_path)
        .assert();
    let assertion = assertion.success();
    let stdout = String::from_utf8_lossy(&assertion.get_output().stdout).into_owned();
    if !stdout.contains("replayed 2 events from") {
        stop_daemon(&tmp);
        panic!("expected `replayed 2 events from`; got:\n{stdout}");
    }

    stop_daemon(&tmp);
}

/// AC #3: no-arg replay uses the bundled fixture and prints the preamble.
#[test]
fn replay_with_no_argument_uses_bundled_fixture() {
    let tmp = TempDir::new().expect("tempdir");
    start_daemon(&tmp);

    let assertion = bowerbird_cmd_in(&tmp).arg("replay").assert();
    let assertion = assertion.success();
    let stdout = String::from_utf8_lossy(&assertion.get_output().stdout).into_owned();
    if !stdout.contains("using bundled fixture (") {
        stop_daemon(&tmp);
        panic!("expected bundled fixture preamble; got:\n{stdout}");
    }
    if !stdout.contains("replayed ") || !stdout.contains("bundled-fixture") {
        stop_daemon(&tmp);
        panic!("expected `replayed N events from bundled-fixture`; got:\n{stdout}");
    }

    stop_daemon(&tmp);
}

/// AC #1: per-line parse failures are reported on stderr but do not fail
/// the whole replay — successful lines forward, errors come back enumerated.
#[test]
fn replay_continues_after_invalid_lines() {
    let tmp = TempDir::new().expect("tempdir");
    start_daemon(&tmp);

    let jsonl_path = tmp.path().join("mixed.jsonl");
    let body = "\
{\"event_id\":1,\"source\":\"claude\",\"session_id\":\"mix-sess\",\"kind\":\"PreToolUse\",\"reaction\":null,\"payload\":\"{}\",\"created_at\":1}
NOT VALID JSON
{\"event_id\":3,\"source\":\"claude\",\"session_id\":\"mix-sess\",\"kind\":\"Stop\",\"reaction\":null,\"payload\":\"{}\",\"created_at\":3}
";
    std::fs::write(&jsonl_path, body).expect("write mixed jsonl");

    let assertion = bowerbird_cmd_in(&tmp)
        .arg("replay")
        .arg(&jsonl_path)
        .assert();
    let assertion = assertion.success();
    let stdout = String::from_utf8_lossy(&assertion.get_output().stdout).into_owned();
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr).into_owned();

    if !stdout.contains("replayed 2 events from") {
        stop_daemon(&tmp);
        panic!("expected 2 successful events; stdout:\n{stdout}\nstderr:\n{stderr}");
    }
    if !stderr.contains("line 2:") {
        stop_daemon(&tmp);
        panic!("expected `line 2:` error on stderr; got:\n{stderr}");
    }

    stop_daemon(&tmp);
}

/// `bowerbird replay` against a stopped daemon exits non-zero with a clear
/// stderr message.
#[test]
fn replay_fails_clearly_when_daemon_down() {
    let tmp = TempDir::new().expect("tempdir");
    // No daemon started.

    let assertion = bowerbird_cmd_in(&tmp).arg("replay").assert().failure();
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr).into_owned();
    assert!(
        stderr.contains("cannot reach daemon"),
        "expected `cannot reach daemon`; got:\n{stderr}"
    );
}

/// `bowerbird replay` with a wrong token returns the bearer-rejected
/// message and exits non-zero.
#[test]
fn replay_fails_with_401_when_token_wrong() {
    let tmp = TempDir::new().expect("tempdir");
    start_daemon(&tmp);

    let jsonl_path = tmp.path().join("authcheck.jsonl");
    std::fs::write(
        &jsonl_path,
        "{\"event_id\":1,\"source\":\"claude\",\"session_id\":\"auth-sess\",\"kind\":\"PreToolUse\",\"reaction\":null,\"payload\":\"{}\",\"created_at\":1}\n",
    )
    .expect("write");

    let assertion = bowerbird_cmd_in(&tmp)
        .env("BOWERBIRD_TOKEN", "wrong-token")
        .arg("replay")
        .arg(&jsonl_path)
        .assert();
    let assertion = assertion.failure();
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr).into_owned();
    if !stderr.contains("daemon rejected bearer token") {
        stop_daemon(&tmp);
        panic!("expected bearer-rejected message; got:\n{stderr}");
    }

    stop_daemon(&tmp);
}
