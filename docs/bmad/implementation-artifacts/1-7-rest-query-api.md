# Story 1.7: REST Query API

Status: done

<!-- Note: Validation is optional. Run validate-create-story for quality check before dev-story. -->

## Story

As a tool builder,
I want to query bowerbird's REST API for session list, projected state, and cursor-paginated event history with gap-detection support,
so that I can build tools that show current Claude Code session state and recover correctly when they have missed events.

## Acceptance Criteria

1. **Given** a running daemon **When** I call `GET /healthz` with no `Authorization` header **Then** I receive `200 {"status":"ok"}` — the process is up and responding. (Already shipped in Story 1.2; this story preserves the behavior and confirms it via a fresh contract test under the new router shape.)

2. **Given** a running daemon with DB reachable and migrations applied **When** I call `GET /readyz` with no `Authorization` header **Then** I receive `200 {"status":"ready"}`; if migrations have not applied OR the database is unreachable (pool checkout fails OR `SELECT 1` errors), I receive `503 {"error":"not ready"}`. Story 1.7 tightens `readyz` from "migrations_complete only" (Story 1.2 behavior) to a `migrations_complete && db_probe_ok` hybrid — this resolves `deferred-work.md` line 32. The DB probe uses a single reader-pool checkout and a literal `SELECT 1 FROM events WHERE 1=0` (fast, no row scan, schema-validates `events` table existence in one shot). Probe latency budget: 50ms p99 under unloaded daemon; if pool checkout exceeds the existing 5s wait timeout, `readyz` returns `503` rather than hanging the LB.

3. **Given** events have been ingested for two distinct `(source, session_id)` pairs **When** I call `GET /sessions` with a valid bearer token **Then** both sessions appear in the JSON response in a stable order (`ORDER BY updated_at DESC, source ASC, session_id ASC`), the daemon sentinel row (`source = '__daemon__'`) is filtered out, and each item carries `{ source, session_id, current_state, last_event_kind, last_event_at_ms, updated_at }`. `current_state` is the **read-time** value from `projection::state::current_state_for_read(&stored, now_ms)` (per Story 1.6 contract: a stale `Working` row falls back to `Idle` after `STALE_WORKING_MS = 5 min`).

4. **Given** an existing session `("claude", "sess-a")` with one or more events **When** I call `GET /sessions/sess-a` with a valid bearer token **Then** I receive `200 { source, session_id, state: { current_state, last_event_kind, last_event_at_ms }, updated_at }` where `current_state` is the read-time projection (stale-Working → Idle per Story 1.6); if `session_id` does not match any row in `session_projections` (excluding `__daemon__`), I receive `404 { "error": "session not found" }`. Multi-source disambiguation (when two sources have collided `session_id` values) is **out of scope** for V1: the endpoint returns the most-recently-updated row by `(source, session_id)` natural key; a deferred-work entry is added for the future `?source=` query param when a second adapter ships.

5. **Given** 100 events ingested for `("claude", "sess-a")` **When** I call `GET /sessions/sess-a/events?since=0` with a valid bearer token **Then** all 100 events are returned in ascending `event_id` order, each row contains all fields of `protocol::Event` including `created_at` (NFR22 timestamp column verified surfaced), the response body shape is `EventListResponse { events: Vec<Event>, cursor: Option<EventId>, oldest_available_event_id: EventId }`, `cursor = Some(events.last().event_id)` (next-since cursor for tailing) and `oldest_available_event_id = <min event_id in events table>` (or `EventId(i64::MAX)` if the events table is empty — already documented in `architecture.md:427`). If the `events` table is empty for that session, `events: []` and `cursor: None`.

6. **Given** the first 50 of 100 events have been purged from the log (this story does not implement purge; the test inserts events with explicit `event_id` ≥ 51 via the test path to simulate post-purge state, OR uses a manual `DELETE FROM events WHERE event_id < 51` against the test DB) **When** I call `GET /sessions/sess-a/events?since=10` with a valid bearer token **Then** the response contains `oldest_available_event_id = 51`, enabling the presenter to detect that events 10–50 are no longer available via the mechanical inference `since < oldest_available_event_id` (gap-detection mechanical fact per Axiom 4 — substrate emits the fact, presenter interprets the gap). The daemon does NOT compute or emit any `gap_detected: bool` field — that is presenter semantics, explicitly forbidden by `project-context.md:481`.

7. **Given** a request to any authenticated endpoint (`/sessions`, `/sessions/:id`, `/sessions/:id/events`, `/sessions/:id/stats`, `/status`) **When** no `Authorization` header is provided OR the header is provided but the bearer token does not match the daemon's active token **Then** I receive `401 { "error": "unauthorized" }` (no body field that distinguishes "missing" vs "wrong" — see Dev Notes "Auth response shape: 401 body invariant"); `/healthz` and `/readyz` remain unauthenticated. Token comparison is **timing-safe** (constant-time `subtle::ConstantTimeEq` comparison; do NOT use `==` on bytes — leaks token length via early-exit). The bearer-token surface is **never** logged (existing `tracing::instrument(skip_all, ...)` discipline from `architecture.md:661-670` extends to all new API handlers and the auth layer).

8. **Given** `GET /sessions/sess-a/stats` with a valid bearer token returns a `SessionStats` JSON body **When** a v1.0 client deserializes a response that contains an **extra unknown field** added in a future daemon release (e.g. `tool_use_breakdown`) **Then** the client deserializes the response without error. The contract test for this AC sends a hand-rolled JSON blob with a future field through `serde_json::from_str::<protocol::SessionStats>` and asserts the parse succeeds — this is the canary for the asymmetric `deny_unknown_fields` policy on the outbound surface (`architecture.md:606-608`, `architecture.md:714`).

## Tasks / Subtasks

- [x] **Task 1: Add bearer-token type + loader to daemon** (AC: #7)
  - [x] Add `secrecy = { workspace = true }` to `crates/daemon/Cargo.toml` `[dependencies]` (workspace already pins `secrecy = "0.10.3"` per `architecture.md:328`). Add `subtle = "2.6"` to workspace deps **and** to daemon deps — required for constant-time comparison; this is a new dep, document the rationale in the commit. (`subtle` is the canonical Rust crate for timing-safe primitives, no_std, ~0 KB. The author considered hand-rolling — rejected: cryptographic primitives are not the place to invent.)
  - [x] Create `crates/daemon/src/api/token.rs` with:
    - `pub struct BearerToken(secrecy::SecretString);` deriving `Clone`. Implement `pub fn new(s: String) -> Self`, `pub fn generate_uuid4() -> Self` (uses `uuid::Uuid::new_v4().to_string()`), and `pub fn verify(&self, candidate: &str) -> bool` doing a constant-time compare via `subtle::ConstantTimeEq` on the raw bytes (extract the inner string via `secrecy::ExposeSecret::expose_secret`). Do NOT implement `Debug` or `Display` — `secrecy::SecretString` already redacts; do NOT add a `pub fn expose(&self) -> &str` on `BearerToken` (the only legitimate caller is `verify`, which lives in this module).
    - `pub fn load_or_generate() -> (BearerToken, TokenSource)` where `TokenSource` is `enum { Env, Generated }`. Resolution order for V1:
      1. `BOWERBIRD_TOKEN` env var: if set and non-empty, use it; `TokenSource::Env`.
      2. Generate a fresh UUID4; `TokenSource::Generated`.
    - **Out of scope for 1.7:** system keychain primary lookup and `~/.bowerbird/config.toml` file fallback. Story 3.3 (Bearer token auth with keychain storage) wires the full chain: `keychain → env → file`. Story 1.7 ships the *validation* layer and a minimal-but-correct token source so REST ACs hold today; 3.3 swaps in the issuance + storage chain without changing the validation surface.
  - [x] Wire it into `crates/daemon/src/main.rs::run`: call `token::load_or_generate()` **after** `init_tracing` and **before** any router construction. If `TokenSource::Generated`, log at WARN (mirror the bind-addr WARN pattern from `main.rs:130-132`) — the operationally important fact ("the daemon generated an ephemeral token; if you have no `BOWERBIRD_TOKEN`, you cannot make authenticated calls without reading this log line") must be visible at the default `error` verbosity. **Do NOT** log the token value itself; log only `"daemon generated ephemeral bearer token; set $BOWERBIRD_TOKEN to control it (see docs)"`.
  - [x] **Critical security invariant** (`architecture.md:444-446`): the ingest path **never** reads the bearer token. Verify by inspection that `crates/daemon/src/ingest/listener.rs` and `crates/daemon/src/ingest/handler.rs` do not import `BearerToken` or `crate::api::token`. Add a brief grep-style comment at the top of `api/token.rs` reminding future contributors of this rule.

- [x] **Task 2: Update `AppState` with the new fields** (AC: #3, #4, #7)
  - [x] Modify `crates/daemon/src/state.rs`:
    ```rust
    pub struct AppState {
        pub db: DbPools,
        pub migrations_complete: Arc<AtomicBool>,
        pub shutdown: CancellationToken,
        pub bearer: BearerToken,         // NEW — added by Story 1.7
        pub started_at_ms: i64,          // NEW — for /status uptime
    }
    ```
  - [x] `BearerToken: Clone` so `AppState: Clone` continues to hold (axum routers require it).
  - [x] `started_at_ms` is set to `current_unix_millis()` (same helper as `projection::session::current_unix_millis`) at daemon startup, BEFORE `AppState` is constructed. The value is immutable for the daemon's lifetime; cloning `AppState` propagates the value unchanged.
  - [x] Update every `AppState { ... }` construction site: `crates/daemon/src/main.rs::run` (one site) and every test fixture in `crates/daemon/tests/contract_daemon.rs` that constructs `AppState` (e.g. `readyz_returns_503_before_migrations_complete`, `healthz_returns_200_immediately`). Add a small test helper `fn make_test_state(pools: DbPools, migrations_complete: Arc<AtomicBool>) -> AppState` if the construction noise gets repetitive — but keep it scoped to the test file, do not put it in `bowerbird_daemon::state`.
  - [x] No new `Error` variants are needed. Existing `Error::Pool`, `Error::Sqlite`, `Error::Clock` cover all new failure paths.

- [x] **Task 3: Implement bearer auth middleware** (AC: #7)
  - [x] Create `crates/daemon/src/api/auth.rs` exporting `pub async fn require_bearer<B>(State(state): State<AppState>, req: Request<B>, next: Next) -> Response`. Implementation outline:
    ```rust
    use axum::extract::{Request, State};
    use axum::http::StatusCode;
    use axum::middleware::Next;
    use axum::response::{IntoResponse, Json, Response};
    use serde_json::json;

    pub async fn require_bearer(
        State(state): State<AppState>,
        req: Request,
        next: Next,
    ) -> Response {
        let header = req.headers().get(axum::http::header::AUTHORIZATION);
        let candidate = match header.and_then(|h| h.to_str().ok()) {
            Some(s) => s.strip_prefix("Bearer ").map(str::trim),
            None => None,
        };
        let ok = match candidate {
            Some(c) if !c.is_empty() => state.bearer.verify(c),
            _ => false,
        };
        if ok {
            next.run(req).await
        } else {
            (
                StatusCode::UNAUTHORIZED,
                Json(json!({ "error": "unauthorized" })),
            ).into_response()
        }
    }
    ```
  - [x] Use `axum::middleware::from_fn_with_state` to wire it. The router shape becomes:
    ```rust
    // crates/daemon/src/api/mod.rs
    let unauthenticated = Router::new()
        .route("/healthz", get(health::healthz))
        .route("/readyz",  get(health::readyz));

    let authenticated = Router::new()
        .route("/sessions",                      get(sessions::list))
        .route("/sessions/{id}",                 get(sessions::detail))
        .route("/sessions/{id}/events",          get(events::list))
        .route("/sessions/{id}/stats",           get(sessions::stats))
        .route("/status",                        get(status::get))
        .route_layer(axum::middleware::from_fn_with_state(state.clone(), auth::require_bearer));

    Router::new()
        .merge(unauthenticated)
        .merge(authenticated)
        .with_state(state)
    ```
    **Note:** axum 0.8 uses `{id}` not `:id` for path params (changed in 0.8). Verify by inspection at `crates/daemon/src/api/mod.rs` if any existing route uses `:id` — none currently exist, but architecture.md text still says `:id`; the implementation must use `{id}`. Add a one-line dev note in api/mod.rs explaining axum 0.8 path-param syntax for the next contributor.
  - [x] **Timing-safe comparison** is non-negotiable. `subtle::ConstantTimeEq::ct_eq(left, right)` returns a `Choice` (u8 wrapper); call `.into()` to get `bool`. Wrong-length tokens still take O(min(left.len(), right.len())) time — this is what `subtle` provides. Do NOT short-circuit on length mismatch outside `subtle`'s contract — the entire comparison must run the same number of cycles regardless of token shape.
  - [x] Add `tower-http::trace::TraceLayer::new_for_http()` and `tower_http::request_id::SetRequestIdLayer` + `PropagateRequestIdLayer` **out of scope for 1.7** — those land in a future hardening story per `project-context.md:495`. This story does *not* expand the middleware chain beyond the bearer auth layer.

- [x] **Task 4: Add `/sessions` list endpoint** (AC: #3)
  - [x] Add protocol type to `crates/protocol/src/rest.rs`:
    ```rust
    use crate::state::{SessionCurrentState, SessionState};

    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct SessionListItem {
        pub source: String,
        pub session_id: String,
        pub current_state: SessionCurrentState,
        pub last_event_kind: crate::EventKind,
        pub last_event_at_ms: i64,
        pub updated_at: i64,
    }
    ```
    No `#[serde(deny_unknown_fields)]` — this is an outbound type; the asymmetric serde rule (`architecture.md:606-608`) requires permissive deserialization. Add a snapshot test in `crates/protocol/tests/contract_protocol.rs` round-tripping a `SessionListItem` with an extra unknown JSON field, asserting parse success (mirrors Story 1.6's pattern for `SessionState`).
  - [x] Update `crates/protocol/src/lib.rs` re-exports: add `SessionListItem` next to the existing `EventListResponse, SessionStats`.
  - [x] Add SQL constant `SELECT_NON_SENTINEL_SESSIONS` in `crates/daemon/src/db/queries.rs`:
    ```rust
    pub const SELECT_NON_SENTINEL_SESSIONS: &str =
        "SELECT source, session_id, state, updated_at FROM session_projections \
         WHERE source != '__daemon__' \
         ORDER BY updated_at DESC, source ASC, session_id ASC";
    ```
    The `__daemon__` literal is duplicated from `projection::session.rs::DAEMON_SENTINEL_SOURCE`. **Do not** import the constant into `queries.rs` (that would couple the SQL strings module to the projection module). Add a single-line comment at the SQL site cross-referencing `projection::session::DAEMON_SENTINEL_SOURCE` so a future rename of the sentinel breaks loudly in code review.
  - [x] Create `crates/daemon/src/api/sessions.rs` with `pub async fn list(State(state): State<AppState>) -> Response`:
    1. Acquire a reader-pool connection via `state.db.reader.get().await`. On pool error, return `500 { "error": "<sanitized>" }` (do not leak internal pool error text; log the original via `tracing::error!` and surface a generic message).
    2. Inside `conn.interact`, prepare `SELECT_NON_SENTINEL_SESSIONS` and `query_map` into rows of `(String /* source */, String /* session_id */, String /* state JSON */, i64 /* updated_at */)`.
    3. For each row: deserialize `state` via `serde_json::from_str::<SessionState>()`. On parse error, log at `error` level and **skip the row** (do not 500 the whole list — one bad projection row should not blank the entire response). This matches the same defensive policy in `projection::session::write` for stored-state deserialization (Story 1.6 Task 3).
    4. Compute `current_state = projection::state::current_state_for_read(&stored, now_ms)` for each row.
    5. Construct `SessionListItem { source, session_id, current_state, last_event_kind: stored.last_event_kind, last_event_at_ms: stored.last_event_at_ms, updated_at }`.
    6. Return `Json(items)` (axum auto-serializes to `application/json`).
  - [x] **Read-time stale fallback (AC #3 invariant)**: this is the single place where Story 1.6's `current_state_for_read` is wired into a public surface. Story 1.6 added the pure function; Story 1.7 wires the first caller. Do NOT mutate the stored row.
  - [x] **Pagination:** out of scope for V1. Even at 10k sessions the response is ~1MB JSON — well under any reasonable limit for a developer-tool. Track as deferred work for a future "many sessions" hardening pass.

- [x] **Task 5: Add `/sessions/:id` detail endpoint** (AC: #4)
  - [x] Add protocol type `SessionDetail` to `crates/protocol/src/rest.rs`:
    ```rust
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct SessionDetail {
        pub source: String,
        pub session_id: String,
        pub state: SessionState,         // includes current_state, last_event_kind, last_event_at_ms
        pub updated_at: i64,
    }
    ```
    Re-export from `lib.rs`. Add round-trip snapshot test in `contract_protocol.rs`.
  - [x] Add SQL constant `SELECT_SESSION_BY_ID` in `queries.rs`:
    ```rust
    pub const SELECT_SESSION_BY_ID: &str =
        "SELECT source, session_id, state, updated_at FROM session_projections \
         WHERE session_id = ? AND source != '__daemon__' \
         ORDER BY updated_at DESC LIMIT 1";
    ```
    The `ORDER BY updated_at DESC LIMIT 1` handles the hypothetical `(source, session_id)` collision case (two sources, same `session_id`) by returning the most-recently-updated row. V1 only has the `"claude"` source, so the ordering never matters in practice. Document this in a comment at the SQL site.
  - [x] In `crates/daemon/src/api/sessions.rs`, add `pub async fn detail(State(state): State<AppState>, Path(id): Path<String>) -> Response`:
    1. Reader-pool checkout.
    2. `interact` → `query_row(SELECT_SESSION_BY_ID, [&id])` → tuple `(String, String, String, i64)`. If `QueryReturnedNoRows`, return `404 { "error": "session not found" }`. Any other rusqlite error → `500` (log).
    3. Deserialize `state` JSON to `SessionState`. On parse error: log at `error` and return `500` (a single-row corruption is not silent-skippable here the way it is in the list endpoint — the user explicitly asked for *this* row; surfacing 200-with-no-state would lie).
    4. Apply `current_state_for_read` to the stored state for the read-time fallback. **However**: `SessionDetail.state` is `SessionState`, not just `current_state` — the wire shape exposes `last_event_kind` and `last_event_at_ms` too. The cleanest implementation: construct a new `SessionState` where `current_state` is the fallback value but `last_event_kind` and `last_event_at_ms` are pulled verbatim from the stored row. This mirrors what `current_state_for_read` would return if it were structured to update an entire `SessionState` (and the function is intentionally narrower — see Story 1.6 rationale).
    5. Return `Json(SessionDetail { source, session_id, state: derived, updated_at })`.
  - [x] **Path-param disambiguation deferred-work entry:** add an entry to `deferred-work.md`: "REST `/sessions/{id}` does not include `source` in the path or as a query param; when a second adapter ships (Codex, OpenCode), the path needs to grow to `/sources/{source}/sessions/{id}` or accept `?source=` — Story 1.7 picks the most-recently-updated row as a stopgap." Include the file path and a one-line rationale.

- [x] **Task 6: Add `/sessions/:id/events?since=<cursor>` history endpoint** (AC: #5, #6)
  - [x] `protocol::EventListResponse` already exists with the right shape (`events: Vec<Event>, cursor: Option<EventId>, oldest_available_event_id: EventId`) — do NOT modify the wire type. Do verify that `protocol::Event` includes `created_at: i64` (NFR22 — already present, see `crates/protocol/src/event.rs:30-38`).
  - [x] Add SQL constants in `queries.rs`:
    ```rust
    pub const SELECT_EVENTS_FOR_SESSION_SINCE: &str =
        "SELECT event_id, source, session_id, kind, reaction, payload, created_at \
         FROM events \
         WHERE source != '__daemon__' AND session_id = ? AND event_id > ? \
         ORDER BY event_id ASC";

    pub const SELECT_MIN_EVENT_ID: &str =
        "SELECT MIN(event_id) FROM events WHERE source != '__daemon__'";
    ```
    The `MIN` query returns `Option<i64>`: `None` when the events table is empty (or only has sentinels); the daemon then surfaces `i64::MAX` per the protocol contract (`architecture.md:427`).
  - [x] Create `crates/daemon/src/api/events.rs` with `pub async fn list(State(state): State<AppState>, Path(id): Path<String>, Query(params): Query<EventsParams>) -> Response`:
    1. Define a `#[derive(serde::Deserialize)] struct EventsParams { #[serde(default)] since: i64 }`. **Inbound type**: add `#[serde(deny_unknown_fields)]` per architecture.md:606 — this is a strict-inbound surface. Unknown query params produce a `400`. (Axum's `Query` extractor surfaces deserialization errors as `400` automatically.)
    2. Reader-pool checkout.
    3. `interact`:
        - Execute `SELECT_EVENTS_FOR_SESSION_SINCE` with params `[id, params.since]`. Map each row to `protocol::Event` (use the same field order as `SELECT_EVENT_BY_ID` for consistency).
        - Reconstruct `kind: EventKind` from the `kind` TEXT column. Reuse `event_kind_from_db_str` (added by Story 1.6 Task 6) — that function returns `Result<EventKind, String>`. If parsing fails, log at `error` and skip the row (same defensive policy as the list endpoint). Document why skip-not-500 is acceptable here: a row with an unparseable `kind` is a schema-drift bug, not user input — the user asked for "events for this session"; surfacing the parseable rows is more useful than 500-blanking the response.
        - Reconstruct `reaction: Option<Reaction>` from the `reaction` TEXT column (NULLABLE). Use `serde_json::from_str(&format!("\"{}\"", s))` if `s` is non-NULL, else `None`. Or — preferred — add an inverse `reaction_from_db_string(s: &str) -> Result<Reaction, String>` helper next to `reaction_as_db_string` in `queries.rs`. This is a parallel inverse to `event_kind_from_db_str` (Story 1.6 Task 6). The pattern is now established; mirror it.
        - Execute `SELECT_MIN_EVENT_ID`. Map `Some(min) → EventId(min)`, `None → EventId(i64::MAX)`.
        - Compute `cursor = events.last().map(|e| e.event_id)`.
    4. Return `Json(EventListResponse { events, cursor, oldest_available_event_id })`.
  - [x] **Cursor semantics:** `cursor = Some(events.last().event_id)` when events non-empty; `None` when empty. Presenters use it as the next `?since=`. This matches the standard "tailing cursor" idiom and is the implicit contract from `architecture.md:142-144` (the type exists; this story defines its semantics). Document the contract in a Rust doc-comment on `EventListResponse` in `crates/protocol/src/rest.rs` so future authors don't reinvent it.
  - [x] **Page size:** no internal limit in V1. The single-developer load profile and SQLite read performance mean even 100k events serializes in under a second. Track as deferred-work for the "many events" hardening pass: "Add `&limit=` query param + `cursor = Some(last_returned_id)` when the limit is reached, else `None`."
  - [x] **Per-session vs global `oldest_available_event_id`?** Global. The protocol contract (`architecture.md:142-145`) is about the entire event log; a presenter holding a cursor for session A wants to know "is event_id 10 still on disk anywhere?" not "is event_id 10 still on disk for session A?" Truncation policy is global (delete-the-DB-or-bust in V1; `bowerbird gc` post-V1), so the answer is also global. Document this in a Rust doc-comment on `EventListResponse.oldest_available_event_id`.

- [x] **Task 7: Add `/sessions/:id/stats` endpoint** (AC: #8)
  - [x] `protocol::SessionStats` already exists with the right shape (`source, session_id, event_count, first_event_at, last_event_at`). Do NOT modify the wire type.
  - [x] Add SQL constant `SELECT_STATS_FOR_SESSION` in `queries.rs`:
    ```rust
    pub const SELECT_STATS_FOR_SESSION: &str =
        "SELECT source, COUNT(*) as event_count, MIN(created_at) as first_event_at, \
                MAX(created_at) as last_event_at \
         FROM events \
         WHERE source != '__daemon__' AND session_id = ? \
         GROUP BY source \
         ORDER BY MAX(created_at) DESC LIMIT 1";
    ```
    Returns `(String /* source */, i64, Option<i64>, Option<i64>)`. `MIN`/`MAX` on `created_at` are `NULL` only when no rows match the WHERE — which is filtered out by `GROUP BY ... LIMIT 1` producing zero result rows. So in practice `first_event_at` and `last_event_at` are non-NULL when the query returns a row. The Option<i64> in `SessionStats` is for the *case where the projection exists but the events table has been purged* — a far-future scenario that V1 doesn't address; the wire type carries it forward for forward-compat.
  - [x] In `crates/daemon/src/api/sessions.rs`, add `pub async fn stats(State(state): State<AppState>, Path(id): Path<String>) -> Response`:
    1. Reader-pool checkout.
    2. `interact` → `query_row(SELECT_STATS_FOR_SESSION, [&id])`. `QueryReturnedNoRows` → `404`. Other errors → `500`.
    3. Return `Json(SessionStats { source, session_id: id, event_count, first_event_at, last_event_at })`.
  - [x] **404 semantics consistency:** `/sessions/:id` returns 404 from `session_projections`; `/sessions/:id/stats` returns 404 from `events`. These two tables can diverge transiently (the projection writes happen first inside the transaction, then the event INSERT — see `projection::session::write` Task 3 of Story 1.6). The probability of a request landing exactly between the two writes is ~zero on a single-writer pool, but the inconsistency is theoretically observable. Document this in a Dev Note: the two 404 sources are not synchronized; the answer is "consult the projection (`/sessions/:id`) if you need authoritative session existence; consult stats only after that returns 200."

- [x] **Task 8: Add `/status` endpoint** (AC: ancillary — listed in epic AC #7 endpoint list, no specific behavioral AC; matches PRD line 356)
  - [x] Add protocol type `DaemonStatus` to `crates/protocol/src/rest.rs`:
    ```rust
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct DaemonStatus {
        pub daemon_version: String,        // env!("CARGO_PKG_VERSION") of the daemon
        pub protocol_version: String,      // pinned at "1.0" for V1
        pub started_at_ms: i64,            // from AppState.started_at_ms
        pub uptime_ms: i64,                // now_ms - started_at_ms
        pub last_event_at_ms: Option<i64>, // MAX(created_at) from events WHERE source != '__daemon__'; None if no non-sentinel events
        pub last_event_id: Option<crate::EventId>,
        // Reserved for Epic 2's WS surface: connected_ws_clients: usize. Omit from V1.
    }
    ```
    Re-export from `lib.rs`. Add round-trip snapshot test in `contract_protocol.rs` including an extra unknown JSON field.
  - [x] Add SQL constant `SELECT_LAST_EVENT` in `queries.rs`:
    ```rust
    pub const SELECT_LAST_EVENT: &str =
        "SELECT event_id, created_at FROM events \
         WHERE source != '__daemon__' \
         ORDER BY event_id DESC LIMIT 1";
    ```
    Returns `Option<(i64, i64)>` — `None` when the events table is empty or only has sentinels.
  - [x] Create `crates/daemon/src/api/status.rs` with `pub async fn get(State(state): State<AppState>) -> Response`:
    1. Reader-pool checkout.
    2. Compute `now_ms = current_unix_millis()` (reuse the projection helper — if it's not exposed publicly today, move it to `crates/daemon/src/db/queries.rs` or a new `crates/daemon/src/time.rs` so both modules can use it. Do NOT duplicate the function — that's a "two clocks divergence" bug waiting to happen).
    3. `interact` → optional `(event_id, created_at)` via `SELECT_LAST_EVENT`. Map to `(Option<EventId>, Option<i64>)`.
    4. Return `Json(DaemonStatus { daemon_version: env!("CARGO_PKG_VERSION").to_string(), protocol_version: "1.0".to_string(), started_at_ms: state.started_at_ms, uptime_ms: now_ms - state.started_at_ms, last_event_at_ms, last_event_id })`.
  - [x] Add `pub mod status;` to `crates/daemon/src/api/mod.rs`.
  - [x] **Where does `current_unix_millis` live now?** Currently a private fn at `crates/daemon/src/projection/session.rs:187-194`. Story 1.7 needs to share it between `projection` and `api::status`. Cleanest move: extract to `crates/daemon/src/time.rs` as a `pub(crate) fn current_unix_millis() -> Result<i64>`, update the one existing call site, and let `api::status` and the auth init path both use it. **Do NOT** add a new dep (`chrono`, `time`) — the existing `SystemTime::now()` approach is the project pattern (Story 1.5 Dev Notes explicitly justify this; ditto Story 1.6 Anti-Patterns).

- [x] **Task 9: Strengthen `/readyz` with DB probe** (AC: #2)
  - [x] Modify `crates/daemon/src/api/health.rs::readyz`:
    ```rust
    pub async fn readyz(State(state): State<AppState>) -> impl IntoResponse {
        if !state.migrations_complete.load(Ordering::Acquire) {
            return (StatusCode::SERVICE_UNAVAILABLE,
                    Json(json!({ "error": "not ready" }))).into_response();
        }
        match probe_db(&state.db.reader).await {
            Ok(()) => (StatusCode::OK, Json(json!({ "status": "ready" }))).into_response(),
            Err(_) => (StatusCode::SERVICE_UNAVAILABLE,
                       Json(json!({ "error": "not ready" }))).into_response(),
        }
    }

    async fn probe_db(reader: &deadpool_sqlite::Pool) -> Result<(), ()> {
        let conn = reader.get().await.map_err(|_| ())?;
        let _ = conn
            .interact(|c| c.query_row::<i64, _, _>("SELECT 1 FROM events WHERE 1=0", [], |r| r.get(0)))
            .await
            .map_err(|_| ())?;
        // Note: `query_row` returns QueryReturnedNoRows here, which is expected;
        // any other error means the DB is unreachable or the schema is broken.
        Ok(())
    }
    ```
    The `WHERE 1=0` literal makes the query exit before scanning any rows — sub-millisecond on any DB size. The probe validates: (a) pool checkout succeeds within the 5s timeout; (b) the connection is alive; (c) the `events` table exists (would fail with "no such table" otherwise — catches a corrupt-schema state that bare `SELECT 1` would miss).
  - [x] **Story 1.2 deferred-work item resolution:** strike line 32 in `docs/bmad/implementation-artifacts/deferred-work.md` (`/readyz` does not probe the database) with a backlink to the Story 1.7 commit. Use the same strike convention used by Story 1.6 (Task 12 of 1.6 strikes the WAL-durability surrogate entry).
  - [x] **Shutdown drain invariant** (`crates/daemon/src/main.rs:152-154`): the daemon flips `migrations_complete.store(false)` during graceful shutdown so a probe in flight observes 503. With the new DB-probe layer, this still works because the migrations_complete check runs FIRST — drain semantics are preserved.
  - [x] **What `/readyz` does NOT probe:** broadcaster initialization (Epic 2), socket binding (Epic 1.3 already enforces), config validity. The probe is intentionally narrow: "DB is reachable; schema is sane; migrations have applied." Anything else belongs in `/status` or a future `/checks` endpoint.

- [x] **Task 10: Wire the new router** (AC: #1, #3, #4, #5, #6, #7, #8)
  - [x] Replace `crates/daemon/src/api/mod.rs` body with the merged unauthenticated/authenticated router (see Task 3 layout). Top of the file:
    ```rust
    pub mod auth;
    pub mod events;
    pub mod health;
    pub mod sessions;
    pub mod status;
    pub mod token;

    use axum::routing::get;
    use axum::Router;

    use crate::state::AppState;

    pub fn router(state: AppState) -> Router {
        let unauthenticated = Router::new()
            .route("/healthz", get(health::healthz))
            .route("/readyz",  get(health::readyz));

        let authenticated = Router::new()
            .route("/sessions",                       get(sessions::list))
            .route("/sessions/{id}",                  get(sessions::detail))
            .route("/sessions/{id}/events",           get(events::list))
            .route("/sessions/{id}/stats",            get(sessions::stats))
            .route("/status",                         get(status::get))
            .route_layer(axum::middleware::from_fn_with_state(state.clone(), auth::require_bearer));

        Router::new()
            .merge(unauthenticated)
            .merge(authenticated)
            .with_state(state)
    }
    ```
  - [x] **Axum path-param syntax (axum 0.8):** `/sessions/{id}` (curly braces). Architecture text uses `:id`; that is axum 0.7 syntax. Pin the curly-brace form by inspection at build time — if `cargo check` fails with "invalid route pattern", the syntax is wrong.

- [x] **Task 11: Contract test — `/healthz` smoke under new router shape** (AC: #1)
  - [x] Keep the existing `healthz_returns_200_immediately` test (`crates/daemon/tests/contract_daemon.rs:382`). Add an assertion that the JSON body is `{ "status": "ok" }` — the existing test only checks status code, not body shape. Use `axum::body::to_bytes` to read the response body and compare.

- [x] **Task 12: Contract test — `/readyz` DB-probe behavior** (AC: #2)
  - [x] Keep the existing `readyz_returns_503_before_migrations_complete` test as-is — it still validates the migrations branch.
  - [x] Add `readyz_returns_503_when_db_unreachable`:
    1. `fresh_pools()` → fresh DB, then set `migrations_complete = true`.
    2. **How to simulate "DB unreachable" without taking down the whole tempfile?** Two options:
       - **Option A (preferred):** point the daemon at a deliberately-bad path. Use a fresh `init_pools` with a path that exists but is not a SQLite DB (e.g., write 16 bytes of garbage to `tmp/junk.db`). On first probe, `SELECT 1 FROM events WHERE 1=0` errors with "file is not a database" — `probe_db` returns Err — `/readyz` returns 503.
       - **Option B:** exhaust the reader pool (4 outstanding `pool.get()` holders) and rely on the 5s wait timeout. Test runtime ≈ 5s — too slow for the PR test loop.
    3. Use Option A. Build a corrupted-DB path, construct an `AppState` against it, hit `/readyz`, assert 503.
    4. Document in the test why Option A is preferred over Option B.

- [x] **Task 13: Contract test — `/sessions` listing + read-time fallback** (AC: #3)
  - [x] `sessions_list_returns_known_sessions_with_read_time_state`:
    1. `fresh_pools()`. Insert two envelopes for `("claude", "sess-a")` (PreToolUse → stored Working) and `("claude", "sess-b")` (PostToolUse → stored Idle).
    2. Insert a sentinel via `write_recording_started` to verify it's filtered out.
    3. `GET /sessions` with a valid bearer. Assert: response is a 2-element JSON array (sentinel excluded); both items have `source == "claude"`; first item is `sess-b` (more recent `updated_at`? or `sess-a`? — depends on insertion order; assert by sort order, not array position).
    4. Verify `current_state` per item. For sess-a (Working, fresh), `current_state == "Working"`. For sess-b (Idle), `current_state == "Idle"`.
  - [x] `sessions_list_applies_stale_working_fallback`:
    1. `fresh_pools()`. Write a `PreToolUse` for `("claude", "sess-old")` then manually UPDATE the `last_event_at_ms` field inside the stored JSON to a value `STALE_WORKING_MS + 1` in the past. (Or: write the event, sleep 5min — DO NOT do this; deterministic-test discipline forbids real sleep. The manual JSON tweak is the right pattern.)
    2. `GET /sessions`. Assert the item for sess-old has `current_state == "Idle"` (stale-Working fallback applied at read time).
    3. **However**, that test reaches into the stored JSON to age it artificially — an alternative is to expose `current_state_for_read` as a unit test in `projection/state.rs` (Story 1.6 already does this) and trust that the integration test catches only the wiring. Pragmatic choice: do the JSON tweak in this integration test so the AC ("the handler calls current_state_for_read") is genuinely covered, not just unit-tested.

- [x] **Task 14: Contract test — `/sessions/:id` detail + 404** (AC: #4)
  - [x] `sessions_detail_returns_projection_state`:
    1. `fresh_pools()`. Write a PreToolUse for `("claude", "sess-x")`.
    2. `GET /sessions/sess-x`. Assert 200; body parses as `SessionDetail`; `state.current_state == "Working"`; `state.last_event_kind == "PreToolUse"`.
  - [x] `sessions_detail_returns_404_when_unknown`:
    1. `fresh_pools()` (empty).
    2. `GET /sessions/does-not-exist`. Assert 404; body is `{ "error": "session not found" }`.

- [x] **Task 15: Contract test — `/sessions/:id/events?since=<cursor>`** (AC: #5)
  - [x] `events_list_returns_all_in_ascending_order`:
    1. `fresh_pools()`. Write 5 PreToolUse events for `("claude", "sess-y")`.
    2. `GET /sessions/sess-y/events?since=0` with valid bearer. Assert 200; `events.len() == 5`; ascending by `event_id`; each row carries `created_at` (NFR22 surface check); `cursor == Some(events[4].event_id)`; `oldest_available_event_id == events[0].event_id`.
  - [x] `events_list_returns_empty_with_none_cursor`:
    1. `fresh_pools()`. Do NOT write any non-sentinel events.
    2. `GET /sessions/sess-y/events?since=0`. Assert 200; `events.len() == 0`; `cursor == None`; `oldest_available_event_id == EventId(i64::MAX)`.
  - [x] `events_list_respects_since_cursor`:
    1. `fresh_pools()`. Write 10 events for sess-y (event_ids 2..=11 after the startup sentinel takes event_id 1).
    2. `GET /sessions/sess-y/events?since=6`. Assert events returned have `event_id > 6` (so 4 events); ascending order.

- [x] **Task 16: Contract test — gap-detection mechanical fact** (AC: #6)
  - [x] `events_list_oldest_available_after_purge`:
    1. `fresh_pools()`. Write 5 events for sess-y. Read the stored event_ids.
    2. Manually `DELETE FROM events WHERE event_id <= <middle>` against the writer pool — simulates a future `bowerbird gc` purge.
    3. `GET /sessions/sess-y/events?since=0`. Assert `oldest_available_event_id == <surviving_min>`. Assert `events.len()` matches the surviving count.
    4. Verify the presenter can mechanically infer the gap: `assert!(0 < oldest_available_event_id)` — that's the Axiom-4-style inference, not a daemon-emitted flag.

- [x] **Task 17: Contract test — bearer auth (401 invariants)** (AC: #7)
  - [x] `authenticated_routes_reject_missing_header`:
    1. `fresh_pools()`. Build state with `BearerToken::new("test-token".to_string())`.
    2. For each authenticated route (`/sessions`, `/sessions/foo`, `/sessions/foo/events`, `/sessions/foo/stats`, `/status`): make a request with NO `Authorization` header; assert 401; assert body is `{ "error": "unauthorized" }`.
  - [x] `authenticated_routes_reject_wrong_bearer`:
    1. Same setup; bearer header `Authorization: Bearer wrong-token`; assert 401.
  - [x] `unauthenticated_routes_accept_missing_header`:
    1. `GET /healthz` and `GET /readyz` with no header; assert non-401 status (200 or 503 depending on readyz state).
  - [x] `authenticated_routes_accept_correct_bearer`:
    1. With `Authorization: Bearer test-token`, assert non-401 status on every authenticated route.
  - [x] `authenticated_routes_reject_empty_bearer`:
    1. `Authorization: Bearer ` (trailing space, empty token); assert 401. Tests the `is_empty()` guard in the middleware.
  - [x] `authenticated_routes_reject_wrong_scheme`:
    1. `Authorization: Basic dGVzdA==`; assert 401. Tests the `strip_prefix("Bearer ")` guard.
  - [x] **Timing-safe property (light-touch):** add a comment-only sanity check in `auth.rs` near the `subtle::ConstantTimeEq` call referencing the `subtle` crate as the authoritative implementation. A real timing-side-channel test would require statistical analysis (multiple runs, timing histogram) — out of scope. The crate choice IS the test.

- [x] **Task 18: Contract test — additive forward-compat on `SessionStats`** (AC: #8)
  - [x] Add a unit test to `crates/protocol/tests/contract_protocol.rs`:
    ```rust
    #[test]
    fn session_stats_accepts_unknown_fields() {
        let future_json = r#"{
            "source": "claude",
            "session_id": "sess-x",
            "event_count": 12,
            "first_event_at": 1000,
            "last_event_at": 2000,
            "tool_use_breakdown": { "Read": 5, "Bash": 7 }
        }"#;
        let parsed: protocol::SessionStats =
            serde_json::from_str(future_json).expect("forward-compat parse");
        assert_eq!(parsed.event_count, 12);
    }
    ```
    This asserts the asymmetric serde policy holds for `SessionStats`. Add equivalent unit tests for `SessionListItem`, `SessionDetail`, and `DaemonStatus` (the three new outbound types this story adds).

- [x] **Task 19: Protocol changelog entry** (AC: #3, #4, #8)
  - [x] Open `docs/protocol-changelog.md` (created by Story 1.6 Task 1). Add a new entry below the 1.6 schema entry, same `## v1.0 → v1.x` section header:
    ```
    - **Added:** `protocol::rest::SessionListItem` — list-shaped row for `GET /sessions` responses.
    - **Added:** `protocol::rest::SessionDetail` — detail-shaped row for `GET /sessions/{id}` responses.
    - **Added:** `protocol::rest::DaemonStatus` — body shape for `GET /status` responses.
    - **Endpoints:** `GET /sessions`, `GET /sessions/{id}`, `GET /sessions/{id}/events?since=<cursor>`, `GET /sessions/{id}/stats`, `GET /status` are now live and require a bearer token. `GET /healthz` and `GET /readyz` remain unauthenticated.
    - **Tightened:** `GET /readyz` now also probes the database (`SELECT 1 FROM events WHERE 1=0`) — a 200 implies migrations applied AND DB reachable. Previously only the migrations branch was checked.
    ```
    Type: `schema` for the first three; type: `behavioral` for the `/readyz` line. Use whatever entry format Story 1.6 established.

- [x] **Task 20: Sprint hygiene — strike resolved deferred-work entries**
  - [x] Open `docs/bmad/implementation-artifacts/deferred-work.md`.
  - [x] Find the line: "`/readyz` does not probe the database" (Story 1.2 review, currently line 32). Strike it: `~~/readyz does not probe...~~ **Resolved by Story 1.7 (Task 9):** hybrid `migrations_complete && db_probe_ok` per AC #2.` Use the same convention as Story 1.6.
  - [x] Add a new entry under a new section header `## Deferred from: Story 1.7 (REST query API) (2026-05-20)`:
    - **`/sessions/{id}` lacks `source` in path or query** — V1 picks most-recently-updated row by natural key; multi-source disambiguation needed when a second adapter (Codex, OpenCode) ships. Path candidates: `/sources/{source}/sessions/{id}` (REST nesting) or `?source=` (query param). [`crates/daemon/src/api/sessions.rs::detail`]
    - **No page-size limit on `GET /sessions/{id}/events`** — V1 returns the entire history slice. At ~100k events the response is ~10MB and the SQLite query holds a reader for the duration. Add a `&limit=` query param + `cursor = Some(last_returned_id)` semantics when the limit is reached. [`crates/daemon/src/api/events.rs::list`]
    - **No pagination on `GET /sessions`** — same shape; ~10k sessions becomes ~1MB JSON. [`crates/daemon/src/api/sessions.rs::list`]
    - **`/status.connected_ws_clients` not included** — Epic 2's WebSocket surface owns this counter; the field is reserved in the API contract but unused in V1. Add when Epic 2 lands. [`crates/protocol/src/rest.rs::DaemonStatus`]
    - **Token issuance + keychain integration deferred to Story 3.3** — V1 reads `BOWERBIRD_TOKEN` env var or generates an ephemeral UUID4 logged at WARN. Story 3.3 wires the full `keychain → env → file` chain. [`crates/daemon/src/api/token.rs::load_or_generate`]
    - **No request-id middleware, no `TraceLayer`, no per-request timeout, no body-size limit** — `project-context.md:495-497` calls these out as required framework infrastructure. Out of scope for 1.7; add in a future hardening pass. [`crates/daemon/src/api/mod.rs`]
    - **Inconsistent 404 source between `/sessions/{id}` and `/sessions/{id}/stats`** — the former 404s on `session_projections`, the latter on `events`. Transient mid-transaction window can produce divergence. Document the contract: `/sessions/{id}` is authoritative for existence. [Task 7 dev note]
    - **Bearer-token timing-safe compare unverified by integration test** — relies on `subtle::ConstantTimeEq` crate guarantee. A statistical timing test would land in a future security-hardening story. [`crates/daemon/src/api/auth.rs::require_bearer`]

- [x] **Task 21: Final checks**
  - [x] `cargo fmt --check` — green
  - [x] `cargo clippy --all-targets --workspace -- -D warnings` — green
  - [x] `cargo test --workspace` — all tests pass. Expected: ~4 protocol-level (round-trip tests for SessionListItem, SessionDetail, DaemonStatus, SessionStats forward-compat) + ~14 daemon contract tests (one per Task 11-17 block, multiple per block).
  - [x] `cargo build --workspace` — zero warnings.
  - [x] `grep -rn 'subtle::ConstantTimeEq\|secrecy::ExposeSecret' crates/daemon/src/` — should appear ONLY in `api/auth.rs` and `api/token.rs`. If grep shows otherwise, fix; the token must not leak into handlers.
  - [x] `grep -rn 'BearerToken\|bearer\b' crates/daemon/src/ingest/` — should produce ZERO matches. The ingest path **never** reads the bearer token (architecture.md:444-446 invariant; this is a release blocker).
  - [x] Manually start the daemon (`HOME=$(mktemp -d) cargo run -p bowerbird-daemon -- -vv`). Read the bind address and token from logs. With `curl`:
    ```
    curl -i http://127.0.0.1:<port>/healthz
    curl -i http://127.0.0.1:<port>/readyz
    curl -i -H "Authorization: Bearer <token>" http://127.0.0.1:<port>/sessions
    curl -i -H "Authorization: Bearer wrong" http://127.0.0.1:<port>/sessions   # 401
    curl -i http://127.0.0.1:<port>/status                                       # 401
    ```
    Document expected output in a one-line code comment somewhere obvious if useful, or omit (manual smoke-test, not a perpetual deliverable).
  - [x] Verify `protocol-changelog.md` has the entry from Task 19.
  - [x] Verify `deferred-work.md` has Story 1.2 line 32 struck AND the new Story 1.7 deferred items appended (Task 20).

## Dev Notes

### Story scope at a glance

This story does ONE thing in two layers:

1. **Protocol layer:** publish three new outbound types — `SessionListItem`, `SessionDetail`, `DaemonStatus` — that REST clients deserialize. `EventListResponse` and `SessionStats` already exist from Story 1.1; this story is their first wire-up to handlers.

2. **Daemon layer:** ship five new REST endpoints (`/sessions`, `/sessions/{id}`, `/sessions/{id}/events`, `/sessions/{id}/stats`, `/status`) behind a bearer-token middleware; tighten `/readyz` to also probe the DB; leave `/healthz` alone.

The story does **NOT** ship:
- WebSocket (Epic 2)
- Keychain-backed token storage or `bowerbird auth token` CLI (Story 3.3)
- `bowerbird install`/`uninstall` (Story 3.1)
- The shim, ingest, projection, or recording-sessions schema work (Stories 1.1-1.6, all done or ready-for-dev)

### Wire contract reference

The bearer-token surface, response shapes, and HTTP codes for this story:

| Endpoint | Auth | Path / Query | Body shape | 200 | 401 | 404 | 503 |
|---|---|---|---|---|---|---|---|
| `GET /healthz` | none | — | `{ "status": "ok" }` | always | n/a | n/a | n/a |
| `GET /readyz` | none | — | `{ "status": "ready" }` / `{ "error": "not ready" }` | migrations done + DB reachable | n/a | n/a | otherwise |
| `GET /status` | bearer | — | `DaemonStatus` | always (auth permitting) | missing/bad bearer | n/a | n/a |
| `GET /sessions` | bearer | — | `Vec<SessionListItem>` | always (auth permitting) | missing/bad bearer | n/a | n/a |
| `GET /sessions/{id}` | bearer | `{id}` | `SessionDetail` | session found | missing/bad bearer | unknown id | n/a |
| `GET /sessions/{id}/events` | bearer | `{id}`, `?since=<i64>` (default 0) | `EventListResponse` | always (auth permitting) | missing/bad bearer | n/a | n/a |
| `GET /sessions/{id}/stats` | bearer | `{id}` | `SessionStats` | session has events | missing/bad bearer | no events for id | n/a |

**Body invariant:** the HTTP error body is exactly `{ "error": "<message>" }` — no `code` field, no nested object, no additional keys (`architecture.md:598-599`). Stick to this on every non-200 response.

**Success body invariant:** axum's `Json(value)` serializer; no envelope (`{ "data": ... }`) wrapping. The response body IS the typed value.

### Authentication response shape: 401 body invariant

The 401 body is `{ "error": "unauthorized" }` regardless of whether:
- the `Authorization` header is missing
- the header is present but malformed
- the bearer scheme is wrong (`Basic`, `Digest`, etc.)
- the token is wrong-length
- the token is the right length but wrong bytes

This is a **deliberate** choice. Distinct error messages for "missing" vs "wrong" leak information about the auth surface (e.g., "the daemon parses your header but rejects your token" tells an attacker something useful). One opaque message; same status code; same body shape; same approximate response timing (the constant-time compare ensures this for the wrong-bytes case). Document this in a code comment in `auth.rs`.

### Read-time stale fallback wiring (Story 1.6 callback)

Story 1.6 added `projection::state::current_state_for_read(stored, now_ms) -> SessionCurrentState`. Story 1.7 is the first caller. The contract:

- `GET /sessions` calls it for every row, then constructs `SessionListItem` with the fallback value.
- `GET /sessions/{id}` calls it once for the queried row, then constructs `SessionDetail` with a derived `SessionState` (where `current_state` is the fallback, but `last_event_kind` and `last_event_at_ms` are stored-row verbatim).
- `GET /sessions/{id}/events` and `GET /sessions/{id}/stats` do NOT call it — they surface the event log, not the projection.
- `GET /status` does NOT call it — `last_event_at_ms` is a wall-clock fact from the events table, not a projection value.

**Do NOT** mutate the stored projection row during a read handler. The Story 1.6 contract is "stored = pure function of events; surfaced = stale-checked view." Mutating at read time would break Story 1.6 AC #5 (rebuild byte-identity).

### Auth model: env-var + ephemeral UUID4 for V1, keychain in 3.3

For Story 1.7, the token chain is:

1. If `BOWERBIRD_TOKEN` is set and non-empty → use it; log at info ("bearer token loaded from env").
2. Else → generate a fresh UUID4 at startup; log at WARN ("daemon generated ephemeral bearer token; set $BOWERBIRD_TOKEN to control it"). The token value is **never logged**.

This is the minimum that makes 1.7's ACs hold (a token exists; auth works) without blocking on 3.3 (keychain integration, file fallback, `bowerbird auth token` CLI). When 3.3 lands, the resolution order becomes `keychain → env → file → generate-with-WARN`. The validation layer (`api::auth::require_bearer`, `BearerToken::verify`) does not change; only the issuance layer (`api::token::load_or_generate`) is swapped.

The architecture's full chain (`architecture.md:442`) is `keychain → env → file`. The "generate ephemeral" fallback is a Story 1.7 addition — without it, the daemon can't start cleanly in V1 (Story 3.3's fail-non-zero-when-no-token-resolvable behavior is also out of scope until 3.3 ships). Document the deviation in `protocol-changelog.md` as `type: behavioral` with a `Resolves-In: 3.3` annotation.

### Architecture compliance — what stays, what changes

This story changes:

- `crates/daemon/src/api/mod.rs` router shape — adds five new routes behind bearer auth, plus the auth middleware itself.
- `crates/daemon/src/state.rs` — adds `bearer: BearerToken` and `started_at_ms: i64`.
- `crates/daemon/src/api/health.rs::readyz` — strengthens with a DB probe.
- `crates/daemon/src/main.rs` — wires the token loader before `AppState` construction.
- `crates/daemon/src/db/queries.rs` — adds five new SQL constants and a `reaction_from_db_string` helper.
- `crates/protocol/src/rest.rs` — adds three new outbound types.
- `crates/protocol/src/lib.rs` — re-exports the new types.

This story does **NOT** change:

- `crates/shim/**` — shim doesn't participate in REST.
- `crates/adapter-claude/**` — adapter normalizes ingest events; REST is downstream.
- `crates/daemon/src/ingest/**` — ingest path is auth-free (Unix-socket 0600 enforces).
- `crates/daemon/src/projection/**` — Story 1.6 owns this; Story 1.7 reads from it.
- The SQLite schema — all new endpoints use existing tables (`events`, `session_projections`, `recording_sessions`).
- `protocol::Event`, `protocol::EventEnvelope`, `protocol::EventKind` — no new event kinds, no new envelope fields.
- The WebSocket surface — Epic 2.

### Library / framework requirements

| Dep | Version | Source | Use in 1.7 |
|---|---|---|---|
| axum | 0.8.9 | workspace | router, handlers, extractors, middleware |
| tower-http | 0.6.10 | workspace | (none added in 1.7; existing dep) |
| tower | 0.5.3 | workspace [dev-dependencies] | `ServiceExt::oneshot` in tests |
| serde | 1.0.228 | workspace | derive on new protocol types |
| serde_json | 1.0.149 | workspace | response body serialization, query parsing |
| secrecy | 0.10.3 | workspace (already pinned; new in daemon deps) | `SecretString` wrapping for `BearerToken` |
| **subtle** | **2.6** | **NEW workspace pin** | constant-time bearer token comparison |
| uuid | 1.23.1 | workspace | UUID4 token generation |
| rusqlite | 0.38.0 | workspace | inverse db-string helpers; SELECT queries |
| deadpool-sqlite | 0.13.0 | workspace | reader-pool checkout for all new endpoints |
| tracing | 0.1.44 | workspace | `instrument(skip_all, ...)` on every new handler |
| anyhow | 1.0.102 | workspace | binary-edge only (no new uses inside library code) |

**Adding `subtle`** is the only new workspace dep. Justification: constant-time comparison is a security primitive; hand-rolling it (`a == b` with manual byte-iteration tricks) is error-prone and compiler-defeated by autovectorization. `subtle` is the canonical Rust crate (used by `ring`, `dalek`, the entire RustCrypto org). It is no_std, zero-allocation, and adds ~0 KB to the binary. Document in the commit body.

**axum 0.8 path-param syntax:** routes use `{id}` not `:id`. The architecture text still says `:id` (axum 0.7); axum 0.8 made the breaking change. Confirm by reading the axum 0.8.9 changelog notes if helpful — but the build will tell you: `:id` compiles but does not match; `{id}` does. There are no `:id` routes in the daemon today (Story 1.2 only added `/healthz`, `/readyz`), so this is a Story 1.7 first-use of dynamic-path routing.

**Tokio current_thread runtime constraint** (`architecture.md:952-954`): all daemon work runs on a single OS thread. Every new handler is `async fn` and uses `.await` for pool checkouts. **Do NOT** spawn `std::thread::spawn` or call blocking SQLite work — `interact` on `deadpool-sqlite` is the canonical path; it runs the closure on a dedicated blocking pool internally.

**WebSocket layer impact:** zero. Story 1.7 does not touch `api/ws.rs` (doesn't exist yet; Epic 2 owns it). The router merge pattern (`unauthenticated` + `authenticated`) leaves room for `/ws` to land later in `authenticated` (it requires bearer auth on upgrade per `architecture.md:462`).

### File structure requirements

**Files to be created:**

```
crates/daemon/src/api/auth.rs
  # require_bearer middleware (from_fn_with_state)
  # constant-time compare via subtle::ConstantTimeEq

crates/daemon/src/api/token.rs
  # BearerToken (SecretString-wrapped, Clone)
  # TokenSource enum, load_or_generate()

crates/daemon/src/api/sessions.rs
  # list, detail, stats handlers
  # uses projection::state::current_state_for_read

crates/daemon/src/api/events.rs
  # list handler with ?since= cursor + oldest_available_event_id

crates/daemon/src/api/status.rs
  # /status handler; reads last event + uptime

crates/daemon/src/time.rs
  # current_unix_millis() — extracted from projection/session.rs to be shared
  # (Internal; pub(crate). No new crate, no chrono/time dep.)
```

**Files to be modified:**

```
crates/protocol/src/rest.rs
  # add SessionListItem, SessionDetail, DaemonStatus
  # Rust doc-comments on EventListResponse.cursor + .oldest_available_event_id

crates/protocol/src/lib.rs
  # re-export SessionListItem, SessionDetail, DaemonStatus

crates/protocol/tests/contract_protocol.rs
  # round-trip + additive-compat tests for the three new types
  # forward-compat canary on SessionStats (Task 18)

crates/daemon/src/api/mod.rs
  # new merged router (unauth + auth)
  # new pub mod declarations for auth, events, sessions, status, token

crates/daemon/src/api/health.rs
  # readyz now also probes the DB

crates/daemon/src/state.rs
  # AppState gains bearer: BearerToken and started_at_ms: i64

crates/daemon/src/main.rs
  # call token::load_or_generate() before AppState construction
  # log token source at info/WARN per source

crates/daemon/src/db/queries.rs
  # add SELECT_NON_SENTINEL_SESSIONS
  # add SELECT_SESSION_BY_ID
  # add SELECT_EVENTS_FOR_SESSION_SINCE
  # add SELECT_MIN_EVENT_ID
  # add SELECT_STATS_FOR_SESSION
  # add SELECT_LAST_EVENT
  # add reaction_from_db_string helper (inverse of reaction_as_db_string)

crates/daemon/src/projection/session.rs
  # current_unix_millis moves to crates/daemon/src/time.rs
  # update the one call site to `use crate::time::current_unix_millis`

crates/daemon/src/lib.rs
  # pub mod time;

crates/daemon/Cargo.toml
  # add secrecy = { workspace = true }
  # add subtle = "2.6"

Cargo.toml (workspace)
  # add subtle = "2.6" to [workspace.dependencies]

crates/daemon/tests/contract_daemon.rs
  # new contract tests for Tasks 11-17 (see task list)
  # update existing AppState fixtures to include bearer + started_at_ms

docs/protocol-changelog.md
  # add v1.0 → v1.x schema entries (Task 19)

docs/bmad/implementation-artifacts/deferred-work.md
  # strike Story 1.2 line 32 (Task 9 resolution)
  # add Story 1.7 deferred-work section (Task 20)
```

**Source tree alignment with architecture.md:806-813**

The architecture lists `api/auth.rs`, `api/token.rs`, `api/sessions.rs`, `api/events.rs`, `api/health.rs`, `api/ws.rs`. Story 1.7 creates all of those EXCEPT `api/ws.rs` (Epic 2). Plus `api/status.rs` — the architecture text doesn't enumerate a file for `/status`, but `/status` is listed in PRD line 356 and was implicitly placed in `api/sessions.rs` by reading too literally. Putting `/status` in its own file matches the one-file-per-resource pattern this story establishes; document in commit body if reviewers ask.

### Testing requirements

**Test placement** (`architecture.md:555-558`):

- Unit tests: `#[cfg(test)] mod tests` at the bottom of the file under test (e.g., `BearerToken::verify` unit tests in `api/token.rs`).
- Integration tests: in `crates/daemon/tests/contract_daemon.rs` (single file; the project doesn't split contract tests across multiple files).
- Snapshot tests for wire format: `crates/protocol/tests/contract_protocol.rs`.

**Testing patterns from Stories 1.2–1.6:**

- Use `fresh_pools()` (line 20 of `contract_daemon.rs`) for every test that needs a clean DB.
- Use `tower::ServiceExt::oneshot` to drive the router without binding a TCP listener — see `readyz_returns_503_before_migrations_complete` (line 343) and `healthz_returns_200_immediately` (line 382) for the canonical pattern.
- Read response bodies with `axum::body::to_bytes(resp.into_body(), usize::MAX)` then `serde_json::from_slice`.
- Add a small test helper `fn make_test_state(pools: DbPools, migrations_complete: Arc<AtomicBool>) -> AppState` (or `make_test_state_with_token(pools, mc, token)`) if AppState construction noise grows past ~3 fields. Keep it local to the test file.

**Deterministic test discipline** (`project-context.md:642`):

- **No real `sleep()`** for synchronization. Story 1.7 doesn't need time-advance — only the stale-Working test (Task 13) needs to age a stored row, and the right pattern there is to manually rewrite the JSON's `last_event_at_ms` field, not to actually wait 5 minutes.
- **`unwrap()`/`expect()` is fine in tests.** The production discipline (no unwrap outside `#[cfg(test)]`) does not extend to tests.

**Contract tests this story adds to the pre-MVP gate list:**

| Contract | Test name |
|---|---|
| `/sessions` filters sentinel, surfaces read-time state | `sessions_list_returns_known_sessions_with_read_time_state` |
| `/sessions/{id}` returns 404 on unknown | `sessions_detail_returns_404_when_unknown` |
| Cursor gap detection (per `project-context.md:591`) | `events_list_oldest_available_after_purge` |
| Bearer auth rejects missing header | `authenticated_routes_reject_missing_header` |
| Bearer auth rejects wrong token | `authenticated_routes_reject_wrong_bearer` |
| Bearer auth accepts correct token | `authenticated_routes_accept_correct_bearer` |
| Outbound additive-compat on `SessionStats` | `session_stats_accepts_unknown_fields` |
| `/readyz` probes the database | `readyz_returns_503_when_db_unreachable` |

Eight new contract tests slot into the pre-MVP gate list. The cursor-gap-detection one specifically satisfies `project-context.md:591` row "Cursor-gap detection" — which this story is the natural home for.

### Critical Context from Stories 1.1–1.6 (DO NOT REPEAT MISTAKES)

**Dependency pins** — use the workspace dep table at `Cargo.toml`, never invent versions:

| Dep | Actually installed |
|---|---|
| serde | 1.0.228 |
| serde_json | 1.0.149 |
| thiserror | 2.0.18 |
| tempfile | 3.20.0 |
| assert_cmd | 2.0.17 |
| rusqlite | 0.38.0 (note: workspace pin is 0.38.0; some doc text references 0.39 — workspace lock is authoritative) |
| deadpool-sqlite | 0.13.0 |
| rusqlite_migration | 2.4.1 |
| tokio | 1.52.1 |
| axum | 0.8.9 |
| tower | 0.5.3 |
| tower-http | 0.6.10 |
| secrecy | 0.10.3 |
| uuid | 1.23.1 |

If a Story 1.6 dev note (or any older doc) lists a different patch version, the workspace `Cargo.toml` is the source of truth — it is the file `cargo build` actually consults.

**Workspace lints:** every crate has `[lints] workspace = true` and the workspace has `unsafe_code = "forbid"`. **Do NOT** add `#![deny(unsafe_code)]` or `#![forbid(unsafe_code)]` to any source file — triggers `clippy::duplicated_attributes` as a hard error (Story 1.4 review finding).

**`anyhow` boundary:** permitted only in `main.rs` of binary crates. All daemon-internal modules use `thiserror::Error` types defined in `crates/daemon/src/error.rs`. Story 1.7 adds **zero** new `Error` variants — the existing `Error::Pool`, `Error::Sqlite`, `Error::Clock`, `Error::Migration`, `Error::Ingest` cover all failure paths in the new endpoints. If a handler needs a new error semantically, model it as an HTTP response in the handler, not as a new error type.

**No `unwrap()` / `expect()` outside `#[cfg(test)]`:** hard rule, enforced by review (Story 1.4, 1.5 reviews both flagged this). Every `Result` is `?`-propagated or `map_err`-converted.

**No `println!` / `eprintln!`:** anywhere in shipped daemon code, except the sanctioned tracing-bootstrap exceptions in `main.rs` (HOME-resolution failure path, RUST_LOG parse failure). New handlers and middleware use `tracing::error!`/`tracing::warn!`.

**Connection factory rule:** never call `rusqlite::Connection::open` outside `crates/daemon/src/db/pool.rs`. The `scripts/lint-db-access.sh` script enforces this in CI. **However:** the `readyz_returns_503_when_db_unreachable` test (Task 12, Option A) DOES write garbage bytes to a path then attempts to open it via the daemon pool — that is the pool calling `Connection::open`, not the test directly. Confirm the lint script targets `crates/` excluding `tests/` if a test happens to need a raw connection.

**Wire-format snapshot discipline** (`architecture.md:711-713`): every new wire-format type gets a snapshot assertion in `crates/protocol/tests/contract_protocol.rs`. Story 1.7 adds three new outbound types — each needs a snapshot test (Task 18 covers `SessionStats`; add equivalents for `SessionListItem`, `SessionDetail`, `DaemonStatus`).

**Additive-only outbound serde** (`architecture.md:606-608`, `architecture.md:714`): no `#[serde(deny_unknown_fields)]` on any outbound type. Story 1.7's three new types are outbound — they MUST NOT carry that attribute. The forward-compat round-trip tests are the canary; without them, a regression would be invisible until a client breaks in the field.

**Strict-inbound serde:** `EventsParams` (the `?since=<cursor>` query param struct) is inbound and MUST carry `#[serde(deny_unknown_fields)]` — unknown query params should 400, not silently absorb.

**Tracing instrumentation** (`architecture.md:661-670`): `#[tracing::instrument(skip_all, fields(...))]` on every new async handler. Stick to `skip_all` to avoid leaking the bearer token or request bodies into spans. Specific fields opted in via `fields(...)` only. Examples:
- `#[tracing::instrument(skip_all, fields(session_id = %id))]` on `/sessions/{id}` handlers
- `#[tracing::instrument(skip_all)]` on `/status` (no useful field except request-id which isn't wired yet)
- `#[tracing::instrument(skip_all)]` on `auth::require_bearer` — do NOT log the candidate token, ever.

**Single-writer pool, multi-reader pool:** all queries in Story 1.7 are reads. Use `state.db.reader.get().await` exclusively. **Do NOT** acquire from `state.db.writer` for any new query — that pool is reserved for the projection write path and would create writer-vs-reader contention.

**SQL discipline:** all SQL strings live in `crates/daemon/src/db/queries.rs`. No inline SQL anywhere else (`architecture.md:798`). Story 1.7 adds six new constants there: `SELECT_NON_SENTINEL_SESSIONS`, `SELECT_SESSION_BY_ID`, `SELECT_EVENTS_FOR_SESSION_SINCE`, `SELECT_MIN_EVENT_ID`, `SELECT_STATS_FOR_SESSION`, `SELECT_LAST_EVENT`.

**No new schema migration:** Story 1.7 reads from `events` and `session_projections` (both created by Story 1.2's `V1_UP` at `crates/daemon/src/db/migrations.rs:5-29`). No new tables, no new columns. The `state` column of `session_projections` is `TEXT NOT NULL`; Story 1.6 starts writing real JSON there; Story 1.7 reads it.

### Anti-Patterns To Avoid

- **Adding a `chrono` / `time` dep** for timestamps. `SystemTime::now().duration_since(UNIX_EPOCH).as_millis()` is the project pattern. Reuse `current_unix_millis` (extracted to `crates/daemon/src/time.rs` in Task 8).
- **Adding a `gap_detected: bool` field to `EventListResponse`** (or anywhere else). The substrate emits facts (`oldest_available_event_id`), not interpretations (`gap_detected`). Presenters compare `since < oldest_available_event_id` in one line client-side. `project-context.md:481` and `architecture.md:145` ("No `gap_detected: bool` — Axiom 4").
- **Skipping `subtle::ConstantTimeEq` and writing `a == b`** for the bearer token compare. The early-exit on first mismatched byte leaks token length and the index of the first wrong byte. There is no acceptable "simpler" version — use the crate.
- **Reading the bearer token in the ingest path.** `crates/daemon/src/ingest/{listener,handler,writer}.rs` must not import `crate::api::token` or `BearerToken`. This is an architecture invariant (`architecture.md:444-446`).
- **Logging the bearer token anywhere.** Not in `tracing::*` calls, not in `Debug` impls, not in error messages, not in panic messages. `secrecy::SecretString` redacts `Debug`/`Display`; do not undo that.
- **Adding a `data` envelope** (`{ "data": ..., "meta": {...} }`) around the JSON response. axum's `Json(value)` is the wire shape; the response body IS the value.
- **Splitting handlers into "handler + service + repository" layers.** This is a developer-tool with five endpoints; the right ratio is "one function = one endpoint = one SQL query." Don't introduce service traits, repository traits, or DI containers. If a handler grows past ~60 lines, the right move is to extract a SQL constant or a parsing helper, not a new layer.
- **Pagination via offset/limit.** Cursor-based via `?since=<event_id>` is the only pattern. Offset/limit drifts under concurrent writes; cursors are stable.
- **Returning `404` from `/sessions` when the list is empty.** The empty list is `200 []`. `404` is reserved for "you asked for a specific thing that does not exist" (a `{id}` mismatch).
- **Returning `200` with `{"error": "..."}` body.** Never. Error bodies have error status codes.
- **Caching session lookups in `AppState`.** The reader pool is fast; `WHERE source != '__daemon__' AND session_id = ?` is an indexed lookup on a small table. Caching adds complexity without measurable benefit at V1 scale.
- **Hot-reloading the bearer token without a daemon restart.** NFR14 explicitly says no. The token is read once at startup; rotation requires a restart. Do NOT add a SIGHUP-rereads-token path.
- **Adding rate limiting.** NFR7 explicitly defers this. Do NOT add `tower-governor` or `tower::limit::RateLimitLayer` in this story.
- **Adding metrics counters in handlers** (`metrics::increment!`, `prometheus::*`). NFR18 defers metrics. The `/status` endpoint is the V1 surface for "how is the daemon doing."
- **Tweaking the SQLite schema** to add a denormalized `current_state` column to `session_projections` for faster `/sessions` reads. The `state` column is a JSON blob by design; deserializing it on read is cheap on the single-developer load. Premature schema optimization breaks Story 1.6's "the projection row is a pure function of events" invariant.

### Performance considerations (V1 scope)

- **Reader pool is sized for 4 concurrent reads.** The new endpoints all hit the reader pool. Worst-case concurrent reads (single developer hammering REST + WS catch-up) hit ~3 readers; 4 is comfortable.
- **`SELECT MIN(event_id)`** scans an indexed primary key in O(log n) — fast even at 100k events. `SELECT MAX(event_id)` likewise.
- **`SELECT_EVENTS_FOR_SESSION_SINCE`** uses `WHERE session_id = ? AND event_id > ?`. There is no compound index on `(session_id, event_id)` in V1 — the query scans the primary-key index filtering by session_id. At 100k events with ~10 sessions, this scans ~10k rows per session-specific query, well under 100ms. Add a compound index in a future hardening pass; track as deferred work (NOT for 1.7).
- **`SELECT_NON_SENTINEL_SESSIONS`** scans `session_projections` (small table; rows ~= concurrent sessions, typically <10). O(n) is fine.
- **`SELECT_STATS_FOR_SESSION`** uses `GROUP BY source` + aggregates over the `events` table for one session_id. Same scan profile as the events list endpoint.
- **`/status`** does one `SELECT_LAST_EVENT` per request. Trivially cheap; even at 1 RPS this is a non-issue.

**No NFR1 / NFR2 implications.** Story 1.7's perf surface is REST — not the shim, not the ingest. NFR1's 5ms p95 shim budget is unaffected; NFR2's "no perceptible lag" is comfortably met by SQLite reader-pool checkouts.

### Latest tech information

**axum 0.8.x changes that affect this story** (vs the 0.7 model some doc text implies):

1. **Path-param syntax:** `:id` (0.7) → `{id}` (0.8). Routes in api/mod.rs must use `{id}`.
2. **`Router::with_state`:** unchanged; still the canonical pattern.
3. **`middleware::from_fn_with_state`:** still the right tool for state-aware middleware.
4. **`Path<String>`, `Query<T>`, `State<AppState>` extractors:** unchanged.
5. **`Json<T>` extractor + responder:** unchanged.
6. **No top-level breakages** beyond the path-param syntax; the code patterns this story uses are stable across 0.7/0.8.

**`subtle::ConstantTimeEq` (2.6.x)** API:
```rust
use subtle::ConstantTimeEq;
let a: &[u8] = b"token-from-header";
let b: &[u8] = b"daemon-token";
let same: bool = a.ct_eq(b).into(); // Choice -> bool conversion via Into
```
The result of `ct_eq` is a `Choice`, a `u8`-wrapper with constant-time `From<Choice> for bool`. Convert via `.into()` or `bool::from(choice)`. Do NOT call `.into_option()` — that's a different API.

**`secrecy::SecretString` (0.10.x)** API:
```rust
use secrecy::{ExposeSecret, SecretString};
let s: SecretString = SecretString::new("my-token".into());
let raw: &str = s.expose_secret(); // narrow access
```
Note: `secrecy 0.10` changed the API from `0.8` (deprecated `Secret<T>` → renamed `SecretString` as a type alias-ish wrapper; `expose_secret` returns `&str` for `SecretString`). The crate is small enough that its README is the canonical reference.

### ADR + sprint cross-references

- **ADR-0002** ([decisions/0002-ingest-wire-framing-and-hook-kind.md](../../decisions/0002-ingest-wire-framing-and-hook-kind.md)) ratifies the ingest wire framing. Story 1.7 is downstream of ingest; no ADR impact.
- **ADR-0003** ([decisions/0003-shim-p99-budget-on-macos-latest.md](../../decisions/0003-shim-p99-budget-on-macos-latest.md)) covers shim perf budget; no impact on REST.
- **Story 1.6** (`docs/bmad/implementation-artifacts/1-6-session-projection-and-hook-unreliability-tolerance.md`) introduced `SessionCurrentState`, `SessionState`, and `projection::state::current_state_for_read`. Story 1.7 is the first wire-up of those types into REST handlers. If 1.6 has not merged when 1.7 starts, the dev should wait on 1.6 OR rebase on 1.6's branch.
- **Story 1.8** (`Tighten daemon hook_kind to a required transport field`, currently backlog) sits AFTER 1.7. Story 1.8 modifies the ingest path; Story 1.7 modifies the API path. No interaction; either can land first.
- **Story 3.3** (`Bearer token auth with keychain storage`, backlog) extends Story 1.7's `api::token::load_or_generate` chain. The validation layer (Task 3) is stable across 1.7 → 3.3; only the issuance source changes. Document the boundary in `api/token.rs` so 3.3's author sees the right entry point.

### Project Structure Notes

**Alignment with `architecture.md:806-813`:**

The architecture's `crates/daemon/src/api/` layout names: `mod.rs, auth.rs, token.rs, sessions.rs, events.rs, health.rs, ws.rs`. Story 1.7 ships all of these EXCEPT `ws.rs` (Epic 2), PLUS a new `status.rs`. Adding `status.rs` is a small additive deviation from the architecture text: the `/status` endpoint is co-listed with the rest in the PRD/epic, but the architecture's file enumeration omits it. The right interpretation is "one file per endpoint cluster" — sessions/events/health/auth/token/status each owns a file. Document this in commit body.

**`crates/daemon/src/time.rs`:** new file, internal-only. Extracted to share `current_unix_millis` between `projection::session` and the new `api::status`. This is the cleanest factoring; the alternative is a circular-import-prone "`api` calls into `projection::session::current_unix_millis`" pattern which couples API to projection internals.

**Symbol re-exports from `protocol`:**

```rust
// crates/protocol/src/lib.rs
pub use rest::{EventListResponse, SessionStats, SessionListItem, SessionDetail, DaemonStatus};
```

Add the three new types to the existing re-export line. Callers always import from the crate root (`protocol::SessionListItem`), never from `protocol::rest::SessionListItem` — the existing pattern (`architecture.md:569-570`).

**`api/sessions.rs` houses three handlers** (`list`, `detail`, `stats`). This is a deviation from "one endpoint per file" — but `/sessions`, `/sessions/{id}`, and `/sessions/{id}/stats` are tightly related (same resource, different views) and live well in one file. `events.rs` and `status.rs` are single-handler files. The principle is "one file per resource"; `/status` is its own resource (a daemon snapshot, not a session).

### References

- [Source: docs/bmad/planning-artifacts/epics.md#Story-1.7] — original AC text
- [Source: docs/bmad/planning-artifacts/architecture.md#API-Communication-Patterns] — REST endpoints list (line 455-459); axum 0.8 server; tower-http middleware (line 452)
- [Source: docs/bmad/planning-artifacts/architecture.md#Authentication-Security] — bearer token model; keychain → env → file chain (line 442); no token in ingest path (line 444-446); UUID4 generation (line 442)
- [Source: docs/bmad/planning-artifacts/architecture.md#Wire-Format-Conventions] — HTTP error body shape `{ "error": "..." }` (line 598-599); EventId on wire (line 596); asymmetric serde (line 606-608)
- [Source: docs/bmad/planning-artifacts/architecture.md#Enforcement-Guidelines] — wire-format snapshot mandate (line 711-713); never `deny_unknown_fields` outbound (line 714)
- [Source: docs/bmad/planning-artifacts/architecture.md#Architectural-Boundaries] — API boundary (line 884-887); `api/auth.rs` and `api/token.rs` ownership
- [Source: docs/bmad/planning-artifacts/architecture.md#Process-Conventions] — tracing instrumentation `skip_all` (line 661-670); bearer-token `SecretString` requirement (line 672-680)
- [Source: docs/bmad/planning-artifacts/architecture.md#Dependency-Version-Pins] — axum 0.8.9, secrecy 0.10.3, tower-http 0.6.10, uuid 1.23.1 (lines 313-330)
- [Source: docs/bmad/planning-artifacts/architecture.md#OQ-Resolutions] — REST `EventListResponse` contract (line 142-145); gap-detection mechanical fact (line 145)
- [Source: docs/bmad/planning-artifacts/prd.md#REST-Endpoints] — full endpoint table (line 350-363); `/status` returns version + uptime + connected tools + last event time (line 356)
- [Source: docs/bmad/planning-artifacts/prd.md#FR] — FR18-FR23 mapped to this story (line 484-489); FR38 bearer auth (line 516)
- [Source: docs/bmad/planning-artifacts/prd.md#NFR] — NFR3 daemon 2s readiness (line 525); NFR11-NFR15 security (line 542-546); NFR21 auto-migration / readyz contract (line 561); NFR22 timestamp column (line 562)
- [Source: docs/bmad/project-context.md#REST-surface] — endpoint sketch + auth model (line 460-475); cursor-based pagination (line 475); gap-detection as mechanical fact (line 477-483)
- [Source: docs/bmad/project-context.md#Required-framework-infrastructure] — `AppState` shape (line 491); WS concurrency cap (line 498); deferred middleware (line 495-497) — explicitly out of scope for 1.7
- [Source: docs/bmad/project-context.md#Required-contract-tests] — cursor-gap detection (line 591); outbound additive-compat canary (line 594); state+event atomicity (line 589 — already covered by Story 1.6's Task 7)
- [Source: docs/bmad/project-context.md#Substrate-not-actor-invariants] — `(source, session_id)` natural key (line 695); single normalization rule (line 697)
- [Source: docs/bmad/project-context.md#Deterministic-test-discipline] — no real sleep (line 642)
- [Source: docs/bmad/implementation-artifacts/1-6-session-projection-and-hook-unreliability-tolerance.md] — `SessionState` + `SessionCurrentState` types; `current_state_for_read` semantics; STALE_WORKING_MS contract
- [Source: docs/bmad/implementation-artifacts/deferred-work.md] — line 32 (`/readyz` does not probe the database) — resolved by Task 9
- [Source: docs/decisions/0002-ingest-wire-framing-and-hook-kind.md] — NDJ wire framing; downstream of Story 1.7's concerns but flagged for context
- [Source: docs/bmad/planning-artifacts/sprint-change-proposal-2026-05-18.md §5] — sprint order: 1.6 → 1.7 → 1.8 confirmed
- [Source: crates/protocol/src/rest.rs] — existing `EventListResponse` and `SessionStats`; this story adds three more types here
- [Source: crates/protocol/src/lib.rs:14] — re-export line for `rest::*`; extend it
- [Source: crates/protocol/tests/contract_protocol.rs] — snapshot test conventions for new wire types
- [Source: crates/daemon/src/api/mod.rs] — current router (only `/healthz`, `/readyz`); replaced by Task 10
- [Source: crates/daemon/src/api/health.rs] — current `readyz` (migrations_complete only); tightened by Task 9
- [Source: crates/daemon/src/state.rs] — `AppState` shape (extended by Task 2)
- [Source: crates/daemon/src/main.rs:67-173] — daemon `run()` orchestrator; Task 1 wires `token::load_or_generate` here; Task 2 updates the `AppState` construction at line 120
- [Source: crates/daemon/src/db/queries.rs] — SQL constants location (six new constants added by 1.7)
- [Source: crates/daemon/src/projection/session.rs:187-194] — `current_unix_millis`; extracted to `crates/daemon/src/time.rs` (Task 8)
- [Source: crates/daemon/tests/contract_daemon.rs:20-26] — `fresh_pools()` fixture; reuse for all new contract tests
- [Source: crates/daemon/tests/contract_daemon.rs:343-380] — `readyz_returns_503_before_migrations_complete` + `healthz_returns_200_immediately`; canonical `tower::ServiceExt::oneshot` patterns
- [Source: Cargo.toml] — workspace dep pins; add `subtle = "2.6"` here AND in `crates/daemon/Cargo.toml`

## Dev Agent Record

### Agent Model Used

claude-opus-4-7 (1M context)

### Debug Log References

- Rebased the story-1.7 branch onto `origin/main` mid-flight after discovering Story 1.6 had landed but the local branch was still based on `b5b798d` (pre-1.6). Conflict in `sprint-status.yaml` was a routine three-way merge between "story 1.6 review" (main) and "story 1.7 ready-for-dev" (this branch); resolved by promoting 1.6→review and 1.7→in-progress.
- `cargo fmt` reflowed several handlers and the rest module after the initial implementation pass; no semantic changes.
- Manual smoke test (binary launched against `$(mktemp -d)`, `BOWERBIRD_TOKEN=smoke-token-42`):
  - `GET /healthz` → 200 `{"status":"ok"}`
  - `GET /readyz` → 200 `{"status":"ready"}` (migrations done + DB probe ok)
  - `GET /sessions` no header → 401 `{"error":"unauthorized"}`
  - `GET /sessions` wrong token → 401 `{"error":"unauthorized"}`
  - `GET /sessions` correct token → 200 `[]`
  - `GET /status` correct token → 200 with `daemon_version`, `protocol_version: "1.0"`, `started_at_ms`, `uptime_ms`, `last_event_at_ms: null`, `last_event_id: null` (only sentinel present)
  - Token value never appeared in the log stream; the `Env` source got an `info` line, not the `Generated` WARN.
  - Graceful shutdown wrote `RecordingEnded` cleanly.

### Completion Notes List

- All 8 ACs are satisfied with passing contract tests (114 total tests, 20 new daemon contract tests + 5 new protocol round-trip / forward-compat tests, plus the existing `/healthz` body assertion under the new router shape).
- `subtle::ConstantTimeEq` and `secrecy::ExposeSecret` are touched only inside `crates/daemon/src/api/token.rs` (`grep -rn 'subtle::\\|secrecy::ExposeSecret' crates/daemon/src/` confirms — `crates/daemon/src/api/auth.rs` and `crates/daemon/src/main.rs` only reference them in doc comments).
- The ingest path is bearer-free: `grep -rn 'BearerToken\\|bearer\\b' crates/daemon/src/ingest/` returns zero matches.
- `current_unix_millis` was extracted from `crates/daemon/src/projection/session.rs` to a new `crates/daemon/src/time.rs` so the projection layer and `api::status` share one wall-clock source (no `chrono`/`time` dep added).
- `/readyz` retains the migrations-first ordering so the existing drain-on-shutdown invariant (`migrations_complete.store(false)` during graceful shutdown) keeps causing 503s; the DB probe runs only when migrations are complete.
- Forward-compat additive serde is asserted for `SessionStats`, `SessionListItem`, `SessionDetail`, and `DaemonStatus`.
- Cursor-gap-detection (`oldest_available_event_id` after purge) is exercised end-to-end in `events_list_oldest_available_after_purge`, which satisfies `project-context.md:591`.
- A `reaction_from_db_string` inverse helper was added next to `reaction_as_db_string` to mirror Story 1.6's `event_kind_from_db_str` pattern.
- `EventsParams` uses `#[serde(deny_unknown_fields)]`; the contract test `events_endpoint_rejects_unknown_query_param` verifies axum surfaces unknown query params as 400.
- Existing 89 daemon/protocol tests continue to pass with no regressions.

### File List

**Created**

- `crates/daemon/src/api/auth.rs`
- `crates/daemon/src/api/events.rs`
- `crates/daemon/src/api/sessions.rs`
- `crates/daemon/src/api/status.rs`
- `crates/daemon/src/api/token.rs`
- `crates/daemon/src/time.rs`

**Modified**

- `Cargo.toml` (added `subtle = "2.6"` to `[workspace.dependencies]`)
- `crates/daemon/Cargo.toml` (added `secrecy` + `subtle` to `[dependencies]`)
- `crates/daemon/src/api/health.rs` (`/readyz` now probes DB)
- `crates/daemon/src/api/mod.rs` (new merged unauth + auth router; new `pub mod` declarations)
- `crates/daemon/src/db/queries.rs` (six new SQL constants + `reaction_from_db_string`)
- `crates/daemon/src/lib.rs` (added `pub mod time`)
- `crates/daemon/src/main.rs` (token loader, `started_at_ms`, AppState construction)
- `crates/daemon/src/projection/session.rs` (removed local `current_unix_millis`; imports from `crate::time`)
- `crates/daemon/src/state.rs` (added `bearer: BearerToken` and `started_at_ms: i64`)
- `crates/daemon/tests/contract_daemon.rs` (test helper + 20 new contract tests + updated existing AppState fixtures + new `/healthz` body assertion)
- `crates/protocol/src/lib.rs` (re-exported the three new outbound types)
- `crates/protocol/src/rest.rs` (added `SessionListItem`, `SessionDetail`, `DaemonStatus`; doc-comments on `EventListResponse`)
- `crates/protocol/tests/contract_protocol.rs` (5 new forward-compat / round-trip tests)
- `docs/bmad/implementation-artifacts/sprint-status.yaml` (1-7 → in-progress → review)
- `docs/bmad/implementation-artifacts/deferred-work.md` (struck Story 1.2 `/readyz` entry; added Story 1.7 deferred section)
- `docs/protocol-changelog.md` (v1.0 → v1.1 schema + behavioral entries for the three new outbound types, five new endpoints, tightened `/readyz`, and token resolution chain)

## Change Log

| Date       | Change                                                                                                                                | Author       |
| ---------- | ------------------------------------------------------------------------------------------------------------------------------------- | ------------ |
| 2026-05-20 | Story 1.7 created (sprint-status promoted to ready-for-dev).                                                                          | bmad         |
| 2026-05-20 | Story 1.7 implemented: bearer auth + 5 REST endpoints + `/readyz` DB probe; 114 tests green; deferred-work + protocol-changelog kept in sync. | claude-opus-4-7 |
