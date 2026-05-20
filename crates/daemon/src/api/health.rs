use std::sync::atomic::Ordering;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Json};
use deadpool_sqlite::Pool;
use serde_json::json;

use crate::db::queries::PROBE_DB_READY;
use crate::state::AppState;

pub async fn healthz() -> impl IntoResponse {
    (StatusCode::OK, Json(json!({ "status": "ok" })))
}

/// `/readyz` is a hybrid `migrations_complete && db_probe_ok` check.
///
/// The migrations branch runs first so the existing drain-on-shutdown invariant
/// (`main::run` flips `migrations_complete.store(false)`) keeps causing 503s
/// even with the new probe — drain takes priority. Only if migrations are
/// complete do we hit the DB.
pub async fn readyz(State(state): State<AppState>) -> impl IntoResponse {
    if !state.migrations_complete.load(Ordering::Acquire) {
        return not_ready();
    }
    match probe_db(&state.db.reader).await {
        Ok(()) => (StatusCode::OK, Json(json!({ "status": "ready" }))).into_response(),
        Err(()) => not_ready(),
    }
}

fn not_ready() -> axum::response::Response {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(json!({ "error": "not ready" })),
    )
        .into_response()
}

/// Probe the reader pool with [`PROBE_DB_READY`].
///
/// `QueryReturnedNoRows` is the success path (the query returns no rows by
/// design); only a real sqlite or pool error returns `Err`. Validates that
/// pool checkout succeeds within the 5s wait timeout, the connection is
/// alive, and the `events` table exists.
async fn probe_db(reader: &Pool) -> Result<(), ()> {
    let conn = reader.get().await.map_err(|e| {
        tracing::warn!(error = %e, "readyz: reader pool checkout failed");
    })?;
    let interact = conn
        .interact(|c| -> rusqlite::Result<()> {
            match c.query_row::<i64, _, _>(PROBE_DB_READY, [], |r| r.get(0)) {
                Ok(_) => Ok(()),
                Err(rusqlite::Error::QueryReturnedNoRows) => Ok(()),
                Err(other) => Err(other),
            }
        })
        .await;
    match interact {
        Ok(Ok(())) => Ok(()),
        Ok(Err(e)) => {
            tracing::warn!(error = %e, "readyz: DB probe failed");
            Err(())
        }
        Err(e) => {
            tracing::warn!(error = %e, "readyz: interact failed");
            Err(())
        }
    }
}
