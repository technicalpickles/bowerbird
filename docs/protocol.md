# Protocol

The bowerbird wire reference. This file describes the current wire surface; [`docs/protocol-changelog.md`](protocol-changelog.md) explains how it got here. For the tool-author walkthrough, see [`docs/presenter-authoring.md`](presenter-authoring.md); this file is the dense lookup.

The current protocol version is `1.0` — the literal value `HelloFrame.protocol_version` and `DaemonStatus.protocol_version` carry on the wire. The value tracks the protocol crate's semver contract (additive-only within `1.x`), NOT the daemon binary version.

## Wire format and conventions

- **Transport.** JSON over TCP for REST and WebSocket; JSON over a Unix-domain socket for ingest. The daemon binds `127.0.0.1` on a kernel-assigned ephemeral port (configurable in `crates/daemon/src/config.rs::Config::bind_addr`).
- **Authentication.** REST and WebSocket gate on a bearer token: `Authorization: Bearer <token>`. The token is a UUID4 stored in the system keychain (macOS Keychain, Linux Secret Service) and resolved by the daemon and CLI alike from `BOWERBIRD_TOKEN` env → keychain → `~/.bowerbird/config.toml`. The ingest socket is filesystem-authenticated (socket mode `0600`) — no token.
- **Asymmetric `deny_unknown_fields` policy.** Inbound parsers are strict; outbound emitters are permissive.
  - **Strict on inbound.** `ClientMessage` (the WebSocket `subscribe` / `unsubscribe` shapes) carries `#[serde(deny_unknown_fields)]` ([`crates/protocol/src/ws.rs:31`](../crates/protocol/src/ws.rs)). Extra fields, unknown ops, malformed JSON, binary frames — all close the connection with WebSocket code 1008.
  - **Permissive on outbound.** None of the outbound types (`ServerMessage`, every `*Frame`, every `*Response`, every `*Item`, `ServerInfo`) carry the attribute. A future daemon adding a field will not break older bindings — they silently ignore the new field. See [`crates/protocol/src/rest.rs:85-94`](../crates/protocol/src/rest.rs) for the `ServerInfo` doc-comment, which states the rule explicitly.
- **`ServerMessage::Unknown` catch-all.** Variant-level additive compat. `ServerMessage` carries `#[serde(other)] Unknown` ([`crates/protocol/src/ws.rs:25`](../crates/protocol/src/ws.rs)) so older clients gracefully deserialize future variants instead of erroring on the tag. `deny_unknown_fields` is field-level; `Unknown` is variant-level. Both together make "additive within v1.x" real.
- **Sentinel events are never broadcast.** `EventKind::RecordingStarted` and `EventKind::RecordingEnded` are persisted with `source = "__daemon__"` for lifecycle bookkeeping but filtered out before any wire emission (REST `/sessions/*` queries, WebSocket `event` frames, replay POST body).
- **`protocol_version` field.** The literal `"1.0"` ships in `HelloFrame.protocol_version` ([`crates/daemon/src/api/ws.rs:49`](../crates/daemon/src/api/ws.rs)) and `DaemonStatus.protocol_version` ([`crates/daemon/src/api/status.rs:13`](../crates/daemon/src/api/status.rs)). It bumps via additive changes within `1.x`; a `2.0` would imply a breaking change. See [§Versioning and compat policy](#versioning-and-compat-policy).

## REST endpoints

The full route set declared at [`crates/daemon/src/api/mod.rs:99-114`](../crates/daemon/src/api/mod.rs):

| Path | Method | Auth | Request | Response | Status |
|------|--------|------|---------|----------|--------|
| `/healthz` | GET | none | — | empty body | 200 |
| `/readyz` | GET | none | — | empty body if migrations done + db probe ok; else error | 200, 503 |
| `/status` | GET | bearer | — | [`DaemonStatus`](#get-status) JSON | 200, 401 |
| `/sessions` | GET | bearer | — | [`SessionListItem[]`](#get-sessions) JSON | 200, 401 |
| `/sessions/{id}` | GET | bearer | — | [`SessionDetail`](#get-sessionsid) JSON | 200, 401, 404 |
| `/sessions/{id}/events?since=<cursor>` | GET | bearer | `since` query param (integer) | [`EventListResponse`](#get-sessionsidevents) JSON | 200, 401, 404 |
| `/sessions/{id}/stats` | GET | bearer | — | [`SessionStats`](#get-sessionsidstats) JSON | 200, 401, 404 |
| `/replay` | POST | bearer | JSONL body of `Event` records | `{"replayed_count":N,"parse_errors":[...]}` JSON | 200, 401, 413 |

All authenticated routes return `401 Unauthorized` on a missing or malformed bearer. All responses carry an `x-request-id` UUID4 header for cross-cut tracing. The HTTP request flow is wrapped in a 30s wall-clock timeout and a 1 MiB body limit; WebSocket connections, once upgraded, are exempt from both (see [§WebSocket endpoint and control mechanics](#websocket-endpoint-and-control-mechanics)).

### `GET /healthz`

- **Auth.** None.
- **Request.** None.
- **Response.** `200 OK`, empty body. Always — there is no failure mode for the liveness probe beyond the process being down.
- **Notes.** Liveness only. Use `/readyz` for "is the daemon ready to serve traffic."

### `GET /readyz`

- **Auth.** None.
- **Request.** None.
- **Response.** `200 OK` (empty body) when both migrations are complete AND the database probe (`SELECT 1 FROM events WHERE 1=0` against the reader pool) succeeds. `503 Service Unavailable` otherwise.
- **Notes.** The db-probe arm was added in Story 1.7 (AC #2) — earlier releases only checked the migrations flag. See [`docs/protocol-changelog.md`](protocol-changelog.md) v1.0→v1.1 Story 1.7 entry.

### `GET /status`

- **Auth.** Bearer required.
- **Request.** None.
- **Response.** `200 OK` with a `DaemonStatus` JSON body:

  ```json
  {
    "daemon_version": "0.1.0",
    "protocol_version": "1.0",
    "started_at_ms": 1748190000000,
    "uptime_ms": 12345,
    "last_event_at_ms": 1748190001000,
    "last_event_id": 42,
    "connected_ws_clients": 1
  }
  ```

- **Field source.** [`crates/protocol/src/rest.rs:67`](../crates/protocol/src/rest.rs) `DaemonStatus`.
- **Notes.** `last_event_at_ms` and `last_event_id` are `null` when the events table contains no non-sentinel rows. `connected_ws_clients` is a snapshot at request time — it can drift between this read and a follow-up read because WS connections come and go. Added in Story 3.2 (the `bowerbird status` CLI's first consumer); older `DaemonStatus` consumers ignore the additive field per the asymmetric serde policy.

### `GET /sessions`

- **Auth.** Bearer required.
- **Request.** All query params optional; absent = unfiltered (the default preserves pre-5.8 behavior exactly). Filters compose.
  - `?state=<csv>` (Story 5.8) — comma-separated, case-insensitive `SessionCurrentState` tokens: `idle`, `working`, `waitinginput`, `ended`, `unknown`. Returns only sessions whose **read-time** `current_state` (after the stale-`Working`→`Idle` fallback, i.e. the value the response actually carries) is in the set. The canonical triage call is `?state=working,waitinginput,idle` (drops the `Ended` graveyard); `?state=ended` is the inverse (a history/audit view). An unrecognized token → `400`.
  - `?since=<updated_at_ms>` (Story 5.8) — exclusive lower bound on the `updated_at` column (a **recency** filter, **not** a pagination cursor): returns only sessions with `updated_at > <updated_at_ms>`, the same `updated_at` each item carries. Poll "what changed since my last poll" by passing the max `updated_at` you have seen. `0`/absent = no bound. A non-integer or negative value → `400`.
  - `?limit=<n>` (Story 5.8) — positive integer SQL `LIMIT` on the ordered (`updated_at DESC, source ASC, session_id ASC`), `since`-filtered row set. Absent = no cap. `0`, negative, or non-integer → `400`.
  - **Unknown query keys → `400`.** The handler is strict-inbound (`deny_unknown_fields`, mirroring the WS `ClientMessage` policy): a typo'd or unrecognized key (e.g. `?stat=working`, `?foo=bar`) returns `400`, never a silently-ignored param. Only `state`/`since`/`limit` are accepted; omitting all three is the unfiltered default.
  - **`?state=` + `?limit=` interaction.** `limit` caps the **pre-state-filter** set, then `?state=` filters in Rust — so a page MAY return fewer than `<n>` items when some of the `<n>` fetched rows are filtered out by state.
  - **These are filters, not pagination.** `?since=`/`?limit=` are a recency bound + a row cap, not a forward/backward cursor. With `updated_at DESC`, `?since=` only ever *narrows toward newer* rows — there is no "next (older) page" direction, and it cannot express one. Two corollaries to know: (1) under `?limit=`, rows that share the boundary `updated_at` can be split across the cap, so a follow-up `?since=<that ms>` poll (exclusive `>`) silently skips the tied rows beyond the cut; (2) a read-time `Working`→`Idle` transition (the staleness fallback) does **not** advance `updated_at`, so a `?since=` poll never surfaces it. A presenter that wants *all* currently-active sessions should just fetch unbounded `?state=working,waitinginput,idle` — at V1 scale that array is small — rather than trying to page. True cursor pagination over `GET /sessions` remains deferred (`deferred-work.md`).
- **Response.** `200 OK` with a JSON array of `SessionListItem`:

  ```json
  [
    {
      "source": "claude",
      "session_id": "session-alpha",
      "current_state": "Idle",
      "last_event_kind": "PostToolUse",
      "last_event_at_ms": 1748190001000,
      "updated_at": 1748190001000,
      "last_pid": 12345,
      "cwd": "/Users/x/code/myrepo",
      "started_at": 1748190000000
    }
  ]
  ```

- **Field source.** [`crates/protocol/src/rest.rs:38`](../crates/protocol/src/rest.rs) `SessionListItem`.
- **Notes.** `current_state` is the read-time projection (stale-`Working` → `Idle` fallback per Story 1.6's `current_state_for_read`), NOT the raw stored value. Sentinel-source sessions (`source = "__daemon__"`) are filtered out. `last_pid` (Story 5.3) is the carry-forwarded PID from the most recent envelope whose `bowerbird_ppid` was set — `null` for sessions ingested before Story 5.3. `cwd` and `started_at` (Story 5.7) carry the session's working directory and start time. `cwd` is `null` for sessions projected before Story 5.7 (and stays `null` until a post-upgrade event reports one). `started_at` is `null` for sessions projected before Story 5.7; a pre-5.7 row that keeps receiving events shows an approximate start time (the first post-upgrade event's clock), and a full rebuild reconstructs the true first-event time. bowerbird is pre-release — on a schema/projection change the supported upgrade path is to remove `~/.bowerbird/bower.db` and restart, after which every session records an exact `started_at`. See the `SessionState` narrative below.

### `GET /sessions/{id}`

- **Auth.** Bearer required.
- **Request.** None. `{id}` is the URL-encoded `session_id`.
- **Response.** `200 OK` with a `SessionDetail` JSON body:

  ```json
  {
    "source": "claude",
    "session_id": "session-alpha",
    "state": {
      "current_state": "Idle",
      "last_event_kind": "PostToolUse",
      "last_event_at_ms": 1748190001000,
      "last_pid": 12345,
      "cwd": "/Users/x/code/myrepo",
      "started_at": 1748190000000
    },
    "updated_at": 1748190001000
  }
  ```

- **Status codes.** `200`, `401`, `404 Not Found` (session-id was never seen).
- **Field source.** [`crates/protocol/src/rest.rs:53`](../crates/protocol/src/rest.rs) `SessionDetail`, [`crates/protocol/src/state.rs:13`](../crates/protocol/src/state.rs) `SessionState`.
- **Notes.** `state.current_state` applies the read-time stale-`Working` → `Idle` fallback (same as `/sessions`).

### `GET /sessions/{id}/events`

- **Auth.** Bearer required.
- **Request.** `?since=<EventId>` query parameter (i64). `since=0` for the start of history.
- **Response.** `404 Not Found` with body `{"error":"session not found"}` if the `session_id` has never existed (no row in `session_projections`); otherwise `200 OK` with an `EventListResponse` JSON body:

  ```json
  {
    "events": [
      {
        "event_id": 1,
        "source": "claude",
        "session_id": "session-alpha",
        "kind": "PreToolUse",
        "reaction": "Continue",
        "payload": "{...native JSON payload as a verbatim string...}",
        "created_at": 1748190000500,
        "pid": 12345,
        "cwd": "/Users/x/code/myrepo"
      }
    ],
    "cursor": 1,
    "oldest_available_event_id": 1
  }
  ```

- **Status codes.** `200`, `401`, `404 Not Found` (session-id was never seen).
- **Field source.** [`crates/protocol/src/rest.rs:18`](../crates/protocol/src/rest.rs) `EventListResponse`, [`crates/protocol/src/event.rs:30`](../crates/protocol/src/event.rs) `Event`.
- **Notes.** Loop until `cursor === null` to catch up to the tail. Per-event `cwd` (string-or-null, Story 5.7) is the source's hook-reported working directory for that specific event — the same value carried on WS `EventFrame.event.cwd`. It is `null` for events with no reported cwd (pre-5.7 rows, non-Claude sources, or a producer that omits it). `started_at` is NOT a per-event field — it is state/list-only (see `/sessions` and `/sessions/{id}`). Gap-detection: when your starting `since` is below the response's `oldest_available_event_id`, events in `(since, oldest_available)` were truncated. `oldest_available_event_id` is the global minimum across the whole event log (filtered to non-sentinel rows); `EventId(i64::MAX)` if the events table is empty. Per-event truncation gaps are silently skipped — the response continues with whatever is on disk. See [`cookbook/rest-cursor-pagination.md`](cookbook/rest-cursor-pagination.md) for the gap-window-rendering pattern.

### `GET /sessions/{id}/stats`

- **Auth.** Bearer required.
- **Request.** None.
- **Response.** `200 OK` with a `SessionStats` JSON body:

  ```json
  {
    "source": "claude",
    "session_id": "session-alpha",
    "event_count": 42,
    "first_event_at": 1748190000000,
    "last_event_at": 1748190001000
  }
  ```

- **Status codes.** `200`, `401`, `404 Not Found`.
- **Field source.** [`crates/protocol/src/rest.rs:25`](../crates/protocol/src/rest.rs) `SessionStats`.
- **Notes.** `first_event_at` and `last_event_at` are `null` for sessions with no surviving events (e.g. all events truncated). `first_event_at` is `MIN(created_at)` and `last_event_at` is `MAX(created_at)` over the session's stored events — they are pure timestamp aggregates, NOT the same field as `SessionState.started_at`. `started_at` is the `created_at` of the session's *first event by `event_id ASC` order* (so it matches a full rebuild, which folds events in `event_id` order). With monotonically increasing timestamps the two coincide, but under clock skew, manually injected data, or replay-reordered events they can diverge: `first_event_at` always reports the smallest timestamp, while `started_at` reports whichever timestamp belongs to the lowest `event_id`. Use `started_at` for "when did this session begin"; use `/stats` aggregates for min/max bookkeeping.

### `POST /replay`

- **Auth.** Bearer required.
- **Request.** JSONL body — one `protocol::Event` JSON object per line. The daemon ignores each line's `event_id` and `created_at` and reassigns both at projection-write time (AUTOINCREMENT + `current_unix_millis()`).
- **Response.** `200 OK` with a JSON body:

  ```json
  {
    "replayed_count": 2,
    "parse_errors": [
      { "line": 4, "error": "invalid JSON: ..." }
    ]
  }
  ```

- **Status codes.** `200`, `401`, `413 Payload Too Large` (body exceeded the 1 MiB cap).
- **Notes.** Replayed events flow through the same `ingest_tx → projection::session::write` path as live shim ingest — they're persisted, projected, and broadcast over WebSocket as `EventFrame` + `StateFrame` pairs. Sentinel kinds (`RecordingStarted`, `RecordingEnded`) are rejected at the replay boundary with a `parse_errors` entry (they're reserved for daemon-lifecycle emission). No rate limiting; the 1 MiB body cap is the only structural limit. Original inter-event timing is NOT preserved — events are forwarded as fast as the bounded `ingest_tx` channel accepts them, and `created_at` reflects replay wall-clock. Consequently a replayed session's `started_at` (set-once from the first event's `created_at`) is the replay wall-clock of the first replayed write, NOT the original exported timestamp; replay carries each event's stored `cwd` forward but does not thread the JSONL `created_at`. Story 4.1 introduced the endpoint plus the `bowerbird replay` and `bowerbird export` CLI commands that pair with it. See [`docs/protocol-changelog.md`](protocol-changelog.md) Story 4.1 entry.

## WebSocket endpoint and control mechanics

`GET /ws` — bearer-authenticated upgrade.

- **Auth.** `Authorization: Bearer <token>` header (preferred). For browser environments that cannot set headers (`new WebSocket(url)`), the daemon also accepts `?token=<token>` as a query parameter. If both are present the header wins; the query is NOT consulted as a fallback when the header is present and malformed.
- **Concurrent connection cap.** 256 (default; configurable via `Config::ws_max_connections`). Over-cap upgrades return `HTTP 503 Service Unavailable` **before** completing the upgrade — no WS handshake is performed.
- **Keep-alive.** Idle ping every 30s (`Config::ws_ping_interval`). If no pong arrives within 10s (`Config::ws_pong_timeout`), the daemon closes the connection at deadline-granularity (not the next tick after the deadline). Pong-miss closes use the standard WS close machinery.
- **Graceful shutdown.** On SIGTERM/SIGINT the daemon stops accepting new HTTP/WS upgrades, drains broadcast backlogs, emits one `ServerMessage::Close { reason: "daemon shutdown" }` per subscribed connection, then issues the WS control close. See [§ServerMessage variants → `close`](#close).
- **Inbound strictness.** `ClientMessage` is `deny_unknown_fields`. Unknown ops, extra fields, empty topics, unknown topics, binary frames, non-JSON payloads — all close the connection with WebSocket code `1008` (Policy Violation) plus a `bad message: ...` reason. The reason string is sanitized via the daemon's `sanitize_for_wire` and capped at 123 bytes per RFC 6455 §5.5.1.

Source: [`crates/daemon/src/api/ws.rs`](../crates/daemon/src/api/ws.rs), [`crates/daemon/src/config.rs`](../crates/daemon/src/config.rs), [`docs/protocol-changelog.md`](protocol-changelog.md) Story 2.1 / 2.5 entries.

## ClientMessage variants (inbound)

The two inbound shapes — tool → daemon. Both are strict-`deny_unknown_fields` ([`crates/protocol/src/ws.rs:31`](../crates/protocol/src/ws.rs)).

### `subscribe`

```json
{ "op": "subscribe", "topic": "state.session.*", "states": ["working", "waitinginput", "idle"] }
```

Subscribes the current connection to one topic (see [§Topic grammar](#topic-grammar) for the supported set).

**Snapshot dedup is per session, not per topic.** The daemon tracks the `(source, session_id)` rows it already holds current on this connection — rows delivered **either** by a snapshot burst **or** by a live `state` frame while covered by an active state subscription — and never re-sends a snapshot for one. (A live `state` frame keeps the row current on the connection, so an identical re-subscribe must not re-snapshot it.) Three consequences follow (all "no double-delivery"):

- Subscribing to the **same topic twice** is idempotent — the second subscribe emits zero snapshot frames. This holds whether or not a `states` filter is set: an identical filtered re-subscribe (e.g. `states:["ended"]` twice) emits nothing the second time.
- Subscribing to a **wildcard then a specific** sub-topic deduplicates — `state.session.*` followed by `state.session.<id>` emits zero additional snapshot frames for that id.
- **Widening** a filter re-sends only the newly-uncovered rows. After `states:["working"]`, the same topic unfiltered snapshots the sessions the narrow burst skipped (the `Ended` graveyard, etc.) and **not** the `Working` rows already sent.

Coverage **lapses on unsubscribe — but only when no remaining subscription still covers the session.** Unsubscribing drops snapshot coverage for a `(source, session_id)` only if no *other* active state subscription still covers it. So unsubscribing `state.session.*` while a `state.session.<id>` subscription is still active does **not** lapse coverage for `<id>` — its live state keeps flowing and a re-subscribe won't re-snapshot it. Unsubscribing the **last** topic covering a session does lapse it: the live stream that kept its snapshot current stops, so a later re-subscribe re-snapshots that session (carrying any state that drifted while it was uncovered). Snapshot coverage is per-connection — a fresh connection always gets a full snapshot.

Coverage **also lapses on lag.** If the connection falls behind and the daemon emits a [`dropped`](#dropped) frame, snapshot coverage is cleared: a dropped batch reports only a count, not the identities of the evicted envelopes, so the daemon cannot tell whether a live `state` frame for a covered session was lost. A re-subscribe after a `dropped` frame therefore re-snapshots, which is how a state-only subscriber recovers state it missed during the gap (it cannot replay missed `state` via `GET /sessions/{id}/events?since=`).

**`states` (optional, Story 5.8).** A list of `SessionCurrentState` tokens (case-insensitive: `idle`, `working`, `waitinginput`, `ended`, `unknown`) that scopes the **snapshot-on-subscribe burst** to sessions whose read-time `current_state` is in the set — keyed identically to the REST `GET /sessions?state=`. Absent or empty = unfiltered (the v1.0 default; every matching session is bursted). A triage presenter passes `["working","waitinginput","idle"]` to drop the `Ended` graveyard from the connect burst. The filter scopes **only** the initial snapshot — the live `state.*`/`events.*` stream is unaffected, so a session that transitions to/from `Ended` after subscribe still delivers a live `state` frame, and the full history (including `Ended`) is always available via REST `GET /sessions` (`?state=ended` for the graveyard alone). An invalid token closes the connection with `bad message` (close code 1008), the same way an invalid topic does. **`states` is only valid on a `state.session.*` family topic** — those are the only topics with a snapshot for it to scope. A non-empty `states` on an event topic (e.g. `events.*`) closes with `bad message` (1008) rather than being silently ignored (strict-inbound: a discarded filter would be a silent lie about the presenter's intent); an empty/absent `states` is fine on any topic. `states` is additive: a v1.0 presenter omitting it is unaffected (`#[serde(default)]`).

### `unsubscribe`

```json
{ "op": "unsubscribe", "topic": "state.session.*" }
```

Removes the topic from the current connection's subscription set. Unsubscribing from a topic you never subscribed to is a no-op.

## ServerMessage variants (outbound)

The seven outbound variants from [`crates/protocol/src/ws.rs:17-27`](../crates/protocol/src/ws.rs). Permissive on deserialize plus the `Unknown` catch-all for additive compat.

### `hello`

```json
{
  "op": "hello",
  "protocol_version": "1.0",
  "daemon_version": "0.1.0",
  "oldest_available_event_id": 1,
  "daemon_started_at": 1748190000000,
  "history_begins_cleanly": true
}
```

Sent once on every connection, before any subscription is processed. `protocol_version` is the literal `"1.0"` shipping today. `daemon_started_at` is unix-ms. `oldest_available_event_id` is the global minimum non-sentinel `event_id` still on disk (`i64::MAX` if the events table is empty); presenters use it as the gap-detection anchor on reconnect. `history_begins_cleanly` is `true` when the daemon has never truncated history (no events have aged out). Source: [`crates/protocol/src/ws.rs:37-44`](../crates/protocol/src/ws.rs).

### `sync`

```json
{
  "op": "sync",
  "oldest_available_event_id": 1,
  "latest_event_id": 42
}
```

NOT currently emitted by any daemon producer. The validated constructor `SyncFrame::new(oldest, latest)` ([`crates/protocol/src/ws.rs:62`](../crates/protocol/src/ws.rs)) enforces `oldest <= latest` at construction time; the type is `#[non_exhaustive]` to block external struct-literal construction. Reserved for forward use — a future story may activate it for "here's the available cursor window" disambiguation after recovery. Document the handler skeleton; do not act on it until a release announces emission.

### `event`

```json
{
  "op": "event",
  "event": {
    "event_id": 42,
    "source": "claude",
    "session_id": "session-alpha",
    "kind": "PostToolUse",
    "reaction": "Continue",
    "payload": "{...native JSON payload as a verbatim string...}",
    "created_at": 1748190001000,
    "pid": 12345,
    "cwd": "/Users/x/code/myrepo"
  }
}
```

The `event.cwd` field (Story 5.7) is the session's working directory for this event as the source's hook payload reported it, verbatim — `null` when the source omitted it. (`started_at` is NOT on `Event`; it is a session-state-only field — see the `state` frame below.)

For `SessionEnded` events (Story 5.3 — daemon-observed liveness), `payload` carries the mechanical observation as a JSON object: `{"reason":"no_pid_at_upgrade"|"pid_dead","pid":<number or null>,"observed_at_ms":<epoch_ms>}`. Presenters that want to render "session is dead" subscribe to `state.session.*` and watch for `current_state: "Ended"` — they do NOT need to call `kill(pid, 0)` themselves.

Emitted after every successful `projection::session::write` for non-sentinel events (the ingest path's single canonical publisher). Subscribers to `events.*`, `events.<source>.*`, or `events.<source>.<session_id>` receive matching frames. Source: [`crates/protocol/src/ws.rs:91`](../crates/protocol/src/ws.rs) `EventFrame`, [`crates/protocol/src/event.rs:30`](../crates/protocol/src/event.rs) `Event`.

The `event.reaction` field is `null` when the adapter cannot classify the tool — see [§Reaction enum](#reaction-enum). The `event.payload` field is the native tool payload as a verbatim JSON-encoded string (architecture.md Axiom 1: "native payloads ride verbatim").

### `state`

```json
{
  "op": "state",
  "source": "claude",
  "session_id": "session-alpha",
  "state": {
    "current_state": "Idle",
    "last_event_kind": "PostToolUse",
    "last_event_at_ms": 1748190001000,
    "last_pid": 12345,
    "cwd": "/Users/x/code/myrepo",
    "started_at": 1748190000000
  }
}
```

Emitted (a) on every `current_state` transition resulting from a projection write, and (b) as a snapshot on subscribe to any `state.*` topic (Story 2.3). Projection writes that update `last_event_kind` or `last_event_at_ms` without changing `current_state` produce no live `state` envelope — presenters compute freshness from the `events.*` stream. Snapshot frames apply the read-time stale-`Working` → `Idle` fallback (Story 1.6's `current_state_for_read`) just like the REST `/sessions/*` responses. Snapshot frames precede live frames on the same connection; subsequent live frames continue without gap. Source: [`crates/protocol/src/ws.rs:102`](../crates/protocol/src/ws.rs) `StateFrame`, [`crates/protocol/src/state.rs:13`](../crates/protocol/src/state.rs) `SessionState`.

`SessionCurrentState` is one of `Idle`, `Working`, `WaitingInput`, `Ended`, `Unknown`. `WaitingInput` means the session is blocked on user input with work queued behind the answer (`permission_prompt` / `elicitation_dialog`, incl. `AskUserQuestion`); as of Story 5.6 / ADR 0005 these are the only two `notification_type` values that *transition a session into* it. `idle_prompt` does NOT transition a session into `WaitingInput` — it is the idle nudge (~60s after a turn ends) and resolves to `Idle`, EXCEPT a session already in `WaitingInput` from a pending block stays `WaitingInput` (the nudge neither creates nor clears a block). Resolving to `Idle` (rather than preserving a prior `Working`) also covers a dropped `Stop`: a finished session whose `Stop` hook was lost still lands on `Idle` on the next idle nudge. The `Ended` variant (Story 5.3) is daemon-observed: the session's `last_pid` is no longer a live OS process. **`Ended` is non-terminal** — a session can transition out on the next hook event since the hook proves the process is alive (e.g. a `UserPromptSubmit` from `claude --resume` returns the session to `Working`). A `Notification` from `Ended` follows the same per-type rules as any other prior state: `permission_prompt` / `elicitation_dialog` transition to `WaitingInput`, while `idle_prompt` and the transient types (`auth_success` / `elicitation_response` / `elicitation_complete` / unknown / missing) resurrect to `Idle`. The `Unknown` variant is the additive-compat catch-all added in Story 4.4 (via `#[serde(other)]`): a future v1.x daemon may introduce new state values (e.g. `Compacting`, `AwaitingApproval`) which v1.0 presenters MUST decode as `Unknown` rather than erroring on the tag. The daemon never *produces* `Unknown` — it's decode-only, same shape as `ServerMessage::Unknown` (Story 2.1). Source: [`crates/protocol/src/state.rs`](../crates/protocol/src/state.rs).

`last_pid` (Story 5.3) is the carry-forward PID — set when an envelope carrying `bowerbird_ppid` projects, preserved across subsequent envelopes that don't carry one. Presenters do NOT need to call `kill(pid, 0)` themselves; the daemon runs a 5-second probe and emits `SessionEnded` for dead-or-no-PID rows, transitioning `current_state` to `Ended`.

`cwd` (Story 5.7) is the session's working directory as the source's hook payload reported it, carry-forwarded across events (overwrite-on-Some, identical to `last_pid`). It is a **mechanical fact**: the daemon stores it verbatim — no path canonicalization, no `~` expansion, no symlink resolution. *repo*, *project name*, and *branch* are presenter derivations from `cwd`, not daemon fields (Axiom 4, ADR 0006). `null` for sessions projected before Story 5.7, for non-Claude sources, or for a producer that omits it. `cwd` also rides each `Event` (above); `started_at` does not.

`started_at` (Story 5.7) is the epoch-ms timestamp of the session's first observed event, daemon-derived and set once (never updated). Presenters render session age from it without a side fetch. It is `null` for sessions projected before Story 5.7. A pre-5.7 row that keeps receiving events takes the first post-upgrade event's clock (an approximate "started just now"); a full rebuild reconstructs the exact first-event time. bowerbird is pre-release, so on a schema/projection change the supported upgrade path is to remove `~/.bowerbird/bower.db` and restart — a fresh db records an exact `started_at` for every session. A migration-era backfill (for `started_at` and any future projection-only field added after rows exist) lands when bowerbird ships a release whose databases must survive upgrades.

**V1 PID-only liveness: known limitations.** The probe checks `kill(last_pid, 0)` only — it confirms *some* process holds the PID, not that it's the original Claude Code session. Two scenarios this does not catch:

1. **PID reuse.** On a long-running host the OS will eventually recycle PIDs (Linux's 32k-PID default is small enough to wrap on busy machines). If the original Claude exited and the OS later reassigned its PID to an unrelated process, the probe will report the row alive when it is not.
2. **Wrong-parent reparenting.** `bowerbird_ppid` captures the shim's immediate parent at shim-invocation time. If a shell wrapper, `nohup`, or sandboxing tool sits between Claude and the shim, `last_pid` points at the wrapper, which may outlive the actual session.

V1 mitigates both via the projection's **overwrite-on-Some** carry-forward: every real hook event from a live session re-binds `last_pid` to the current `bowerbird_ppid`, so a session that's still actually firing hooks remains correctly tracked. A future story may capture process start time alongside the PID (`bowerbird_pstart`) and compare both on probe; see `docs/bmad/implementation-artifacts/deferred-work.md` entry "Process-birth marker for PID identity."

### `dropped`

```json
{
  "op": "dropped",
  "count": 73,
  "first_dropped_event_id": 100,
  "last_dropped_event_id": 172
}
```

Emitted when this connection's broadcast receiver lagged more than `ws_broadcast_capacity` positions (default 1024) behind the publisher. `count` is in **envelopes**, not bytes. `first_dropped_event_id` and `last_dropped_event_id` are **best-estimate upper-bound values** — the broadcast channel does not expose the post-lag cursor synchronously, so the daemon reports a bound, not an authoritative range. Presenters MUST recover from the cursor they authoritatively tracked from prior `event` frames, NOT from the ids inside `dropped`. Lag bursts are coalesced over `ws_broadcast_coalesce_window` (default 1s); sustained lag emits a bounded number of `dropped` frames per window. Source: [`crates/protocol/src/ws.rs:122`](../crates/protocol/src/ws.rs) `DroppedFrame`. The type is `#[non_exhaustive]` to block external struct-literal construction; daemon producers go through `DroppedFrame::new(count, first, last)` which enforces `count > 0` and `first <= last`. The socket stays open after `dropped` emission and live frames continue.

### `close`

```json
{ "op": "close", "reason": "daemon shutdown" }
```

Emitted before graceful shutdown's WS control close. Today's reason is `"daemon shutdown"` (Story 2.5); future stories may use other reasons (e.g. token rotation, idle-evict). `reason` is optional — `null` is a valid value if the daemon shuts down on a path without a reason string. Source: [`crates/protocol/src/ws.rs:169`](../crates/protocol/src/ws.rs) `CloseFrame`.

### `Unknown` (catch-all)

The variant-level additive-compat hatch. The daemon never *produces* `Unknown`; older client code (or third-party bindings) reading a newer daemon decode unrecognized variants as `Unknown` instead of erroring on the tag. Pattern in handlers: switch on `op`; default branch logs at debug and continues. Source: [`crates/protocol/src/ws.rs:25`](../crates/protocol/src/ws.rs) `#[serde(other)] Unknown`.

## Topic grammar

The six supported subscription topics. One topic per `Subscribe` message — no comma-separated lists, no batch shape.

- `events.*` — every persisted event, every session.
- `events.<source>.*` — every event from one adapter (e.g. `events.claude.*`).
- `events.<source>.<session_id>` — one specific session's events.
- `state.session.*` — every session's state changes (plus snapshot-on-subscribe).
- `state.session.<id>` — one session's state changes (plus snapshot).
- `state.session.<id>.current_state` — just the `current_state` sub-field of one session's state (high-frequency consumers).

Rules:

- **One string topic per Subscribe.** The `topic` field is a single string: a comma-list or array in the `topic` field, batched topics, and multiple `topic` fields are all malformed. (This rule constrains `topic` only — the sibling `states` field *is* an array; see [§subscribe](#subscribe).)
- **Unknown ops, unknown topics, empty topics, extra fields, binary frames, non-JSON** — all close the connection with WS code `1008` plus a `bad message: ...` reason (sanitized, capped at 123 bytes per RFC 6455 §5.5.1).
- **Snapshot dedup.** Subscribing to a wildcard then a specific sub-topic does NOT double-deliver snapshots (Story 2.3 dedup).
- **Wildcards are single-level.** `state.session.*` matches `state.session.<id>` but NOT `state.session.<id>.current_state`. Source: [`crates/daemon/src/broadcast/hub.rs`](../crates/daemon/src/broadcast/hub.rs) (matching logic).
- **`events.*` topics emit no snapshot.** Event history is fetched via REST `GET /sessions/{id}/events?since=0`, not via WebSocket replay.

## Ingest socket contract

- **Location.** `~/.bowerbird/ingest.sock` — a Unix-domain socket.
- **Auth.** Filesystem-only. Socket mode is `0600` (current OS user only). No bearer token; no challenge-response.
- **Producer.** In V1, the shim is the only producer. `bowerbird install` wires Claude Code's `~/.claude/settings.json` hooks to invoke `bowerbird-shim --hook-kind <KIND>`; the shim writes a single line of newline-delimited JSON to the socket then exits. Hot-path budget is p99 ≤5ms (the shim is `std`-only, no async runtime).
- **Wire framing.** One `{object}\n` in, one status line out. Newline-delimited JSON (NDJ). The daemon side: [`crates/daemon/src/ingest/listener.rs`](../crates/daemon/src/ingest/listener.rs) (accept loop) plus [`crates/daemon/src/ingest/handler.rs`](../crates/daemon/src/ingest/handler.rs) (per-line parse). The shim side: [`crates/shim/src/socket.rs`](../crates/shim/src/socket.rs). The framing decision is documented in [ADR-0002](decisions/0002-ingest-wire-framing-and-hook-kind.md).
- **`hook_kind` requirement** (Story 1.8, extended in Story 5.2). Every ingest line MUST carry a `hook_kind` field whose value is one of `PreToolUse`, `PostToolUse`, `Stop`, `Notification`, `UserPromptSubmit`. A missing field returns `400 missing hook_kind\n`. An unknown value returns `400 unknown hook_kind: <value>\n` where `<value>` is sanitized via the daemon's `sanitize_for_wire`. The shim injects `hook_kind` automatically on every payload — no consumer-side concern unless you're writing a custom producer.
- **`bowerbird_ppid` injection** (Story 5.3). The shim injects `bowerbird_ppid` (an integer equal to `libc::getppid()` at shim-invocation time — i.e. Claude Code's PID) on every payload. The daemon's adapter normalize step extracts it as `EventEnvelope.pid` which then projects through to `SessionState.last_pid`. A custom producer that wants the daemon to track liveness MUST inject `bowerbird_ppid`; without it, the session's `last_pid` stays `None` and the startup probe will emit `SessionEnded` with `reason: "no_pid_at_upgrade"` on the next daemon restart.
- **`cwd` extraction** (Story 5.7). Unlike `bowerbird_ppid` (shim-injected), `cwd` is a NATIVE Claude Code hook field present at the top level of every payload (alongside `session_id`, `transcript_path`, `hook_event_name`); the shim forwards it verbatim and the `adapter-claude` normalize step reads it as `EventEnvelope.cwd`, which projects to `SessionState.cwd` / `Event.cwd` (carry-forward / overwrite-on-Some). A custom producer that wants location tracking includes a top-level `cwd` string; absent or non-string → `SessionState.cwd` stays `None`. (`started_at` needs no producer cooperation — the daemon derives it from the first event it sees for a session.)
- **`notification_type` extraction** (Story 5.3; `idle_prompt` reclassified in Story 5.6 / ADR 0005). The adapter (NOT the shim) reads the upstream Claude Code `notification_type` field on `Notification`-kind payloads and uses the typed value to drive the projection transition via three rules: (1) input-required types (`permission_prompt`, `elicitation_dialog`) → `WaitingInput`; (2) `idle_prompt` → `Idle`, EXCEPT a prior `WaitingInput` is preserved — the idle nudge (~60s after a turn ends) signals the turn is over (so it covers a dropped `Stop`) but never clears a real block; (3) transient types (`auth_success`, `elicitation_response`, `elicitation_complete`) and any unknown/future/missing value → preserve prior `current_state`, except a prior `Ended` resurrects to `Idle` (a hook proves the process is alive). The typed value is NOT surfaced on `SessionState` or the wire `StateFrame` — it stays in `events.payload` (verbatim) for presenters that want richer rendering by subscribing to the events stream.
- **Framing rationale.** NDJ is a deliberate choice for **shim-dependency minimalism**, NOT a latency optimization. The shim is `std`-only with no async runtime; any framing more complex than "write a line, exit" would require pulling in a parser or state machine that violates the p99 ≤5ms budget. This narration is load-bearing — the Epic 1 retrospective (Agreement A3) and Epic 2 retrospective (AI-6) both explicitly mandate that protocol.md describe the choice this way, so a future presenter author building a custom shim understands the constraint hierarchy.
- **Adapter trait.** `SourceAdapter` is the V1 extension point for new event sources (Codex, OpenCode, etc.). The `adapter-claude` crate is the reference implementation. Source: [`crates/protocol/src/adapter.rs`](../crates/protocol/src/adapter.rs) — `SourceAdapter` trait, `NormalizeResult`, `AdapterMeta`. V2 may move adapters to subprocesses; for V1, in-process is the model.

## EventKind enum

The nine values from [`crates/protocol/src/event.rs:9`](../crates/protocol/src/event.rs):

| Value | User-facing? | Meaning |
|---|---|---|
| `UserPromptSubmit` | yes | The user submitted a prompt; Claude is about to start a turn |
| `PreToolUse` | yes | Claude is about to invoke a tool |
| `PostToolUse` | yes | A tool invocation completed; projection transitions to `Working` (Story 5.3 — was: preserve prior under Story 5.2) |
| `Stop` | yes | Claude finished a turn |
| `Notification` | yes | A non-tool side-channel event. `current_state` depends on the payload's `notification_type` field (Story 5.3; reworked by Story 5.6 / ADR 0005): `permission_prompt`, `elicitation_dialog` → `WaitingInput`; `idle_prompt` → `Idle` (except a prior `WaitingInput` is preserved); `auth_success`, `elicitation_response`, `elicitation_complete`, future-unknown, or missing → preserve prior (except a prior `Ended` → `Idle`) |
| `SessionEnded` | yes — daemon-observed | Daemon's 5-second liveness probe (Story 5.3) observed that the session's `last_pid` is no longer a live OS process. Projection transitions to `Ended`. **Non-terminal** — a subsequent hook event (e.g. `claude --resume`) transitions back out via the normal rules |
| `RecordingStarted` | **no — internal sentinel** | Daemon started a recording session |
| `RecordingEnded` | **no — internal sentinel** | Daemon ended a recording session |
| `Unknown` | **decode-only catch-all** | Forward-compat hatch added in Story 4.4 via `#[serde(other)]`; v1.0 presenters decode future v1.x event kinds as `Unknown` instead of erroring on the tag |

Sentinel events are stored with `source = "__daemon__"` and filtered out of every wire emission (REST, WebSocket, replay). The daemon never *produces* `Unknown` — adapter `normalize` rejects unknown hook strings at the boundary, `event_kind_as_str` debug-asserts against persisting Unknown to SQLite, and `POST /replay` rejects Unknown at the JSONL parse boundary with a clear "this build is older than the source daemon" message. `Unknown` is strictly a wire-decode safety net for v1.x → v1.0 forward-compat.

## Reaction enum

The classification an adapter assigns to a tool invocation. Source: [`crates/protocol/src/reaction.rs`](../crates/protocol/src/reaction.rs).

| Variant | JSON form | Meaning |
|---|---|---|
| `Pause` | `"Pause"` | Tool warrants pausing the presenter UI / signaling attention |
| `Continue` | `"Continue"` | Routine tool; no presenter action needed |
| `Vendor(u16)` | `"Vendor(<n>)"` | Adapter-defined extension; `n` is the vendor-specific code |
| `Unknown` | `"Unknown"` | Adapter could not classify (e.g. tool name not in `tool-reactions.toml`) |

The `Vendor(u16)` variant uses a custom string serializer/deserializer (the `Serialize` and `Deserialize` impls in `reaction.rs`) — the wire form is `"Vendor(42)"`, not `{"Vendor":42}`. This keeps the on-wire shape stable while allowing adapter authors to mint vendor codes without modifying the protocol crate.

## EventEnvelope vs Event

Two related types, deliberately distinct:

- **`EventEnvelope`** — pre-storage shape ([`crates/protocol/src/event.rs:20`](../crates/protocol/src/event.rs)). Carries `source`, `session_id`, `kind`, `reaction`, `payload`. No `event_id`, no `created_at` — the daemon assigns both at `INSERT` time (AUTOINCREMENT + `current_unix_millis()`). Used internally on the ingest channel; NEVER on the wire.
- **`Event`** — post-storage shape ([`crates/protocol/src/event.rs:30`](../crates/protocol/src/event.rs)). The `EventEnvelope` fields plus `event_id` and `created_at`. This is the on-wire shape — every WebSocket `EventFrame` and every REST `EventListResponse.events[]` element is an `Event`.

The `POST /replay` endpoint accepts `Event`-shaped JSON lines but discards each line's `event_id` and `created_at` (the daemon reassigns both). Round-tripping `bowerbird export <id> | bowerbird replay /dev/stdin` therefore produces fresh event_ids on the replayed events, NOT the originals.

## Versioning and compat policy

The wire surface follows additive-only semver within `1.x`:

- No field removal from any outbound type.
- No required-field addition to any outbound type (additive optional fields are fine; older clients ignore them per the asymmetric serde policy).
- No required-field addition to any inbound type.
- No breaking semantic change to any existing operation.
- New `ServerMessage` variants are additive — older clients decode them as `Unknown`.
- New REST endpoints are additive.
- New `ClientMessage` variants are additive — older clients never send them.

The phrasing from NFR19 and FR36: **No breaking changes to the REST or WebSocket protocol within any v1.x release series; tools built against v1.0 continue to work on any v1.x daemon without modification.**

A `2.0` release would imply at least one of the above breaking. There is no such release planned; the policy is the contract.

For the change history — what shipped when, what was deferred, what motivated each addition — see [`docs/protocol-changelog.md`](protocol-changelog.md). Story 4.4 will land the mechanical contract test suite that enforces these constraints in CI; until then the discipline is documented + reviewer-enforced.

## Further reading

- [`docs/presenter-authoring.md`](presenter-authoring.md) — conceptual guide to building tools against this protocol.
- [`docs/cookbook/`](cookbook/) — canonical recipes for common patterns.
- [`docs/protocol-changelog.md`](protocol-changelog.md) — change history.
- [`docs/decisions/`](decisions/) — ADRs (project name, ingest framing, shim performance budget).
