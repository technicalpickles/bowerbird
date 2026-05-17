use rusqlite_migration::{Migrations, M};

use crate::db::DbPools;
use crate::error::Result;

const M0001_INITIAL_SCHEMA: &str = "\
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
    ended_event_id   INTEGER
);
";

pub fn migrations() -> Migrations<'static> {
    Migrations::new(vec![M::up(M0001_INITIAL_SCHEMA)])
}

pub async fn run_migrations(pools: &DbPools) -> Result<()> {
    let conn = pools
        .writer
        .get()
        .await
        .map_err(crate::error::Error::from)?;
    conn.interact(|c| migrations().to_latest(c).map_err(crate::error::Error::from))
        .await
        .map_err(crate::error::Error::from)??;
    Ok(())
}
