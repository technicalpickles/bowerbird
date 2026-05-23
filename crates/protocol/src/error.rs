// Error surface between source adapters and the daemon. Adapters convert
// their internal errors into this enum via `From`; the daemon matches on it
// to choose the wire response.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("serde error: {0}")]
    Serde(String),
    #[error("unknown hook_kind: {0}")]
    UnknownHookKind(String),
    // Story 2.3 fold-in from Epic 1 retro (deferred-work.md:8). Surfaced by
    // `SyncFrame::new` when a caller attempts to construct a frame with
    // `oldest_available_event_id > latest_event_id`. `Deserialize` does
    // NOT call the constructor, so wire payloads continue to round-trip
    // unchanged — the asymmetric inbound/outbound policy.
    #[error("invalid SyncFrame ordering: oldest={oldest:?} > latest={latest:?}")]
    InvalidSyncFrameOrdering {
        oldest: crate::event::EventId,
        latest: crate::event::EventId,
    },
    // Story 2.4 fold-in from deferred-work.md:9. Surfaced by
    // `DroppedFrame::new` when a caller attempts to construct a frame with
    // `count == 0` or `first_dropped_event_id > last_dropped_event_id`.
    // `Deserialize` does NOT call the constructor, so wire payloads
    // continue to round-trip unchanged — the asymmetric inbound/outbound
    // policy.
    #[error("invalid DroppedFrame: count={count} first={first:?} last={last:?}")]
    InvalidDroppedFrame {
        count: u64,
        first: crate::event::EventId,
        last: crate::event::EventId,
    },
}

pub type Result<T> = std::result::Result<T, Error>;
