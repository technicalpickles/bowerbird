use deadpool_sqlite::Pool;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use protocol::EventEnvelope;

use crate::projection;

pub async fn run(
    mut rx: mpsc::Receiver<EventEnvelope>,
    writer_pool: Pool,
    shutdown: CancellationToken,
) {
    loop {
        tokio::select! {
            item = rx.recv() => {
                match item {
                    Some(envelope) => {
                        if let Err(e) = projection::session::write(&writer_pool, envelope).await {
                            tracing::error!(error = ?e, "projection write failed; event dropped");
                        }
                    }
                    None => break,
                }
            }
            _ = shutdown.cancelled() => {
                while let Ok(envelope) = rx.try_recv() {
                    if let Err(e) = projection::session::write(&writer_pool, envelope).await {
                        tracing::error!(error = ?e, "projection write failed during drain; event dropped");
                    }
                }
                break;
            }
        }
    }
}
