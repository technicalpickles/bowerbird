use std::sync::Arc;

use deadpool_sqlite::Pool;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::broadcast::BroadcastHub;
use crate::ingest::{IngestItem, IngestOrigin};
use crate::projection;

/// Write one ingest item. Story 5.11 / ADR 0009 §7: live hooks run the
/// PID-supersession follow-up (`write`); `/replay` does not (`write_replayed`)
/// — the synthetic `SessionEnded` rows are already in the log being replayed.
async fn write_item(
    writer_pool: &Pool,
    broadcaster: &BroadcastHub,
    item: IngestItem,
) -> crate::error::Result<protocol::EventId> {
    match item.origin {
        IngestOrigin::Live => {
            projection::session::write(writer_pool, broadcaster, item.envelope).await
        }
        IngestOrigin::Replay => {
            projection::session::write_replayed(writer_pool, broadcaster, item.envelope).await
        }
    }
}

pub async fn run(
    mut rx: mpsc::Receiver<IngestItem>,
    writer_pool: Pool,
    broadcaster: Arc<BroadcastHub>,
    shutdown: CancellationToken,
) {
    loop {
        tokio::select! {
            item = rx.recv() => {
                match item {
                    Some(item) => {
                        if let Err(e) = write_item(&writer_pool, &broadcaster, item).await {
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
                while let Ok(item) = rx.try_recv() {
                    if let Err(e) = write_item(&writer_pool, &broadcaster, item).await {
                        tracing::error!(error = ?e, "projection write failed during drain; event dropped");
                    }
                }
                break;
            }
        }
    }
}
