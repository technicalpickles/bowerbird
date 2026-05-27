//! Contract tests for `adapter_claude::install` / `uninstall`.
//!
//! In-module unit tests in `crates/adapter-claude/src/install.rs` cover the
//! happy-path JSON merge shape. This file covers the cross-cutting contracts
//! the story's ACs name explicitly: concurrent-write tolerance (AC #2),
//! atomic-on-interrupt invariants (AC #3), round-trip preservation of user
//! content (AC #1 + #4), and the AC #5 "create if missing" behavior including
//! parent-directory creation.

use std::fs;
use std::sync::{Arc, Barrier};
use std::thread;

use adapter_claude::{install, uninstall};
use serde_json::{json, Value};
use tempfile::TempDir;

fn fresh_settings(dir: &TempDir) -> std::path::PathBuf {
    dir.path().join("settings.json")
}

/// AC #1: the command written into settings.json uses the protocol constant
/// for the shim binary name and the shim's `--hook-kind <kind>` CLI shape.
/// This is the canary for both the SHIM_BINARY_NAME value (Task 5, Approach B
/// = "bowerbird-shim") and the command shape Claude Code's hook engine will
/// invoke.
#[test]
fn install_writes_shim_command_with_hook_kind_for_each_known_kind() {
    let dir = TempDir::new().unwrap();
    let path = fresh_settings(&dir);

    install(&path).expect("install");
    let parsed: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();

    for kind in [
        "UserPromptSubmit",
        "PreToolUse",
        "PostToolUse",
        "Stop",
        "Notification",
    ] {
        let cmd = parsed
            .pointer(&format!("/hooks/{kind}/0/hooks/0/command"))
            .and_then(|v| v.as_str())
            .unwrap_or_else(|| panic!("missing command for {kind}"));
        let expected = format!("{} --hook-kind {kind}", protocol::SHIM_BINARY_NAME);
        assert_eq!(cmd, expected);
    }
}

/// AC #1 + #4: install followed by uninstall should leave the user's content
/// untouched, byte-for-byte for the unrelated fields. Idempotent up to
/// the trailing-newline convention.
#[test]
fn install_then_uninstall_round_trips_user_content() {
    let dir = TempDir::new().unwrap();
    let path = fresh_settings(&dir);
    let initial = json!({
        "theme": "dark",
        "editor": {"fontSize": 14, "tabSize": 2},
        "hooks": {
            "PreToolUse": [
                {"hooks": [{"type": "command", "command": "/my/own/hook --flag"}]}
            ]
        }
    });
    fs::write(&path, serde_json::to_vec_pretty(&initial).unwrap()).unwrap();

    install(&path).expect("install");
    uninstall(&path).expect("uninstall");

    let after: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    assert_eq!(after.get("theme"), Some(&json!("dark")));
    assert_eq!(
        after.get("editor"),
        Some(&json!({"fontSize": 14, "tabSize": 2}))
    );
    let pre = after
        .pointer("/hooks/PreToolUse")
        .and_then(|v| v.as_array())
        .expect("PreToolUse array");
    assert_eq!(pre.len(), 1, "user hook must remain alone after uninstall");
    let cmd = pre[0]
        .pointer("/hooks/0/command")
        .and_then(|v| v.as_str())
        .expect("user command");
    assert_eq!(cmd, "/my/own/hook --flag");
}

/// AC #2: under concurrent install calls against the same settings.json,
/// the file is never left in a partially-overwritten state. Both calls
/// either succeed (one wins, one retries) or one returns the typed
/// rename-race error — but the file always parses as valid JSON and
/// always contains the bowerbird hook on a successful return.
///
/// The atomic write protocol is the contract; this test is the canary.
#[test]
fn concurrent_install_yields_consistent_final_state() {
    let dir = TempDir::new().unwrap();
    let path = fresh_settings(&dir);
    // Seed the file so both threads have a baseline mtime to race against.
    fs::write(&path, b"{}\n").unwrap();

    let barrier = Arc::new(Barrier::new(4));
    let mut handles = Vec::new();
    for _ in 0..4 {
        let path = path.clone();
        let barrier = barrier.clone();
        handles.push(thread::spawn(move || {
            barrier.wait();
            install(&path)
        }));
    }

    let mut succeeded = 0;
    let mut raced = 0;
    for h in handles {
        match h.join().expect("thread") {
            Ok(_) => succeeded += 1,
            Err(adapter_claude::InstallError::SettingsAtomicRenameRace { .. }) => {
                raced += 1;
            }
            Err(other) => panic!("unexpected install error: {other}"),
        }
    }
    assert!(
        succeeded >= 1,
        "at least one concurrent install must succeed (succeeded={succeeded}, raced={raced})"
    );

    // Final state invariant: valid JSON, top-level object, bowerbird hook
    // present exactly once per known kind.
    let parsed: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    assert!(
        parsed.is_object(),
        "settings.json must remain a JSON object"
    );
    for kind in [
        "UserPromptSubmit",
        "PreToolUse",
        "PostToolUse",
        "Stop",
        "Notification",
    ] {
        let groups = parsed
            .pointer(&format!("/hooks/{kind}"))
            .and_then(|v| v.as_array())
            .unwrap_or_else(|| panic!("missing /hooks/{kind} after concurrent install"));
        let bowerbird_count = groups
            .iter()
            .filter(|g| {
                g.pointer("/hooks/0/command")
                    .and_then(|v| v.as_str())
                    .is_some_and(|c| c.starts_with(protocol::SHIM_BINARY_NAME))
            })
            .count();
        assert_eq!(
            bowerbird_count, 1,
            "exactly one bowerbird entry expected for {kind}, found {bowerbird_count}"
        );
    }
}

/// AC #3: a successful install leaves no .tmp files in the settings dir.
/// The atomic-write contract says the tmp file is the only thing on disk
/// between the write and rename; after rename completes the tmp name is
/// the target name, and no stale tmp should remain.
#[test]
fn install_leaves_no_tmp_files_in_settings_directory() {
    let dir = TempDir::new().unwrap();
    let path = fresh_settings(&dir);

    install(&path).expect("install");

    let entries: Vec<_> = fs::read_dir(dir.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    let tmps: Vec<&String> = entries
        .iter()
        .filter(|n| n.contains(".bowerbird-install.") || n.ends_with(".tmp"))
        .collect();
    assert!(
        tmps.is_empty(),
        "no .tmp file should remain after a successful install; found: {tmps:?}"
    );
    assert!(entries.iter().any(|n| n == "settings.json"));
}

/// AC #3: a malformed settings.json must not be overwritten by install.
/// The atomic write protocol's "read → parse → write tmp → rename" sequence
/// surfaces parse errors before any disk mutation. The original bytes
/// remain on disk verbatim.
#[test]
fn install_does_not_modify_original_on_parse_failure() {
    let dir = TempDir::new().unwrap();
    let path = fresh_settings(&dir);
    let original = b"{not-json: never::was}\n";
    fs::write(&path, original).unwrap();

    let err = install(&path).expect_err("install must reject malformed JSON");
    assert!(matches!(
        err,
        adapter_claude::InstallError::SettingsParse { .. }
    ));

    let after = fs::read(&path).unwrap();
    assert_eq!(
        after, original,
        "malformed settings.json must not be touched by install"
    );

    // And no tmp files left behind from the aborted attempt.
    let stale: Vec<_> = fs::read_dir(dir.path())
        .unwrap()
        .filter_map(|e| {
            let n = e.unwrap().file_name().into_string().unwrap();
            if n.contains(".bowerbird-install.") {
                Some(n)
            } else {
                None
            }
        })
        .collect();
    assert!(
        stale.is_empty(),
        "aborted install should leave no .tmp; found: {stale:?}"
    );
}

/// AC #5: install creates the parent directory if it does not exist. Mirrors
/// the real-world case where the user has not yet launched Claude Code from
/// their account; `~/.claude/` may not exist when `bowerbird install` runs.
#[test]
fn install_creates_parent_directory_when_missing() {
    let dir = TempDir::new().unwrap();
    let nested = dir.path().join("brand-new").join("settings.json");
    assert!(!nested.parent().unwrap().exists());

    install(&nested).expect("install must create parent dir");
    assert!(nested.exists());
    assert!(nested.parent().unwrap().exists());
    let _parsed: Value =
        serde_json::from_slice(&fs::read(&nested).unwrap()).expect("created file is valid JSON");
}

/// Story 3.4 AC #4: the command written into settings.json is PATH-relative —
/// no path component, no absolute path, no `current_exe()` interpolation. A
/// user who downloads a tarball today, extracts it, and copies the binaries to
/// `/usr/local/bin/` can re-download tomorrow into `~/.local/bin/` (or any
/// other `$PATH` location) WITHOUT re-running `bowerbird install` — Claude
/// Code's hook engine resolves the bare name `bowerbird-shim` via `$PATH` at
/// each invocation. This test pins the invariant against future drift.
///
/// Pinned by two assertions per hook kind:
///   1. The command string contains zero `/` characters (PATH-relative form).
///   2. The first whitespace-separated token equals `protocol::SHIM_BINARY_NAME`.
#[test]
fn installed_command_uses_path_relative_binary_name_no_slash_in_first_token() {
    let dir = TempDir::new().unwrap();
    let path = fresh_settings(&dir);

    install(&path).expect("install");
    let parsed: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();

    for kind in [
        "UserPromptSubmit",
        "PreToolUse",
        "PostToolUse",
        "Stop",
        "Notification",
    ] {
        let cmd = parsed
            .pointer(&format!("/hooks/{kind}/0/hooks/0/command"))
            .and_then(|v| v.as_str())
            .unwrap_or_else(|| panic!("missing command for {kind}"));

        assert!(
            !cmd.contains('/'),
            "AC #4 regression: command for {kind} must be PATH-relative \
             (no `/` characters), found {cmd:?}"
        );

        let first_token = cmd
            .split_whitespace()
            .next()
            .unwrap_or_else(|| panic!("empty command for {kind}"));
        assert_eq!(
            first_token,
            protocol::SHIM_BINARY_NAME,
            "AC #4 regression: first token of {kind} command must equal \
             SHIM_BINARY_NAME ({:?}), found {first_token:?}",
            protocol::SHIM_BINARY_NAME
        );
    }
}

/// AC #4: uninstall is idempotent when called on a file that does not contain
/// the bowerbird entries — including a file that was never installed to.
#[test]
fn uninstall_is_idempotent_when_no_bowerbird_entries_present() {
    let dir = TempDir::new().unwrap();
    let path = fresh_settings(&dir);
    let initial = json!({
        "theme": "light",
        "hooks": {
            "PreToolUse": [
                {"hooks": [{"type": "command", "command": "/user/script"}]}
            ]
        }
    });
    let serialized = serde_json::to_vec_pretty(&initial).unwrap();
    fs::write(&path, &serialized).unwrap();

    let outcome = uninstall(&path).expect("uninstall");
    assert!(outcome.existed);
    assert!(
        outcome.hook_kinds_removed.is_empty(),
        "nothing to remove if no bowerbird entries existed"
    );

    let after: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    assert_eq!(after.get("theme"), Some(&json!("light")));
    let pre = after
        .pointer("/hooks/PreToolUse")
        .unwrap()
        .as_array()
        .unwrap();
    assert_eq!(pre.len(), 1);
    assert_eq!(
        pre[0].pointer("/hooks/0/command").and_then(|v| v.as_str()),
        Some("/user/script")
    );
}
