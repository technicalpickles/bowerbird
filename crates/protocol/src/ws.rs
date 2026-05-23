use serde::{Deserialize, Serialize};

use crate::event::{Event, EventId};
use crate::state::SessionState;

/// Outbound (daemon → tool). Permissive: no deny_unknown_fields, plus an
/// `Unknown` catch-all so older clients (or third-party bindings) that
/// don't know about future variants gracefully deserialize them instead
/// of failing on the tag. Serde's internally-tagged enums fail on unknown
/// tags by default — the asymmetric `deny_unknown_fields` policy only
/// covers struct fields, not enum variants. Without this fallback, the
/// "additive within v1.x" claim in `docs/protocol-changelog.md` would be
/// false the moment a new variant ships. The daemon never constructs
/// `Unknown` (it only ever produces concrete variants on the wire), so
/// the catch-all is decode-only in practice.
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum ServerMessage {
    Hello(HelloFrame),
    Sync(SyncFrame),
    Event(EventFrame),
    State(StateFrame),
    Dropped(DroppedFrame),
    Close(CloseFrame),
    #[serde(other)]
    Unknown,
}

/// Inbound (tool → daemon). STRICT: deny_unknown_fields.
#[derive(Debug, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum ClientMessage {
    Subscribe { topic: String },
    Unsubscribe { topic: String },
}

#[derive(Debug, Serialize, Deserialize)]
pub struct HelloFrame {
    pub protocol_version: String,
    pub daemon_version: String,
    pub oldest_available_event_id: EventId,
    pub daemon_started_at: i64,
    pub history_begins_cleanly: bool,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SyncFrame {
    pub oldest_available_event_id: EventId,
    pub latest_event_id: EventId,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct EventFrame {
    pub event: Event,
}

/// Outbound state-change frame. Carries a session's projection so presenters
/// don't have to re-query the REST surface on every state transition.
///
/// `(source, session_id)` is the natural key; both are present so a future
/// multi-adapter world (Codex + Claude) can disambiguate. Permissive on
/// deserialize per the asymmetric `deny_unknown_fields` policy.
#[derive(Debug, Serialize, Deserialize)]
pub struct StateFrame {
    pub source: String,
    pub session_id: String,
    pub state: SessionState,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct DroppedFrame {
    pub count: u64,
    pub first_dropped_event_id: EventId,
    pub last_dropped_event_id: EventId,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CloseFrame {
    pub reason: Option<String>,
}
