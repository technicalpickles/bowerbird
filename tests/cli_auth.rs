//! End-to-end tests for `bowerbird auth token` (Story 3.3).
//!
//! Parallel-safe: these tests spawn real `bowerbird` and `bowerbird-daemon`
//! subprocesses, but every env var is passed per-child via `assert_cmd` /
//! `Command::env` (never `std::env::set_var`) and all state lives under a
//! per-test TempDir.
//!
//! **Keychain isolation discipline:** every test sets
//! `BOWERBIRD_KEYRING_BACKEND` to `disable` or `mock` explicitly. A test
//! without this env IS A BUG — it would touch the developer's real macOS
//! Keychain or Linux Secret Service.
//!
//! **Cross-process mock sharing:** keyring's `mock` backend is per-process
//! and in-memory. Tests that need to verify daemon→CLI round-trip use
//! `disable` + a shared `BOWERBIRD_TOKEN` env (see `tests/cli_lifecycle.rs`
//! for the established pattern). In-process tests of the mock-write/read
//! cycle live in `crates/daemon/tests/contract_daemon.rs::story_3_3_auth`.

use std::os::unix::fs::OpenOptionsExt;
use std::path::PathBuf;

use assert_cmd::Command;
use tempfile::TempDir;

fn data_dir(tmp: &TempDir) -> PathBuf {
    tmp.path().join(".bowerbird")
}

/// taskwarrior 2e9cfda3: an isolated label so `stop`/`start` launchd probes
/// address a service that never exists, instead of the developer's real
/// agent. Real `launchctl print` on this label exits 113 ("Could not find"),
/// which the CLI classifies NotLoaded, falling to the pid-file path.
const TEST_LAUNCH_AGENT_LABEL: &str = "com.technicalpickles.bowerbird.test-isolation";

/// Build a `bowerbird` CLI invocation with the standard test-isolation env.
/// Defaults to `BOWERBIRD_KEYRING_BACKEND=disable` so the test never reaches
/// the developer's real keychain; tests override per-invocation as needed.
fn bowerbird_auth_command(tmp: &TempDir) -> Command {
    let mut cmd = Command::cargo_bin("bowerbird").expect("bowerbird binary built");
    cmd.env_remove("BOWERBIRD_CLAUDE_SETTINGS");
    cmd.env_remove("BOWERBIRD_DAEMON_BIN");
    cmd.env_remove("BOWERBIRD_INGEST_SOCK");
    cmd.env_remove("BOWERBIRD_TOKEN");
    cmd.env("HOME", tmp.path());
    cmd.env("BOWERBIRD_DATA_DIR", data_dir(tmp));
    cmd.env("BOWERBIRD_KEYRING_BACKEND", "disable");
    cmd.env("BOWERBIRD_LAUNCH_AGENT_LABEL", TEST_LAUNCH_AGENT_LABEL);
    cmd
}

fn write_config_toml(tmp: &TempDir, contents: &str, mode: u32) {
    use std::io::Write;
    let dir = data_dir(tmp);
    std::fs::create_dir_all(&dir).expect("mkdir data dir");
    let path = dir.join("config.toml");
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .mode(mode)
        .open(&path)
        .expect("open config.toml");
    f.write_all(contents.as_bytes()).expect("write config.toml");
}

/// 7.2 — env var path: `bowerbird auth token` with `BOWERBIRD_TOKEN` set
/// prints the env value verbatim on stdout (no banner, no prefix).
#[test]
fn auth_token_prints_env_var_when_set() {
    let tmp = TempDir::new().unwrap();
    let assertion = bowerbird_auth_command(&tmp)
        .env("BOWERBIRD_TOKEN", "test-token-7-2")
        .args(["auth", "token"])
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&assertion.get_output().stdout).into_owned();
    assert_eq!(
        stdout, "test-token-7-2\n",
        "stdout must be exactly the token + \\n; got {stdout:?}",
    );
}

/// 7.5 — config.toml path: with keychain disabled and no env, the token
/// in `~/.bowerbird/config.toml` is printed.
#[test]
fn auth_token_reads_from_config_toml_when_no_env_and_no_keychain() {
    let tmp = TempDir::new().unwrap();
    write_config_toml(&tmp, "token = \"from-cfg-file\"\n", 0o600);

    let assertion = bowerbird_auth_command(&tmp)
        .args(["auth", "token"])
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&assertion.get_output().stdout).into_owned();
    assert_eq!(stdout, "from-cfg-file\n", "stdout: {stdout:?}");
}

/// 7.6 — failure path: keychain disabled, no env, no config.toml → exit 1
/// (exactly, per Task 3.4 spec) with stderr enumerating every tried path
/// (NFR13 end-to-end).
#[test]
fn auth_token_returns_nonzero_when_all_paths_exhausted() {
    let tmp = TempDir::new().unwrap();

    let assertion = bowerbird_auth_command(&tmp)
        .args(["auth", "token"])
        .assert()
        .failure()
        .code(1);
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr).into_owned();
    assert!(
        stderr.contains("BOWERBIRD_TOKEN"),
        "stderr missing BOWERBIRD_TOKEN; got:\n{stderr}"
    );
    assert!(
        stderr.contains("keychain"),
        "stderr missing keychain; got:\n{stderr}"
    );
    assert!(
        stderr.contains("config.toml"),
        "stderr missing config.toml; got:\n{stderr}"
    );
}

/// 7.8 — stdout discipline: when stderr is piped (not a TTY), the
/// provenance hint stays silent. Stdout always carries exactly the token
/// followed by `\n` regardless of TTY state.
#[test]
fn auth_token_stderr_quiet_when_piped() {
    let tmp = TempDir::new().unwrap();

    // assert_cmd captures both stdout and stderr (neither is a TTY). The
    // CLI's `is_terminal` check on stderr returns false in this mode, so
    // no provenance hint should be emitted.
    let assertion = bowerbird_auth_command(&tmp)
        .env("BOWERBIRD_TOKEN", "piped-mode-token")
        .args(["auth", "token"])
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&assertion.get_output().stdout).into_owned();
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr).into_owned();
    assert_eq!(stdout, "piped-mode-token\n", "stdout: {stdout:?}");
    assert!(
        stderr.is_empty(),
        "stderr must stay silent when piped; got:\n{stderr}"
    );
}

/// 7.x — clap surface: `bowerbird auth --help` lists `token` and
/// `bowerbird --help` lists `auth`. Catches regressions where the
/// subcommand is silently dropped from the dispatch table.
#[test]
fn auth_appears_in_top_level_help_and_token_appears_in_auth_help() {
    let tmp = TempDir::new().unwrap();

    let top = bowerbird_auth_command(&tmp)
        .arg("--help")
        .assert()
        .success();
    let top_stdout = String::from_utf8_lossy(&top.get_output().stdout).into_owned();
    assert!(
        top_stdout.contains("auth"),
        "top-level help missing `auth`:\n{top_stdout}"
    );

    let sub = bowerbird_auth_command(&tmp)
        .args(["auth", "--help"])
        .assert()
        .success();
    let sub_stdout = String::from_utf8_lossy(&sub.get_output().stdout).into_owned();
    assert!(
        sub_stdout.contains("token"),
        "`auth --help` missing `token`:\n{sub_stdout}"
    );
}

/// 7.7 — AC #6 integration: with NO `BOWERBIRD_TOKEN` env set, both the
/// daemon and `bowerbird status` resolve the same token from the shared
/// `config.toml` (the user-facing UX win of Story 3.3). Without this test
/// the previous "set $BOWERBIRD_TOKEN or wait for Story 3.3" degraded path
/// could silently regress.
#[test]
fn status_shows_full_block_without_user_supplied_token() {
    use std::time::{Duration, Instant};

    let tmp = TempDir::new().unwrap();
    let daemon_bin = assert_cmd::cargo::cargo_bin("bowerbird-daemon");

    // Stage a config.toml token that both the daemon and the CLI will
    // resolve to (env is unset; keychain is disabled).
    write_config_toml(&tmp, "token = \"status-cfg-token-7-7\"\n", 0o600);

    // Spawn the daemon WITHOUT BOWERBIRD_TOKEN — it must pick up the
    // token from config.toml via the resolver chain.
    let mut start_assertion = bowerbird_auth_command(&tmp);
    start_assertion
        .env("BOWERBIRD_DAEMON_BIN", &daemon_bin)
        .env_remove("BOWERBIRD_TOKEN")
        .arg("start");
    let start_out = start_assertion.assert().success();
    let start_stdout = String::from_utf8_lossy(&start_out.get_output().stdout).into_owned();
    assert!(
        start_stdout.contains("started bowerbird-daemon") || start_stdout.contains("daemon ready"),
        "expected start confirmation; got:\n{start_stdout}"
    );

    // Wait for the ingest socket.
    let sock = data_dir(&tmp).join("ingest.sock");
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if std::os::unix::net::UnixStream::connect(&sock).is_ok() {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }

    // Cleanup helper closure — call before any panic so we don't leak the
    // spawned daemon into the runner's process tree.
    let pid_path = data_dir(&tmp).join("bowerbird.pid");
    let cleanup = || {
        let _ = bowerbird_auth_command(&tmp)
            .env("BOWERBIRD_DAEMON_BIN", &daemon_bin)
            .arg("stop")
            .assert();
        if let Ok(pid_s) = std::fs::read_to_string(&pid_path) {
            if let Ok(pid) = pid_s.trim().parse::<i32>() {
                let d = Instant::now() + Duration::from_secs(5);
                while Instant::now() < d {
                    if unsafe { libc::kill(pid, 0) } != 0 {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(20));
                }
            }
        }
    };

    let status_assertion = bowerbird_auth_command(&tmp)
        .env("BOWERBIRD_DAEMON_BIN", &daemon_bin)
        .env_remove("BOWERBIRD_TOKEN")
        .arg("status")
        .assert()
        .success();
    let status_stdout = String::from_utf8_lossy(&status_assertion.get_output().stdout).into_owned();

    if !status_stdout.contains("status        : running")
        || !status_stdout.contains("connected ws  : 0")
    {
        cleanup();
        panic!("expected full /status block (no degraded fallback); got:\n{status_stdout}");
    }
    if status_stdout.contains("$BOWERBIRD_TOKEN") || status_stdout.contains("wait for Story 3.3") {
        cleanup();
        panic!("CLI hit the degraded fallback path; got:\n{status_stdout}");
    }

    cleanup();
}

/// 7.x — wrong-mode config.toml: a non-0600 mode emits a warning on stderr
/// but the token value still comes through on stdout (operator-friendly:
/// refusing the file would lock users out on shared-machine boxes).
#[test]
fn auth_token_warns_on_wide_mode_but_still_loads() {
    let tmp = TempDir::new().unwrap();
    write_config_toml(&tmp, "token = \"wide-mode-still-loads\"\n", 0o644);

    let assertion = bowerbird_auth_command(&tmp)
        .args(["auth", "token"])
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&assertion.get_output().stdout).into_owned();
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr).into_owned();
    assert_eq!(stdout, "wide-mode-still-loads\n");
    assert!(
        stderr.contains("mode") && stderr.contains("0600"),
        "expected a 0600 mode warning on stderr; got:\n{stderr}"
    );
}

/// Spawn `bowerbird-daemon` via `bowerbird start` for a TempDir that already
/// has a populated `config.toml`. The daemon inherits the parent env, so
/// `BOWERBIRD_TOKEN` is unset and the keychain is disabled — forcing the
/// resolver to land on config.toml. Waits up to 5s for the ingest socket.
fn spawn_daemon_with_config_toml_backing(tmp: &TempDir, daemon_bin: &std::path::Path) {
    use std::time::{Duration, Instant};

    let mut start = bowerbird_auth_command(tmp);
    start
        .env("BOWERBIRD_DAEMON_BIN", daemon_bin)
        .env_remove("BOWERBIRD_TOKEN")
        .arg("start");
    let _ = start.assert().success();

    let sock = data_dir(tmp).join("ingest.sock");
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if std::os::unix::net::UnixStream::connect(&sock).is_ok() {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("daemon ingest socket did not come up within 5s");
}

/// Best-effort `bowerbird stop` for a TempDir; reaps the PID to avoid leaking
/// daemon processes into the test runner.
fn stop_daemon(tmp: &TempDir, daemon_bin: &std::path::Path) {
    use std::time::{Duration, Instant};

    let _ = bowerbird_auth_command(tmp)
        .env("BOWERBIRD_DAEMON_BIN", daemon_bin)
        .arg("stop")
        .assert();

    let pid_path = data_dir(tmp).join("bowerbird.pid");
    if let Ok(pid_s) = std::fs::read_to_string(&pid_path) {
        if let Ok(pid) = pid_s.trim().parse::<i32>() {
            let deadline = Instant::now() + Duration::from_secs(5);
            while Instant::now() < deadline {
                if unsafe { libc::kill(pid, 0) } != 0 {
                    break;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
        }
    }
}

/// 7.4 — daemon→CLI round-trip: with a shared `config.toml` backing, the
/// daemon's authoritative token and the token printed by `bowerbird auth
/// token` agree byte-for-byte. Pins Task 1.5's `SERVICE`/`USER`/file-format
/// invariants — a future rename in one binary but not the other would
/// silently break the round trip on the keychain path; the config-file path
/// catches the file-format slice (parser, field name, mode handling). The
/// mock-keyring backend can't bridge subprocess boundaries (see the
/// limitation note on `mock_keychain_first_run_generates_and_tags_source`
/// in `crates/daemon/tests/contract_daemon.rs::story_3_3_auth`), so this
/// test uses config.toml as the shared persistent backing store — the
/// recommended option-1 path from the story's sub-decision section.
#[test]
fn auth_token_matches_daemon_token_via_shared_config_file() {
    let tmp = TempDir::new().unwrap();
    let daemon_bin = assert_cmd::cargo::cargo_bin("bowerbird-daemon");

    write_config_toml(&tmp, "token = \"round-trip-7-4-token\"\n", 0o600);
    spawn_daemon_with_config_toml_backing(&tmp, &daemon_bin);

    let auth_out = bowerbird_auth_command(&tmp)
        .args(["auth", "token"])
        .assert();
    let cli_stdout = String::from_utf8_lossy(&auth_out.get_output().stdout).into_owned();

    // Always stop the daemon before asserting, so a content mismatch doesn't
    // leak the spawned daemon into the runner's process tree.
    stop_daemon(&tmp, &daemon_bin);

    auth_out.success();
    assert_eq!(
        cli_stdout, "round-trip-7-4-token\n",
        "CLI must print the same token the daemon read from config.toml; got: {cli_stdout:?}"
    );
}

/// NFR14 — "token is never reloaded at runtime without a restart." Stages a
/// `config.toml` with token A, starts the daemon (which caches A in
/// `AppState.bearer`), then rewrites `config.toml` to token B and proves the
/// daemon still authenticates with A and rejects B. Without this test, a
/// well-meaning future change that adds a `tokio::time::interval` re-reading
/// the config file at runtime would silently regress NFR14 — every existing
/// 3.3 test would still pass because none of them mutate `config.toml`
/// post-startup.
///
/// Fills the dropped Task 6.9 (`keychain_value_preserved_across_two_load_or_
/// generate_calls`) which had to be merged out at the resolver level because
/// the keyring mock backend doesn't persist across `Entry::new` calls; the
/// E2E flavor lands the same invariant via a backing store that DOES
/// persist (config.toml on disk).
#[test]
fn config_toml_rotation_does_not_affect_running_daemon() {
    let tmp = TempDir::new().unwrap();
    let daemon_bin = assert_cmd::cargo::cargo_bin("bowerbird-daemon");

    // Stage token A and start the daemon — it caches A at startup.
    write_config_toml(&tmp, "token = \"rotation-token-A\"\n", 0o600);
    spawn_daemon_with_config_toml_backing(&tmp, &daemon_bin);

    // Simulate a token rotation: rewrite config.toml to token B. The daemon
    // must NOT pick up B without a restart (NFR14).
    write_config_toml(&tmp, "token = \"rotation-token-B-after-start\"\n", 0o600);

    // /status with the daemon's cached token A → still works.
    let with_a = bowerbird_auth_command(&tmp)
        .env("BOWERBIRD_DAEMON_BIN", &daemon_bin)
        .env("BOWERBIRD_TOKEN", "rotation-token-A")
        .arg("status")
        .assert();
    let a_stdout = String::from_utf8_lossy(&with_a.get_output().stdout).into_owned();

    // /status with the new (post-rotation) token B → 401, because the daemon
    // never reloaded. The CLI prints the 401 message into the running-basic
    // form (status remains "running" + a 401 note).
    let with_b = bowerbird_auth_command(&tmp)
        .env("BOWERBIRD_DAEMON_BIN", &daemon_bin)
        .env("BOWERBIRD_TOKEN", "rotation-token-B-after-start")
        .arg("status")
        .assert();
    let b_stdout = String::from_utf8_lossy(&with_b.get_output().stdout).into_owned();

    // Always stop the daemon before any panic.
    stop_daemon(&tmp, &daemon_bin);

    with_a.success();
    with_b.success();

    assert!(
        a_stdout.contains("status        : running") && a_stdout.contains("connected ws  :"),
        "old token A must still authenticate (proves daemon cached A); got:\n{a_stdout}"
    );
    assert!(
        b_stdout.contains("running") && b_stdout.contains("401"),
        "new token B must be rejected with 401 (proves daemon did NOT reload \
         the rotated config.toml); got:\n{b_stdout}"
    );
}
