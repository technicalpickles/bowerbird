pub mod handler;
pub mod listener;
pub mod writer;

pub use listener::run as listener_run;
pub use writer::run as writer_run;

use std::sync::Arc;

use deadpool_sqlite::Pool;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::broadcast::BroadcastHub;

/// The broadcaster + writer task that [`wire_writer`] produces. The
/// `broadcaster` field is the same `Arc<BroadcastHub>` the writer
/// publishes through, so a caller that stores it in `AppState` shares
/// the channel with the writer by construction.
pub struct IngestWiring {
    pub broadcaster: Arc<BroadcastHub>,
    pub writer_task: JoinHandle<()>,
}

/// Construct a `BroadcastHub`, spawn `ingest::writer::run` with it,
/// and return the same `Arc` so the caller can stash it in `AppState`.
///
/// Production (`main.rs::run`) and integration tests that exercise the
/// ingest -> WS path both call this helper. Keeping the broadcaster
/// construction and the writer spawn in one place rules out a class of
/// `main.rs` regressions where a refactor accidentally creates two
/// hubs (writer publishes into hub A, `AppState` holds hub B, WS
/// subscribers see nothing). The structural fix is the type: callers
/// cannot return a broadcaster different from the one the writer was
/// given.
pub fn wire_writer(
    rx: mpsc::Receiver<protocol::EventEnvelope>,
    writer_pool: Pool,
    shutdown: CancellationToken,
    ws_broadcast_capacity: usize,
) -> IngestWiring {
    let broadcaster = Arc::new(BroadcastHub::new(ws_broadcast_capacity));
    let writer_task = tokio::spawn(writer::run(rx, writer_pool, broadcaster.clone(), shutdown));
    IngestWiring {
        broadcaster,
        writer_task,
    }
}
