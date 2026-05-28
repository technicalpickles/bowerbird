use protocol::{Event, EventEnvelope, EventId, SessionCurrentState, SessionState};
use rusqlite::OptionalExtension;

use crate::broadcast::{BroadcastEnvelope, BroadcastHub};
use crate::db::queries::{
    event_kind_as_str, event_kind_from_db_str, reaction_as_db_string, INSERT_EVENT,
    INSERT_RECORDING_SESSION_STARTED, SELECT_DISTINCT_SESSIONS_FROM_EVENTS,
    SELECT_EVENT_KINDS_FOR_SESSION, SELECT_SESSION_PROJECTION_STATE,
    UPDATE_RECORDING_SESSION_ENDED, UPSERT_SESSION_PROJECTION,
};
use crate::error::{Error, Result};
use crate::projection::state::{current_state_for_read, transition};

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

/// Conditional-write guard for [`write_if_state_matches`]. The synthetic
/// liveness probe's SessionEnded write is only committed if the projection
/// row's `current_state` and `last_pid` still match the snapshot the probe
/// observed — otherwise a real hook event has already moved the session and
/// the probe should yield to it (story 5.3 review finding #2).
#[derive(Debug, Clone, Copy)]
pub struct WritePrecondition {
    pub expected_current_state: SessionCurrentState,
    pub expected_last_pid: Option<u32>,
}

/// Sole owner of the SQLite write transaction AND the sole publisher of
/// user-facing `BroadcastEnvelope::Event` / `BroadcastEnvelope::State`
/// frames (per story 2.2; refined by story 5.2).
///
/// Inserts one row into `events` and upserts the matching row in
/// `session_projections` inside a single transaction containing exactly those
/// two writes — nothing else. After `tx.commit()` succeeds, publishes one
/// `BroadcastEnvelope::Event` followed by ZERO-OR-ONE `BroadcastEnvelope::State`
/// so any WS subscribers see the event before the resulting projection update
/// IFF the projection update actually changed `current_state` (story 5.2). A
/// first event for a previously-unknown session counts as a transition
/// (`None != Some(new_state.current_state)`) and DOES publish State.
/// Publishing is gated on commit success: if `interact` or `tx.commit` returns
/// `Err`, no envelope is published (story 2.2 AC #6).
///
/// Sentinel writes (`write_recording_started` / `write_recording_ended`) do
/// NOT publish — daemon lifecycle is excluded from the user-facing surface
/// (story 2.2 AC #7).
#[tracing::instrument(skip_all, fields(source = %envelope.source, session_id = %envelope.session_id))]
pub async fn write(
    writer_pool: &deadpool_sqlite::Pool,
    broadcaster: &BroadcastHub,
    envelope: EventEnvelope,
) -> Result<EventId> {
    write_inner(writer_pool, broadcaster, envelope, None)
        .await?
        .ok_or_else(|| {
            Error::Projection("write() with no precondition unexpectedly skipped".into())
        })
}

/// Conditional variant of [`write`] used by the liveness probe. Re-reads the
/// projection row inside the writer transaction and only commits if
/// `current_state` AND `last_pid` still match `precondition` — otherwise the
/// txn drops without committing, no broadcast is emitted, and `Ok(None)` is
/// returned. Story 5.3 review finding #2: a real hook event may land between
/// the probe's SELECT and the synthetic write; the probe must yield to it.
#[tracing::instrument(skip_all, fields(source = %envelope.source, session_id = %envelope.session_id))]
pub async fn write_if_state_matches(
    writer_pool: &deadpool_sqlite::Pool,
    broadcaster: &BroadcastHub,
    envelope: EventEnvelope,
    precondition: WritePrecondition,
) -> Result<Option<EventId>> {
    write_inner(writer_pool, broadcaster, envelope, Some(precondition)).await
}

/// Closure return type from `write_inner`'s `interact` callback. `None`
/// means the precondition (if any) was not met — the txn dropped without
/// committing, no broadcast is owed.
type WriteInteractResult = Option<(
    i64,
    SessionState,
    Option<SessionCurrentState>,
    Option<SessionCurrentState>,
)>;

async fn write_inner(
    writer_pool: &deadpool_sqlite::Pool,
    broadcaster: &BroadcastHub,
    envelope: EventEnvelope,
    precondition: Option<WritePrecondition>,
) -> Result<Option<EventId>> {
    // Sentinel kinds route through `write_recording_started` /
    // `write_recording_ended` and must never reach this function
    // (architecture.md:634-641). A runtime guard (not just `debug_assert!`)
    // is required so release builds also cannot publish daemon-lifecycle
    // sentinels through the user-facing broadcast path (story 2.2 AC #7).
    // Reject before pool checkout so a misuse cannot insert a row, commit
    // a transaction, or emit a broadcast envelope.
    if matches!(
        envelope.kind,
        protocol::EventKind::RecordingStarted | protocol::EventKind::RecordingEnded
    ) {
        return Err(Error::Projection(format!(
            "sentinel EventKind ({:?}) cannot be written through projection::session::write; \
             use write_recording_started / write_recording_ended",
            envelope.kind
        )));
    }

    let conn = writer_pool
        .get()
        .await
        .map_err(|e| Error::Pool(format!("writer pool get failed: {e}")))?;

    let now_ms = current_unix_millis()?;
    let source = envelope.source;
    let session_id = envelope.session_id;
    let kind = envelope.kind;
    let reaction = envelope.reaction;
    let payload = envelope.payload;
    let pid = envelope.pid;
    let notification_type = envelope.notification_type;

    // The closure moves its captures, so duplicate the fields the post-commit
    // publish path needs. SessionState is returned out of the closure to avoid
    // recomputing `transition` against a post-commit DB (which would race with
    // a future multi-writer world even though today's single-writer pool
    // serializes writes).
    let source_for_closure = source.clone();
    let session_id_for_closure = session_id.clone();
    let kind_for_transition = kind.clone();
    let kind_str = event_kind_as_str(&kind);
    let reaction_str = reaction.as_ref().map(reaction_as_db_string);
    let payload_for_closure = payload.clone();

    let interact_res = conn
        .interact(move |c| -> rusqlite::Result<WriteInteractResult> {
            let tx = c.transaction()?;

            // Read the prior state inside the transaction so a concurrent
            // writer cannot interleave between SELECT and UPSERT. Reads do
            // not break the "exactly two writes" invariant from
            // architecture.md:634-641 — the invariant is about *writes*.
            let prev_state: Option<SessionState> = tx
                .query_row(
                    SELECT_SESSION_PROJECTION_STATE,
                    rusqlite::params![&source_for_closure, &session_id_for_closure],
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

            // Story 5.3 review finding #2: if the caller supplied a
            // precondition (only the liveness probe does), bail before any
            // write if the row no longer matches the snapshot we observed.
            // A real hook event has already moved the session; the probe
            // yields to it. Dropping `tx` here rolls back (no rows
            // touched), so this branch leaves the DB and the broadcast
            // channel untouched.
            if let Some(pc) = precondition {
                let matches = match prev_state.as_ref() {
                    Some(s) => {
                        s.current_state == pc.expected_current_state
                            && s.last_pid == pc.expected_last_pid
                    }
                    // No prev row means the projection was deleted (or
                    // never existed) after the probe SELECT — definitely
                    // changed since snapshot. Skip.
                    None => false,
                };
                if !matches {
                    return Ok(None);
                }
            }

            // Capture prev's READ-FACING current_state BEFORE the closure
            // consumes `prev_state` into `transition` — the post-commit
            // publish path needs both prev and new current_state to decide
            // whether to emit a `BroadcastEnvelope::State` (story 5.2).
            //
            // The read-facing value (via `current_state_for_read`) folds in
            // the `STALE_WORKING_MS` fallback so a stale stored `Working`
            // that subscribers were seeing as `Idle` (via snapshot, REST,
            // or any read path) triggers a State envelope when a new event
            // restores live `Working`. Comparing raw stored states would
            // miss that transition because the stored row sat at
            // `Working` the whole time.
            let prev_raw_current_state = prev_state.as_ref().map(|s| s.current_state);
            let prev_read_current_state = prev_state
                .as_ref()
                .map(|s| current_state_for_read(s, now_ms));

            let new_state = transition(
                prev_state.as_ref(),
                kind_for_transition,
                notification_type,
                pid,
                now_ms,
            );
            let state_json = serde_json::to_string(&new_state).map_err(|e| {
                rusqlite::Error::ToSqlConversionFailure(Box::new(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("SessionState serialize failed: {e}"),
                )))
            })?;

            tx.execute(
                UPSERT_SESSION_PROJECTION,
                rusqlite::params![
                    source_for_closure,
                    session_id_for_closure,
                    state_json,
                    now_ms
                ],
            )?;
            tx.execute(
                INSERT_EVENT,
                rusqlite::params![
                    source_for_closure,
                    session_id_for_closure,
                    kind_str,
                    reaction_str,
                    payload_for_closure,
                    now_ms,
                    pid,
                ],
            )?;
            let id = tx.last_insert_rowid();
            tx.commit()?;
            Ok(Some((
                id,
                new_state,
                prev_raw_current_state,
                prev_read_current_state,
            )))
        })
        .await
        .map_err(|e| Error::Pool(format!("interact failed: {e}")))?;

    let (event_id_raw, new_state, prev_raw_current_state, prev_read_current_state) =
        match interact_res? {
            Some(t) => t,
            None => {
                tracing::debug!(
                    "write_if_state_matches: precondition not met; skipping synthetic write"
                );
                return Ok(None);
            }
        };
    let event_id = EventId(event_id_raw);

    // Post-commit publish. Event BEFORE State so a presenter consuming both
    // topics sees the triggering event before the resulting projection
    // update. `tokio::sync::broadcast` preserves per-channel order across
    // sequential publishes, so every subscriber sees Event → State in the
    // same relative order; different subscribers may see them interleaved
    // with other publishes but never reversed within their own stream.
    let event = Event {
        event_id,
        source: source.clone(),
        session_id: session_id.clone(),
        kind,
        reaction,
        payload,
        created_at: now_ms,
        pid,
    };
    broadcaster.publish(BroadcastEnvelope::Event(event));

    // Story 5.2: only publish State when `current_state` actually changed.
    // First-event semantics fall out naturally — `prev_raw_current_state` is
    // `None` for a new session, `None != Some(new_state.current_state)`
    // returns true, and the State envelope publishes.
    //
    // Compare both the stored state and the read-facing state:
    // - raw catches a delayed `Stop` after stale stored `Working` (Working→Idle)
    // - read-facing catches renewed activity after stale stored `Working`
    //   (Idle-as-read→Working)
    let state_changed = prev_raw_current_state != Some(new_state.current_state)
        || prev_read_current_state != Some(new_state.current_state);
    if state_changed {
        broadcaster.publish(BroadcastEnvelope::State {
            source,
            session_id,
            state: new_state,
        });
    }
    tracing::debug!(
        event_id = event_id.0,
        state_published = state_changed,
        "ws: published event envelope; state envelope gated on transition"
    );

    Ok(Some(event_id))
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
                    now_ms,
                    None::<u32>,
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
                    now_ms,
                    None::<u32>,
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

                // Story 5.3: SELECT now returns (kind, created_at, pid, payload)
                // so rebuild can thread last_pid carry-forward AND parse
                // notification_type from Notification payloads (the typed
                // value lives in events.payload, not on the stored Event).
                let kinds: Vec<(String, i64, Option<u32>, String)> = {
                    let mut stmt = tx.prepare(SELECT_EVENT_KINDS_FOR_SESSION)?;
                    let rows = stmt.query_map(rusqlite::params![&source, &session_id], |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, i64>(1)?,
                            row.get::<_, Option<u32>>(2)?,
                            row.get::<_, String>(3)?,
                        ))
                    })?;
                    rows.collect::<rusqlite::Result<Vec<_>>>()?
                };

                let mut state: Option<SessionState> = None;
                let mut last_created_at: i64 = 0;
                let mut bad_kind = false;
                for (kind_str, created_at, pid, payload) in kinds {
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
                    // Parse notification_type from payload only for Notification
                    // events — the same logic the adapter applies at ingest
                    // (Task 3) but here we read from stored payload at rebuild.
                    let notification_type = if matches!(kind, protocol::EventKind::Notification) {
                        serde_json::from_str::<serde_json::Value>(&payload)
                            .ok()
                            .as_ref()
                            .and_then(|v| v.get("notification_type"))
                            .and_then(|v| v.as_str())
                            .map(|s| match s {
                                "permission_prompt" => protocol::NotificationType::PermissionPrompt,
                                "idle_prompt" => protocol::NotificationType::IdlePrompt,
                                "auth_success" => protocol::NotificationType::AuthSuccess,
                                "elicitation_dialog" => {
                                    protocol::NotificationType::ElicitationDialog
                                }
                                "elicitation_response" => {
                                    protocol::NotificationType::ElicitationResponse
                                }
                                "elicitation_complete" => {
                                    protocol::NotificationType::ElicitationComplete
                                }
                                _ => protocol::NotificationType::Unknown,
                            })
                    } else {
                        None
                    };
                    // Sentinel kinds should be filtered by the source != '__daemon__' clause
                    // on SELECT_DISTINCT_SESSIONS_FROM_EVENTS, but defend against future shape
                    // changes by routing them through transition's defensive branch anyway.
                    let next = transition(state.as_ref(), kind, notification_type, pid, created_at);
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
