//! Story 4.4 / AC #2 — v1.0 wire compatibility corpus.
//!
//! The fixtures under `tests/fixtures/protocol-v1-corpus/` are the
//! load-bearing v1.0 wire surface. Every file in the corpus MUST continue to
//! deserialize cleanly through the current protocol crate; a fixture that no
//! longer decodes is a v1.x compatibility break — fail loudly in CI.
//!
//! **Future v1.x changes ADD fixtures here; they do NOT modify existing
//! fixtures.** The corpus is the changelog (`docs/protocol-changelog.md`)
//! made executable: every fixture traces back to a specific changelog entry
//! describing the wire shape it pins. When `protocol-changelog.md` gains a
//! new v1.x feature entry, that PR should also add the fixture(s) covering
//! the new shape — see AC #2 of Story 4.4.
//!
//! The corpus lives at WORKSPACE-ROOT `tests/fixtures/`, NOT under
//! `crates/protocol/tests/fixtures/`, because it is shared infrastructure
//! (the contract is "the wire surface bowerbird ships," not "things the
//! protocol crate internals depend on"). This matches the precedent set by
//! `tests/cli_*.rs` and `tests/release_pipeline_docs.rs` for workspace-level
//! invariants.

use std::path::PathBuf;

use protocol::{
    DaemonStatus, EventKind, EventListResponse, Reaction, ServerMessage, SessionCurrentState,
    SessionDetail, SessionListItem,
};

fn corpus_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/protocol-v1-corpus")
}

fn read_fixture(name: &str) -> String {
    let p = corpus_root().join(name);
    std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("read {}: {e}", p.display()))
}

// ---------------------------------------------------------------------------
// ServerMessage fixtures (WS outbound surface)
// ---------------------------------------------------------------------------

#[test]
fn hello_minimal_decodes_as_server_message_hello() {
    let body = read_fixture("hello-minimal.json");
    let parsed: ServerMessage = serde_json::from_str(&body).expect("hello-minimal decodes");
    match parsed {
        ServerMessage::Hello(h) => {
            assert_eq!(h.protocol_version, "1.0");
            assert_eq!(h.daemon_version, "0.1.0");
            assert!(h.history_begins_cleanly);
        }
        other => panic!("expected Hello, got {other:?}"),
    }
}

#[test]
fn hello_with_future_field_decodes_and_ignores_extra_field() {
    let body = read_fixture("hello-with-future-field.json");
    let parsed: ServerMessage = serde_json::from_str(&body).expect("future-field hello decodes");
    match parsed {
        ServerMessage::Hello(h) => {
            assert_eq!(h.daemon_version, "9.9.9");
            assert!(!h.history_begins_cleanly);
        }
        other => panic!("expected Hello, got {other:?}"),
    }
}

#[test]
fn sync_frame_decodes_as_server_message_sync() {
    let body = read_fixture("sync-frame.json");
    let parsed: ServerMessage = serde_json::from_str(&body).expect("sync-frame decodes");
    assert!(
        matches!(parsed, ServerMessage::Sync(_)),
        "expected Sync, got {parsed:?}"
    );
}

#[test]
fn event_pretooluse_decodes() {
    let body = read_fixture("event-pretooluse.json");
    let parsed: ServerMessage = serde_json::from_str(&body).expect("event-pretooluse decodes");
    match parsed {
        ServerMessage::Event(ef) => {
            assert_eq!(ef.event.event_id.0, 42);
            assert_eq!(ef.event.kind, EventKind::PreToolUse);
            assert_eq!(ef.event.reaction, Some(Reaction::Continue));
        }
        other => panic!("expected Event, got {other:?}"),
    }
}

#[test]
fn event_posttooluse_decodes() {
    let body = read_fixture("event-posttooluse.json");
    let parsed: ServerMessage = serde_json::from_str(&body).expect("event-posttooluse decodes");
    match parsed {
        ServerMessage::Event(ef) => {
            assert_eq!(ef.event.kind, EventKind::PostToolUse);
            assert_eq!(ef.event.reaction, None);
        }
        other => panic!("expected Event, got {other:?}"),
    }
}

#[test]
fn event_with_vendor_reaction_decodes() {
    let body = read_fixture("event-with-vendor-reaction.json");
    let parsed: ServerMessage =
        serde_json::from_str(&body).expect("event-with-vendor-reaction decodes");
    match parsed {
        ServerMessage::Event(ef) => {
            assert_eq!(ef.event.reaction, Some(Reaction::Vendor(42)));
        }
        other => panic!("expected Event, got {other:?}"),
    }
}

#[test]
fn event_with_unknown_reaction_decodes_via_unknown() {
    // LOAD-BEARING for Story 4.4 AC #6: the `Reaction::deserialize` fix maps
    // unknown reaction strings to `Reaction::Unknown` rather than erroring.
    // This fixture exercises a v1.x-shipped future reaction string and pins
    // the additive-compat behavior at the corpus level.
    let body = read_fixture("event-with-unknown-reaction.json");
    let parsed: ServerMessage =
        serde_json::from_str(&body).expect("event-with-unknown-reaction must decode");
    match parsed {
        ServerMessage::Event(ef) => {
            assert_eq!(
                ef.event.reaction,
                Some(Reaction::Unknown),
                "unknown reaction string must decode to Reaction::Unknown for additive-compat"
            );
        }
        other => panic!("expected Event, got {other:?}"),
    }
}

#[test]
fn event_with_unknown_kind_decodes_via_unknown() {
    // Story 4.4 AC #6: `EventKind::Unknown` was added via `#[serde(other)]`
    // so a future v1.x daemon emitting `Event.kind = "SubAgentSpawn"` (or
    // any other unknown PascalCase) decodes gracefully on v1.0 presenters
    // instead of failing the whole `ServerMessage::Event` parse. The corpus
    // pins this at the FULL envelope decode path, complementing the protocol-
    // crate unit test which pins the bare-enum decode.
    let body = read_fixture("event-with-unknown-kind.json");
    let parsed: ServerMessage =
        serde_json::from_str(&body).expect("event-with-unknown-kind must decode");
    match parsed {
        ServerMessage::Event(ef) => {
            assert_eq!(
                ef.event.kind,
                EventKind::Unknown,
                "unknown EventKind string must decode to EventKind::Unknown for additive-compat"
            );
        }
        other => panic!("expected Event, got {other:?}"),
    }
}

#[test]
fn state_unknown_decodes_via_unknown() {
    // Story 4.4 AC #6: `SessionCurrentState::Unknown` was added via
    // `#[serde(other)]`. A future v1.x state (e.g. `"Compacting"`,
    // `"AwaitingApproval"`) must decode through the full `ServerMessage::State`
    // → `StateFrame` → `SessionState` path on v1.0 presenters.
    let body = read_fixture("state-unknown.json");
    let parsed: ServerMessage = serde_json::from_str(&body).expect("state-unknown must decode");
    match parsed {
        ServerMessage::State(sf) => {
            assert_eq!(
                sf.state.current_state,
                SessionCurrentState::Unknown,
                "unknown current_state must decode to SessionCurrentState::Unknown for additive-compat"
            );
        }
        other => panic!("expected State, got {other:?}"),
    }
}

#[test]
fn server_message_unknown_op_decodes_via_unknown() {
    // Story 2.1 / Story 4.4 AC #6: `ServerMessage` carries `#[serde(other)]
    // Unknown`. A future v1.x daemon emitting a brand-new top-level `op`
    // (e.g. `"telemetry"`) decodes to `ServerMessage::Unknown` on v1.0
    // presenters rather than failing the tagged-enum dispatch. The fixture
    // pins this envelope-level behavior; the protocol-crate test
    // `server_message_unknown_variant_round_trips_as_unknown` pins the same
    // behavior at the bare-string level — both layers stay green together.
    let body = read_fixture("server-message-unknown.json");
    let parsed: ServerMessage =
        serde_json::from_str(&body).expect("server-message-unknown must decode");
    assert!(
        matches!(parsed, ServerMessage::Unknown),
        "unknown `op` must decode to ServerMessage::Unknown, got {parsed:?}"
    );
}

#[test]
fn state_idle_decodes() {
    let body = read_fixture("state-idle.json");
    let parsed: ServerMessage = serde_json::from_str(&body).expect("state-idle decodes");
    match parsed {
        ServerMessage::State(sf) => {
            assert_eq!(sf.state.current_state, SessionCurrentState::Idle);
        }
        other => panic!("expected State, got {other:?}"),
    }
}

#[test]
fn state_working_decodes() {
    let body = read_fixture("state-working.json");
    let parsed: ServerMessage = serde_json::from_str(&body).expect("state-working decodes");
    match parsed {
        ServerMessage::State(sf) => {
            assert_eq!(sf.state.current_state, SessionCurrentState::Working);
        }
        other => panic!("expected State, got {other:?}"),
    }
}

#[test]
fn state_waitinginput_decodes() {
    let body = read_fixture("state-waitinginput.json");
    let parsed: ServerMessage = serde_json::from_str(&body).expect("state-waitinginput decodes");
    match parsed {
        ServerMessage::State(sf) => {
            assert_eq!(sf.state.current_state, SessionCurrentState::WaitingInput);
            assert_eq!(sf.state.last_event_kind, EventKind::Notification);
        }
        other => panic!("expected State, got {other:?}"),
    }
}

#[test]
fn dropped_frame_decodes() {
    let body = read_fixture("dropped-frame.json");
    let parsed: ServerMessage = serde_json::from_str(&body).expect("dropped-frame decodes");
    match parsed {
        ServerMessage::Dropped(df) => {
            assert_eq!(df.count, 5);
            assert_eq!(df.first_dropped_event_id.0, 10);
            assert_eq!(df.last_dropped_event_id.0, 14);
        }
        other => panic!("expected Dropped, got {other:?}"),
    }
}

#[test]
fn close_frame_decodes() {
    let body = read_fixture("close-frame.json");
    let parsed: ServerMessage = serde_json::from_str(&body).expect("close-frame decodes");
    match parsed {
        ServerMessage::Close(cf) => {
            assert_eq!(cf.reason.as_deref(), Some("daemon shutdown"));
        }
        other => panic!("expected Close, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// REST outbound type fixtures
// ---------------------------------------------------------------------------

#[test]
fn event_list_response_decodes() {
    let body = read_fixture("event-list-response.json");
    let parsed: EventListResponse =
        serde_json::from_str(&body).expect("event-list-response decodes");
    assert_eq!(parsed.events.len(), 3);
    assert_eq!(parsed.cursor.map(|c| c.0), Some(102));
    assert_eq!(parsed.oldest_available_event_id.0, 1);
    assert_eq!(parsed.events[0].kind, EventKind::PreToolUse);
    assert_eq!(parsed.events[2].kind, EventKind::Stop);
}

#[test]
fn session_list_item_array_decodes() {
    let body = read_fixture("session-list-item-array.json");
    let parsed: Vec<SessionListItem> =
        serde_json::from_str(&body).expect("session-list-item-array decodes");
    assert_eq!(parsed.len(), 2);
    assert_eq!(parsed[0].current_state, SessionCurrentState::Working);
    assert_eq!(parsed[1].current_state, SessionCurrentState::Idle);
}

#[test]
fn session_detail_decodes() {
    let body = read_fixture("session-detail.json");
    let parsed: SessionDetail = serde_json::from_str(&body).expect("session-detail decodes");
    assert_eq!(parsed.session_id, "sess-detail");
    assert_eq!(
        parsed.state.current_state,
        SessionCurrentState::WaitingInput
    );
}

#[test]
fn daemon_status_decodes() {
    let body = read_fixture("daemon-status.json");
    let parsed: DaemonStatus = serde_json::from_str(&body).expect("daemon-status decodes");
    assert_eq!(parsed.protocol_version, "1.0");
    assert_eq!(parsed.connected_ws_clients, 3);
    assert_eq!(parsed.last_event_id.map(|e| e.0), Some(102));
}

#[test]
fn daemon_status_null_fields_decodes() {
    let body = read_fixture("daemon-status-null-fields.json");
    let parsed: DaemonStatus =
        serde_json::from_str(&body).expect("daemon-status-null-fields decodes");
    assert!(parsed.last_event_id.is_none());
    assert!(parsed.last_event_at_ms.is_none());
    assert_eq!(parsed.connected_ws_clients, 0);
}

// ---------------------------------------------------------------------------
// Corpus invariants
// ---------------------------------------------------------------------------

#[test]
fn every_corpus_file_is_valid_json() {
    // Walk the corpus dir and assert every `.json` file at least parses as
    // generic `serde_json::Value`. Catches a fixture that gets corrupted by
    // a future hand-edit before the type-specific test would notice.
    let dir = corpus_root();
    let entries = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("read corpus dir {}: {e}", dir.display()));
    let mut count = 0;
    for entry in entries {
        let entry = entry.expect("dir entry");
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("json") {
            continue;
        }
        let body = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
        let _: serde_json::Value = serde_json::from_str(&body).unwrap_or_else(|e| {
            panic!(
                "corpus fixture {} is not valid JSON: {e}",
                path.file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or("<unknown>")
            )
        });
        count += 1;
    }
    assert!(
        count >= 20,
        "expected at least 20 corpus fixtures (AC #2 baseline + Unknown-variant \
         envelope coverage), found {count}; did someone delete fixtures \
         instead of adding them?"
    );
}
