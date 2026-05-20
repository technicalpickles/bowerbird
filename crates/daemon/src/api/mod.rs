pub mod auth;
pub mod events;
pub mod health;
pub mod sessions;
pub mod status;
pub mod token;

use axum::routing::get;
use axum::Router;

use crate::state::AppState;

/// Compose the daemon's HTTP router.
///
/// `/healthz` and `/readyz` are unauthenticated (LB probes). Every other
/// route is gated by [`auth::require_bearer`].
///
/// Path-param syntax uses axum 0.8's `{id}` (not `:id`, which is axum 0.7 syntax) —
/// see Dev Notes in story 1.7 for the breaking-change reference.
pub fn router(state: AppState) -> Router {
    let unauthenticated = Router::new()
        .route("/healthz", get(health::healthz))
        .route("/readyz", get(health::readyz));

    let authenticated = Router::new()
        .route("/sessions", get(sessions::list))
        .route("/sessions/{id}", get(sessions::detail))
        .route("/sessions/{id}/events", get(events::list))
        .route("/sessions/{id}/stats", get(sessions::stats))
        .route("/status", get(status::get))
        .route_layer(axum::middleware::from_fn_with_state(
            state.clone(),
            auth::require_bearer,
        ));

    Router::new()
        .merge(unauthenticated)
        .merge(authenticated)
        .with_state(state)
}
