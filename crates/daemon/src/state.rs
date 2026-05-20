use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use tokio_util::sync::CancellationToken;

use crate::api::token::BearerToken;
use crate::db::DbPools;

#[derive(Clone)]
pub struct AppState {
    pub db: DbPools,
    pub migrations_complete: Arc<AtomicBool>,
    pub shutdown: CancellationToken,
    pub bearer: BearerToken,
    pub started_at_ms: i64,
}
