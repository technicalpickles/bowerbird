//! Error surface between source adapters and the daemon.
//!
//! Adapters convert their internal errors into [`Error`] via `From`; the
//! daemon matches on it to choose the wire response.
//!
//! **`Error` is never directly wire-serialized.** This enum exists only as
//! an in-process error type — its `#[derive(thiserror::Error)]` impl produces
//! a `Display` string that the daemon then formats into one of the wire
//! shapes the protocol actually defines:
//!
//! - HTTP error responses: `{"error": "<message>"}` JSON body (the wrapper
//!   struct, not `Error` itself; see `daemon::api::error::ApiError`).
//! - Ingest socket replies: `400 <reason>\n` plain text per ADR-0002 (the
//!   shim parses the leading status code; the reason string is operator-facing).
//! - WebSocket close frames: WS code 1008 + sanitized reason ≤123 bytes per
//!   Story 2.1.
//!
//! Because `Error` never appears as a JSON-encoded enum on the wire, the
//! asymmetric `deny_unknown_fields` / `#[serde(other)]` sweep that covers the
//! other protocol enums (Story 4.4, Epic 2 retro AI-4) does NOT add a catch-
//! all variant here — there is no wire surface to keep additive. Adding
//! variants is an in-process API change governed by SemVer on this crate, not
//! a v1.x protocol break.

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
