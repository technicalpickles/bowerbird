use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Json};
use axum::routing::get;
use axum::Router;
use serde_json::{json, Value};

use crate::db::queries::READYZ_PROBE;
use crate::state::SharedState;

pub async fn healthz() -> Json<Value> {
    Json(json!({"status": "ok"}))
}

pub async fn readyz(State(state): State<SharedState>) -> impl IntoResponse {
    match state.db.readers.get().await {
        Ok(conn) => {
            let probe = conn
                .interact(|c| -> rusqlite::Result<()> {
                    let mut stmt = c.prepare(READYZ_PROBE)?;
                    let _ = stmt.query([])?;
                    Ok(())
                })
                .await;
            match probe {
                Ok(Ok(())) => (StatusCode::OK, Json(json!({"status": "ready"}))).into_response(),
                Ok(Err(e)) => (
                    StatusCode::SERVICE_UNAVAILABLE,
                    Json(json!({"error": e.to_string()})),
                )
                    .into_response(),
                Err(e) => (
                    StatusCode::SERVICE_UNAVAILABLE,
                    Json(json!({"error": e.to_string()})),
                )
                    .into_response(),
            }
        }
        Err(e) => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({"error": e.to_string()})),
        )
            .into_response(),
    }
}

pub fn router(state: SharedState) -> Router {
    Router::new()
        .route("/healthz", get(healthz))
        .route("/readyz", get(readyz))
        .with_state(state)
}
