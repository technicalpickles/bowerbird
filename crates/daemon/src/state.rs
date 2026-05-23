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
    pub shutdown: CancellationToken,
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
}
