use std::path::Path;

use deadpool_sqlite::{Config, Hook, HookError, Pool, Runtime};

use crate::error::{Error, Result};

#[derive(Debug)]
pub struct DbPools {
    pub writer: Pool,
    pub reader: Pool,
}

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

    let pool = builder
        .max_size(max_size)
        .post_create(Hook::async_fn(|wrapper, _metrics| {
            Box::pin(async move {
                let inner_res: std::result::Result<
                    std::result::Result<(), rusqlite::Error>,
                    deadpool_sqlite::InteractError,
                > = wrapper
                    .interact(|conn: &mut rusqlite::Connection| -> rusqlite::Result<()> {
                        conn.execute_batch(
                            "PRAGMA foreign_keys = ON;\
                             PRAGMA journal_mode = WAL;\
                             PRAGMA synchronous = NORMAL;\
                             PRAGMA busy_timeout = 5000;",
                        )?;
                        Ok(())
                    })
                    .await;

                match inner_res {
                    Ok(Ok(())) => Ok(()),
                    Ok(Err(sqlite_err)) => Err(HookError::Backend(sqlite_err)),
                    Err(interact_err) => Err(HookError::message(interact_err.to_string())),
                }
            })
        }))
        .build()
        .map_err(|e| Error::Pool(e.to_string()))?;

    Ok(pool)
}
