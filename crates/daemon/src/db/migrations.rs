use rusqlite_migration::{Migrations, M};

use crate::error::{Error, Result};

const V1_UP: &str = "
    CREATE TABLE events (
        event_id   INTEGER PRIMARY KEY AUTOINCREMENT,
        source     TEXT    NOT NULL,
        session_id TEXT    NOT NULL,
        kind       TEXT    NOT NULL,
        reaction   TEXT,
        payload    TEXT    NOT NULL,
        created_at INTEGER NOT NULL
    );
    CREATE TABLE session_projections (
        source     TEXT    NOT NULL,
        session_id TEXT    NOT NULL,
        state      TEXT    NOT NULL,
        updated_at INTEGER NOT NULL,
        PRIMARY KEY (source, session_id)
    );
    CREATE TABLE recording_sessions (
        id               INTEGER PRIMARY KEY AUTOINCREMENT,
        started_event_id INTEGER NOT NULL,
        ended_event_id   INTEGER,
        FOREIGN KEY (started_event_id) REFERENCES events (event_id),
        FOREIGN KEY (ended_event_id)   REFERENCES events (event_id)
    );
";

pub fn migrations() -> Migrations<'static> {
    Migrations::new(vec![M::up(V1_UP)])
}

/// Run all pending migrations against the writer pool.
///
/// Returns `Ok(())` on success. On any failure (including a `user_version` ahead
/// of what this binary knows), returns `Error::Migration(...)` with a
/// human-readable message.
pub async fn run_migrations(writer_pool: &deadpool_sqlite::Pool) -> Result<()> {
    let conn = writer_pool
        .get()
        .await
        .map_err(|e| Error::Pool(format!("writer pool get failed: {e}")))?;

    let interact_res = conn
        .interact(|c| -> std::result::Result<(), String> {
            migrations()
                .to_latest(c)
                .map_err(|e| format!("rusqlite_migration: {e}"))
        })
        .await
        .map_err(|e| Error::Pool(format!("interact failed: {e}")))?;

    interact_res.map_err(Error::Migration)
}
