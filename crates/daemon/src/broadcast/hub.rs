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
    pub fn new(capacity: usize) -> Self {
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
