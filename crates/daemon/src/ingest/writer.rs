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
                // Events accepted before shutdown drain after `cancelled()`
                // MUST still publish so any WS clients still attached during
                // graceful shutdown receive them — consistent with the
                // story 2.5 graceful-shutdown design.
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
