//! End-to-end tests for `bowerbird start` / `stop` / `status` (Story 3.2).
//!
//! Run under `--test-threads=1` (workspace default for daemon contract tests;
//! see `project-context.md` and `crates/daemon/tests/contract_daemon.rs`).
//! These tests spawn real `bowerbird-daemon` subprocesses and share TCP-port
//! and signal-handler state; running them in parallel produces flakes.
//!
//! Every test isolates state via `BOWERBIRD_DATA_DIR` pointing at a `TempDir`
//! plus `BOWERBIRD_DAEMON_BIN` pointing at the workspace-built daemon. This
//! ensures the lifecycle tests cannot touch the developer's real
//! `~/.bowerbird/` and do not depend on the daemon being on `PATH`.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use assert_cmd::Command;
use tempfile::TempDir;

fn bowerbird_bin() -> Command {
    let mut cmd = Command::cargo_bin("bowerbird").expect("bowerbird binary built");
    cmd.env_remove("BOWERBIRD_CLAUDE_SETTINGS");
    cmd.env_remove("BOWERBIRD_DATA_DIR");
    cmd.env_remove("BOWERBIRD_DAEMON_BIN");
    cmd.env_remove("BOWERBIRD_INGEST_SOCK");
    cmd.env_remove("BOWERBIRD_TOKEN");
    cmd
}

fn data_dir(tmp: &TempDir) -> PathBuf {
    tmp.path().join(".bowerbird")
}

fn daemon_bin() -> PathBuf {
    assert_cmd::cargo::cargo_bin("bowerbird-daemon")
}

/// Configure a `bowerbird` invocation with the standard test-isolation env:
/// HOME → tmp, BOWERBIRD_DATA_DIR → tmp/.bowerbird, BOWERBIRD_DAEMON_BIN →
/// the workspace-built daemon binary.
fn bowerbird_cmd_in(tmp: &TempDir) -> Command {
    let mut cmd = bowerbird_bin();
    cmd.env("HOME", tmp.path())
        .env("BOWERBIRD_DATA_DIR", data_dir(tmp))
        .env("BOWERBIRD_DAEMON_BIN", daemon_bin());
    cmd
}

/// Read the singleton PID file content. Returns `None` if absent or if the
/// content is not a positive integer.
fn read_pid_file(tmp: &TempDir) -> Option<i32> {
    let path = data_dir(tmp).join("bowerbird.pid");
    let s = std::fs::read_to_string(&path).ok()?;
    s.trim().parse::<i32>().ok().filter(|p| *p > 0)
}

/// Poll the ingest socket until it accepts connections, up to `deadline`.
/// Returns true if the socket came up; false on timeout.
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

/// Poll until the named pid is no longer alive (via `kill(pid, 0)`), up to
/// `deadline`. Returns true if the process exited; false on timeout.
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

/// Cleanup helper: best-effort SIGTERM the daemon out if a test leaves one
/// running. Called from test fail paths so a panic doesn't leak a daemon into
/// the runner's process tree.
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

/// AC #3 (stopped path): with no PID file and no daemon, `bowerbird status`
/// reports `stopped` and exits 0.
#[test]
fn status_when_no_pid_file_reports_stopped() {
    let tmp = TempDir::new().expect("tempdir");

    let assertion = bowerbird_cmd_in(&tmp).arg("status").assert().success();
    let stdout = String::from_utf8_lossy(&assertion.get_output().stdout).into_owned();
    assert!(
        stdout.contains("stopped"),
        "expected `stopped` in stdout; got:\n{stdout}"
    );
}

/// AC #1 + AC #2 + AC #3: full round-trip. Start the daemon, observe it via
/// status, stop it, observe the process is gone.
#[test]
fn start_then_status_then_stop_round_trip() {
    let tmp = TempDir::new().expect("tempdir");

    // start.
    let assertion = bowerbird_cmd_in(&tmp).arg("start").assert().success();
    let stdout = String::from_utf8_lossy(&assertion.get_output().stdout).into_owned();
    assert!(
        stdout.contains("started bowerbird-daemon") || stdout.contains("daemon ready"),
        "expected start confirmation; got:\n{stdout}"
    );

    // Confirm the daemon is fully up (ingest socket accepts connections).
    if !wait_for_daemon_up(&tmp, Instant::now() + Duration::from_secs(3)) {
        force_stop(&tmp);
        panic!("daemon did not come up within 3s after `bowerbird start`");
    }

    let pid = match read_pid_file(&tmp) {
        Some(p) => p,
        None => {
            force_stop(&tmp);
            panic!("expected PID file at .bowerbird/bowerbird.pid");
        }
    };

    // status.
    let assertion = bowerbird_cmd_in(&tmp).arg("status").assert().success();
    let stdout = String::from_utf8_lossy(&assertion.get_output().stdout).into_owned();
    assert!(
        stdout.contains("running"),
        "expected `running` in status stdout; got:\n{stdout}"
    );
    let pid_marker = format!("{pid}");
    assert!(
        stdout.contains(&pid_marker),
        "expected pid {pid} in status stdout; got:\n{stdout}"
    );

    // stop.
    let assertion = bowerbird_cmd_in(&tmp).arg("stop").assert().success();
    let stdout = String::from_utf8_lossy(&assertion.get_output().stdout).into_owned();
    assert!(
        stdout.contains("daemon stopped"),
        "expected `daemon stopped`; got:\n{stdout}"
    );

    // The named pid must no longer be alive within a reasonable drain window.
    if !wait_for_pid_dead(pid, Instant::now() + Duration::from_secs(5)) {
        force_stop(&tmp);
        panic!("pid {pid} still alive 5s after `bowerbird stop`");
    }
}

/// `bowerbird start` must be idempotent: running it twice does not spawn a
/// second daemon and the second invocation prints an `already running` hint.
#[test]
fn start_when_already_running_is_idempotent() {
    let tmp = TempDir::new().expect("tempdir");

    bowerbird_cmd_in(&tmp).arg("start").assert().success();
    if !wait_for_daemon_up(&tmp, Instant::now() + Duration::from_secs(3)) {
        force_stop(&tmp);
        panic!("first start did not come up");
    }
    let first_pid = read_pid_file(&tmp).expect("pid after first start");

    let assertion = bowerbird_cmd_in(&tmp).arg("start").assert().success();
    let stdout = String::from_utf8_lossy(&assertion.get_output().stdout).into_owned();
    assert!(
        stdout.contains("already running"),
        "expected `already running`; got:\n{stdout}"
    );

    let second_pid = read_pid_file(&tmp).expect("pid after second start");
    assert_eq!(
        first_pid, second_pid,
        "second start must not spawn a new daemon"
    );

    // Cleanup.
    bowerbird_cmd_in(&tmp).arg("stop").assert().success();
    let _ = wait_for_pid_dead(first_pid, Instant::now() + Duration::from_secs(5));
}

/// AC #2 (no-daemon path): `bowerbird stop` against an empty data dir is a
/// clean no-op. The string is contracted with `tests/cli_install.rs`'s
/// uninstall path; do NOT change without updating both.
#[test]
fn stop_when_not_running_is_a_clean_noop() {
    let tmp = TempDir::new().expect("tempdir");

    let assertion = bowerbird_cmd_in(&tmp).arg("stop").assert().success();
    let stdout = String::from_utf8_lossy(&assertion.get_output().stdout).into_owned();
    assert_eq!(
        stdout.trim(),
        "daemon not running (no pid file); nothing to stop",
        "exact wording is contracted; got:\n{stdout}"
    );
}

/// AC #4: a stale PID file (process named is dead) does not block a fresh
/// `bowerbird start`. Mirrors the Story 3.1 singleton stale-PID test pattern.
#[test]
fn start_recovers_from_stale_pid_file() {
    let tmp = TempDir::new().expect("tempdir");

    // Spawn-and-reap a child to obtain a known-dead PID. Writing a hardcoded
    // PID like 99999 risks colliding with a real process on busy systems.
    let child = std::process::Command::new("true")
        .spawn()
        .expect("spawn true");
    let dead_pid = child.id();
    let out = child.wait_with_output().expect("reap");
    assert!(out.status.success());

    std::fs::create_dir_all(data_dir(&tmp)).expect("mkdir data dir");
    std::fs::write(
        data_dir(&tmp).join("bowerbird.pid"),
        format!("{dead_pid}\n"),
    )
    .expect("write stale pid");

    bowerbird_cmd_in(&tmp).arg("start").assert().success();
    if !wait_for_daemon_up(&tmp, Instant::now() + Duration::from_secs(3)) {
        force_stop(&tmp);
        panic!("daemon did not come up after stale-pid recovery");
    }

    let new_pid = read_pid_file(&tmp).expect("pid after start");
    assert_ne!(
        i32::try_from(dead_pid).unwrap(),
        new_pid,
        "stale pid must be overwritten with the new daemon's pid"
    );

    bowerbird_cmd_in(&tmp).arg("stop").assert().success();
    let _ = wait_for_pid_dead(new_pid, Instant::now() + Duration::from_secs(5));
}

/// CLI surface check: `--help` mentions all five lifecycle-relevant
/// subcommands (`install`, `start`, `status`, `stop`, `uninstall`). Catches
/// regressions where a clap typo silently drops a subcommand from the dispatch
/// table.
#[test]
fn help_lists_all_lifecycle_subcommands() {
    let mut cmd = bowerbird_bin();
    let assertion = cmd.arg("--help").assert().success();
    let stdout = String::from_utf8_lossy(&assertion.get_output().stdout).into_owned();
    for name in ["install", "start", "status", "stop", "uninstall"] {
        assert!(
            stdout.contains(name),
            "help missing `{name}`:\n{stdout}"
        );
    }
}

/// AC #3 + AC #6 (CLI rendering): when `$BOWERBIRD_TOKEN` is shared between the
/// daemon and the CLI, `bowerbird status` must render the full status block —
/// version, protocol, uptime, `connected ws  : N`, and last-event row — not
/// the token-unset degraded fallback. Without this, the `connected ws  : 0`
/// formatter in `status::print_full_status` has zero E2E coverage; the contract
/// test in `crates/daemon/tests/contract_daemon.rs::story_3_2_lifecycle` proves
/// the JSON shape, but only this test proves the CLI surfaces it to the user.
#[test]
fn status_with_shared_token_renders_full_block_including_ws_clients() {
    let tmp = TempDir::new().expect("tempdir");
    const TOKEN: &str = "bb-test-token-3-2";

    // Start the daemon. The CLI's `spawn_detached` inherits this env, so the
    // daemon picks up the same token via its `load_or_generate` path.
    bowerbird_cmd_in(&tmp)
        .env("BOWERBIRD_TOKEN", TOKEN)
        .arg("start")
        .assert()
        .success();
    if !wait_for_daemon_up(&tmp, Instant::now() + Duration::from_secs(3)) {
        force_stop(&tmp);
        panic!("daemon did not come up within 3s after `bowerbird start`");
    }
    let pid = match read_pid_file(&tmp) {
        Some(p) => p,
        None => {
            force_stop(&tmp);
            panic!("expected PID file at .bowerbird/bowerbird.pid");
        }
    };

    let assertion = bowerbird_cmd_in(&tmp)
        .env("BOWERBIRD_TOKEN", TOKEN)
        .arg("status")
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&assertion.get_output().stdout).into_owned();

    // Cleanup before any panic so a failed assertion doesn't leak a daemon.
    let cleanup = || {
        let _ = bowerbird_cmd_in(&tmp)
            .env("BOWERBIRD_TOKEN", TOKEN)
            .arg("stop")
            .assert();
        let _ = wait_for_pid_dead(pid, Instant::now() + Duration::from_secs(5));
    };

    // The token-unset hint must NOT appear — if it does, the daemon and CLI
    // disagreed on the token (regression in env inheritance or token plumbing).
    if stdout.contains("$BOWERBIRD_TOKEN") {
        cleanup();
        panic!("CLI hit the token-unset degraded path; stdout:\n{stdout}");
    }

    // Each labeled row must be present. The labels are user-facing contract.
    for label in [
        "status        : running",
        "pid           :",
        "version       :",
        "protocol      :",
        "uptime        :",
        "connected ws  : 0",
        "last event    :",
    ] {
        if !stdout.contains(label) {
            cleanup();
            panic!("status block missing `{label}`; got:\n{stdout}");
        }
    }

    cleanup();
}

/// AC #3 (stopped path, stale-PID variant): when `bowerbird.pid` exists but
/// the named PID is dead (e.g., the previous daemon crashed and the kernel
/// reaped it without unlinking the file), `bowerbird status` reports
/// `stopped (stale pid file: pid X is not running)` and exits 0. Covers the
/// `print_stopped(Some(...))` branch in `src/commands/status.rs:42-45`, which
/// had no test before.
#[test]
fn status_with_stale_pid_file_reports_stopped_stale() {
    let tmp = TempDir::new().expect("tempdir");

    // Spawn-and-reap to get a known-dead PID, same trick used by
    // `start_recovers_from_stale_pid_file`.
    let child = std::process::Command::new("true")
        .spawn()
        .expect("spawn true");
    let dead_pid = child.id();
    let out = child.wait_with_output().expect("reap");
    assert!(out.status.success());

    std::fs::create_dir_all(data_dir(&tmp)).expect("mkdir data dir");
    std::fs::write(
        data_dir(&tmp).join("bowerbird.pid"),
        format!("{dead_pid}\n"),
    )
    .expect("write stale pid");

    let assertion = bowerbird_cmd_in(&tmp).arg("status").assert().success();
    let stdout = String::from_utf8_lossy(&assertion.get_output().stdout).into_owned();
    assert!(
        stdout.contains("stopped") && stdout.contains("stale pid"),
        "expected `stopped (stale pid file: ...)`; got:\n{stdout}"
    );
    let dead_marker = format!("{dead_pid}");
    assert!(
        stdout.contains(&dead_marker),
        "expected stale pid {dead_pid} in stdout; got:\n{stdout}"
    );
}
