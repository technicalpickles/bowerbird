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
    pub last_pid: Option<u32>,
    /// Session working directory, verbatim (Story 5.7). Parity with
    /// `SessionDetail.state.cwd` so a presenter listing sessions can group /
    /// label by directory without fetching each detail.
    pub cwd: Option<String>,
    /// Epoch-ms timestamp of the session's first observed event (Story 5.7).
    /// Lets a presenter render session age directly from the list response.
    pub started_at: Option<i64>,
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
/// contains no non-sentinel rows. `connected_ws_clients` is a count of active
/// WebSocket subscribers — snapshot at request time; can drift between this
/// read and a follow-up read because WS connections come and go.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DaemonStatus {
    pub daemon_version: String,
    pub protocol_version: String,
    pub started_at_ms: i64,
    pub uptime_ms: i64,
    pub last_event_at_ms: Option<i64>,
    pub last_event_id: Option<EventId>,
    pub connected_ws_clients: u32,
}

/// Content of `~/.bowerbird/server.json`.
///
/// The daemon publishes its bound HTTP address to this file once
/// `listener.local_addr()` resolves (the daemon's default bind is
/// `127.0.0.1:0`, so the kernel picks the port). The CLI reads it to find the
/// daemon's HTTP surface for `bowerbird start`'s `/healthz` probe and
/// `bowerbird status`'s `/status` query.
///
/// **Asymmetric serde note.** This file is read by an inbound consumer (the
/// CLI), but its content is daemon-controlled — effectively an outbound
/// emission from one bowerbird binary to another. The asymmetric
/// `deny_unknown_fields` rule applies per-direction, so `ServerInfo` does NOT
/// carry `deny_unknown_fields`: a future daemon adding a field must not break
/// older CLIs that round-trip the file. (Story 3.3 ultimately landed the
/// bearer token in the system keychain + `~/.bowerbird/config.toml` fallback
/// rather than extending `ServerInfo`, so the field-set is unchanged at this
/// release — but the permissive policy stands for any future additive field.)
/// See `project-context.md` "Wire format conventions" for the rule.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerInfo {
    pub bind_addr: String,
}
