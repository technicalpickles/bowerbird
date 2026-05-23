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
}

// Silence the unused UnixStream import warning since it appears only in
// helper signatures via Drop / try_clone.
#[allow(dead_code)]
fn _suppress_unused(_: UnixStream) {}
