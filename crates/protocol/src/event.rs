use serde::{Deserialize, Serialize};

use crate::reaction::Reaction;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct EventId(pub i64);

/// Hook event kinds emitted by source adapters.
///
/// On the wire as `Event.kind`. The PascalCase variant names ARE the wire
/// format (no `rename_all`); changing a variant identifier is a protocol break.
///
/// **`Unknown` is a decode-only catch-all** (Story 4.4, Epic 2 retro AI-4
/// fold-in). Serde's `#[serde(other)]` maps any unrecognized JSON string into
/// this variant so v1.0 presenters continue to decode events whose `kind` is a
/// future v1.x addition (e.g. `SubAgentSpawn`) instead of failing the parse and
/// dropping the whole event. The daemon never constructs `Unknown` — adapters
/// map known hook strings to known variants and reject the rest at the
/// normalize boundary; the storage layer round-trips through serde so an
/// `Unknown` would serialize as the literal string `"Unknown"` if it ever
/// reached `event_kind_as_str`, but that path is unreachable in practice.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum EventKind {
    UserPromptSubmit,
    PreToolUse,
    PostToolUse,
    Stop,
    Notification,
    RecordingStarted,
    RecordingEnded,
    #[serde(other)]
    Unknown,
}

/// Pre-storage; daemon sets event_id at INSERT. Never pass to wire.
#[derive(Debug, Clone)]
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
