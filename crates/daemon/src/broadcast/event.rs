//! Internal broadcast envelope and topic parsing/matching.
//!
//! `BroadcastEnvelope` is the value carried on `tokio::sync::broadcast`
//! channels; per-connection tasks project it into a wire `ServerMessage`
//! after the topic-match check.
//!
//! `Topic` parses subscription wire strings into a finite enum. The grammar
//! is exact — see `Topic::parse` for the supported strings.

use protocol::{Event, SessionState};

/// The internal value carried on `tokio::sync::broadcast` channels.
/// One variant per topic class. The per-connection task projects this
/// into a wire `ServerMessage` after the topic-match check.
#[derive(Debug, Clone)]
pub enum BroadcastEnvelope {
    /// A new event was committed to the events table.
    /// Topic: `events.<source>.<session_id>` (matches both `events.*`
    /// and `events.<source>.*` and `events.<source>.<session_id>`).
    Event(Event),
    /// A session's projection was updated.
    /// Topic: `state.session.<session_id>`
    /// Multiple `state.session.<id>.<field>` sub-topics may match the
    /// same envelope — see `Topic::matches`.
    State {
        source: String,
        session_id: String,
        state: SessionState,
    },
    // Story 2.4 will add: Dropped { count, first_id, last_id, recipient_token }
    // Story 2.5 will add: ShutdownClose
}

/// A parsed subscription topic. Stored per-connection.
///
/// Supported wire strings:
///
/// | Wire string                                | Variant                              |
/// |--------------------------------------------|--------------------------------------|
/// | `events.*`                                 | `EventsAll`                          |
/// | `events.<source>.*`                        | `EventsBySource(...)`                |
/// | `events.<source>.<session_id>`             | `EventsBySourceSession(..., ...)`    |
/// | `state.session.*`                          | `StateAll`                           |
/// | `state.session.<id>`                       | `StateSession(...)`                  |
/// | `state.session.<id>.current_state`         | `StateSessionCurrent(...)`           |
/// | empty / anything else                      | `Err(())`                            |
///
/// `state.session.<id>.current_state` is a strict-subset filter of
/// `StateSession`: when a `State` envelope arrives, both `StateSession(id)`
/// and `StateSessionCurrent(id)` match — they do not filter different
/// content in Story 2.1.
///
/// `StateSession` is single-key on `session_id` (no `source` discriminator)
/// because V1 only has the `"claude"` source. See
/// `docs/bmad/implementation-artifacts/deferred-work.md` line 51 for the
/// per-source disambiguation deferred-work entry that will apply when a
/// second adapter ships.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Topic {
    EventsAll,
    EventsBySource(String),
    EventsBySourceSession(String, String),
    StateAll,
    StateSession(String),
    StateSessionCurrent(String),
}

impl Topic {
    /// Parse a wire-format topic string. `Err(())` on unrecognized shape or
    /// empty input. Empty is rejected here so the WS handler can route it
    /// to the `bad message:` close path (AC #4).
    ///
    /// The unit-error return type is intentional: the WS layer only needs
    /// to know "couldn't parse" — the offending string is already available
    /// to the caller for the close-frame reason. Adding an error enum would
    /// be ceremony without payoff at this layer.
    #[allow(clippy::result_unit_err)]
    pub fn parse(s: &str) -> Result<Self, ()> {
        if s.is_empty() {
            return Err(());
        }
        if s.ends_with('.') {
            return Err(());
        }

        let segments: Vec<&str> = s.split('.').collect();
        // Reject any empty interior segment (catches "events..foo", leading dots, etc).
        if segments.iter().any(|seg| seg.is_empty()) {
            return Err(());
        }

        // Reject `*` in any literal `<source>` / `<session_id>` / `<id>`
        // position. Without this, strings like `events.*.*`, `events.*.sess-1`,
        // and `state.session.*.current_state` would parse as literal-source
        // subscriptions, violating the exact-grammar contract.
        fn literal_ok(s: &str) -> bool {
            s != "*"
        }

        match segments.as_slice() {
            ["events", "*"] => Ok(Topic::EventsAll),
            ["events", source, "*"] if literal_ok(source) => {
                Ok(Topic::EventsBySource((*source).to_string()))
            }
            ["events", source, session_id] if literal_ok(source) && literal_ok(session_id) => Ok(
                Topic::EventsBySourceSession((*source).to_string(), (*session_id).to_string()),
            ),
            ["state", "session", "*"] => Ok(Topic::StateAll),
            ["state", "session", id] if literal_ok(id) => {
                Ok(Topic::StateSession((*id).to_string()))
            }
            ["state", "session", id, "current_state"] if literal_ok(id) => {
                Ok(Topic::StateSessionCurrent((*id).to_string()))
            }
            _ => Err(()),
        }
    }

    /// True if this subscription topic should deliver `envelope`.
    pub fn matches(&self, envelope: &BroadcastEnvelope) -> bool {
        match (self, envelope) {
            (Topic::EventsAll, BroadcastEnvelope::Event(_)) => true,
            (Topic::EventsBySource(want_source), BroadcastEnvelope::Event(ev)) => {
                ev.source == *want_source
            }
            (Topic::EventsBySourceSession(want_source, want_sid), BroadcastEnvelope::Event(ev)) => {
                ev.source == *want_source && ev.session_id == *want_sid
            }
            (Topic::StateAll, BroadcastEnvelope::State { .. }) => true,
            (Topic::StateSession(want_sid), BroadcastEnvelope::State { session_id, .. }) => {
                session_id == want_sid
            }
            (Topic::StateSessionCurrent(want_sid), BroadcastEnvelope::State { session_id, .. }) => {
                session_id == want_sid
            }
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use protocol::{Event, EventId, EventKind, Reaction, SessionCurrentState, SessionState};

    fn make_event(source: &str, session_id: &str) -> Event {
        Event {
            event_id: EventId(1),
            source: source.to_string(),
            session_id: session_id.to_string(),
            kind: EventKind::PreToolUse,
            reaction: Some(Reaction::Continue),
            payload: "{}".to_string(),
            created_at: 0,
            pid: None,
        }
    }

    fn make_state(source: &str, session_id: &str) -> BroadcastEnvelope {
        BroadcastEnvelope::State {
            source: source.to_string(),
            session_id: session_id.to_string(),
            state: SessionState {
                current_state: SessionCurrentState::Working,
                last_event_kind: EventKind::PreToolUse,
                last_event_at_ms: 0,
                last_pid: None,
            },
        }
    }

    #[test]
    fn parse_events_all() {
        assert_eq!(Topic::parse("events.*"), Ok(Topic::EventsAll));
    }

    #[test]
    fn parse_events_by_source() {
        assert_eq!(
            Topic::parse("events.claude.*"),
            Ok(Topic::EventsBySource("claude".to_string()))
        );
    }

    #[test]
    fn parse_events_by_source_session() {
        assert_eq!(
            Topic::parse("events.claude.sess-1"),
            Ok(Topic::EventsBySourceSession(
                "claude".to_string(),
                "sess-1".to_string()
            ))
        );
    }

    #[test]
    fn parse_state_all() {
        assert_eq!(Topic::parse("state.session.*"), Ok(Topic::StateAll));
    }

    #[test]
    fn parse_state_session() {
        assert_eq!(
            Topic::parse("state.session.sess-1"),
            Ok(Topic::StateSession("sess-1".to_string()))
        );
    }

    #[test]
    fn parse_state_session_current() {
        assert_eq!(
            Topic::parse("state.session.sess-1.current_state"),
            Ok(Topic::StateSessionCurrent("sess-1".to_string()))
        );
    }

    #[test]
    fn parse_rejects_empty() {
        assert!(Topic::parse("").is_err());
    }

    #[test]
    fn parse_rejects_unknown_prefix() {
        assert!(Topic::parse("metrics.foo").is_err());
        assert!(Topic::parse("bogus").is_err());
    }

    #[test]
    fn parse_rejects_too_few_segments() {
        assert!(Topic::parse("events").is_err());
        assert!(Topic::parse("state").is_err());
        assert!(Topic::parse("state.session").is_err());
    }

    #[test]
    fn parse_rejects_too_many_segments() {
        assert!(Topic::parse("events.claude.sess-1.extra").is_err());
        assert!(Topic::parse("state.session.sess-1.current_state.extra").is_err());
    }

    #[test]
    fn parse_rejects_trailing_dot() {
        assert!(Topic::parse("events.").is_err());
        assert!(Topic::parse("events.claude.").is_err());
    }

    #[test]
    fn parse_rejects_empty_interior_segment() {
        assert!(Topic::parse("events..claude").is_err());
        assert!(Topic::parse(".events.*").is_err());
    }

    #[test]
    fn parse_rejects_wildcard_in_literal_positions() {
        // Review finding: `*` must only appear in the wildcard positions
        // documented by the grammar, not as a stand-in for literal
        // <source>/<session_id>/<id> segments.
        assert!(
            Topic::parse("events.*.*").is_err(),
            "events.*.* must not parse as EventsBySource(\"*\")"
        );
        assert!(
            Topic::parse("events.*.sess-1").is_err(),
            "events.*.sess-1 must not parse as EventsBySourceSession(\"*\", \"sess-1\")"
        );
        assert!(
            Topic::parse("events.claude.*.extra").is_err(),
            "trailing segment after wildcard must still be rejected"
        );
        assert!(
            Topic::parse("state.session.*.current_state").is_err(),
            "state.session.*.current_state must not parse as StateSessionCurrent(\"*\")"
        );
    }

    #[test]
    fn matches_events_all_matches_any_event() {
        let t = Topic::EventsAll;
        assert!(t.matches(&BroadcastEnvelope::Event(make_event("claude", "sess-1"))));
        assert!(t.matches(&BroadcastEnvelope::Event(make_event("codex", "sess-2"))));
    }

    #[test]
    fn matches_events_by_source_filters_other_sources() {
        let t = Topic::EventsBySource("claude".to_string());
        assert!(t.matches(&BroadcastEnvelope::Event(make_event("claude", "sess-1"))));
        assert!(!t.matches(&BroadcastEnvelope::Event(make_event("codex", "sess-1"))));
    }

    #[test]
    fn matches_events_by_source_session() {
        let t = Topic::EventsBySourceSession("claude".to_string(), "sess-1".to_string());
        assert!(t.matches(&BroadcastEnvelope::Event(make_event("claude", "sess-1"))));
        assert!(!t.matches(&BroadcastEnvelope::Event(make_event("claude", "sess-2"))));
        assert!(!t.matches(&BroadcastEnvelope::Event(make_event("codex", "sess-1"))));
    }

    #[test]
    fn matches_state_all_matches_any_state_envelope() {
        let t = Topic::StateAll;
        assert!(t.matches(&make_state("claude", "sess-1")));
        assert!(t.matches(&make_state("codex", "sess-2")));
    }

    #[test]
    fn matches_state_session_does_not_match_other_session() {
        let t = Topic::StateSession("sess-1".to_string());
        assert!(t.matches(&make_state("claude", "sess-1")));
        assert!(!t.matches(&make_state("claude", "sess-2")));
    }

    #[test]
    fn matches_state_session_current_does_not_match_other_session() {
        let t = Topic::StateSessionCurrent("sess-1".to_string());
        assert!(t.matches(&make_state("claude", "sess-1")));
        assert!(!t.matches(&make_state("claude", "sess-2")));
    }

    #[test]
    fn matches_events_does_not_match_state_envelope() {
        let t = Topic::EventsAll;
        assert!(!t.matches(&make_state("claude", "sess-1")));
    }

    #[test]
    fn matches_state_does_not_match_event_envelope() {
        let t = Topic::StateAll;
        assert!(!t.matches(&BroadcastEnvelope::Event(make_event("claude", "sess-1"))));
    }
}
