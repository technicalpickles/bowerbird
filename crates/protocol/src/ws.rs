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
///
/// Story 4.4 (Epic 2 retro AI-4) confirmed this enum already had the catch-
/// all from Story 2.1 — no code change. The sweep added the same pattern to
/// `EventKind` (`event.rs`), `SessionCurrentState` (`state.rs`), and fixed
/// `Reaction::deserialize` (`reaction.rs`) to map unknown strings to
/// `Reaction::Unknown` instead of erroring.
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

/// Inbound (tool → daemon). STRICT: `deny_unknown_fields` plus no `Unknown`
/// catch-all variant.
///
/// The asymmetric serde policy (project-context.md §Wire format conventions,
/// architecture.md:606-608) says **outbound** types stay permissive so a
/// newer daemon can ship additive variants without breaking older presenters
/// — that is what `ServerMessage::Unknown` (above) implements. `ClientMessage`
/// is the inbound direction and gets the opposite treatment: a presenter
/// sending an `op` the daemon doesn't recognize is a protocol violation, NOT
/// a forward-compat surface. The daemon rejects it via WS close code 1008
/// (Policy Violation) with a `bad message: unknown op` reason per Story 2.1.
///
/// Story 4.4 (Epic 2 retro AI-4) audited every wire-surface enum for this
/// rule; `ClientMessage` is the documented exception. If a future story
/// genuinely needs additive inbound ops, the daemon's `bad message: ...`
/// rejection path is the right place to widen — not this enum.
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

/// Sync frame carrying the available event-id window.
///
/// `#[non_exhaustive]` blocks struct-literal construction from outside
/// the `protocol` crate, so the daemon (and any future producer) must
/// go through [`SyncFrame::new`] which enforces `oldest <= latest`.
/// `Deserialize` is generated inside this crate and is therefore
/// exempt from the attribute — wire payloads, including ones with
/// inverted IDs from a hypothetical buggy peer, continue to parse
/// without validation (asymmetric inbound/outbound policy).
#[derive(Debug, Serialize, Deserialize)]
#[non_exhaustive]
pub struct SyncFrame {
    pub oldest_available_event_id: EventId,
    pub latest_event_id: EventId,
}

impl SyncFrame {
    /// Construct a `SyncFrame` with the cursor-ordering invariant enforced.
    /// Returns `Err(Error::InvalidSyncFrameOrdering)` when `oldest > latest`.
    /// Equality is permitted (empty event log: both at 0).
    ///
    /// `Deserialize` deliberately does NOT route through this constructor —
    /// wire payloads (including ones from a hypothetical buggy peer with
    /// inverted IDs) round-trip without validation per the asymmetric
    /// inbound/outbound policy. The constructor is the daemon-side
    /// construction-time gate; no Story 2.3 producer activates it yet
    /// (Story 2.4's lagged-consumer recovery will).
    pub fn new(
        oldest_available_event_id: EventId,
        latest_event_id: EventId,
    ) -> crate::error::Result<Self> {
        if oldest_available_event_id > latest_event_id {
            return Err(crate::error::Error::InvalidSyncFrameOrdering {
                oldest: oldest_available_event_id,
                latest: latest_event_id,
            });
        }
        Ok(Self {
            oldest_available_event_id,
            latest_event_id,
        })
    }
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

/// Dropped frame carrying the lag burst's envelope count and best-estimate
/// event-id range.
///
/// `#[non_exhaustive]` blocks struct-literal construction from outside the
/// `protocol` crate, so the daemon (and any future producer) must go through
/// [`DroppedFrame::new`] which enforces `count > 0` and
/// `first_dropped_event_id <= last_dropped_event_id`. `Deserialize` is
/// generated inside this crate and is therefore exempt from the attribute —
/// wire payloads, including ones from a hypothetical buggy peer with
/// inverted IDs or a zero count, continue to parse without validation
/// (asymmetric inbound/outbound policy; same pattern as
/// [`SyncFrame::new`]).
#[derive(Debug, Serialize, Deserialize)]
#[non_exhaustive]
pub struct DroppedFrame {
    pub count: u64,
    pub first_dropped_event_id: EventId,
    pub last_dropped_event_id: EventId,
}

impl DroppedFrame {
    /// Construct a `DroppedFrame` with field invariants enforced:
    /// - `count > 0` (vacuous Dropped is a bug; the daemon's coalescing
    ///   suppresses zero-count emissions before reaching this constructor).
    /// - `first_dropped_event_id <= last_dropped_event_id`.
    ///
    /// Returns `Err(Error::InvalidDroppedFrame { ... })` on violation.
    ///
    /// `Deserialize` does NOT call this — wire payloads (including ones
    /// from a hypothetical buggy peer) still parse without validation per
    /// the asymmetric inbound/outbound policy. The constructor is the
    /// daemon-side construction-time gate. Story 2.3's `SyncFrame::new`
    /// is the sibling pattern.
    pub fn new(
        count: u64,
        first_dropped_event_id: EventId,
        last_dropped_event_id: EventId,
    ) -> crate::error::Result<Self> {
        if count == 0 {
            return Err(crate::error::Error::InvalidDroppedFrame {
                count,
                first: first_dropped_event_id,
                last: last_dropped_event_id,
            });
        }
        if first_dropped_event_id > last_dropped_event_id {
            return Err(crate::error::Error::InvalidDroppedFrame {
                count,
                first: first_dropped_event_id,
                last: last_dropped_event_id,
            });
        }
        Ok(Self {
            count,
            first_dropped_event_id,
            last_dropped_event_id,
        })
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CloseFrame {
    pub reason: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::Error;

    #[test]
    fn sync_frame_new_accepts_ordered_cursors() {
        let f = SyncFrame::new(EventId(10), EventId(20)).expect("ordered must succeed");
        assert_eq!(f.oldest_available_event_id, EventId(10));
        assert_eq!(f.latest_event_id, EventId(20));
    }

    #[test]
    fn sync_frame_new_rejects_inverted_cursors() {
        let err = SyncFrame::new(EventId(20), EventId(10)).expect_err("inverted must fail");
        assert!(matches!(
            err,
            Error::InvalidSyncFrameOrdering {
                oldest: EventId(20),
                latest: EventId(10),
            }
        ));
    }

    #[test]
    fn sync_frame_new_allows_equal_cursors() {
        // Empty event log: oldest == latest == 0 is a legitimate state.
        let f = SyncFrame::new(EventId(5), EventId(5)).expect("equal must succeed");
        assert_eq!(f.oldest_available_event_id, EventId(5));
        assert_eq!(f.latest_event_id, EventId(5));
    }

    #[test]
    fn sync_frame_deserialize_tolerates_inverted_cursors_from_wire() {
        // Asymmetric inbound/outbound policy: Deserialize does not call the
        // constructor, so a hypothetical buggy peer's inverted SyncFrame
        // still parses cleanly. The construction-side gate is the only
        // place ordering is enforced.
        let wire = r#"{"oldest_available_event_id":20,"latest_event_id":10}"#;
        let f: SyncFrame =
            serde_json::from_str(wire).expect("wire payload must deserialize unchanged");
        assert_eq!(f.oldest_available_event_id, EventId(20));
        assert_eq!(f.latest_event_id, EventId(10));
    }

    #[test]
    fn dropped_frame_new_accepts_valid() {
        let f = DroppedFrame::new(5, EventId(10), EventId(14)).expect("valid must succeed");
        assert_eq!(f.count, 5);
        assert_eq!(f.first_dropped_event_id, EventId(10));
        assert_eq!(f.last_dropped_event_id, EventId(14));
    }

    #[test]
    fn dropped_frame_new_rejects_zero_count() {
        let err = DroppedFrame::new(0, EventId(10), EventId(14)).expect_err("zero must fail");
        assert!(matches!(
            err,
            Error::InvalidDroppedFrame {
                count: 0,
                first: EventId(10),
                last: EventId(14),
            }
        ));
    }

    #[test]
    fn dropped_frame_new_rejects_inverted_ids() {
        let err =
            DroppedFrame::new(3, EventId(20), EventId(10)).expect_err("inverted ids must fail");
        assert!(matches!(
            err,
            Error::InvalidDroppedFrame {
                count: 3,
                first: EventId(20),
                last: EventId(10),
            }
        ));
    }

    #[test]
    fn dropped_frame_new_allows_count_one_first_eq_last() {
        // Singleton drop: count = 1 with first == last is the natural shape
        // of "one envelope dropped at exactly this position."
        let f = DroppedFrame::new(1, EventId(42), EventId(42)).expect("singleton must succeed");
        assert_eq!(f.count, 1);
        assert_eq!(f.first_dropped_event_id, EventId(42));
        assert_eq!(f.last_dropped_event_id, EventId(42));
    }

    #[test]
    fn dropped_frame_deserialize_tolerates_invalid_from_wire() {
        // Asymmetric inbound/outbound policy: Deserialize does not call the
        // constructor. A hypothetical buggy peer's invalid DroppedFrame
        // (zero count, inverted ids) still parses cleanly. The construction
        // gate is the only place invariants are enforced.
        let zero_count = r#"{"count":0,"first_dropped_event_id":1,"last_dropped_event_id":1}"#;
        let f: DroppedFrame =
            serde_json::from_str(zero_count).expect("zero-count payload must deserialize");
        assert_eq!(f.count, 0);

        let inverted = r#"{"count":3,"first_dropped_event_id":20,"last_dropped_event_id":10}"#;
        let f: DroppedFrame =
            serde_json::from_str(inverted).expect("inverted payload must deserialize");
        assert_eq!(f.first_dropped_event_id, EventId(20));
        assert_eq!(f.last_dropped_event_id, EventId(10));
    }
}
