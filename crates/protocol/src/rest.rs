use serde::{Deserialize, Serialize};

use crate::event::{Event, EventId, EventKind};
use crate::state::{SessionCurrentState, SessionState};

/// History slice for a single session.
///
/// `cursor` is the next-since cursor for tailing — set to `Some(events.last().event_id)`
/// when `events` is non-empty, `None` otherwise. Presenters pass this back as
/// the `?since=` query param on the next request.
///
/// `oldest_available_event_id` is the **global** minimum `event_id` still on
/// disk across the entire event log (filtered to non-sentinel rows). It is
/// `EventId(i64::MAX)` if the events table is empty. Presenters mechanically
/// infer a gap with `since < oldest_available_event_id` (Axiom 4: substrate
/// emits the fact, presenter interprets the gap).
#[derive(Debug, Serialize, Deserialize)]
pub struct EventListResponse {
    pub events: Vec<Event>,
    pub cursor: Option<EventId>,
    pub oldest_available_event_id: EventId,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SessionStats {
    pub source: String,
    pub session_id: String,
    pub event_count: i64,
    pub first_event_at: Option<i64>,
    pub last_event_at: Option<i64>,
}

/// One entry in the `GET /sessions` response array.
///
/// `current_state` is the **read-time** projection (stale-Working → Idle per
/// Story 1.6's `current_state_for_read`), not the raw stored value.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionListItem {
    pub source: String,
    pub session_id: String,
    pub current_state: SessionCurrentState,
    pub last_event_kind: EventKind,
    pub last_event_at_ms: i64,
    pub updated_at: i64,
}

/// Body of `GET /sessions/{id}`.
///
/// `state.current_state` is the read-time view (stale-Working → Idle); the
/// other `SessionState` fields are passed through unchanged from the stored
/// row.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionDetail {
    pub source: String,
    pub session_id: String,
    pub state: SessionState,
    pub updated_at: i64,
}

/// Body of `GET /status`.
///
/// `last_event_at_ms` and `last_event_id` are `None` when the events table
/// contains no non-sentinel rows. `connected_ws_clients` is reserved for
/// Epic 2's WebSocket surface and intentionally absent from V1.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DaemonStatus {
    pub daemon_version: String,
    pub protocol_version: String,
    pub started_at_ms: i64,
    pub uptime_ms: i64,
    pub last_event_at_ms: Option<i64>,
    pub last_event_id: Option<EventId>,
}
