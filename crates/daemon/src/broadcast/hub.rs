//! `BroadcastHub`: the daemon-wide pub/sub for `BroadcastEnvelope`.
//!
//! Story 2.1 introduced the hub and exposed `subscribe` / `publish`.
//! Story 2.2 wires production publishing through
//! `projection::session::write` — every committed event publishes an
//! `Event` envelope followed by a `State` envelope. Tests in
//! `crates/daemon/tests/contract_daemon.rs::story_2_2_publish` may
//! still call `publish` directly as a synthetic publisher for
//! topic-filter coverage; production publishes only come from
//! `projection::session::write`.
//!
//! Per Story 2.4's design, lag is surfaced via `RecvError::Lagged(n)`;
//! per-connection tasks translate that into a `DroppedFrame` (Story 2.4
//! wires the projection).

use tokio::sync::broadcast;

use crate::broadcast::BroadcastEnvelope;

/// Owns the broadcast channel that fan-outs `BroadcastEnvelope` to every
/// connected WebSocket client.
pub struct BroadcastHub {
    tx: broadcast::Sender<BroadcastEnvelope>,
}

impl BroadcastHub {
    /// Minimum broadcast-channel capacity. Story 2.2's
    /// `projection::session::write` publishes `BroadcastEnvelope::Event`
    /// immediately followed by `BroadcastEnvelope::State` for every
    /// committed event. A capacity of `1` would let the `State` evict the
    /// preceding `Event` before any subscriber reads it; a subscriber
    /// reading both topics could then observe the projection update
    /// without the triggering event. `0` is rejected by
    /// `tokio::sync::broadcast::channel` outright. Floor both at `2` so a
    /// misconfigured `ws_broadcast_capacity` cannot break the Event+State
    /// pair contract.
    const MIN_CAPACITY: usize = 2;

    /// Construct a hub whose channel capacity is at least
    /// [`Self::MIN_CAPACITY`] AND rounded up to an even number.
    ///
    /// The even-rounding is defense-in-depth for small odd raw
    /// capacities (`3`, `5`, …). Each committed write publishes the
    /// Event-then-State pair, so the ring naturally holds whole pairs
    /// when capacity is even; rounding `3 → 4` makes "two pairs fit
    /// without `Lagged`" hold true. Even rounding does NOT solve all
    /// lag-recovery scenarios — once published count exceeds the
    /// rounded capacity, a lagged receiver can still recover on a
    /// `State` boundary depending on parity. The full pair-aware
    /// recovery story lives in Story 2.4's Lagged → `DroppedFrame`
    /// projection.
    pub fn new(capacity: usize) -> Self {
        let capacity = capacity.max(Self::MIN_CAPACITY);
        // `% 2` yields 1 for odd capacities and 0 for even ones, so this
        // rounds odd capacities up by one and leaves even ones alone.
        let capacity = capacity + (capacity % 2);
        let (tx, _rx) = broadcast::channel(capacity);
        Self { tx }
    }

    /// Subscribe — every new WS connection calls this once.
    pub fn subscribe(&self) -> broadcast::Receiver<BroadcastEnvelope> {
        self.tx.subscribe()
    }

    /// Publish — called by `projection::session::write` for every
    /// committed event (Story 2.2). Crate-private on purpose: the only
    /// production caller is `projection::session::write`, which keeps
    /// publishing commit-gated and filtered through the sentinel-kind
    /// and reserved-source guards (Story 2.2 ACs #6 and #7). External
    /// crates use [`Self::publish_for_tests`] for synthetic probes.
    pub(crate) fn publish(&self, envelope: BroadcastEnvelope) {
        // SendError is fine to swallow: it only happens when there are zero
        // subscribers, which is the normal idle daemon state.
        let _ = self.tx.send(envelope);
    }

    /// Test-only synthetic publisher. Mirrors [`Self::publish`] but is
    /// reachable from integration tests (which see this crate as
    /// external). The name is intentionally distinct so a future
    /// production callsite outside `projection::session::write` cannot
    /// accidentally bypass commit-gating by reaching for "publish."
    ///
    /// Production code MUST NOT call this. The only legitimate uses are
    /// readiness probes (`wait_subscribe_live`) and topic-filter
    /// coverage that needs to inject envelopes the production publish
    /// path cannot produce (e.g. a synthetic non-`claude` event).
    #[doc(hidden)]
    pub fn publish_for_tests(&self, envelope: BroadcastEnvelope) {
        self.publish(envelope);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use protocol::{Event, EventId, EventKind, Reaction, SessionCurrentState, SessionState};

    fn dummy_event(id: i64) -> BroadcastEnvelope {
        BroadcastEnvelope::Event(Event {
            event_id: EventId(id),
            source: "claude".to_string(),
            session_id: "sess-1".to_string(),
            kind: EventKind::PreToolUse,
            reaction: Some(Reaction::Continue),
            payload: "{}".to_string(),
            created_at: 0,
        })
    }

    fn dummy_state() -> BroadcastEnvelope {
        BroadcastEnvelope::State {
            source: "claude".to_string(),
            session_id: "sess-1".to_string(),
            state: SessionState {
                current_state: SessionCurrentState::Working,
                last_event_kind: EventKind::PreToolUse,
                last_event_at_ms: 0,
            },
        }
    }

    /// Misconfigured `0` or `1` capacities must not destroy the Event+State
    /// pair contract from story 2.2. A single subscriber that has not yet
    /// read must be able to drain both envelopes without `Lagged`.
    #[tokio::test(flavor = "current_thread")]
    async fn capacity_floor_holds_event_state_pair() {
        for raw_capacity in [0_usize, 1, 2] {
            let hub = BroadcastHub::new(raw_capacity);
            let mut rx = hub.subscribe();
            hub.publish(dummy_event(1));
            hub.publish(dummy_state());

            let first = rx.recv().await.expect("first recv");
            assert!(
                matches!(first, BroadcastEnvelope::Event(_)),
                "raw_capacity={raw_capacity}: expected Event, got {first:?}"
            );
            let second = rx.recv().await.expect("second recv");
            assert!(
                matches!(second, BroadcastEnvelope::State { .. }),
                "raw_capacity={raw_capacity}: expected State, got {second:?}"
            );
        }
    }

    /// Odd raw capacities (`3`, `5`, …) must be rounded up to even by
    /// `BroadcastHub::new` so two whole Event+State pairs fit without
    /// `Lagged`. Without the rounding, raw capacity `3` would evict
    /// `Event1` after the second pair publishes, leaving a lagged
    /// subscriber to recover on `State1` (an orphaned state frame).
    #[tokio::test(flavor = "current_thread")]
    async fn odd_capacity_rounds_up_to_hold_two_pairs() {
        let hub = BroadcastHub::new(3); // rounded up to 4 internally
        let mut rx = hub.subscribe();

        // Publish two pairs without reading. With capacity = 4, the
        // ring holds all four envelopes and no `Lagged` fires.
        hub.publish(dummy_event(1));
        hub.publish(dummy_state());
        hub.publish(dummy_event(2));
        hub.publish(dummy_state());

        // The first deliverable envelope must be `Event1` — broadcast
        // ordering plus pair-fitting capacity guarantees pair-aligned
        // recovery for this scenario. A raw capacity of `3` (no
        // rounding) would deliver `State1` here instead.
        let first = rx.recv().await.expect("first recv");
        assert!(
            matches!(first, BroadcastEnvelope::Event(ref e) if e.event_id.0 == 1),
            "odd-capacity rounding broken: expected Event(1) first, got {first:?}"
        );
        let second = rx.recv().await.expect("second recv");
        assert!(
            matches!(second, BroadcastEnvelope::State { .. }),
            "expected State after Event(1), got {second:?}"
        );
        let third = rx.recv().await.expect("third recv");
        assert!(
            matches!(third, BroadcastEnvelope::Event(ref e) if e.event_id.0 == 2),
            "expected Event(2) after State1, got {third:?}"
        );
        let fourth = rx.recv().await.expect("fourth recv");
        assert!(
            matches!(fourth, BroadcastEnvelope::State { .. }),
            "expected State after Event(2), got {fourth:?}"
        );
    }
}
