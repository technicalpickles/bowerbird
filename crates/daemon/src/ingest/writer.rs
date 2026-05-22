use std::sync::Arc;

use deadpool_sqlite::Pool;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use protocol::EventEnvelope;

use crate::broadcast::BroadcastHub;
use crate::projection;

pub async fn run(
    mut rx: mpsc::Receiver<EventEnvelope>,
    writer_pool: Pool,
    broadcaster: Arc<BroadcastHub>,
    shutdown: CancellationToken,
) {
    loop {
        tokio::select! {
            item = rx.recv() => {
                match item {
                    Some(envelope) => {
                        if let Err(e) = projection::session::write(&writer_pool, &broadcaster, envelope).await {
                            tracing::error!(error = ?e, "projection write failed; event dropped");
                        }
                    }
                    None => break,
                }
            }
            _ = shutdown.cancelled() => {
                // Events accepted before shutdown still commit and publish
                // during drain so the SQLite + broadcast paths agree about
                // what was accepted. Whether attached WS clients actually
                // observe these drained envelopes is a separate concern:
                // production WS tasks share this same shutdown token and
                // may already have exited their dispatch loop before the
                // drain finishes. End-to-end WS flush semantics are
                // deferred to story 2.5's graceful-shutdown design.
                while let Ok(envelope) = rx.try_recv() {
                    if let Err(e) = projection::session::write(&writer_pool, &broadcaster, envelope).await {
                        tracing::error!(error = ?e, "projection write failed during drain; event dropped");
                    }
                }
                break;
            }
        }
    }
}
