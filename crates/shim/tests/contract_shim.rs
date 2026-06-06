use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use assert_cmd::Command;
use tempfile::TempDir;

// ─── Helpers ─────────────────────────────────────────────────────────────────

struct MockIngest {
    sock_path: PathBuf,
    captured: Arc<Mutex<Vec<u8>>>,
    stop: Arc<AtomicBool>,
}

fn start_mock_ingest(tmp: &TempDir, response: &'static [u8]) -> MockIngest {
    let sock_path = tmp.path().join("ingest.sock");
    let listener = UnixListener::bind(&sock_path).expect("bind");
    listener.set_nonblocking(true).expect("set_nonblocking");

    let captured: Arc<Mutex<Vec<u8>>> = Arc::new(Mutex::new(Vec::new()));
    let captured_clone = captured.clone();
    let stop = Arc::new(AtomicBool::new(false));
    let stop_clone = stop.clone();

    thread::spawn(move || {
        while !stop_clone.load(Ordering::Relaxed) {
            match listener.accept() {
                Ok((stream, _)) => {
                    let _ = stream.set_nonblocking(false);
                    let mut reader = BufReader::new(stream.try_clone().expect("clone"));
                    let mut line = Vec::new();
                    let _ = reader.read_until(b'\n', &mut line);
                    {
                        let mut g = captured_clone.lock().expect("lock");
                        g.extend_from_slice(&line);
                    }
                    let mut writer = stream;
                    let _ = writer.write_all(response);
                    let _ = writer.flush();
                }
                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_micros(200));
                }
                Err(_) => break,
            }
        }
    });

    thread::sleep(Duration::from_millis(10));

    MockIngest {
        sock_path,
        captured,
        stop,
    }
}

impl Drop for MockIngest {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

fn run_shim_with_env(
    sock: &Path,
    log: &Path,
    hook_kind: &str,
    stdin: &[u8],
    extra_home: Option<&Path>,
) -> std::process::Output {
    let mut cmd = Command::cargo_bin("bowerbird-shim").expect("cargo_bin");
    cmd.arg("--hook-kind")
        .arg(hook_kind)
        .env("BOWERBIRD_INGEST_SOCK", sock)
        .env("BOWERBIRD_SHIM_LOG", log)
        .write_stdin(stdin.to_vec());
    if let Some(home) = extra_home {
        cmd.env("HOME", home);
    }
    cmd.output().expect("shim spawn")
}

fn wait_for_capture(mock: &MockIngest) -> Vec<u8> {
    let start = Instant::now();
    loop {
        {
            let g = mock.captured.lock().expect("lock");
            if !g.is_empty() {
                return g.clone();
            }
        }
        if start.elapsed() > Duration::from_secs(2) {
            panic!("timeout waiting for mock to capture request");
        }
        thread::sleep(Duration::from_millis(5));
    }
}

fn parse_captured_payload(captured: &[u8]) -> serde_json::Value {
    let trimmed = captured.split(|b| *b == b'\n').next().expect("had data");
    serde_json::from_slice(trimmed).expect("captured bytes are JSON")
}

// ─── AC #2: exit 0 + silent on 200 ───────────────────────────────────────────

#[test]
fn shim_exit_0_on_200() {
    let tmp = TempDir::new().expect("tempdir");
    let log_tmp = TempDir::new().expect("log tmpdir");
    let log = log_tmp.path().join("shim.log");
    let mock = start_mock_ingest(&tmp, b"200\n");

    let out = run_shim_with_env(
        &mock.sock_path,
        &log,
        "PreToolUse",
        br#"{"session_id":"s1","tool_name":"Bash"}"#,
        None,
    );
    assert_eq!(
        out.status.code(),
        Some(0),
        "stderr: {:?}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        out.stdout.is_empty(),
        "stdout must be empty on success, got: {:?}",
        String::from_utf8_lossy(&out.stdout)
    );
    assert!(
        out.stderr.is_empty(),
        "stderr must be empty on success, got: {:?}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn shim_silent_on_success() {
    let tmp = TempDir::new().expect("tempdir");
    let log_tmp = TempDir::new().expect("log tmpdir");
    let log = log_tmp.path().join("shim.log");
    let mock = start_mock_ingest(&tmp, b"200\n");

    let out = run_shim_with_env(
        &mock.sock_path,
        &log,
        "PreToolUse",
        br#"{"session_id":"s1","tool_name":"Bash"}"#,
        None,
    );
    assert_eq!(out.status.code(), Some(0));

    assert!(
        !log.exists(),
        "log file must NOT be created on success path, but exists at {}",
        log.display()
    );
}

// ─── AC #3: connection refused → exit non-zero, error log ────────────────────

#[test]
fn shim_exit_nonzero_on_connection_refused() {
    let tmp = TempDir::new().expect("tempdir");
    let log_tmp = TempDir::new().expect("log tmpdir");
    let log = log_tmp.path().join("shim.log");
    let bogus_sock = tmp.path().join("nonexistent.sock");

    let out = run_shim_with_env(
        &bogus_sock,
        &log,
        "PreToolUse",
        br#"{"session_id":"s1","tool_name":"Bash"}"#,
        None,
    );

    let code = out.status.code().expect("exited cleanly");
    assert_ne!(code, 0, "expected non-zero exit on connect failure");
    assert_ne!(code, 2, "exit code 2 is forbidden");

    let log_contents = std::fs::read_to_string(&log).expect("log should exist");
    assert!(log_contents.contains("ERROR"), "log: {log_contents:?}");
    assert_eq!(
        log_contents.matches('\n').count(),
        1,
        "expected exactly one log line, got: {log_contents:?}"
    );
    let sock_str = bogus_sock.to_string_lossy().into_owned();
    assert!(
        log_contents.contains(&sock_str),
        "log line must include the socket path that failed to connect, got: {log_contents:?}"
    );

    // Story 5.10 AC1/AC2: the exit-1 daemon-down path now NAMES its cause on
    // stderr instead of leaving Claude with a causeless "No stderr output".
    // AC1 requires EXACTLY one line of a fixed shape — assert it verbatim so a
    // regression that prints a duplicate line, drops "event dropped", or
    // rewords the cause cannot slip through (the temp log path here has no
    // control chars, so the sanitized pointer equals `log.display()`).
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(
        stderr,
        format!(
            "bowerbird: daemon not running, event dropped (see {})\n",
            log.display()
        ),
        "exit-1 connect failure must emit exactly the AC1 stderr line"
    );
}

// Story 5.10 review F2: when the log append itself fails (here the log path is
// an existing directory, so open(2) returns EISDIR), the stderr hint must NOT
// append `(see <log>)` — Claude would be pointed at a file that was never
// written. The cause is still named; the pointer is dropped.
#[test]
fn shim_omits_log_pointer_when_log_append_fails() {
    let tmp = TempDir::new().expect("tempdir");
    // Point BOWERBIRD_SHIM_LOG at a directory: log::append's open(2) fails.
    let log_dir = TempDir::new().expect("log dir");
    let log_as_dir = log_dir.path();
    let bogus_sock = tmp.path().join("nonexistent.sock");

    let out = run_shim_with_env(
        &bogus_sock,
        log_as_dir,
        "PreToolUse",
        br#"{"session_id":"s1","tool_name":"Bash"}"#,
        None,
    );

    let code = out.status.code().expect("exited cleanly");
    assert_eq!(code, 1, "connect failure with unusable log still exits 1");
    assert_ne!(code, 2, "exit code 2 is forbidden");

    // Pointer-less: names the cause, no `(see ...)` because the log was unwritable.
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(
        stderr, "bowerbird: daemon not running, event dropped\n",
        "stderr must name the cause without a (see <log>) pointer when the log append failed"
    );
}

// Story 5.10 review F3 (AC4 coverage): the pre-run log-path-resolution failure
// branch (no HOME, no BOWERBIRD_SHIM_LOG) must emit exactly the pointer-less
// cause line and exit 1. The standard helper always sets BOWERBIRD_SHIM_LOG, so
// this test removes both vars to actually reach that branch.
#[test]
fn shim_names_cause_when_no_home_and_no_log_path() {
    let mut cmd = Command::cargo_bin("bowerbird-shim").expect("cargo_bin");
    cmd.arg("--hook-kind")
        .arg("PreToolUse")
        .env_remove("HOME")
        .env_remove("BOWERBIRD_SHIM_LOG")
        .write_stdin(br#"{"session_id":"s1","tool_name":"Bash"}"#.to_vec());
    let out = cmd.output().expect("shim spawn");

    let code = out.status.code().expect("exited cleanly");
    assert_eq!(code, 1, "no resolvable log path must exit 1");
    assert_ne!(code, 2, "exit code 2 is forbidden");
    assert!(
        out.stdout.is_empty(),
        "stdout must stay empty, got: {:?}",
        String::from_utf8_lossy(&out.stdout)
    );
    assert_eq!(
        String::from_utf8_lossy(&out.stderr),
        "bowerbird: HOME not set, cannot record event\n",
        "pre-run no-HOME branch must emit exactly the pointer-less cause line"
    );
}

// Story 5.10 review F5: a BOWERBIRD_SHIM_LOG path containing a newline (a legal
// byte in a unix filename) must NOT turn the one-line hook message into multiple
// lines — the path is escaped before it is embedded in stderr.
#[test]
fn shim_stderr_stays_one_line_with_newline_in_log_path() {
    let tmp = TempDir::new().expect("tempdir");
    let log_tmp = TempDir::new().expect("log tmpdir");
    // Newline inside the filename component; the parent dir exists so the append
    // succeeds and the pointer (sanitized) is emitted.
    let log = log_tmp.path().join("foo\nbar.log");
    let bogus_sock = tmp.path().join("nonexistent.sock");

    let out = run_shim_with_env(
        &bogus_sock,
        &log,
        "PreToolUse",
        br#"{"session_id":"s1","tool_name":"Bash"}"#,
        None,
    );

    let code = out.status.code().expect("exited cleanly");
    assert_eq!(code, 1, "connect failure exits 1");
    assert_ne!(code, 2, "exit code 2 is forbidden");

    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(
        stderr.matches('\n').count(),
        1,
        "stderr must be exactly one line (only the trailing newline), got: {stderr:?}"
    );
    assert!(
        stderr.starts_with("bowerbird: daemon not running, event dropped"),
        "stderr must still name the cause, got: {stderr:?}"
    );
    assert!(
        stderr.contains("\\n"),
        "the newline in the log path must be escaped to a literal backslash-n, got: {stderr:?}"
    );
}

// ─── AC #4: 503 → exit 0 with warning log ────────────────────────────────────

#[test]
fn shim_exit_0_on_503_with_warning_log() {
    let tmp = TempDir::new().expect("tempdir");
    let log_tmp = TempDir::new().expect("log tmpdir");
    let log = log_tmp.path().join("shim.log");
    let mock = start_mock_ingest(&tmp, b"503\n");

    let out = run_shim_with_env(
        &mock.sock_path,
        &log,
        "PreToolUse",
        br#"{"session_id":"s1","tool_name":"Bash"}"#,
        None,
    );
    assert_eq!(out.status.code(), Some(0));

    let log_contents = std::fs::read_to_string(&log).expect("log should exist");
    assert!(log_contents.contains("WARN"), "log: {log_contents:?}");
    assert_eq!(
        log_contents.matches('\n').count(),
        1,
        "expected exactly one WARN line, got: {log_contents:?}"
    );

    // Story 5.10 AC2 (NFR20 regression guard): the daemon is up and answering
    // (503 backpressure), so this is fire-and-forget — stderr must stay EMPTY.
    // The new exit-1 stderr voice must not leak into the exit-0 WARN class.
    assert!(
        out.stderr.is_empty(),
        "exit-0 (503 backpressure) path must leave stderr empty, got: {:?}",
        String::from_utf8_lossy(&out.stderr)
    );
}

// ─── AC #5: log file mode is 0600 regardless of umask ────────────────────────

fn assert_log_mode_0600_under_umask(umask: u32) {
    let tmp = TempDir::new().expect("tempdir");
    let log_tmp = TempDir::new().expect("log tmpdir");
    let log = log_tmp.path().join("shim.log");
    let bogus_sock = tmp.path().join("nonexistent.sock");

    let shim_bin = assert_cmd::cargo::cargo_bin("bowerbird-shim");
    let shim_path = shim_bin.to_string_lossy().into_owned();
    let sock_path_str = bogus_sock.to_string_lossy().into_owned();
    let log_path_str = log.to_string_lossy().into_owned();

    let umask_oct = format!("{umask:04o}");
    // sh: set umask then exec the shim so the child inherits the umask but is
    // NOT a shell child (clean argv[0]).
    let script = format!("umask {umask_oct} && exec \"{shim_path}\" --hook-kind PreToolUse",);

    let mut child = std::process::Command::new("sh")
        .arg("-c")
        .arg(&script)
        .env("BOWERBIRD_INGEST_SOCK", &sock_path_str)
        .env("BOWERBIRD_SHIM_LOG", &log_path_str)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("spawn sh");

    {
        let stdin = child.stdin.as_mut().expect("stdin");
        stdin
            .write_all(br#"{"session_id":"s1","tool_name":"Bash"}"#)
            .expect("write stdin");
    }

    let result = child.wait_with_output().expect("wait");
    assert!(
        !result.status.success(),
        "expected failure exit on connect-refused"
    );

    let meta = std::fs::metadata(&log).expect("log metadata");
    let mode = meta.permissions().mode() & 0o777;
    assert_eq!(
        mode, 0o600,
        "log file mode must be 0o600 under umask {umask:#o}, got {mode:#o}"
    );
}

#[test]
fn shim_log_mode_is_0600_with_permissive_umask() {
    assert_log_mode_0600_under_umask(0o022);
}

#[test]
fn shim_log_mode_is_0600_with_restrictive_umask() {
    assert_log_mode_0600_under_umask(0o077);
}

// ─── AC #6: shim source contains no async runtime ────────────────────────────

#[test]
fn shim_source_has_no_async() {
    let crate_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let src_dir = crate_root.join("src");

    let mut violations = Vec::new();
    walk_rs(&src_dir, &mut |path, contents| {
        for needle in &["tokio", "async fn", ".await"] {
            if contents.contains(needle) {
                violations.push(format!("{} contains '{needle}'", path.display()));
            }
        }
    });

    assert!(
        violations.is_empty(),
        "AC #6 violation — async tokens found in shim/src/:\n{}",
        violations.join("\n")
    );
}

fn walk_rs(dir: &Path, visit: &mut impl FnMut(&Path, &str)) {
    let read = match std::fs::read_dir(dir) {
        Ok(r) => r,
        Err(_) => return,
    };
    for entry in read.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk_rs(&path, visit);
        } else if path.extension().and_then(|s| s.to_str()) == Some("rs") {
            if let Ok(contents) = std::fs::read_to_string(&path) {
                visit(&path, &contents);
            }
        }
    }
}

// ─── Env var overrides actually take effect ──────────────────────────────────

#[test]
fn shim_respects_env_var_sock_path() {
    let sock_tmp = TempDir::new().expect("sock tmpdir");
    let log_tmp = TempDir::new().expect("log tmpdir");
    let log = log_tmp.path().join("shim.log");
    let mock = start_mock_ingest(&sock_tmp, b"200\n");
    assert!(!mock.sock_path.to_string_lossy().contains(".bowerbird"));

    let out = run_shim_with_env(
        &mock.sock_path,
        &log,
        "PreToolUse",
        br#"{"session_id":"s1","tool_name":"Bash"}"#,
        None,
    );
    assert_eq!(out.status.code(), Some(0));
}

#[test]
fn shim_respects_env_var_log_path() {
    let tmp = TempDir::new().expect("tempdir");
    let log_tmp = TempDir::new().expect("log tmpdir");
    let custom_log = log_tmp.path().join("custom-shim.log");
    let bogus_sock = tmp.path().join("nope.sock");
    let fake_home_tmp = TempDir::new().expect("fake home tmpdir");
    let fake_home = fake_home_tmp.path();

    let out = run_shim_with_env(
        &bogus_sock,
        &custom_log,
        "PreToolUse",
        br#"{"session_id":"s1","tool_name":"Bash"}"#,
        Some(fake_home),
    );
    assert!(!out.status.success());

    assert!(custom_log.exists(), "log should be at env-specified path");
    let home_log = fake_home.join(".bowerbird/shim.log");
    assert!(
        !home_log.exists(),
        "log should NOT be at default $HOME path when env override is set"
    );
}

// ─── Wire payload integrity ──────────────────────────────────────────────────

#[test]
fn shim_wire_payload_is_valid_ndjson() {
    let tmp = TempDir::new().expect("tempdir");
    let log_tmp = TempDir::new().expect("log tmpdir");
    let log = log_tmp.path().join("shim.log");
    let mock = start_mock_ingest(&tmp, b"200\n");

    let stdin = br#"{"session_id":"s1","tool_name":"Bash"}"#;
    let out = run_shim_with_env(&mock.sock_path, &log, "PreToolUse", stdin, None);
    assert_eq!(out.status.code(), Some(0));

    let captured = wait_for_capture(&mock);
    let lines: Vec<&[u8]> = captured
        .split(|b| *b == b'\n')
        .filter(|s| !s.is_empty())
        .collect();
    assert_eq!(lines.len(), 1, "expected one line, got {}", lines.len());

    let value = parse_captured_payload(&captured);
    let obj = value.as_object().expect("payload is a JSON object");
    assert_eq!(obj.get("session_id").and_then(|v| v.as_str()), Some("s1"));
    assert_eq!(obj.get("tool_name").and_then(|v| v.as_str()), Some("Bash"));
    assert_eq!(
        obj.get("hook_kind").and_then(|v| v.as_str()),
        Some("PreToolUse")
    );
}

#[test]
fn shim_injects_hook_kind() {
    let tmp = TempDir::new().expect("tempdir");
    let log_tmp = TempDir::new().expect("log tmpdir");
    let log = log_tmp.path().join("shim.log");
    let mock = start_mock_ingest(&tmp, b"200\n");

    let stdin = br#"{"session_id":"s1","tool_name":"Bash"}"#;
    let out = run_shim_with_env(&mock.sock_path, &log, "PreToolUse", stdin, None);
    assert_eq!(out.status.code(), Some(0));

    let captured = wait_for_capture(&mock);
    let value = parse_captured_payload(&captured);
    let obj = value.as_object().expect("object");
    assert_eq!(
        obj.get("hook_kind").and_then(|v| v.as_str()),
        Some("PreToolUse")
    );
    assert_eq!(obj.get("session_id").and_then(|v| v.as_str()), Some("s1"));
    assert_eq!(obj.get("tool_name").and_then(|v| v.as_str()), Some("Bash"));
}

#[test]
fn shim_accepts_user_prompt_submit_hook_kind() {
    // Story 5.2: `bowerbird install` writes a hook entry for
    // `bowerbird-shim --hook-kind UserPromptSubmit`. The shim's
    // `parse_hook_kind` must accept the new kind end-to-end, otherwise
    // the installed hook fails at the CLI parse boundary and the daemon
    // never sees the event.
    let tmp = TempDir::new().expect("tempdir");
    let log_tmp = TempDir::new().expect("log tmpdir");
    let log = log_tmp.path().join("shim.log");
    let mock = start_mock_ingest(&tmp, b"200\n");

    let stdin = br#"{"session_id":"sess-ups","prompt":"hello"}"#;
    let out = run_shim_with_env(&mock.sock_path, &log, "UserPromptSubmit", stdin, None);
    assert_eq!(
        out.status.code(),
        Some(0),
        "shim must exit 0; stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );

    let captured = wait_for_capture(&mock);
    let value = parse_captured_payload(&captured);
    let obj = value.as_object().expect("payload is a JSON object");
    assert_eq!(
        obj.get("hook_kind").and_then(|v| v.as_str()),
        Some("UserPromptSubmit"),
        "shim must inject hook_kind=UserPromptSubmit"
    );
    assert_eq!(
        obj.get("session_id").and_then(|v| v.as_str()),
        Some("sess-ups")
    );
}

// Story 5.3 AC #1: shim injects `bowerbird_ppid` (= libc::getppid()) so the
// daemon can probe Claude Code's PID for liveness.
#[test]
fn shim_injects_bowerbird_ppid_into_payload() {
    let tmp = TempDir::new().expect("tempdir");
    let log_tmp = TempDir::new().expect("log tmpdir");
    let log = log_tmp.path().join("shim.log");
    let mock = start_mock_ingest(&tmp, b"200\n");

    let stdin = br#"{"session_id":"s1","tool_name":"Bash"}"#;
    let out = run_shim_with_env(&mock.sock_path, &log, "PreToolUse", stdin, None);
    assert_eq!(out.status.code(), Some(0));

    let captured = wait_for_capture(&mock);
    let value = parse_captured_payload(&captured);
    let obj = value.as_object().expect("object");

    let ppid = obj
        .get("bowerbird_ppid")
        .and_then(|v| v.as_i64())
        .expect("bowerbird_ppid must be present and an integer");
    assert!(
        ppid > 0,
        "bowerbird_ppid must be a positive integer, got {ppid}"
    );
    // The shim's parent IS the test runner (assert_cmd::Command spawns the
    // shim as a direct child of the cargo test binary).
    let test_runner_pid = std::process::id() as i64;
    assert_eq!(
        ppid, test_runner_pid,
        "bowerbird_ppid should equal the test runner's PID (the shim's parent)"
    );
}

// Story 5.3: the shim must NOT extract or strip `notification_type` — that
// field is the adapter's concern. The shim preserves it verbatim in the
// payload.
#[test]
fn shim_preserves_notification_type_field_verbatim() {
    let tmp = TempDir::new().expect("tempdir");
    let log_tmp = TempDir::new().expect("log tmpdir");
    let log = log_tmp.path().join("shim.log");
    let mock = start_mock_ingest(&tmp, b"200\n");

    let stdin =
        br#"{"session_id":"sess-n","notification_type":"permission_prompt","message":"do?"}"#;
    let out = run_shim_with_env(&mock.sock_path, &log, "Notification", stdin, None);
    assert_eq!(out.status.code(), Some(0));

    let captured = wait_for_capture(&mock);
    let value = parse_captured_payload(&captured);
    let obj = value.as_object().expect("object");
    assert_eq!(
        obj.get("notification_type").and_then(|v| v.as_str()),
        Some("permission_prompt"),
        "notification_type must survive the shim verbatim"
    );
    assert_eq!(
        obj.get("hook_kind").and_then(|v| v.as_str()),
        Some("Notification")
    );
    assert!(
        obj.get("bowerbird_ppid").is_some(),
        "bowerbird_ppid must be injected for Notification hooks too"
    );
}

// Story 5.7: `cwd` is a NATIVE Claude Code hook field. The shim must NOT strip
// or rewrite it — it forwards the payload verbatim and the adapter reads `cwd`
// at normalize. This pins that the native field survives the shim untouched
// (no shim change was made for Story 5.7; this is the regression guard).
#[test]
fn shim_preserves_cwd_field_verbatim() {
    let tmp = TempDir::new().expect("tempdir");
    let log_tmp = TempDir::new().expect("log tmpdir");
    let log = log_tmp.path().join("shim.log");
    let mock = start_mock_ingest(&tmp, b"200\n");

    let stdin = br#"{"session_id":"sess-c","tool_name":"Bash","cwd":"/Users/x/repo"}"#;
    let out = run_shim_with_env(&mock.sock_path, &log, "PreToolUse", stdin, None);
    assert_eq!(out.status.code(), Some(0));

    let captured = wait_for_capture(&mock);
    let value = parse_captured_payload(&captured);
    let obj = value.as_object().expect("object");
    assert_eq!(
        obj.get("cwd").and_then(|v| v.as_str()),
        Some("/Users/x/repo"),
        "native cwd must survive the shim verbatim"
    );
    assert_eq!(
        obj.get("hook_kind").and_then(|v| v.as_str()),
        Some("PreToolUse")
    );
}

#[test]
fn shim_preserves_existing_hook_event_name_field() {
    let tmp = TempDir::new().expect("tempdir");
    let log_tmp = TempDir::new().expect("log tmpdir");
    let log = log_tmp.path().join("shim.log");
    let mock = start_mock_ingest(&tmp, b"200\n");

    let stdin = br#"{"session_id":"s1","tool_name":"Bash","hook_event_name":"PreToolUse"}"#;
    let out = run_shim_with_env(&mock.sock_path, &log, "PreToolUse", stdin, None);
    assert_eq!(out.status.code(), Some(0));

    let captured = wait_for_capture(&mock);
    let value = parse_captured_payload(&captured);
    let obj = value.as_object().expect("object");
    assert_eq!(
        obj.get("hook_event_name").and_then(|v| v.as_str()),
        Some("PreToolUse"),
        "hook_event_name must be preserved verbatim"
    );
    assert_eq!(
        obj.get("hook_kind").and_then(|v| v.as_str()),
        Some("PreToolUse"),
        "hook_kind must be injected by the shim"
    );
}

// ─── Stdin > 1 MiB cap → reject, do not silently truncate ────────────────────

#[test]
fn shim_rejects_stdin_over_1mib_cap() {
    let tmp = TempDir::new().expect("tempdir");
    let log_tmp = TempDir::new().expect("log tmpdir");
    let log = log_tmp.path().join("shim.log");
    let mock = start_mock_ingest(&tmp, b"200\n");

    // The smoking-gun construction for the silent-truncation hazard: the
    // first 1 MiB of stdin is a complete, valid JSON object. Anything past
    // the cap is extra bytes. Under the old `take(1 MiB).read_to_end()` flow
    // serde_json would happily parse the first 1 MiB and the shim would
    // deliver a partial event as if it were complete. The fix MUST detect
    // the overflow before parsing.
    const ONE_MIB: usize = 1 << 20;
    let prefix = r#"{"k":""#;
    let suffix = r#""}"#;
    let padding = "a".repeat(ONE_MIB - prefix.len() - suffix.len());
    let mut oversized = String::with_capacity(ONE_MIB + 16);
    oversized.push_str(prefix);
    oversized.push_str(&padding);
    oversized.push_str(suffix);
    assert_eq!(
        oversized.len(),
        ONE_MIB,
        "first 1 MiB must be exactly a valid JSON object"
    );
    // Append a trailing byte so the total exceeds the cap. Anything works;
    // the shim must reject without inspecting trailing content.
    oversized.push('X');
    assert!(oversized.len() > ONE_MIB);

    let out = run_shim_with_env(
        &mock.sock_path,
        &log,
        "PreToolUse",
        oversized.as_bytes(),
        None,
    );

    let code = out.status.code().expect("exited cleanly");
    assert_ne!(code, 0, "oversized stdin must fail (got exit {code})");
    assert_ne!(code, 2, "exit code 2 is forbidden");

    let log_contents = std::fs::read_to_string(&log).expect("log should exist");
    assert!(
        log_contents.contains("ERROR"),
        "oversized stdin must log ERROR: {log_contents:?}"
    );
    assert_eq!(
        log_contents.matches('\n').count(),
        1,
        "expected exactly one ERROR line, got: {log_contents:?}"
    );
    // The log line must reference the cap so a triaging operator can tell
    // this from generic "stdin parse failed" noise.
    assert!(
        log_contents.to_lowercase().contains("too large"),
        "oversized stdin log should describe the size cap, got: {log_contents:?}"
    );

    // Sleep briefly to give the mock listener a chance to capture if the shim
    // (incorrectly) sent anything before bailing.
    thread::sleep(Duration::from_millis(50));
    let captured_len = mock.captured.lock().expect("lock").len();
    assert_eq!(
        captured_len, 0,
        "no bytes should reach the daemon when stdin is over the cap, got {captured_len} bytes"
    );
}

// ─── 400 from daemon → fire-and-forget warning log ───────────────────────────

#[test]
fn shim_exit_0_on_400_from_daemon() {
    let tmp = TempDir::new().expect("tempdir");
    let log_tmp = TempDir::new().expect("log tmpdir");
    let log = log_tmp.path().join("shim.log");
    let mock = start_mock_ingest(&tmp, b"400 invalid JSON: something\n");

    let out = run_shim_with_env(
        &mock.sock_path,
        &log,
        "PreToolUse",
        br#"{"session_id":"s1","tool_name":"Bash"}"#,
        None,
    );
    assert_eq!(out.status.code(), Some(0));

    let log_contents = std::fs::read_to_string(&log).expect("log");
    assert!(log_contents.contains("WARN"), "log: {log_contents:?}");
    assert_eq!(
        log_contents.matches('\n').count(),
        1,
        "expected exactly one WARN line, got: {log_contents:?}"
    );

    // Story 5.10 AC2 (NFR20 regression guard): a daemon-answered 400 is
    // fire-and-forget (exit 0) — stderr must stay EMPTY, no exit-1 leakage.
    assert!(
        out.stderr.is_empty(),
        "exit-0 (daemon 400) path must leave stderr empty, got: {:?}",
        String::from_utf8_lossy(&out.stderr)
    );
}

// Silence the unused UnixStream import warning since it appears only in
// helper signatures via Drop / try_clone.
#[allow(dead_code)]
fn _suppress_unused(_: UnixStream) {}
