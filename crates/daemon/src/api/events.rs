//! `GET /sessions/{id}/events?since=<event_id>` history endpoint.
//!
//! Cursor semantics: `cursor = Some(events.last().event_id)` when events is
//! non-empty (the next `?since=` to tail), `None` when empty. `oldest_available_event_id`
//! is the global MIN(event_id) across non-sentinel rows, or `EventId(i64::MAX)`
//! when the table is empty.
//!
//! Page-size limit is intentionally absent in V1 — see `deferred-work.md`.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Json, Response};
use protocol::{Event, EventId, EventListResponse, Reaction};
use serde::Deserialize;
use serde_json::json;

use crate::db::queries::{
    event_kind_from_db_str, reaction_from_db_string, SELECT_EVENTS_FOR_SESSION_SINCE,
    SELECT_MIN_EVENT_ID, SELECT_SESSION_EXISTS_BY_ID,
};
use crate::state::AppState;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EventsParams {
    #[serde(default)]
    pub since: i64,
}

#[tracing::instrument(skip_all, fields(session_id = %id, since = params.since))]
pub async fn list(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(params): Query<EventsParams>,
) -> Response {
    let conn = match state.db.reader.get().await {
        Ok(c) => c,
        Err(e) => {
            tracing::error!(error = %e, "reader pool checkout failed in /sessions/{{id}}/events");
            return internal_error();
        }
    };

    type EventRow = (
        i64,
        String,
        String,
        String,
        Option<String>,
        String,
        i64,
        Option<u32>,
    );
    // Story 5.4 AC #5 — existence probe runs inside the same `interact`
    // closure as the events SELECT so both reads see the same SQLite
    // snapshot. A `QueryReturnedNoRows` here means the session_id has never
    // been observed (no `session_projections` row) and we return 404 — same
    // shape as `/sessions/{id}` and `/sessions/{id}/stats`. The previous
    // behavior (`200 {events:[], cursor: null, oldest_available_event_id:
    // i64::MAX}`) silently masked typos on `?since=0` calls.
    enum InteractResult {
        Found {
            rows: Vec<EventRow>,
            min: Option<i64>,
        },
        SessionNotFound,
    }
    let interact = conn
        .interact({
            let id_for_select = id.clone();
            let since = params.since;
            move |c| -> rusqlite::Result<InteractResult> {
                let exists = c.query_row(SELECT_SESSION_EXISTS_BY_ID, [&id_for_select], |_| Ok(()));
                if matches!(exists, Err(rusqlite::Error::QueryReturnedNoRows)) {
                    return Ok(InteractResult::SessionNotFound);
                }
                exists?;
                let rows: Vec<EventRow> = {
                    let mut stmt = c.prepare(SELECT_EVENTS_FOR_SESSION_SINCE)?;
                    let mapped = stmt.query_map(rusqlite::params![id_for_select, since], |r| {
                        Ok((
                            r.get::<_, i64>(0)?,
                            r.get::<_, String>(1)?,
                            r.get::<_, String>(2)?,
                            r.get::<_, String>(3)?,
                            r.get::<_, Option<String>>(4)?,
                            r.get::<_, String>(5)?,
                            r.get::<_, i64>(6)?,
                            r.get::<_, Option<u32>>(7)?,
                        ))
                    })?;
                    mapped.collect::<rusqlite::Result<Vec<_>>>()?
                };
                let min: Option<i64> = c.query_row(SELECT_MIN_EVENT_ID, [], |r| r.get(0))?;
                Ok(InteractResult::Found { rows, min })
            }
        })
        .await;
    let (raw_rows, min_event_id) = match interact {
        Ok(Ok(InteractResult::Found { rows, min })) => (rows, min),
        Ok(Ok(InteractResult::SessionNotFound)) => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({ "error": "session not found" })),
            )
                .into_response();
        }
        Ok(Err(e)) => {
            tracing::error!(error = %e, "events query failed in /sessions/{{id}}/events");
            return internal_error();
        }
        Err(e) => {
            tracing::error!(error = %e, "interact failed in /sessions/{{id}}/events");
            return internal_error();
        }
    };

    let mut events: Vec<Event> = Vec::with_capacity(raw_rows.len());
    for (event_id, source, session_id, kind_str, reaction_str, payload, created_at, pid) in raw_rows
    {
        let kind = match event_kind_from_db_str(&kind_str) {
            Ok(k) => k,
            Err(msg) => {
                tracing::error!(
                    error = %msg,
                    event_id,
                    "skipping event row with unparseable kind — schema drift"
                );
                continue;
            }
        };
        let reaction: Option<Reaction> = match reaction_str.as_deref() {
            None => None,
            Some(s) => match reaction_from_db_string(s) {
                Ok(r) => Some(r),
                Err(msg) => {
                    tracing::error!(
                        error = %msg,
                        event_id,
                        "skipping event row with unparseable reaction — schema drift"
                    );
                    continue;
                }
            },
        };
        events.push(Event {
            event_id: EventId(event_id),
            source,
            session_id,
            kind,
            reaction,
            payload,
            created_at,
            pid,
        });
    }

    let cursor = events.last().map(|e| e.event_id);
    // `MIN(event_id)` is NULL when the events table contains no non-sentinel
    // rows. The protocol contract surfaces `EventId(i64::MAX)` in that case
    // (architecture.md:427) so presenters mechanically infer "no gap possible
    // since no events exist."
    let oldest_available_event_id = EventId(min_event_id.unwrap_or(i64::MAX));

    Json(EventListResponse {
        events,
        cursor,
        oldest_available_event_id,
    })
    .into_response()
}

fn internal_error() -> Response {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(json!({ "error": "internal error" })),
    )
        .into_response()
}
