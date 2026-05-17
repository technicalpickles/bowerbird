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
