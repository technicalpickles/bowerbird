use serde::{Deserialize, Serialize};

use crate::event::EventKind;

/// Per-session current-state projection.
///
/// On the wire as `StateFrame.state.current_state`. PascalCase variant names
/// ARE the wire format.
///
/// **`Unknown` is a decode-only catch-all** (Story 4.4, Epic 2 retro AI-4
/// fold-in). Serde's `#[serde(other)]` maps any unrecognized JSON string into
/// this variant so v1.0 presenters continue to decode `StateFrame` payloads
/// whose `current_state` is a future v1.x addition (e.g. `Compacting`,
/// `AwaitingApproval`) instead of failing the parse. The daemon never
/// constructs `Unknown` — `projection::state::transition` maps known event
/// kinds to known states and ignores the sentinels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SessionCurrentState {
    Idle,
    Working,
    WaitingInput,
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionState {
    pub current_state: SessionCurrentState,
    pub last_event_kind: EventKind,
    pub last_event_at_ms: i64,
}
