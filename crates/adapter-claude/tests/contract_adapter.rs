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
    // TOML file does not exist — should degrade gracefully to Unknown
    let toml_path = std::path::PathBuf::from("/nonexistent/path/tool-reactions.toml");
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
