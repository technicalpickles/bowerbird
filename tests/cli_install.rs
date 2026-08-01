//! End-to-end tests for the user-facing `bowerbird` CLI.
//!
//! Library-level coverage of the settings.json atomic-write contract lives in
//! `crates/adapter-claude/`. The tests here exercise the user surface that
//! library tests cannot reach: clap subcommand wiring, `--settings` /
//! `BOWERBIRD_CLAUDE_SETTINGS` resolution, `--no-start` / `--no-stop` flags
//! (story 3.1 daemon-lifecycle scope cut), exit codes, and the round-trip
//! `bowerbird install` → `bowerbird uninstall` user journey via real
//! subprocess invocations of the compiled binary.
//!
//! The daemon-start / daemon-stop paths are deliberately bypassed via the
//! `--no-start` and `--no-stop` flags — covering those would require spawning
//! a real `bowerbird-daemon` process, which is already exercised end-to-end
//! by `crates/daemon/tests/contract_daemon.rs::story_3_1_singleton`. This
//! file's job is to prove the CLI surface itself behaves; the daemon
//! interaction has its own contract tests.

use std::fs;
use std::path::PathBuf;

use assert_cmd::Command;
use serde_json::{json, Value};
use tempfile::TempDir;

#[cfg(target_os = "macos")]
#[path = "support/fake_launchctl.rs"]
mod fake_launchctl;
#[cfg(target_os = "macos")]
use fake_launchctl::{with_fake_launchctl, write_executable, FAKE_LAUNCHCTL};

fn bowerbird_bin() -> Command {
    let mut cmd = Command::cargo_bin("bowerbird").expect("bowerbird binary built");
    // Every test isolates HOME so a misconfigured resolve_claude_settings or
    // resolve_bowerbird_dir cannot accidentally touch the developer's real
    // ~/.claude or ~/.bowerbird directories.
    cmd.env_remove("BOWERBIRD_CLAUDE_SETTINGS");
    cmd.env_remove("BOWERBIRD_DATA_DIR");
    // Story 5.9: on macOS `install` now writes a launchd plist. Drop any stray
    // override so each test's HOME-based `$HOME/Library/LaunchAgents` isolation
    // holds, and provide an absolute daemon-path placeholder so the existing
    // settings / tool-reactions tests don't depend on `bowerbird-daemon` being
    // built. Every install test below passes `--no-start`, so launchctl is
    // never invoked and the placeholder path is never exec'd.
    cmd.env_remove("BOWERBIRD_LAUNCH_AGENTS_DIR");
    cmd.env("BOWERBIRD_DAEMON_BIN", "/usr/local/bin/bowerbird-daemon");
    // Pin the real label: these tests assert plist filenames and never run
    // real launchctl (--no-start/--no-stop/fake-launchctl PATH seam), so the
    // scripts/test.sh isolation backstop must not rename the plist under them.
    cmd.env(
        "BOWERBIRD_LAUNCH_AGENT_LABEL",
        "com.technicalpickles.bowerbird.daemon",
    );
    cmd
}

fn settings_path(dir: &TempDir) -> PathBuf {
    dir.path().join("settings.json")
}

/// AC #5: invoking `bowerbird install --settings <missing-path> --no-start`
/// creates the file and returns exit code 0. Mirrors the user's first-run case
/// where `~/.claude/settings.json` does not yet exist.
#[test]
fn install_creates_settings_when_missing() {
    let dir = TempDir::new().expect("tempdir");
    let path = settings_path(&dir);
    assert!(!path.exists());

    bowerbird_bin()
        .arg("install")
        .arg("--settings")
        .arg(&path)
        .arg("--no-start")
        .env("HOME", dir.path())
        .assert()
        .success();

    let parsed: Value = serde_json::from_slice(&fs::read(&path).expect("read settings"))
        .expect("settings.json is valid JSON");
    // The bowerbird hook must be present under at least one known kind.
    let pre = parsed
        .pointer("/hooks/PreToolUse")
        .and_then(|v| v.as_array())
        .expect("hooks/PreToolUse array");
    assert!(
        !pre.is_empty(),
        "hooks/PreToolUse must contain at least one group"
    );
}

/// AC #1 + #4: install → uninstall round-trip via the CLI. The user's other
/// settings.json content (theme, user-authored hooks) survives both operations
/// intact. This is the journey the user actually performs.
#[test]
fn install_then_uninstall_via_cli_preserves_user_content() {
    let dir = TempDir::new().expect("tempdir");
    let path = settings_path(&dir);
    let initial = json!({
        "theme": "dark",
        "editor": {"fontSize": 14},
        "hooks": {
            "PreToolUse": [
                {"hooks": [{"type": "command", "command": "/user/own/hook --flag"}]}
            ]
        }
    });
    fs::write(
        &path,
        serde_json::to_vec_pretty(&initial).expect("seed json"),
    )
    .unwrap();

    bowerbird_bin()
        .arg("install")
        .arg("--settings")
        .arg(&path)
        .arg("--no-start")
        .env("HOME", dir.path())
        .assert()
        .success();

    bowerbird_bin()
        .arg("uninstall")
        .arg("--settings")
        .arg(&path)
        .arg("--no-stop")
        .env("HOME", dir.path())
        .assert()
        .success();

    let after: Value =
        serde_json::from_slice(&fs::read(&path).expect("read settings")).expect("valid json");
    assert_eq!(after.get("theme"), Some(&json!("dark")));
    assert_eq!(after.get("editor"), Some(&json!({"fontSize": 14})));
    let pre = after
        .pointer("/hooks/PreToolUse")
        .and_then(|v| v.as_array())
        .expect("PreToolUse array");
    let user_hook_count = pre
        .iter()
        .filter(|g| {
            g.pointer("/hooks/0/command").and_then(|v| v.as_str()) == Some("/user/own/hook --flag")
        })
        .count();
    assert_eq!(user_hook_count, 1, "user hook must survive the round-trip");
    let bowerbird_count = pre
        .iter()
        .filter(|g| {
            g.pointer("/hooks/0/command")
                .and_then(|v| v.as_str())
                .is_some_and(|c| c.starts_with(protocol::SHIM_BINARY_NAME))
        })
        .count();
    assert_eq!(
        bowerbird_count, 0,
        "uninstall must remove every bowerbird hook entry"
    );
}

/// AC #1: the CLI honors `BOWERBIRD_CLAUDE_SETTINGS` as the settings.json path
/// override. This is the test path that lets every other test target a
/// TempDir without `--settings` flag plumbing.
#[test]
fn install_respects_env_override_for_settings_path() {
    let dir = TempDir::new().expect("tempdir");
    let path = dir.path().join("custom-settings.json");

    bowerbird_bin()
        .arg("install")
        .arg("--no-start")
        .env("HOME", dir.path())
        .env("BOWERBIRD_CLAUDE_SETTINGS", &path)
        .assert()
        .success();

    assert!(path.exists(), "env override path must be created");
    let parsed: Value =
        serde_json::from_slice(&fs::read(&path).expect("read settings")).expect("valid json");
    assert!(parsed.pointer("/hooks/PreToolUse").is_some());
}

/// AC #1 (negative case): malformed settings.json must surface a non-zero exit
/// with a descriptive error message. The atomic-write contract requires the
/// original bytes stay on disk untouched — that part is library-tested in
/// `contract_install.rs`; here we only assert the CLI's exit code and that the
/// error message points at the file.
#[test]
fn install_exits_nonzero_on_malformed_settings_json() {
    let dir = TempDir::new().expect("tempdir");
    let path = settings_path(&dir);
    let original = b"{not-json: never::was}\n";
    fs::write(&path, original).unwrap();

    let assertion = bowerbird_bin()
        .arg("install")
        .arg("--settings")
        .arg(&path)
        .arg("--no-start")
        .env("HOME", dir.path())
        .assert()
        .failure();
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr).into_owned();
    assert!(
        stderr.contains(&path.display().to_string()),
        "error must reference the offending settings path; stderr={stderr}"
    );

    // The malformed bytes must be preserved verbatim (library contract).
    let after = fs::read(&path).expect("read settings");
    assert_eq!(after, original);
}

/// AC #4: `bowerbird uninstall` on a missing settings.json is a clean no-op
/// (exit 0, file not created). Mirrors the user's "I never installed" path.
#[test]
fn uninstall_on_missing_settings_is_a_clean_noop() {
    let dir = TempDir::new().expect("tempdir");
    let path = settings_path(&dir);
    assert!(!path.exists());

    bowerbird_bin()
        .arg("uninstall")
        .arg("--settings")
        .arg(&path)
        .arg("--no-stop")
        .env("HOME", dir.path())
        .assert()
        .success();

    assert!(
        !path.exists(),
        "uninstall must not create the file when it did not exist"
    );
}

/// AC #1: re-running `bowerbird install` is idempotent (exit 0, file unchanged).
/// This is the user pattern of running install during a setup script that may
/// already have configured the hook.
#[test]
fn install_twice_is_idempotent() {
    let dir = TempDir::new().expect("tempdir");
    let path = settings_path(&dir);

    bowerbird_bin()
        .arg("install")
        .arg("--settings")
        .arg(&path)
        .arg("--no-start")
        .env("HOME", dir.path())
        .assert()
        .success();
    let first = fs::read_to_string(&path).expect("read after first install");

    bowerbird_bin()
        .arg("install")
        .arg("--settings")
        .arg(&path)
        .arg("--no-start")
        .env("HOME", dir.path())
        .assert()
        .success();
    let second = fs::read_to_string(&path).expect("read after second install");

    assert_eq!(
        first, second,
        "re-installing must leave settings.json byte-identical"
    );
}

/// Story 5.4 AC #1: `bowerbird install` seeds the bundled `tool-reactions.toml`
/// under `$BOWERBIRD_DATA_DIR/adapters/claude/` on first run, and a second
/// install reports the file as already present without rewriting it.
#[test]
fn install_seeds_tool_reactions_on_fresh_bowerbird_dir() {
    let dir = TempDir::new().expect("tempdir");
    let settings = settings_path(&dir);
    let data_dir = dir.path().join("bowerbird-data");
    let seeded = data_dir
        .join("adapters")
        .join("claude")
        .join("tool-reactions.toml");

    let assertion = bowerbird_bin()
        .arg("install")
        .arg("--settings")
        .arg(&settings)
        .arg("--no-start")
        .env("HOME", dir.path())
        .env("BOWERBIRD_DATA_DIR", &data_dir)
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&assertion.get_output().stdout).into_owned();
    assert!(
        stdout.contains("seeded "),
        "first install must announce the seed; stdout={stdout}"
    );

    assert!(seeded.exists(), "tool-reactions.toml must be seeded");
    let bytes = fs::read(&seeded).expect("read seeded file");
    assert!(
        bytes.starts_with(b"# Claude Code tool name"),
        "seeded bytes must come from the bundled file (header marker)"
    );

    // Mutate the seeded file; a second install must leave it untouched.
    let custom = b"# user-edited\n[tool_reactions]\nCustom = \"Pause\"\n";
    fs::write(&seeded, custom).expect("overwrite for idempotency probe");

    let assertion = bowerbird_bin()
        .arg("install")
        .arg("--settings")
        .arg(&settings)
        .arg("--no-start")
        .env("HOME", dir.path())
        .env("BOWERBIRD_DATA_DIR", &data_dir)
        .assert()
        .success();
    // The skip hint goes to STDERR so scripted stdout stays clean (Story 5.4
    // review). stdout must NOT carry the skip line.
    let stdout = String::from_utf8_lossy(&assertion.get_output().stdout).into_owned();
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr).into_owned();
    assert!(
        stderr.contains("already exists; leaving user copy in place"),
        "second install must announce the skip on stderr; stderr={stderr}"
    );
    assert!(
        !stdout.contains("already exists; leaving user copy in place"),
        "skip hint must not pollute stdout; stdout={stdout}"
    );
    let after = fs::read(&seeded).expect("read seeded file after re-install");
    assert_eq!(
        after, custom,
        "second install must NOT overwrite the user-modified file"
    );
}

/// Story 5.9 AC 2/3/4/6: on macOS, `install --no-start` writes a well-formed
/// LaunchAgent plist into `BOWERBIRD_LAUNCH_AGENTS_DIR` (never the developer's
/// real `~/Library/LaunchAgents`), carrying the absolute daemon path,
/// `RunAtLoad`, and `KeepAlive = { SuccessfulExit = false }`. `--no-start`
/// means no real `launchctl` runs.
#[cfg(target_os = "macos")]
#[test]
fn install_writes_launch_agent_plist_on_macos() {
    let dir = TempDir::new().expect("tempdir");
    let settings = settings_path(&dir);
    let la_dir = dir.path().join("LaunchAgents");
    let plist = la_dir.join("com.technicalpickles.bowerbird.daemon.plist");
    // The default HOME-based location must NOT be touched when the override is set.
    let home_default = dir
        .path()
        .join("Library/LaunchAgents/com.technicalpickles.bowerbird.daemon.plist");

    let assertion = bowerbird_bin()
        .arg("install")
        .arg("--settings")
        .arg(&settings)
        .arg("--no-start")
        .env("HOME", dir.path())
        .env("BOWERBIRD_LAUNCH_AGENTS_DIR", &la_dir)
        .env(
            "BOWERBIRD_DAEMON_BIN",
            "/opt/bowerbird/bin/bowerbird-daemon",
        )
        .assert()
        .success();

    assert!(plist.exists(), "plist must land in the override dir");
    assert!(
        !home_default.exists(),
        "override dir must be honored; real $HOME/Library/LaunchAgents must stay untouched"
    );

    let xml = fs::read_to_string(&plist).expect("read plist");
    assert!(
        xml.starts_with("<?xml version=\"1.0\""),
        "well-formed plist prolog; xml={xml}"
    );
    assert!(xml.contains("<string>com.technicalpickles.bowerbird.daemon</string>"));
    assert!(
        xml.contains("<string>/opt/bowerbird/bin/bowerbird-daemon</string>"),
        "ProgramArguments must be the absolute daemon path; xml={xml}"
    );
    assert!(xml.contains("<key>RunAtLoad</key>"));
    assert!(
        xml.contains("<key>SuccessfulExit</key>") && xml.contains("<false/>"),
        "KeepAlive must be SuccessfulExit=false; xml={xml}"
    );

    // The success line is on stdout for scripted consumption.
    let stdout = String::from_utf8_lossy(&assertion.get_output().stdout).into_owned();
    assert!(
        stdout.contains("start on login"),
        "install must announce the login registration on stdout; stdout={stdout}"
    );
}

/// Story 5.9 AC 7/9: on macOS, `install --no-start` then `uninstall --no-stop`
/// leaves no LaunchAgent residue — the plist is gone. `--no-stop` skips the
/// bootout (no real `launchctl`) but still removes the plist.
#[cfg(target_os = "macos")]
#[test]
fn install_uninstall_round_trip_leaves_no_launch_agent_residue_on_macos() {
    let dir = TempDir::new().expect("tempdir");
    let settings = settings_path(&dir);
    let la_dir = dir.path().join("LaunchAgents");
    let plist = la_dir.join("com.technicalpickles.bowerbird.daemon.plist");

    bowerbird_bin()
        .arg("install")
        .arg("--settings")
        .arg(&settings)
        .arg("--no-start")
        .env("HOME", dir.path())
        .env("BOWERBIRD_LAUNCH_AGENTS_DIR", &la_dir)
        .assert()
        .success();
    assert!(plist.exists(), "plist must exist after install");

    bowerbird_bin()
        .arg("uninstall")
        .arg("--settings")
        .arg(&settings)
        .arg("--no-stop")
        .env("HOME", dir.path())
        .env("BOWERBIRD_LAUNCH_AGENTS_DIR", &la_dir)
        .assert()
        .success();
    assert!(
        !plist.exists(),
        "uninstall must remove the plist (no LaunchAgent residue)"
    );
}

/// Story 5.9 AC 7: `uninstall --no-stop` on a system that was never installed
/// (no plist present) is a clean no-op — removing a missing plist must not error.
#[cfg(target_os = "macos")]
#[test]
fn uninstall_on_missing_launch_agent_is_a_clean_noop_on_macos() {
    let dir = TempDir::new().expect("tempdir");
    let settings = settings_path(&dir);
    let la_dir = dir.path().join("LaunchAgents");

    bowerbird_bin()
        .arg("uninstall")
        .arg("--settings")
        .arg(&settings)
        .arg("--no-stop")
        .env("HOME", dir.path())
        .env("BOWERBIRD_LAUNCH_AGENTS_DIR", &la_dir)
        .assert()
        .success();
}

/// Story 5.9 AC 8: on non-macOS, `install` writes no plist and prints one
/// stderr note that start-on-login supervision is macOS-only for V1.
#[cfg(not(target_os = "macos"))]
#[test]
fn install_notes_supervision_is_macos_only_on_non_macos() {
    let dir = TempDir::new().expect("tempdir");
    let settings = settings_path(&dir);
    let la_dir = dir.path().join("LaunchAgents");

    let assertion = bowerbird_bin()
        .arg("install")
        .arg("--settings")
        .arg(&settings)
        .arg("--no-start")
        .env("HOME", dir.path())
        .env("BOWERBIRD_LAUNCH_AGENTS_DIR", &la_dir)
        .assert()
        .success();

    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr).into_owned();
    assert!(
        stderr.contains("macOS-only"),
        "non-macOS install must note macOS-only supervision on stderr; stderr={stderr}"
    );
    assert!(
        !la_dir.exists(),
        "no LaunchAgent plist must be written on non-macOS"
    );
}

/// Story 5.9 review F4: on macOS, `install` WITHOUT `--no-start` (the bootstrap
/// path) refuses to register a daemon launchd cannot exec. A non-executable
/// `BOWERBIRD_DAEMON_BIN` must fail with a clear error and write no plist —
/// proving the executable check runs before the plist write and any `launchctl`
/// call (so this test never invokes real launchctl).
#[cfg(target_os = "macos")]
#[test]
fn install_bootstrap_path_rejects_unlaunchable_daemon_on_macos() {
    let dir = TempDir::new().expect("tempdir");
    let settings = settings_path(&dir);
    let la_dir = dir.path().join("LaunchAgents");
    let plist = la_dir.join("com.technicalpickles.bowerbird.daemon.plist");
    let bogus_daemon = dir.path().join("does-not-exist-bowerbird-daemon");

    let assertion = bowerbird_bin()
        .arg("install")
        .arg("--settings")
        .arg(&settings)
        // NOTE: no --no-start — exercise the bootstrap path's F4 gate.
        .env("HOME", dir.path())
        .env("BOWERBIRD_LAUNCH_AGENTS_DIR", &la_dir)
        .env("BOWERBIRD_DAEMON_BIN", &bogus_daemon)
        .assert()
        .failure();

    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr).into_owned();
    assert!(
        stderr.contains("not an executable file"),
        "install must explain the daemon path is unlaunchable; stderr={stderr}"
    );
    assert!(
        !plist.exists(),
        "no plist may be written when the daemon path is unlaunchable (validation precedes write)"
    );
}

// --- Story 5.9 review F6: a fake `launchctl` seam --------------------------
//
// The `--no-start`/`--no-stop` tests above never invoke `launchctl`, leaving the
// highest-risk macOS lifecycle branches (loaded-agent reinstall handoff,
// bootout-failure handling, the `start` launchd path) unexercised. Rather than a
// Rust-level trait seam — useless here because these tests spawn the real
// `bowerbird` binary as a subprocess — we put a fake `launchctl` on the spawned
// process's PATH. The fake records each invocation to `$FAKE_LAUNCHCTL_LOG` and
// exits with a per-subcommand code from `FAKE_LAUNCHCTL_*` env vars (default:
// `print` exits 1 = "not loaded"; everything else exits 0). No real launchd is
// touched, so this is CI-safe on the macOS runner.

/// Spawn a REAL BSD-flock holder on `<...>/bowerbird.pid`, mirroring the daemon's
/// singleton (`crates/daemon/src/singleton.rs`: a `flock(LOCK_EX)` held for the
/// process lifetime, the daemon's pid written into the file). The pre-pass-6
/// tests modeled a "singleton holder" with a plain `sleep` + a hand-written pid
/// file, which proved PID liveness, not lock ownership — the CLI's flock probe
/// (pass-6 F4) correctly ignores such a file as stale. macOS ships perl, whose
/// `flock` is `flock(2)` (the same primitive nix/libc use), so the spawned
/// `bowerbird` subprocess sees the lock held. Returns the child so the test can
/// reap it after `bowerbird` SIGTERMs it (or kill it directly when `bowerbird` is
/// expected to refuse). Blocks until the holder has acquired the lock and written
/// its pid.
#[cfg(target_os = "macos")]
fn spawn_flock_holder(pid_path: &std::path::Path) -> std::process::Child {
    // +>> = read/write + create (no truncate-on-open, avoiding a truncate-before-
    // lock race); flock LOCK_EX (2); then clear + write our own pid and sleep,
    // releasing the lock only when the fd closes (SIGTERM/SIGKILL), exactly like
    // the daemon.
    const HOLD: &str = r#"
        open(my $f, "+>>", $ARGV[0]) or die "open: $!";
        flock($f, 2) or die "flock: $!";
        seek($f, 0, 0); truncate($f, 0);
        my $old = select($f); $| = 1; select($old);
        print $f "$$\n";
        $SIG{TERM} = sub { exit 0 };
        sleep 600;
    "#;
    let child = std::process::Command::new("perl")
        .arg("-e")
        .arg(HOLD)
        .arg(pid_path)
        .spawn()
        .expect("spawn perl flock holder");
    for _ in 0..500 {
        if let Ok(s) = fs::read_to_string(pid_path) {
            if s.trim().parse::<i32>().is_ok() {
                break;
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    child
}

/// Story 5.9 review F2/F3: a reinstall over an already-loaded agent must boot the
/// old agent out (so the freshly-written plist's ProgramArguments/env take
/// effect) and then re-bootstrap — never bootstrap on top of the stale loaded
/// job. Exercises the full install launchd handoff with the fake `launchctl`.
#[cfg(target_os = "macos")]
#[test]
fn install_reinstall_over_loaded_agent_bootouts_then_bootstraps_on_macos() {
    let dir = TempDir::new().expect("tempdir");
    let settings = settings_path(&dir);
    let la_dir = dir.path().join("LaunchAgents");
    let plist = la_dir.join("com.technicalpickles.bowerbird.daemon.plist");
    let bin_dir = dir.path().join("bin");
    fs::create_dir_all(&bin_dir).expect("bin dir");
    write_executable(&bin_dir.join("launchctl"), FAKE_LAUNCHCTL);
    // A real executable daemon so the F4 launchable check passes.
    let daemon = bin_dir.join("bowerbird-daemon");
    write_executable(&daemon, "#!/bin/sh\nexit 0\n");
    let log = dir.path().join("launchctl.log");

    let mut cmd = bowerbird_bin();
    cmd.arg("install")
        .arg("--settings")
        .arg(&settings)
        // NOTE: no --no-start — drive the launchd handoff + bootstrap.
        .env("HOME", dir.path())
        .env("BOWERBIRD_LAUNCH_AGENTS_DIR", &la_dir)
        .env("BOWERBIRD_DAEMON_BIN", &daemon)
        .env("FAKE_LAUNCHCTL_PRINT_EXIT", "0") // agent reports loaded
        .env("FAKE_LAUNCHCTL_BOOTOUT_EXIT", "0")
        .env("FAKE_LAUNCHCTL_BOOTSTRAP_EXIT", "0");
    with_fake_launchctl(&mut cmd, &bin_dir, &log);
    cmd.assert().success();

    assert!(plist.exists(), "plist must be written");
    let calls = fs::read_to_string(&log).unwrap_or_default();
    let bootout_at = calls.find("bootout");
    let bootstrap_at = calls.find("bootstrap");
    assert!(
        bootout_at.is_some(),
        "a loaded agent must be booted out before re-bootstrap; calls=\n{calls}"
    );
    assert!(
        bootstrap_at.is_some(),
        "install must re-bootstrap the fresh plist; calls=\n{calls}"
    );
    assert!(
        bootout_at < bootstrap_at,
        "bootout must precede bootstrap; calls=\n{calls}"
    );
}

/// Story 5.9 review F3: a real `bootout` failure (agent still loaded,
/// unverifiable) must fail `uninstall` loudly — it must NOT downgrade to a
/// warning and then claim "removed login registration", and it must leave the
/// plist in place so a retry still has the registration to act on.
#[cfg(target_os = "macos")]
#[test]
fn uninstall_fails_loudly_when_bootout_fails_on_macos() {
    let dir = TempDir::new().expect("tempdir");
    let settings = settings_path(&dir);
    let la_dir = dir.path().join("LaunchAgents");
    let plist = la_dir.join("com.technicalpickles.bowerbird.daemon.plist");
    let bin_dir = dir.path().join("bin");
    fs::create_dir_all(&bin_dir).expect("bin dir");
    write_executable(&bin_dir.join("launchctl"), FAKE_LAUNCHCTL);
    let log = dir.path().join("launchctl.log");

    // Register the plist first (launchctl-free via --no-start).
    bowerbird_bin()
        .arg("install")
        .arg("--settings")
        .arg(&settings)
        .arg("--no-start")
        .env("HOME", dir.path())
        .env("BOWERBIRD_LAUNCH_AGENTS_DIR", &la_dir)
        .assert()
        .success();
    assert!(plist.exists(), "plist registered");

    // Uninstall WITHOUT --no-stop: bootout (and the legacy unload fallback) fail
    // and `print` reports the agent still loaded — a real, unverifiable failure.
    let mut cmd = bowerbird_bin();
    cmd.arg("uninstall")
        .arg("--settings")
        .arg(&settings)
        .env("HOME", dir.path())
        .env("BOWERBIRD_LAUNCH_AGENTS_DIR", &la_dir)
        .env("FAKE_LAUNCHCTL_PRINT_EXIT", "0") // still loaded
        .env("FAKE_LAUNCHCTL_BOOTOUT_EXIT", "1") // real failure
        .env("FAKE_LAUNCHCTL_UNLOAD_EXIT", "1");
    with_fake_launchctl(&mut cmd, &bin_dir, &log);
    let assertion = cmd.assert().failure();

    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr).into_owned();
    assert!(
        stderr.contains("could not unload") || stderr.contains("LaunchAgent out of launchd"),
        "uninstall must surface the bootout failure; stderr={stderr}"
    );
    assert!(
        plist.exists(),
        "a failed bootout must NOT remove the plist (retry must still find the registration)"
    );
}

/// Story 5.9 review F2/F6: when a plist is registered and the agent is loaded
/// but the daemon is down (the post-`bowerbird stop` state under
/// KeepAlive={SuccessfulExit=false}), `bowerbird start` drives launchd via
/// `kickstart` rather than spawning a competing detached daemon. Readiness then
/// times out (no real daemon writes server.json), so `start` exits non-zero —
/// but the kickstart branch is what we are verifying.
#[cfg(target_os = "macos")]
#[test]
fn start_kickstarts_loaded_but_down_agent_on_macos() {
    let dir = TempDir::new().expect("tempdir");
    let la_dir = dir.path().join("LaunchAgents");
    let plist = la_dir.join("com.technicalpickles.bowerbird.daemon.plist");
    let data_dir = dir.path().join("data");
    let bin_dir = dir.path().join("bin");
    fs::create_dir_all(&la_dir).expect("la dir");
    fs::create_dir_all(&data_dir).expect("data dir");
    fs::create_dir_all(&bin_dir).expect("bin dir");
    write_executable(&bin_dir.join("launchctl"), FAKE_LAUNCHCTL);
    // A plist must exist for `start` to choose the launchd path; content is
    // irrelevant (the fake launchctl ignores it).
    fs::write(&plist, "<plist/>\n").expect("write plist stub");
    let log = dir.path().join("launchctl.log");

    let mut cmd = bowerbird_bin();
    cmd.arg("start")
        .env("HOME", dir.path())
        .env("BOWERBIRD_DATA_DIR", &data_dir)
        .env("BOWERBIRD_LAUNCH_AGENTS_DIR", &la_dir)
        .env("FAKE_LAUNCHCTL_PRINT_EXIT", "0") // agent loaded
        .env("FAKE_LAUNCHCTL_KICKSTART_EXIT", "0");
    with_fake_launchctl(&mut cmd, &bin_dir, &log);
    cmd.assert().failure(); // readiness times out; no real daemon

    let calls = fs::read_to_string(&log).unwrap_or_default();
    assert!(
        calls.contains("kickstart"),
        "a loaded-but-down agent must be kickstarted, not respawned; calls=\n{calls}"
    );
    // F3: the kickstart must use `-k` (kill-then-restart) so a daemon wedged
    // before binding the socket is repaired rather than left running.
    assert!(
        calls.lines().any(|l| l.starts_with("kickstart -k ")),
        "kickstart must pass -k (force-restart) for loaded-but-down recovery; calls=\n{calls}"
    );
    assert!(
        !calls.contains("bootstrap"),
        "an already-loaded agent must not be re-bootstrapped by start; calls=\n{calls}"
    );
}

/// Story 5.9 review pass-3 F1: a `bootout` that fails with an *unverifiable*
/// `launchctl print` (a non-zero exit whose stderr is NOT an explicit
/// absent-service signal) must fail `uninstall` loudly — the CLI must not
/// collapse "cannot verify" into "not loaded", silently remove the plist, and
/// claim success while the agent may still be supervising the session.
#[cfg(target_os = "macos")]
#[test]
fn uninstall_fails_when_launchctl_print_is_unverifiable_on_macos() {
    let dir = TempDir::new().expect("tempdir");
    let settings = settings_path(&dir);
    let la_dir = dir.path().join("LaunchAgents");
    let plist = la_dir.join("com.technicalpickles.bowerbird.daemon.plist");
    let bin_dir = dir.path().join("bin");
    fs::create_dir_all(&bin_dir).expect("bin dir");
    write_executable(&bin_dir.join("launchctl"), FAKE_LAUNCHCTL);
    let log = dir.path().join("launchctl.log");

    // Register the plist first (launchctl-free via --no-start).
    bowerbird_bin()
        .arg("install")
        .arg("--settings")
        .arg(&settings)
        .arg("--no-start")
        .env("HOME", dir.path())
        .env("BOWERBIRD_LAUNCH_AGENTS_DIR", &la_dir)
        .assert()
        .success();
    assert!(plist.exists(), "plist registered");

    // bootout fails, and `print` returns a NON-absent error (permission), so the
    // loaded state is unverifiable — uninstall must error, not claim success.
    let mut cmd = bowerbird_bin();
    cmd.arg("uninstall")
        .arg("--settings")
        .arg(&settings)
        .env("HOME", dir.path())
        .env("BOWERBIRD_LAUNCH_AGENTS_DIR", &la_dir)
        .env("FAKE_LAUNCHCTL_BOOTOUT_EXIT", "1")
        .env("FAKE_LAUNCHCTL_UNLOAD_EXIT", "1")
        .env("FAKE_LAUNCHCTL_PRINT_EXIT", "1")
        .env("FAKE_LAUNCHCTL_PRINT_STDERR", "Operation not permitted");
    with_fake_launchctl(&mut cmd, &bin_dir, &log);
    let assertion = cmd.assert().failure();

    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr).into_owned();
    assert!(
        stderr.contains("could not verify launchd state"),
        "uninstall must surface the unverifiable launchd state; stderr={stderr}"
    );
    assert!(
        plist.exists(),
        "an unverifiable bootout must NOT remove the plist (retry must still find it)"
    );
}

/// Story 5.9 review pass-5 F5: when the modern `bootout` fails as
/// unsupported AND `launchctl print` is unverifiable, the legacy `unload -w`
/// fallback must still run — and if it succeeds, uninstall succeeds and removes
/// the plist. The previous code called `!agent_loaded(uid)?` between `bootout`
/// and the legacy `unload`, so an unverifiable `print` returned early ("cannot
/// verify") and the fallback never ran, failing uninstall on environments where
/// `unload` would have worked.
#[cfg(target_os = "macos")]
#[test]
fn uninstall_legacy_unload_runs_when_bootout_and_print_unsupported_on_macos() {
    let dir = TempDir::new().expect("tempdir");
    let settings = settings_path(&dir);
    let la_dir = dir.path().join("LaunchAgents");
    let plist = la_dir.join("com.technicalpickles.bowerbird.daemon.plist");
    let bin_dir = dir.path().join("bin");
    fs::create_dir_all(&bin_dir).expect("bin dir");
    write_executable(&bin_dir.join("launchctl"), FAKE_LAUNCHCTL);
    let log = dir.path().join("launchctl.log");

    // Register the plist first (launchctl-free via --no-start).
    bowerbird_bin()
        .arg("install")
        .arg("--settings")
        .arg(&settings)
        .arg("--no-start")
        .env("HOME", dir.path())
        .env("BOWERBIRD_LAUNCH_AGENTS_DIR", &la_dir)
        .assert()
        .success();
    assert!(plist.exists(), "plist registered");

    // Modern `bootout` fails (unsupported), `print` is unverifiable (non-absent
    // stderr), but the legacy `unload -w` succeeds. Uninstall must fall through
    // to the legacy path and succeed.
    let mut cmd = bowerbird_bin();
    cmd.arg("uninstall")
        .arg("--settings")
        .arg(&settings)
        .env("HOME", dir.path())
        .env("BOWERBIRD_LAUNCH_AGENTS_DIR", &la_dir)
        .env("FAKE_LAUNCHCTL_BOOTOUT_EXIT", "1") // modern unsupported
        .env("FAKE_LAUNCHCTL_PRINT_EXIT", "1")
        .env("FAKE_LAUNCHCTL_PRINT_STDERR", "Operation not permitted") // unverifiable
        .env("FAKE_LAUNCHCTL_UNLOAD_EXIT", "0"); // legacy fallback works
    with_fake_launchctl(&mut cmd, &bin_dir, &log);
    cmd.assert().success();

    let calls = fs::read_to_string(&log).unwrap_or_default();
    assert!(
        calls.lines().any(|l| l.starts_with("unload ")),
        "the legacy `unload` fallback must run when bootout+print are unsupported; calls=\n{calls}"
    );
    assert!(
        !plist.exists(),
        "a successful legacy unload must let uninstall remove the plist"
    );
}

/// Story 5.9 review pass-5 F6: `uninstall --no-stop` on macOS removes the plist
/// without booting the agent out, so it must NOT imply the registration is fully
/// gone — a job already loaded this login session keeps being supervised by
/// launchd until logout/manual bootout. Assert uninstall says so (and that it
/// did not call bootout). Guards the docs/message from regressing to "the
/// registration is gone".
#[cfg(target_os = "macos")]
#[test]
fn uninstall_no_stop_warns_in_session_supervision_survives_on_macos() {
    let dir = TempDir::new().expect("tempdir");
    let settings = settings_path(&dir);
    let la_dir = dir.path().join("LaunchAgents");
    let plist = la_dir.join("com.technicalpickles.bowerbird.daemon.plist");
    let bin_dir = dir.path().join("bin");
    fs::create_dir_all(&bin_dir).expect("bin dir");
    write_executable(&bin_dir.join("launchctl"), FAKE_LAUNCHCTL);
    let log = dir.path().join("launchctl.log");

    bowerbird_bin()
        .arg("install")
        .arg("--settings")
        .arg(&settings)
        .arg("--no-start")
        .env("HOME", dir.path())
        .env("BOWERBIRD_LAUNCH_AGENTS_DIR", &la_dir)
        .assert()
        .success();
    assert!(plist.exists(), "plist registered");

    let mut cmd = bowerbird_bin();
    cmd.arg("uninstall")
        .arg("--settings")
        .arg(&settings)
        .arg("--no-stop")
        .env("HOME", dir.path())
        .env("BOWERBIRD_LAUNCH_AGENTS_DIR", &la_dir);
    with_fake_launchctl(&mut cmd, &bin_dir, &log);
    let assertion = cmd.assert().success();

    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr).into_owned();
    assert!(
        stderr.contains("launchd keeps supervising") && stderr.contains("bootout"),
        "uninstall --no-stop must warn that in-session supervision survives plist removal; \
         stderr={stderr}"
    );
    assert!(
        !plist.exists(),
        "uninstall --no-stop still removes the plist"
    );
    let calls = fs::read_to_string(&log).unwrap_or_default();
    assert!(
        !calls.contains("bootout"),
        "--no-stop must NOT boot the agent out; calls=\n{calls}"
    );
}

/// Story 5.9 review pass-3 F2: `bowerbird start` must look for the daemon where
/// launchd will actually run it — the data dir registered in the plist's
/// `EnvironmentVariables` — not the data dir in the *current* CLI env. We
/// install with data dir A (so the plist embeds A), drop a STALE `server.json`
/// into A, then run `start` from a *different* CLI data dir B. `start` (pass-6 F5)
/// removes the EFFECTIVE data dir's stale `server.json` before handing to launchd.
/// If `start` resolved the effective dir from the plist env (A) rather than the
/// CLI env (B), it is A's `server.json` that gets removed — proving F2 (reads the
/// registered env) and F5 (clears the stale hint) at once. (Pre-pass-6 this test
/// proved F2 via "start finds A's stale server.json", but F5 now deliberately
/// removes that stale file, so the proof flips to "A's server.json was cleared".)
#[cfg(target_os = "macos")]
#[test]
fn start_uses_registered_plist_data_dir_not_cli_env_on_macos() {
    let dir = TempDir::new().expect("tempdir");
    let settings = settings_path(&dir);
    let la_dir = dir.path().join("LaunchAgents");
    let dir_a = dir.path().join("data-a");
    let dir_b = dir.path().join("data-b");
    let bin_dir = dir.path().join("bin");
    fs::create_dir_all(&dir_a).expect("data-a");
    fs::create_dir_all(&dir_b).expect("data-b");
    fs::create_dir_all(&bin_dir).expect("bin dir");
    write_executable(&bin_dir.join("launchctl"), FAKE_LAUNCHCTL);
    // A real executable daemon so start's F6 launchable revalidation passes.
    let daemon = bin_dir.join("bowerbird-daemon");
    write_executable(&daemon, "#!/bin/sh\nexit 0\n");

    // Install with data dir A (--no-start: writes the plist, no real launchctl).
    bowerbird_bin()
        .arg("install")
        .arg("--settings")
        .arg(&settings)
        .arg("--no-start")
        .env("HOME", dir.path())
        .env("BOWERBIRD_LAUNCH_AGENTS_DIR", &la_dir)
        .env("BOWERBIRD_DATA_DIR", &dir_a)
        .env("BOWERBIRD_DAEMON_BIN", &daemon)
        .assert()
        .success();

    // install canonicalizes the data dir before embedding it; write the stale
    // server.json where the plist actually points.
    let dir_a_canon = fs::canonicalize(&dir_a).expect("canonicalize data-a");
    fs::write(
        dir_a_canon.join("server.json"),
        r#"{"bind_addr":"127.0.0.1:1","token":"x"}"#,
    )
    .expect("write stale server.json into A");

    // start from a DIFFERENT CLI data dir (B). If F2 is honored it reads A's env.
    let log = dir.path().join("launchctl.log");
    let mut cmd = bowerbird_bin();
    cmd.arg("start")
        .env("HOME", dir.path())
        .env("BOWERBIRD_DATA_DIR", &dir_b)
        .env("BOWERBIRD_LAUNCH_AGENTS_DIR", &la_dir)
        .env("FAKE_LAUNCHCTL_PRINT_EXIT", "1") // not loaded -> bootstrap
        .env("FAKE_LAUNCHCTL_BOOTSTRAP_EXIT", "0");
    with_fake_launchctl(&mut cmd, &bin_dir, &log);
    // Readiness times out (the fake daemon writes no fresh server.json).
    cmd.assert().failure();

    // F2 + F5: start operated on A (the plist env), so it cleared A's stale
    // server.json. If it had used CLI dir B, A's file would still be present.
    assert!(
        !dir_a_canon.join("server.json").exists(),
        "start must resolve the effective dir from the plist env (A) and clear A's stale \
         server.json (F2 + F5)"
    );
    assert!(
        !dir_b.join("server.json").exists(),
        "the test must not have created server.json in B"
    );
    let calls = fs::read_to_string(&log).unwrap_or_default();
    assert!(
        calls.contains("bootstrap"),
        "start must bootstrap the not-loaded agent; calls=\n{calls}"
    );
}

/// Story 5.9 review pass-3 F4: install refuses to persist a *relative*
/// `BOWERBIRD_INGEST_SOCK` into launchd (launchd/shim/CLI would resolve it
/// against differing working directories). Even with `--no-start`, the plist
/// must not be written with a non-absolute socket.
#[cfg(target_os = "macos")]
#[test]
fn install_rejects_relative_ingest_sock_on_macos() {
    let dir = TempDir::new().expect("tempdir");
    let settings = settings_path(&dir);
    let la_dir = dir.path().join("LaunchAgents");
    let plist = la_dir.join("com.technicalpickles.bowerbird.daemon.plist");

    let assertion = bowerbird_bin()
        .arg("install")
        .arg("--settings")
        .arg(&settings)
        .arg("--no-start")
        .env("HOME", dir.path())
        .env("BOWERBIRD_LAUNCH_AGENTS_DIR", &la_dir)
        .env("BOWERBIRD_DAEMON_BIN", "/usr/local/bin/bowerbird-daemon")
        .env("BOWERBIRD_INGEST_SOCK", "relative/ingest.sock")
        .assert()
        .failure();

    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr).into_owned();
    assert!(
        stderr.contains("non-absolute") && stderr.contains("BOWERBIRD_INGEST_SOCK"),
        "install must explain the relative socket is rejected; stderr={stderr}"
    );
    assert!(
        !plist.exists(),
        "no plist may be written when the ingest socket is non-absolute"
    );
}

/// Story 5.9 review pass-3 F4 (the accepted side): an *absolute*
/// `BOWERBIRD_INGEST_SOCK` is embedded verbatim into the plist's
/// `EnvironmentVariables`.
#[cfg(target_os = "macos")]
#[test]
fn install_embeds_absolute_ingest_sock_in_plist_on_macos() {
    let dir = TempDir::new().expect("tempdir");
    let settings = settings_path(&dir);
    let la_dir = dir.path().join("LaunchAgents");
    let plist = la_dir.join("com.technicalpickles.bowerbird.daemon.plist");
    let sock = dir.path().join("custom-ingest.sock");

    bowerbird_bin()
        .arg("install")
        .arg("--settings")
        .arg(&settings)
        .arg("--no-start")
        .env("HOME", dir.path())
        .env("BOWERBIRD_LAUNCH_AGENTS_DIR", &la_dir)
        .env("BOWERBIRD_DAEMON_BIN", "/usr/local/bin/bowerbird-daemon")
        .env("BOWERBIRD_INGEST_SOCK", &sock)
        .assert()
        .success();

    let xml = fs::read_to_string(&plist).expect("read plist");
    assert!(
        xml.contains("<key>BOWERBIRD_INGEST_SOCK</key>"),
        "an absolute ingest sock must be embedded in EnvironmentVariables; xml={xml}"
    );
    assert!(
        xml.contains(&format!("<string>{}</string>", sock.display())),
        "the embedded socket must be the absolute path; xml={xml}"
    );
}

/// Story 5.9 review pass-3 F5: when `uninstall` (without `--no-stop`) boots the
/// agent out but a manual / pre-5.9 daemon is still accepting on the ingest
/// socket with no PID file to stop it, uninstall must warn clearly instead of
/// silently claiming success. We bind a real listener on the effective socket
/// (no PID file present) and assert the warning.
#[cfg(target_os = "macos")]
#[test]
fn uninstall_warns_when_live_daemon_survives_missing_pid_on_macos() {
    use std::os::unix::net::UnixListener;

    let dir = TempDir::new().expect("tempdir");
    let settings = settings_path(&dir);
    let la_dir = dir.path().join("LaunchAgents");
    let data_dir = dir.path().join("data");
    let bin_dir = dir.path().join("bin");
    fs::create_dir_all(&data_dir).expect("data dir");
    fs::create_dir_all(&bin_dir).expect("bin dir");
    write_executable(&bin_dir.join("launchctl"), FAKE_LAUNCHCTL);
    let log = dir.path().join("launchctl.log");

    // Register the plist (launchctl-free).
    bowerbird_bin()
        .arg("install")
        .arg("--settings")
        .arg(&settings)
        .arg("--no-start")
        .env("HOME", dir.path())
        .env("BOWERBIRD_LAUNCH_AGENTS_DIR", &la_dir)
        .env("BOWERBIRD_DATA_DIR", &data_dir)
        .assert()
        .success();

    // A live daemon on the effective socket, but NO bowerbird.pid file.
    let sock = data_dir.join("ingest.sock");
    let _listener = UnixListener::bind(&sock).expect("bind fake daemon socket");

    let mut cmd = bowerbird_bin();
    cmd.arg("uninstall")
        .arg("--settings")
        .arg(&settings)
        .env("HOME", dir.path())
        .env("BOWERBIRD_DATA_DIR", &data_dir)
        .env("BOWERBIRD_LAUNCH_AGENTS_DIR", &la_dir)
        .env("FAKE_LAUNCHCTL_BOOTOUT_EXIT", "0"); // bootout succeeds
    with_fake_launchctl(&mut cmd, &bin_dir, &log);
    // Non-fatal: settings are already un-merged, so uninstall still exits 0.
    let assertion = cmd.assert().success();

    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr).into_owned();
    assert!(
        stderr.contains("still accepting on"),
        "uninstall must warn that a live daemon survived the missing PID file; stderr={stderr}"
    );
}

/// Story 5.9 review pass-4 #1 + pass-6 F3: when a daemon is already accepting on
/// the registered socket and `launchctl print` is UNVERIFIABLE (a non-zero exit
/// whose stderr is not an absent-service signal — `Err` / `LoadState::Unknown`),
/// `bowerbird start` must NOT fail and must NOT kill the working daemon to migrate
/// it on a guess: it leaves the daemon running, warns that launchd supervision
/// could not be confirmed, and exits 0. (pass-6 F3 lets `start` consult `print` to
/// decide supervision, so — unlike the pass-4 wording — a `print` call is now
/// expected; what must NOT happen is any launchd ACTION: bootstrap/kickstart/bootout.)
#[cfg(target_os = "macos")]
#[test]
fn start_keeps_live_daemon_when_launchctl_print_unverifiable_on_macos() {
    use std::os::unix::net::UnixListener;

    let dir = TempDir::new().expect("tempdir");
    let la_dir = dir.path().join("LaunchAgents");
    let plist = la_dir.join("com.technicalpickles.bowerbird.daemon.plist");
    let bin_dir = dir.path().join("bin");
    fs::create_dir_all(&la_dir).expect("la dir");
    fs::create_dir_all(&bin_dir).expect("bin dir");
    write_executable(&bin_dir.join("launchctl"), FAKE_LAUNCHCTL);
    // A no-env plist stub => start resolves the launchd-default data dir
    // ($HOME/.bowerbird); bind the live daemon socket there.
    fs::write(&plist, "<plist/>\n").expect("write plist stub");
    let data_dir = dir.path().join(".bowerbird");
    fs::create_dir_all(&data_dir).expect("data dir");
    let sock = data_dir.join("ingest.sock");
    let _listener = UnixListener::bind(&sock).expect("bind fake daemon socket");
    let log = dir.path().join("launchctl.log");

    let mut cmd = bowerbird_bin();
    cmd.arg("start")
        .env("HOME", dir.path())
        .env("BOWERBIRD_LAUNCH_AGENTS_DIR", &la_dir)
        // print is unverifiable (non-zero exit, non-absent stderr) => Unknown.
        .env("FAKE_LAUNCHCTL_PRINT_EXIT", "1")
        .env("FAKE_LAUNCHCTL_PRINT_STDERR", "Operation not permitted");
    with_fake_launchctl(&mut cmd, &bin_dir, &log);
    let assertion = cmd.assert().success();

    let stdout = String::from_utf8_lossy(&assertion.get_output().stdout).into_owned();
    assert!(
        stdout.contains("daemon already running"),
        "start must report the already-live daemon neutrally; stdout={stdout}"
    );
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr).into_owned();
    assert!(
        stderr.contains("could not be verified"),
        "start must warn that launchd supervision is unverifiable; stderr={stderr}"
    );
    let calls = fs::read_to_string(&log).unwrap_or_default();
    assert!(
        !calls.contains("bootstrap")
            && !calls.contains("kickstart")
            && !calls.contains("bootout"),
        "start must take NO launchd action over a live daemon on an unverifiable print; calls=\n{calls}"
    );
}

/// Story 5.9 review pass-4 #2: a registered plist with NO `BOWERBIRD_DATA_DIR`
/// (a legacy / no-env registration) must make `bowerbird start` look where
/// launchd actually runs the daemon — launchd's default `$HOME/.bowerbird` — NOT
/// the current CLI data dir (which the launchd process never sees). We register a
/// no-env plist, drop a STALE `server.json` into `$HOME/.bowerbird`, and run
/// `start` from a *different* CLI data dir B. `start` (pass-6 F5) clears the
/// EFFECTIVE dir's stale `server.json` before handing to launchd; if it honored
/// the launchd default it is `$HOME/.bowerbird`'s file that gets removed (not B's),
/// proving it ignored the CLI env. (Pre-pass-6 the proof was "start finds the
/// launchd-default server.json"; F5 now removes it, so the proof flips to "the
/// launchd-default server.json was cleared".)
#[cfg(target_os = "macos")]
#[test]
fn start_uses_launchd_default_dir_for_no_env_plist_on_macos() {
    let dir = TempDir::new().expect("tempdir");
    let la_dir = dir.path().join("LaunchAgents");
    let plist = la_dir.join("com.technicalpickles.bowerbird.daemon.plist");
    let dir_b = dir.path().join("data-b");
    let bin_dir = dir.path().join("bin");
    let launchd_default = dir.path().join(".bowerbird");
    fs::create_dir_all(&la_dir).expect("la dir");
    fs::create_dir_all(&dir_b).expect("data-b");
    fs::create_dir_all(&bin_dir).expect("bin dir");
    fs::create_dir_all(&launchd_default).expect("launchd default dir");
    write_executable(&bin_dir.join("launchctl"), FAKE_LAUNCHCTL);
    // No-env plist stub (no EnvironmentVariables block; also no ProgramArguments,
    // so start's F6 launchable check is skipped).
    fs::write(&plist, "<plist/>\n").expect("write plist stub");
    // Stale server.json in launchd's default dir.
    fs::write(
        launchd_default.join("server.json"),
        r#"{"bind_addr":"127.0.0.1:1","token":"x"}"#,
    )
    .expect("write stale server.json into launchd default");

    let log = dir.path().join("launchctl.log");
    let mut cmd = bowerbird_bin();
    cmd.arg("start")
        .env("HOME", dir.path())
        .env("BOWERBIRD_DATA_DIR", &dir_b) // CLI env dir B — must be ignored
        .env("BOWERBIRD_LAUNCH_AGENTS_DIR", &la_dir)
        .env("FAKE_LAUNCHCTL_PRINT_EXIT", "1") // not loaded -> bootstrap
        .env("FAKE_LAUNCHCTL_BOOTSTRAP_EXIT", "0");
    with_fake_launchctl(&mut cmd, &bin_dir, &log);
    // Readiness times out (the fake daemon writes no fresh server.json).
    cmd.assert().failure();

    // F2 + F5: start resolved the effective dir to the launchd default and cleared
    // its stale server.json; the CLI env dir B was never touched.
    assert!(
        !launchd_default.join("server.json").exists(),
        "start must resolve the effective dir to launchd's default $HOME/.bowerbird and clear its \
         stale server.json (ignoring the CLI env dir B)"
    );
    assert!(
        !dir_b.join("server.json").exists(),
        "the test must not have created server.json in B"
    );
}

/// Story 5.9 review pass-4 #3: macOS `uninstall` must resolve the data dir /
/// ingest socket for its manual-daemon fallback from the LaunchAgent's REGISTERED
/// env, not the current CLI env. Install registers data dir A (plist embeds A); a
/// later uninstall run with a *different* CLI data dir B must still inspect A. We
/// bind a live daemon on A's socket (no PID file) and uninstall from B; the
/// surviving-daemon warning must name A's socket (it would be silent if uninstall
/// had probed B).
#[cfg(target_os = "macos")]
#[test]
fn uninstall_inspects_registered_data_dir_not_cli_env_on_macos() {
    use std::os::unix::net::UnixListener;

    let dir = TempDir::new().expect("tempdir");
    let settings = settings_path(&dir);
    let la_dir = dir.path().join("LaunchAgents");
    let dir_a = dir.path().join("data-a");
    let dir_b = dir.path().join("data-b");
    let bin_dir = dir.path().join("bin");
    fs::create_dir_all(&dir_a).expect("data-a");
    fs::create_dir_all(&dir_b).expect("data-b");
    fs::create_dir_all(&bin_dir).expect("bin dir");
    write_executable(&bin_dir.join("launchctl"), FAKE_LAUNCHCTL);
    let log = dir.path().join("launchctl.log");

    // Install with data dir A (--no-start: writes the plist embedding A's
    // canonical BOWERBIRD_DATA_DIR; no real launchctl).
    bowerbird_bin()
        .arg("install")
        .arg("--settings")
        .arg(&settings)
        .arg("--no-start")
        .env("HOME", dir.path())
        .env("BOWERBIRD_LAUNCH_AGENTS_DIR", &la_dir)
        .env("BOWERBIRD_DATA_DIR", &dir_a)
        .assert()
        .success();

    // A live daemon on A's effective (canonical) socket, with NO PID file.
    let dir_a_canon = fs::canonicalize(&dir_a).expect("canonicalize data-a");
    let sock_a = dir_a_canon.join("ingest.sock");
    let _listener = UnixListener::bind(&sock_a).expect("bind fake daemon socket on A");

    // Uninstall from CLI data dir B. F3: it must read A from the plist env.
    let mut cmd = bowerbird_bin();
    cmd.arg("uninstall")
        .arg("--settings")
        .arg(&settings)
        .env("HOME", dir.path())
        .env("BOWERBIRD_DATA_DIR", &dir_b)
        .env("BOWERBIRD_LAUNCH_AGENTS_DIR", &la_dir)
        .env("FAKE_LAUNCHCTL_BOOTOUT_EXIT", "0");
    with_fake_launchctl(&mut cmd, &bin_dir, &log);
    let assertion = cmd.assert().success();

    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr).into_owned();
    assert!(
        stderr.contains("still accepting on") && stderr.contains(&sock_a.display().to_string()),
        "uninstall must probe A's registered socket (not CLI env B) and warn naming it; \
         stderr={stderr}"
    );
}

/// Story 5.9 review pass-4 #4 (install side): a stale/wrong PID file or PID reuse
/// can make `stop_daemon_via_pid_file` report a nominal `Stopped` while the real
/// daemon socket is still accepting. Install must re-probe the socket after ANY
/// stop outcome and refuse to bootstrap launchd over a daemon it cannot manage —
/// the previous code only bailed on `NotRunning` + a live socket. We point the
/// PID file at a killable `sleep` process (reaped by a helper thread so the stop
/// sees a clean `Stopped`) and hold a separate live listener on the socket.
#[cfg(target_os = "macos")]
#[test]
fn install_fails_when_socket_live_after_clean_stop_on_macos() {
    use std::os::unix::net::UnixListener;

    let dir = TempDir::new().expect("tempdir");
    let settings = settings_path(&dir);
    let la_dir = dir.path().join("LaunchAgents");
    let data_dir = dir.path().join("data");
    let bin_dir = dir.path().join("bin");
    fs::create_dir_all(&data_dir).expect("data dir");
    fs::create_dir_all(&bin_dir).expect("bin dir");
    write_executable(&bin_dir.join("launchctl"), FAKE_LAUNCHCTL);
    let daemon = bin_dir.join("bowerbird-daemon");
    write_executable(&daemon, "#!/bin/sh\nexit 0\n");
    let log = dir.path().join("launchctl.log");

    // A live listener on the effective (canonical) socket.
    let data_dir_canon = fs::canonicalize(&data_dir).expect("canonicalize data dir");
    let sock = data_dir_canon.join("ingest.sock");
    let _listener = UnixListener::bind(&sock).expect("bind live socket");

    // A REAL singleton holder (flock) the stop SIGTERMs to a clean `Stopped`; a
    // helper thread reaps it. The flock probe (pass-6 F4) ignores a plain
    // sleep+pid-file as stale, so we must hold an actual flock — and the separate
    // live listener above keeps the socket up after the holder is stopped.
    let mut child = spawn_flock_holder(&data_dir_canon.join("bowerbird.pid"));
    let reaper = std::thread::spawn(move || {
        let _ = child.wait();
    });

    let mut cmd = bowerbird_bin();
    cmd.arg("install")
        .arg("--settings")
        .arg(&settings)
        // NOTE: no --no-start — drive the existing-daemon handoff.
        .env("HOME", dir.path())
        .env("BOWERBIRD_DATA_DIR", &data_dir)
        .env("BOWERBIRD_LAUNCH_AGENTS_DIR", &la_dir)
        .env("BOWERBIRD_DAEMON_BIN", &daemon)
        .env("FAKE_LAUNCHCTL_PRINT_EXIT", "1") // agent not loaded -> no bootout
        .env("FAKE_LAUNCHCTL_BOOTSTRAP_EXIT", "0");
    with_fake_launchctl(&mut cmd, &bin_dir, &log);
    let assertion = cmd.assert().failure();
    reaper.join().ok();

    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr).into_owned();
    assert!(
        stderr.contains("still accepting on") && stderr.contains("after attempting to stop"),
        "install must refuse to bootstrap over a socket still live after the stop; stderr={stderr}"
    );
    let calls = fs::read_to_string(&log).unwrap_or_default();
    assert!(
        !calls.contains("bootstrap"),
        "install must NOT bootstrap when the socket survives the stop; calls=\n{calls}"
    );
}

/// Story 5.9 review pass-5 F2 (install side): the daemon takes the singleton
/// (PID file) BEFORE binding the socket, so a holder can be alive with the
/// ingest socket DOWN (wedged before bind / bound to a previous socket). Install
/// must still stop that holder before bootstrap — the previous code only ran the
/// handoff when the socket probe succeeded, so a socket-down holder was invisible
/// and launchd would bootstrap into a singleton-lock crash loop. Here the holder
/// is killable (reaped so the stop sees a clean `Stopped`), so install stops it
/// and proceeds to bootstrap.
#[cfg(target_os = "macos")]
#[test]
fn install_stops_singleton_holder_when_socket_down_on_macos() {
    let dir = TempDir::new().expect("tempdir");
    let settings = settings_path(&dir);
    let la_dir = dir.path().join("LaunchAgents");
    let data_dir = dir.path().join("data");
    let bin_dir = dir.path().join("bin");
    fs::create_dir_all(&data_dir).expect("data dir");
    fs::create_dir_all(&bin_dir).expect("bin dir");
    write_executable(&bin_dir.join("launchctl"), FAKE_LAUNCHCTL);
    let daemon = bin_dir.join("bowerbird-daemon");
    write_executable(&daemon, "#!/bin/sh\nexit 0\n");
    let log = dir.path().join("launchctl.log");

    // NO socket listener: the ingest socket is DOWN. But a REAL singleton holder
    // (flock on bowerbird.pid). A plain sleep + hand-written pid file would be
    // ignored as stale by the flock probe (pass-6 F4).
    let data_dir_canon = fs::canonicalize(&data_dir).expect("canonicalize data dir");
    let mut child = spawn_flock_holder(&data_dir_canon.join("bowerbird.pid"));
    let reaper = std::thread::spawn(move || {
        let _ = child.wait();
    });

    let mut cmd = bowerbird_bin();
    cmd.arg("install")
        .arg("--settings")
        .arg(&settings)
        .env("HOME", dir.path())
        .env("BOWERBIRD_DATA_DIR", &data_dir)
        .env("BOWERBIRD_LAUNCH_AGENTS_DIR", &la_dir)
        .env("BOWERBIRD_DAEMON_BIN", &daemon)
        .env("FAKE_LAUNCHCTL_PRINT_EXIT", "1") // agent not loaded -> no bootout
        .env("FAKE_LAUNCHCTL_BOOTSTRAP_EXIT", "0");
    with_fake_launchctl(&mut cmd, &bin_dir, &log);
    let assertion = cmd.assert().success();
    reaper.join().ok();

    let stdout = String::from_utf8_lossy(&assertion.get_output().stdout).into_owned();
    assert!(
        stdout.contains("stopping the running unsupervised daemon"),
        "install must stop a singleton holder even when the socket is down; stdout={stdout}"
    );
    let calls = fs::read_to_string(&log).unwrap_or_default();
    assert!(
        calls.contains("bootstrap"),
        "install must bootstrap once the holder is stopped; calls=\n{calls}"
    );
}

/// Story 5.9 review pass-5 F2 (start side): same singleton-before-socket gap for
/// `bowerbird start`. With the socket DOWN but a live PID holder, start must stop
/// the holder before driving launchd (kickstart/bootstrap), so the launchd start
/// has a free singleton instead of crash-looping. The holder is killable
/// (reaped), so start stops it then kickstarts the loaded agent (readiness then
/// times out — no real daemon — but the stop + kickstart are what we verify).
#[cfg(target_os = "macos")]
#[test]
fn start_stops_singleton_holder_when_socket_down_on_macos() {
    let la_parent = TempDir::new().expect("tempdir");
    let la_dir = la_parent.path().join("LaunchAgents");
    let plist = la_dir.join("com.technicalpickles.bowerbird.daemon.plist");
    let bin_dir = la_parent.path().join("bin");
    // No-env plist => start resolves effective dir to launchd default $HOME/.bowerbird.
    let bowerbird_home = la_parent.path().join(".bowerbird");
    fs::create_dir_all(&la_dir).expect("la dir");
    fs::create_dir_all(&bin_dir).expect("bin dir");
    fs::create_dir_all(&bowerbird_home).expect("bowerbird home");
    write_executable(&bin_dir.join("launchctl"), FAKE_LAUNCHCTL);
    fs::write(&plist, "<plist/>\n").expect("write plist stub");
    let log = la_parent.path().join("launchctl.log");

    // Socket down (no listener); a REAL singleton holder (flock) in the
    // launchd-default dir. A plain sleep + pid file would be ignored as stale by
    // the flock probe (pass-6 F4).
    let mut child = spawn_flock_holder(&bowerbird_home.join("bowerbird.pid"));
    let reaper = std::thread::spawn(move || {
        let _ = child.wait();
    });

    let mut cmd = bowerbird_bin();
    cmd.arg("start")
        .env("HOME", la_parent.path())
        .env("BOWERBIRD_LAUNCH_AGENTS_DIR", &la_dir)
        .env("FAKE_LAUNCHCTL_PRINT_EXIT", "0") // agent loaded -> kickstart path
        .env("FAKE_LAUNCHCTL_KICKSTART_EXIT", "0");
    with_fake_launchctl(&mut cmd, &bin_dir, &log);
    let assertion = cmd.assert().failure(); // readiness times out; no real daemon
    reaper.join().ok();

    let stdout = String::from_utf8_lossy(&assertion.get_output().stdout).into_owned();
    assert!(
        stdout.contains("sending SIGTERM to bowerbird-daemon"),
        "start must stop a singleton holder even when the socket is down; stdout={stdout}"
    );
    let calls = fs::read_to_string(&log).unwrap_or_default();
    assert!(
        calls.contains("kickstart"),
        "start must drive launchd (kickstart) once the holder is stopped; calls=\n{calls}"
    );
}

/// Story 5.9 review pass-4 #4 (uninstall side): uninstall must re-probe the socket
/// after ANY stop outcome and warn if a daemon survived — the previous code only
/// probed when the outcome was NOT `Stopped`/`Escalated`, so a stale PID file that
/// produced a nominal `Stopped` while the real daemon kept accepting was reported
/// as a clean removal. Same `sleep`-reaper + live-listener setup as the install
/// case.
#[cfg(target_os = "macos")]
#[test]
fn uninstall_warns_when_socket_live_after_clean_stop_on_macos() {
    use std::os::unix::net::UnixListener;

    let dir = TempDir::new().expect("tempdir");
    let settings = settings_path(&dir);
    let la_dir = dir.path().join("LaunchAgents");
    let data_dir = dir.path().join("data");
    let bin_dir = dir.path().join("bin");
    fs::create_dir_all(&data_dir).expect("data dir");
    fs::create_dir_all(&bin_dir).expect("bin dir");
    write_executable(&bin_dir.join("launchctl"), FAKE_LAUNCHCTL);
    let log = dir.path().join("launchctl.log");

    // Register the plist (embeds canonical data dir; launchctl-free).
    bowerbird_bin()
        .arg("install")
        .arg("--settings")
        .arg(&settings)
        .arg("--no-start")
        .env("HOME", dir.path())
        .env("BOWERBIRD_LAUNCH_AGENTS_DIR", &la_dir)
        .env("BOWERBIRD_DATA_DIR", &data_dir)
        .assert()
        .success();

    let data_dir_canon = fs::canonicalize(&data_dir).expect("canonicalize data dir");
    let sock = data_dir_canon.join("ingest.sock");
    let _listener = UnixListener::bind(&sock).expect("bind live socket");

    // A REAL singleton holder (flock) the manual-daemon fallback SIGTERMs to a
    // clean `Stopped`; the separate live listener above keeps the socket up
    // afterward so uninstall must still warn. The flock probe (pass-6 F4) ignores
    // a plain sleep+pid-file as stale.
    let mut child = spawn_flock_holder(&data_dir_canon.join("bowerbird.pid"));
    let reaper = std::thread::spawn(move || {
        let _ = child.wait();
    });

    let mut cmd = bowerbird_bin();
    cmd.arg("uninstall")
        .arg("--settings")
        .arg(&settings)
        .env("HOME", dir.path())
        .env("BOWERBIRD_DATA_DIR", &data_dir)
        .env("BOWERBIRD_LAUNCH_AGENTS_DIR", &la_dir)
        .env("FAKE_LAUNCHCTL_BOOTOUT_EXIT", "0");
    with_fake_launchctl(&mut cmd, &bin_dir, &log);
    let assertion = cmd.assert().success();
    reaper.join().ok();

    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr).into_owned();
    assert!(
        stderr.contains("still accepting on"),
        "uninstall must warn that a daemon survived a nominal Stopped; stderr={stderr}"
    );
}

/// Story 5.9 review pass-4 #5: install must create (or fail before writing) the
/// parent directory of an absolute custom `BOWERBIRD_INGEST_SOCK`. The daemon's
/// bind path does not create the socket parent, so an absolute socket under a
/// missing parent would let bootstrap "succeed" while the daemon crash-loops on
/// bind. We point at an absolute socket under a missing parent and assert install
/// creates it (even with `--no-start`, since the plist is a future registration)
/// and embeds the socket.
#[cfg(target_os = "macos")]
#[test]
fn install_creates_missing_parent_for_custom_ingest_sock_on_macos() {
    let dir = TempDir::new().expect("tempdir");
    let settings = settings_path(&dir);
    let la_dir = dir.path().join("LaunchAgents");
    let plist = la_dir.join("com.technicalpickles.bowerbird.daemon.plist");
    let missing_parent = dir.path().join("missing-parent");
    let sock = missing_parent.join("ingest.sock");
    assert!(!missing_parent.exists(), "parent must start missing");

    bowerbird_bin()
        .arg("install")
        .arg("--settings")
        .arg(&settings)
        .arg("--no-start")
        .env("HOME", dir.path())
        .env("BOWERBIRD_LAUNCH_AGENTS_DIR", &la_dir)
        .env("BOWERBIRD_DAEMON_BIN", "/usr/local/bin/bowerbird-daemon")
        .env("BOWERBIRD_INGEST_SOCK", &sock)
        .assert()
        .success();

    assert!(
        missing_parent.is_dir(),
        "install must create the missing parent of a custom ingest socket"
    );
    let xml = fs::read_to_string(&plist).expect("read plist");
    assert!(
        xml.contains(&format!("<string>{}</string>", sock.display())),
        "the absolute custom socket must be embedded in the plist; xml={xml}"
    );
}

/// CLI surface check: the `bowerbird --help` output mentions both subcommands
/// story 3.1 wires (`install`, `uninstall`). Catches regressions where the
/// clap derive grows a typo or a subcommand is accidentally dropped.
#[test]
fn help_lists_install_and_uninstall_subcommands() {
    let assertion = bowerbird_bin().arg("--help").assert().success();
    let stdout = String::from_utf8_lossy(&assertion.get_output().stdout).into_owned();
    assert!(
        stdout.contains("install"),
        "help missing `install`:\n{stdout}"
    );
    assert!(
        stdout.contains("uninstall"),
        "help missing `uninstall`:\n{stdout}"
    );
}

// --- Story 5.9 review pass-6 -----------------------------------------------

/// Pass-6 F1: `bowerbird stop` must stop a launchd-supervised daemon by booting
/// the agent OUT, not by PID-file SIGTERM/SIGKILL. A SIGKILL-escalated stop is a
/// non-zero exit, which `KeepAlive={SuccessfulExit=false}` reads as a crash and
/// restarts — so a forced stop would bounce right back. Bootout removes the job
/// from the domain, so there is nothing for KeepAlive to restart.
#[cfg(target_os = "macos")]
#[test]
fn stop_boots_out_loaded_launch_agent_on_macos() {
    let dir = TempDir::new().expect("tempdir");
    let la_dir = dir.path().join("LaunchAgents");
    let plist = la_dir.join("com.technicalpickles.bowerbird.daemon.plist");
    let bin_dir = dir.path().join("bin");
    fs::create_dir_all(&la_dir).expect("la dir");
    fs::create_dir_all(&bin_dir).expect("bin dir");
    write_executable(&bin_dir.join("launchctl"), FAKE_LAUNCHCTL);
    fs::write(&plist, "<plist/>\n").expect("write plist stub");
    let log = dir.path().join("launchctl.log");

    let mut cmd = bowerbird_bin();
    cmd.arg("stop")
        .env("HOME", dir.path())
        .env("BOWERBIRD_LAUNCH_AGENTS_DIR", &la_dir)
        .env("FAKE_LAUNCHCTL_PRINT_EXIT", "0") // agent loaded
        .env("FAKE_LAUNCHCTL_BOOTOUT_EXIT", "0");
    with_fake_launchctl(&mut cmd, &bin_dir, &log);
    let assertion = cmd.assert().success();

    let stdout = String::from_utf8_lossy(&assertion.get_output().stdout).into_owned();
    assert!(
        stdout.contains("daemon stopped") && stdout.contains("supervision paused"),
        "stop must report a launchd-owned stop; stdout={stdout}"
    );
    let calls = fs::read_to_string(&log).unwrap_or_default();
    assert!(
        calls.contains("bootout"),
        "stop must boot the loaded agent out (not SIGKILL it into a KeepAlive respawn); calls=\n{calls}"
    );
}

/// Pass-6 F1 (fallback side): when a plist exists but the agent is NOT loaded,
/// `bowerbird stop` falls back to the PID-file path and never boots anything out.
#[cfg(target_os = "macos")]
#[test]
fn stop_falls_back_to_pid_file_when_agent_not_loaded_on_macos() {
    let dir = TempDir::new().expect("tempdir");
    let la_dir = dir.path().join("LaunchAgents");
    let plist = la_dir.join("com.technicalpickles.bowerbird.daemon.plist");
    let bin_dir = dir.path().join("bin");
    let data_dir = dir.path().join(".bowerbird");
    fs::create_dir_all(&la_dir).expect("la dir");
    fs::create_dir_all(&bin_dir).expect("bin dir");
    fs::create_dir_all(&data_dir).expect("data dir");
    write_executable(&bin_dir.join("launchctl"), FAKE_LAUNCHCTL);
    fs::write(&plist, "<plist/>\n").expect("write plist stub");
    let log = dir.path().join("launchctl.log");

    let mut cmd = bowerbird_bin();
    cmd.arg("stop")
        .env("HOME", dir.path())
        .env("BOWERBIRD_LAUNCH_AGENTS_DIR", &la_dir)
        .env("FAKE_LAUNCHCTL_PRINT_EXIT", "1"); // agent not loaded -> PID-file path
    with_fake_launchctl(&mut cmd, &bin_dir, &log);
    let assertion = cmd.assert().success();

    let stdout = String::from_utf8_lossy(&assertion.get_output().stdout).into_owned();
    assert!(
        stdout.contains("daemon not running"),
        "stop must take the PID-file path (no daemon to stop here); stdout={stdout}"
    );
    let calls = fs::read_to_string(&log).unwrap_or_default();
    assert!(
        !calls.contains("bootout"),
        "stop must NOT bootout when the agent is not loaded; calls=\n{calls}"
    );
}

/// Pass-6 F2 (install side): when modern `bootstrap` AND `launchctl print` are
/// unsupported/unverifiable but legacy `load -w` works, install must still reach
/// the legacy fallback rather than bailing at the `launch_agent_load_state()`
/// preflight. The preflight is now tri-state, so an unverifiable `print` becomes
/// `Unknown` and the launch helpers attempt modern-then-legacy.
#[cfg(target_os = "macos")]
#[test]
fn install_uses_legacy_load_when_bootstrap_and_print_unsupported_on_macos() {
    let dir = TempDir::new().expect("tempdir");
    let settings = settings_path(&dir);
    let la_dir = dir.path().join("LaunchAgents");
    let bin_dir = dir.path().join("bin");
    fs::create_dir_all(&bin_dir).expect("bin dir");
    write_executable(&bin_dir.join("launchctl"), FAKE_LAUNCHCTL);
    let daemon = bin_dir.join("bowerbird-daemon");
    write_executable(&daemon, "#!/bin/sh\nexit 0\n");
    let log = dir.path().join("launchctl.log");

    let mut cmd = bowerbird_bin();
    cmd.arg("install")
        .arg("--settings")
        .arg(&settings)
        .env("HOME", dir.path())
        .env("BOWERBIRD_LAUNCH_AGENTS_DIR", &la_dir)
        .env("BOWERBIRD_DAEMON_BIN", &daemon)
        // print unverifiable => Unknown; modern bootstrap/bootout fail; legacy works.
        .env("FAKE_LAUNCHCTL_PRINT_EXIT", "1")
        .env("FAKE_LAUNCHCTL_PRINT_STDERR", "Operation not permitted")
        .env("FAKE_LAUNCHCTL_BOOTOUT_EXIT", "1")
        .env("FAKE_LAUNCHCTL_UNLOAD_EXIT", "0")
        .env("FAKE_LAUNCHCTL_BOOTSTRAP_EXIT", "1")
        .env("FAKE_LAUNCHCTL_LOAD_EXIT", "0");
    with_fake_launchctl(&mut cmd, &bin_dir, &log);
    cmd.assert().success();

    let calls = fs::read_to_string(&log).unwrap_or_default();
    assert!(
        calls.contains("load -w"),
        "install must fall back to legacy `load -w` when bootstrap+print are unsupported; calls=\n{calls}"
    );
}

/// Pass-6 F2 (start side): same legacy-fallback path for `bowerbird start` — an
/// unverifiable `print` must not bail before the modern-then-legacy launch helper.
#[cfg(target_os = "macos")]
#[test]
fn start_uses_legacy_load_when_bootstrap_and_print_unsupported_on_macos() {
    let dir = TempDir::new().expect("tempdir");
    let la_dir = dir.path().join("LaunchAgents");
    let plist = la_dir.join("com.technicalpickles.bowerbird.daemon.plist");
    let bin_dir = dir.path().join("bin");
    fs::create_dir_all(&la_dir).expect("la dir");
    fs::create_dir_all(&bin_dir).expect("bin dir");
    write_executable(&bin_dir.join("launchctl"), FAKE_LAUNCHCTL);
    fs::write(&plist, "<plist/>\n").expect("write plist stub"); // no ProgramArguments => F6 skipped
    let log = dir.path().join("launchctl.log");

    let mut cmd = bowerbird_bin();
    cmd.arg("start")
        .env("HOME", dir.path())
        .env("BOWERBIRD_LAUNCH_AGENTS_DIR", &la_dir)
        .env("FAKE_LAUNCHCTL_PRINT_EXIT", "1")
        .env("FAKE_LAUNCHCTL_PRINT_STDERR", "Operation not permitted") // Unknown
        .env("FAKE_LAUNCHCTL_BOOTSTRAP_EXIT", "1")
        .env("FAKE_LAUNCHCTL_LOAD_EXIT", "0");
    with_fake_launchctl(&mut cmd, &bin_dir, &log);
    // Readiness times out (no real daemon), but the legacy load must have run.
    cmd.assert().failure();

    let calls = fs::read_to_string(&log).unwrap_or_default();
    assert!(
        calls.contains("bootstrap") && calls.contains("load -w"),
        "start must attempt modern bootstrap then legacy `load -w` on Unknown; calls=\n{calls}"
    );
}

/// Pass-6 F3 / pass-7 F5 (loaded side): a live socket whose LaunchAgent is loaded
/// is "already running under launchd" ONLY when launchd is actually RUNNING the
/// job AND its reported pid is the singleton holder. A loaded label alone is not
/// proof (pass-7 F5), so this test now also stands up a real flock holder whose
/// pid matches launchd's reported pid. `start` reports it and takes no launchd
/// action.
#[cfg(target_os = "macos")]
#[test]
fn start_reports_supervised_when_loaded_and_socket_live_on_macos() {
    use std::os::unix::net::UnixListener;

    let dir = TempDir::new().expect("tempdir");
    let la_dir = dir.path().join("LaunchAgents");
    let plist = la_dir.join("com.technicalpickles.bowerbird.daemon.plist");
    let bin_dir = dir.path().join("bin");
    let data_dir = dir.path().join(".bowerbird");
    fs::create_dir_all(&la_dir).expect("la dir");
    fs::create_dir_all(&bin_dir).expect("bin dir");
    fs::create_dir_all(&data_dir).expect("data dir");
    write_executable(&bin_dir.join("launchctl"), FAKE_LAUNCHCTL);
    fs::write(&plist, "<plist/>\n").expect("write plist stub");
    let _listener = UnixListener::bind(data_dir.join("ingest.sock")).expect("bind socket");
    let log = dir.path().join("launchctl.log");

    // A REAL singleton holder (flock + its pid in `bowerbird.pid`), and launchd's
    // reported pid set to that same pid — the only state that proves supervision.
    let mut holder = spawn_flock_holder(&data_dir.join("bowerbird.pid"));
    let holder_pid = holder.id();

    let mut cmd = bowerbird_bin();
    cmd.arg("start")
        .env("HOME", dir.path())
        .env("BOWERBIRD_LAUNCH_AGENTS_DIR", &la_dir)
        .env("FAKE_LAUNCHCTL_PRINT_EXIT", "0") // loaded
        .env("FAKE_LAUNCHCTL_PRINT_PID", holder_pid.to_string()); // running, pid == holder
    with_fake_launchctl(&mut cmd, &bin_dir, &log);
    let assertion = cmd.assert().success();

    holder.kill().ok();
    holder.wait().ok();

    let stdout = String::from_utf8_lossy(&assertion.get_output().stdout).into_owned();
    assert!(
        stdout.contains("already running under launchd"),
        "start must report a supervised live daemon as launchd-owned; stdout={stdout}"
    );
    let calls = fs::read_to_string(&log).unwrap_or_default();
    assert!(
        !calls.contains("bootstrap") && !calls.contains("kickstart"),
        "start must take no launchd action for an already-supervised daemon; calls=\n{calls}"
    );
}

/// Pass-7 F5: a loaded LaunchAgent label is NOT proof launchd owns the live
/// socket. Repro: the agent stays loaded after a clean daemon exit, then a manual
/// / pre-5.9 daemon binds the registered socket. launchd reports the job is not
/// running (no `pid =` line), so `start` must NOT report "already running under
/// launchd" — it must try to migrate the manual daemon and fail clearly when it
/// cannot stop the socket owner, rather than leaving it unsupervised.
#[cfg(target_os = "macos")]
#[test]
fn start_does_not_claim_supervision_when_loaded_but_not_running_on_macos() {
    use std::os::unix::net::UnixListener;

    let dir = TempDir::new().expect("tempdir");
    let la_dir = dir.path().join("LaunchAgents");
    let plist = la_dir.join("com.technicalpickles.bowerbird.daemon.plist");
    let bin_dir = dir.path().join("bin");
    let data_dir = dir.path().join(".bowerbird");
    fs::create_dir_all(&la_dir).expect("la dir");
    fs::create_dir_all(&bin_dir).expect("bin dir");
    fs::create_dir_all(&data_dir).expect("data dir");
    write_executable(&bin_dir.join("launchctl"), FAKE_LAUNCHCTL);
    fs::write(&plist, "<plist/>\n").expect("write plist stub");
    // A manual daemon owns the live socket; NO flock holder (singleton Free), and
    // launchd reports the job loaded-but-NOT-running (PRINT_EXIT=0, no pid line).
    let _listener = UnixListener::bind(data_dir.join("ingest.sock")).expect("bind socket");
    let log = dir.path().join("launchctl.log");

    let mut cmd = bowerbird_bin();
    cmd.arg("start")
        .env("HOME", dir.path())
        .env("BOWERBIRD_LAUNCH_AGENTS_DIR", &la_dir)
        .env("FAKE_LAUNCHCTL_PRINT_EXIT", "0"); // loaded, but no FAKE_LAUNCHCTL_PRINT_PID => not running
    with_fake_launchctl(&mut cmd, &bin_dir, &log);
    let assertion = cmd.assert().failure();

    let stdout = String::from_utf8_lossy(&assertion.get_output().stdout).into_owned();
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr).into_owned();
    assert!(
        !stdout.contains("already running under launchd"),
        "start must NOT claim launchd supervision when launchd is not running the job; stdout={stdout}"
    );
    assert!(
        stdout.contains("migrating it to launchd supervision"),
        "start must attempt to migrate the unsupervised manual daemon; stdout={stdout}"
    );
    assert!(
        stderr.contains("still accepting"),
        "start must fail clearly when it cannot stop the manual daemon; stderr={stderr}"
    );
    let calls = fs::read_to_string(&log).unwrap_or_default();
    assert!(
        !calls.contains("bootstrap") && !calls.contains("kickstart"),
        "start must NOT hand launchd a daemon it could not migrate; calls=\n{calls}"
    );
}

// --- Story 5.9 review pass-7 -----------------------------------------------

/// Pass-7 F1 (missing-plist loaded agent): `uninstall --no-stop` removes the plist
/// while an already-loaded in-session agent keeps being supervised by launchd. A
/// later `bowerbird stop` must still boot that agent out — probing launchd by
/// LABEL, not by plist presence — instead of skipping launchd and SIGKILL-ing the
/// daemon into a KeepAlive respawn.
#[cfg(target_os = "macos")]
#[test]
fn stop_boots_out_loaded_agent_when_plist_absent_on_macos() {
    let dir = TempDir::new().expect("tempdir");
    let la_dir = dir.path().join("LaunchAgents");
    let bin_dir = dir.path().join("bin");
    fs::create_dir_all(&la_dir).expect("la dir");
    fs::create_dir_all(&bin_dir).expect("bin dir");
    write_executable(&bin_dir.join("launchctl"), FAKE_LAUNCHCTL);
    // Deliberately do NOT write the plist file (uninstall --no-stop removed it).
    let log = dir.path().join("launchctl.log");

    let mut cmd = bowerbird_bin();
    cmd.arg("stop")
        .env("HOME", dir.path())
        .env("BOWERBIRD_LAUNCH_AGENTS_DIR", &la_dir)
        .env("FAKE_LAUNCHCTL_PRINT_EXIT", "0") // agent still loaded in-session
        .env("FAKE_LAUNCHCTL_BOOTOUT_EXIT", "0");
    with_fake_launchctl(&mut cmd, &bin_dir, &log);
    let assertion = cmd.assert().success();

    let stdout = String::from_utf8_lossy(&assertion.get_output().stdout).into_owned();
    assert!(
        stdout.contains("supervision paused"),
        "stop must boot the loaded agent out even with the plist gone; stdout={stdout}"
    );
    let calls = fs::read_to_string(&log).unwrap_or_default();
    assert!(
        calls.contains("bootout"),
        "stop must address launchd by label (bootout) when loaded and the plist is absent; calls=\n{calls}"
    );
}

/// Pass-7 F1 (unverifiable-print bootout fallback): when launchd's load state
/// cannot be verified (`Unknown`), `bowerbird stop` must PREFER a bootout (it
/// addresses the agent by label and no-ops cleanly if absent) before the PID-file
/// fallback, so a launchd-supervised daemon is not SIGKILL-ed into a KeepAlive
/// respawn.
#[cfg(target_os = "macos")]
#[test]
fn stop_attempts_bootout_on_unverifiable_print_on_macos() {
    let dir = TempDir::new().expect("tempdir");
    let la_dir = dir.path().join("LaunchAgents");
    let plist = la_dir.join("com.technicalpickles.bowerbird.daemon.plist");
    let bin_dir = dir.path().join("bin");
    fs::create_dir_all(&la_dir).expect("la dir");
    fs::create_dir_all(&bin_dir).expect("bin dir");
    write_executable(&bin_dir.join("launchctl"), FAKE_LAUNCHCTL);
    fs::write(&plist, "<plist/>\n").expect("write plist stub");
    let log = dir.path().join("launchctl.log");

    let mut cmd = bowerbird_bin();
    cmd.arg("stop")
        .env("HOME", dir.path())
        .env("BOWERBIRD_LAUNCH_AGENTS_DIR", &la_dir)
        // print unverifiable => Unknown; bootout succeeds.
        .env("FAKE_LAUNCHCTL_PRINT_EXIT", "1")
        .env("FAKE_LAUNCHCTL_PRINT_STDERR", "Operation not permitted")
        .env("FAKE_LAUNCHCTL_BOOTOUT_EXIT", "0");
    with_fake_launchctl(&mut cmd, &bin_dir, &log);
    cmd.assert().success();

    let calls = fs::read_to_string(&log).unwrap_or_default();
    assert!(
        calls.contains("bootout"),
        "stop must attempt a bootout on an unverifiable load state before the PID-file fallback; calls=\n{calls}"
    );
}

/// Pass-7 F2: an unreadable `bowerbird.pid` is NOT proof the singleton is free.
/// `install` must classify it as a held-but-unidentifiable singleton and refuse to
/// bootstrap launchd over it, rather than collapsing the open error into `Free` and
/// crash-looping the launchd daemon against the singleton lock.
#[cfg(target_os = "macos")]
#[test]
fn install_refuses_when_pid_file_unreadable_on_macos() {
    use std::os::unix::fs::PermissionsExt;

    let dir = TempDir::new().expect("tempdir");
    let settings = settings_path(&dir);
    let la_dir = dir.path().join("LaunchAgents");
    let data_dir = dir.path().join("data");
    let bin_dir = dir.path().join("bin");
    fs::create_dir_all(&data_dir).expect("data dir");
    fs::create_dir_all(&bin_dir).expect("bin dir");
    write_executable(&bin_dir.join("launchctl"), FAKE_LAUNCHCTL);
    let daemon = bin_dir.join("bowerbird-daemon");
    write_executable(&daemon, "#!/bin/sh\nexit 0\n");
    let log = dir.path().join("launchctl.log");

    // An unreadable PID file: the CLI cannot open it to flock-probe, so it cannot
    // prove the singleton free. (Owner-mode 000 denies even us — we run as non-root.)
    let pid_file = data_dir.join("bowerbird.pid");
    fs::write(&pid_file, "12345\n").expect("write pid file");
    fs::set_permissions(&pid_file, fs::Permissions::from_mode(0o000)).expect("chmod 000");

    let mut cmd = bowerbird_bin();
    cmd.arg("install")
        .arg("--settings")
        .arg(&settings)
        .env("HOME", dir.path())
        .env("BOWERBIRD_DATA_DIR", &data_dir)
        .env("BOWERBIRD_LAUNCH_AGENTS_DIR", &la_dir)
        .env("BOWERBIRD_DAEMON_BIN", &daemon)
        .env("FAKE_LAUNCHCTL_PRINT_EXIT", "1") // not loaded
        .env("FAKE_LAUNCHCTL_BOOTSTRAP_EXIT", "0");
    with_fake_launchctl(&mut cmd, &bin_dir, &log);
    let assertion = cmd.assert().failure();

    // Restore perms so TempDir cleanup is unobstructed.
    fs::set_permissions(&pid_file, fs::Permissions::from_mode(0o644)).ok();

    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr).into_owned();
    assert!(
        stderr.contains("refusing to bootstrap"),
        "install must refuse to bootstrap over an unreadable (unprovable-free) singleton; stderr={stderr}"
    );
    let calls = fs::read_to_string(&log).unwrap_or_default();
    assert!(
        !calls.contains("bootstrap"),
        "install must NOT bootstrap when it cannot prove the singleton is free; calls=\n{calls}"
    );
}

/// Pass-7 F4: an effective ingest socket path longer than `sockaddr_un.sun_path`
/// would let the daemon "register" via launchd but crash-loop on bind under
/// KeepAlive. `install` must reject it up front with a clear error and write no
/// plist / invoke no launchctl.
#[cfg(target_os = "macos")]
#[test]
fn install_rejects_too_long_ingest_sock_on_macos() {
    let dir = TempDir::new().expect("tempdir");
    let settings = settings_path(&dir);
    let la_dir = dir.path().join("LaunchAgents");
    let plist = la_dir.join("com.technicalpickles.bowerbird.daemon.plist");
    let bin_dir = dir.path().join("bin");
    fs::create_dir_all(&bin_dir).expect("bin dir");
    write_executable(&bin_dir.join("launchctl"), FAKE_LAUNCHCTL);
    let daemon = bin_dir.join("bowerbird-daemon");
    write_executable(&daemon, "#!/bin/sh\nexit 0\n");
    let log = dir.path().join("launchctl.log");

    // An absolute custom socket whose total path exceeds the 103-byte sun_path
    // limit, but whose parent is a single creatable component.
    let long_sock = dir.path().join("s".repeat(90)).join("ingest.sock");
    assert!(
        long_sock.as_os_str().len() > 103,
        "test socket path must exceed the sun_path limit: {}",
        long_sock.display()
    );

    let mut cmd = bowerbird_bin();
    cmd.arg("install")
        .arg("--settings")
        .arg(&settings)
        .env("HOME", dir.path())
        .env("BOWERBIRD_LAUNCH_AGENTS_DIR", &la_dir)
        .env("BOWERBIRD_DAEMON_BIN", &daemon)
        .env("BOWERBIRD_INGEST_SOCK", &long_sock);
    with_fake_launchctl(&mut cmd, &bin_dir, &log);
    let assertion = cmd.assert().failure();

    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr).into_owned();
    assert!(
        stderr.contains("too long"),
        "install must reject a too-long ingest socket path with a clear error; stderr={stderr}"
    );
    assert!(
        !plist.exists(),
        "install must not write a plist for a too-long socket"
    );
    let calls = fs::read_to_string(&log).unwrap_or_default();
    assert!(
        calls.is_empty(),
        "install must fail before invoking launchctl for a too-long socket; calls=\n{calls}"
    );
}

/// Pass-6 F3 (unsupervised side): a live socket whose LaunchAgent is positively
/// NOT loaded is a manual / pre-5.9 daemon. `start` must NOT silently accept it as
/// "already running" (the pre-pass-6 bug that left a registered install
/// unsupervised); it must attempt to migrate it into launchd ownership and fail
/// clearly when it cannot stop the manual daemon.
#[cfg(target_os = "macos")]
#[test]
fn start_does_not_silently_accept_unsupervised_manual_daemon_on_macos() {
    use std::os::unix::net::UnixListener;

    let dir = TempDir::new().expect("tempdir");
    let la_dir = dir.path().join("LaunchAgents");
    let plist = la_dir.join("com.technicalpickles.bowerbird.daemon.plist");
    let bin_dir = dir.path().join("bin");
    let data_dir = dir.path().join(".bowerbird");
    fs::create_dir_all(&la_dir).expect("la dir");
    fs::create_dir_all(&bin_dir).expect("bin dir");
    fs::create_dir_all(&data_dir).expect("data dir");
    write_executable(&bin_dir.join("launchctl"), FAKE_LAUNCHCTL);
    fs::write(&plist, "<plist/>\n").expect("write plist stub");
    // A live socket the test holds (so it survives any stop attempt), and NO flock
    // holder — so the singleton is Free and start cannot stop the socket owner.
    let _listener = UnixListener::bind(data_dir.join("ingest.sock")).expect("bind socket");
    let log = dir.path().join("launchctl.log");

    let mut cmd = bowerbird_bin();
    cmd.arg("start")
        .env("HOME", dir.path())
        .env("BOWERBIRD_LAUNCH_AGENTS_DIR", &la_dir)
        .env("FAKE_LAUNCHCTL_PRINT_EXIT", "1"); // not loaded -> NotLoaded -> migrate
    with_fake_launchctl(&mut cmd, &bin_dir, &log);
    let assertion = cmd.assert().failure();

    let stdout = String::from_utf8_lossy(&assertion.get_output().stdout).into_owned();
    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr).into_owned();
    assert!(
        stdout.contains("migrating it to launchd supervision"),
        "start must attempt to migrate an unsupervised manual daemon; stdout={stdout}"
    );
    assert!(
        stderr.contains("still accepting"),
        "start must fail clearly when it cannot stop the manual daemon; stderr={stderr}"
    );
    let calls = fs::read_to_string(&log).unwrap_or_default();
    assert!(
        !calls.contains("bootstrap"),
        "start must NOT bootstrap launchd over a daemon it could not stop; calls=\n{calls}"
    );
}

/// Pass-6 F4 (stale-pid-reuse safety): a `bowerbird.pid` that names a LIVE but
/// unrelated process, with NO flock held, is stale. `bowerbird stop` must treat it
/// as "not running" and must NOT SIGTERM the reused pid.
#[cfg(target_os = "macos")]
#[test]
fn stop_ignores_stale_pid_file_without_flock_on_macos() {
    use nix::sys::signal::kill;
    use nix::unistd::Pid;

    let dir = TempDir::new().expect("tempdir");
    let data_dir = dir.path().join("data");
    fs::create_dir_all(&data_dir).expect("data dir");
    // No plist under HOME => stop uses the PID-file path directly.

    // A live, unrelated process whose pid we plant as a stale pid file (no flock).
    let mut victim = std::process::Command::new("sleep")
        .arg("30")
        .spawn()
        .expect("spawn victim");
    let victim_pid = victim.id() as i32;
    fs::write(data_dir.join("bowerbird.pid"), victim_pid.to_string()).expect("write stale pid");

    let mut cmd = bowerbird_bin();
    cmd.arg("stop")
        .env("HOME", dir.path())
        .env("BOWERBIRD_DATA_DIR", &data_dir);
    let assertion = cmd.assert().success();

    let stdout = String::from_utf8_lossy(&assertion.get_output().stdout).into_owned();
    assert!(
        stdout.contains("daemon not running"),
        "stop must treat a lock-free pid file as stale; stdout={stdout}"
    );
    // The unrelated process must NOT have been signaled.
    assert!(
        kill(Pid::from_raw(victim_pid), None).is_ok(),
        "stop must NOT SIGTERM a reused pid from a stale (lock-free) pid file"
    );

    victim.kill().ok();
    victim.wait().ok();
}

/// Pass-6 F4 (real holder, unusable pid): when a daemon genuinely HOLDS the
/// singleton flock but `bowerbird.pid` is corrupt (no usable pid), install must
/// refuse to bootstrap launchd over it (it cannot identify a process to stop), not
/// silently proceed into the singleton crash-loop.
#[cfg(target_os = "macos")]
#[test]
fn install_fails_when_singleton_held_with_unusable_pid_on_macos() {
    let dir = TempDir::new().expect("tempdir");
    let settings = settings_path(&dir);
    let la_dir = dir.path().join("LaunchAgents");
    let data_dir = dir.path().join("data");
    let bin_dir = dir.path().join("bin");
    fs::create_dir_all(&data_dir).expect("data dir");
    fs::create_dir_all(&bin_dir).expect("bin dir");
    write_executable(&bin_dir.join("launchctl"), FAKE_LAUNCHCTL);
    let daemon = bin_dir.join("bowerbird-daemon");
    write_executable(&daemon, "#!/bin/sh\nexit 0\n");
    let log = dir.path().join("launchctl.log");

    // A REAL flock holder; then corrupt the pid file content (the flock is
    // advisory, so a separate write does not disturb it) => HeldUnknownPid.
    let data_dir_canon = fs::canonicalize(&data_dir).expect("canonicalize data dir");
    let mut holder = spawn_flock_holder(&data_dir_canon.join("bowerbird.pid"));
    fs::write(data_dir_canon.join("bowerbird.pid"), "not-a-pid").expect("corrupt pid file");

    let mut cmd = bowerbird_bin();
    cmd.arg("install")
        .arg("--settings")
        .arg(&settings)
        .env("HOME", dir.path())
        .env("BOWERBIRD_DATA_DIR", &data_dir)
        .env("BOWERBIRD_LAUNCH_AGENTS_DIR", &la_dir)
        .env("BOWERBIRD_DAEMON_BIN", &daemon)
        .env("FAKE_LAUNCHCTL_PRINT_EXIT", "1") // not loaded
        .env("FAKE_LAUNCHCTL_BOOTSTRAP_EXIT", "0");
    with_fake_launchctl(&mut cmd, &bin_dir, &log);
    let assertion = cmd.assert().failure();

    holder.kill().ok();
    holder.wait().ok();

    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr).into_owned();
    assert!(
        stderr.contains("refusing to bootstrap"),
        "install must refuse to bootstrap over a singleton holder it cannot stop; stderr={stderr}"
    );
    let calls = fs::read_to_string(&log).unwrap_or_default();
    assert!(
        !calls.contains("bootstrap"),
        "install must NOT bootstrap over an unidentifiable singleton holder; calls=\n{calls}"
    );
}

/// Pass-6 F6: `install --no-start` may register a plist before the daemon binary
/// is in place; a later `bowerbird start` must revalidate the registered
/// ProgramArguments and fail clearly (before touching launchctl) if it is not
/// launchable, instead of handing launchd a dead job that it retries behind a bare
/// readiness timeout.
#[cfg(target_os = "macos")]
#[test]
fn start_fails_when_registered_daemon_not_executable_on_macos() {
    let dir = TempDir::new().expect("tempdir");
    let settings = settings_path(&dir);
    let la_dir = dir.path().join("LaunchAgents");
    let plist = la_dir.join("com.technicalpickles.bowerbird.daemon.plist");
    let bin_dir = dir.path().join("bin");
    fs::create_dir_all(&bin_dir).expect("bin dir");
    write_executable(&bin_dir.join("launchctl"), FAKE_LAUNCHCTL);
    // A NON-executable "daemon" (regular file, no +x), absolute path.
    let fake_daemon = dir.path().join("bowerbird-daemon-not-exec");
    fs::write(&fake_daemon, b"#!/bin/sh\n").expect("write non-exec daemon");

    // install --no-start registers the plist (the documented pre-registration
    // exception skips the launchable check at install time).
    bowerbird_bin()
        .arg("install")
        .arg("--settings")
        .arg(&settings)
        .arg("--no-start")
        .env("HOME", dir.path())
        .env("BOWERBIRD_LAUNCH_AGENTS_DIR", &la_dir)
        .env("BOWERBIRD_DAEMON_BIN", &fake_daemon)
        .assert()
        .success();
    assert!(plist.exists(), "plist registered");

    let log = dir.path().join("launchctl.log");
    let mut cmd = bowerbird_bin();
    cmd.arg("start")
        .env("HOME", dir.path())
        .env("BOWERBIRD_LAUNCH_AGENTS_DIR", &la_dir);
    with_fake_launchctl(&mut cmd, &bin_dir, &log);
    let assertion = cmd.assert().failure();

    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr).into_owned();
    assert!(
        stderr.contains("not an executable file") || stderr.contains("not launchable"),
        "start must reject a non-executable registered daemon; stderr={stderr}"
    );
    let calls = fs::read_to_string(&log).unwrap_or_default();
    assert!(
        !calls.contains("bootstrap") && !calls.contains("kickstart"),
        "start must fail before invoking launchctl; calls=\n{calls}"
    );
}
