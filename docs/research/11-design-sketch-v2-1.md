# v2.1 design sketch: capabilities, liveness, and pub/sub

This is an incremental revision of `08-design-sketch-v2.md`. The changes come from two places: the multi-agent tool patterns analysis (`10-multi-agent-tool-patterns.md`) and a focused look at how presenters actually want to consume events.

The principle from v2 holds: the daemon preserves and exposes underlying data, and resists modeling application-level concepts on top. v2.1 extends that with three things v2 left underspecified — agent capabilities, liveness vs lifecycle separation, and a pub/sub model that lets presenters declare what they care about instead of polling.

## What v2.1 changes from v2

Three changes, in order of significance:

1. **Pub/sub as the primary live-consumption model.** Two channels (raw events, state changes), hierarchical topics, declarative filters. Polling stays available; subscribers no longer need it.
2. **`AgentCapabilities` per source.** Borrowed from AgentDeck. Lets presenters check what an agent supports before rendering UI for it.
3. **`liveness` separated from `lifecycle`.** Borrowed from opensessions. `liveness` is the attachment's status (alive/exited/unknown); `lifecycle` is the session's status (live/paused/abandoned/ended). They're orthogonal.

Everything else from v2 stays — passthrough events, `agent_type` as first-class, the 11-value reaction enum, worktree derivation, terminal attribution, the out-of-scope boundary.

## 1. Pub/sub

The v2 design had WebSocket subscriptions but treated them as "the event firehose, optionally filtered." That's enough for tools that want raw events (disler-style observability) but it forces every other tool to either poll or do client-side projection of the firehose.

The multi-agent analysis surfaced four distinct consumer patterns:

| Consumer pattern | Examples | Wants |
|---|---|---|
| Firehose | disler, ccam, agents-observe, agent-flow | All events, with filters |
| State-change | OpenPets, claude-lamp, ccpet, AgentDeck sprites | Derived transitions only |
| Single-event-kind | claude-receipts, m5-paper-buddy, claude-watch | One specific event kind |
| Snapshot + delta | claude-status, Outworked, dashboards | One-shot snapshot then live updates |

The v2 design served firehose well; the other three had to roll their own projection. v2.1 makes them first-class.

### Two channels, hierarchical topics

```
EVENTS channel — raw events as they're ingested
STATE   channel — derived state transitions
```

Topics are dot-separated hierarchical paths. Presenters subscribe to patterns.

**EVENTS channel topics:**

```
events.<kind>                                  # e.g. events.toolStart
events.<kind>.<source>                         # e.g. events.toolStart.claude
events.session.<session_id>                    # all events for one session
events.session.<session_id>.<kind>             # one kind, one session
events.agent.<agent_type>                      # all events from a subagent role
events.agent.<agent_type>.<kind>               # one kind from one role
events.*                                       # firehose (rarely needed)
```

**STATE channel topics:**

```
state.session.<session_id>                     # any change to session row
state.session.<session_id>.current_state       # only reaction enum changes
state.session.<session_id>.lifecycle           # only lifecycle changes
state.session.<session_id>.attachment          # attachment open/close/heartbeat-stale
state.session.<session_id>.agent.<agent_type>  # one subagent's state
state.session.<session_id>.tokens              # token-count changes
state.session.<session_id>.context             # context-percentage changes
state.sessions.added                           # new session appeared
state.sessions.removed                         # session ended
state.agents.<agent_type>                      # any agent of this type changed
state.*                                        # all state changes (firehose equivalent)
```

Topics are read on the broker side using simple prefix-and-glob matching. No per-subscriber state machine; just a routing table.

### Filters as topic patterns, not query parameters

The v2 design had `GET /events?kind=toolStart&agent_type=researcher`. That's fine for one-shot reads. For subscriptions, the topic shape *is* the filter:

```
SUBSCRIBE events.toolStart.claude              # all Claude toolStart events
SUBSCRIBE events.toolStart.*                   # toolStart from any source
SUBSCRIBE events.*.codex                       # all events from Codex
SUBSCRIBE state.session.abc-123.current_state  # the one signal a pet needs
SUBSCRIBE state.session.abc-123.*              # everything about this session
```

A subscriber can register multiple topics on one connection. The broker tracks `{subscriber_id → [topics]}` and dispatches each event to matching subscribers.

### Snapshot-on-subscribe

When a subscriber registers a topic that has current state, the broker sends a **snapshot frame** first, then live updates:

```
→ SUBSCRIBE state.session.abc-123
← SNAPSHOT state.session.abc-123 {
    session_id: "abc-123",
    current_state: "working",
    lifecycle: "live",
    ...full session row
  }
← (live) state.session.abc-123.current_state {
    old: "working", new: "waiting", changed_at: 1234567890
  }
```

This solves the snapshot+delta pattern in one round trip. The subscriber doesn't have to do a separate `GET /sessions/abc-123` before subscribing.

For topics that aren't "current state" (the EVENTS channel, mostly), the snapshot frame contains the last N events matching the topic (configurable, default 10) so a subscriber that reconnects can see what it missed. Plus an `event_id` cursor for precise resume.

### State-change events: derived, not raw

The STATE channel doesn't carry raw events. It carries **transition signals** computed from the state projection. Shape:

```ts
type StateChange = {
  topic: string,                  // e.g. "state.session.abc-123.current_state"
  changed_at: number,
  old: any,                       // previous value
  new: any,                       // new value
  caused_by_event_id?: number,    // back-pointer to the event that triggered this
};
```

The daemon already computes the projection. The change emitter is a cheap diff on every projection update: compare old session row to new, emit one `state.session.<id>.<field>` change per modified column. No new computation; just emit what already changed.

This is what gives presenters cheap consumption. A lamp that wants "tell me when the user's main session goes to `waiting`" subscribes to `state.session.<id>.current_state` and acts on `new === "waiting"`. No event-folding. No projection logic. The daemon already did it.

### Wire protocol

```
# Connect
ws://127.0.0.1:9876/subscribe
  Headers: Authorization: Bearer <token>

# Subscribe (can send multiple)
→ { op: "subscribe", topic: "state.session.abc-123.current_state" }
← { op: "subscribed", topic: "state.session.abc-123.current_state",
    snapshot: { topic, value: "working", at: 123456789 } }

# Live update
← { op: "publish", topic: "state.session.abc-123.current_state",
    change: { old: "working", new: "waiting", changed_at: ..., caused_by_event_id: 999 } }

# Raw event (EVENTS channel)
← { op: "publish", topic: "events.toolStart.claude",
    event: { event_id, session_id, agent_type, kind, payload, ... } }

# Unsubscribe
→ { op: "unsubscribe", topic: "state.session.abc-123.current_state" }

# Disconnect: ws.close()
```

Plain JSON over WebSocket. Reconnects resume from `event_id` cursor on the EVENTS channel; STATE channel resends snapshots on reconnect.

### Why two channels and not one

Could collapse to one topic tree (`events.*` and `state.*` under the same broker). The split is conceptual, not architectural:

- EVENTS: append-only, ordered, replayable. The log.
- STATE: derived, last-write-wins, snapshot-able. The projection.

Tools that confuse them write bugs. A lamp that subscribes to `events.toolStart` and tries to map tool names to colors will get every tool call, including ones from subagents the user doesn't care about. A lamp that subscribes to `state.session.<id>.current_state` gets exactly one signal per state transition, debounced and deduped. The split is to keep presenters from reinventing projection logic.

### Polling endpoints stay

```
GET /sessions
GET /sessions/:id
GET /events?since=<cursor>
GET /sessions/:id/stats
GET /sources
```

Subscribers that prefer polling can. The pub/sub model is an addition, not a replacement. Some integrations (Cron-driven scripts, CI pipelines, things that want to fire once per session-end) don't need a live connection.

### Backpressure and slow consumers

A subscriber that can't keep up gets dropped after a bounded queue fills (default: 1,000 messages, configurable per-subscriber). The broker sends a `dropped` frame indicating the count and the event_id range, so the subscriber knows to do a snapshot refetch. This is opensessions' approach; better than blocking the producer.

For tools that explicitly opt into reliability (`durable: true` on subscribe), the broker tracks delivery offsets and resumes from the last acked event_id. Disk-backed queue. More expensive; opt-in only.

## 2. Agent capabilities

The novelty-tool survey and the multi-agent-tool analysis both surfaced this. AgentDeck's `AgentCapabilities` matrix is the right pattern; v2.1 lifts it.

Each source has a capabilities document, exposed via `GET /sources`:

```ts
type SourceCapabilities = {
  source: string,                     // "claude", "codex", "gemini", "cursor", "opencode", "aider"
  display_name: string,
  // What the source provides
  has_hooks: boolean,                 // installable hook system
  has_jsonl_transcripts: boolean,
  has_statusline: boolean,
  has_subagents: boolean,
  has_permission_payload: boolean,    // permission requests carry useful payload
  has_token_telemetry: boolean,
  has_context_telemetry: boolean,     // contextPercentage available
  has_model_metadata: boolean,
  // What the daemon can observe
  reaction_enum_subset: string[],     // which of the 11 values this source can produce
  // How the daemon ingests
  ingest: "hook" | "transcript" | "sqlite" | "cloud" | "terminal-scrape",
};
```

Per-source YAMLs:

```yaml
# adapters/claude/capabilities.yaml
source: claude
display_name: Claude Code
has_hooks: true
has_jsonl_transcripts: true
has_statusline: true
has_subagents: true
has_permission_payload: true
has_token_telemetry: true
has_context_telemetry: true
has_model_metadata: true
reaction_enum_subset:
  [idle, thinking, working, editing, running, testing,
   waiting, waving, success, error, celebrating]
ingest: hook
```

```yaml
# adapters/codex/capabilities.yaml
source: codex
display_name: Codex CLI
has_hooks: true
has_jsonl_transcripts: true
has_statusline: false              # uses `notify` on agent-turn-complete only
has_subagents: partial             # [agents] in config.toml, less mature
has_permission_payload: true
has_token_telemetry: true
has_context_telemetry: false
has_model_metadata: true
reaction_enum_subset:
  [idle, thinking, working, editing, running, waiting, success, error]
ingest: hook
```

```yaml
# adapters/gemini/capabilities.yaml
source: gemini
display_name: Gemini CLI
has_hooks: true
has_jsonl_transcripts: true
has_statusline: false
has_subagents: false               # not first-class
has_permission_payload: true       # BeforeTool with decision: deny
has_token_telemetry: true
has_context_telemetry: false
has_model_metadata: true
reaction_enum_subset:
  [idle, thinking, working, editing, running, waiting, success, error]
ingest: hook
```

```yaml
# adapters/aider/capabilities.yaml
source: aider
display_name: Aider
has_hooks: false
has_jsonl_transcripts: false       # markdown transcripts only
has_statusline: false
has_subagents: false
has_permission_payload: false
has_token_telemetry: false
has_context_telemetry: false
has_model_metadata: false
reaction_enum_subset:
  [idle, working, success, error]
ingest: transcript
```

Presenters check capabilities before rendering. Wearable tools that show permission prompts skip Aider sessions because `has_permission_payload: false`. HP-bar pets fall back to a tokens-relative bar for sources where `has_context_telemetry: false`. The capabilities surface tells presenters what to expect without surprising them mid-render.

## 3. Liveness vs lifecycle

The v2 design folded both concerns into a single `lifecycle` enum: `live | paused | abandoned | ended`. opensessions split them, and the reasoning holds.

**Lifecycle** is about the session as a logical entity:

- `live` — agent is in the middle of work
- `paused` — between turns, waiting for user input
- `abandoned` — no activity for long enough that the user probably moved on
- `ended` — `SessionEnd` received

**Liveness** is about the underlying process:

- `alive` — process token says it's running
- `exited` — process gone
- `unknown` — we never had a pane/pid handle to begin with (file-watch-only sources)

The combinations carry meaning:

| Lifecycle | Liveness | Meaning |
|---|---|---|
| `live` | `alive` | Working normally |
| `live` | `exited` | Process died mid-turn — crash or kill (rare; sweep promotes to `abandoned`) |
| `paused` | `alive` | Between turns; resumable in this terminal |
| `paused` | `exited` | Between turns; would need `--resume <id>` |
| `paused` | `unknown` | Transcript-only source; we can't tell |
| `abandoned` | `alive` | Process still running but no activity for hours — probably forgotten |
| `abandoned` | `exited` | Crashed and abandoned |
| `ended` | `exited` | Clean exit |
| `ended` | `alive` | Should be impossible; flag for diagnostics |

Schema change:

```sql
-- on the attachments table
ALTER TABLE attachments ADD COLUMN liveness TEXT
  NOT NULL DEFAULT 'unknown'
  CHECK(liveness IN ('alive','exited','unknown'));

-- The sessions table's lifecycle column already exists; no change.
```

Liveness is per-attachment. A session can have multiple attachments over its life (a `claude --resume` reattaches with a new process). Each has its own liveness.

Topics for state changes:

```
state.session.<id>.lifecycle    # session moved between live/paused/abandoned/ended
state.session.<id>.attachment   # attachment opened, closed, or liveness changed
```

## 4. Per-adapter refresh rates

Push-based sources (Claude, Codex, Gemini, Cursor hooks) don't need polling. Pull-based sources (OpenCode SQLite, Amp REST, transcript-only Aider) do. v2.1 makes this per-adapter config:

```yaml
# adapters/opencode/runtime.yaml
ingest: sqlite
db_path: ~/.local/share/opencode/opencode.db
poll_interval_active_ms: 2000
poll_interval_idle_ms: 30000
poll_interval_when_focused_ms: 500     # if a subscriber wants live updates

# adapters/amp/runtime.yaml
ingest: cloud
discovery_poll_interval_ms: 10000
# WebSocket per thread = push, no polling beyond discovery

# adapters/claude/runtime.yaml
ingest: hook
# push-only; no polling
```

The daemon's adapter loader reads runtime.yaml per source, knows whether to spin up a poll loop, and sizes the poll interval appropriately. Subscribers can hint "I'm actively rendering this session" via a heartbeat ping; the daemon shortens the poll interval for that source while at least one focused subscriber exists.

## 5. Schema updates

Net schema changes from v2:

```sql
-- attachments gets explicit liveness
ALTER TABLE attachments ADD COLUMN liveness TEXT
  NOT NULL DEFAULT 'unknown'
  CHECK(liveness IN ('alive','exited','unknown'));

-- events gets the source column lifted to first-class
-- (was already there in v2; v2.1 makes (source, session_id) the documented natural key)

-- sources is a new table for the capabilities registry (could also be config-file only)
CREATE TABLE sources (
  source       TEXT PRIMARY KEY,
  display_name TEXT NOT NULL,
  capabilities TEXT NOT NULL,           -- JSON
  ingest       TEXT NOT NULL,
  enabled      INTEGER NOT NULL DEFAULT 1
);
```

Sessions, agents, session_usage stay as in v2.

### `last_event_at` is the universal "is this active" signal

Implicit in v2 (the session row updates on every event); v2.1 makes it explicit. The activity survey (`14-activity-survey.md`) showed that 5 of 8 surveyed pet/sprite/dashboard tools key off a single "time since last activity" signal — claude-status uses it for sort and labels, Outworked thresholds against it for slow/stuck detection, Pixel Agents uses it across three timescales (5s / 2min / 10min), and others compute "Xm ago" labels from it.

The substrate maintains `last_event_at` on the session row as a UNIX millisecond timestamp. Presenters that want rate computation (sliding windows, leaky buckets) subscribe to the EVENTS channel and compute client-side — see `14-activity-survey.md` for the four common patterns and their ~5-15-line implementations.

No `state.session.<id>.last_event_at` topic exists — by definition it changes on every event, which would be noisy. Presenters wanting fine-grained activity use `events.session.<id>.*` directly; presenters wanting coarse "is this active recently" poll `/sessions/:id` periodically.

## 6. API surface, complete

```
# Polling — unchanged from v2
GET  /sessions
GET  /sessions?lifecycle=live
GET  /sessions?repo_root=...
GET  /sessions?branch=...
GET  /sessions/:id
GET  /sessions/:id/agents
GET  /sessions/:id/stats
GET  /sessions/:id/usage
GET  /sessions/:id/events?since=<cursor>
GET  /events?since=<cursor>&kind=<...>&source=<...>&agent_type=<...>
GET  /stats/today
GET  /stats/lifetime

# Capabilities — new in v2.1
GET  /sources
GET  /sources/:source

# Pub/sub — new in v2.1
WS   /subscribe
       → subscribe / unsubscribe / publish frames over JSON
```

## 7. Open questions specific to v2.1

These are the design decisions where reasonable people would disagree:

1. **Should state-change events include a `caused_by_event_id` back-pointer to the raw event?** Probably yes — it lets debuggers correlate state transitions to the events that caused them. The cost is one extra column on the change emission.

2. **Are state-change emissions guaranteed to follow the event that caused them on the EVENTS channel?** Two subscribers, one on each channel, might want consistent ordering: see the raw event, then the state change. The daemon needs to emit them in that order within a single transaction. Worth specifying.

3. **Topic wildcards: `*` only, or glob (`?`, `[abc]`)?** Probably `*` only — simpler, predictable. NATS-style hierarchical wildcards (`state.session.*.current_state`) are sufficient for every pattern surveyed.

4. **Durable subscriptions for "I want every event since I last connected":** opt-in only, disk-backed. Default ephemeral. Probably ship ephemeral first; add durable when there's evidence a presenter wants it.

5. **Per-source pub/sub topics on STATE:** should `state.session.abc.attachment` carry the per-attachment liveness, or fan out to `state.session.abc.attachment.<id>.liveness`? Probably one level shallower — the per-attachment ID isn't a useful topic key for most subscribers; presenters that need it filter client-side.

## 8. What this enables, by consumer

Walking the inventory through the v2.1 pub/sub model:

- **OpenPets**: `SUBSCRIBE state.session.<id>.current_state`. Snapshot tells it the current state on connect; live updates fire on every transition. ~20 lines.
- **claude-lamp**: same as OpenPets; switches BLE bulb color on state change. ~25 lines.
- **ccpet, Tamagotchi (statusline)**: doesn't subscribe — it's a statusline tool, polled by Claude each turn. Reads `GET /sessions/<id>/stats`. ~30 lines.
- **claude-receipts**: `SUBSCRIBE events.sessionEnd.*`. Fires exactly once per session end. Reads `/sessions/<id>/usage` for the per-model breakdown. ~50 lines.
- **PAI / AgentVibes voice**: `SUBSCRIBE events.subagentEnd.*`. Reads `agent_type` from the event, looks up voice in its config. ~30 lines.
- **m5-paper-buddy, claude-watch**: `SUBSCRIBE events.permissionRequest.*`. Renders the option list from `payload`. (Approving back is HITL, still out of scope.) ~80 lines for the render side.
- **agent-flow, disler**: `SUBSCRIBE events.*` for the firehose. ~50 lines.
- **claude-status menu bar**: `SUBSCRIBE state.sessions.added`, `state.sessions.removed`, `state.session.*.current_state`. Snapshot on connect populates the menu; live updates keep it fresh. No polling. ~200 lines.
- **Outworked, claude-office**: `SUBSCRIBE state.sessions.added/removed` for hiring/firing employees; `SUBSCRIBE state.session.*.current_state` for sprite animations; `SUBSCRIBE state.session.*.agent.*` for per-employee work. ~300 lines.
- **AgentDeck bridge**: `SUBSCRIBE state.*` for the full state firehose; routes to 13 display surfaces. ~500 lines of bridge logic, but most of it isn't substrate-related.

Common shape: every presenter subscribes to one or two topics, registers a callback per topic, and never polls. The substrate gives them the *changes they care about* — not the events that caused them, not the full projection, not snapshots-on-request. Just the transition signals.

## 9. The v2 → v2.1 diff

| Aspect | v2 | v2.1 |
|---|---|---|
| Live consumption | WS firehose with query filters | Two channels (EVENTS, STATE), hierarchical topics |
| State changes | Implicit (poll `/sessions/:id`) | First-class `state.*` topics with old/new |
| Snapshot+delta | Two separate calls | Single subscribe with snapshot frame |
| Agent capabilities | Implicit / undocumented | `AgentCapabilities` per source via `/sources` |
| Liveness | Folded into `lifecycle` | Separate column on `attachments` |
| `last_event_at` | Implicit | Documented as the universal activity signal; presenters compute rate client-side |
| Polling rates | Not modeled | Per-adapter `runtime.yaml` |
| Backpressure | Unspecified | Bounded queue with `dropped` frames |
| Durable subscriptions | Not mentioned | Opt-in, disk-backed |

The corrections are small in code but substantial in what they enable. The pub/sub model alone collapses most presenter code by an order of magnitude — what used to be "poll, fold, diff, dispatch" becomes "subscribe, dispatch."

## 10. What's explicitly still out of scope

Reiterating from v2, sharpened by what the pub/sub surface tempts:

- **HITL backflow.** Subscribing to `events.permissionRequest.*` to render the prompt is fine. The substrate does not accept "I am the authoritative answerer" registrations. If many presenters want this, it becomes a sibling service (a "permission broker") that consumes from the substrate, not part of the substrate itself.
- **Cross-machine pub/sub.** The broker is localhost-bound. AgentDeck-class bridges that need LAN distribution build their own; the substrate is one of their data sources.
- **Authoritative tool blocking.** The substrate doesn't intercept tool calls. Subscribers can be informed of `events.toolStart` but cannot veto. Policy hooks remain a separate per-agent install.
- **Application-level concepts.** Personas, voice maps, sprite maps, pet stats, mood, color themes — all presenter side. The substrate gives them `agent_type` and `current_state`; what they do with it is their job.
- **Cross-agent normalization beyond the reaction enum.** Tool names stay native in `payload`. Permission payload shapes stay agent-specific. Subagent semantics stay agent-specific. Presenters that want unified rendering across agents do the per-agent translation themselves.

The substrate's value proposition holds: be the data layer the existing tools could adopt, freeing them from per-agent watcher code. Pub/sub is what makes that adoption easy — subscribe to the changes you care about, get on with rendering, never poll.