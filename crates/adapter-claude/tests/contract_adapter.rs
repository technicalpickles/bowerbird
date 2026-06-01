use std::io::Write;

use adapter_claude::ClaudeAdapter;
use protocol::{EventKind, NotificationType, Reaction, SourceAdapter};
use tempfile::TempDir;

const BASH_PAYLOAD: &str = include_str!("fixtures/pre_tool_use_bash.json");
const UNKNOWN_TOOL_PAYLOAD: &str = include_str!("fixtures/pre_tool_use_unknown.json");

fn write_toml(dir: &TempDir, content: &str) -> std::path::PathBuf {
    let path = dir.path().join("tool-reactions.toml");
    let mut f = std::fs::File::create(&path).unwrap();
    f.write_all(content.as_bytes()).unwrap();
    path
}

fn minimal_toml_with_bash() -> &'static str {
    "[tool_reactions]\nBash = \"Continue\"\n"
}

#[test]
fn normalize_pretooluse_bash_known_reaction() {
    let dir = TempDir::new().unwrap();
    let toml_path = write_toml(&dir, minimal_toml_with_bash());
    let adapter = ClaudeAdapter::new(toml_path);

    let result = adapter
        .normalize("PreToolUse", BASH_PAYLOAD.as_bytes())
        .unwrap();
    let env = result.envelope;

    assert_eq!(env.source, "claude");
    assert_eq!(env.session_id, "test-session-abc123");
    assert_eq!(env.kind, EventKind::PreToolUse);
    assert_eq!(env.reaction, Some(Reaction::Continue));
    // Payload must contain original fields verbatim
    assert!(env.payload.contains("test-session-abc123"));
    assert!(env.payload.contains("Bash"));
    assert!(env.payload.contains("cargo test"));
}

#[test]
fn normalize_user_prompt_submit_round_trip() {
    // Story 5.2: the adapter must map the new "UserPromptSubmit" hook
    // string to EventKind::UserPromptSubmit. UserPromptSubmit payloads
    // carry no `tool_name` (only PreToolUse does); the envelope's
    // `reaction` must therefore be None — the load-bearing match arm in
    // normalize.rs falls through to `_ => None` for non-PreToolUse kinds.
    let dir = TempDir::new().unwrap();
    let toml_path = write_toml(&dir, minimal_toml_with_bash());
    let adapter = ClaudeAdapter::new(toml_path);

    let payload = br#"{"session_id":"sess-ups","prompt":"hello"}"#;
    let result = adapter.normalize("UserPromptSubmit", payload).unwrap();
    let env = result.envelope;

    assert_eq!(env.source, "claude");
    assert_eq!(env.session_id, "sess-ups");
    assert_eq!(env.kind, EventKind::UserPromptSubmit);
    assert_eq!(
        env.reaction, None,
        "UserPromptSubmit carries no tool_name and therefore no reaction"
    );
    // Native payload rides verbatim — substrate-not-actor invariant
    // (project-context.md §Substrate-not-actor invariants).
    assert!(env.payload.contains("sess-ups"));
    assert!(env.payload.contains("hello"));
}

#[test]
fn normalize_unknown_tool_returns_unknown_reaction() {
    let dir = TempDir::new().unwrap();
    let toml_path = write_toml(&dir, minimal_toml_with_bash());
    let adapter = ClaudeAdapter::new(toml_path);

    let result = adapter
        .normalize("PreToolUse", UNKNOWN_TOOL_PAYLOAD.as_bytes())
        .unwrap();
    let env = result.envelope;

    assert_eq!(env.source, "claude");
    assert_eq!(env.session_id, "test-session-xyz789");
    assert_eq!(env.kind, EventKind::PreToolUse);
    assert_eq!(env.reaction, Some(Reaction::Unknown));
    // No panic; event is still returned
}

#[test]
fn normalize_runtime_toml_update() {
    let dir = TempDir::new().unwrap();
    // Start with empty tool_reactions (no Bash entry)
    let toml_path = write_toml(&dir, "[tool_reactions]\n");
    let adapter = ClaudeAdapter::new(toml_path.clone());

    // First normalize: no entry → Unknown
    let result1 = adapter
        .normalize("PreToolUse", BASH_PAYLOAD.as_bytes())
        .unwrap();
    assert_eq!(result1.envelope.reaction, Some(Reaction::Unknown));

    // Update TOML at runtime: add Bash → Continue
    let mut f = std::fs::File::create(&toml_path).unwrap();
    f.write_all(minimal_toml_with_bash().as_bytes()).unwrap();
    drop(f);

    // Second normalize with same adapter: should use updated mapping
    let result2 = adapter
        .normalize("PreToolUse", BASH_PAYLOAD.as_bytes())
        .unwrap();
    assert_eq!(result2.envelope.reaction, Some(Reaction::Continue));
}

#[test]
fn normalize_extra_fields_preserved_verbatim() {
    let dir = TempDir::new().unwrap();
    let toml_path = write_toml(&dir, minimal_toml_with_bash());
    let adapter = ClaudeAdapter::new(toml_path);

    // Payload with extra unknown fields
    let payload_with_extras = r#"{
      "hook_kind": "PreToolUse",
      "session_id": "sess-extras",
      "tool_name": "Bash",
      "tool_input": {"command": "ls"},
      "unknown_future_field": "some_value",
      "another_extra": 42
    }"#;

    let result = adapter
        .normalize("PreToolUse", payload_with_extras.as_bytes())
        .unwrap();
    let env = result.envelope;

    // All original fields must be in the stored payload verbatim
    assert!(env.payload.contains("unknown_future_field"));
    assert!(env.payload.contains("some_value"));
    assert!(env.payload.contains("another_extra"));
    assert!(env.payload.contains("42"));
}

#[test]
fn normalize_posttooluse_has_no_reaction() {
    let dir = TempDir::new().unwrap();
    let toml_path = write_toml(&dir, minimal_toml_with_bash());
    let adapter = ClaudeAdapter::new(toml_path);

    let payload = r#"{"session_id": "sess-post", "tool_name": "Bash", "tool_output": "ok"}"#;

    let result = adapter
        .normalize("PostToolUse", payload.as_bytes())
        .unwrap();
    assert_eq!(result.envelope.kind, EventKind::PostToolUse);
    assert_eq!(result.envelope.reaction, None);
}

#[test]
fn normalize_missing_session_id_returns_error() {
    let dir = TempDir::new().unwrap();
    let toml_path = write_toml(&dir, minimal_toml_with_bash());
    let adapter = ClaudeAdapter::new(toml_path);

    let payload = r#"{"tool_name": "Bash", "tool_input": {}}"#;

    let result = adapter.normalize("PreToolUse", payload.as_bytes());
    assert!(result.is_err());
}

#[test]
fn normalize_missing_toml_returns_unknown_reaction() {
    // TOML file does not exist — should degrade gracefully to Unknown.
    // Use a TempDir-scoped non-existent path so the test is hermetic.
    let dir = TempDir::new().unwrap();
    let toml_path = dir.path().join("does-not-exist.toml");
    let adapter = ClaudeAdapter::new(toml_path);

    let result = adapter
        .normalize("PreToolUse", BASH_PAYLOAD.as_bytes())
        .unwrap();
    assert_eq!(result.envelope.reaction, Some(Reaction::Unknown));
}

#[test]
fn normalize_source_is_always_claude() {
    let dir = TempDir::new().unwrap();
    let toml_path = write_toml(&dir, minimal_toml_with_bash());
    let adapter = ClaudeAdapter::new(toml_path);

    let payload = r#"{"session_id": "s1", "tool_name": "Read"}"#;
    let result = adapter.normalize("PreToolUse", payload.as_bytes()).unwrap();
    assert_eq!(result.envelope.source, "claude");
}

#[test]
fn normalize_stop_event_no_reaction() {
    let dir = TempDir::new().unwrap();
    let toml_path = write_toml(&dir, minimal_toml_with_bash());
    let adapter = ClaudeAdapter::new(toml_path);

    let payload = r#"{"session_id": "sess-stop", "exit_code": 0}"#;
    let result = adapter.normalize("Stop", payload.as_bytes()).unwrap();
    assert_eq!(result.envelope.kind, EventKind::Stop);
    assert_eq!(result.envelope.reaction, None);
}

#[test]
fn normalize_pretooluse_missing_tool_name_returns_error() {
    let dir = TempDir::new().unwrap();
    let toml_path = write_toml(&dir, minimal_toml_with_bash());
    let adapter = ClaudeAdapter::new(toml_path);

    // PreToolUse without tool_name is malformed — adapter must reject, not
    // silently look up "" in the TOML.
    let payload = r#"{"session_id": "s1"}"#;
    let result = adapter.normalize("PreToolUse", payload.as_bytes());
    assert!(result.is_err(), "missing tool_name should be an error");
}

#[test]
fn normalize_pretooluse_pause_reaction() {
    let dir = TempDir::new().unwrap();
    let toml_path = write_toml(&dir, "[tool_reactions]\nBash = \"Pause\"\n");
    let adapter = ClaudeAdapter::new(toml_path);

    let result = adapter
        .normalize("PreToolUse", BASH_PAYLOAD.as_bytes())
        .unwrap();
    assert_eq!(result.envelope.reaction, Some(Reaction::Pause));
}

#[test]
fn normalize_pretooluse_vendor_reaction() {
    let dir = TempDir::new().unwrap();
    let toml_path = write_toml(&dir, "[tool_reactions]\nBash = \"Vendor(7)\"\n");
    let adapter = ClaudeAdapter::new(toml_path);

    let result = adapter
        .normalize("PreToolUse", BASH_PAYLOAD.as_bytes())
        .unwrap();
    assert_eq!(result.envelope.reaction, Some(Reaction::Vendor(7)));
}

#[test]
fn normalize_pretooluse_vendor_garbage_degrades_to_unknown() {
    let dir = TempDir::new().unwrap();
    let toml_path = write_toml(&dir, "[tool_reactions]\nBash = \"Vendor(abc)\"\n");
    let adapter = ClaudeAdapter::new(toml_path);

    let result = adapter
        .normalize("PreToolUse", BASH_PAYLOAD.as_bytes())
        .unwrap();
    assert_eq!(result.envelope.reaction, Some(Reaction::Unknown));
}

#[test]
fn normalize_pretooluse_vendor_overflow_degrades_to_unknown() {
    let dir = TempDir::new().unwrap();
    // 99999 doesn't fit in u16
    let toml_path = write_toml(&dir, "[tool_reactions]\nBash = \"Vendor(99999)\"\n");
    let adapter = ClaudeAdapter::new(toml_path);

    let result = adapter
        .normalize("PreToolUse", BASH_PAYLOAD.as_bytes())
        .unwrap();
    assert_eq!(result.envelope.reaction, Some(Reaction::Unknown));
}

// Story 1.8: the adapter's internal `Error::InvalidHookKind` must convert into
// the typed `protocol::Error::UnknownHookKind` (not a stringly-typed `Serde`
// variant). The daemon `match`es on this variant to emit the dedicated
// `400 unknown hook_kind: ...` wire response. The internal `Error` enum is
// `pub(crate)`, so this test exercises the boundary via the public `normalize`
// API rather than constructing the variant directly.
// ─── Story 5.3: bowerbird_ppid extraction ────────────────────────────────────

#[test]
fn normalize_extracts_pid_when_bowerbird_ppid_set() {
    let dir = TempDir::new().unwrap();
    let toml_path = write_toml(&dir, minimal_toml_with_bash());
    let adapter = ClaudeAdapter::new(toml_path);

    let payload = r#"{"session_id":"s1","tool_name":"Bash","bowerbird_ppid":12345}"#;
    let result = adapter.normalize("PreToolUse", payload.as_bytes()).unwrap();
    assert_eq!(result.envelope.pid, Some(12345));
}

#[test]
fn normalize_extracts_pid_none_when_missing() {
    let dir = TempDir::new().unwrap();
    let toml_path = write_toml(&dir, minimal_toml_with_bash());
    let adapter = ClaudeAdapter::new(toml_path);

    let payload = r#"{"session_id":"s1","tool_name":"Bash"}"#;
    let result = adapter.normalize("PreToolUse", payload.as_bytes()).unwrap();
    assert_eq!(result.envelope.pid, None);
}

#[test]
fn normalize_extracts_pid_none_when_non_integer() {
    let dir = TempDir::new().unwrap();
    let toml_path = write_toml(&dir, minimal_toml_with_bash());
    let adapter = ClaudeAdapter::new(toml_path);

    let payload = r#"{"session_id":"s1","tool_name":"Bash","bowerbird_ppid":"not-an-integer"}"#;
    let result = adapter.normalize("PreToolUse", payload.as_bytes()).unwrap();
    assert_eq!(
        result.envelope.pid, None,
        "non-integer ppid yields None without failing normalization"
    );
}

#[test]
fn normalize_extracts_pid_none_when_negative() {
    let dir = TempDir::new().unwrap();
    let toml_path = write_toml(&dir, minimal_toml_with_bash());
    let adapter = ClaudeAdapter::new(toml_path);

    // serde_json::Value::as_u64() refuses negatives; u32::try_from also rejects
    // out-of-range. Both yield None.
    let payload = r#"{"session_id":"s1","tool_name":"Bash","bowerbird_ppid":-1}"#;
    let result = adapter.normalize("PreToolUse", payload.as_bytes()).unwrap();
    assert_eq!(result.envelope.pid, None);
}

#[test]
fn normalize_rejects_bowerbird_ppid_zero() {
    let dir = TempDir::new().unwrap();
    let toml_path = write_toml(&dir, minimal_toml_with_bash());
    let adapter = ClaudeAdapter::new(toml_path);

    // PID 0 must be filtered at the adapter boundary: kill(0, 0) has process-
    // group semantics, which would make the liveness probe report any session
    // with last_pid: Some(0) as eternally alive.
    let payload = r#"{"session_id":"s1","tool_name":"Bash","bowerbird_ppid":0}"#;
    let result = adapter.normalize("PreToolUse", payload.as_bytes()).unwrap();
    assert_eq!(
        result.envelope.pid, None,
        "bowerbird_ppid: 0 must be treated as absent so liveness never calls kill(0, 0)"
    );
}

// ─── Story 5.7: cwd extraction ───────────────────────────────────────────────

#[test]
fn normalize_extracts_cwd_when_present() {
    let dir = TempDir::new().unwrap();
    let toml_path = write_toml(&dir, minimal_toml_with_bash());
    let adapter = ClaudeAdapter::new(toml_path);

    let payload = r#"{"session_id":"s1","tool_name":"Bash","cwd":"/Users/x/repo"}"#;
    let result = adapter.normalize("PreToolUse", payload.as_bytes()).unwrap();
    assert_eq!(result.envelope.cwd, Some("/Users/x/repo".to_string()));
}

#[test]
fn normalize_extracts_cwd_none_when_missing() {
    let dir = TempDir::new().unwrap();
    let toml_path = write_toml(&dir, minimal_toml_with_bash());
    let adapter = ClaudeAdapter::new(toml_path);

    let payload = r#"{"session_id":"s1","tool_name":"Bash"}"#;
    let result = adapter.normalize("PreToolUse", payload.as_bytes()).unwrap();
    assert_eq!(result.envelope.cwd, None);
}

#[test]
fn normalize_extracts_cwd_none_when_non_string() {
    let dir = TempDir::new().unwrap();
    let toml_path = write_toml(&dir, minimal_toml_with_bash());
    let adapter = ClaudeAdapter::new(toml_path);

    // A number, an object, and null all yield cwd: None without failing
    // normalization (as_str returns None for non-strings).
    for raw in [
        r#"{"session_id":"s1","tool_name":"Bash","cwd":123}"#,
        r#"{"session_id":"s1","tool_name":"Bash","cwd":{"nested":true}}"#,
        r#"{"session_id":"s1","tool_name":"Bash","cwd":null}"#,
    ] {
        let result = adapter.normalize("PreToolUse", raw.as_bytes()).unwrap();
        assert_eq!(
            result.envelope.cwd, None,
            "non-string cwd in {raw} must normalize to None, not error"
        );
    }
}

#[test]
fn normalize_extracts_cwd_for_all_hook_kinds() {
    let dir = TempDir::new().unwrap();
    let toml_path = write_toml(&dir, minimal_toml_with_bash());
    let adapter = ClaudeAdapter::new(toml_path);

    // cwd is a top-level field on every Claude hook payload — NOT kind-gated
    // like notification_type (which is Notification-only). PreToolUse also
    // needs a tool_name; the others don't.
    let cases: &[(&str, &str)] = &[
        ("UserPromptSubmit", r#"{"session_id":"s1","cwd":"/p/ups"}"#),
        (
            "PreToolUse",
            r#"{"session_id":"s1","tool_name":"Bash","cwd":"/p/pre"}"#,
        ),
        ("PostToolUse", r#"{"session_id":"s1","cwd":"/p/post"}"#),
        ("Stop", r#"{"session_id":"s1","cwd":"/p/stop"}"#),
        ("Notification", r#"{"session_id":"s1","cwd":"/p/notif"}"#),
    ];
    for (hook_kind, payload) in cases {
        let result = adapter.normalize(hook_kind, payload.as_bytes()).unwrap();
        assert!(
            result.envelope.cwd.is_some(),
            "cwd must be extracted for hook kind {hook_kind}"
        );
    }
}

// ─── Story 5.3: notification_type extraction ─────────────────────────────────

#[test]
fn normalize_extracts_notification_type_for_six_known_values() {
    let dir = TempDir::new().unwrap();
    let toml_path = write_toml(&dir, minimal_toml_with_bash());
    let adapter = ClaudeAdapter::new(toml_path);

    let cases: &[(&str, NotificationType)] = &[
        ("permission_prompt", NotificationType::PermissionPrompt),
        ("idle_prompt", NotificationType::IdlePrompt),
        ("auth_success", NotificationType::AuthSuccess),
        ("elicitation_dialog", NotificationType::ElicitationDialog),
        (
            "elicitation_response",
            NotificationType::ElicitationResponse,
        ),
        (
            "elicitation_complete",
            NotificationType::ElicitationComplete,
        ),
    ];

    for (wire, expected) in cases {
        let payload = format!(r#"{{"session_id":"s1","notification_type":"{wire}"}}"#);
        let result = adapter
            .normalize("Notification", payload.as_bytes())
            .unwrap();
        assert_eq!(
            result.envelope.notification_type,
            Some(*expected),
            "wire value {wire:?} should map to expected variant"
        );
    }
}

#[test]
fn normalize_extracts_notification_type_unknown_for_future_value() {
    let dir = TempDir::new().unwrap();
    let toml_path = write_toml(&dir, minimal_toml_with_bash());
    let adapter = ClaudeAdapter::new(toml_path);

    let payload = r#"{"session_id":"s1","notification_type":"future_type_v2"}"#;
    let result = adapter
        .normalize("Notification", payload.as_bytes())
        .unwrap();
    assert_eq!(
        result.envelope.notification_type,
        Some(NotificationType::Unknown),
        "unrecognized notification_type → Unknown (decode-only catch-all)"
    );
}

#[test]
fn normalize_extracts_notification_type_none_when_missing() {
    let dir = TempDir::new().unwrap();
    let toml_path = write_toml(&dir, minimal_toml_with_bash());
    let adapter = ClaudeAdapter::new(toml_path);

    let payload = r#"{"session_id":"s1"}"#;
    let result = adapter
        .normalize("Notification", payload.as_bytes())
        .unwrap();
    assert_eq!(result.envelope.notification_type, None);
}

#[test]
fn normalize_does_not_extract_notification_type_for_non_notification_kinds() {
    let dir = TempDir::new().unwrap();
    let toml_path = write_toml(&dir, minimal_toml_with_bash());
    let adapter = ClaudeAdapter::new(toml_path);

    // A stray notification_type field on a non-Notification payload must be
    // ignored by the adapter (still preserved verbatim in payload).
    let payload =
        r#"{"session_id":"s1","tool_name":"Bash","notification_type":"permission_prompt"}"#;
    let result = adapter.normalize("PreToolUse", payload.as_bytes()).unwrap();
    assert_eq!(
        result.envelope.notification_type, None,
        "non-Notification kinds must not extract notification_type"
    );
    // But the field should still ride in payload verbatim.
    assert!(result.envelope.payload.contains("permission_prompt"));
}

#[test]
fn normalize_unknown_hook_kind_yields_protocol_unknown_hook_kind() {
    let dir = TempDir::new().unwrap();
    let toml_path = write_toml(&dir, minimal_toml_with_bash());
    let adapter = ClaudeAdapter::new(toml_path);

    let payload = r#"{"session_id": "s1", "tool_name": "Bash"}"#;
    let result = adapter.normalize("BogusKind", payload.as_bytes());
    match result {
        Err(protocol::Error::UnknownHookKind(k)) => assert_eq!(k, "BogusKind"),
        Err(other) => panic!("expected protocol::Error::UnknownHookKind, got Err({other:?})"),
        Ok(_) => panic!("expected Err, got Ok"),
    }
}
