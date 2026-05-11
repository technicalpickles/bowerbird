# v2 design sketch: a minimal substrate for agent state and events

This is a rewrite of `03-design-sketch.md` that incorporates what we learned from the inventory walkthrough, the session-vs-process analysis, the worktree analysis, the terminal-attribution analysis, the novelty-tool survey, and the foundational-vs-buildable critique. The v1 design tried to be a state engine *and* an event bus *and* a normalizer *and* a presenter-helper. v2 narrows the scope.

## The principle

**The daemon's job is to preserve and expose underlying data, not to define application-level concepts on top of it.**

The test for "does this belong in the daemon" has three rungs:

1. Is this *native data* (from hooks, JSONL, statusline) that presenters need but can't access without reimplementing ingestion? → Daemon preserves it.
2. Is this a *derived aggregation* that many presenters compute redundantly? → Optional convenience query.
3. Is this an *application-level concept* layered on top of agent state? → Presenter's responsibility.

Most of the v1 design serves (1). A few places quietly dropped to (3) in disguise. v2 tightens that.

## What v2 keeps from v1

These pieces hold up against the critique:

- **Three data sources fanning into one daemon:** hooks (push), JSONL transcript tail (pull), statusline tap (pull). All optional; the daemon is useful with any subset.
- **A single hook router** so presenters don't fight over `~/.claude/settings.json`. One shim per hook event type. Subscribers register with the daemon, never touch the settings file.
- **A single statusline composer** so presenters don't fight over the statusline slot. One shim, segment providers register, daemon composes per tick.
- **Event log as canonical, state view as derived projection.** Same as v1.
- **Cursor-based queries that double as live tail.** `GET /events?since=<cursor>` and `WS /events` use the same cursor shape; reconnects resume.
- **Session-vs-process separation** via the `attachments` table with heartbeat-driven liveness. The lifecycle states (`live | paused | abandoned | ended`) remain.
- **Periodic sweep that emits synthetic reconciliation events into the same log** rather than mutating state directly. Replayability stays intact.
- **Worktree / repo / branch derivation at first event** via a single `git rev-parse` call, cached by cwd. `worktreeCreate`/`worktreeRemove` events from orchestrators.
- **Terminal/multiplexer/IDE attribution** captured once per attachment as a JSON fingerprint. Not stable handles — snapshots.

The architectural shape is the same. What changes is what fields ride on the events and what columns ride on the state view.

## What v2 changes

### Change 1 — events are passthrough by default

In v1, the event shape was a normalized union (`{ kind, ...kind-specific fields }`) where each event kind enumerated its own typed fields. That was an attempt to be tidy. In practice, every time a presenter wanted a field the union didn't include (`tool_use_id`, `tool_input`, `permission options`, `subagent_type`, `model_name`), it had to re-tail the JSONL to recover it.

v2 reverses the default:

```ts
type Event = {
  event_id: number,           // monotonic, also the cursor
  session_id: string,
  attachment_id: string | null,
  agent_id: string | null,    // populated when a subagent fired this event
  agent_type: string | null,  // populated when a subagent fired this event
  timestamp: number,
  source: "hook" | "jsonl" | "mcp" | "statusline" | "sweep",
  kind: string,               // normalized event kind, see below
  payload: object             // full native payload, preserved verbatim
}
```

The `kind` field stays normalized — that's the thing presenters key off when they don't care about source. The `payload` field is the **full original** hook/JSONL/statusline body. Nothing is stripped. Nothing is reshaped to fit a tidy schema. If Claude Code's hook gave us `tool_input.file_path`, the event carries `tool_input.file_path`. If a future Claude Code version adds a new field, presenters see it the day it ships.

The cost is that two consumers may have to handle the same logical event two slightly different ways (a hook-sourced `toolStart` vs. a JSONL-sourced one may have different fields). The benefit is that no presenter has to re-tail JSONL to recover a field the daemon "didn't think was important."

### Change 2 — `agent_type` and `agent_id` are first-class event fields

Pulled out of `payload` and promoted to top-level columns on the event. This is the single most important change and the reason every voice/sprite/teammate-routing tool can stop installing its own hooks.

Claude Code's hook payload already provides these fields when the hook fires inside a subagent. The Pixel Agents inventory and Benny Cheung's PAI voice system both demonstrate that `agent_type` is the natural key for per-role mapping (sprite, voice, color, room).

Indexed in storage, queryable directly:

```
GET /events?agent_type=researcher&since=<cursor>
GET /sessions/:id/agents?agent_type=engineer
```

### Change 3 — the canonical event kinds shrink

v1 had 18 event kinds. Many were duplicated across hook events and would-be-JSONL events. v2 cuts to a smaller set that maps cleanly to what hooks emit, and lets `payload` carry the rest:

```
sessionStart          // SessionStart
sessionEnd            // SessionEnd (clean) or sweep-emitted (timeout/crash/replaced)
attachmentOpen        // shim sees first event for a new process
attachmentClose       // shim's clean exit, or sweep timeout
heartbeat             // any liveness signal, source identifies origin
userTurn              // UserPromptSubmit
toolStart             // PreToolUse
toolEnd               // PostToolUse (ok=true) or PostToolUseFailure (ok=false)
turnEnd               // Stop
subagentStart         // SubagentStart
subagentEnd           // SubagentStop
permissionRequest     // PermissionRequest or matching Notification
notification          // non-permission Notification (catch-all)
preCompact            // PreCompact
cwdChanged            // derived: cwd field changed across events
worktreeCreate        // orchestrator-emitted
worktreeRemove        // orchestrator-emitted
reconcile             // sweep-emitted; carries what_changed
```

Tokens, cost, and context %  **don't get their own event kinds.** They ride inside `payload` on the events that already carry them — `heartbeat` (from statusline) and `toolEnd` (from JSONL). Presenters that want them read them from `payload`. The previous `tokens`/`cost` events in v1 were the daemon trying to invent a tidy abstraction; the underlying data isn't shaped that way.

Same logic for `assistantMessage` — if a presenter wants assistant text, they read `payload` on JSONL-sourced events. The daemon doesn't need its own event kind for it.

### Change 4 — `permissionRequest` carries the full native payload, no stripping

v1 made `permissionRequest` a near-empty event. v2 makes it a normal passthrough: question text, option list, tool name, tool input, harm-potential signals — all in `payload`, exactly as the hook delivered them.

Hardware-approval surfaces (m5-paper-buddy, AgentDeck D200H, claude-watch, Redlight Greenlight) can render meaningful prompts. Whether and how they answer back is a separate question (out of scope for v2's daemon; see "What v2 leaves out" below).

### Change 5 — drop the `tokens`, `cost`, `agentMessage` event kinds

These were v1 inventions. The data lives in payloads of other events.

If a presenter wants per-model rollups, they read the events and aggregate. The daemon ships an optional convenience query (`GET /sessions/:id/usage`) computed by folding events on the daemon side; presenters who don't need it ignore it. The query is a tier-2 feature, not a tier-1 event vocabulary item.

### Change 6 — explicit out-of-scope statements

The daemon is **read-only observability**. It doesn't:

- Block tool calls (no synchronous PreToolUse interception)
- Answer permission requests on the agent's behalf (no HITL backflow)
- Spawn agents (no control plane)
- Define personas, voice maps, sprite maps, or any presenter UI concept
- Reach across machines (single-machine; multi-host is future work)
- Hold presenter-side persistent state (pet names, leaderboard tokens, etc.)

These boundaries are explicit because tools like claude-code-tamagotchi (blocking), disler observability (HITL), Outworked (control plane), and Marc Nuri's dashboard (multi-host) all exceed them. Saying so up front avoids confusion and keeps the daemon's surface stable.

If evidence accumulates for HITL specifically — the novelty survey turned up seven independent tools wanting approval-from-elsewhere — it gets revisited as an *extension surface*, not as built-in functionality.

## The two surfaces, v2

### State view — current truth

```
GET /sessions
  -> [
       {
         session_id: "abc-123",
         project_dir: "/Users/josh/code/foo",
         repo_root: "/Users/josh/code/foo",
         worktree: "/Users/josh/code/foo/.worktrees/feature-auth",
         branch: "feature/auth",
         model: "claude-sonnet-4-7",
         lifecycle: "live" | "paused" | "abandoned" | "ended",
         current_state: "idle" | "thinking" | "working" | "editing"
                      | "running" | "testing" | "waiting" | "success" | "error",
         current_tool: "Edit",
         started_at, last_event_at,
         attachments: [
           {
             attachment_id, started_at, last_heartbeat_at, alive,
             location: {
               host, terminal, multiplexer, ide, ssh   // see terminal attribution
             }
           }
         ],
         agents: [
           {
             agent_id, agent_type: "main" | "<subagent-name>" | null,
             parent_agent_id,
             status: "idle" | "working" | "waiting" | "completed" | "error",
             current_tool
           }
         ]
       }
     ]

GET /sessions/:id                   -> one session
GET /sessions/:id/agents            -> agent list
GET /sessions/:id/agents?agent_type=researcher
GET /sessions?lifecycle=live
GET /sessions?repo_root=...
GET /sessions?branch=...
```

What v2 explicitly does *not* include in the state view: any persona, display name, role description, voice ID, sprite key, color theme, or other presenter-defined metadata. Those live in presenter config files keyed on `agent_type`.

### Event stream — what happened

```
GET /events?since=<cursor>&limit=N
GET /events?since=<cursor>&agent_type=researcher
GET /events?since=<cursor>&session_id=...
GET /events?since=<cursor>&kind=toolStart
WS  /events
WS  /sessions/:id/events
GET /sessions/:id/events?since=<cursor>
```

Filter on the top-level event fields (`session_id`, `agent_id`, `agent_type`, `kind`, `source`). Filter on payload fields by reading and filtering client-side — those are presenter-specific.

### Optional convenience queries (tier 2)

```
GET /sessions/:id/stats     -> { total_tokens, total_tool_calls, total_user_turns,
                                 last_user_turn_at, models_used: [...] }
GET /sessions/:id/usage     -> [{ model, input, output, cache, cost_usd }, ...]
GET /stats/today
GET /stats/lifetime
```

These are folded from events on the daemon side. Presenters that want them save themselves a fold. Presenters that don't need them ignore them.

## Hook installation

Same as v1, single shim per event type:

```jsonc
{
  "hooks": {
    "PreToolUse":       [{ "command": "bowerbird emit PreToolUse" }],
    "PostToolUse":      [{ "command": "bowerbird emit PostToolUse" }],
    "SessionStart":     [{ "command": "bowerbird emit SessionStart" }],
    "SessionEnd":       [{ "command": "bowerbird emit SessionEnd" }],
    "Stop":             [{ "command": "bowerbird emit Stop" }],
    "SubagentStart":    [{ "command": "bowerbird emit SubagentStart" }],
    "SubagentStop":     [{ "command": "bowerbird emit SubagentStop" }],
    "Notification":     [{ "command": "bowerbird emit Notification" }],
    "UserPromptSubmit": [{ "command": "bowerbird emit UserPromptSubmit" }],
    "PermissionRequest":[{ "command": "bowerbird emit PermissionRequest" }],
    "PreCompact":       [{ "command": "bowerbird emit PreCompact" }]
  },
  "statusLine": {
    "type": "command",
    "command": "bowerbird statusline"
  }
}
```

The shim:

1. Reads JSON from stdin (the full hook payload)
2. Reads environment for the terminal/multiplexer attribution fingerprint (on first event for a new session+pid pair)
3. POSTs to the daemon at `127.0.0.1:<port>` with `{ hook_event, payload, env_capture? }`
4. Exits 0 in <5ms regardless of daemon state (failsafe — never blocks Claude)

Subscribers register over WS/socket. They never touch `~/.claude/settings.json`.

## State machine

Same fold logic as v1; the projections derive from the event log. Key rules:

- `agent_id` and `agent_type` in event → agent row upserted with that type
- `payload.tool_name` on `toolStart` → agent's `current_tool` is set
- `payload.notification_type == "permission_prompt"` (or `permissionRequest` event) → agent status is `waiting`
- statusline `payload.contextPercentage` updates session row
- statusline `payload.input_tokens`/`output_tokens` updates session row + per-model usage table
- sweep emits `reconcile` or `attachmentClose { reason: "timeout" }` for stale attachments

The state machine is one place; presenters never reimplement it.

### The reaction enum (current_state)

Derived from agent activity. Eleven values, copied directly from OpenPets' canonical vocabulary:

```
idle, thinking, working, editing, running, testing,
waiting, waving, success, error, celebrating
```

Computed roughly as:

- `userTurn` → `thinking`
- `toolStart` with Edit/Write/MultiEdit → `editing`
- `toolStart` with Bash → `running` (or `testing` if command matches test patterns)
- `toolStart` (other) → `working`
- `permissionRequest` → `waiting`
- `turnEnd` ok → `success` briefly → `idle`
- `turnEnd` error → `error`

This is the only normalization layer the daemon performs that isn't pure passthrough. It exists because it's the universal projection every cheap presenter wants — statuslines, pets, lamps, LED matrices. Without it, every one of those tools reimplements the same five rules.

## Storage

```sql
CREATE TABLE sessions (
  session_id    TEXT PRIMARY KEY,
  project_dir   TEXT,
  repo_root     TEXT,
  worktree      TEXT,
  branch        TEXT,
  model         TEXT,
  started_at    INTEGER NOT NULL,
  last_event_at INTEGER NOT NULL,
  ended_at      INTEGER,
  lifecycle     TEXT NOT NULL CHECK(lifecycle IN ('live','paused','abandoned','ended')),
  -- folded counters (cheap to update incrementally; saves presenters from scanning)
  total_tokens         INTEGER NOT NULL DEFAULT 0,
  total_tool_calls     INTEGER NOT NULL DEFAULT 0,
  total_user_turns     INTEGER NOT NULL DEFAULT 0,
  last_user_turn_at    INTEGER,
  context_percentage   REAL,
  -- free-form for things we don't model explicitly
  metadata             TEXT
);
CREATE INDEX idx_sessions_lifecycle ON sessions(lifecycle);
CREATE INDEX idx_sessions_repo_root ON sessions(repo_root);
CREATE INDEX idx_sessions_branch    ON sessions(branch);

CREATE TABLE attachments (
  attachment_id     TEXT PRIMARY KEY,
  session_id        TEXT NOT NULL REFERENCES sessions(session_id),
  process_token     TEXT,
  location          TEXT,         -- JSON: AttachmentLocation
  started_at        INTEGER NOT NULL,
  last_heartbeat_at INTEGER NOT NULL,
  ended_at          INTEGER,
  end_reason        TEXT
);

CREATE TABLE agents (
  agent_id        TEXT PRIMARY KEY,
  session_id      TEXT NOT NULL REFERENCES sessions(session_id),
  parent_agent_id TEXT REFERENCES agents(agent_id),
  agent_type      TEXT,           -- 'main', '<subagent-name>', or null
  type            TEXT NOT NULL CHECK(type IN ('main','subagent','teammate')),
  status          TEXT NOT NULL CHECK(status IN ('idle','working','waiting','completed','error')),
  current_tool    TEXT,
  started_at      INTEGER NOT NULL,
  ended_at        INTEGER
);
CREATE INDEX idx_agents_agent_type ON agents(agent_type);

CREATE TABLE session_usage (
  session_id     TEXT NOT NULL REFERENCES sessions(session_id),
  model          TEXT NOT NULL,
  input_tokens   INTEGER NOT NULL DEFAULT 0,
  output_tokens  INTEGER NOT NULL DEFAULT 0,
  cache_read     INTEGER NOT NULL DEFAULT 0,
  cache_creation INTEGER NOT NULL DEFAULT 0,
  cost_usd       REAL    NOT NULL DEFAULT 0,
  PRIMARY KEY (session_id, model)
);

CREATE TABLE events (
  event_id      INTEGER PRIMARY KEY AUTOINCREMENT,
  session_id    TEXT NOT NULL,
  attachment_id TEXT,
  agent_id      TEXT,
  agent_type    TEXT,
  timestamp     INTEGER NOT NULL,
  source        TEXT NOT NULL CHECK(source IN ('hook','jsonl','mcp','statusline','sweep')),
  kind          TEXT NOT NULL,
  payload       TEXT NOT NULL  -- full native payload, JSON
);
CREATE INDEX idx_events_session    ON events(session_id, event_id);
CREATE INDEX idx_events_agent_type ON events(agent_type);
CREATE INDEX idx_events_kind       ON events(kind);
CREATE INDEX idx_events_timestamp  ON events(timestamp);
```

Sessions/agents/attachments/usage are derived projections. They can be rebuilt from the events table alone — a `rebuild-projections` command exists for debugging and migrations.

## What v2 leaves out (deliberately)

These came up across the inventory and survey work. They're real concerns, but not the daemon's job:

- **Persona, display name, role description, voice map, sprite key.** Presenter config keyed on `agent_type`. PAI's `voices.json`, AgentVibes' voice slots, Outworked's agent definitions all already work this way.
- **Tool-name-to-human-readable-string formatting.** Ships as a separate library (`@bowerbird/format-tool-status`) that presenters can use or override. Not in the daemon.
- **HITL backflow.** Different abstraction (bidirectional, blocking, auth-sensitive). Documented as an *extension surface* in the design — what would have to change to add it — without shipping it. Revisit if evidence grows.
- **LAN reachability and mDNS discovery.** Presenter-side. AgentDeck's bridge is the LAN listener; it can subscribe to a localhost daemon. The daemon stays localhost-bound.
- **Codex / OpenCode adapter.** Validate the abstraction by documenting it. Ship the adapter when a presenter actually needs it. The daemon's provider interface should be designed to make this possible, but v1 doesn't have to include it.
- **Tool blocking (synchronous PreToolUse veto).** The unified hook exits 0 always. Tools that need to block tool calls install their own hook separately (collision is back, but for this single use case only).
- **Agent spawning / control plane.** ccam's "Run Claude" button, orchestrators that spawn agents — all out of scope. Daemon picks up state from spawned processes through hooks once they exist.
- **Multi-host.** Single-machine deployment. SSH-attached terminals report to the local daemon on whichever host they're on; remote-host aggregation is future work.
- **Presenter-side persistent state.** Pet names, leaderboard tokens, theme preferences, layouts. Presenter's own files, presenter's own responsibility.

## Open questions

These are the design decisions where reasonable people would disagree, surfaced explicitly:

1. **Should `current_state` (the reaction enum) be on the session row or computed per-request?** Storing it as a column means cheap reads but adds a write per state transition. Computing it on-demand from the latest events is more pure but slower. v2 stores it.

2. **Auth model for the localhost API.** A per-daemon-run token rotated on restart, written to `~/.bowerbird/server.json` (Pixel Agents' pattern), is probably sufficient. WS subscribers present the token at connect time.

3. **Retention policy on events.** Forever (default), bounded by config, or pruned at first replay? Forever is fine for personal use; a config knob exists.

4. **Statusline composition order.** Multiple statusline-segment subscribers register; the daemon composes per tick. In what order? Probably presenter-declared priority with a stable tie-break.

5. **What language/runtime?** v1 raised this. Rust or Go avoids every presenter shipping its own Node runtime. The shim alone (the part that runs on every hook) benefits most from being a small static binary — sub-5ms exit time matters.

6. **Should the daemon expose `payload` filtering or stay top-level only?** SQLite supports JSON path queries; the daemon could let consumers filter by `payload.tool_name`. For now, top-level only — presenters fetch and filter client-side. Revisit if presenters consistently want it.

## What this enables, concretely

A clean tour of the inventory through v2:

- **OpenPets** subscribes to `current_state` changes, calls `pet.react`. ~50 lines.
- **PAI / AgentVibes** subscribes to `subagentEnd` filtered by `agent_type`, looks up voice in its config file. ~30 lines.
- **claude-lamp** subscribes to `current_state`, sets BLE bulb color. ~20 lines.
- **ccpet, Tamagotchi (pet half)** register as statusline segment, read `total_tokens` and `last_user_turn_at` from `/sessions/:id/stats`. ~30 lines.
- **claude-receipts** subscribes to `sessionEnd`, reads `/sessions/:id/usage` for the per-model breakdown, prints. ~50 lines.
- **Claude HUD, claude-status, ClaudeBar, ccseva** read `/sessions`, render. Standard backend-frontend split.
- **disler observability, agents-observe** read `/events?since=` for backfill, `WS /events` for live. Pure event-log consumers.
- **Pixel Agents** consumes `current_state` per session + `agent_type` for teammate routing; the provider abstraction it built becomes the daemon's adapter interface.
- **ccam** stops being a state engine entirely; consumes `/sessions` + `WS /events`. Spawning ("Run Claude") stays in ccam.
- **claude-team-dashboard** still watches `~/.claude/teams/` (different data source); could also subscribe to `agentMessage`-shaped events if the daemon's Claude adapter were extended to emit them from teams/ inbox file watches.
- **AgentDeck** spans 13 surfaces from one bridge daemon, which subscribes to the unified daemon and re-broadcasts to its devices. Pure presenter-side fanout.
- **m5-paper-buddy, claude-watch** subscribe to `permissionRequest` events to render. Answering back is HITL — out of scope, still requires a separate per-tool hook (revisit if pressure grows).
- **Outworked, claude-office** read `/sessions` and `/sessions/:id/agents`, render sprites. Their persona system is layered on top — they bring their own agent definitions, but they consume the daemon's state.
- **Claude Quest, claude-pixel-quest** subscribe to `toolStart`/`toolEnd` and `context_percentage` from session row. ~100 lines.

The common shape across all of them: register as a subscriber, react to events or query state, never touch `~/.claude/settings.json`, never reimplement hook ingestion, never re-tail JSONL for fields the daemon already has.

## Summary of the v1 → v2 diff

| Aspect | v1 | v2 |
|---|---|---|
| Event payload | Normalized per-kind fields | `payload` field carries full native data |
| `agent_type`, `agent_id` | In some payloads | First-class event columns, indexed |
| `permissionRequest` body | Near-empty | Full native payload (question, options, tool input) |
| `tokens`, `cost`, `agentMessage` events | First-class kinds | Live inside other events' payloads; convenience queries derived |
| `current_state` enum | Implicit in v1 | Explicit projection; canonical 11 values from OpenPets |
| Persona / display name / voice | Considered as columns | Out of scope; presenter config keyed on `agent_type` |
| HITL backflow | "Out of scope" footnote | Documented extension surface, still not shipped |
| LAN reachability | "Open question" | Out of scope; presenter-side concern |
| Multi-runtime adapters | Promised but undefined | Document the interface; ship Claude only in v1 |
| Tool blocking | Implicit in failsafe shim | Explicit non-goal |

The throughline: v2 commits harder to being a substrate. It preserves native data faithfully, exposes a small set of well-chosen projections, and resists the temptation to model application-level concepts that presenters are already handling well in their own configs.