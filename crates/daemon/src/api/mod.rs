pub mod auth;
pub mod events;
pub mod health;
pub mod replay;
pub mod sessions;
pub mod status;
pub mod token;
pub mod ws;

use std::time::Duration;

use axum::extract::Request;
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::{IntoResponse, Json, Response};
use axum::routing::{get, post};
use axum::Router;
use serde_json::json;
use tower::ServiceBuilder;
use tower_http::limit::RequestBodyLimitLayer;
use tower_http::request_id::{MakeRequestUuid, PropagateRequestIdLayer, SetRequestIdLayer};
use tower_http::trace::TraceLayer;

use crate::state::AppState;

/// Default per-request body cap. Well above any v1 GET response; cheap
/// insurance against accidental POST/PUT explosions in a future story.
const BODY_LIMIT_BYTES: usize = 1_048_576; // 1 MiB

/// Per-HTTP-request wall-clock cap. Story 2.1 AC #10. WS routes are
/// intentionally NOT layered with this — long-lived WebSocket connections
/// must not be terminated at 30s.
const HTTP_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// Wall-clock timeout middleware. Using `axum::middleware::from_fn` rather
/// than `tower_http::timeout::TimeoutLayer` because the latter requires
/// `ResBody: Default` and is incompatible with wrapping outside
/// `RequestBodyLimitLayer` (which wraps the response body). Doing this in a
/// custom middleware lets us put timeout BEFORE body-limit in the request
/// flow — the order AC #10 specifies.
async fn timeout_middleware(req: Request, next: Next) -> Response {
    match tokio::time::timeout(HTTP_REQUEST_TIMEOUT, next.run(req)).await {
        Ok(resp) => resp,
        Err(_) => (
            StatusCode::REQUEST_TIMEOUT,
            Json(json!({ "error": "request timeout" })),
        )
            .into_response(),
    }
}

/// Span maker for `TraceLayer` that records only the method and path,
/// deliberately omitting the URI query string. The `?token=...` auth
/// fallback for `/ws` (AC #5) means a default `request.uri()` span would
/// log the bearer token. Method + path is enough to correlate requests
/// across logs without leaking secrets.
#[derive(Clone)]
struct RedactedSpan;

impl<B> tower_http::trace::MakeSpan<B> for RedactedSpan {
    fn make_span(&mut self, request: &axum::http::Request<B>) -> tracing::Span {
        tracing::info_span!(
            "http_request",
            method = %request.method(),
            path = %request.uri().path(),
        )
    }
}

/// Compose the daemon's HTTP router.
///
/// Routing surface:
///
/// - `/healthz`, `/readyz` — unauthenticated (LB probes).
/// - `/sessions/{...}`, `/status` — REST surface gated by
///   [`auth::require_bearer`].
/// - `/ws` — WebSocket upgrade; auth is hand-rolled in
///   [`ws::handle_upgrade`] because the bearer can arrive on EITHER the
///   `Authorization` header OR a `?token=` query parameter, and the existing
///   `require_bearer` middleware only consults the header.
///
/// Middleware stack (Story 2.1 AC #10). Request-direction order:
///
/// 1. `SetRequestIdLayer` + `PropagateRequestIdLayer` — every response
///    carries an `x-request-id` UUID4 header for cross-cut tracing.
/// 2. `TraceLayer::new_for_http().make_span_with(RedactedSpan)` — emits
///    request spans that record method + path only. The default
///    `DefaultMakeSpan` would record the URI including `?token=...`, which
///    would leak the WS-fallback bearer token into tracing output.
/// 3. `timeout_middleware` — 30s wall-clock; HTTP only, WS exempt.
/// 4. `RequestBodyLimitLayer::new(1 MiB)` — HTTP only, WS exempt.
/// 5. `require_bearer` — route-layered on the authenticated REST subset.
///
/// Path-param syntax uses axum 0.8's `{id}` (not `:id`, which is axum 0.7
/// syntax). The `/ws` route has no path params — flagging this so a future
/// `/ws/v2`-style addition follows the same convention.
pub fn router(state: AppState) -> Router {
    let unauthenticated = Router::new()
        .route("/healthz", get(health::healthz))
        .route("/readyz", get(health::readyz));

    let authenticated = Router::new()
        .route("/replay", post(replay::run))
        .route("/sessions", get(sessions::list))
        .route("/sessions/{id}", get(sessions::detail))
        .route("/sessions/{id}/events", get(events::list))
        .route("/sessions/{id}/stats", get(sessions::stats))
        .route("/status", get(status::get))
        .route_layer(axum::middleware::from_fn_with_state(
            state.clone(),
            auth::require_bearer,
        ));

    let ws_only = Router::new().route("/ws", get(ws::handle_upgrade));

    // HTTP-only stack: timeout (outer) wraps body-limit (inner), so the
    // request flow is timeout → body-limit → handler. This matches AC #10's
    // ordering of `TimeoutLayer` (step 3) before `RequestBodyLimitLayer`
    // (step 4). Custom `timeout_middleware` is used (not
    // `tower_http::timeout::TimeoutLayer`) because the latter would need
    // `ResBody: Default` and that fails when `RequestBodyLimit` wraps the
    // response body.
    let http_routes = Router::new()
        .merge(unauthenticated)
        .merge(authenticated)
        .layer(RequestBodyLimitLayer::new(BODY_LIMIT_BYTES))
        .layer(axum::middleware::from_fn(timeout_middleware));

    // Cross-cut middleware applied to ALL routes including /ws — the
    // upgrade itself is an HTTP request, so request-id and trace spans
    // remain useful for it.
    let common_stack = ServiceBuilder::new()
        .layer(SetRequestIdLayer::new(
            "x-request-id".parse().expect("static header name"),
            MakeRequestUuid,
        ))
        .layer(PropagateRequestIdLayer::new(
            "x-request-id".parse().expect("static header name"),
        ))
        .layer(TraceLayer::new_for_http().make_span_with(RedactedSpan));

    Router::new()
        .merge(http_routes)
        .merge(ws_only)
        .layer(common_stack)
        .with_state(state)
}
