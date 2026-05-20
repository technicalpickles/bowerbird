use protocol::{EventKind, Reaction};

pub const INSERT_EVENT: &str =
    "INSERT INTO events (source, session_id, kind, reaction, payload, created_at) \
     VALUES (?, ?, ?, ?, ?, ?)";

pub const UPSERT_SESSION_PROJECTION: &str =
    "INSERT INTO session_projections (source, session_id, state, updated_at) \
     VALUES (?, ?, ?, ?) \
     ON CONFLICT(source, session_id) \
     DO UPDATE SET state = excluded.state, updated_at = excluded.updated_at";

pub const INSERT_RECORDING_SESSION_STARTED: &str =
    "INSERT INTO recording_sessions (started_event_id, ended_event_id) VALUES (?, NULL)";

pub const UPDATE_RECORDING_SESSION_ENDED: &str =
    "UPDATE recording_sessions SET ended_event_id = ? WHERE id = ?";

pub const SELECT_EVENT_BY_ID: &str =
    "SELECT event_id, source, session_id, kind, reaction, payload, created_at \
     FROM events WHERE event_id = ?";

pub const SELECT_SESSION_PROJECTION_STATE: &str =
    "SELECT state FROM session_projections WHERE source = ? AND session_id = ?";

pub const SELECT_DISTINCT_SESSIONS_FROM_EVENTS: &str =
    "SELECT DISTINCT source, session_id FROM events WHERE source != '__daemon__'";

pub const SELECT_EVENT_KINDS_FOR_SESSION: &str =
    "SELECT kind, created_at FROM events WHERE source = ? AND session_id = ? \
     ORDER BY event_id ASC";

/// Stable wire string for an [`EventKind`] used by the daemon's SQLite storage.
///
/// Delegates to the protocol's serde representation so storage stays in lockstep
/// with the wire format. Trims the surrounding JSON quotes.
pub fn event_kind_as_str(k: &EventKind) -> String {
    let mut s = serde_json::to_string(k).expect("EventKind serialize is infallible");
    if s.starts_with('"') && s.ends_with('"') && s.len() >= 2 {
        s.truncate(s.len() - 1);
        s.remove(0);
    }
    s
}

/// Inverse of [`event_kind_as_str`]: parses an `events.kind` TEXT column value
/// back into an [`EventKind`]. Returns a parse-error message string on
/// unknown values; callers map to their preferred error type.
pub fn event_kind_from_db_str(s: &str) -> Result<EventKind, String> {
    // Round-trip through serde so storage stays in lockstep with the wire
    // format. JSON quote the input first since serde expects `"PreToolUse"`,
    // not `PreToolUse`.
    let quoted = format!("\"{s}\"");
    serde_json::from_str::<EventKind>(&quoted).map_err(|e| format!("unknown EventKind {s:?}: {e}"))
}

/// Stable wire string for a [`Reaction`] used by the daemon's SQLite storage.
///
/// Delegates to the protocol's serde representation so storage stays in lockstep
/// with the wire format. Trims the surrounding JSON quotes.
pub fn reaction_as_db_string(r: &Reaction) -> String {
    let mut s = serde_json::to_string(r).expect("Reaction serialize is infallible");
    if s.starts_with('"') && s.ends_with('"') && s.len() >= 2 {
        s.truncate(s.len() - 1);
        s.remove(0);
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_kind_db_string_round_trip_all_variants() {
        for kind in [
            EventKind::PreToolUse,
            EventKind::PostToolUse,
            EventKind::Stop,
            EventKind::Notification,
            EventKind::RecordingStarted,
            EventKind::RecordingEnded,
        ] {
            let s = event_kind_as_str(&kind);
            let parsed = event_kind_from_db_str(&s).expect("round-trip must succeed");
            assert_eq!(parsed, kind, "round-trip lost {kind:?} via {s:?}");
        }
    }

    #[test]
    fn event_kind_from_db_str_rejects_unknown() {
        assert!(event_kind_from_db_str("Bogus").is_err());
        assert!(event_kind_from_db_str("").is_err());
    }
}
