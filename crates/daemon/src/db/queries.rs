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
    "UPDATE recording_sessions SET ended_event_id = ? \
     WHERE id = (SELECT MAX(id) FROM recording_sessions)";

pub const SELECT_EVENT_BY_ID: &str =
    "SELECT event_id, source, session_id, kind, reaction, payload, created_at \
     FROM events WHERE event_id = ?";

/// Stable wire string for an [`EventKind`] used by the daemon's SQLite storage.
///
/// Matches the protocol's `Debug` / serde representation (PascalCase, no rename).
pub fn event_kind_as_str(k: &EventKind) -> &'static str {
    match k {
        EventKind::PreToolUse => "PreToolUse",
        EventKind::PostToolUse => "PostToolUse",
        EventKind::Stop => "Stop",
        EventKind::Notification => "Notification",
        EventKind::RecordingStarted => "RecordingStarted",
        EventKind::RecordingEnded => "RecordingEnded",
    }
}

/// Stable wire string for a [`Reaction`] used by the daemon's SQLite storage.
pub fn reaction_as_db_string(r: &Reaction) -> String {
    match r {
        Reaction::Pause => "Pause".to_string(),
        Reaction::Continue => "Continue".to_string(),
        Reaction::Vendor(n) => format!("Vendor({n})"),
        Reaction::Unknown => "Unknown".to_string(),
    }
}
