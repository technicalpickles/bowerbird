# Story 2.1: WebSocket connection and topic subscription

Status: ready-for-dev

<!-- Note: Validation is optional. Run validate-create-story for quality check before dev-story. -->

## Story

As a tool builder,
I want to establish an authenticated WebSocket connection to bowerbird, declare which event topics I want to receive, and have my socket stay healthy across idle periods,
so that I can build long-running presenter tools that only receive the agent activity relevant to them without filtering it myself.

This is the first story in Epic 2 (live event streaming). It builds the WebSocket *surface* — connect, auth, hello frame, subscribe/unsubscribe filtering, per-client ping/pong, concurrency cap, broadcaster scaffolding — without yet publishing live events into the broadcaster (Story 2.2 wires `projection::session::write` to publish; Story 2.3 adds snapshot-on-subscribe; Story 2.4 adds `DroppedFrame`; Story 2.5 adds `CloseFrame` on shutdown).

Story 2.1 also lands the deferred-work middleware items the Epic 1 retrospective (`epic-1-retro-2026-05-20.md` § "Action items for Epic 2") explicitly folded into this story under "Option C": **request-id**, **TraceLayer**, **TimeoutLayer (30s, ws-exempt)**, **RequestBodyLimitLayer**, and `ClientMessage` **empty-topic rejection**. These are ACs of this story, not "consider" items.

## Acceptance Criteria

1. **Given** a tool connects to `ws://127.0.0.1:<port>/ws` and presents a valid bearer token in the `Authorization: Bearer <token>` header **OR** in the `?token=<token>` query parameter **When** the WebSocket upgrade completes **Then** the daemon sends a `hello` frame as a single text message immediately (before any other frame), the JSON body has `op: "hello"` and a `protocol_version` field equal to `"1.0"` and a `daemon_version` field equal to `env!("CARGO_PKG_VERSION")` — both fields populated, the additional `HelloFrame` fields (`oldest_available_event_id`, `daemon_started_at`, `history_begins_cleanly`) are populated using the same sources as the REST `/status` and `/sessions/{id}/events` endpoints (Story 1.7) so a single client sees a consistent snapshot across the WS hello and any same-startup REST call.

2. **Given** an authenticated, post-Hello WebSocket session **When** a client sends `{"op":"subscribe","topic":"state.session.*"}` followed later by `{"op":"subscribe","topic":"events.*"}` **Then** the daemon stores BOTH topics in the per-connection subscription set (the second Subscribe is additive, not replacing); when subsequent server frames are dispatched by the broadcast hub, only frames whose topic matches at least one declared topic are sent to that connection. **For Story 2.1**: the broadcast hub exists (Task 3) but no event publishers wire into it yet — so the *filtering* must be verifiable via a unit-level test that exercises the per-connection topic-match predicate against synthetic broadcast envelopes. Story 2.2 will exercise the full end-to-end filtering when it wires `projection::session::write` to publish.

3. **Given** an authenticated, post-Hello session **When** a client sends `{"op":"unsubscribe","topic":"state.session.*"}` **Then** that topic is removed from the per-connection subscription set; subsequent broadcast envelopes matching that topic are no longer delivered to the connection; the unsubscribe-of-a-not-currently-subscribed topic is a no-op (no error frame, no close).

4. **Given** any inbound message on the WS that does not deserialize as a `ClientMessage` per `crates/protocol/src/ws.rs` (`deny_unknown_fields` strict-inbound contract) — including an empty-string `topic` (`{"op":"subscribe","topic":""}`), an unknown `op`, an extra unknown field, a non-JSON payload, or any other malformed input **When** the daemon parses it **Then** the daemon closes the WebSocket with **close code 1008 (Policy Violation)** and a reason string starting with `"bad message:"` followed by a `sanitize_for_wire`-sanitized error excerpt (`\n` / `\r` stripped, capped at 123 chars — the WebSocket reason-string total limit is 123 bytes per RFC 6455 §5.5.1, account for the prefix). The connection is closed; no further frames are sent. This single AC covers the deferred-work entry "ClientMessage empty topic accepted" (`deferred-work.md` line 10) folded in via Epic 1 retro Action Item.

5. **Given** a tool attempts the WebSocket upgrade with no `Authorization` header **AND** no `?token=` query param **OR** with a header/param whose token does not match the daemon's active token (constant-time compare via `BearerToken::verify`) **When** the upgrade handshake runs **Then** the daemon returns **HTTP `401 {"error":"unauthorized"}`** and the WebSocket upgrade does NOT complete — the client receives a normal HTTP response, not a successful upgrade followed by an immediate close. Header-versus-query precedence: if both are present, the header wins (consistent with HTTP convention; `?token=` exists for clients that cannot set headers, e.g. browser `new WebSocket()` constructor). The 401 body shape and `tracing::instrument(skip_all)` discipline are identical to the existing `api::auth::require_bearer` middleware so a single 401 contract test asserts both surfaces.

6. **Given** the WS concurrent-connection cap is set to **256** (the architecture default, declared in `Config` as `ws_max_connections: usize` with default `256`) **When** a 257th WebSocket upgrade arrives while 256 connections are already established **Then** the daemon returns **HTTP `503 {"error":"too many ws clients"}`** (without completing the WebSocket upgrade) and the 256 existing connections are unaffected. The cap is enforced by a `tokio::sync::Semaphore` whose permit is acquired BEFORE the upgrade completes and held by the per-connection task for the duration of the connection. A test must exercise the boundary: 256 connections succeed, the 257th fails with 503, then closing one of the 256 allows a fresh upgrade to succeed (semaphore permit returned on drop).

7. **Given** an authenticated WebSocket session that exchanges no application frames for 30 seconds **When** the per-connection ping timer fires **Then** the daemon sends a WebSocket Ping control frame; when the client responds with a Pong, the connection remains open and the next ping timer arms for another 30 seconds. The ping interval is `Config::ws_ping_interval: Duration` defaulting to `Duration::from_secs(30)`. Pings are emitted by axum's built-in `WebSocket::send(Message::Ping(_))`; Pongs are auto-handled by axum's WS framing (the next `socket.recv()` for application code sees the Ping/Pong already consumed unless the client closes mid-handshake).

8. **Given** an authenticated WebSocket session whose underlying TCP connection has been dropped without a TCP FIN (e.g. abrupt network loss simulated by `socket.into_inner().shutdown()` on the test client OR a closed `tokio::net::TcpStream` half) **When** the daemon sends a Ping and no Pong arrives within `Config::ws_pong_timeout: Duration` (default `Duration::from_secs(10)`) **Then** the per-connection task exits, the semaphore permit is released, the subscription state is dropped, and no task is leaked (asserted by `tokio::task::JoinHandle::await` returning `Ok(())` in the test). The cap-restoration test from AC #6 indirectly proves the permit-release path.

9. **Given** SIGTERM is received by the daemon **When** the graceful shutdown sequence runs **Then** the WS surface stops accepting new upgrades (returns 503 or refuses connect; documented behavior either way) and existing per-connection tasks observe `state.shutdown.cancelled()` and exit cleanly. **For Story 2.1**: a `CloseFrame` is NOT yet sent on shutdown — that is Story 2.5's contract. Story 2.1 only requires that no task is leaked and the existing axum graceful-shutdown path in `crates/daemon/src/main.rs` continues to exit with code 0.

10. **Given** the daemon's HTTP surface is composed in `crates/daemon/src/api/mod.rs::router` **When** a non-WS REST request is processed **Then** the request flows through, in order: (1) `tower_http::request_id::SetRequestIdLayer` (generates a UUID4 per request, sets `x-request-id` header), (2) `tower_http::trace::TraceLayer::new_for_http()` (emits a tracing span per request with the request-id field), (3) `tower_http::timeout::TimeoutLayer::new(Duration::from_secs(30))` (30s wall-clock limit per `project-context.md`'s required-middleware list), (4) `tower_http::limit::RequestBodyLimitLayer::new(BODY_LIMIT_BYTES)` where `BODY_LIMIT_BYTES = 1_048_576` (1 MiB; documented in code as "well above any v1 GET response; cheap insurance against accidental POST/PUT explosions that might land in a future story"), and finally (5) `auth::require_bearer` for authenticated routes. **WS routes are exempt from the 30s TimeoutLayer** (long-lived connections must not be terminated at 30s); see Dev Notes "WS-vs-HTTP middleware split" for the structural pattern. **A single contract test asserts that `GET /healthz` returns an `x-request-id` header** populated with a 36-character UUID4-shaped string — this is the canary that the request-id middleware is wired correctly.

## Tasks / Subtasks

- [ ] **Task 1: Wire axum's WebSocket feature + tower-http middleware features** (AC: #1, #10)
  - [ ] In `Cargo.toml` workspace deps, change `axum = "0.8.9"` to `axum = { version = "0.8.9", features = ["ws"] }`. Crate-level Cargo.toml in `crates/daemon/Cargo.toml` uses `axum = { workspace = true }` so no per-crate change is needed there. **Do NOT add `tokio-tungstenite` as a direct dep** — axum 0.8's `ws` feature already wraps `tokio-tungstenite-0.27`, and that's the only WS lib we touch. (Retro § "Standards-by-default" / Agreement A1: axum is the standard; tokio-tungstenite is a private transitive dep.)
  - [ ] In `Cargo.toml` workspace deps, change `tower-http = "0.6.10"` to `tower-http = { version = "0.6.10", features = ["request-id", "trace", "timeout", "limit", "util"] }`. The `util` feature gives `MapRequestLayer`/`MapResponseLayer` if needed; safe default to include. Daemon `Cargo.toml` continues to use `tower-http = { workspace = true }`.
  - [ ] Add `tokio-tungstenite` to **dev-dependencies** (workspace + daemon) for the contract tests' client side. Pin `tokio-tungstenite = "0.27"` to match what axum 0.8.9 wraps so message-type compatibility is automatic. Rationale: there is no first-class `axum::test_helpers::ws` client; tests need a real WS client to exercise the surface. `tokio-tungstenite` is the canonical Rust WS client. Do NOT also add `tungstenite` (sync) — we only need the async client.
  - [ ] Run `cargo check --workspace` after the dep changes to confirm clean compile before touching any code.

- [ ] **Task 2: Add WS-related fields to `Config` and `AppState`** (AC: #1, #6, #7, #8, #9)
  - [ ] Modify `crates/daemon/src/config.rs::Config` to add four new fields (Cargo.toml comments not required; document each in code comments):
    ```rust
    pub struct Config {
        pub db_path: PathBuf,
        pub bind_addr: SocketAddr,
        pub ingest_channel_capacity: usize,
        pub ingest_sock_path: PathBuf,
        pub tool_reactions_path: PathBuf,
        // NEW for Story 2.1:
        pub ws_max_connections: usize,        // default 256
        pub ws_ping_interval: Duration,       // default 30s
        pub ws_pong_timeout: Duration,        // default 10s
        pub ws_broadcast_capacity: usize,     // default 1024 (event-channel capacity per topic class; tuned by 2.4)
    }
    ```
    Defaults in `Config::with_bowerbird_dir`: `ws_max_connections: 256`, `ws_ping_interval: Duration::from_secs(30)`, `ws_pong_timeout: Duration::from_secs(10)`, `ws_broadcast_capacity: 1024`.
  - [ ] Add `use std::time::Duration;` at the top of `config.rs`.
  - [ ] Modify `crates/daemon/src/state.rs::AppState` to add three new fields:
    ```rust
    pub struct AppState {
        pub db: DbPools,
        pub migrations_complete: Arc<AtomicBool>,
        pub shutdown: CancellationToken,
        pub bearer: BearerToken,
        pub started_at_ms: i64,
        // NEW for Story 2.1:
        pub broadcaster: Arc<crate::broadcast::BroadcastHub>,
        pub ws_semaphore: Arc<tokio::sync::Semaphore>,
        pub ws_config: WsConfig,
    }

    #[derive(Debug, Clone, Copy)]
    pub struct WsConfig {
        pub ping_interval: Duration,
        pub pong_timeout: Duration,
    }
    ```
    `WsConfig` is a small `Copy` struct so per-connection tasks don't have to clone the whole `AppState` to read these. `BroadcastHub` is `Arc`-shared. `Semaphore` is `Arc`-shared. Both are constructed once at daemon startup and live for the daemon's lifetime.
  - [ ] Update every existing `AppState { ... }` construction site:
    - `crates/daemon/src/main.rs::run` (one site) — constructs the hub via `Arc::new(BroadcastHub::new(config.ws_broadcast_capacity))`, semaphore via `Arc::new(tokio::sync::Semaphore::new(config.ws_max_connections))`, `WsConfig` from `config.ws_ping_interval`/`pong_timeout`.
    - `crates/daemon/tests/contract_daemon.rs::make_test_state` — extend to construct test-friendly defaults (broadcaster with capacity 16, semaphore with permits 4 for cap-edge tests, ping interval 100ms / pong timeout 50ms for fast tests). Add overloads or builder-style helpers if multiple tests need different timings: prefer a single helper `make_test_state_with_ws(pools, migrations_complete, ws_max_conns, ping_interval, pong_timeout)` that the original `make_test_state` calls with defaults.

- [ ] **Task 3: Create `crates/daemon/src/broadcast/` module (scaffolding for 2.2–2.5)** (AC: #2, #3)
  - [ ] Create `crates/daemon/src/broadcast/mod.rs`:
    ```rust
    pub mod event;
    pub mod hub;
    pub use event::{BroadcastEnvelope, Topic};
    pub use hub::BroadcastHub;
    ```
  - [ ] Create `crates/daemon/src/broadcast/event.rs` defining the **internal** (not wire) broadcast envelope. This is what the broadcast channel carries; per-connection tasks transform it into a wire `ServerMessage` before sending.
    ```rust
    use protocol::{Event, EventId, ServerMessage, SessionState};

    /// The internal value carried on `tokio::sync::broadcast` channels.
    /// One variant per topic class. The per-connection task projects this
    /// into a wire `ServerMessage` after the topic-match check.
    #[derive(Debug, Clone)]
    pub enum BroadcastEnvelope {
        /// A new event was committed to the events table.
        /// Topic: `events.<source>.<session_id>` (matches both `events.*`
        /// and `events.<source>.*` and `events.<source>.<session_id>`).
        Event(Event),
        /// A session's projection was updated.
        /// Topic: `state.session.<session_id>`
        /// Multiple `state.session.<id>.<field>` sub-topics may match the
        /// same envelope — see `Topic::matches`.
        State {
            source: String,
            session_id: String,
            state: SessionState,
        },
        // Story 2.4 will add: Dropped { count, first_id, last_id, recipient_token }
        // Story 2.5 will add: ShutdownClose
    }

    /// A parsed subscription topic. Stored per-connection.
    #[derive(Debug, Clone, PartialEq, Eq, Hash)]
    pub enum Topic {
        EventsAll,                                    // "events.*"
        EventsBySource(String),                       // "events.<source>.*"
        EventsBySourceSession(String, String),        // "events.<source>.<session_id>" (no wildcards)
        StateAll,                                     // "state.session.*"
        StateSession(String),                         // "state.session.<id>"
        StateSessionCurrent(String),                  // "state.session.<id>.current_state"
    }

    impl Topic {
        /// Parse a wire-format topic string. `Err(())` on unrecognized
        /// shape or empty input. Empty is rejected here so the WS handler
        /// can route it to the `bad message:` close path (AC #4).
        pub fn parse(s: &str) -> Result<Self, ()> { /* ... */ }

        /// True if this subscription topic should deliver `envelope`.
        pub fn matches(&self, envelope: &BroadcastEnvelope) -> bool { /* ... */ }
    }
    ```
    **The `Topic::parse` grammar is exact**; document the supported strings as a Rust doc-comment block on the enum. Anything outside the table → `Err(())`:

    | Wire string | Variant |
    |---|---|
    | `"events.*"` | `EventsAll` |
    | `"events.<source>.*"` | `EventsBySource("<source>")` |
    | `"events.<source>.<session_id>"` | `EventsBySourceSession(..., ...)` |
    | `"state.session.*"` | `StateAll` |
    | `"state.session.<id>"` | `StateSession("<id>")` |
    | `"state.session.<id>.current_state"` | `StateSessionCurrent("<id>")` |
    | empty / anything else | `Err(())` |

    `state.session.<id>.current_state` is a strict-subset filter of `StateSession`: when a `State` envelope arrives, **both** `StateSession(id)` and `StateSessionCurrent(id)` match — they don't filter different content in 2.1. Story 2.2 may evolve the per-connection task to project `State` into a smaller wire frame for the `.current_state` subscriber, but that's not 2.1's contract.

  - [ ] Create `crates/daemon/src/broadcast/hub.rs`:
    ```rust
    use tokio::sync::broadcast;

    /// Owns the broadcast channel that fan-outs `BroadcastEnvelope` to
    /// every connected WebSocket client. Per Story 2.4's design, lag is
    /// surfaced via `RecvError::Lagged(n)`; per-connection tasks translate
    /// that into a `DroppedFrame` (2.4 wires it).
    pub struct BroadcastHub {
        tx: broadcast::Sender<crate::broadcast::BroadcastEnvelope>,
    }

    impl BroadcastHub {
        pub fn new(capacity: usize) -> Self {
            let (tx, _rx) = broadcast::channel(capacity);
            Self { tx }
        }

        /// Subscribe — every new WS connection calls this once.
        pub fn subscribe(&self) -> broadcast::Receiver<crate::broadcast::BroadcastEnvelope> {
            self.tx.subscribe()
        }

        /// Publish — Story 2.2 wires this into `projection::session::write`.
        /// Story 2.1 does not publish; the path exists so tests for AC #2
        /// can use it as a synthetic publisher.
        pub fn publish(&self, envelope: crate::broadcast::BroadcastEnvelope) {
            // SendError is fine to swallow: it only happens when there are
            // zero subscribers, which is the normal idle daemon state.
            let _ = self.tx.send(envelope);
        }
    }
    ```
  - [ ] Add `pub mod broadcast;` to `crates/daemon/src/lib.rs`.
  - [ ] Add unit tests in `crates/daemon/src/broadcast/event.rs::tests`:
    - `parse_events_all`, `parse_events_by_source`, `parse_events_by_source_session`, `parse_state_all`, `parse_state_session`, `parse_state_session_current` — happy paths.
    - `parse_rejects_empty`, `parse_rejects_unknown_prefix`, `parse_rejects_too_few_segments`, `parse_rejects_too_many_segments`, `parse_rejects_trailing_dot`.
    - `matches_events_all_matches_any_event`, `matches_events_by_source_filters_other_sources`, `matches_state_all_matches_any_state_envelope`, `matches_state_session_does_not_match_other_session`, `matches_events_does_not_match_state_envelope` (and inverse).

- [ ] **Task 4: Wire `BroadcastHub` and `Semaphore` into daemon startup** (AC: #1, #6)
  - [ ] In `crates/daemon/src/main.rs::run`, after `let (bearer, token_source) = token::load_or_generate();` and before the `AppState` construction, build the hub and semaphore:
    ```rust
    let broadcaster = Arc::new(bowerbird_daemon::broadcast::BroadcastHub::new(config.ws_broadcast_capacity));
    let ws_semaphore = Arc::new(tokio::sync::Semaphore::new(config.ws_max_connections));
    let ws_config = bowerbird_daemon::state::WsConfig {
        ping_interval: config.ws_ping_interval,
        pong_timeout: config.ws_pong_timeout,
    };
    ```
  - [ ] Extend the `AppState { ... }` literal with the three new fields.
  - [ ] No change to graceful-shutdown is required in 2.1; the per-connection task observes `state.shutdown.cancelled()` in its select loop (Task 5). Story 2.5 will refine this to send `CloseFrame` first.

- [ ] **Task 5: Implement the WebSocket handler at `crates/daemon/src/api/ws.rs`** (AC: #1, #2, #3, #4, #5, #6, #7, #8, #9)
  - [ ] Create `crates/daemon/src/api/ws.rs`. High-level structure:
    ```rust
    use std::time::{Duration, Instant};

    use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
    use axum::extract::{Query, State};
    use axum::http::{header, HeaderMap, StatusCode};
    use axum::response::{IntoResponse, Json, Response};
    use serde::Deserialize;
    use serde_json::json;
    use tokio::sync::broadcast::error::RecvError;

    use protocol::{ClientMessage, CloseFrame as _, HelloFrame, ServerMessage};

    use crate::broadcast::{BroadcastEnvelope, Topic};
    use crate::state::AppState;

    const PROTOCOL_VERSION: &str = "1.0";

    #[derive(Deserialize)]
    pub struct WsQuery {
        token: Option<String>,
    }

    /// `GET /ws` handler. Authenticates the bearer before the upgrade; if
    /// auth fails, returns `401` without upgrading. If the connection
    /// semaphore is exhausted, returns `503` without upgrading.
    #[tracing::instrument(skip_all)]
    pub async fn handle_upgrade(
        ws: WebSocketUpgrade,
        State(state): State<AppState>,
        Query(query): Query<WsQuery>,
        headers: HeaderMap,
    ) -> Response { /* ... see body below */ }
    ```
  - [ ] **Auth resolution** (AC #5): extract the candidate token; **header wins if both present**:
    ```rust
    let header_token = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer ").map(str::trim))
        .filter(|s| !s.is_empty());
    let query_token = query.token.as_deref().filter(|s| !s.is_empty());
    let candidate = header_token.or(query_token);
    let authorized = match candidate {
        Some(c) => state.bearer.verify(c),
        None => false,
    };
    if !authorized {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({ "error": "unauthorized" })),
        ).into_response();
    }
    ```
    Mirror the constant-time-compare discipline of `api/auth.rs::require_bearer` — same 401 body, same `tracing::instrument(skip_all)`, never log the candidate. The `?token=` value is sensitive too; it goes through the same `verify` path.
  - [ ] **Semaphore acquire BEFORE upgrade** (AC #6): use `try_acquire_owned` so the permit is owned by the connection task and released on drop:
    ```rust
    let permit = match state.ws_semaphore.clone().try_acquire_owned() {
        Ok(p) => p,
        Err(_) => {
            tracing::warn!("ws connection cap reached; rejecting upgrade");
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({ "error": "too many ws clients" })),
            ).into_response();
        }
    };
    ```
    `try_acquire_owned` is non-blocking; AC #6's test for "256 succeed, 257th fails" requires the rejection path to be synchronous. **Do NOT** use `acquire_owned()` (the async, blocking version) — that would queue the 257th client instead of rejecting it.
  - [ ] **Construct the Hello frame BEFORE returning the upgrade response**, so a same-startup HTTP `/status` snapshot and this WS Hello see consistent values:
    ```rust
    let oldest = compute_oldest_available_event_id(&state.db.reader).await; // helper from §"Hello frame data sources"
    let history_begins_cleanly = compute_history_begins_cleanly(&state.db.reader).await; // helper
    let hello = HelloFrame {
        protocol_version: PROTOCOL_VERSION.to_string(),
        daemon_version: env!("CARGO_PKG_VERSION").to_string(),
        oldest_available_event_id: oldest,
        daemon_started_at: state.started_at_ms,
        history_begins_cleanly,
    };
    ```
    See Dev Notes "Hello frame data sources" for `compute_oldest_available_event_id` and `compute_history_begins_cleanly`. Both reuse existing SQL constants from Story 1.7 (`SELECT_MIN_EVENT_ID`) where possible; one new SQL constant (`SELECT_LAST_RECORDING_CLEANLY_ENDED`) is added by this story.
  - [ ] Subscribe to the broadcast hub **before** the upgrade completes:
    ```rust
    let rx = state.broadcaster.subscribe();
    ```
    The order is intentional: subscribing pre-upgrade guarantees no events committed between subscribe-time and Hello-send can be lost off the broadcast channel. (For 2.1 no one publishes yet, but the discipline matters for 2.2.)
  - [ ] Return `ws.on_upgrade(move |socket| connection_task(socket, state, rx, hello, permit))`.
  - [ ] **Per-connection task body** — a `tokio::select!` loop that:
    1. Sends the `hello` frame as `Message::Text(serde_json::to_string(&ServerMessage::Hello(hello))?)`. If serialization fails (shouldn't), `tracing::error!` and exit; if send fails, exit.
    2. Tracks state:
       - `subscriptions: HashSet<Topic>` (starts empty)
       - `awaiting_pong: Option<Instant>` (the time we sent the last unanswered Ping)
       - `permit: OwnedSemaphorePermit` (kept alive for the entire task)
    3. Loop with `tokio::select!` over four branches:
       - **a.** `inbound = socket.recv()` — handle `Message::Text` by deserializing into `ClientMessage`; on `Subscribe { topic }` and `Unsubscribe { topic }`, validate via `Topic::parse(&topic).map_err(...)` and either insert/remove from the set or close per AC #4. `Message::Ping`/`Message::Pong` are handled by axum framing (the `Pong` arm clears `awaiting_pong`); `Message::Binary` → close per AC #4 ("we only speak text JSON"); `Message::Close` → exit the task.
       - **b.** `envelope = rx.recv()` — match on `Ok(env)`, `Err(RecvError::Lagged(n))`, `Err(RecvError::Closed)`. For `Ok(env)`: check `subscriptions.iter().any(|t| t.matches(&env))`; if matched, project to a wire `ServerMessage` (see "Envelope → wire frame projection" in Dev Notes) and send. For `Lagged(n)`: **Story 2.4** wires the `DroppedFrame` here; **Story 2.1** logs at WARN and continues — the socket does not close on lag. For `Closed`: this only happens if the hub is dropped, which only happens at daemon shutdown; exit the task.
       - **c.** `_ = ping_timer.tick()` (a `tokio::time::interval(state.ws_config.ping_interval)`) — if `awaiting_pong.is_some()` and `now - awaiting_pong > ws_config.pong_timeout`, close the task (AC #8, dead-connection cleanup). Else send `Message::Ping(b"".to_vec())` and set `awaiting_pong = Some(Instant::now())`.
       - **d.** `_ = state.shutdown.cancelled()` (AC #9) — exit the task. (Story 2.5 will refine to send a `Close` frame first.)

  - [ ] **`bad message:` close path** (AC #4): write a single helper `async fn close_with_bad_message(socket: &mut WebSocket, detail: &str)` that sends an axum `Message::Close` with `code: 1008` (Policy Violation) and `reason: sanitize_for_wire_ws(format!("bad message: {}", detail))` where `sanitize_for_wire_ws` strips `\n`/`\r` and caps total bytes at 123 (the WS close-reason limit). Document: this helper is a near-twin of `crates/daemon/src/ingest/handler.rs::sanitize_for_wire` but with a different byte cap. Do NOT reuse the existing helper directly — its 512-byte cap is wrong here.

  - [ ] Add `pub mod ws;` to `crates/daemon/src/api/mod.rs`.

- [ ] **Task 6: Wire `/ws` route + Story 2.1 middleware** (AC: #1, #5, #6, #10)
  - [ ] Modify `crates/daemon/src/api/mod.rs::router` to add the `/ws` route on the authenticated side; the auth is hand-rolled in the WS handler (Task 5) because the upgrade requires reading the bearer from EITHER header OR query — `require_bearer` only consults the header. **Do not** apply `require_bearer` as a `route_layer` to `/ws`; the WS handler does its own auth (header-or-query) per AC #5:
    ```rust
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

    let ws_only = Router::new()
        .route("/ws", get(ws::handle_upgrade));
    ```
    Then merge: `Router::new().merge(unauthenticated).merge(authenticated).merge(ws_only).with_state(state)`.
  - [ ] Apply the middleware stack — **WS routes must be exempt from the 30s `TimeoutLayer`** (AC #10). Two router-shape options; pick the one with less surface area:
    - **Option A (preferred):** apply `TimeoutLayer` only to the `unauthenticated` and `authenticated` sub-routers, not to `ws_only`. This is the lowest-risk path because it does not introduce any custom predicate layer.
    - **Option B:** apply `TimeoutLayer` at the top-level merged router with a custom predicate that exempts `/ws` paths. More general but more code; not warranted at 2.1's scope.
    Take Option A. Code shape:
    ```rust
    use tower_http::{
        limit::RequestBodyLimitLayer,
        request_id::{MakeRequestUuid, PropagateRequestIdLayer, SetRequestIdLayer},
        timeout::TimeoutLayer,
        trace::TraceLayer,
    };

    const BODY_LIMIT_BYTES: usize = 1_048_576; // 1 MiB

    let http_only_stack = ServiceBuilder::new()
        .layer(TimeoutLayer::new(Duration::from_secs(30)))
        .layer(RequestBodyLimitLayer::new(BODY_LIMIT_BYTES));

    let common_stack = ServiceBuilder::new()
        .layer(SetRequestIdLayer::new("x-request-id".parse().unwrap(), MakeRequestUuid))
        .layer(PropagateRequestIdLayer::new("x-request-id".parse().unwrap()))
        .layer(TraceLayer::new_for_http());

    let http_routes = Router::new()
        .merge(unauthenticated)
        .merge(authenticated)
        .layer(http_only_stack);

    Router::new()
        .merge(http_routes)
        .merge(ws_only)
        .layer(common_stack)
        .with_state(state)
    ```
    `request-id` and `TraceLayer` apply to both HTTP and WS upgrades (the upgrade request is HTTP; request-id is useful for the upgrade log line). `TimeoutLayer` and `RequestBodyLimitLayer` apply to HTTP only — WS does not have a "request body" once upgraded, and a 30s timeout would kill long-lived connections.
  - [ ] Update doc-comment on `api::mod::router` to describe the layering. Reference AC #10 in the doc-comment by story number ("Story 2.1 AC #10") for traceability.
  - [ ] **Note** the architecture document (`architecture.md:495-497`) calls for `CatchPanicLayer` too. That is NOT folded into Story 2.1 by the Epic 1 retro action items; defer to a future hardening story rather than expanding 2.1 scope. Add a deferred-work entry: "`CatchPanicLayer` not yet wired (architecture.md:495); panic in a request handler currently bubbles to axum's default tower handling".

- [ ] **Task 7: Daemon contract tests for the WS surface** (AC: #1, #2, #3, #4, #5, #6, #7, #8, #9, #10)
  - [ ] All new tests live in `crates/daemon/tests/contract_daemon.rs`, alongside existing tests. Use the `tokio_tungstenite` dev-dep (Task 1) as the client. Tests use `tokio::test(flavor = "current_thread")` to match the existing convention.
  - [ ] **Test helpers** (add near the top of the test file, after `make_test_state`):
    ```rust
    async fn spawn_test_daemon(state: AppState) -> (SocketAddr, JoinHandle<()>) {
        let router = api::router(state.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("local_addr");
        let handle = tokio::spawn(async move {
            let _ = axum::serve(listener, router)
                .with_graceful_shutdown(state.shutdown.clone().cancelled_owned())
                .await;
        });
        (addr, handle)
    }

    async fn ws_connect_authed(addr: SocketAddr, token: &str) -> ... { /* uses tokio_tungstenite */ }
    async fn ws_connect_unauthed(addr: SocketAddr) -> http::Response<...> { /* expects 401 */ }
    ```
    If `cancelled_owned` is not exposed on the daemon's exact `CancellationToken` version, use `let cancel = state.shutdown.clone(); async move { cancel.cancelled().await }` instead.
  - [ ] **`ws_hello_frame_on_connect`** (AC #1) — connect with valid bearer in `Authorization`, read the first message, assert it is `Text` with `op: "hello"`, `protocol_version: "1.0"`, `daemon_version` equal to `env!("CARGO_PKG_VERSION")` of the daemon crate, and `daemon_started_at == state.started_at_ms`. The `oldest_available_event_id` and `history_begins_cleanly` fields are present (not asserted to specific values — those are tested separately).
  - [ ] **`ws_hello_frame_query_token_path`** — same as above but with `?token=` instead of `Authorization` header. Asserts AC #5's query-param auth path.
  - [ ] **`ws_subscribe_accumulates_then_unsubscribe_removes`** (AC #2, #3) — connect, send two Subscribe messages (`state.session.*` and `events.*`), publish synthetic envelopes via `state.broadcaster.publish(...)` (this is the per-connection topic-match path); assert both envelopes round-trip; send Unsubscribe for one; assert the other still matches and the unsubscribed one does not. **Important**: use a small `tokio::time::sleep(Duration::from_millis(10))` between `subscribe` send and `publish` to ensure the daemon's recv loop has processed the subscribe; otherwise the publish races the subscribe-handling. (This is a test-only concern; real clients can also race, which is fine because subscribe-then-publish is idempotent.)
  - [ ] **`ws_empty_topic_closes_with_policy_violation`** (AC #4) — connect, send `{"op":"subscribe","topic":""}`, assert the daemon closes with WS close code 1008 and reason starts with `"bad message:"`.
  - [ ] **`ws_unknown_op_closes_with_policy_violation`** (AC #4) — send `{"op":"bogus","topic":"events.*"}`, assert close with 1008.
  - [ ] **`ws_extra_field_closes_with_policy_violation`** (AC #4) — send `{"op":"subscribe","topic":"events.*","extra":1}`, assert close with 1008 (this exercises `deny_unknown_fields` on `ClientMessage`).
  - [ ] **`ws_binary_message_closes_with_policy_violation`** (AC #4) — send a binary frame, assert close with 1008.
  - [ ] **`ws_401_when_no_auth`** (AC #5) — attempt upgrade with no `Authorization` and no `?token=`; assert HTTP `401 {"error":"unauthorized"}` and no WS upgrade.
  - [ ] **`ws_401_when_bad_token`** (AC #5) — attempt upgrade with `Authorization: Bearer wrong`; assert HTTP `401`.
  - [ ] **`ws_header_token_wins_over_query`** (AC #5) — attempt upgrade with VALID `Authorization: Bearer <correct>` AND `?token=wrong`; assert upgrade succeeds. The inverse — invalid header, valid query — must FAIL (header wins; failed header is not silently overridden by query). Add this as a second assertion in the same test, in a sub-block, OR as a separate test `ws_invalid_header_does_not_fall_through_to_query`.
  - [ ] **`ws_257th_connection_rejected_503`** (AC #6) — use a small `ws_max_connections: 3` test state; open 3 connections successfully (await Hello on each, keep them alive); attempt a 4th, expect HTTP `503 {"error":"too many ws clients"}`; close one of the 3, attempt a 4th again, expect success. The "close one → permit returned" half of the test is the regression guard against a permit-leak bug.
  - [ ] **`ws_ping_within_idle_window`** (AC #7) — use `ws_ping_interval: Duration::from_millis(100), ws_pong_timeout: Duration::from_millis(50)`; connect; do nothing on the client; assert a `Message::Ping` is received within ~200ms (allow 2× slack on CI). Respond with `Pong`; assert the connection remains open for at least one more ping cycle.
  - [ ] **`ws_no_pong_within_timeout_closes`** (AC #8) — same fast timings; connect; do not respond to the Ping; assert the WS closes within `ping_interval + pong_timeout + slack` (~200ms). Use a `tokio::time::timeout` wrapper around `socket.next().await` to bound the test. Asserts the per-connection task exits.
  - [ ] **`ws_shutdown_token_closes_task`** (AC #9) — connect; trigger `state.shutdown.cancel()`; assert the WS closes within ~100ms. The test scaffolding's `spawn_test_daemon` must propagate cancellation; if it doesn't, refactor it to take an explicit `shutdown` token argument shared with the AppState.
  - [ ] **`x_request_id_on_healthz`** (AC #10) — `GET /healthz`; assert `200 OK`, assert an `x-request-id` response header exists and matches a UUID4 shape (36 chars, hyphens at the right positions). This is the canary that `SetRequestIdLayer` is wired. Do NOT assert the *exact* UUID value (it's random); only the shape.

- [ ] **Task 8: Update `epics.md` AC text to match the actual `ClientMessage` shape** (AC: #2, retro Agreement A2)
  - [ ] `docs/bmad/planning-artifacts/epics.md` lines 488-494 (Story 2.1 § Acceptance Criteria, second block) currently say:
    > **Given** a tool sends a subscribe message `{"topics": ["state.session.*", "events.*"]}`
    > **When** the daemon processes it
    > **Then** subsequent frames are filtered to only those matching the declared topics
    The actual protocol shape per `crates/protocol/src/ws.rs::ClientMessage` is `Subscribe { topic: String }` — one topic per message, with `op: "subscribe"`. Multi-topic subscription is "send multiple Subscribe messages." Replace the AC text in place with the actual wire shape. Epic 1 retro Agreement A2 mandates that when a story-creation pass resolves an upstream-vs-implementation drift, the upstream doc is back-amended inline. Same applies to the PRD reference (`prd.md:371-374`) which currently shows `{"topics": [...]}`. Update both.
  - [ ] Wording template — keep the BDD shape; replace the JSON example only:
    > **Given** a tool sends a subscribe message `{"op":"subscribe","topic":"state.session.*"}`, then later `{"op":"subscribe","topic":"events.*"}`
    > **When** the daemon processes each one
    > **Then** the per-connection subscription set is the union of the declared topics; subsequent server frames are filtered to deliver only matches.
  - [ ] Add a one-line note at the bottom of the Story 2.1 section in `epics.md` ("Wire shape clarified per Story 2.1 creation, 2026-05-20 — single topic per Subscribe message; multi-topic via repeated sends") so future readers see the trail.
  - [ ] DO NOT change the `ClientMessage` enum to use a `Vec<String>` topics array — additive-only-within-v1.x forbids removing the `topic: String` shape, and we have no consumer of multi-topic-in-one-message yet. The cost of the wire-shape mismatch with the epic text is one find-and-replace in the docs.

- [ ] **Task 9: Strike resolved deferred-work entries** (AC: #4, #10)
  - [ ] `docs/bmad/implementation-artifacts/deferred-work.md` line 10 (`ClientMessage empty topic accepted`) — strike with `~~ ... ~~` and append `**Resolved by Story 2.1:** empty-string topics now route to the WS Policy-Violation close path (`crates/daemon/src/api/ws.rs::close_with_bad_message`); see contract test `ws_empty_topic_closes_with_policy_violation`.` Same convention as Stories 1.6/1.7/1.8.
  - [ ] `deferred-work.md` line 56 (`No request-id middleware, no TraceLayer, no per-request timeout, no body-size limit`) — strike with: `**Resolved by Story 2.1 Task 6:** `SetRequestIdLayer` + `PropagateRequestIdLayer` (request-id), `TraceLayer::new_for_http()` (tracing), `TimeoutLayer::new(30s)` (HTTP only; WS exempt), and `RequestBodyLimitLayer::new(1 MiB)` are wired in `crates/daemon/src/api/mod.rs::router`. Contract test `x_request_id_on_healthz` asserts the request-id surface. `CatchPanicLayer` was not folded into 2.1 — a new deferred-work entry below tracks it.`
  - [ ] Add a NEW deferred-work entry (under a new `## Deferred from: Story 2.1 ...` section):
    > - **`CatchPanicLayer` not yet wired** — `architecture.md:495` lists this as required middleware; Story 2.1 wired request-id, trace, timeout, and body-limit per the Epic 1 retro fold-in but did not expand scope to include panic catching. A handler panic currently bubbles to axum's default tower handling (which closes the connection without a structured 500 body). Wire `tower_http::catch_panic::CatchPanicLayer::custom(...)` with a JSON 500 response in a future hardening story. [`crates/daemon/src/api/mod.rs`]
  - [ ] Add ONE more new deferred-work entry if applicable, for any incidental gaps the dev encounters during implementation (do not pre-populate; add as discovered). Format follows the existing convention: bullet, **bold** filename/title, file:line, one-line rationale.

- [ ] **Task 10: Update `docs/protocol-changelog.md`** (AC: #1, #2, #4, #5, #6, #7, #10)
  - [ ] Read the current changelog first to understand the section structure Stories 1.7/1.8 established (entries grouped by `behavioral` / `schema` / `security`).
  - [ ] Add ONE schema entry (the WS surface is new) and ONE behavioral entry (the HTTP middleware reshape changes observable response headers):

    **Schema:**
    > - **WebSocket surface live at `GET /ws`** (Story 2.1). Authenticated via bearer token in `Authorization: Bearer <token>` header (preferred) or `?token=<token>` query parameter (fallback for clients that cannot set headers). On upgrade, the daemon sends one `hello` ServerMessage containing `protocol_version`, `daemon_version`, `oldest_available_event_id`, `daemon_started_at`, and `history_begins_cleanly`. Subscribe/Unsubscribe topic filtering accepts the topic strings `events.*`, `events.<source>.*`, `events.<source>.<session_id>`, `state.session.*`, `state.session.<id>`, `state.session.<id>.current_state`. Unknown topics and empty topics close the connection with WS close code 1008 (Policy Violation) and a `bad message: ...` reason string. Concurrent connection cap is 256 (configurable); 257th upgrade returns HTTP 503 without upgrading. Idle ping interval is 30s; pong timeout is 10s; on pong timeout the connection is closed. Event publishing into the broadcast hub is NOT YET wired in this release; Story 2.2 ships that. Tools can connect, subscribe, and observe Hello + lifecycle frames as of this release.

    **Behavioral:**
    > - **HTTP surface now emits `x-request-id` and respects a 30s per-request timeout and a 1 MiB request-body limit** (Story 2.1). Every HTTP request (including WS upgrades for trace purposes) receives an `x-request-id` UUID4 header on the response, propagated for cross-cut tracing. HTTP request handlers (NOT WebSocket connections) are bounded by a 30-second wall-clock timeout; requests exceeding it receive HTTP `408 Request Timeout`. Request bodies larger than 1 MiB receive `413 Payload Too Large`. WebSocket connections are exempt from both. Existing clients that completed all v1.0 requests in under 30s and stayed under 1 MiB are unaffected.
  - [ ] Do NOT include a `security` entry — the auth model (bearer token) is unchanged; the WS surface just adds a second carrier (`?token=`) for the same token.

- [ ] **Task 11: Full-workspace verification** (AC: all)
  - [ ] Run `cargo fmt --check` and `cargo clippy --all-targets --workspace -- -D warnings` from the repo root.
  - [ ] Run `cargo test --workspace`. Expect at least 121 + new tests (count of new WS tests: ~13).
  - [ ] Run `cargo build --workspace --release`. The release build must remain clean (no new release-only warnings).
  - [ ] **Bench non-regression**: run `cargo bench --no-run --workspace`. The shim hot-path bench must still compile; no new bench is added in 2.1. If a new bench is wanted for WS hub publish-throughput, defer it — `tokio::sync::broadcast` is a known commodity; benching it on day-one of the broadcaster's existence is speculative optimization (Axiom 3).
  - [ ] **Smoke test** (manual, optional but recommended): launch the daemon against a `$(mktemp -d)`, then:
    ```sh
    # In one terminal, start the daemon:
    BOWERBIRD_TOKEN=local-dev-token cargo run -p bowerbird-daemon -- -vv
    # In another terminal, connect with websocat:
    websocat 'ws://127.0.0.1:<port>/ws' -H "Authorization: Bearer local-dev-token"
    # Expect: a Hello frame as the first text message.
    # Then type: {"op":"subscribe","topic":"events.*"}
    # Expect: no further frames yet (Story 2.1 has no publishers).
    # Try: {"op":"subscribe","topic":""}
    # Expect: connection closes with code 1008.
    ```
    Port comes from the daemon's WARN log line `addr=... daemon listening`. `websocat` is installable via `brew install websocat` / `cargo install websocat`.

## Dev Notes

### Why this story is the deferred-work watershed for Epic 1 → Epic 2

The Epic 1 retrospective (`docs/bmad/implementation-artifacts/epic-1-retro-2026-05-20.md`) chose **Option C** for handling unresolved Epic 1 deferred-work: items relevant to Epic 2 are folded into existing Epic 2 stories as **required** ACs, not "consider" notes. Story 2.1 absorbs:

- `deferred-work.md` line 10 — `ClientMessage` empty-topic rejection — folded into AC #4.
- `deferred-work.md` line 56 — request-id, TraceLayer, TimeoutLayer (30s HTTP), RequestBodyLimitLayer — folded into AC #10.

These are not best-effort; the contract tests for Tasks 6 and 7 enforce them. The retro § "Action items for Epic 2" lists these explicitly as Story 2.1 ACs; do not silently de-scope them.

### Files this story TOUCHES (UPDATE)

Per the Epic 1 codebase scan; verify line numbers in source before editing (Story 1.8 noted these can drift):

| File | Change | Why |
|---|---|---|
| `Cargo.toml` (workspace) | Add `features = ["ws"]` to axum; expand tower-http features; add `tokio-tungstenite = "0.27"` to dev-deps | Task 1 |
| `crates/daemon/src/config.rs` | Add 4 new WS-related fields; add `Duration` import | Task 2 |
| `crates/daemon/src/state.rs` | Add `broadcaster`, `ws_semaphore`, `ws_config` fields; define `WsConfig` struct | Task 2 |
| `crates/daemon/src/lib.rs` | Add `pub mod broadcast;` | Task 3 |
| `crates/daemon/src/main.rs::run` | Construct `BroadcastHub`, `Semaphore`, and `WsConfig`; extend `AppState` literal | Task 4 |
| `crates/daemon/src/api/mod.rs` | Add `pub mod ws;`; reshape router with `ws_only` merge + Tower middleware stack | Task 6 |
| `crates/daemon/tests/contract_daemon.rs` | Extend `make_test_state` for new fields; add ~13 new WS contract tests; helper `spawn_test_daemon`; helper `ws_connect_*` | Task 7 |
| `docs/bmad/planning-artifacts/epics.md` | Back-amend Story 2.1 AC #2 to match actual `ClientMessage` shape; add 2026-05-20 trail note | Task 8 |
| `docs/bmad/planning-artifacts/prd.md` | Back-amend WebSocket Subscribe wire example near line 371 | Task 8 |
| `docs/bmad/implementation-artifacts/deferred-work.md` | Strike lines 10 and 56; append Story 2.1 resolution notes | Task 9 |
| `docs/protocol-changelog.md` | Add one schema + one behavioral entry | Task 10 |

### Files this story CREATES (NEW)

| File | Purpose |
|---|---|
| `crates/daemon/src/broadcast/mod.rs` | Module root: `pub mod event;`, `pub mod hub;` + re-exports |
| `crates/daemon/src/broadcast/event.rs` | `BroadcastEnvelope` enum (internal-only); `Topic` enum with `parse` and `matches`; unit tests |
| `crates/daemon/src/broadcast/hub.rs` | `BroadcastHub` wrapping `tokio::sync::broadcast::Sender`; `subscribe`/`publish` |
| `crates/daemon/src/api/ws.rs` | `handle_upgrade` + `connection_task` + `close_with_bad_message` + small helpers |

No new test file: all WS tests live in the existing `crates/daemon/tests/contract_daemon.rs` next to the existing REST + ingest tests. Rationale: the Epic 1 pattern is one contract-test file per crate; introducing `contract_ws.rs` now would split the AppState helper infrastructure across files. If `contract_daemon.rs` grows past ~3000 lines after 2.1–2.5, revisit the split decision in Epic 2's retro.

### Existing files the dev MUST read before editing (context, no changes)

These files document patterns the WS handler must follow. Read them, do not change them:

| File | What to learn from it |
|---|---|
| `crates/daemon/src/api/auth.rs` | The 401 body shape (`{"error":"unauthorized"}`), `tracing::instrument(skip_all)`, never-log-token discipline. The WS handler's auth path **must** match this verbatim so a single 401 contract assertion covers both surfaces. |
| `crates/daemon/src/api/health.rs` | The pool-checkout + `interact` pattern. The `compute_oldest_available_event_id` helper (Task 5) follows this exact shape — reader pool → `interact` → `query_row` → map `QueryReturnedNoRows` to a sentinel. |
| `crates/daemon/src/api/status.rs` | `env!("CARGO_PKG_VERSION")` + `current_unix_millis()` usage for daemon-version + uptime; the Hello frame reuses both. The `PROTOCOL_VERSION: &str = "1.0"` constant is intentionally duplicated between `status.rs` and `ws.rs` rather than centralized — the architecture treats REST and WS as siblings with the same protocol version, and a future protocol-version-mismatch story would update both. |
| `crates/daemon/src/ingest/handler.rs` | `sanitize_for_wire`'s shape and intent — the WS `close_with_bad_message` helper is a sibling with a different byte cap (123 vs 512). |
| `crates/protocol/src/ws.rs` | The wire shape that is authoritative. `ClientMessage::Subscribe { topic: String }` is single-topic per message; `op: "subscribe"`/`"unsubscribe"`; `deny_unknown_fields` on inbound. The PRD/epic example showing `{"topics":[...]}` is wrong and is being back-amended (Task 8). |
| `crates/daemon/src/main.rs::run` | Where the new `BroadcastHub` + `Semaphore` are constructed; where `AppState` is built; how graceful shutdown propagates via `CancellationToken`. |

### Hello frame data sources

The Hello frame has five fields. Three are constants or read from `AppState`; two require small DB queries:

| Field | Source |
|---|---|
| `protocol_version` | Literal `"1.0"` (`PROTOCOL_VERSION` constant in `ws.rs`; matches `api::status::get`) |
| `daemon_version` | `env!("CARGO_PKG_VERSION")` (matches `api::status::get`) |
| `daemon_started_at` | `state.started_at_ms` (already in `AppState` from Story 1.7) |
| `oldest_available_event_id` | Reuse `db::queries::SELECT_MIN_EVENT_ID` (added by Story 1.7). `Option<i64>` → `Some(min) → EventId(min)` / `None → EventId(i64::MAX)`. |
| `history_begins_cleanly` | New SQL: `SELECT_LAST_RECORDING_CLEANLY_ENDED` — query the `recording_sessions` shadow table for "is there a `RecordingEnded` event_id between the `MIN(events.event_id)` and `started_event_id` of the current recording?" — see grammar below. |

**`history_begins_cleanly` semantics** (per `architecture.md:155-159`): a presenter cold-starting wants to know whether `oldest_available_event_id` falls inside a known-clean recording window. The mechanical fingerprint of "clean" is: `recording_sessions` table has at least one row whose `started_event_id <= oldest_available_event_id <= ended_event_id`. If yes, the history below `oldest_available_event_id` was a deliberate truncation (post-V1 `bowerbird gc`); the gap is documented. If no, the gap is a crash gap.

For V1 (no `bowerbird gc` yet), the only way `oldest_available_event_id > MIN(events.event_id)` is via manual DB delete, which is outside the substrate's contract. So `history_begins_cleanly` is computable as:

```sql
-- New SQL constant in db/queries.rs:
pub const SELECT_HISTORY_BEGINS_CLEANLY: &str =
    "SELECT EXISTS( \
       SELECT 1 FROM recording_sessions \
       WHERE started_event_id <= (SELECT COALESCE(MIN(event_id), 0) FROM events WHERE source != '__daemon__') \
         AND ended_event_id IS NOT NULL \
         AND ended_event_id >= (SELECT COALESCE(MIN(event_id), 0) FROM events WHERE source != '__daemon__') \
     ) AS clean";
```

The `COALESCE(..., 0)` handles the empty-events case (no recording yet → no clean window claim). Returns `1` if clean, `0` if not. The daemon maps to `bool`.

**If the DB probe fails** during Hello construction (extreme edge — DB lock contention with the writer pool), use the conservative defaults: `oldest_available_event_id = EventId(i64::MAX)` (empty), `history_begins_cleanly = false`. Log a WARN with the request-id. The connection still upgrades and the Hello frame still sends — the alternative (refuse the upgrade) blocks subscribers for a transient DB issue.

### Envelope → wire frame projection

When the per-connection task pops a `BroadcastEnvelope` off the broadcast receiver and the topic-match check passes, project it to a wire `ServerMessage`:

| `BroadcastEnvelope` variant | Wire `ServerMessage` |
|---|---|
| `Event(event)` | `ServerMessage::Event(EventFrame { event })` |
| `State { source, session_id, state }` | For `Topic::StateAll`, `Topic::StateSession(_)`: send `ServerMessage` carrying the full `SessionState` (current_state + last_event_kind + last_event_at_ms). **Important**: `crates/protocol/src/ws.rs` does NOT YET have a `State` variant on `ServerMessage` — the protocol crate's WS module ships only `Hello`/`Sync`/`Event`/`Dropped`/`Close`. Adding `State` is **additive** to `ServerMessage` (an outbound permissive enum); old clients ignore unknown variants per `architecture.md:606-608`. So this story extends `ServerMessage` with `State(StateFrame)` AND adds `StateFrame { source, session_id, state }` to `crates/protocol/src/ws.rs`. A snapshot test in `crates/protocol/tests/contract_protocol.rs` round-trips a `StateFrame` with an extra unknown field. |

The `StateFrame` extension is part of Task 5's scope; document the protocol crate change in a Task 10 changelog entry (already covered by the schema entry).

For `Topic::StateSessionCurrent(_)`, Story 2.1 still sends the full `StateFrame` — projecting down to just `current_state` is an opt-in future optimization noted as deferred-work (Task 9 should add it): "`state.session.<id>.current_state` subscribers currently receive the full `StateFrame`; a future story may project to a smaller wire frame if presenter bandwidth demands it." This avoids 2.1 needing to design a second `StateFrame` variant.

### WS-vs-HTTP middleware split

axum lets `Router::layer` stack middleware. The catch: applying `TimeoutLayer::new(30s)` to a router that also serves `/ws` would kill long-lived WebSocket connections at 30s — wrong. There are three patterns to avoid this:

1. **Pattern A (chosen)**: split the router. HTTP-only routes get their own sub-router with `TimeoutLayer` and `RequestBodyLimitLayer` layered on; `/ws` gets its own sub-router with only the cross-cut middleware (request-id, trace). The top-level merge layers shared middleware once. This is the lowest-code-surface option.

2. **Pattern B**: use `tower_http::timeout::TimeoutLayer` with a custom `Predicate` that returns `false` for `request.uri().path() == "/ws"`. tower-http does support `RequireAuthorizationLayer::with_predicate`-style filters, but `TimeoutLayer` does not expose a predicate API in 0.6.x. You'd write a `tower::Service` wrapper. Not warranted at 2.1's scope.

3. **Pattern C**: keep `TimeoutLayer` at the top level and live with it. **Wrong** — would close every WS at 30s.

Task 6 specifies Pattern A.

### Topic-match invariant: every envelope visits at most one matching topic class per subscription

Given a subscription set `{StateAll, StateSession("sess-1")}`, an envelope `State { session_id: "sess-1", ... }` matches **both** entries. The per-connection task must NOT send the wire frame twice. The match logic should be `any(|t| t.matches(&env))` (short-circuit or merge), not `for t in topics { if t.matches(&env) { send } }`. Document this in a comment near the per-connection match loop. Add a contract test if doing so doesn't blow the test count past reasonable: `ws_overlapping_subscriptions_dedup` — subscribe to `state.session.*` AND `state.session.sess-1`; publish a single `State` envelope for sess-1; assert exactly ONE wire frame arrives on the client. Add this as a "stretch" test, OPTIONAL if scope is tight — the AC #2 wording allows it.

### Library/version pins (verified against `Cargo.toml`)

| Crate | Version | Source |
|---|---|---|
| `axum` | `0.8.9` with `["ws"]` (was unfeatured) | Workspace; adding `ws` is an additive feature flip |
| `tower-http` | `0.6.10` with `["request-id", "trace", "timeout", "limit", "util"]` | Workspace |
| `tokio-tungstenite` | `0.27` (dev-deps only) | NEW workspace dev-dep; matches axum 0.8.9's wrapped version |
| `tokio` | `1.52.1`; sync feature already enabled (uses `broadcast` + `Semaphore`) | Workspace |
| `serde` / `serde_json` | `1.0.228` / `1.0.149` | Workspace |
| `tracing` | `0.1.44` | Workspace |

**Why axum 0.8 for WS specifically**: axum 0.7 → 0.8 changed path param syntax from `:id` to `{id}` (already adopted for REST in Story 1.7 — see `api/mod.rs` doc-comment). The `ws` feature itself is stable across 0.7/0.8 — the upgrade is a no-op for WS surface area; we just turn it on.

**Why NOT add `tokio-tungstenite` as a non-dev dep**: axum re-exports the message and frame types we need via `axum::extract::ws::{Message, WebSocket, WebSocketUpgrade, CloseFrame}`. The daemon code never imports `tokio_tungstenite` directly; only the test client does.

### Project-context references for invariants this story must hold

These are the **load-bearing** discipline rules from `architecture.md` and `project-context.md` that the dev MUST NOT violate. Many appear in Story 1.7's Dev Notes too — same project, same rules.

- **`unsafe_code = "forbid"`** workspace-wide (`Cargo.toml:5-6`). No `unsafe` blocks.
- **`#[tracing::instrument(skip_all)]`** on every async fn that takes `AppState` or any user input. `skip_all` prevents bearer tokens from appearing in spans. The `connection_task` body uses targeted `tracing::info!(client_count = ...)` or `tracing::debug!(topic = %t)` only — never `?candidate_token`, never `?headers`.
- **`thiserror` only** in the daemon's lib code; `anyhow` only at `main.rs` edges. `api/ws.rs` returns `Response` from `handle_upgrade` and uses tower-http's built-in 500-on-unwrap behavior; there is no `Error` enum needed in `ws.rs`. If you find yourself reaching for `anyhow::Result` in `ws.rs`, stop — return `Response` directly with `IntoResponse`.
- **No raw `Connection::open`**, no SQL outside `db/queries.rs` — `SELECT_HISTORY_BEGINS_CLEANLY` is added to `queries.rs`, not inlined in `ws.rs`. The `scripts/lint-db-access.sh` CI lint enforces this.
- **The token is never logged.** `secrecy::SecretString` handles the daemon's stored value; the `?token=` query param is only ever passed into `BearerToken::verify` and never bound to a variable that lives past the auth check.
- **axum 0.8 path-param syntax**: `{id}`, not `:id`. The new `/ws` route has no path params, so this is mostly a flag for the next story (`/ws/v2`? — not planned). Document the convention near the new route in `api/mod.rs` if it's not already documented (Story 1.7 added a doc-comment line for this — preserve it).
- **`(source, session_id)` natural-key discipline.** Topic `state.session.<id>` is **single-key** (just `session_id`), which is a deliberate Epic 2 surface decision — see `prd.md:387-393` "Topics (v1)". V1 has only one source (`"claude"`), so single-key is unambiguous. If a second adapter (Codex, OpenCode) ships post-V1, the **deferred-work** entry for `/sessions/{id}` source disambiguation (`deferred-work.md` line 51) also applies to WS topics. Add a one-line cross-reference to `deferred-work.md` line 51 in the `Topic::StateSession` variant's doc-comment.

### Tests to update (existing, may break with new fields)

`make_test_state` is the only existing function that constructs `AppState`. Story 1.7 added five fields; Story 2.1 adds three more. Every existing daemon test that calls `make_test_state` should keep working with the extended signature (the helper does the construction). Verify with `cargo test --workspace`. If any existing test panics on `Semaphore::try_acquire` because the default test semaphore is too small (default 4 per Task 2), bump the test default or let the test pass its own permit count.

### "Standards-by-default" (retro Agreement A1) check

The retro committed the team to standards-by-default. Story 2.1's WS surface is axum's built-in WebSocket (which is `tokio-tungstenite` underneath). No bespoke binary protocol, no custom framing, no hand-rolled handshake. The only non-standard choice is the `?token=` auth fallback — and that's because browser `new WebSocket()` cannot set headers, which is a known industry constraint (see [RFC 6750 §2.3](https://datatracker.ietf.org/doc/html/rfc6750#section-2.3) for the URI-Query-Parameter form). Including `?token=` is the standards-aligned solution to the standards-imposed constraint.

The ingest socket is still the only bespoke surface in the system (retro § Key Insight 1). The WS surface does not inherit that lesson; it inherits the opposite lesson (use axum + tokio-tungstenite as the standard).

### Anti-patterns (explicitly forbidden)

- **Sending the Hello frame from inside `on_upgrade`'s closure** before reading the broadcaster subscribe handle. The subscribe MUST happen before Hello-send to guarantee no events committed between subscribe-time and Hello-send are lost (matters for 2.2; the discipline is set in 2.1).
- **Holding the semaphore permit in `AppState` or the broadcast hub**. The permit is owned by the per-connection task and only the task. Anywhere else and a permit leak becomes a connection-cap underflow.
- **Acquiring the semaphore via `.acquire_owned().await`** (blocking). Use `.try_acquire_owned()`. The 257th client must be rejected synchronously, not queued.
- **Reusing `sanitize_for_wire` from `ingest/handler.rs`** for the WS close reason. Its 512-byte cap exceeds the 123-byte WS close-reason RFC limit. Make `close_with_bad_message` use its own constant.
- **Adding a `?token=` to the `/healthz` or `/readyz` routes**. Those routes are unauthenticated; preserving that is a Story 1.7 invariant. Adding query-param auth to them would silently break LB probes that don't send query strings.
- **Reading the `Authorization` header in `connection_task`**. By that point the upgrade is already accepted and headers are gone. All auth happens in `handle_upgrade` BEFORE the upgrade response is constructed.
- **Logging the candidate token, the verified token, or any header byte that could contain it.** `tracing::instrument(skip_all)` plus `secrecy::SecretString` handles this for the stored token; `handle_upgrade` adds no field bindings.
- **Adding a publish call to `BroadcastHub` anywhere outside `crates/daemon/src/broadcast/`**. Story 2.1 ships the hub without publishers. Story 2.2 wires the first publisher into `projection::session::write`. If you find yourself tempted to publish from `ws.rs` or `main.rs`, stop — you're outside 2.1's scope.

## Previous Story Intelligence (from Story 1.8)

Story 1.8 was the small tail-end story that tightened the ingest `hook_kind` field from "silent default to PreToolUse" to "required, 400 on absence." Its dev notes contain three patterns Story 2.1 should mirror:

1. **`sanitize_for_wire` as a single canonical helper for any user-controlled string flowing into a wire response.** Story 2.1's WS-close path needs a sibling helper (`close_with_bad_message`) for the same reason: a malicious or buggy client can inject `\n` or `\r` into a topic string, and the close-reason field must not corrupt the frame structure. The byte cap differs (123 vs 512), but the discipline is the same.
2. **Typed errors over string-prefix sniffing.** Story 1.8 added `protocol::Error::UnknownHookKind` rather than reading the error string. Story 2.1 follows the same discipline: parse `ClientMessage` via serde, parse `Topic` via the new `Topic::parse`, and route distinct error cases via Rust pattern-match — not string contains.
3. **Inline ADR-or-correct-the-upstream-doc whenever a story-creation pass resolves drift.** Story 1.8 struck the deferred-work entry that birthed it and added a `behavioral` protocol-changelog entry. Story 2.1 does the same: strikes `deferred-work.md` lines 10 and 56, back-amends `epics.md`/`prd.md` for the Subscribe shape, and adds two `protocol-changelog.md` entries (one `schema`, one `behavioral`).

The Story 1.8 implementation also introduced `tracing::debug!` log lines at every 400 path so a dev running with `-vv` could see the exact malformed payload class. Story 2.1's per-connection task should do the same for the WS close path — `tracing::debug!(detail = %d, "ws: bad message; closing")` — never `tracing::debug!(?message)` (the message body might contain a token in some pathological client).

## Git Intelligence Summary (last 5 commits)

```
1f62549  Merge pull request #22 from technicalpickles/epic-1-retro
bc657f7  docs(epic-1): retrospective and epic completion status
0eed00c  Merge pull request #21 from technicalpickles/story-1.8
183ac41  feat(story-1.8): require hook_kind on daemon ingest payloads
926eeb5  Merge pull request #20 from technicalpickles/story-1.7
```

Epic 1 is fully merged into `main`; tree is clean; tests are 121 passing across 13 suites at the start of Story 2.1. The commit convention `feat(story-X.Y): <subject>` should be followed; final merge will be `Merge pull request #N from technicalpickles/story-2.1`.

No commits to Epic 2 yet — Story 2.1 is the first.

## Latest Tech Information

- **axum 0.8.9** — Latest stable. The `ws` feature is stable and well-documented; `WebSocketUpgrade`, `WebSocket`, `Message`, and `CloseFrame` are the public surface. Reference: <https://docs.rs/axum/0.8.9/axum/extract/ws/index.html>. No breaking changes between 0.8.0 and 0.8.9 in the `ws` API.
- **tokio 1.52.1** — Latest stable for the 1.x series; `tokio::sync::broadcast::Receiver::recv` returning `Err(RecvError::Lagged(n))` is the standard mechanism the architecture chose for slow consumers; documented at <https://docs.rs/tokio/1.52.1/tokio/sync/broadcast/index.html#lagging>. Story 2.4 wires this into `DroppedFrame`; Story 2.1 only needs to compile cleanly against the `RecvError` enum.
- **tower-http 0.6.10** — `SetRequestIdLayer` + `PropagateRequestIdLayer` are the canonical request-id pair; `MakeRequestUuid` is the default ID generator (UUID4). `TimeoutLayer::new(d)` is the wall-clock per-request layer (returns `408` on timeout). `RequestBodyLimitLayer::new(n)` returns `413` on oversize. All four are stable and additive within 0.6.x.
- **tokio-tungstenite 0.27** — axum 0.8.9 wraps this exact version (via `axum-tungstenite` internally). Adding it as a dev-dep at the matching version avoids the message-type mismatch hazard where a 0.26-dev-dep client cannot interpret 0.27-axum messages.
- **`secrecy 0.10.3`** — already in workspace; `BearerToken` wraps `SecretString`; the WS `?token=` flow reuses `BearerToken::verify` without any change.

No security advisories outstanding for any of the above as of 2026-05-20.

## Project Context Reference

Read these documents in this order if you have not yet, then return here:

1. **`docs/bmad/planning-artifacts/architecture.md`** — especially §"Architectural Decisions › API & Communication Patterns" (lines ~450–478), §"Implementation Patterns & Consistency Rules › Wire Format Conventions" (lines ~573–610), §"Project Structure & Boundaries › Architectural Boundaries" (lines ~870–897).
2. **`docs/bmad/planning-artifacts/prd.md`** — §"API Surface (v1 Stable) › WebSocket" (lines ~367–394).
3. **`docs/bmad/planning-artifacts/epics.md`** — §"Epic 2 › Story 2.1" (lines ~480–512). NOTE: AC #2's example JSON is being back-amended by Task 8 of this story.
4. **`docs/bmad/implementation-artifacts/epic-1-retro-2026-05-20.md`** — §"Action items for Epic 2 › Story 2.1" (lines ~98–107). These are the **mandatory** middleware fold-ins.
5. **`docs/bmad/implementation-artifacts/deferred-work.md`** — lines 10, 56 (folded into Story 2.1); other lines as context.
6. **`docs/decisions/0002-ingest-wire-framing-and-hook-kind.md`** — explains the ingest-side NDJ choice. The WS surface does **not** inherit this; the WS surface is standards-by-default per retro Agreement A1.
7. **`docs/bmad/implementation-artifacts/1-7-rest-query-api.md`** — the closest prior pattern (auth middleware, axum 0.8 router shape, protocol-changelog format). Mirror its discipline.

## Story Completion Status

This story was created via `bmad-create-story` on 2026-05-20 immediately after Epic 1 close. The Ultimate context engine analysis was completed and a comprehensive developer guide produced. The story is **`ready-for-dev`**.

### Dev Agent Record

#### Agent Model Used

(populated by dev-story)

#### Debug Log References

(populated by dev-story)

#### Completion Notes List

(populated by dev-story)

#### File List

(populated by dev-story)
