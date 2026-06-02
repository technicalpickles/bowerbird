use protocol::{
    ClientMessage, EventId, EventKind, HelloFrame, Reaction, ServerMessage, SessionCurrentState,
    SessionState,
};

#[test]
fn event_kind_serializes_pascal_case() {
    // Story 5.2 variant — slotted before PreToolUse in lifecycle order.
    assert_eq!(
        serde_json::to_string(&EventKind::UserPromptSubmit).unwrap(),
        "\"UserPromptSubmit\""
    );
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
    // Story 4.4 / Epic 2 retro AI-4: the decode-only `Unknown` variant
    // serializes back to the literal string `"Unknown"`. The daemon never
    // constructs `Unknown` (it's a wire-decode catch-all), but Serialize must
    // round-trip cleanly so a presenter that does see `Unknown` can re-emit
    // it without losing information.
    assert_eq!(
        serde_json::to_string(&EventKind::Unknown).unwrap(),
        "\"Unknown\""
    );
}

#[test]
fn user_prompt_submit_round_trips() {
    // Story 5.2 — the new variant must round-trip cleanly through serde so
    // every consumer (REST snapshot, WS frame, DB column) sees the same
    // wire string.
    let json = serde_json::to_string(&EventKind::UserPromptSubmit).unwrap();
    assert_eq!(json, "\"UserPromptSubmit\"");
    let parsed: EventKind = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed, EventKind::UserPromptSubmit);
}

#[test]
fn event_kind_unknown_variant_round_trips_as_unknown() {
    // Story 4.4 / Epic 2 retro AI-4 (closes part of #6 in the AC sweep):
    // a future v1.x daemon may emit a new EventKind variant (e.g.
    // `"SubAgentSpawn"`) in `Event.kind`. v1.0 presenters reading the wire
    // must decode it gracefully via `#[serde(other)] Unknown` rather than
    // failing the whole `Event` parse.
    let future_kind = r#""SubAgentSpawn""#;
    let parsed: EventKind = serde_json::from_str(future_kind).unwrap();
    assert!(
        matches!(parsed, EventKind::Unknown),
        "unknown EventKind string must decode to Unknown, got {parsed:?}"
    );

    // Known variants still decode normally.
    let known = r#""PreToolUse""#;
    let parsed: EventKind = serde_json::from_str(known).unwrap();
    assert!(
        matches!(parsed, EventKind::PreToolUse),
        "known EventKind string must decode to its variant, got {parsed:?}"
    );

    // The literal `"Unknown"` itself round-trips.
    let literal_unknown = r#""Unknown""#;
    let parsed: EventKind = serde_json::from_str(literal_unknown).unwrap();
    assert!(matches!(parsed, EventKind::Unknown));
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
fn reaction_unknown_variant_round_trips_via_unknown() {
    // Story 4.4 / Epic 2 retro AI-4 — the LOAD-BEARING behavioral fix. Prior
    // to 2026-05-25, `Reaction::deserialize` returned `Err(...)` on any
    // unknown reaction string, which would have broken the additive-compat
    // claim the moment a future v1.x daemon shipped a new reaction (e.g.
    // `"Block"`). v1.0 presenters would have failed the whole `Event` parse
    // instead of gracefully decoding the reaction as `Unknown`.
    //
    // After the fix, the catch-all maps any unknown string to
    // `Reaction::Unknown`. This test pins the behavior so a future refactor
    // that "tightens" the deserialize back to erroring fails CI loudly.
    let future_reaction = r#""Block""#;
    let parsed: Reaction = serde_json::from_str(future_reaction).unwrap();
    assert_eq!(
        parsed,
        Reaction::Unknown,
        "future reaction string must decode to Unknown for additive-compat"
    );

    // A handful of other future-shipped shapes also round-trip:
    for future in &[
        r#""Allow""#,
        r#""Deny""#,
        r#""Defer""#,
        r#""SomethingNewIn2027""#,
    ] {
        let parsed: Reaction = serde_json::from_str(future).unwrap();
        assert_eq!(parsed, Reaction::Unknown);
    }

    // Malformed `Vendor(...)` shapes still ERROR — they're not additive-
    // compat misses, they're broken payloads. Pinning this so a future
    // refactor doesn't silently swallow Vendor parse errors as Unknown.
    let bad_vendor = r#""Vendor(not-a-number)""#;
    assert!(
        serde_json::from_str::<Reaction>(bad_vendor).is_err(),
        "malformed Vendor(...) must still error, not fall through to Unknown"
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

// Story 5.8 (ADR 0008) AC #13: the `Subscribe.states` field is additive. A
// pre-5.8 presenter omits it; `#[serde(default)]` fills it empty (= unfiltered)
// so the wire shape still parses with no behavior change. NOTE this is the
// INBOUND/strict mirror of the permissive-outbound `additive_compat_*` family
// (`ClientMessage` carries `deny_unknown_fields`, so an *unknown* extra key
// still errors — see `inbound_type_rejects_unknown_fields` above).
#[test]
fn subscribe_without_states_deserializes_to_empty() {
    let no_states = r#"{"op":"subscribe","topic":"state.session.*"}"#;
    let parsed: ClientMessage =
        serde_json::from_str(no_states).expect("pre-5.8 subscribe must still deserialize");
    match parsed {
        ClientMessage::Subscribe { topic, states } => {
            assert_eq!(topic, "state.session.*");
            assert!(
                states.is_empty(),
                "absent states must default to empty (unfiltered)"
            );
        }
        other => panic!("expected Subscribe, got {other:?}"),
    }
}

#[test]
fn subscribe_with_states_round_trips() {
    let with_states = r#"{"op":"subscribe","topic":"state.session.*","states":["working","idle"]}"#;
    let parsed: ClientMessage =
        serde_json::from_str(with_states).expect("subscribe with states must deserialize");
    match &parsed {
        ClientMessage::Subscribe { topic, states } => {
            assert_eq!(topic, "state.session.*");
            assert_eq!(states, &vec!["working".to_string(), "idle".to_string()]);
        }
        other => panic!("expected Subscribe, got {other:?}"),
    }
    // Actually round-trip: ClientMessage derives Serialize (protocol-crate rule
    // that every public type round-trips), so re-serializing then re-parsing
    // must reproduce the value. Compare the parsed values, not raw strings —
    // serde does not guarantee key/field order in the emitted JSON.
    let reserialized = serde_json::to_string(&parsed).expect("ClientMessage must serialize");
    let reparsed: ClientMessage =
        serde_json::from_str(&reserialized).expect("re-serialized subscribe must re-parse");
    assert_eq!(parsed, reparsed, "Subscribe with states must round-trip");
}

#[test]
fn subscribe_unfiltered_serializes_to_v1_0_wire_shape() {
    // `skip_serializing_if = "Vec::is_empty"` means an unfiltered Subscribe
    // serializes to the EXACT pre-5.8 wire bytes (no `states` key), so a Rust
    // presenter that omits the filter is byte-compatible with a v1.0 daemon.
    let unfiltered = ClientMessage::Subscribe {
        topic: "state.session.*".to_string(),
        states: Vec::new(),
    };
    let json = serde_json::to_string(&unfiltered).expect("must serialize");
    assert_eq!(json, r#"{"op":"subscribe","topic":"state.session.*"}"#);
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
    // Story 4.4 / Epic 2 retro AI-4: the decode-only `Unknown` variant
    // serializes back to the literal string `"Unknown"` so a presenter that
    // sees a future v1.x state can re-emit it without information loss.
    assert_eq!(
        serde_json::to_string(&SessionCurrentState::Unknown).unwrap(),
        "\"Unknown\""
    );
}

#[test]
fn session_current_state_unknown_variant_round_trips_as_unknown() {
    // Story 4.4 / Epic 2 retro AI-4: a future v1.x daemon may add a state
    // (e.g. `"Compacting"`, `"AwaitingApproval"`). v1.0 presenters must
    // decode it as `SessionCurrentState::Unknown` via `#[serde(other)]`
    // rather than failing the whole `StateFrame` parse.
    let future_state = r#""Compacting""#;
    let parsed: SessionCurrentState = serde_json::from_str(future_state).unwrap();
    assert!(
        matches!(parsed, SessionCurrentState::Unknown),
        "unknown state must decode to Unknown, got {parsed:?}"
    );

    // Known variants still decode normally.
    let known = r#""Working""#;
    let parsed: SessionCurrentState = serde_json::from_str(known).unwrap();
    assert!(matches!(parsed, SessionCurrentState::Working));

    // The literal `"Unknown"` itself round-trips.
    let literal_unknown = r#""Unknown""#;
    let parsed: SessionCurrentState = serde_json::from_str(literal_unknown).unwrap();
    assert!(matches!(parsed, SessionCurrentState::Unknown));
}

#[test]
fn session_state_round_trips() {
    let state = SessionState {
        current_state: SessionCurrentState::Working,
        last_event_kind: EventKind::PreToolUse,
        last_event_at_ms: 1_747_574_400_000,
        last_pid: Some(12345),
        cwd: None,
        started_at: None,
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
        last_pid: Some(42),
        cwd: None,
        started_at: None,
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
            last_pid: Some(7),
            cwd: None,
            started_at: None,
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
        connected_ws_clients: 2,
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

// ─── Story 5.3 — wire variants + additive-compat ────────────────────────────

#[test]
fn session_ended_event_kind_serializes_pascal_case() {
    assert_eq!(
        serde_json::to_string(&EventKind::SessionEnded).unwrap(),
        "\"SessionEnded\""
    );
    let parsed: EventKind = serde_json::from_str(r#""SessionEnded""#).unwrap();
    assert_eq!(parsed, EventKind::SessionEnded);
}

#[test]
fn ended_session_current_state_serializes_pascal_case() {
    assert_eq!(
        serde_json::to_string(&SessionCurrentState::Ended).unwrap(),
        "\"Ended\""
    );
    let parsed: SessionCurrentState = serde_json::from_str(r#""Ended""#).unwrap();
    assert_eq!(parsed, SessionCurrentState::Ended);
}

#[test]
fn notification_type_wire_form_is_snake_case() {
    use protocol::NotificationType;
    // Each known variant has an explicit serde rename to snake_case — pin
    // the wire string for every one so a future rename can't silently change
    // the contract.
    let cases: &[(NotificationType, &str)] = &[
        (NotificationType::PermissionPrompt, "permission_prompt"),
        (NotificationType::IdlePrompt, "idle_prompt"),
        (NotificationType::AuthSuccess, "auth_success"),
        (NotificationType::ElicitationDialog, "elicitation_dialog"),
        (
            NotificationType::ElicitationResponse,
            "elicitation_response",
        ),
        (
            NotificationType::ElicitationComplete,
            "elicitation_complete",
        ),
    ];
    for (variant, wire) in cases {
        let serialized = serde_json::to_string(variant).unwrap();
        assert_eq!(
            serialized,
            format!("\"{wire}\""),
            "variant {variant:?} should serialize to {wire:?}"
        );
        let parsed: NotificationType =
            serde_json::from_str(&format!("\"{wire}\"")).expect("round trip");
        assert_eq!(parsed, *variant);
    }
    // Future variant decodes to Unknown via #[serde(other)].
    let future = r#""future_notification_type_v2""#;
    let parsed: NotificationType = serde_json::from_str(future).unwrap();
    assert_eq!(parsed, NotificationType::Unknown);
}

// Story 5.3 AC #17: a v1.0 presenter compiled against the pre-5.3 protocol
// must decode an `Ended` `current_state` as `Unknown` via the
// `#[serde(other)]` catch-all — no decode error, no crash. Mock the legacy
// shape with only the v1.0 variants.
#[test]
fn additive_compat_ended_session_current_state_decodes_as_unknown() {
    use serde::{Deserialize, Serialize};
    #[derive(Debug, Deserialize, Serialize, PartialEq, Eq, Clone, Copy)]
    enum LegacySessionCurrentState {
        Idle,
        Working,
        WaitingInput,
        #[serde(other)]
        Unknown,
    }
    #[derive(Debug, Deserialize)]
    #[allow(dead_code)]
    struct LegacySessionState {
        current_state: LegacySessionCurrentState,
        last_event_kind: EventKind,
        last_event_at_ms: i64,
    }
    let wire = r#"{
        "current_state": "Ended",
        "last_event_kind": "Stop",
        "last_event_at_ms": 0,
        "last_pid": 12345
    }"#;
    let parsed: LegacySessionState = serde_json::from_str(wire).expect("legacy decode");
    assert_eq!(parsed.current_state, LegacySessionCurrentState::Unknown);
}

// Story 5.3 AC #17: a v1.0 presenter must decode a `SessionEnded` `Event`
// `kind` field as `Unknown` via the same catch-all.
#[test]
fn additive_compat_session_ended_event_kind_decodes_as_unknown() {
    use serde::{Deserialize, Serialize};
    #[derive(Debug, Deserialize, Serialize, PartialEq, Eq, Clone)]
    enum LegacyEventKind {
        UserPromptSubmit,
        PreToolUse,
        PostToolUse,
        Stop,
        Notification,
        RecordingStarted,
        RecordingEnded,
        #[serde(other)]
        Unknown,
    }
    #[derive(Debug, Deserialize)]
    #[allow(dead_code)]
    struct LegacyEvent {
        event_id: i64,
        source: String,
        session_id: String,
        kind: LegacyEventKind,
        reaction: Option<String>,
        payload: String,
        created_at: i64,
    }
    let wire = r#"{
        "event_id": 1,
        "source": "claude",
        "session_id": "s1",
        "kind": "SessionEnded",
        "reaction": null,
        "payload": "{\"reason\":\"pid_dead\",\"pid\":12345,\"observed_at_ms\":0}",
        "created_at": 0,
        "pid": 12345
    }"#;
    let parsed: LegacyEvent = serde_json::from_str(wire).expect("legacy decode");
    assert_eq!(parsed.kind, LegacyEventKind::Unknown);
}

// Story 5.3 AC #17: outbound types must not carry `deny_unknown_fields`, so
// a v1.0 consumer reading a v1.x SessionState frame with the new `last_pid`
// field decodes silently — the field is dropped on the legacy side.
#[test]
fn additive_compat_last_pid_is_ignored_by_v1_consumer() {
    use serde::Deserialize;
    #[derive(Debug, Deserialize)]
    #[allow(dead_code)]
    struct LegacySessionState {
        current_state: SessionCurrentState,
        last_event_kind: EventKind,
        last_event_at_ms: i64,
    }
    let wire = r#"{
        "current_state": "Working",
        "last_event_kind": "PreToolUse",
        "last_event_at_ms": 42,
        "last_pid": 12345
    }"#;
    let parsed: LegacySessionState = serde_json::from_str(wire).expect("legacy decode");
    assert_eq!(parsed.current_state, SessionCurrentState::Working);
    assert_eq!(parsed.last_event_at_ms, 42);
}

// Same canary for `Event.pid`: a v1.0 presenter must ignore the new
// `pid` field on stored Event without erroring.
#[test]
fn additive_compat_pid_is_ignored_by_v1_consumer() {
    use serde::Deserialize;
    #[derive(Debug, Deserialize)]
    #[allow(dead_code)]
    struct LegacyEvent {
        event_id: i64,
        source: String,
        session_id: String,
        kind: EventKind,
        reaction: Option<String>,
        payload: String,
        created_at: i64,
    }
    let wire = r#"{
        "event_id": 5,
        "source": "claude",
        "session_id": "s1",
        "kind": "PreToolUse",
        "reaction": null,
        "payload": "{}",
        "created_at": 0,
        "pid": 12345
    }"#;
    let parsed: LegacyEvent = serde_json::from_str(wire).expect("legacy decode");
    assert_eq!(parsed.event_id, 5);
    assert_eq!(parsed.kind, EventKind::PreToolUse);
}

// Story 5.3 review finding #7: AC #17 names `SessionState`, `StateFrame`,
// AND `EventFrame` decode paths. The bare-struct tests above pin the inner
// shape; the four tests below pin the actual WS wire shape — a v1.0
// presenter compiled against the pre-5.3 protocol decodes a 5.3+ daemon's
// `ServerMessage::{Event,State}` frames containing `SessionEnded` /
// `Ended` / `last_pid` / `pid` without erroring, mapping unknown variants
// to `Unknown` via `#[serde(other)]` and dropping unknown fields per the
// asymmetric outbound permissive policy.

// Legacy mocks reused by the four tests below. Each mirrors the v1.0
// shape: no `Ended` / no `SessionEnded` / no `last_pid` / no `pid`.
mod legacy_v1_0_mocks {
    use serde::{Deserialize, Serialize};

    #[derive(Debug, Deserialize, Serialize, PartialEq, Eq, Clone, Copy)]
    pub(super) enum LegacySessionCurrentState {
        Idle,
        Working,
        WaitingInput,
        #[serde(other)]
        Unknown,
    }

    #[derive(Debug, Deserialize, Serialize, PartialEq, Eq, Clone)]
    pub(super) enum LegacyEventKind {
        UserPromptSubmit,
        PreToolUse,
        PostToolUse,
        Stop,
        Notification,
        RecordingStarted,
        RecordingEnded,
        #[serde(other)]
        Unknown,
    }

    #[derive(Debug, Deserialize)]
    #[allow(dead_code)]
    pub(super) struct LegacySessionState {
        pub current_state: LegacySessionCurrentState,
        pub last_event_kind: LegacyEventKind,
        pub last_event_at_ms: i64,
    }

    #[derive(Debug, Deserialize)]
    #[allow(dead_code)]
    pub(super) struct LegacyEvent {
        pub event_id: i64,
        pub source: String,
        pub session_id: String,
        pub kind: LegacyEventKind,
        pub reaction: Option<String>,
        pub payload: String,
        pub created_at: i64,
    }

    #[derive(Debug, Deserialize)]
    #[allow(dead_code)]
    pub(super) struct LegacyEventFrame {
        pub event: LegacyEvent,
    }

    #[derive(Debug, Deserialize)]
    #[allow(dead_code)]
    pub(super) struct LegacyStateFrame {
        pub source: String,
        pub session_id: String,
        pub state: LegacySessionState,
    }

    /// Pre-5.7 shape of one `GET /sessions` array entry — no `cwd` /
    /// `started_at`. AC #10/#13: a v1.0 presenter must decode a 5.7+ list
    /// item carrying both new fields without erroring.
    #[derive(Debug, Deserialize)]
    #[allow(dead_code)]
    pub(super) struct LegacySessionListItem {
        pub source: String,
        pub session_id: String,
        pub current_state: LegacySessionCurrentState,
        pub last_event_kind: LegacyEventKind,
        pub last_event_at_ms: i64,
        pub updated_at: i64,
        pub last_pid: Option<u32>,
    }

    #[derive(Debug, Deserialize)]
    #[serde(tag = "op", rename_all = "snake_case")]
    #[allow(dead_code)]
    pub(super) enum LegacyServerMessage {
        Hello, // accept any payload silently
        Sync,  // ditto
        Event(LegacyEventFrame),
        State(LegacyStateFrame),
        Dropped, // ditto
        Close,   // ditto
        #[serde(other)]
        Unknown,
    }
}

// Story 5.3 finding #7: full WS `ServerMessage::State` payload with `Ended`
// + `last_pid` decoded through the legacy ServerMessage dispatcher. The
// outer op variant matches (`State`), the inner `current_state` falls
// through to `LegacySessionCurrentState::Unknown`, and the unknown
// `last_pid` field is silently ignored — no error.
#[test]
fn additive_compat_state_frame_with_ended_decodes_through_server_message() {
    use legacy_v1_0_mocks::*;
    let wire = r#"{
        "op": "state",
        "source": "claude",
        "session_id": "s1",
        "state": {
            "current_state": "Ended",
            "last_event_kind": "Stop",
            "last_event_at_ms": 0,
            "last_pid": 12345
        }
    }"#;
    let parsed: LegacyServerMessage =
        serde_json::from_str(wire).expect("legacy ServerMessage decode");
    match parsed {
        LegacyServerMessage::State(frame) => {
            assert_eq!(frame.source, "claude");
            assert_eq!(frame.session_id, "s1");
            assert_eq!(
                frame.state.current_state,
                LegacySessionCurrentState::Unknown,
                "Ended must decode as Unknown through the legacy SessionCurrentState"
            );
        }
        other => panic!("expected State variant, got {other:?}"),
    }
}

// Story 5.3 finding #7: full WS `ServerMessage::Event` payload with
// `SessionEnded` + `pid` decoded through the legacy dispatcher. The outer
// op variant matches (`Event`), the inner `kind` falls through to
// `LegacyEventKind::Unknown`, and the unknown `pid` field on Event is
// silently dropped.
#[test]
fn additive_compat_event_frame_with_session_ended_decodes_through_server_message() {
    use legacy_v1_0_mocks::*;
    let wire = r#"{
        "op": "event",
        "event": {
            "event_id": 7,
            "source": "claude",
            "session_id": "s1",
            "kind": "SessionEnded",
            "reaction": null,
            "payload": "{\"reason\":\"pid_dead\",\"pid\":12345,\"observed_at_ms\":0}",
            "created_at": 0,
            "pid": 12345
        }
    }"#;
    let parsed: LegacyServerMessage =
        serde_json::from_str(wire).expect("legacy ServerMessage decode");
    match parsed {
        LegacyServerMessage::Event(frame) => {
            assert_eq!(frame.event.event_id, 7);
            assert_eq!(
                frame.event.kind,
                LegacyEventKind::Unknown,
                "SessionEnded must decode as Unknown through the legacy EventKind"
            );
        }
        other => panic!("expected Event variant, got {other:?}"),
    }
}

// Story 5.3 finding #7: `EventFrame` decoded directly (without the outer
// ServerMessage envelope, as used by replay export / event-stream tools).
// Same expectation: `SessionEnded` kind maps to Unknown, `pid` is dropped.
#[test]
fn additive_compat_event_frame_with_session_ended_decodes_directly() {
    use legacy_v1_0_mocks::*;
    let wire = r#"{
        "event": {
            "event_id": 7,
            "source": "claude",
            "session_id": "s1",
            "kind": "SessionEnded",
            "reaction": null,
            "payload": "{}",
            "created_at": 0,
            "pid": 12345
        }
    }"#;
    let parsed: LegacyEventFrame = serde_json::from_str(wire).expect("legacy EventFrame decode");
    assert_eq!(parsed.event.kind, LegacyEventKind::Unknown);
    assert_eq!(parsed.event.event_id, 7);
}

// Story 5.3 finding #7: `StateFrame` decoded directly (without the outer
// ServerMessage envelope, as used by snapshot-on-subscribe consumers that
// pull the inner shape via REST `/sessions/{id}` and reuse the type).
// Same expectation: `Ended` maps to Unknown, `last_pid` is dropped.
#[test]
fn additive_compat_state_frame_with_ended_decodes_directly() {
    use legacy_v1_0_mocks::*;
    let wire = r#"{
        "source": "claude",
        "session_id": "s1",
        "state": {
            "current_state": "Ended",
            "last_event_kind": "Stop",
            "last_event_at_ms": 0,
            "last_pid": 12345
        }
    }"#;
    let parsed: LegacyStateFrame = serde_json::from_str(wire).expect("legacy StateFrame decode");
    assert_eq!(
        parsed.state.current_state,
        LegacySessionCurrentState::Unknown
    );
    assert_eq!(parsed.source, "claude");
}

// Story 5.7 AC #10/#13: the asymmetric outbound-permissive policy means a v1.0
// presenter compiled before Story 5.7 decodes a 5.7+ daemon's `StateFrame`
// (carrying `cwd` + `started_at`) and `EventFrame` (carrying `cwd`) without
// erroring — the unknown fields are silently dropped. Decode through the actual
// nested WS frame shape (ServerMessage), per Story 5.3 review finding #7.
#[test]
fn additive_compat_cwd_and_started_at_ignored_by_v1_consumer() {
    use legacy_v1_0_mocks::*;

    // StateFrame via ServerMessage: state carries cwd + started_at.
    let state_wire = r#"{
        "op": "state",
        "source": "claude",
        "session_id": "s1",
        "state": {
            "current_state": "Working",
            "last_event_kind": "PreToolUse",
            "last_event_at_ms": 42,
            "last_pid": 12345,
            "cwd": "/Users/x/repo",
            "started_at": 1717200000000
        }
    }"#;
    let parsed: LegacyServerMessage =
        serde_json::from_str(state_wire).expect("legacy ServerMessage decode with cwd/started_at");
    match parsed {
        LegacyServerMessage::State(frame) => {
            assert_eq!(frame.session_id, "s1");
            assert_eq!(
                frame.state.current_state,
                LegacySessionCurrentState::Working
            );
            assert_eq!(frame.state.last_event_at_ms, 42);
        }
        other => panic!("expected State variant, got {other:?}"),
    }

    // EventFrame via ServerMessage: event carries cwd.
    let event_wire = r#"{
        "op": "event",
        "event": {
            "event_id": 9,
            "source": "claude",
            "session_id": "s1",
            "kind": "PreToolUse",
            "reaction": null,
            "payload": "{}",
            "created_at": 0,
            "pid": 12345,
            "cwd": "/Users/x/repo"
        }
    }"#;
    let parsed: LegacyServerMessage =
        serde_json::from_str(event_wire).expect("legacy ServerMessage decode with cwd");
    match parsed {
        LegacyServerMessage::Event(frame) => {
            assert_eq!(frame.event.event_id, 9);
            assert_eq!(frame.event.kind, LegacyEventKind::PreToolUse);
        }
        other => panic!("expected Event variant, got {other:?}"),
    }
}

// Story 5.7 review pass 2 (AC #10/#13): a v1.0 presenter decodes one
// `GET /sessions` list item from a 5.7+ daemon carrying `cwd` + `started_at`
// without erroring — the unknown fields drop silently and the known fields
// decode unchanged. The other compat test decodes `StateFrame`/`EventFrame`
// through `ServerMessage`; the existing list-item round-trip test uses the
// CURRENT `SessionListItem` type. This pins the LEGACY `SessionListItem` shape
// (lacking both fields) the REST `/sessions` array consumer sees.
#[test]
fn additive_compat_session_list_item_ignores_cwd_and_started_at() {
    use legacy_v1_0_mocks::*;

    let item_wire = r#"{
        "source": "claude",
        "session_id": "s1",
        "current_state": "Working",
        "last_event_kind": "PreToolUse",
        "last_event_at_ms": 42,
        "updated_at": 99,
        "last_pid": 12345,
        "cwd": "/Users/x/repo",
        "started_at": 1717200000000
    }"#;
    let parsed: LegacySessionListItem = serde_json::from_str(item_wire)
        .expect("legacy SessionListItem decode with cwd/started_at must succeed");
    assert_eq!(parsed.source, "claude");
    assert_eq!(parsed.session_id, "s1");
    assert_eq!(parsed.current_state, LegacySessionCurrentState::Working);
    assert_eq!(parsed.last_event_kind, LegacyEventKind::PreToolUse);
    assert_eq!(parsed.last_event_at_ms, 42);
    assert_eq!(parsed.updated_at, 99);
    assert_eq!(parsed.last_pid, Some(12345));
}

// Story 5.7 AC #13 (inverse direction): a CURRENT `SessionState` deserializes a
// pre-5.7 projection blob that LACKS both `cwd` and `started_at` → both default
// to `None`, no error. This is the no-migration read path: serde defaults absent
// `Option` fields (same mechanism `last_pid` relied on in Story 5.3).
#[test]
fn pre_5_7_session_state_blob_reads_back_with_none_cwd_and_started_at() {
    use protocol::SessionState;
    let pre_5_7 = r#"{
        "current_state": "Idle",
        "last_event_kind": "Stop",
        "last_event_at_ms": 7,
        "last_pid": null
    }"#;
    let parsed: SessionState = serde_json::from_str(pre_5_7).expect("pre-5.7 blob decodes");
    assert_eq!(parsed.cwd, None);
    assert_eq!(parsed.started_at, None);
    assert_eq!(parsed.last_event_at_ms, 7);
}

// Story 5.7: cwd + started_at round-trip through serialize → deserialize for
// both Some and None.
#[test]
fn session_state_and_event_cwd_started_at_round_trip() {
    use protocol::{Event, EventId, EventKind, SessionCurrentState, SessionState};

    for (cwd, started_at) in [
        (
            Some("/Users/x/repo".to_string()),
            Some(1_717_200_000_000_i64),
        ),
        (None, None),
    ] {
        let state = SessionState {
            current_state: SessionCurrentState::Working,
            last_event_kind: EventKind::PreToolUse,
            last_event_at_ms: 1,
            last_pid: Some(5),
            cwd: cwd.clone(),
            started_at,
        };
        let back: SessionState =
            serde_json::from_str(&serde_json::to_string(&state).unwrap()).unwrap();
        assert_eq!(back, state);

        // Event carries cwd (not started_at).
        let event = Event {
            event_id: EventId(1),
            source: "claude".to_string(),
            session_id: "s1".to_string(),
            kind: EventKind::PreToolUse,
            reaction: None,
            payload: "{}".to_string(),
            created_at: 0,
            pid: None,
            cwd: cwd.clone(),
        };
        let back: Event = serde_json::from_str(&serde_json::to_string(&event).unwrap()).unwrap();
        assert_eq!(back.cwd, cwd);
    }
}
