use std::io::Write;

use adapter_claude::ClaudeAdapter;
use protocol::{EventKind, Reaction, SourceAdapter};
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
