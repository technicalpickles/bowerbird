use protocol::{EventKind, Reaction};

pub const INSERT_EVENT: &str =
    "INSERT INTO events (source, session_id, kind, reaction, payload, created_at, pid, cwd) \
     VALUES (?, ?, ?, ?, ?, ?, ?, ?)";

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
    "SELECT event_id, source, session_id, kind, reaction, payload, created_at, pid, cwd \
     FROM events WHERE event_id = ?";

pub const SELECT_SESSION_PROJECTION_STATE: &str =
    "SELECT state FROM session_projections WHERE source = ? AND session_id = ?";

pub const SELECT_DISTINCT_SESSIONS_FROM_EVENTS: &str =
    "SELECT DISTINCT source, session_id FROM events WHERE source != '__daemon__'";

pub const SELECT_EVENT_KINDS_FOR_SESSION: &str =
    "SELECT kind, created_at, pid, cwd, payload FROM events WHERE source = ? AND session_id = ? \
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
    "SELECT event_id, source, session_id, kind, reaction, payload, created_at, pid, cwd \
     FROM events \
     WHERE source != '__daemon__' AND session_id = ? AND event_id > ? \
     ORDER BY event_id ASC";

/// Story 5.4 — existence probe for `GET /sessions/{id}/events`. Mirrors the
/// shape of [`SELECT_SESSION_BY_ID`] so the events endpoint and the
/// `/sessions/{id}` endpoint agree on what "session exists" means: there is
/// at least one non-sentinel `session_projections` row for the id. Used
/// before the actual events SELECT inside the same `conn.interact` closure
/// so both reads see the same SQLite snapshot.
pub const SELECT_SESSION_EXISTS_BY_ID: &str = "SELECT 1 FROM session_projections \
     WHERE session_id = ? AND source != '__daemon__' \
     LIMIT 1";

pub const SELECT_MIN_EVENT_ID: &str =
    "SELECT MIN(event_id) FROM events WHERE source != '__daemon__'";

/// Story 5.7 review — true first-event timestamp for one session. Used to
/// backfill `SessionState.started_at` for legacy (pre-5.7) projection rows
/// whose stored blob deserializes with `started_at: None`. Reading the earliest
/// `created_at` from the event log keeps the live projection equal to what a
/// full rebuild would produce (`started_at = MIN(created_at)`), preserving the
/// byte-identical-rebuild contract and ADR 0006's "reconstructs identically on
/// rebuild" guarantee. Fires only for legacy rows — a post-5.7 session sets
/// `started_at` on its first event and never reaches this path.
pub const SELECT_MIN_CREATED_AT_FOR_SESSION: &str =
    "SELECT MIN(created_at) FROM events WHERE source = ? AND session_id = ?";

/// Story 2.1 — Hello frame `history_begins_cleanly` probe.
///
/// Returns `1` if there exists a `recording_sessions` row whose
/// `started_event_id <= MIN(events.event_id) <= ended_event_id`, i.e. the
/// minimum-available event_id falls inside a known-clean recording window.
/// `COALESCE(..., 0)` handles the empty-events case (returns `0`, meaning
/// no clean-window claim — the conservative default for an empty daemon).
pub const SELECT_HISTORY_BEGINS_CLEANLY: &str =
    "SELECT EXISTS( \
       SELECT 1 FROM recording_sessions \
       WHERE started_event_id <= (SELECT COALESCE(MIN(event_id), 0) FROM events WHERE source != '__daemon__') \
         AND ended_event_id IS NOT NULL \
         AND ended_event_id >= (SELECT COALESCE(MIN(event_id), 0) FROM events WHERE source != '__daemon__') \
     ) AS clean";

/// Story 2.1 — both Hello-frame DB fields in one snapshot.
///
/// Returns `(min_event_id, history_begins_cleanly)` from a single SQL
/// statement so a concurrent commit between the two reads cannot make the
/// fields disagree. Equivalent to running [`SELECT_MIN_EVENT_ID`] and
/// [`SELECT_HISTORY_BEGINS_CLEANLY`] inside a read transaction, but cheaper.
pub const SELECT_HELLO_DB_FIELDS: &str = "SELECT \
       (SELECT MIN(event_id) FROM events WHERE source != '__daemon__') AS min_event_id, \
       EXISTS( \
         SELECT 1 FROM recording_sessions \
         WHERE started_event_id <= (SELECT COALESCE(MIN(event_id), 0) FROM events WHERE source != '__daemon__') \
           AND ended_event_id IS NOT NULL \
           AND ended_event_id >= (SELECT COALESCE(MIN(event_id), 0) FROM events WHERE source != '__daemon__') \
       ) AS history_begins_cleanly";

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
///
/// **`EventKind::Unknown` must never reach this function.** Unknown is the
/// decode-only catch-all the additive-compat policy requires (Story 4.4 /
/// Epic 2 retro AI-4); it appears at wire-deserialize boundaries (Event JSON
/// in `/replay`, broadcast envelope decode in third-party bindings) but the
/// daemon never CONSTRUCTS it, and `/replay` rejects it at the parse boundary.
/// A `debug_assert!` fires in debug builds if this invariant breaks; release
/// builds tolerate it by serializing through as the literal string `"Unknown"`
/// (lossy, but no panic on the hot ingest path).
pub fn event_kind_as_str(k: &EventKind) -> String {
    debug_assert!(
        !matches!(k, EventKind::Unknown),
        "EventKind::Unknown is decode-only; the daemon must never persist it (see Story 4.4)"
    );
    let mut s = serde_json::to_string(k).expect("EventKind serialize is infallible");
    if s.starts_with('"') && s.ends_with('"') && s.len() >= 2 {
        s.truncate(s.len() - 1);
        s.remove(0);
    }
    s
}

/// Inverse of [`event_kind_as_str`]: parses an `events.kind` TEXT column value
/// back into an [`EventKind`]. Returns a parse-error message string on
/// malformed input.
///
/// **Permissive on unknown variants** since Story 4.4 — the wire-decode
/// catch-all `#[serde(other)] Unknown` (Epic 2 retro AI-4) means any string
/// that isn't a recognized variant deserializes to `EventKind::Unknown`
/// rather than erroring. Storage staying in lockstep with the wire is
/// intentional: a DB row written by a future v1.x daemon with a kind this
/// build doesn't know about reads back as `Unknown`, the same way the wire
/// would deserialize it.
///
/// Empty strings are still rejected explicitly — they aren't future
/// variants, they're corrupt storage rows. Surface the corruption rather
/// than silently mapping to `Unknown`.
pub fn event_kind_from_db_str(s: &str) -> Result<EventKind, String> {
    if s.is_empty() {
        return Err("empty EventKind string: corrupt storage row".to_string());
    }
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
            EventKind::UserPromptSubmit,
            EventKind::PreToolUse,
            EventKind::PostToolUse,
            EventKind::Stop,
            EventKind::Notification,
            EventKind::SessionEnded,
            EventKind::RecordingStarted,
            EventKind::RecordingEnded,
        ] {
            let s = event_kind_as_str(&kind);
            let parsed = event_kind_from_db_str(&s).expect("round-trip must succeed");
            assert_eq!(parsed, kind, "round-trip lost {kind:?} via {s:?}");
        }
    }

    #[test]
    fn event_kind_from_db_str_rejects_garbage_but_accepts_unknown_literal() {
        // Empty/whitespace strings still error — they're malformed storage rows.
        assert!(event_kind_from_db_str("").is_err());

        // Story 4.4 / Epic 2 retro AI-4: the wire-decode catch-all
        // `EventKind::Unknown` would deserialize from the literal string
        // `"Unknown"`. The DB round-trip therefore accepts it — but
        // `event_kind_as_str` debug-asserts against constructing it on the
        // write side, so this code path is unreachable in normal daemon
        // operation. The test pins the read-side tolerance for completeness.
        let parsed = event_kind_from_db_str("Unknown").expect("Unknown literal round-trips");
        assert_eq!(parsed, EventKind::Unknown);

        // A garbage string that isn't a known variant decodes to
        // `EventKind::Unknown` because the wire derive carries
        // `#[serde(other)]`. Storage staying in lockstep with the wire is
        // intentional — operators reading a DB row with a future-only kind
        // see the same lossy `Unknown` token the wire would deserialize to.
        let parsed = event_kind_from_db_str("FutureVariant").unwrap();
        assert_eq!(parsed, EventKind::Unknown);
    }

    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "EventKind::Unknown is decode-only")]
    fn event_kind_as_str_panics_on_unknown_in_debug_builds() {
        // Story 4.4: the daemon must never persist `EventKind::Unknown`.
        // Adapters reject unknown hook strings at the normalize boundary and
        // /replay rejects Unknown at the parse boundary, so this path is
        // unreachable in practice — but the debug-assert is the canary that
        // catches a future regression where a code path slips Unknown
        // through. Release builds tolerate it (no panic) so the hot ingest
        // path is unaffected.
        let _ = event_kind_as_str(&EventKind::Unknown);
    }
}
