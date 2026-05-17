use std::path::Path;
use std::time::Duration;

use deadpool_sqlite::{Config, Hook, HookError, Pool, Runtime, Timeouts};

use crate::error::{Error, Result};

#[derive(Debug, Clone)]
pub struct DbPools {
    pub writer: Pool,
    pub reader: Pool,
}

const PRAGMA_BUNDLE: &str = "PRAGMA foreign_keys = ON;\
                             PRAGMA journal_mode = WAL;\
                             PRAGMA synchronous = NORMAL;\
                             PRAGMA busy_timeout = 5000;";

const POOL_WAIT_TIMEOUT: Duration = Duration::from_secs(5);

pub async fn init_pools(db_path: &Path) -> Result<DbPools> {
    let writer = build_pool(db_path, 1)?;
    let reader = build_pool(db_path, 4)?;
    Ok(DbPools { writer, reader })
}

fn build_pool(db_path: &Path, max_size: usize) -> Result<Pool> {
    let cfg = Config::new(db_path);
    // ConfigError is Infallible — the `_` arm is unreachable in practice.
    let builder = cfg
        .builder(Runtime::Tokio1)
        .map_err(|_| Error::Pool("infallible config error".to_string()))?;

    // PRAGMA bundle is re-applied on every checkout (post_create + post_recycle)
    // so AC#2's "every checkout without exception" holds literally, even though
    // sqlite pragmas like foreign_keys/synchronous persist for the connection
    // lifetime.
    let pool = builder
        .max_size(max_size)
        .timeouts(Timeouts {
            wait: Some(POOL_WAIT_TIMEOUT),
            create: None,
            recycle: None,
        })
        .post_create(Hook::async_fn(|wrapper, _metrics| {
            Box::pin(async move {
                let inner_res = wrapper
                    .interact(|conn: &mut rusqlite::Connection| -> rusqlite::Result<()> {
                        conn.execute_batch(PRAGMA_BUNDLE)?;
                        verify_wal(conn)
                    })
                    .await;
                hook_result(inner_res)
            })
        }))
        .post_recycle(Hook::async_fn(|wrapper, _metrics| {
            Box::pin(async move {
                let inner_res = wrapper
                    .interact(|conn: &mut rusqlite::Connection| -> rusqlite::Result<()> {
                        conn.execute_batch(PRAGMA_BUNDLE)?;
                        verify_wal(conn)
                    })
                    .await;
                hook_result(inner_res)
            })
        }))
        .build()
        .map_err(|e| Error::Pool(e.to_string()))?;

    Ok(pool)
}

/// Refuse a connection whose `journal_mode` did not actually flip to WAL — on
/// a read-only or network filesystem the PRAGMA silently no-ops and returns
/// the current mode, which would violate the durability contract.
fn verify_wal(conn: &rusqlite::Connection) -> rusqlite::Result<()> {
    let mode: String = conn.query_row("PRAGMA journal_mode", [], |r| r.get(0))?;
    if mode.eq_ignore_ascii_case("wal") {
        Ok(())
    } else {
        Err(rusqlite::Error::InvalidQuery)
    }
}

fn hook_result(
    inner_res: std::result::Result<rusqlite::Result<()>, deadpool_sqlite::InteractError>,
) -> std::result::Result<(), HookError> {
    match inner_res {
        Ok(Ok(())) => Ok(()),
        Ok(Err(sqlite_err)) => Err(HookError::Backend(sqlite_err)),
        Err(interact_err) => Err(HookError::message(interact_err.to_string())),
    }
}
