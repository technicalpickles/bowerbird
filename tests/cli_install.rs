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

/// Fake `launchctl` script body (POSIX sh). `$1` is the subcommand.
///
/// `print` models the real `launchctl print` for an absent service: a non-zero
/// exit carries a "Could not find …" stderr by default so the CLI classifies it
/// as genuinely-absent (Ok(false)). `FAKE_LAUNCHCTL_PRINT_STDERR` overrides the
/// message so a test can simulate an *unverifiable* failure (non-absent stderr),
/// which the CLI must surface as "cannot verify" rather than "not loaded"
/// (Story 5.9 review pass-3 F1).
#[cfg(target_os = "macos")]
const FAKE_LAUNCHCTL: &str = r#"#!/bin/sh
echo "$@" >> "$FAKE_LAUNCHCTL_LOG"
case "$1" in
  print)
     code="${FAKE_LAUNCHCTL_PRINT_EXIT:-1}"
     [ "$code" -ne 0 ] && echo "${FAKE_LAUNCHCTL_PRINT_STDERR:-Could not find service in domain for gui}" >&2
     exit "$code" ;;
  bootstrap) exit "${FAKE_LAUNCHCTL_BOOTSTRAP_EXIT:-0}" ;;
  bootout)
     code="${FAKE_LAUNCHCTL_BOOTOUT_EXIT:-0}"
     [ "$code" -ne 0 ] && echo "Boot-out failed: 1: Operation not permitted" >&2
     exit "$code" ;;
  kickstart) exit "${FAKE_LAUNCHCTL_KICKSTART_EXIT:-0}" ;;
  load)      exit "${FAKE_LAUNCHCTL_LOAD_EXIT:-0}" ;;
  unload)    exit "${FAKE_LAUNCHCTL_UNLOAD_EXIT:-0}" ;;
  *)         exit 0 ;;
esac
"#;

#[cfg(target_os = "macos")]
fn write_executable(path: &std::path::Path, body: &str) {
    use std::os::unix::fs::PermissionsExt;
    fs::write(path, body).expect("write executable");
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).expect("chmod +x");
}

/// Prepend `fake_bin_dir` (holding the fake `launchctl`) to the spawned
/// process's PATH and point `FAKE_LAUNCHCTL_LOG` at `log`.
#[cfg(target_os = "macos")]
fn with_fake_launchctl(cmd: &mut Command, fake_bin_dir: &std::path::Path, log: &std::path::Path) {
    let orig = std::env::var("PATH").unwrap_or_default();
    cmd.env("PATH", format!("{}:{}", fake_bin_dir.display(), orig));
    cmd.env("FAKE_LAUNCHCTL_LOG", log);
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

/// Story 5.9 review pass-3 F2: `bowerbird start` must look for the daemon where
/// launchd will actually run it — the data dir registered in the plist's
/// `EnvironmentVariables` — not the data dir in the *current* CLI env. We
/// install with data dir A (so the plist embeds A), drop a `server.json` into A
/// pointing at an unreachable health port, then run `start` from a *different*
/// CLI data dir B. If `start` reads A's env it finds A's `server.json` and times
/// out on the unreachable healthz ("failed to become healthy"); if it had used B
/// it would instead time out waiting for a `server.json` that does not exist
/// ("did not appear"). The error message distinguishes the two.
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

    // Install with data dir A (--no-start: writes the plist, no real launchctl).
    bowerbird_bin()
        .arg("install")
        .arg("--settings")
        .arg(&settings)
        .arg("--no-start")
        .env("HOME", dir.path())
        .env("BOWERBIRD_LAUNCH_AGENTS_DIR", &la_dir)
        .env("BOWERBIRD_DATA_DIR", &dir_a)
        .env("BOWERBIRD_DAEMON_BIN", "/usr/local/bin/bowerbird-daemon")
        .assert()
        .success();

    // install canonicalizes the data dir before embedding it; write server.json
    // where the plist actually points.
    let dir_a_canon = fs::canonicalize(&dir_a).expect("canonicalize data-a");
    fs::write(
        dir_a_canon.join("server.json"),
        r#"{"bind_addr":"127.0.0.1:1","token":"x"}"#,
    )
    .expect("write server.json into A");

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
    let assertion = cmd.assert().failure();

    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr).into_owned();
    assert!(
        stderr.contains("failed to become healthy"),
        "start must find A's server.json (proving it read the plist's data dir, not the CLI \
         env); stderr={stderr}"
    );
    assert!(
        !stderr.contains("did not appear"),
        "start must NOT wait against the CLI env data dir B (where no server.json exists); \
         stderr={stderr}"
    );
    assert!(
        !dir_b.join("server.json").exists(),
        "the test must not have created server.json in B"
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

/// Story 5.9 review pass-4 #1: `bowerbird start` must probe the registered ingest
/// socket BEFORE querying launchd. If a daemon is already accepting, an
/// unverifiable `launchctl print` (a non-zero exit whose stderr is not an
/// absent-service signal — which `launch_agent_loaded` surfaces as `Err`) must
/// NOT make `start` fail: no launchd action is needed at all. We bind a live
/// listener on the registered socket and configure `print` to be unverifiable;
/// `start` must still exit 0 with the neutral "daemon already running" and never
/// invoke launchctl.
#[cfg(target_os = "macos")]
#[test]
fn start_succeeds_on_live_socket_even_when_launchctl_print_unverifiable_on_macos() {
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
        // print is unverifiable (non-zero exit, non-absent stderr); it must never
        // be reached because the live-socket probe short-circuits first.
        .env("FAKE_LAUNCHCTL_PRINT_EXIT", "1")
        .env("FAKE_LAUNCHCTL_PRINT_STDERR", "Operation not permitted");
    with_fake_launchctl(&mut cmd, &bin_dir, &log);
    let assertion = cmd.assert().success();

    let stdout = String::from_utf8_lossy(&assertion.get_output().stdout).into_owned();
    assert!(
        stdout.contains("daemon already running"),
        "start must report the already-live daemon neutrally; stdout={stdout}"
    );
    let calls = fs::read_to_string(&log).unwrap_or_default();
    assert!(
        calls.trim().is_empty(),
        "start must not invoke launchctl when the socket is already live; calls=\n{calls}"
    );
}

/// Story 5.9 review pass-4 #2: a registered plist with NO `BOWERBIRD_DATA_DIR`
/// (a legacy / no-env registration) must make `bowerbird start` look where
/// launchd actually runs the daemon — launchd's default `$HOME/.bowerbird` — NOT
/// the current CLI data dir (which the launchd process never sees). We register a
/// no-env plist, drop a `server.json` into `$HOME/.bowerbird` pointing at an
/// unreachable health port, and run `start` from a *different* CLI data dir B. If
/// start honors the launchd default it finds `$HOME/.bowerbird`'s server.json and
/// times out on healthz ("failed to become healthy"); if it wrongly used B it
/// would time out waiting for a server.json that does not exist ("did not
/// appear").
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
    // No-env plist stub (no EnvironmentVariables block).
    fs::write(&plist, "<plist/>\n").expect("write plist stub");
    // server.json in launchd's default dir, pointing at an unreachable port.
    fs::write(
        launchd_default.join("server.json"),
        r#"{"bind_addr":"127.0.0.1:1","token":"x"}"#,
    )
    .expect("write server.json into launchd default");

    let log = dir.path().join("launchctl.log");
    let mut cmd = bowerbird_bin();
    cmd.arg("start")
        .env("HOME", dir.path())
        .env("BOWERBIRD_DATA_DIR", &dir_b) // CLI env dir B — must be ignored
        .env("BOWERBIRD_LAUNCH_AGENTS_DIR", &la_dir)
        .env("FAKE_LAUNCHCTL_PRINT_EXIT", "1") // not loaded -> bootstrap
        .env("FAKE_LAUNCHCTL_BOOTSTRAP_EXIT", "0");
    with_fake_launchctl(&mut cmd, &bin_dir, &log);
    let assertion = cmd.assert().failure();

    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr).into_owned();
    assert!(
        stderr.contains("failed to become healthy"),
        "start must find the launchd-default server.json (proving it ignored the CLI env dir B); \
         stderr={stderr}"
    );
    assert!(
        !stderr.contains("did not appear"),
        "start must NOT wait against the CLI env data dir B; stderr={stderr}"
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

    // A killable process the PID file points at; a helper thread reaps it so the
    // stop sees a clean `Stopped` (bowerbird is not the process's parent and
    // could not otherwise reap the zombie).
    let mut child = std::process::Command::new("sleep")
        .arg("30")
        .spawn()
        .expect("spawn sleep");
    let pid = child.id();
    fs::write(data_dir_canon.join("bowerbird.pid"), pid.to_string()).expect("write pid file");
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

    let mut child = std::process::Command::new("sleep")
        .arg("30")
        .spawn()
        .expect("spawn sleep");
    let pid = child.id();
    fs::write(data_dir_canon.join("bowerbird.pid"), pid.to_string()).expect("write pid file");
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
