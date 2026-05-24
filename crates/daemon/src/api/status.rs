//! `GET /status` — daemon snapshot (version, uptime, last event).
//!
//! `connected_ws_clients` is deferred to Story 3.2 alongside the
//! `bowerbird status` CLI (its first V1 consumer). Epic 2 shipped the
//! semaphore that produces the count (`AppState::ws_semaphore`); only the
//! `DaemonStatus` surfacing was deferred.

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Json, Response};
use protocol::{DaemonStatus, EventId};
use serde_json::json;

use crate::db::queries::SELECT_LAST_EVENT;
use crate::state::AppState;
use crate::time::current_unix_millis;

const PROTOCOL_VERSION: &str = "1.0";

#[tracing::instrument(skip_all)]
pub async fn get(State(state): State<AppState>) -> Response {
    let conn = match state.db.reader.get().await {
        Ok(c) => c,
        Err(e) => {
            tracing::error!(error = %e, "reader pool checkout failed in /status");
            return internal_error();
        }
    };

    let last_event = conn
        .interact(|c| -> rusqlite::Result<Option<(i64, i64)>> {
            // query_row returns QueryReturnedNoRows for an empty events table.
            // Use the optional helper so "no rows" maps to `Ok(None)`.
            c.query_row(SELECT_LAST_EVENT, [], |r| {
                Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?))
            })
            .map(Some)
            .or_else(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => Ok(None),
                other => Err(other),
            })
        })
        .await;
    let last_event = match last_event {
        Ok(Ok(opt)) => opt,
        Ok(Err(e)) => {
            tracing::error!(error = %e, "SELECT_LAST_EVENT failed");
            return internal_error();
        }
        Err(e) => {
            tracing::error!(error = %e, "interact failed in /status");
            return internal_error();
        }
    };

    let now_ms = match current_unix_millis() {
        Ok(n) => n,
        Err(e) => {
            tracing::error!(error = %e, "clock read failed in /status");
            return internal_error();
        }
    };

    let (last_event_id, last_event_at_ms) = match last_event {
        Some((id, at_ms)) => (Some(EventId(id)), Some(at_ms)),
        None => (None, None),
    };

    Json(DaemonStatus {
        daemon_version: env!("CARGO_PKG_VERSION").to_string(),
        protocol_version: PROTOCOL_VERSION.to_string(),
        started_at_ms: state.started_at_ms,
        uptime_ms: now_ms.saturating_sub(state.started_at_ms),
        last_event_at_ms,
        last_event_id,
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
