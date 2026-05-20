use serde::{Deserialize, Serialize};

use crate::event::EventKind;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SessionCurrentState {
    Idle,
    Working,
    WaitingInput,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionState {
    pub current_state: SessionCurrentState,
    pub last_event_kind: EventKind,
    pub last_event_at_ms: i64,
}
