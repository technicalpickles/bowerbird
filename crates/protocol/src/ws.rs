use serde::{Deserialize, Serialize};

use crate::event::{Event, EventId};

/// Outbound (daemon → tool). Permissive: no deny_unknown_fields.
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum ServerMessage {
    Hello(HelloFrame),
    Sync(SyncFrame),
    Event(EventFrame),
    Dropped(DroppedFrame),
    Close(CloseFrame),
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
