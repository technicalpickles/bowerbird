use protocol::{ClientMessage, EventId, EventKind, HelloFrame, Reaction, ServerMessage};

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
fn server_message_dispatch_accepts_unknown_fields() {
    // AC#2: permissive deserialization must hold through the ServerMessage tagged-enum
    // dispatch path, not just when deserializing frame structs directly.
    let hello_with_extra = r#"{"op":"hello","protocol_version":"1.0","daemon_version":"0.1.0","oldest_available_event_id":0,"daemon_started_at":0,"history_begins_cleanly":true,"unknown_future_field":"ok"}"#;
    assert!(serde_json::from_str::<ServerMessage>(hello_with_extra).is_ok());
}
