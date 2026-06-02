//! `/sessions` family handlers — list, detail, stats.
//!
//! All three reads use the reader pool. The daemon sentinel row
//! (`source = '__daemon__'`) is filtered out at the SQL layer, so callers
//! never see it.
//!
//! Read-time stale-Working → Idle fallback (Story 1.6's
//! `current_state_for_read`) is applied to `current_state` only — `list` and
//! `detail` both call it. The stored row is never mutated at read time.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Json, Response};
use protocol::{SessionCurrentState, SessionDetail, SessionListItem, SessionState, SessionStats};
use serde::Deserialize;
use serde_json::json;

use crate::api::filter::{parse_state_filter, state_matches};
use crate::db::queries::{build_sessions_query, SELECT_SESSION_BY_ID, SELECT_STATS_FOR_SESSION};
use crate::projection::state::current_state_for_read;
use crate::state::AppState;
use crate::time::current_unix_millis;

/// Query params for `GET /sessions` (Story 5.8, ADR 0008). All optional;
/// absent = unfiltered, preserving pre-5.8 behavior. Mirrors
/// [`crate::api::events::EventsParams`] (`deny_unknown_fields` +
/// `#[serde(default)]`), so an unknown query key is a 400 (strict-inbound
/// policy).
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionsParams {
    /// CSV of `SessionCurrentState` tokens (case-insensitive). `None` =
    /// unfiltered. Applied in Rust against the read-time `current_state`.
    #[serde(default)]
    pub state: Option<String>,
    /// Exclusive `updated_at` lower bound (epoch-ms). `0`/absent = no bound.
    #[serde(default)]
    pub since: i64,
    /// SQL row cap on the ordered, since-filtered set. `None` = no cap.
    #[serde(default)]
    pub limit: Option<i64>,
}

#[tracing::instrument(skip_all, fields(state_filter = ?params.state, since = params.since, limit = ?params.limit))]
pub async fn list(State(state): State<AppState>, Query(params): Query<SessionsParams>) -> Response {
    // Parse + validate up front; 400 on bad input (AC #4/#5/#6). Done before
    // the reader checkout so a malformed query never touches the DB.
    let state_filter: Vec<SessionCurrentState> = match &params.state {
        Some(raw) => match parse_state_filter(raw) {
            Ok(v) => v,
            Err(msg) => return bad_request(msg),
        },
        None => Vec::new(),
    };
    if params.since < 0 {
        return bad_request("since must be a non-negative epoch-ms integer".to_string());
    }
    if let Some(n) = params.limit {
        if n <= 0 {
            return bad_request("limit must be a positive integer".to_string());
        }
    }

    let conn = match state.db.reader.get().await {
        Ok(c) => c,
        Err(e) => {
            tracing::error!(error = %e, "reader pool checkout failed in /sessions");
            return internal_error();
        }
    };

    // `since`/`limit` are pushed to SQL (they shrink the scan); `?state=` is
    // applied in Rust below against the read-derived `current_state` (ADR 0008
    // — a SQL filter on the stored JSON would contradict the rendered field).
    let (sql, sql_params) = build_sessions_query(params.since, params.limit);
    let rows = conn
        .interact(
            move |c| -> rusqlite::Result<Vec<(String, String, String, i64)>> {
                let mut stmt = c.prepare(&sql)?;
                let rows = stmt.query_map(rusqlite::params_from_iter(sql_params), |r| {
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
            tracing::error!(error = %e, "sessions list query failed");
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
        // Story 5.8: `?state=` filters on the read-derived `current_state`
        // (never the stored value), so the filter matches the rendered field
        // exactly. Empty filter = match all (unfiltered default).
        if !state_matches(&state_filter, current_state) {
            continue;
        }
        items.push(SessionListItem {
            source,
            session_id,
            current_state,
            last_event_kind: stored.last_event_kind,
            last_event_at_ms: stored.last_event_at_ms,
            updated_at,
            last_pid: stored.last_pid,
            cwd: stored.cwd,
            started_at: stored.started_at,
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
        last_pid: stored.last_pid,
        cwd: stored.cwd,
        started_at: stored.started_at,
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

/// `400 Bad Request` with a `{"error": msg}` body — the strict-inbound
/// response for a malformed `GET /sessions` query param (Story 5.8).
fn bad_request(msg: String) -> Response {
    (StatusCode::BAD_REQUEST, Json(json!({ "error": msg }))).into_response()
}
