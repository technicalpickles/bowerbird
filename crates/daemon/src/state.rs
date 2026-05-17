use std::sync::Arc;

use tokio_util::sync::CancellationToken;

use crate::db::DbPools;

pub struct AppState {
    pub db: DbPools,
    pub shutdown: CancellationToken,
}

pub type SharedState = Arc<AppState>;
