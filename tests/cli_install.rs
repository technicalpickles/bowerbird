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
