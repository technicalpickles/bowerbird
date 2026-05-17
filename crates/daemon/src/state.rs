use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use tokio_util::sync::CancellationToken;

use crate::db::DbPools;

#[derive(Clone)]
pub struct AppState {
    pub db: Arc<DbPools>,
    pub migrations_complete: Arc<AtomicBool>,
    pub shutdown: CancellationToken,
}
