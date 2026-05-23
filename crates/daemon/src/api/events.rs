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
    SELECT_MIN_EVENT_ID,
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

    type EventRow = (i64, String, String, String, Option<String>, String, i64);
    let interact = conn
        .interact({
            let id_for_select = id.clone();
            let since = params.since;
            move |c| -> rusqlite::Result<(Vec<EventRow>, Option<i64>)> {
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
                        ))
                    })?;
                    mapped.collect::<rusqlite::Result<Vec<_>>>()?
                };
                let min: Option<i64> = c.query_row(SELECT_MIN_EVENT_ID, [], |r| r.get(0))?;
                Ok((rows, min))
            }
        })
        .await;
    let (raw_rows, min_event_id) = match interact {
        Ok(Ok(t)) => t,
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
    for (event_id, source, session_id, kind_str, reaction_str, payload, created_at) in raw_rows {
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
