# Presenter authoring

A *presenter* is any tool that connects to the bowerbird daemon and consumes its outbound surface: REST for history and snapshots, WebSocket for live events and state. This guide explains the pieces and how they compose. It's the second stop after the [Quickstart](quickstart.md) and the prerequisite for reading [`docs/cookbook/`](cookbook/), which gives end-to-end recipes.

The examples here use TypeScript on Node 22.6+, mirroring the three [reference tools](../examples/) under `examples/`. The substrate doesn't care what speaks WebSocket and JSON; any language with a JSON parser and a WebSocket client works the same way.

## The substrate model

The daemon is a long-running local process started by `bowerbird install` (which wires it into Claude Code's hooks) or `bowerbird start` (which leaves Claude Code alone). It accepts hook events on a Unix-domain socket, persists them to a local SQLite database, projects per-session state, and broadcasts both events and state changes to whatever tools have connected. Presenters talk to the daemon over two surfaces: REST for history and point-in-time snapshots, WebSocket for live events plus snapshot-on-subscribe semantics.

```
Claude Code  ──hooks──▶  bowerbird-shim
                              │
                              ▼
                         ingest.sock
                              │
                              ▼
                    ┌─────────────────────┐
                    │ bowerbird-daemon    │
                    │  ┌───────────────┐  │
                    │  │ SQLite        │  │
                    │  │ + pub/sub hub │  │
                    │  └───────────────┘  │
                    └──────┬──────────────┘
                           │
                  REST     │     WebSocket
                  (history)│     (live + snapshot)
                           ▼
                       presenter
```

Source: route declarations at [`crates/daemon/src/api/mod.rs:99-114`](../crates/daemon/src/api/mod.rs); data flow in [`docs/bmad/planning-artifacts/architecture.md` §Data Flow](bmad/planning-artifacts/architecture.md).

## Establishing a WebSocket connection

Three pieces: the bound address, the bearer token, and the upgrade request.

**Bind address.** The daemon binds `127.0.0.1` plus a kernel-assigned ephemeral port. It writes the resolved address to `~/.bowerbird/server.json` (mode 0600) once the listener is up. Read that file to discover where to connect:

```ts
import { homedir } from "node:os";
import { readFileSync } from "node:fs";
import { join } from "node:path";

interface ServerInfo { bind_addr: string; }

const { bind_addr } = JSON.parse(
  readFileSync(join(homedir(), ".bowerbird", "server.json"), "utf8"),
) as ServerInfo;
```

The `ServerInfo` shape lives in [`crates/protocol/src/rest.rs`](../crates/protocol/src/rest.rs). It's permissive on deserialize — a future daemon adding fields will not break older readers (see [`protocol.md` §Wire format and conventions](protocol.md#wire-format-and-conventions) for the asymmetric serde policy).

**Bearer token.** The daemon resolves its bearer token from `BOWERBIRD_TOKEN` env, system keychain, or `~/.bowerbird/config.toml` (in that order). The CLI's `bowerbird auth token` reproduces the same chain and prints the resolved value to stdout. For presenters, the env var is the canonical source — it's what every example uses, and pulling it from the CLI subprocess adds latency to startup:

```ts
const token = process.env.BOWERBIRD_TOKEN;
if (!token) throw new Error("BOWERBIRD_TOKEN not set; run `bowerbird auth token`.");
```

**Upgrade request.** Construct the WebSocket against `ws://<bind_addr>/ws` and send the bearer in the `Authorization` header:

```ts
const ws = new WebSocket(`ws://${bind_addr}/ws`, {
  // @ts-expect-error -- Node's undici WebSocket accepts an options bag with
  // `headers`; the DOM lib's constructor type does not.
  headers: { Authorization: `Bearer ${token}` },
});
```

The `@ts-expect-error` is necessary because Node's `WebSocket` (backed by undici) extends the browser signature with a per-request options bag, but `tsc`'s DOM lib types describe only the two-arg browser constructor. The three reference examples all carry the same suppression.

For browser environments where you cannot set headers (e.g. `new WebSocket(url)` from page JS), pass the token as a query parameter instead: `ws://${bind_addr}/ws?token=${token}`. The daemon accepts either; the header wins if both are present (`Authorization` header → token, even if the query string also has one). This fallback shipped with the WebSocket surface in Story 2.1 — see [`docs/protocol-changelog.md`](protocol-changelog.md).

## Sending a Subscribe message

A subscription is one `ClientMessage::Subscribe` per topic:

```ts
ws.send(JSON.stringify({ op: "subscribe", topic: "state.session.*" }));
```

One topic per message — no comma-separated lists. Inbound `ClientMessage` is strict-`deny_unknown_fields` (see [`crates/protocol/src/ws.rs:31`](../crates/protocol/src/ws.rs)): extra fields, unknown ops, empty topics, unknown topics, binary frames, and non-JSON payloads all close the connection with WebSocket code 1008 (Policy Violation) plus a `bad message: ...` reason (sanitized, capped at 123 bytes per RFC 6455 §5.5.1). The reason string tells you which contract you violated; log it before reconnecting.

The supported topics:

| Topic | Delivers |
|---|---|
| `events.*` | every persisted event, every session |
| `events.<source>.*` | every event from one adapter (e.g. `events.claude.*`) |
| `events.<source>.<session_id>` | one specific session's events |
| `state.session.*` | every session's state changes (plus snapshot-on-subscribe) |
| `state.session.<id>` | one session's state changes (plus snapshot) |
| `state.session.<id>.current_state` | just the `current_state` field of one session |

`state.*` subscriptions get a snapshot of matching session projections on subscribe — the daemon walks `session_projections` ordered `updated_at DESC` and emits one `state` frame per matching session BEFORE any live frames. That's why a multi-session router never needs a separate `GET /sessions` to bootstrap. `events.*` subscriptions get NO snapshot — pull event history via REST (`GET /sessions/{id}/events?since=0`) instead.

Subscribing to the same topic twice on one connection is idempotent. Subscribing to a wildcard first, then a specific sub-topic, deduplicates the snapshot — `state.session.*` followed by `state.session.<id>` does not double-emit the matching session.

See [`protocol.md` §Topic grammar](protocol.md#topic-grammar) for the full grammar.

## Handling each ServerMessage frame

Every outbound message carries an `op` discriminator. The seven concrete variants plus the `Unknown` catch-all are defined in [`crates/protocol/src/ws.rs`](../crates/protocol/src/ws.rs); below is the per-variant guide, with short TypeScript handler skeletons.

### `hello`

Sent once on connection. Shape:

```json
{
  "op": "hello",
  "protocol_version": "0.1.0",
  "daemon_version": "0.1.0",
  "oldest_available_event_id": 1,
  "daemon_started_at": 1748190000000,
  "history_begins_cleanly": true
}
```

```ts
if (msg.op === "hello") {
  console.error(`connected to daemon ${msg.daemon_version} (protocol ${msg.protocol_version})`);
  // Use msg.oldest_available_event_id for gap-detection on reconnect.
}
```

Source: [`crates/protocol/src/ws.rs:38`](../crates/protocol/src/ws.rs) `HelloFrame`. The `oldest_available_event_id` is your gap-detection anchor: if your last-seen `event_id` is below it on reconnect, events were truncated between sessions.

### `event`

Every persisted, non-sentinel event. Shape:

```json
{
  "op": "event",
  "event": {
    "event_id": 42,
    "source": "claude",
    "session_id": "session-alpha",
    "kind": "PostToolUse",
    "reaction": "Continue",
    "payload": "{...native tool payload as a verbatim JSON string...}",
    "created_at": 1748190001000
  }
}
```

```ts
if (msg.op === "event") {
  lastEventId = Math.max(lastEventId, msg.event.event_id);
  // ...consume msg.event.kind / .session_id / .payload as your tool needs.
}
```

Source: [`crates/protocol/src/ws.rs:91`](../crates/protocol/src/ws.rs) `EventFrame`, [`crates/protocol/src/event.rs:30`](../crates/protocol/src/event.rs) `Event`. The `payload` field is a verbatim JSON-encoded string — the native tool payload rides through unchanged ("native payloads ride verbatim"). Parse it lazily, only when you need to read inside.

The user-facing `EventKind` values are `PreToolUse`, `PostToolUse`, `Stop`, `Notification`. The daemon also has two internal sentinels (`RecordingStarted`, `RecordingEnded`) that are NEVER broadcast — they're persisted with `source = "__daemon__"` for lifecycle bookkeeping but filtered out before any wire emission.

### `state`

Every projection write — fired on ingest and on `state.*` subscribe (snapshot). Shape:

```json
{
  "op": "state",
  "source": "claude",
  "session_id": "session-alpha",
  "state": {
    "current_state": "Idle",
    "last_event_kind": "PostToolUse",
    "last_event_at_ms": 1748190001000,
    "last_pid": 12345
  }
}
```

```ts
if (msg.op === "state") {
  const key = `${msg.source}/${msg.session_id}`;
  if (!seen.has(key)) console.error(`new session: ${key}`);
  seen.set(key, msg.state);
}
```

Source: [`crates/protocol/src/ws.rs:102`](../crates/protocol/src/ws.rs) `StateFrame`, [`crates/protocol/src/state.rs:13`](../crates/protocol/src/state.rs) `SessionState`. `SessionCurrentState` is one of `Idle`, `Working`, `WaitingInput`, `Ended`. The `current_state` is the read-time projection — the daemon's `current_state_for_read` applies a stale-`Working` → `Idle` fallback (Story 1.6) so a session that started a tool call and never recorded a matching `PostToolUse` won't pin to `Working` forever.

**Rendering `Ended` sessions** (Story 5.3). `Ended` means the daemon's 5-second liveness probe observed that the session's `last_pid` is no longer a live OS process — typically because the user closed the terminal without firing `Stop`. Default presenter behavior: hide `Ended` rows (they're not actionable). Alternative: dim or strike-through (preserves the deck's last-N-sessions history). **Do NOT call `kill(pid, 0)` from the presenter** — the substrate has already done that and emitted the mechanical observation; presenters subscribe to `state.session.*` and react. `Ended` is **non-terminal**: a `claude --resume` triggers a new `UserPromptSubmit` hook for the same `(source, session_id)`, transitioning the row back to `Working`.

**Rendering `WaitingInput` sessions** (Story 5.3; reworked by Story 5.6 / ADR 0005). The substrate uses Claude Code's typed `notification_type` field on `Notification` events to decide whether the session is genuinely waiting for input. Only `permission_prompt` and `elicitation_dialog` (incl. `AskUserQuestion`) cause `current_state` to transition into `WaitingInput`. `idle_prompt` (the idle nudge Claude fires ~60s after a turn ends) resolves to `Idle` — it is not a block — EXCEPT a session already in `WaitingInput` stays there (an idle nudge neither creates nor clears a block). The remaining transient notifications (`auth_success`, `elicitation_response`, `elicitation_complete`, unknown/missing) preserve prior state, except a prior `Ended` resurrects to `Idle` (any notification hook is evidence the process is alive). The typed value is NOT surfaced on `SessionState` — it stays in `events.payload` (verbatim). Presenters that want to distinguish "Claude is asking for a tool-use permission" from "Claude is idle and pinging" subscribe to `events.<source>.<session_id>` (or `events.*`) and parse `notification_type` from the payload themselves.

For the canonical per-session fan-out pattern (subscribe to `state.session.*`, route by `(source, session_id)`), see [`cookbook/state-session-fanout.md`](cookbook/state-session-fanout.md). It pairs with [`examples/multi-session-router/`](../examples/multi-session-router/).

### `sync`

Currently unused on outbound. The validated constructor exists ([`crates/protocol/src/ws.rs:62`](../crates/protocol/src/ws.rs) `SyncFrame::new`, added in Story 2.3 to enforce `oldest <= latest`), but no daemon producer activates it as of this release. Document it as the `Unknown`-tolerant pattern: your handler should accept the variant gracefully when a future story turns it on.

### `dropped`

Sent when this connection's broadcast receiver fell more than `ws_broadcast_capacity` positions (default 1024) behind the publisher. Shape:

```json
{
  "op": "dropped",
  "count": 73,
  "first_dropped_event_id": 100,
  "last_dropped_event_id": 172
}
```

```ts
if (msg.op === "dropped") {
  // DO NOT use msg.first_dropped_event_id / .last_dropped_event_id as
  // cursors — they are best-estimate upper bounds. Use the lastEventId
  // YOU tracked from prior `event` frames.
  enterRecoveryMode(lastEventId);
}
```

Source: [`crates/protocol/src/ws.rs:122`](../crates/protocol/src/ws.rs) `DroppedFrame`. The `count` is in envelopes (not bytes). The first/last event ids are best-estimate — the broadcast channel does not expose the post-lag cursor synchronously, so the daemon reports an upper bound. Always recover from the cursor you tracked from `event` frames, never from the ids inside `dropped`. The socket stays open after a `dropped` emission; the recovery is your job, not the daemon's.

The full recovery flow lives in [`cookbook/dropped-frame-recovery.md`](cookbook/dropped-frame-recovery.md). It pairs with [`examples/reconnect-recovery/`](../examples/reconnect-recovery/).

### `close`

Sent before graceful shutdown. Shape:

```json
{ "op": "close", "reason": "daemon shutdown" }
```

```ts
if (msg.op === "close") {
  console.error(`daemon closed: ${msg.reason ?? "no reason given"}`);
  // Clean shutdown — close the socket, optionally reconnect with backoff.
  ws.close();
}
```

Source: [`crates/protocol/src/ws.rs:169`](../crates/protocol/src/ws.rs) `CloseFrame`. SIGTERM and SIGINT both trigger this path: the daemon stops accepting new connections, drains broadcast backlogs, emits the protocol `close` to each subscribed client, then sends the WebSocket control close. Treat it as "the daemon is going away; the next connection attempt will need backoff."

### `Unknown` (catch-all)

The catch-all variant for additive compat. If a v1.x daemon ships a new `ServerMessage` variant your code doesn't recognize, `JSON.parse` will still succeed and you'll see an `op` string you don't recognize. Route it through your default branch:

```ts
default:
  console.error(`unhandled op: ${msg.op}`);
  // continue; don't crash on a future-additive variant
```

This is what makes "additive within v1.x" real ([`crates/protocol/src/ws.rs:25`](../crates/protocol/src/ws.rs) `#[serde(other)] Unknown`). The asymmetric `deny_unknown_fields` policy alone covers struct fields, not enum variants; the catch-all closes the gap. The daemon never *produces* `Unknown` — it's a decode-only fallback for older client code reading a newer daemon.

## The dropped-frame recovery loop

The `close` / `dropped` / unsolicited-disconnect handling all collapse into one pattern: the substrate told you it can no longer guarantee live delivery, so consult REST to catch up.

The cursor that matters is the `last_event_id` your code tracked from prior `event` frames. NOT the ids inside `dropped` (best-estimate upper bounds, per Story 2.4). NOT the `HelloFrame.oldest_available_event_id` (that's the global window minimum, not your position).

The shape of the recovery:

1. **Discover sessions.** `GET /sessions` returns a `SessionListItem[]` for every session the daemon knows about. On a tool that watches all sessions, this is the universe to walk.
2. **Page each session.** For each session, `GET /sessions/<id>/events?since=<your-cursor>` in a loop until `cursor === null`. The daemon's `EventListResponse` carries `events: Event[]`, `cursor: EventId | null`, and `oldest_available_event_id: EventId`.
3. **Detect unrecoverable gaps.** Before consuming events from a response, check `since < oldest_available_event_id`. If your cursor predates the oldest available event, history was truncated and the events in `(since, oldest_available)` are gone for good. Log the gap range and continue with what survived.
4. **Reconnect.** Open a new WebSocket, subscribe again. Live frames resume from the daemon's current position; your local cursor is now consistent with the live stream.

See [`cookbook/dropped-frame-recovery.md`](cookbook/dropped-frame-recovery.md) for the full implementation, pinned to [`examples/reconnect-recovery/`](../examples/reconnect-recovery/) via a cookbook-include directive that fails CI on drift.

## Fetching a REST snapshot

Three operations cover the REST surface presenters use day-to-day:

- **`GET /sessions`** → `SessionListItem[]`. List every session the daemon has ever seen, with each one's current state, last event kind, last event timestamp, and updated_at. Use it on cold-start to bootstrap your universe of sessions.

- **`GET /sessions/{id}`** → `SessionDetail { source, session_id, state, updated_at }`. The point-in-time state for one specific session, without subscribing to its live stream. The `state.current_state` field reflects the read-time stale-`Working` → `Idle` fallback (same as the `state` WebSocket frame).

- **`GET /sessions/{id}/events?since=<cursor>`** → `EventListResponse { events: Event[], cursor: EventId | null, oldest_available_event_id: EventId }`. Page through history. Start with `since=0` to fetch from the beginning; loop until `cursor === null`. Gap-detection: if your starting `since` is below `oldest_available_event_id`, events in that range were truncated.

All three require `Authorization: Bearer <token>`. The error story:

- **401 Unauthorized** — token wrong, expired, or missing. Re-resolve via `BOWERBIRD_TOKEN` / `bowerbird auth token`.
- **404 Not Found** — the session-id was never seen, or has been truncated out of existence.
- **5xx** — daemon problem. Retry with exponential backoff; the daemon may be restarting or under load.

The full per-endpoint reference, including response field-by-field shapes copied from the source-of-truth structs, lives in [`docs/protocol.md`](protocol.md).

For the canonical REST cursor-pagination + gap-detection pattern, see [`cookbook/rest-cursor-pagination.md`](cookbook/rest-cursor-pagination.md). It pairs with [`examples/event-log-viewer/`](../examples/event-log-viewer/).

## Putting it together

A long-running presenter composes the pieces like this:

```
on startup:
  GET /sessions                                      # bootstrap session universe
  open WS, subscribe (state.session.* or events.*)
  on `hello`:
    record oldest_available_event_id                 # gap-detection anchor
  on `event`:
    lastEventId = max(lastEventId, event_id)         # cursor maintenance
    update local model
  on `state`:
    update per-session map (snapshot frames arrive first; live frames follow)
  on `dropped` | `close` | unsolicited close:
    recover(lastEventId)                             # REST catch-up; see cookbook
    open WS, subscribe again                         # resume live stream
```

The three reference examples under [`examples/`](../examples/) implement different slices of this:

- [`multi-session-router/`](../examples/multi-session-router/) — `state.session.*` fan-out with snapshot-on-subscribe. No recovery loop; demonstrates Story 2.3's snapshot semantics.
- [`event-log-viewer/`](../examples/event-log-viewer/) — pure REST cursor-pagination with gap-detection. No WebSocket; demonstrates Story 1.7's history surface plus the truncation-tolerance pattern.
- [`reconnect-recovery/`](../examples/reconnect-recovery/) — long-running WebSocket with `close` / `dropped` → REST catch-up → re-subscribe. Demonstrates Stories 2.4 + 2.5 end-to-end.

Pick the one closest to your use case and adapt — that's exactly what the cookbook entries are for.

## Further reading

- [`docs/protocol.md`](protocol.md) — wire-surface reference (REST + WebSocket + ingest socket).
- [`docs/cookbook/`](cookbook/) — canonical recipes, each pinned to a reference example.
- [`docs/no-list.md`](no-list.md) — explicit V1 scope cuts; check before proposing a new daemon responsibility.
- [`docs/protocol-changelog.md`](protocol-changelog.md) — the immutable change history; what shipped when and why.
