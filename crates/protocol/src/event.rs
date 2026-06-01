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
    SessionEnded,
    RecordingStarted,
    RecordingEnded,
    #[serde(other)]
    Unknown,
}

/// Typed `notification_type` field carried on Claude Code `Notification` hook
/// payloads. The wire form is snake_case (`"permission_prompt"`); the Rust
/// identifier is PascalCase via `#[serde(rename = ...)]`. `Unknown` is the
/// `#[serde(other)]` catch-all so future Claude additions decode without
/// failing.
///
/// This enum is internal to `EventEnvelope` (the daemon's pre-storage type) and
/// drives the `transition()` decision for `Notification` events. It is NOT
/// stored on the wire `Event` or surfaced on `SessionState` / `StateFrame` —
/// per ADR 0004 §3, the typed value stays in `events.payload` (verbatim) for
/// archaeology and projection-side use only.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NotificationType {
    #[serde(rename = "permission_prompt")]
    PermissionPrompt,
    #[serde(rename = "idle_prompt")]
    IdlePrompt,
    #[serde(rename = "auth_success")]
    AuthSuccess,
    #[serde(rename = "elicitation_dialog")]
    ElicitationDialog,
    #[serde(rename = "elicitation_response")]
    ElicitationResponse,
    #[serde(rename = "elicitation_complete")]
    ElicitationComplete,
    #[serde(other)]
    Unknown,
}

/// Pre-storage; daemon sets event_id at INSERT. Never pass to wire.
///
/// `pid` and `notification_type` are internal — they drive projection writes
/// (carry-forward of `last_pid`; typed transition for `Notification`) but are
/// not serialized to the wire from this type. `pid` IS exposed on the stored
/// `Event` (which IS on the wire); `notification_type` stays in `payload`.
#[derive(Debug, Clone)]
pub struct EventEnvelope {
    pub source: String,
    pub session_id: String,
    pub kind: EventKind,
    pub reaction: Option<Reaction>,
    pub payload: String,
    pub pid: Option<u32>,
    pub notification_type: Option<NotificationType>,
    /// Session working directory from the source's hook payload (Story 5.7).
    /// Native Claude Code field (top-level `cwd` on every hook kind), extracted
    /// by the adapter — not shim-injected like `pid`. Threaded into the
    /// projection where it follows `last_pid`-style carry-forward.
    pub cwd: Option<String>,
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
    pub pid: Option<u32>,
    /// Session working directory for this event, verbatim (Story 5.7). On the
    /// wire (`GET /sessions/{id}/events`, WS `EventFrame.event`). `None` when
    /// the source omitted it. `started_at` is NOT here — it is a session-level
    /// projection fact on `SessionState`, not a per-event field.
    pub cwd: Option<String>,
}
