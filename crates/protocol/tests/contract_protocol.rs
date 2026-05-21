use protocol::{
    ClientMessage, EventId, EventKind, HelloFrame, Reaction, ServerMessage, SessionCurrentState,
    SessionState,
};

#[test]
fn event_kind_serializes_pascal_case() {
    assert_eq!(
        serde_json::to_string(&EventKind::PreToolUse).unwrap(),
        "\"PreToolUse\""
    );
    assert_eq!(
        serde_json::to_string(&EventKind::PostToolUse).unwrap(),
        "\"PostToolUse\""
    );
    assert_eq!(serde_json::to_string(&EventKind::Stop).unwrap(), "\"Stop\"");
    assert_eq!(
        serde_json::to_string(&EventKind::Notification).unwrap(),
        "\"Notification\""
    );
    assert_eq!(
        serde_json::to_string(&EventKind::RecordingStarted).unwrap(),
        "\"RecordingStarted\""
    );
    assert_eq!(
        serde_json::to_string(&EventKind::RecordingEnded).unwrap(),
        "\"RecordingEnded\""
    );
}

#[test]
fn event_id_serializes_as_plain_number() {
    assert_eq!(serde_json::to_string(&EventId(42)).unwrap(), "42");
    assert_eq!(serde_json::to_string(&EventId(0)).unwrap(), "0");
    assert_eq!(serde_json::to_string(&EventId(-1)).unwrap(), "-1");
}

#[test]
fn reaction_vendor_serializes_correctly() {
    assert_eq!(
        serde_json::to_string(&Reaction::Vendor(42)).unwrap(),
        "\"Vendor(42)\""
    );
    assert_eq!(
        serde_json::from_str::<Reaction>("\"Vendor(99)\"").unwrap(),
        Reaction::Vendor(99)
    );
}

#[test]
fn reaction_named_variants_round_trip() {
    assert_eq!(
        serde_json::to_string(&Reaction::Pause).unwrap(),
        "\"Pause\""
    );
    assert_eq!(
        serde_json::to_string(&Reaction::Continue).unwrap(),
        "\"Continue\""
    );
    assert_eq!(
        serde_json::to_string(&Reaction::Unknown).unwrap(),
        "\"Unknown\""
    );
    assert_eq!(
        serde_json::from_str::<Reaction>("\"Pause\"").unwrap(),
        Reaction::Pause
    );
    assert_eq!(
        serde_json::from_str::<Reaction>("\"Continue\"").unwrap(),
        Reaction::Continue
    );
    assert_eq!(
        serde_json::from_str::<Reaction>("\"Unknown\"").unwrap(),
        Reaction::Unknown
    );
}

#[test]
fn outbound_type_accepts_unknown_fields() {
    let extra_field = r#"{"protocol_version":"1.0","daemon_version":"0.1.0","oldest_available_event_id":0,"daemon_started_at":0,"history_begins_cleanly":true,"unknown_future_field":"ok"}"#;
    assert!(serde_json::from_str::<HelloFrame>(extra_field).is_ok());
}

#[test]
fn inbound_type_rejects_unknown_fields() {
    let with_unknown = r#"{"op":"subscribe","topic":"events.*","unknown_field":"bad"}"#;
    assert!(serde_json::from_str::<ClientMessage>(with_unknown).is_err());
}

#[test]
fn session_current_state_serializes_pascal_case() {
    assert_eq!(
        serde_json::to_string(&SessionCurrentState::Idle).unwrap(),
        "\"Idle\""
    );
    assert_eq!(
        serde_json::to_string(&SessionCurrentState::Working).unwrap(),
        "\"Working\""
    );
    assert_eq!(
        serde_json::to_string(&SessionCurrentState::WaitingInput).unwrap(),
        "\"WaitingInput\""
    );
}

#[test]
fn session_state_round_trips() {
    let state = SessionState {
        current_state: SessionCurrentState::Working,
        last_event_kind: EventKind::PreToolUse,
        last_event_at_ms: 1_747_574_400_000,
    };
    let json = serde_json::to_string(&state).unwrap();
    let parsed: SessionState = serde_json::from_str(&json).unwrap();
    assert_eq!(state, parsed);
}

#[test]
fn session_state_accepts_unknown_fields() {
    // SessionState is an outbound type — additive forward-compat per
    // crates/protocol's asymmetric serde policy (no deny_unknown_fields).
    let json = r#"{"current_state":"Idle","last_event_kind":"PreToolUse","last_event_at_ms":1234,"future_field":"ignored"}"#;
    let parsed: SessionState = serde_json::from_str(json).unwrap();
    assert_eq!(parsed.current_state, SessionCurrentState::Idle);
    assert_eq!(parsed.last_event_kind, EventKind::PreToolUse);
    assert_eq!(parsed.last_event_at_ms, 1234);
}

#[test]
fn server_message_dispatch_accepts_unknown_fields() {
    // AC#2: permissive deserialization must hold through the ServerMessage tagged-enum
    // dispatch path, not just when deserializing frame structs directly.
    let hello_with_extra = r#"{"op":"hello","protocol_version":"1.0","daemon_version":"0.1.0","oldest_available_event_id":0,"daemon_started_at":0,"history_begins_cleanly":true,"unknown_future_field":"ok"}"#;
    assert!(serde_json::from_str::<ServerMessage>(hello_with_extra).is_ok());
}

// =====================================================================
// Story 1.7 — REST outbound type round-trip + additive-compat
// =====================================================================
//
// The wire-format snapshot mandate (architecture.md:711-713) plus the
// asymmetric-serde policy (architecture.md:606-608, :714) requires every new
// outbound type to (a) round-trip cleanly and (b) accept unknown fields. The
// tests below pin both invariants per new type.

#[test]
fn session_stats_accepts_unknown_fields() {
    let future_json = r#"{
        "source": "claude",
        "session_id": "sess-x",
        "event_count": 12,
        "first_event_at": 1000,
        "last_event_at": 2000,
        "tool_use_breakdown": { "Read": 5, "Bash": 7 }
    }"#;
    let parsed: protocol::SessionStats =
        serde_json::from_str(future_json).expect("forward-compat parse");
    assert_eq!(parsed.event_count, 12);
}

#[test]
fn session_list_item_round_trips() {
    let item = protocol::SessionListItem {
        source: "claude".to_string(),
        session_id: "sess-a".to_string(),
        current_state: SessionCurrentState::Working,
        last_event_kind: EventKind::PreToolUse,
        last_event_at_ms: 1_000_000,
        updated_at: 2_000_000,
    };
    let json = serde_json::to_string(&item).unwrap();
    // Hand-parse back through serde_json::Value so we can also assert specific
    // wire-field names beyond struct equality.
    let v: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(v["source"], "claude");
    assert_eq!(v["session_id"], "sess-a");
    assert_eq!(v["current_state"], "Working");
    assert_eq!(v["last_event_kind"], "PreToolUse");
}

#[test]
fn session_list_item_accepts_unknown_fields() {
    let future_json = r#"{
        "source": "claude",
        "session_id": "sess-a",
        "current_state": "Idle",
        "last_event_kind": "PostToolUse",
        "last_event_at_ms": 0,
        "updated_at": 0,
        "tool_count": 7
    }"#;
    let parsed: protocol::SessionListItem = serde_json::from_str(future_json).unwrap();
    assert_eq!(parsed.session_id, "sess-a");
}

#[test]
fn session_detail_round_trips_and_accepts_unknown_fields() {
    let detail = protocol::SessionDetail {
        source: "claude".to_string(),
        session_id: "sess-x".to_string(),
        state: SessionState {
            current_state: SessionCurrentState::WaitingInput,
            last_event_kind: EventKind::Notification,
            last_event_at_ms: 42,
        },
        updated_at: 100,
    };
    let json = serde_json::to_string(&detail).unwrap();
    let parsed: protocol::SessionDetail = serde_json::from_str(&json).unwrap();
    assert_eq!(
        parsed.state.current_state,
        SessionCurrentState::WaitingInput
    );

    let future_json = r#"{
        "source": "claude",
        "session_id": "s",
        "state": {
            "current_state": "Idle",
            "last_event_kind": "Stop",
            "last_event_at_ms": 0
        },
        "updated_at": 0,
        "annotation": "ok"
    }"#;
    assert!(serde_json::from_str::<protocol::SessionDetail>(future_json).is_ok());
}

#[test]
fn daemon_status_round_trips_and_accepts_unknown_fields() {
    let status = protocol::DaemonStatus {
        daemon_version: "0.1.0".to_string(),
        protocol_version: "1.0".to_string(),
        started_at_ms: 1,
        uptime_ms: 100,
        last_event_at_ms: Some(50),
        last_event_id: Some(EventId(7)),
    };
    let json = serde_json::to_string(&status).unwrap();
    let parsed: protocol::DaemonStatus = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.protocol_version, "1.0");
    assert_eq!(parsed.last_event_id, Some(EventId(7)));

    let future_json = r#"{
        "daemon_version": "9.9.9",
        "protocol_version": "1.0",
        "started_at_ms": 0,
        "uptime_ms": 0,
        "last_event_at_ms": null,
        "last_event_id": null,
        "connected_ws_clients": 3
    }"#;
    let parsed: protocol::DaemonStatus = serde_json::from_str(future_json).unwrap();
    assert!(parsed.last_event_at_ms.is_none());
    assert!(parsed.last_event_id.is_none());
}

#[test]
fn server_message_unknown_variant_round_trips_as_unknown() {
    // Story 2.1 review finding #9: adding new ServerMessage variants must be
    // additive at the wire level. With #[serde(other)] Unknown, an older
    // client deserializing a future-only `op` value should not error — it
    // should map to ServerMessage::Unknown.
    let future_json = r#"{"op":"future_variant_we_have_not_built_yet","payload":{"answer":42}}"#;
    let parsed: protocol::ServerMessage = serde_json::from_str(future_json).unwrap();
    assert!(
        matches!(parsed, protocol::ServerMessage::Unknown),
        "unknown op tag must deserialize to ServerMessage::Unknown, got {parsed:?}"
    );

    // Known variants must still round-trip normally.
    let known = r#"{"op":"close","reason":"goodbye"}"#;
    let parsed: protocol::ServerMessage = serde_json::from_str(known).unwrap();
    assert!(
        matches!(parsed, protocol::ServerMessage::Close(_)),
        "known op must deserialize to its variant, got {parsed:?}"
    );
}
