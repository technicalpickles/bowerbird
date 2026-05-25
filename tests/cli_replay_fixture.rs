//! Hermetic doc-drift guardrails for Story 4.1.
//!
//! Per Epic 3 retro agreement A7 ("doc-drift verification as a compiled
//! test, not a verification-block grep"), these tests assert structural
//! invariants between the bundled fixture, the architecture document, the
//! protocol changelog, and the CLI help surface.
//!
//! No daemon, no subprocess setup, fast — every test here is parallel-safe.

use std::collections::HashSet;
use std::path::PathBuf;

use protocol::{Event, EventKind};

const FIXTURE_REL_PATH: &str = "fixtures/replay-demo.jsonl";

fn workspace_root() -> PathBuf {
    // `CARGO_MANIFEST_DIR` is the package root for the `bowerbird` package,
    // which lives at the workspace root. Workspace-root files like
    // `fixtures/replay-demo.jsonl` and `docs/bmad/planning-artifacts/
    // architecture.md` resolve from this base.
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read_fixture() -> String {
    let path = workspace_root().join(FIXTURE_REL_PATH);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {} failed: {e}", path.display()))
}

fn parse_fixture_events(body: &str) -> Vec<Event> {
    let mut events = Vec::new();
    for (idx, line) in body.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let event: Event = serde_json::from_str(trimmed).unwrap_or_else(|e| {
            panic!(
                "fixture line {} did not parse as protocol::Event: {e}\nline: {line}",
                idx + 1
            )
        });
        events.push(event);
    }
    events
}

/// AC #3 / AC #4: the bundled fixture parses cleanly into `Vec<Event>`,
/// has ≥10 events, and includes at least one `PreToolUse` variant (the
/// canary that catches accidental rename of an EventKind variant).
#[test]
fn bundled_fixture_is_valid_jsonl() {
    let body = read_fixture();
    let events = parse_fixture_events(&body);
    assert!(
        events.len() >= 10,
        "fixture must have ≥10 events; got {}",
        events.len()
    );
    assert!(
        events.iter().any(|e| e.kind == EventKind::PreToolUse),
        "fixture must contain at least one PreToolUse event (round-trip canary)",
    );
}

/// AC #4 explicit guardrail: the bundled fixture spans ≥2 distinct
/// `(source, session_id)` keys so the no-arg `bowerbird replay` exercises
/// multi-session fan-out.
#[test]
fn bundled_fixture_spans_at_least_two_sessions() {
    let body = read_fixture();
    let events = parse_fixture_events(&body);
    let sessions: HashSet<(String, String)> = events
        .iter()
        .map(|e| (e.source.clone(), e.session_id.clone()))
        .collect();
    assert!(
        sessions.len() >= 2,
        "fixture must span ≥2 distinct (source, session_id) keys; got {sessions:?}"
    );
}

/// AC #6: `architecture.md` lists `replay` and `export` as shipped CLI
/// subcommands; the legacy "arrive in Story 4.1" deferral language is
/// gone.
#[test]
fn architecture_md_lists_replay_and_export_as_shipped() {
    let path = workspace_root().join("docs/bmad/planning-artifacts/architecture.md");
    let body = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("read {} failed: {e}", path.display()));

    // The shipped subcommands list must now mention both `replay` and
    // `export` (alongside the existing surface). We check substring-presence;
    // surrounding formatting (markdown emphasis, code-fence) is allowed to
    // drift.
    assert!(
        body.contains("`replay`") || body.contains("replay`"),
        "architecture.md must reference `replay` as a shipped subcommand",
    );
    assert!(
        body.contains("`export`") || body.contains("export`"),
        "architecture.md must reference `export` as a shipped subcommand",
    );

    // Legacy deferral phrases must be gone.
    for stale in [
        "arrive in Story 4.1",
        "Epic 4 will add",
        "replay/export arrive in Epic 4",
        "`replay` and `export` arrive in",
    ] {
        assert!(
            !body.contains(stale),
            "architecture.md still contains legacy phrase {stale:?}; \
             Story 4.1 must replace it"
        );
    }
}

/// AC #7: the protocol changelog has an entry documenting the new
/// `POST /replay` endpoint with a `Resolves: 4.1` marker. Mirrors Story
/// 3.4's release-pipeline-docs assertion shape.
#[test]
fn protocol_changelog_documents_post_replay_endpoint() {
    let path = workspace_root().join("docs/protocol-changelog.md");
    let body = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("read {} failed: {e}", path.display()));

    assert!(
        body.contains("POST /replay"),
        "docs/protocol-changelog.md must mention `POST /replay` (Story 4.1)"
    );
    assert!(
        body.contains("Resolves: 4.1") || body.contains("Resolves: Story 4.1"),
        "docs/protocol-changelog.md must mark the entry with `Resolves: 4.1`"
    );
}

/// Surface check: `bowerbird --help` lists both `replay` and `export`
/// subcommands. Structurally enforced by clap's derive, but the
/// compiled test makes the binding explicit.
#[test]
fn cli_help_lists_replay_and_export() {
    let mut cmd = assert_cmd::Command::cargo_bin("bowerbird").expect("bowerbird binary built");
    let assertion = cmd.arg("--help").assert().success();
    let stdout = String::from_utf8_lossy(&assertion.get_output().stdout).into_owned();
    assert!(
        stdout.contains("replay"),
        "--help missing `replay`:\n{stdout}"
    );
    assert!(
        stdout.contains("export"),
        "--help missing `export`:\n{stdout}"
    );
}
