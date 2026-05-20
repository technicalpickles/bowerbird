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

// Story 1.7 — REST query API SQL constants.
//
// The `__daemon__` literal is the sentinel source used by lifecycle markers
// (`RecordingStarted` / `RecordingEnded`). Kept in lockstep with
// `projection::session::DAEMON_SENTINEL_SOURCE`; a future rename of that
// constant must update these strings too.

pub const SELECT_NON_SENTINEL_SESSIONS: &str =
    "SELECT source, session_id, state, updated_at FROM session_projections \
     WHERE source != '__daemon__' \
     ORDER BY updated_at DESC, source ASC, session_id ASC";

// V1 only has the `"claude"` source, so the `ORDER BY ... LIMIT 1` ordering
// never matters in practice. When a second adapter ships (Codex, OpenCode),
// callers should disambiguate with an explicit `?source=` query param or by
// nesting `/sources/{source}/sessions/{id}`. See deferred-work.md.
pub const SELECT_SESSION_BY_ID: &str =
    "SELECT source, session_id, state, updated_at FROM session_projections \
     WHERE session_id = ? AND source != '__daemon__' \
     ORDER BY updated_at DESC LIMIT 1";

pub const SELECT_EVENTS_FOR_SESSION_SINCE: &str =
    "SELECT event_id, source, session_id, kind, reaction, payload, created_at \
     FROM events \
     WHERE source != '__daemon__' AND session_id = ? AND event_id > ? \
     ORDER BY event_id ASC";

pub const SELECT_MIN_EVENT_ID: &str =
    "SELECT MIN(event_id) FROM events WHERE source != '__daemon__'";

pub const SELECT_STATS_FOR_SESSION: &str =
    "SELECT source, COUNT(*) as event_count, MIN(created_at) as first_event_at, \
            MAX(created_at) as last_event_at \
     FROM events \
     WHERE source != '__daemon__' AND session_id = ? \
     GROUP BY source \
     ORDER BY MAX(created_at) DESC LIMIT 1";

pub const SELECT_LAST_EVENT: &str = "SELECT event_id, created_at FROM events \
     WHERE source != '__daemon__' \
     ORDER BY event_id DESC LIMIT 1";

/// `/readyz` DB-liveness probe.
///
/// `WHERE 1=0` makes the planner short-circuit before scanning any rows, so
/// latency is sub-millisecond on any DB size. The query validates three
/// things at once: pool checkout succeeds, the connection is alive, and the
/// `events` table exists (a corrupt-schema state would otherwise pass a bare
/// `SELECT 1`).
pub const PROBE_DB_READY: &str = "SELECT 1 FROM events WHERE 1=0";

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

/// Inverse of [`reaction_as_db_string`]: parses an `events.reaction` TEXT
/// column value back into a [`Reaction`]. Returns a parse-error message
/// string on unknown values; callers map to their preferred error type.
pub fn reaction_from_db_string(s: &str) -> Result<Reaction, String> {
    let quoted = format!("\"{s}\"");
    serde_json::from_str::<Reaction>(&quoted).map_err(|e| format!("unknown Reaction {s:?}: {e}"))
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
