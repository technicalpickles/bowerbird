use serde::{Deserialize, Serialize};

use crate::reaction::Reaction;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct EventId(pub i64);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum EventKind {
    PreToolUse,
    PostToolUse,
    Stop,
    Notification,
    RecordingStarted,
    RecordingEnded,
}

/// Pre-storage; daemon sets event_id at INSERT. Never pass to wire.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventEnvelope {
    pub source: String,
    pub session_id: String,
    pub kind: EventKind,
    pub reaction: Option<Reaction>,
    pub payload: String,
}

/// Stored event — includes assigned event_id and created_at timestamp.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Event {
    pub event_id: EventId,
    pub source: String,
    pub session_id: String,
    pub kind: EventKind,
    pub reaction: Option<Reaction>,
    pub payload: String,
    pub created_at: i64,
}
