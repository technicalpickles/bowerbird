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
- **Request.** None.
- **Response.** `200 OK` with a JSON array of `SessionListItem`:

  ```json
  [
    {
      "source": "claude",
      "session_id": "session-alpha",
      "current_state": "Idle",
      "last_event_kind": "PostToolUse",
      "last_event_at_ms": 1748190001000,
      "updated_at": 1748190001000
    }
  ]
  ```

- **Field source.** [`crates/protocol/src/rest.rs:38`](../crates/protocol/src/rest.rs) `SessionListItem`.
- **Notes.** `current_state` is the read-time projection (stale-`Working` → `Idle` fallback per Story 1.6's `current_state_for_read`), NOT the raw stored value. Sentinel-source sessions (`source = "__daemon__"`) are filtered out.

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
      "last_event_at_ms": 1748190001000
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
- **Response.** `200 OK` with an `EventListResponse` JSON body:

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
        "created_at": 1748190000500
      }
    ],
    "cursor": 1,
    "oldest_available_event_id": 1
  }
  ```

- **Status codes.** `200`, `401`, `404 Not Found` (session-id was never seen).
- **Field source.** [`crates/protocol/src/rest.rs:18`](../crates/protocol/src/rest.rs) `EventListResponse`, [`crates/protocol/src/event.rs:30`](../crates/protocol/src/event.rs) `Event`.
- **Notes.** Loop until `cursor === null` to catch up to the tail. Gap-detection: when your starting `since` is below the response's `oldest_available_event_id`, events in `(since, oldest_available)` were truncated. `oldest_available_event_id` is the global minimum across the whole event log (filtered to non-sentinel rows); `EventId(i64::MAX)` if the events table is empty. Per-event truncation gaps are silently skipped — the response continues with whatever is on disk. See [`cookbook/rest-cursor-pagination.md`](cookbook/rest-cursor-pagination.md) for the gap-window-rendering pattern.

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
- **Notes.** `first_event_at` and `last_event_at` are `null` for sessions with no surviving events (e.g. all events truncated).

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
- **Notes.** Replayed events flow through the same `ingest_tx → projection::session::write` path as live shim ingest — they're persisted, projected, and broadcast over WebSocket as `EventFrame` + `StateFrame` pairs. Sentinel kinds (`RecordingStarted`, `RecordingEnded`) are rejected at the replay boundary with a `parse_errors` entry (they're reserved for daemon-lifecycle emission). No rate limiting; the 1 MiB body cap is the only structural limit. Original inter-event timing is NOT preserved — events are forwarded as fast as the bounded `ingest_tx` channel accepts them, and `created_at` reflects replay wall-clock. Story 4.1 introduced the endpoint plus the `bowerbird replay` and `bowerbird export` CLI commands that pair with it. See [`docs/protocol-changelog.md`](protocol-changelog.md) Story 4.1 entry.

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
{ "op": "subscribe", "topic": "state.session.*" }
```

Subscribes the current connection to one topic (see [§Topic grammar](#topic-grammar) for the supported set). Subscribing to the same topic twice on one connection is idempotent (no double-delivery). Subscribing to a wildcard then a specific sub-topic deduplicates snapshots — `state.session.*` followed by `state.session.<id>` emits zero additional snapshot frames for that id.

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
    "created_at": 1748190001000
  }
}
```

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
    "last_event_at_ms": 1748190001000
  }
}
```

Emitted (a) on every `current_state` transition resulting from a projection write, and (b) as a snapshot on subscribe to any `state.*` topic (Story 2.3). Projection writes that update `last_event_kind` or `last_event_at_ms` without changing `current_state` produce no live `state` envelope — presenters compute freshness from the `events.*` stream. Snapshot frames apply the read-time stale-`Working` → `Idle` fallback (Story 1.6's `current_state_for_read`) just like the REST `/sessions/*` responses. Snapshot frames precede live frames on the same connection; subsequent live frames continue without gap. Source: [`crates/protocol/src/ws.rs:102`](../crates/protocol/src/ws.rs) `StateFrame`, [`crates/protocol/src/state.rs:13`](../crates/protocol/src/state.rs) `SessionState`.

`SessionCurrentState` is one of `Idle`, `Working`, `WaitingInput`, `Unknown`. The `Unknown` variant is the additive-compat catch-all added in Story 4.4 (via `#[serde(other)]`): a future v1.x daemon may introduce new state values (e.g. `Compacting`, `AwaitingApproval`) which v1.0 presenters MUST decode as `Unknown` rather than erroring on the tag. The daemon never *produces* `Unknown` — it's decode-only, same shape as `ServerMessage::Unknown` (Story 2.1). Source: [`crates/protocol/src/state.rs`](../crates/protocol/src/state.rs).

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

- **One topic per Subscribe.** Comma-lists, arrays, and multiple `topic` fields are all malformed.
- **Unknown ops, unknown topics, empty topics, extra fields, binary frames, non-JSON** — all close the connection with WS code `1008` plus a `bad message: ...` reason (sanitized, capped at 123 bytes per RFC 6455 §5.5.1).
- **Snapshot dedup.** Subscribing to a wildcard then a specific sub-topic does NOT double-deliver snapshots (Story 2.3 dedup).
- **Wildcards are single-level.** `state.session.*` matches `state.session.<id>` but NOT `state.session.<id>.current_state`. Source: [`crates/daemon/src/broadcast/hub.rs`](../crates/daemon/src/broadcast/hub.rs) (matching logic).
- **`events.*` topics emit no snapshot.** Event history is fetched via REST `GET /sessions/{id}/events?since=0`, not via WebSocket replay.

## Ingest socket contract

- **Location.** `~/.bowerbird/ingest.sock` — a Unix-domain socket.
- **Auth.** Filesystem-only. Socket mode is `0600` (current OS user only). No bearer token; no challenge-response.
- **Producer.** In V1, the shim is the only producer. `bowerbird install` wires Claude Code's `~/.claude/settings.json` hooks to invoke `bowerbird-shim --hook-kind <KIND>`; the shim writes a single line of newline-delimited JSON to the socket then exits. Hot-path budget is p99 ≤5ms (the shim is `std`-only, no async runtime).
- **Wire framing.** One `{object}\n` in, one status line out. Newline-delimited JSON (NDJ). The daemon side: [`crates/daemon/src/ingest/listener.rs`](../crates/daemon/src/ingest/listener.rs) (accept loop) plus [`crates/daemon/src/ingest/handler.rs`](../crates/daemon/src/ingest/handler.rs) (per-line parse). The shim side: [`crates/shim/src/socket.rs`](../crates/shim/src/socket.rs). The framing decision is documented in [ADR-0002](decisions/0002-ingest-wire-framing-and-hook-kind.md).
- **`hook_kind` requirement** (Story 1.8, extended in Story 5.7). Every ingest line MUST carry a `hook_kind` field whose value is one of `PreToolUse`, `PostToolUse`, `Stop`, `Notification`, `UserPromptSubmit`. A missing field returns `400 missing hook_kind\n`. An unknown value returns `400 unknown hook_kind: <value>\n` where `<value>` is sanitized via the daemon's `sanitize_for_wire`. The shim injects `hook_kind` automatically on every payload — no consumer-side concern unless you're writing a custom producer.
- **Framing rationale.** NDJ is a deliberate choice for **shim-dependency minimalism**, NOT a latency optimization. The shim is `std`-only with no async runtime; any framing more complex than "write a line, exit" would require pulling in a parser or state machine that violates the p99 ≤5ms budget. This narration is load-bearing — the Epic 1 retrospective (Agreement A3) and Epic 2 retrospective (AI-6) both explicitly mandate that protocol.md describe the choice this way, so a future presenter author building a custom shim understands the constraint hierarchy.
- **Adapter trait.** `SourceAdapter` is the V1 extension point for new event sources (Codex, OpenCode, etc.). The `adapter-claude` crate is the reference implementation. Source: [`crates/protocol/src/adapter.rs`](../crates/protocol/src/adapter.rs) — `SourceAdapter` trait, `NormalizeResult`, `AdapterMeta`. V2 may move adapters to subprocesses; for V1, in-process is the model.

## EventKind enum

The eight values from [`crates/protocol/src/event.rs:9`](../crates/protocol/src/event.rs):

| Value | User-facing? | Meaning |
|---|---|---|
| `UserPromptSubmit` | yes | The user submitted a prompt; Claude is about to start a turn |
| `PreToolUse` | yes | Claude is about to invoke a tool |
| `PostToolUse` | yes | A tool invocation completed |
| `Stop` | yes | Claude finished a turn |
| `Notification` | yes | A non-tool side-channel event (e.g. permission prompt) |
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
