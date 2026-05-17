//! # Invariants
//!
//! This module is the sole permitted caller of `rusqlite::Connection::open*` in
//! the daemon crate. The CI lint (see `.github/workflows/ci.yml`) enforces this:
//! any `rusqlite::Connection::open` reference outside this file fails the build.
//!
//! Every connection produced here is configured with the daemon's PRAGMA
//! invariants — `journal_mode=WAL`, `synchronous=NORMAL`, `foreign_keys=ON`,
//! `busy_timeout=5000` — applied immediately on open, so any subsequent checkout
//! observes them without additional setup.

use std::path::Path;
use std::time::Duration;

use deadpool_sqlite::{Config as DeadpoolConfig, Hook, HookError, Pool, Runtime};
use rusqlite::Connection;

use crate::config::Config;
use crate::error::{Error, Result};

const BUSY_TIMEOUT_MS: u64 = 5_000;

/// Apply the daemon's required PRAGMAs to a freshly opened connection.
///
/// `foreign_keys` is the canary — it is NOT on by default in SQLite, and a
/// contract test asserts it is `1` on every checkout.
pub(crate) fn apply_pragmas(conn: &Connection) -> rusqlite::Result<()> {
    // busy_timeout FIRST so any subsequent pragma contention (notably
    // journal_mode=WAL acquiring an EXCLUSIVE lock during the first mode
    // transition) waits within a bounded window instead of deadlocking.
    conn.busy_timeout(Duration::from_millis(BUSY_TIMEOUT_MS))?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    Ok(())
}

/// Open a connection and apply the daemon PRAGMA invariants.
///
/// This is the **only** path through which raw SQLite connections enter daemon
/// code. The connection pool's manager (`deadpool_sqlite::Manager`) opens its
/// own connections internally; the `post_create` hook applies the same PRAGMAs
/// via `apply_pragmas`, so the invariants hold for every checkout regardless of
/// origin.
pub(crate) fn open_connection(path: &Path) -> Result<Connection> {
    let conn = Connection::open(path)?;
    apply_pragmas(&conn)?;
    Ok(conn)
}

pub struct DbPools {
    pub writer: Pool,
    pub readers: Pool,
}

impl DbPools {
    pub async fn close(&self) {
        self.writer.close();
        self.readers.close();
    }
}

fn build_single_pool(path: &Path, max_size: usize) -> Result<Pool> {
    let cfg = DeadpoolConfig::new(path);
    let pool = cfg
        .builder(Runtime::Tokio1)
        .map_err(|e| Error::Config(format!("pool builder: {e}")))?
        .max_size(max_size)
        .post_create(Hook::async_fn(|wrapper, _metrics| {
            Box::pin(async move {
                let pragma_result: rusqlite::Result<()> = wrapper
                    .interact(|conn| apply_pragmas(conn))
                    .await
                    .map_err(|e| HookError::message(e.to_string()))?;
                pragma_result.map_err(|e| HookError::message(e.to_string()))?;
                Ok(())
            })
        }))
        .build()
        .map_err(|e| Error::Config(format!("pool build: {e}")))?;
    Ok(pool)
}

pub async fn build_pools(cfg: &Config) -> Result<DbPools> {
    let path = cfg.db_path();
    let writer = build_single_pool(&path, cfg.writer_pool_max)?;
    let readers = build_single_pool(&path, cfg.reader_pool_max)?;
    Ok(DbPools { writer, readers })
}

pub async fn checkout_writer(pools: &DbPools) -> Result<deadpool_sqlite::Object> {
    pools.writer.get().await.map_err(Error::from)
}

pub async fn checkout_reader(pools: &DbPools) -> Result<deadpool_sqlite::Object> {
    pools.readers.get().await.map_err(Error::from)
}
