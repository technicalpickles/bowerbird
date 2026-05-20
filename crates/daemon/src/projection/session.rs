use protocol::{EventEnvelope, EventId, SessionState};
use rusqlite::OptionalExtension;

use crate::db::queries::{
    event_kind_as_str, event_kind_from_db_str, reaction_as_db_string, INSERT_EVENT,
    INSERT_RECORDING_SESSION_STARTED, SELECT_DISTINCT_SESSIONS_FROM_EVENTS,
    SELECT_EVENT_KINDS_FOR_SESSION, SELECT_SESSION_PROJECTION_STATE,
    UPDATE_RECORDING_SESSION_ENDED, UPSERT_SESSION_PROJECTION,
};
use crate::error::{Error, Result};
use crate::projection::state::transition;

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
#[tracing::instrument(skip_all, fields(source = %envelope.source, session_id = %envelope.session_id))]
pub async fn write(
    writer_pool: &deadpool_sqlite::Pool,
    envelope: EventEnvelope,
) -> Result<EventId> {
    // The ingest path only produces normalized session events. Sentinel
    // kinds route through `write_recording_started` / `write_recording_ended`
    // and must never reach this function (architecture.md:634-641).
    #[cfg(debug_assertions)]
    debug_assert!(
        !matches!(
            envelope.kind,
            protocol::EventKind::RecordingStarted | protocol::EventKind::RecordingEnded
        ),
        "sentinel EventKind reached projection::session::write — use sentinel writers"
    );

    let conn = writer_pool
        .get()
        .await
        .map_err(|e| Error::Pool(format!("writer pool get failed: {e}")))?;

    let now_ms = current_unix_millis()?;
    let source = envelope.source;
    let session_id = envelope.session_id;
    let kind_for_transition = envelope.kind.clone();
    let kind_str = event_kind_as_str(&envelope.kind);
    let reaction_str = envelope.reaction.as_ref().map(reaction_as_db_string);
    let payload = envelope.payload;

    let interact_res = conn
        .interact(move |c| -> rusqlite::Result<i64> {
            let tx = c.transaction()?;

            // Read the prior state inside the transaction so a concurrent
            // writer cannot interleave between SELECT and UPSERT. Reads do
            // not break the "exactly two writes" invariant from
            // architecture.md:634-641 — the invariant is about *writes*.
            let prev_state: Option<SessionState> = tx
                .query_row(
                    SELECT_SESSION_PROJECTION_STATE,
                    rusqlite::params![&source, &session_id],
                    |row| row.get::<_, String>(0),
                )
                .optional()?
                .and_then(|raw| match serde_json::from_str::<SessionState>(&raw) {
                    Ok(v) => Some(v),
                    Err(e) => {
                        tracing::error!(
                            error = %e,
                            raw = %raw,
                            "session_projections.state failed to deserialize; \
                             treating as fresh — next event will overwrite",
                        );
                        None
                    }
                });

            let new_state = transition(prev_state.as_ref(), kind_for_transition, now_ms);
            let state_json = serde_json::to_string(&new_state).map_err(|e| {
                rusqlite::Error::ToSqlConversionFailure(Box::new(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("SessionState serialize failed: {e}"),
                )))
            })?;

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
    // Sentinel rows have no meaningful `current_state`. The `__daemon__/__daemon__`
    // row is excluded from session-listing queries (Story 1.7), so the placeholder
    // `"{}"` is intentional and does not flow to any presenter.
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
    // Sentinel rows have no meaningful `current_state` — see
    // `write_recording_started` for rationale.
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

/// Replay the event log to rebuild any missing `session_projections` rows.
///
/// Iterates distinct non-sentinel `(source, session_id)` pairs in `events`,
/// rebuilds only the ones that have no projection row, and UPSERTs the
/// derived [`SessionState`] computed by folding [`transition`] over the
/// event stream. Returns the number of projections rebuilt.
///
/// Best-effort: a per-session failure is logged but does not abort the
/// remaining rebuild. Callers should treat a `Err` only as a data-correctness
/// warning, never as a startup blocker (see story 1.6 Task 6 rationale).
///
/// Runs in a single transaction so a partial crash mid-rebuild does not
/// leave a half-populated projection table.
#[tracing::instrument(skip_all)]
pub async fn rebuild_missing_projections(writer_pool: &deadpool_sqlite::Pool) -> Result<usize> {
    let conn = writer_pool
        .get()
        .await
        .map_err(|e| Error::Pool(format!("writer pool get failed: {e}")))?;

    let rebuilt = conn
        .interact(move |c| -> rusqlite::Result<usize> {
            let tx = c.transaction()?;

            let pairs: Vec<(String, String)> = {
                let mut stmt = tx.prepare(SELECT_DISTINCT_SESSIONS_FROM_EVENTS)?;
                let rows = stmt.query_map([], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })?;
                rows.collect::<rusqlite::Result<Vec<_>>>()?
            };

            let mut rebuilt = 0usize;
            for (source, session_id) in pairs {
                let existing: Option<String> = tx
                    .query_row(
                        SELECT_SESSION_PROJECTION_STATE,
                        rusqlite::params![&source, &session_id],
                        |row| row.get(0),
                    )
                    .optional()?;
                if existing.is_some() {
                    continue;
                }

                let kinds: Vec<(String, i64)> = {
                    let mut stmt = tx.prepare(SELECT_EVENT_KINDS_FOR_SESSION)?;
                    let rows = stmt.query_map(rusqlite::params![&source, &session_id], |row| {
                        Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
                    })?;
                    rows.collect::<rusqlite::Result<Vec<_>>>()?
                };

                let mut state: Option<SessionState> = None;
                let mut last_created_at: i64 = 0;
                let mut bad_kind = false;
                for (kind_str, created_at) in kinds {
                    last_created_at = created_at;
                    let kind = match event_kind_from_db_str(&kind_str) {
                        Ok(k) => k,
                        Err(e) => {
                            tracing::error!(
                                source = %source,
                                session_id = %session_id,
                                kind = %kind_str,
                                error = %e,
                                "rebuild_missing_projections: unknown EventKind in events.kind; \
                                 skipping this session"
                            );
                            bad_kind = true;
                            break;
                        }
                    };
                    // Sentinel kinds should be filtered by the source != '__daemon__' clause
                    // on SELECT_DISTINCT_SESSIONS_FROM_EVENTS, but defend against future shape
                    // changes by routing them through transition's defensive branch anyway.
                    let next = transition(state.as_ref(), kind, created_at);
                    state = Some(next);
                }
                if bad_kind {
                    continue;
                }
                let Some(state) = state else { continue };
                let state_json = match serde_json::to_string(&state) {
                    Ok(s) => s,
                    Err(e) => {
                        tracing::error!(
                            source = %source,
                            session_id = %session_id,
                            error = %e,
                            "rebuild_missing_projections: SessionState serialize failed"
                        );
                        continue;
                    }
                };
                tx.execute(
                    UPSERT_SESSION_PROJECTION,
                    rusqlite::params![&source, &session_id, &state_json, last_created_at],
                )?;
                rebuilt += 1;
                tracing::info!(
                    source = %source,
                    session_id = %session_id,
                    "rebuilt session projection"
                );
            }

            tx.commit()?;
            Ok(rebuilt)
        })
        .await
        .map_err(|e| Error::Pool(format!("interact failed: {e}")))??;

    Ok(rebuilt)
}

use crate::time::current_unix_millis;
