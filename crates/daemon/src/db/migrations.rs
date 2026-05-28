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

// Story 5.3 — add `events.pid` for daemon-observed session liveness.
// Existing rows get `pid = NULL` by default; rebuild_missing_projections sees
// NULL as "no pid known" and the startup liveness probe emits a SessionEnded
// for any projection row whose carried-forward last_pid stays NULL
// ("no_pid_at_upgrade" reason).
const V2_UP: &str = "ALTER TABLE events ADD COLUMN pid INTEGER";

pub fn migrations() -> Migrations<'static> {
    Migrations::new(vec![M::up(V1_UP), M::up(V2_UP)])
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

#[cfg(test)]
mod tests {
    use super::*;

    // Story 5.3: re-running `to_latest` against an already-migrated DB must be
    // a no-op. Story 5.4 added the populated-DB contract test in
    // `crates/daemon/tests/contract_daemon.rs::story_5_4_migrations`; this unit
    // test stays as the in-memory baseline canary.
    #[test]
    fn migrations_are_idempotent() {
        let mut conn = rusqlite::Connection::open_in_memory().expect("in-memory connection");
        let m = migrations();
        m.to_latest(&mut conn).expect("first to_latest");
        let v1: i64 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .expect("user_version read");
        m.to_latest(&mut conn)
            .expect("second to_latest must succeed");
        let v2: i64 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .expect("user_version read");
        assert_eq!(v1, v2, "user_version must be stable on re-apply");
        assert!(
            v1 >= 2,
            "expected user_version >= 2 after migrations, got {v1}"
        );
    }

    // Story 5.3: migration v2 must add `events.pid` as a nullable INTEGER, and
    // existing rows must default to NULL.
    #[test]
    fn migration_v2_adds_nullable_pid_column() {
        let mut conn = rusqlite::Connection::open_in_memory().expect("in-memory connection");
        migrations()
            .to_latest(&mut conn)
            .expect("migrations must apply");

        // Insert a row and verify pid is NULL by default (the migration uses
        // ALTER TABLE ADD COLUMN, which yields NULL for pre-existing rows; new
        // rows that don't set pid also get NULL).
        conn.execute(
            "INSERT INTO events (source, session_id, kind, payload, created_at) \
             VALUES ('claude', 's1', 'Stop', '{}', 0)",
            [],
        )
        .expect("insert");

        let pid: Option<i64> = conn
            .query_row("SELECT pid FROM events WHERE session_id = 's1'", [], |r| {
                r.get(0)
            })
            .expect("read pid");
        assert_eq!(pid, None, "default pid must be NULL");
    }
}
