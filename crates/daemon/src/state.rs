use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Duration;

use tokio_util::sync::CancellationToken;

use crate::api::token::BearerToken;
use crate::broadcast::BroadcastHub;
use crate::db::DbPools;

#[derive(Clone)]
pub struct AppState {
    pub db: DbPools,
    pub migrations_complete: Arc<AtomicBool>,
    pub shutdown_requested: CancellationToken,
    pub ws_close_requested: CancellationToken,
    pub bearer: BearerToken,
    pub started_at_ms: i64,
    pub broadcaster: Arc<BroadcastHub>,
    pub ws_semaphore: Arc<tokio::sync::Semaphore>,
    pub ws_config: WsConfig,
}

/// Small `Copy` snapshot of the WS-specific knobs so per-connection tasks
/// don't have to clone the entire `AppState` to read them.
#[derive(Debug, Clone, Copy)]
pub struct WsConfig {
    pub ping_interval: Duration,
    pub pong_timeout: Duration,
    /// Coalescing window for `DroppedFrame` emissions on lag. See
    /// `Config::ws_broadcast_coalesce_window`.
    pub coalesce_window: Duration,
    /// Cap on concurrent WebSocket connections, mirroring
    /// `Config::ws_max_connections`. Needed in `WsConfig` so the `/status`
    /// handler can compute `connected_ws_clients = max - available_permits`
    /// without threading a second `Arc<usize>` through `AppState`.
    pub max_connections: usize,
}

pub async fn wait_for_ws_connection_drain(
    semaphore: Arc<tokio::sync::Semaphore>,
    max_connections: usize,
    timeout: Duration,
) -> Result<(), tokio::time::error::Elapsed> {
    let permit_count = u32::try_from(max_connections).unwrap_or(u32::MAX);
    let result = tokio::time::timeout(timeout, async move {
        match semaphore.acquire_many_owned(permit_count).await {
            Ok(permits) => drop(permits),
            Err(e) => {
                tracing::debug!(error = ?e, "ws drain wait skipped; semaphore closed");
            }
        }
    })
    .await;
    if result.is_err() {
        tracing::warn!(
            timeout = ?timeout,
            "ws connection drain timed out during shutdown; proceeding"
        );
    }
    result
}
