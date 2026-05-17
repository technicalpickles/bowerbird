use protocol::{EventEnvelope, EventId};

use crate::db::queries::{
    event_kind_as_str, reaction_as_db_string, INSERT_EVENT, UPSERT_SESSION_PROJECTION,
};
use crate::error::{Error, Result};

/// Sole owner of the SQLite write transaction.
///
/// Inserts one row into `events` and upserts the matching row in
/// `session_projections` inside a single transaction containing exactly those
/// two writes — nothing else.
pub async fn write(
    writer_pool: &deadpool_sqlite::Pool,
    envelope: EventEnvelope,
) -> Result<EventId> {
    let conn = writer_pool
        .get()
        .await
        .map_err(|e| Error::Pool(format!("writer pool get failed: {e}")))?;

    let now_ms = current_unix_millis();
    let source = envelope.source;
    let session_id = envelope.session_id;
    let kind_str = event_kind_as_str(&envelope.kind);
    let reaction_str = envelope.reaction.as_ref().map(reaction_as_db_string);
    let payload = envelope.payload;
    // For Story 1.2 the projection state is a placeholder. Story 1.6 populates it.
    let state_json = "{}".to_string();

    let interact_res = conn
        .interact(move |c| -> rusqlite::Result<i64> {
            let tx = c.transaction()?;
            tx.execute(
                UPSERT_SESSION_PROJECTION,
                rusqlite::params![source, session_id, state_json, now_ms],
            )?;
            tx.execute(
                INSERT_EVENT,
                rusqlite::params![source, session_id, kind_str, reaction_str, payload, now_ms],
            )?;
            let id = tx.last_insert_rowid();
            tx.commit()?;
            Ok(id)
        })
        .await
        .map_err(|e| Error::Pool(format!("interact failed: {e}")))?;

    let event_id = interact_res?;
    Ok(EventId(event_id))
}

fn current_unix_millis() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    i64::try_from(now).unwrap_or(i64::MAX)
}
