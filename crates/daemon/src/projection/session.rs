use protocol::{EventEnvelope, EventId};

use crate::db::queries::{
    event_kind_as_str, reaction_as_db_string, INSERT_EVENT, INSERT_RECORDING_SESSION_STARTED,
    UPDATE_RECORDING_SESSION_ENDED, UPSERT_SESSION_PROJECTION,
};
use crate::error::{Error, Result};

/// Sentinel `source`/`session_id` for daemon-emitted lifecycle events.
const DAEMON_SENTINEL_SOURCE: &str = "__daemon__";
const DAEMON_SENTINEL_SESSION: &str = "__daemon__";
const EMPTY_PAYLOAD: &str = "{}";

/// Returned by [`write_recording_started`] — the caller passes
/// [`RecordingStarted::recording_session_id`] back to
/// [`write_recording_ended`] so the right `recording_sessions` row is closed.
#[derive(Debug, Clone, Copy)]
pub struct RecordingStarted {
    pub event_id: EventId,
    pub recording_session_id: i64,
}

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

    let now_ms = current_unix_millis()?;
    let source = envelope.source;
    let session_id = envelope.session_id;
    let kind_str = event_kind_as_str(&envelope.kind);
    let reaction_str = envelope.reaction.as_ref().map(reaction_as_db_string);
    let payload = envelope.payload;
    // For Story 1.2 the projection state is a placeholder. Story 1.6 populates it.
    let state_json = EMPTY_PAYLOAD.to_string();

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

/// Write the daemon's `RecordingStarted` sentinel atomically with the
/// `recording_sessions` row. Three writes in one transaction: projection
/// upsert, event insert, recording-session insert — a deliberate
/// exception to the two-statement rule for `write`, justified because the
/// lifecycle marker must be inseparable from the event that opened it.
pub async fn write_recording_started(
    writer_pool: &deadpool_sqlite::Pool,
) -> Result<RecordingStarted> {
    let conn = writer_pool
        .get()
        .await
        .map_err(|e| Error::Pool(format!("writer pool get failed: {e}")))?;

    let now_ms = current_unix_millis()?;
    let kind_str = event_kind_as_str(&protocol::EventKind::RecordingStarted);
    let state_json = EMPTY_PAYLOAD.to_string();
    let payload = EMPTY_PAYLOAD.to_string();

    let interact_res = conn
        .interact(move |c| -> rusqlite::Result<(i64, i64)> {
            let tx = c.transaction()?;
            tx.execute(
                UPSERT_SESSION_PROJECTION,
                rusqlite::params![
                    DAEMON_SENTINEL_SOURCE,
                    DAEMON_SENTINEL_SESSION,
                    state_json,
                    now_ms
                ],
            )?;
            tx.execute(
                INSERT_EVENT,
                rusqlite::params![
                    DAEMON_SENTINEL_SOURCE,
                    DAEMON_SENTINEL_SESSION,
                    kind_str,
                    None::<String>,
                    payload,
                    now_ms
                ],
            )?;
            let event_id = tx.last_insert_rowid();
            tx.execute(
                INSERT_RECORDING_SESSION_STARTED,
                rusqlite::params![event_id],
            )?;
            let recording_session_id = tx.last_insert_rowid();
            tx.commit()?;
            Ok((event_id, recording_session_id))
        })
        .await
        .map_err(|e| Error::Pool(format!("interact failed: {e}")))?;

    let (event_id, recording_session_id) = interact_res?;
    Ok(RecordingStarted {
        event_id: EventId(event_id),
        recording_session_id,
    })
}

/// Close the `recording_sessions` row identified by `recording_session_id`
/// atomically with the `RecordingEnded` sentinel event. Three writes in one
/// transaction (same exception as [`write_recording_started`]).
pub async fn write_recording_ended(
    writer_pool: &deadpool_sqlite::Pool,
    recording_session_id: i64,
) -> Result<EventId> {
    let conn = writer_pool
        .get()
        .await
        .map_err(|e| Error::Pool(format!("writer pool get failed: {e}")))?;

    let now_ms = current_unix_millis()?;
    let kind_str = event_kind_as_str(&protocol::EventKind::RecordingEnded);
    let state_json = EMPTY_PAYLOAD.to_string();
    let payload = EMPTY_PAYLOAD.to_string();

    let interact_res = conn
        .interact(move |c| -> rusqlite::Result<i64> {
            let tx = c.transaction()?;
            tx.execute(
                UPSERT_SESSION_PROJECTION,
                rusqlite::params![
                    DAEMON_SENTINEL_SOURCE,
                    DAEMON_SENTINEL_SESSION,
                    state_json,
                    now_ms
                ],
            )?;
            tx.execute(
                INSERT_EVENT,
                rusqlite::params![
                    DAEMON_SENTINEL_SOURCE,
                    DAEMON_SENTINEL_SESSION,
                    kind_str,
                    None::<String>,
                    payload,
                    now_ms
                ],
            )?;
            let event_id = tx.last_insert_rowid();
            let rows = tx.execute(
                UPDATE_RECORDING_SESSION_ENDED,
                rusqlite::params![event_id, recording_session_id],
            )?;
            if rows != 1 {
                return Err(rusqlite::Error::QueryReturnedNoRows);
            }
            tx.commit()?;
            Ok(event_id)
        })
        .await
        .map_err(|e| Error::Pool(format!("interact failed: {e}")))?;

    let event_id = interact_res?;
    Ok(EventId(event_id))
}

fn current_unix_millis() -> Result<i64> {
    use std::time::{SystemTime, UNIX_EPOCH};
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| Error::Clock(format!("system time before UNIX_EPOCH: {e}")))?
        .as_millis();
    i64::try_from(now_ms).map_err(|_| Error::Clock(format!("timestamp overflows i64: {now_ms}")))
}
