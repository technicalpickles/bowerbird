use serde::{Deserialize, Serialize};

use crate::event::{Event, EventId};

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
