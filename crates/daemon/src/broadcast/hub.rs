//! `BroadcastHub`: the daemon-wide pub/sub for `BroadcastEnvelope`.
//!
//! Per Story 2.4's design, lag is surfaced via `RecvError::Lagged(n)`;
//! per-connection tasks translate that into a `DroppedFrame` (Story 2.4
//! wires it). Story 2.1 only constructs the hub and exposes
//! `subscribe`/`publish`; no daemon code publishes yet.

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

    pub fn new(capacity: usize) -> Self {
        let capacity = capacity.max(Self::MIN_CAPACITY);
        let (tx, _rx) = broadcast::channel(capacity);
        Self { tx }
    }

    /// Subscribe — every new WS connection calls this once.
    pub fn subscribe(&self) -> broadcast::Receiver<BroadcastEnvelope> {
        self.tx.subscribe()
    }

    /// Publish — Story 2.2 wires this into `projection::session::write`.
    /// Story 2.1 does not publish from daemon code; the path exists so tests
    /// for AC #2 can use it as a synthetic publisher.
    pub fn publish(&self, envelope: BroadcastEnvelope) {
        // SendError is fine to swallow: it only happens when there are zero
        // subscribers, which is the normal idle daemon state.
        let _ = self.tx.send(envelope);
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
}
