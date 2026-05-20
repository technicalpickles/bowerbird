//! Bearer-token authentication middleware.
//!
//! The 401 body is `{"error":"unauthorized"}` regardless of why the request
//! was rejected — missing header, wrong scheme, empty token, wrong-length
//! token, wrong bytes. Distinct error messages leak information about the
//! auth surface (e.g. "header parsed but token rejected" tells an attacker
//! something). Same status, same body, same approximate timing (the
//! constant-time compare keeps the wrong-bytes branch isochronous).
//!
//! The candidate token is never logged. `tracing::instrument(skip_all)`
//! plus `secrecy::SecretString` redaction inside `BearerToken` keep the
//! token out of spans and `Debug` output even on panic paths.

use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{header, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Json, Response};
use serde_json::json;

use crate::state::AppState;

const UNAUTHORIZED: (StatusCode, &str) = (StatusCode::UNAUTHORIZED, "unauthorized");

#[tracing::instrument(skip_all)]
pub async fn require_bearer(
    State(state): State<AppState>,
    req: Request<Body>,
    next: Next,
) -> Response {
    let header = req.headers().get(header::AUTHORIZATION);
    let candidate = match header.and_then(|h| h.to_str().ok()) {
        Some(s) => s.strip_prefix("Bearer ").map(str::trim),
        None => None,
    };
    let authorized = match candidate {
        Some(c) if !c.is_empty() => state.bearer.verify(c),
        _ => false,
    };
    if authorized {
        next.run(req).await
    } else {
        let (status, body) = UNAUTHORIZED;
        (status, Json(json!({ "error": body }))).into_response()
    }
}
