//! `/sessions` family handlers — list, detail, stats.
//!
//! All three reads use the reader pool. The daemon sentinel row
//! (`source = '__daemon__'`) is filtered out at the SQL layer, so callers
//! never see it.
//!
//! Read-time stale-Working → Idle fallback (Story 1.6's
//! `current_state_for_read`) is applied to `current_state` only — `list` and
//! `detail` both call it. The stored row is never mutated at read time.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Json, Response};
use protocol::{SessionCurrentState, SessionDetail, SessionListItem, SessionState, SessionStats};
use serde_json::json;

use crate::db::queries::{
    SELECT_NON_SENTINEL_SESSIONS, SELECT_SESSION_BY_ID, SELECT_STATS_FOR_SESSION,
};
use crate::projection::state::current_state_for_read;
use crate::state::AppState;
use crate::time::current_unix_millis;

#[tracing::instrument(skip_all)]
pub async fn list(State(state): State<AppState>) -> Response {
    let conn = match state.db.reader.get().await {
        Ok(c) => c,
        Err(e) => {
            tracing::error!(error = %e, "reader pool checkout failed in /sessions");
            return internal_error();
        }
    };

    let rows = conn
        .interact(
            |c| -> rusqlite::Result<Vec<(String, String, String, i64)>> {
                let mut stmt = c.prepare(SELECT_NON_SENTINEL_SESSIONS)?;
                let rows = stmt.query_map([], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, i64>(3)?,
                    ))
                })?;
                rows.collect()
            },
        )
        .await;
    let rows = match rows {
        Ok(Ok(rs)) => rs,
        Ok(Err(e)) => {
            tracing::error!(error = %e, "SELECT_NON_SENTINEL_SESSIONS failed");
            return internal_error();
        }
        Err(e) => {
            tracing::error!(error = %e, "interact failed in /sessions");
            return internal_error();
        }
    };

    let now_ms = match current_unix_millis() {
        Ok(n) => n,
        Err(e) => {
            tracing::error!(error = %e, "clock read failed in /sessions");
            return internal_error();
        }
    };

    let mut items: Vec<SessionListItem> = Vec::with_capacity(rows.len());
    for (source, session_id, state_json, updated_at) in rows {
        // Skip rows with corrupt JSON rather than 500-ing the whole list — a
        // single bad projection row shouldn't blank the entire response.
        // Mirrors `projection::session::write` defensive policy for stored
        // state deserialization.
        let stored: SessionState = match serde_json::from_str(&state_json) {
            Ok(s) => s,
            Err(e) => {
                tracing::error!(
                    error = %e,
                    source = %source,
                    session_id = %session_id,
                    "skipping session row with unparseable state JSON"
                );
                continue;
            }
        };
        let current_state = current_state_for_read(&stored, now_ms);
        items.push(SessionListItem {
            source,
            session_id,
            current_state,
            last_event_kind: stored.last_event_kind,
            last_event_at_ms: stored.last_event_at_ms,
            updated_at,
        });
    }

    Json(items).into_response()
}

#[tracing::instrument(skip_all, fields(session_id = %id))]
pub async fn detail(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    let conn = match state.db.reader.get().await {
        Ok(c) => c,
        Err(e) => {
            tracing::error!(error = %e, "reader pool checkout failed in /sessions/{{id}}");
            return internal_error();
        }
    };

    let lookup = conn
        .interact(
            move |c| -> rusqlite::Result<(String, String, String, i64)> {
                c.query_row(SELECT_SESSION_BY_ID, [&id], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, i64>(3)?,
                    ))
                })
            },
        )
        .await;
    let (source, session_id, state_json, updated_at) = match lookup {
        Ok(Ok(row)) => row,
        Ok(Err(rusqlite::Error::QueryReturnedNoRows)) => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({ "error": "session not found" })),
            )
                .into_response();
        }
        Ok(Err(e)) => {
            tracing::error!(error = %e, "SELECT_SESSION_BY_ID failed");
            return internal_error();
        }
        Err(e) => {
            tracing::error!(error = %e, "interact failed in /sessions/{{id}}");
            return internal_error();
        }
    };

    let stored: SessionState = match serde_json::from_str(&state_json) {
        Ok(s) => s,
        Err(e) => {
            // Detail differs from list: the user asked for *this* row. Returning
            // a partial 200 here would lie. 500 surfaces the bug instead.
            tracing::error!(error = %e, "session_projections.state JSON parse failed in /sessions/{{id}}");
            return internal_error();
        }
    };

    let now_ms = match current_unix_millis() {
        Ok(n) => n,
        Err(e) => {
            tracing::error!(error = %e, "clock read failed in /sessions/{{id}}");
            return internal_error();
        }
    };
    let current_state: SessionCurrentState = current_state_for_read(&stored, now_ms);

    // Derived state carries the read-time current_state but keeps the stored
    // last_event_* fields verbatim. See Dev Notes "Read-time stale fallback
    // wiring (Story 1.6 callback)."
    let derived_state = SessionState {
        current_state,
        last_event_kind: stored.last_event_kind,
        last_event_at_ms: stored.last_event_at_ms,
    };

    Json(SessionDetail {
        source,
        session_id,
        state: derived_state,
        updated_at,
    })
    .into_response()
}

#[tracing::instrument(skip_all, fields(session_id = %id))]
pub async fn stats(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    let conn = match state.db.reader.get().await {
        Ok(c) => c,
        Err(e) => {
            tracing::error!(error = %e, "reader pool checkout failed in /sessions/{{id}}/stats");
            return internal_error();
        }
    };

    let lookup = conn
        .interact({
            let id = id.clone();
            move |c| -> rusqlite::Result<(String, i64, Option<i64>, Option<i64>)> {
                c.query_row(SELECT_STATS_FOR_SESSION, [&id], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, i64>(1)?,
                        r.get::<_, Option<i64>>(2)?,
                        r.get::<_, Option<i64>>(3)?,
                    ))
                })
            }
        })
        .await;
    match lookup {
        Ok(Ok((source, event_count, first_event_at, last_event_at))) => Json(SessionStats {
            source,
            session_id: id,
            event_count,
            first_event_at,
            last_event_at,
        })
        .into_response(),
        Ok(Err(rusqlite::Error::QueryReturnedNoRows)) => (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "session not found" })),
        )
            .into_response(),
        Ok(Err(e)) => {
            tracing::error!(error = %e, "SELECT_STATS_FOR_SESSION failed");
            internal_error()
        }
        Err(e) => {
            tracing::error!(error = %e, "interact failed in /sessions/{{id}}/stats");
            internal_error()
        }
    }
}

fn internal_error() -> Response {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(json!({ "error": "internal error" })),
    )
        .into_response()
}
