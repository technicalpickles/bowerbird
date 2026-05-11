# Detailed Inventory: Per-Tool Architecture

This document inventories the priority tools by examining their source. For each tool, we capture:

- **Data source(s)**: where state comes from
- **State model**: what's tracked
- **Event vocabulary**: what events are emitted/stored
- **Storage**: where state lives
- **Install model**: how it gets wired into Claude Code
- **Notable design choices**

The goal is to identify what a generalized state layer would need to subsume each.

---

## 1. OpenPets (alvinunreal/openpets)

**Repo:** `packages/agent-events`, `packages/claude`, `packages/mcp`, `packages/client`

### Data sources
- **MCP** (primary): `openpets_status`, `openpets_react`, `openpets_say` — agent self-reports
- **Claude hooks** (secondary): `UserPromptSubmit`, `PreToolUse`, `PermissionRequest`, `Notification`, `Stop`, `StopFailure`

### State model — the canonical reaction vocabulary

In `packages/client/src/protocol.ts`:

```ts
export const allowedReactions = [
  "idle",
  "thinking",
  "working",
  "editing",
  "running",
  "testing",
  "waiting",
  "waving",
  "success",
  "error",
  "celebrating",
] as const;
```

This is the only normalized state vocabulary I found in any of the tools. It's small, agent-agnostic, and explicitly the contract between data sources and presenters.

### Event/method vocabulary

```ts
export type OpenPetsIpcMethod =
  | "hello"
  | "status"
  | "pets.list"
  | "pets.install"
  | "lease.acquire"
  | "lease.heartbeat"
  | "lease.release"
  | "pet.react"
  | "pet.say";
```

This is RPC-shaped, not event-shaped: clients call `pet.react` with a reaction name. There is no "history of events" — only "what is the current reaction."

### Hook → reaction mapping (Claude-specific)

In `packages/claude/src/hooks.ts`:

| Hook event             | Reaction              |
|------------------------|-----------------------|
| `UserPromptSubmit`     | `thinking`            |
| `PreToolUse` (Edit/Write/MultiEdit) | `editing` |
| `PreToolUse` (Bash, test commands)  | `testing` |
| `PreToolUse` (other)   | (no reaction)         |
| `PermissionRequest`    | `waiting` + `permission` speech |
| `Stop`                 | `success`             |
| `StopFailure`          | `error` + `error` speech |

### Storage
- No persistent state — desktop app holds runtime reaction state in memory
- IPC discovery file with port + auth token (token rotates per app run)
- Throttle state at `$XDG_STATE_HOME/openpets/claude-hook-throttle.json`

### Install model
- `claude mcp add --scope user openpets -- npx -y @open-pets/mcp` for global MCP
- Optional Claude hook installer that adds entries to `~/.claude/settings.json`
- The MCP layer is independent of hooks — agent can self-report even without hooks

### Notable design choices
- **Lease + token model**: each MCP/CLI client acquires a short-lived lease, sends a per-run token with every request. Prevents one project's pet from being hijacked by another.
- **Throttling and cooldown**: speech (20s), permission (3s), reactions (10s) are throttled to prevent spam.
- **Speech validation**: `validateHookSpeech` rejects messages containing URLs, paths, secrets, code-like syntax. Prevents accidental leakage to a desktop bubble.
- **Decorative-only**: hooks explicitly do not approve/deny/block — they just react.
- Hook script lives next to OpenPets in TypeScript and reads `process.env.CLAUDE_PROJECT_DIR` to detect project-local override.

### What it's missing (relative to a "generalized" state layer)
- No event history / time-series queryable state
- No multi-session awareness — one pet, one current reaction
- No way for other consumers (a HUD, a dashboard) to subscribe to OpenPets state
- The reaction vocabulary is excellent but locked inside OpenPets — there's no separate package exposing just the protocol

---

## 2. Pixel Agents (pablodelucca/pixel-agents)

**Repo:** `server/src/provider.ts`, `server/src/hookEventHandler.ts`, `server/src/providers/hook/claude/claude.ts`, `src/transcriptParser.ts`

### Data sources

Two, in parallel:

- **Hooks** via the `HookProvider.normalizeHookEvent` interface (POST to local HTTP server)
- **JSONL transcripts** via `parseTranscriptLine` and file-watching `~/.claude/projects/<encoded-cwd>/*.jsonl`

The JSONL path is the fallback / heuristic mode; hooks are authoritative when installed (`agent.hookDelivered = true` suppresses heuristic timers).

### State model

In `src/types.ts`, the `AgentState` carries:

```
sessionId, projectDir, jsonlFile, fileOffset, lineBuffer
activeToolIds: Set<string>
activeToolStatuses: Map<string, string>   // toolId -> human-readable status
activeToolNames: Map<string, string>
activeSubagentToolIds: Map<string, Set<string>>  // parentToolId -> sub-tool IDs
isWaiting, permissionSent, hadToolsInTurn
inputTokens, outputTokens
teamName, agentName, isTeamLead, leadAgentId   // Agent Teams support
hookDelivered  // suppresses heuristic timers when hooks active
```

Plus normalized presenter messages with these `type` values:
`agentCreated`, `agentClosed`, `agentStatus`, `agentToolStart`, `agentToolDone`, `agentToolsClear`, `subagentToolStart`, `subagentToolDone`, `subagentClear`, `agentTokenUsage`, `agentTeamInfo`, `agentDiagnostics`, `existingAgents`, `layoutLoaded`, etc.

### Event vocabulary — the normalized AgentEvent

In `server/src/provider.ts` — this is the most important code in the entire inventory:

```ts
export type AgentEvent =
  | { kind: 'toolStart'; toolId: string; toolName: string; input?: unknown }
  | { kind: 'toolEnd'; toolId: string }
  | { kind: 'turnEnd' }
  | { kind: 'userTurn' }
  | { kind: 'subagentStart'; parentToolId: string; toolId: string; toolName: string; input?: unknown }
  | { kind: 'subagentEnd'; parentToolId: string; toolId: string }
  | { kind: 'subagentTurnEnd'; parentToolId: string }
  | { kind: 'progress'; toolId: string; data: unknown }
  | { kind: 'permissionRequest' }
  | { kind: 'sessionStart'; source?: string }
  | { kind: 'sessionEnd'; reason?: string };
```

And the provider abstraction:

```ts
export interface HookProvider {
  readonly kind: 'hook';
  readonly id: string;
  readonly displayName: string;
  normalizeHookEvent(raw: Record<string, unknown>): { sessionId: string; event: AgentEvent } | null;
  installHooks(serverUrl: string, authToken: string): Promise<void>;
  uninstallHooks(): Promise<void>;
  areHooksInstalled(): Promise<boolean>;
  formatToolStatus(toolName: string, input?: unknown): string;
  readonly permissionExemptTools: ReadonlySet<string>;
  readonly subagentToolNames: ReadonlySet<string>;
  // optional file-fallback fields
  getSessionDirs?(workspacePath: string): string[];
  readonly sessionFilePattern?: string;
  parseTranscriptLine?(line: string): AgentEvent | null;
  buildLaunchCommand?(...): { command, args, env };
  readonly team?: TeamProvider;
}
```

Comments on file `provider.ts` explicitly note: *"FileProvider (polling-only CLIs) and StreamProvider (push-based external services) will be added alongside the first real second provider."*

### Storage
- VS Code workspace state for agent registry (`pixel-agents.agents`, `pixel-agents.agentSeats`, `pixel-agents.layout`)
- User-level layout file at `~/.pixel-agents/layout.json`
- Server discovery file at `~/.pixel-agents/server.json` with `{port, pid, token, startedAt}`

### Install model
- VS Code extension installs the local HTTP server (single instance, multi-window aware: second window reuses via `server.json` PID check)
- `claudeHookInstaller.ts` writes hook entries to `~/.claude/settings.json` that POST to `http://127.0.0.1:<port>/api/hooks/claude` with auth header

### Notable design choices
- **The `HookProvider` interface is exactly the abstraction Josh is describing.** A new agent (Codex, OpenCode, Cursor) becomes a new file under `server/src/providers/hook/<id>/`. Adding a provider is a registry edit and an `installHooks` impl.
- **Single normalization boundary**: the comment in `claude.ts` says "All raw Claude hook payload fields are read HERE and HERE ONLY. Downstream sees only the normalized AgentEvent union."
- **Multi-window cooperation**: discovery file + PID check — only one VS Code window owns the server, the others reuse.
- **Token correlation**: `currentHookToolId` carries forward across PreToolUse / PostToolUse because the raw hook payload doesn't carry the tool id, but JSONL polling does.
- **Team awareness as a separate optional interface (`TeamProvider`)**, attached to a `HookProvider` — clean separation between basic single-agent and Agent Teams behavior.
- **HITL modeling**: Claude's hook payloads use both `PermissionRequest` and `Notification` with `notification_type=permission_prompt` for the same logical event; the normalizer flattens both into `kind: 'permissionRequest'`.

### What it's missing
- The provider interface and `AgentEvent` union are not packaged separately — they live inside the VS Code extension. To use them outside Pixel Agents you'd extract `server/`.
- The state store is not queryable by external consumers — it's a webview message stream tied to a UI.
- No public read-side API (`GET /sessions`, `GET /agents/:id`).

---

## 3. disler/claude-code-hooks-multi-agent-observability

**Repo:** `apps/server/src/types.ts`, `.claude/hooks/send_event.py`, `.claude/settings.json`

### Data source
- **Hooks only**, with one Python forwarder per hook event type (`pre_tool_use.py`, `post_tool_use.py`, `stop.py`, etc.) plus a single `send_event.py` that wraps them and POSTs to the server.

### State model — none

The server-side model is intentionally a passthrough:

```ts
export interface HookEvent {
  id?: number;
  source_app: string;       // e.g. "cc-hook-multi-agent-obvs"
  session_id: string;
  hook_event_type: string;  // e.g. "PreToolUse", stringly typed
  payload: Record<string, any>;
  chat?: any[];             // optional: full transcript snapshot
  summary?: string;         // optional: AI-generated event summary
  timestamp?: number;
  model_name?: string;
  humanInTheLoop?: HumanInTheLoop;
  humanInTheLoopStatus?: HumanInTheLoopStatus;
}
```

**There is no `AgentState`** in the server. Everything is event-stream + UI rendering.

### Event vocabulary

The 12 hook event types are stringly named, exactly matching Claude Code's:

```
SessionStart, SessionEnd, UserPromptSubmit, PreToolUse, PostToolUse,
PostToolUseFailure, PermissionRequest, Notification, SubagentStart,
SubagentStop, Stop, PreCompact
```

### Storage
- SQLite (Bun sqlite, WAL mode)
- Two tables: `events` (HookEvent rows) and `themes` (UI theming, unrelated)

### Install model
- Copy the entire `.claude/` directory to your project root
- Edit `.claude/settings.json` to set `--source-app YOUR_PROJECT_NAME`
- Hooks all run via `uv run` for Python deps

### Notable design choices
- **Stringly-typed events**: no parsing/normalization. Server stores what it receives.
- **`source_app` field for multi-project filtering** — a swim lane key.
- **Hook scripts in Python**, not TS/JS: each hook is a standalone `uv` script with inline deps. Heavier than ccam's pure-Node forwarder, but trivially editable per-hook.
- **AI summarization as a hook flag**: `--summarize` triggers an Anthropic call inside the hook to generate `summary` text for UI display. Adds latency but improves event readability.
- **HITL extension**: the `humanInTheLoop` field on `HookEvent` is server-vended state, not from Claude. The server can emit a "question" payload that the UI surfaces and posts back via WebSocket. Effectively the dashboard becomes an out-of-band approval channel.

### What it's missing
- No state model — only an event log. Computing "is this session waiting" requires reading the latest event. UI does this; no canonical answer in the data.
- No agent-graph: parent/child relationships are inferred from the raw payload. No first-class subagent table.
- Single global namespace per `source_app` — multi-team or nested-team scoping isn't modeled.

---

## 4. claude-code-tamagotchi (Ido-Levi)

**Repo:** `src/index.ts`, `src/engine/StateManager.ts`, `src/commands/violation-check.ts`

### Data sources
- **Statusline JSON on stdin** (primary): the pet renders on every Claude Code statusline tick
- **Transcript JSONL polling** (for the violation system): reads recent assistant messages and analyzes them with Groq
- **PreToolUse hook** (optional, for behavioral enforcement): runs `violation-check`

### State model — pet-specific

In `src/engine/StateManager.ts`, `PetState` has a large stat block:

```
identity:        name, type ('dog'|'cat'|'dragon'|'robot'), birthTime, age
vital stats:     happiness, hunger, energy, health, cleanliness  (all 0-100)
timestamps:      lastUpdate, lastFed, lastPlayed, lastPetted, lastCleaned, lastSlept
animation:       currentAnimation, animationFrame, animationStartTime
behavioral:      isAsleep, isSick
activity:        sessionUpdateCount, totalUpdateCount, recentUpdateTimestamps[],
                 sessionStartTime, sessionsToday
mood:            currentMood (15 named moods)
feedback:        claudeBehaviorScore (0-100), recentViolations,
                 feedbackHistory[], lastTranscriptCheck, currentFeedback
```

The "Claude is misbehaving" signal is a single 0-100 score, derived from violation history.

### Event vocabulary

There is no exposed event API. Violations are written to a SQLite DB (`feedback.db`) by an LLM-analysis worker. The pet polls and reads.

### Storage
- Pet state JSON: `~/.claude-pet/pet-state.json` (or `$PET_STATE_FILE`)
- Feedback DB: `~/.claude/pets/feedback.db` (SQLite)
- Animation counter: `~/.claude-pet/animation-counter.json`

### Install model
- npm install + add a statusline command to `~/.claude/settings.json`
- Optional: add a `PreToolUse` hook for `violation-check`

### Notable design choices
- **Activity-driven, not real-time**: pet only ticks on statusline calls. Nice for low overhead.
- **Mood derivation**: complex set of rules in `ActivitySystem.ts` map (recent activity rate × violation score × time-of-day) → mood label.
- **Decay model**: vital stats decay every N statusline updates, not by wall-clock. Means a pet doesn't "die" if you're not coding.
- **LLM-as-classifier**: violations are scored by a remote Groq call comparing the user prompt to Claude's actions. Slowest path but the qualitative signal is unique.

### What it's missing
- Not multi-session aware. One pet, global.
- Statusline output competes with every other statusline tool.
- Violation system is gated to a specific external LLM (Groq).

---

## 5. ccpet (terryso/ccpet)

**Repo:** `src/ccpet.ts`, `src/core/Pet.ts`

### Data source
- **Statusline JSON only**.

### State model — minimal

`IPetState` in `core/Pet.ts`:

```
uuid, petName, animalType, emoji, expression
energy, accumulatedTokens, totalTokensConsumed, totalLifetimeTokens
birthTime, lastFeedTime, lastDecayTime
sessionTotalInputTokens, sessionTotalOutputTokens, sessionTotalCachedTokens
contextLength, contextPercentage, contextPercentageUsable, sessionTotalCostUsd
```

Token consumption feeds the pet (`TOKENS_PER_ENERGY` constant); time decays it. That's the entire lifecycle.

### Storage
- `~/.claude-pet/pet-state.json`
- `~/.claude-pet/animation-counter.json`
- Graveyard at `~/.claude-pet/graveyard/<name>/`

### Install model
- npm install + statusline command

### Notable design choices
- **Pure statusline**: no hooks, no transcript reading, no network. Very low friction.
- **Global leaderboard**: opt-in upload to `https://ccpet.surge.sh/`. Single shared service.
- **Per-pet UUID** decoupled from session: lets the leaderboard track a pet across reinstalls.

### What it's missing
- Same as tamagotchi — single-tenant, statusline-only, no event API.
- No model of "what is Claude doing right now" beyond token totals.

---

## 6. hoangsonww/Claude-Code-Agent-Monitor (ccam)

**Repo:** `scripts/hook-handler.js`, `server/db.js`, `server/routes/hooks.js`, `ARCHITECTURE.md`

### Data sources
- **Hooks** via a Node forwarder (`scripts/hook-handler.js`)
- **JSONL transcripts** via `scripts/import-history.js` (sharing the same parser as live import)
- **OpenAPI / REST**, **WebSocket**, **MCP**, **VS Code extension**, **plugins** all consume the same backend

### State model — explicit relational schema

In `server/db.js`:

```sql
CREATE TABLE sessions (
  id TEXT PRIMARY KEY,
  name TEXT,
  status TEXT NOT NULL DEFAULT 'active'
    CHECK(status IN ('active','completed','error','abandoned')),
  cwd TEXT,
  model TEXT,
  started_at TEXT, ended_at TEXT,
  metadata TEXT
);

CREATE TABLE agents (
  id TEXT PRIMARY KEY,
  session_id TEXT NOT NULL,
  name TEXT NOT NULL,
  type TEXT DEFAULT 'main' CHECK(type IN ('main','subagent')),
  subagent_type TEXT,
  status TEXT DEFAULT 'waiting'
    CHECK(status IN ('working','waiting','completed','error')),
  task TEXT,
  current_tool TEXT,
  parent_agent_id TEXT,
  metadata TEXT
);

CREATE TABLE events (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  session_id TEXT NOT NULL,
  agent_id TEXT,
  event_type TEXT NOT NULL,
  tool_name TEXT,
  summary TEXT,
  data TEXT
);
```

This is the most complete normalized state model in any of the tools — sessions, agents (with parent/child), events all explicit and constrained.

### Event vocabulary

- Hook event types stored in `events.event_type`: `PreToolUse`, `PostToolUse`, `Stop`, `SubagentStop`, `SessionStart`, `SessionEnd`, `Notification`, `UserPromptSubmit`, `PreCompact`, etc.
- WebSocket broadcast topics: `session_created`, `session_updated`, `agent_created`, `agent_updated`, `event_inserted`, etc.

### Hook → state transition logic

In `server/routes/hooks.js`:

| Hook                  | State change                                         |
|-----------------------|------------------------------------------------------|
| `SessionStart`        | upsert session, create main agent                    |
| `PreToolUse`          | session→`active`, main agent→`working`, set `current_tool` |
| `PostToolUse`         | clear `current_tool`                                  |
| `Stop`                | main agent→`completed` if not waiting                |
| `SubagentStop`        | subagent→`completed`                                  |
| `SessionEnd`          | session→`completed`                                   |
| `Notification` (matches WAITING_INPUT_PATTERN) | agent→`waiting`         |
| `UserPromptSubmit`    | reactivate, clear waiting                             |

Includes reactivation logic for stale/abandoned sessions and a 3-hour stale threshold.

### Storage
- SQLite (better-sqlite3 with WAL); fallback to node:sqlite for Node 22+
- Default path `data/dashboard.db`

### Install model
- `scripts/install-hooks.js` writes 7 hook entries (`SessionStart`, `PreToolUse`, `PostToolUse`, `Stop`, `SubagentStop`, `Notification`, `SessionEnd`) into `~/.claude/settings.json`
- Preserves existing hooks — only adds/updates entries containing `hook-handler.js`
- Hook handler is failsafe: 3s timeout, 5s safety net, always exits 0

### Notable design choices
- **Live + historical share the parser** (`parseSessionFile` / `importSession`) — guarantees imported state matches live ingestion.
- **Status reactivation logic** for resumed/imported sessions: a `Stop` event on a `completed` session reactivates only if the session wasn't `error`.
- **`Notification` parsing with `WAITING_INPUT_PATTERN` regex**: this is the heuristic for "is the agent waiting for user." Nontrivial because Claude Code conflates idle and waiting.
- **Plugin marketplace included**: `plugins/ccam-analytics`, `ccam-productivity`, `ccam-insights`, `ccam-dashboard`, `ccam-devtools` — separately installable Claude plugins that consume the same backend.
- **Failsafe forwarder**: same fail-silent pattern as Pixel Agents and disler — never blocks Claude Code.

### What it's missing
- Coupled to its UI — no documented contract for "third-party tools subscribing to the same state via the WebSocket".
- The state machine is implicit in hook handler logic, not in a separate state-machine module.

---

## 7. simple10/agents-observe

**Repo:** `hooks/scripts/hook.sh`, `app/server/src/storage/types.ts`, `.claude/settings.json`

### Data source
- **Hooks only**, via a tiny bash wrapper that backgrounds a Node CLI.

### State model

`InsertEventParams` is the storage shape:

```ts
export interface InsertEventParams {
  agentId: string
  sessionId: string
  hookName: string         // raw hook event name
  timestamp: number
  payload: Record<string, unknown>
  cwd?: string | null
  _meta?: Record<string, unknown> | null
}
```

Same passthrough pattern as disler. Plus a `projects` table (slug-keyed) for multi-project filtering.

### Storage
- SQLite (`sqlite-adapter.ts`)
- Sessions auto-resolve to projects by `cwd` or `transcript_path` dirname

### Install model
- Configured as a plugin with `/observe debug`, `/observe status` slash commands
- Bash hook wrapper backgrounds Node so hook returns in 2-5 ms instead of 50-100 ms

### Notable design choices
- **Bash wrapper for speed**: the entire reason for the bash layer is hook-exit latency. A Claude Code hook blocks the agent until exit; backgrounding the heavyweight Node process with `&` and redirecting fds returns ~10x faster.
- **Project auto-detection**: if `AGENTS_OBSERVE_PROJECT_SLUG` isn't set, the server walks back from `transcript_path` to find a sibling session with a project ID.
- **Docker for the server**: explicitly designed to run the backend in Docker; the host runs only the hook wrapper.

### What it's missing
- Same as disler — passthrough event log, no normalized state machine.

---

## 8. claude-team-dashboard (mukul975)

**Repo:** `server.js`

### Data source
- **Filesystem watcher** on `~/.claude/teams/*/config.json` and `~/.claude/teams/*/inboxes/*.json` via `chokidar`. **No hooks.**

### State model

Implicit — derived from Claude Code's own teams-on-disk format:
- `~/.claude/teams/<team-name>/config.json` — team config + members
- `~/.claude/teams/<team-name>/inboxes/<agent>.json` — per-agent message inbox

The dashboard reads these directly; state is "whatever is in the files."

### Notable design choices
- **No hook installation needed**: Claude Code already writes the team state to disk. Watcher is the entire integration.
- **Inter-agent message tracing**: by reading the inbox files, the UI shows D3-rendered communication graphs.
- **Side-effect-free**: doesn't install hooks, doesn't touch settings.json, doesn't run anything in the agent process.

### What it's missing
- Only works for Agent Teams mode. A solo Claude Code session has nothing in `~/.claude/teams/`.
- No tool-call or subagent timing — just the messages-and-inboxes layer.

## 9. tmux-agent-sidebar (hiroppy)

**Repo:** `src/event.rs`, `src/state.rs`, `src/process.rs`, `src/adapter/`, `hooks/hooks.json`, `hook.sh`

### Data sources
- **Hooks** (primary): a 14-event hook config covering `SessionStart`, `SessionEnd`, `UserPromptSubmit`, `Notification`, `Stop`, `StopFailure`, `PermissionDenied`, `CwdChanged`, `SubagentStart`, `SubagentStop`, `PostToolUse`, `TaskCreated`, `TaskCompleted`, `TeammateIdle`, `WorktreeCreate`, `WorktreeRemove`. The most comprehensive hook coverage of any tool surveyed.
- **Process tree scanning** (secondary): `ps -eo pid,ppid,comm,args` walked into a parent→children map, used to detect dead agents whose hooks never fired.

### State model — explicit pane status enum

In `src/tmux/types.rs`:

```rust
pub enum PaneStatus {
    Running,
    Background,
    Waiting,
    Idle,
    Error,
    Unknown,
}
```

Plus a permission-mode enum (`Default | Plan | AcceptEdits | Auto | DontAsk`) tracked per pane. State is persisted to **tmux pane options** (`@pane_status`, `@pane_agent`) — readable by other tools via `tmux show -t "$pane_id" -pv @pane_status`. This makes tmux-agent-sidebar a publisher as well as a presenter.

### Event vocabulary — third independent instance of `AgentEvent`

In `src/event.rs`, the `AgentEvent` enum is structurally similar to Pixel Agents's but more granular:

```rust
pub enum AgentEvent {
    SessionStart { agent, cwd, permission_mode, source, worktree, agent_id, session_id }
    SessionEnd { end_reason }
    UserPromptSubmit { agent, cwd, permission_mode, prompt, worktree, agent_id, session_id }
    Notification { agent, cwd, permission_mode, wait_reason, meta_only, worktree, agent_id, session_id }
    Stop { agent, cwd, permission_mode, last_message, response, worktree, agent_id, session_id }
    StopFailure { agent, cwd, permission_mode, error, worktree, agent_id, session_id }
    SubagentStart { agent_type, agent_id }
    SubagentStop { agent_type, agent_id, last_message, transcript_path }
    ActivityLog { tool_name, tool_input, tool_response }
    PermissionDenied { agent, cwd, permission_mode, worktree, agent_id, session_id }
    CwdChanged { cwd, worktree, agent_id, session_id }
    TaskCreated { task_id, task_subject }
    TaskCompleted { task_id, task_subject }
    TeammateIdle { teammate_name, team_name, idle_reason }
    WorktreeCreate
    WorktreeRemove { worktree_path }
}
```

Notable additions over Pixel Agents:
- `WorktreeInfo` as first-class metadata on most events
- `PermissionDenied` and `StopFailure` (failure-mode events)
- `CwdChanged` (working dir tracking)
- `TaskCreated` / `TaskCompleted` (task tracking)
- `TeammateIdle` (Agent Teams)
- `WorktreeCreate` / `WorktreeRemove` (sidebar-managed worktree lifecycle)

### Adapter abstraction — drift-free table

`src/adapter/mod.rs` defines:

```rust
pub struct HookRegistration {
    pub trigger: &'static str,        // "SessionStart", "PostToolUse", etc.
    pub matcher: Option<&'static str>,
    pub kind: AgentEventKind,         // compile-time enum, not string
}
```

Each adapter (claude, codex, opencode) exposes its `HOOK_REGISTRATIONS` table as the single source of truth. Setup wizards, README snippets, and the dispatcher all read from it. Tests enforce **bidirectional drift**: every entry in the table must `parse()` to the matching kind, and every kind `parse()` accepts must appear in the table. This catches "added a parse arm, forgot to update the registration table."

### Storage
- tmux pane options for live state (`@pane_status`, `@pane_agent`)
- tmux global variables for shared sidebar state
- No persistent database; state is rebuilt from hook events + ps scan on start

### Install model
- Distributed as a TPM (tmux plugin) + a Claude Code plugin marketplace entry
- Installs hooks via `claude /plugin install` rather than direct settings.json mutation
- The shim (`hook.sh`) delegates to a Rust binary, located via several fallback paths so the user can rebuild without regenerating config

### Notable design choices
- **Process tree scanning as ground truth.** This is the **first tool surveyed that actually walks `ps`** to detect dead agents. The README explicitly says: "Dead pane cleanup — If an agent exits without a hook, the periodic pid scan removes the stale pane on the next refresh cycle." This directly addresses the session-vs-process gap identified in the cross-cutting analysis.
- **State published to tmux pane options.** Other tools can read `@pane_status` for any pane without integrating with tmux-agent-sidebar's API. This is a poor-man's pub/sub via tmux's variable system — surprisingly elegant.
- **Compile-time event enum + drift-free table.** Strongest type discipline of any surveyed tool.
- **Multi-agent from day one.** Codex and OpenCode adapters ship alongside Claude, with each adapter implementing a shared trait.
- **`meta_only` flag on Notification events.** Recognizes that some notifications carry metadata but shouldn't trigger a visible status change — exactly the kind of nuance other tools handle with regex hacks.
- **Has a pet too.** `src/ui/pet.rs` — yet another tool with a built-in pet. The pet is an idle-state animation, not an external thing.

### What it's missing (relative to a generalized layer)
- State is published to tmux only — programmatic consumers outside tmux can't easily subscribe
- The event log isn't exposed as an API for historical queries — only the current pane state

### Net assessment as a presenter
tmux-agent-sidebar is **structurally the closest existing tool to a generalized layer.** Its `AgentEvent` enum, `HookRegistration` table, multi-agent adapter abstraction, and process-tree scanning are exactly the pieces a unified daemon needs. If the daemon existed, tmux-agent-sidebar would be its strongest validation — but also its most painful refactor, because its current architecture *is* the abstraction.

---

## 10. opensessions (Ataraxy-Labs)

**Repo:** `CONTRACTS.md`, `packages/runtime/src/server/index.ts`, `packages/runtime/src/agents/`, `packages/runtime/src/agents/watchers/`

### Data sources — a five-source architecture

opensessions has the broadest ingest surface of any tool surveyed:

- **JSONL tail** for Claude Code (`~/.claude/projects/<encoded-path>/*.jsonl`)
- **JSONL tail** for Codex (`~/.codex/sessions/**/*.jsonl`)
- **JSON tail** for Amp (`~/.local/share/amp/threads/T-*.json`)
- **SQLite poll** for OpenCode (`~/.local/share/opencode/opencode.db`)
- **HTTP API** at `POST /api/agent-event` for any tool to push events

**No hook-based ingest.** All four built-in agents are detected by tailing files the agent itself writes — no hook installation required.

### State model — formal `AgentStatus`

From `CONTRACTS.md`:

```ts
type AgentStatus =
  | "idle"
  | "running"
  | "done"
  | "error"
  | "waiting"
  | "interrupted";
```

Six values, with `done | error | interrupted` defined as terminal states. The tracker's behavior depends on this distinction — terminal states trigger unseen markers and a 5-minute pruning window; non-terminal `running` events get pruned after 3 minutes.

### Event shape — formal contract

```ts
interface AgentEvent {
  agent: string;           // "amp" | "claude-code" | "codex" | "opencode"
  session: string;         // resolved mux session name
  status: AgentStatus;
  ts: number;
  threadId?: string;       // for tracking multiple threads in one session
  threadName?: string;
  unseen?: boolean;        // tracker-derived
}
```

Tracker semantics:
- Keys instances by `agent:threadId`, falling back to `agent`
- One session can have multiple active agent instances
- Unseen state is per-instance, then derived to the session level
- Stale `running` events pruned after 3 minutes
- Seen terminal instances pruned after 5 minutes

### `AgentWatcher` extension interface — documented

```ts
interface AgentWatcher {
  readonly name: string;
  start(ctx: AgentWatcherContext): void;
  stop(): void;
}

interface AgentWatcherContext {
  resolveSession(projectDir: string): string | null;
  emit(event: AgentEvent): void;
}
```

`resolveSession(projectDir)` does exact match across mux sessions, then parent-child prefix matching for nested project paths. New agents become a Watcher class plus a registration line. **This is a documented public extension surface, with code examples in the contracts doc.**

### Mux abstraction — also documented

```ts
interface MuxProviderV1 {
  readonly specificationVersion: "v1";
  readonly name: string;
  listSessions(): MuxSessionInfo[];
  switchSession(name: string, clientTty?: string): void;
  // ...
}
```

Plus optional capabilities (`WindowCapable`, `SidebarCapable`). tmux is the supported impl; zellij is experimental. The mux layer is separate from the agent layer — meaning the same agent watchers work regardless of the underlying multiplexer.

### HTTP API endpoints

The Bun-based server exposes a real HTTP API:

```
POST /api/agent-event     # accept agent state from any external tool
POST /api/runtime/pi/upsert
POST /api/runtime/pi/delete
POST /set-status          # session-level free-text status
POST /set-progress
POST /log
POST /notify
POST /focus, /toggle, /quit, /refresh, /switch-index, /ensure-sidebar, /pane-exited
WS  (sidebar topic)        # broadcast state to sidebar UI
```

### Storage
- In-memory tracker with periodic pruning
- Plus per-session metadata stored in tmux session/global variables

### Install model
- TPM plugin → clones the repo, runs `bun` against the checkout
- Auto-restarts the server on plugin update so new code takes effect
- Has an explicit uninstall script that cleans up tmux hooks, keybindings, env vars

### Notable design choices
- **No agent-side hook installation.** This is the most distinctive choice. Every other comprehensive tool installs hooks; opensessions instead tails the files agents already write. **Tradeoff:** slower-to-detect transitions (file polling at 2s for Claude/Codex/Amp, 3s for OpenCode), but zero collision with other hook-installing tools.
- **Capability-based mux abstraction** — the same agent watchers work for tmux, zellij, or any future multiplexer.
- **Per-thread instance tracking.** A single mux session can host multiple threads (e.g., a Claude Code main session that spawns subagents). The tracker handles them as distinct instances.
- **Documented extension contracts.** `CONTRACTS.md` is a real document — agent and mux extensions have public interfaces with code examples.
- **The `/api/agent-event` endpoint is exactly the design's `POST /events` shape.** A custom tool can push agent state in via HTTP without writing a Watcher.
- **markPluginOwned mechanism** — when an external plugin pushes events for a thread, the corresponding watcher backs off cloud/WebSocket polling for that thread. Co-existence between watchers and external pushers.

### What it's missing (relative to a generalized layer)
- No event log persistence — events are projected to current state and old events are pruned
- No historical query API — once a session ends + its grace period expires, it's gone
- No formal projection separation: tracker mixes "current state per instance" with "session-level rollup" without a canonical projection layer
- The reaction enum (6 values) is smaller than OpenPets's (11) — fewer presentation hooks for fancy UIs

### Net assessment as a presenter
opensessions is **already most of the design from `03-design-sketch.md`**, minus the historical event log and minus the hook-router pattern. The contracts are public, the architecture is the right shape, and the multi-source ingest pattern (file tails + HTTP API) is the same idea.

If the daemon were built today, the most pragmatic path might be to **fork or extend opensessions** rather than build from scratch. The pieces missing are:
1. Persistent event log (SQLite, append-only)
2. Hook router as a fifth ingest source (the unified `claude-state-bus emit`)
3. Process-tree scanning for dead-agent detection (steal from tmux-agent-sidebar)
4. Statusline shim composition
5. Agent-agnostic projection of the 11-value reaction enum for pets/HUDs

---

## 11. tmux-agent-status (samleeney)

**Repo:** `hooks/better-hook.sh`, `scripts/`, `setup-server.sh`

### Data sources
- **Hooks** (primary): Claude Code via `hooks/better-hook.sh`, Codex via `hooks/codex-hook.sh`
- **Process polling** (fallback) for status verification
- **File-based protocol** for custom integrations: any tool can write to `~/.cache/tmux-agent-status/<session>.status`

### State model — minimal

Three values written into status files: `working | done | wait`. Plus per-pane variants (`<session>_<pane>.status`) and parking flags (`<session>_<pane>.parked`).

### Event vocabulary — none

Hook handler is a bash script that writes status strings. No structured event log. No event types — hooks dispatch on `$1` (`UserPromptSubmit`, `PreToolUse`, `Stop`, `Notification`) and write the corresponding status string.

### Hook → status mapping (Claude-specific)

| Hook              | Status    | Notes |
|-------------------|-----------|-------|
| `UserPromptSubmit`| `working` | Also clears wait/park overrides |
| `PreToolUse`      | `working` | Does NOT unpark |
| `Stop`            | `done`    |       |
| `Notification`    | `done` + sound | Treats notification as "needs attention" |

### Storage
- Status files at `~/.cache/tmux-agent-status/<session>.status` (per-session)
- Pane files at `~/.cache/tmux-agent-status/panes/<session>_<pane>.status`
- Wait files, parked files at `~/.cache/tmux-agent-status/{wait,parked}/`
- Refresh trigger file at `~/.cache/tmux-agent-status/.sidebar-refresh`

### Install model
- TPM plugin
- Manual `~/.claude/settings.json` edit to add hooks
- Repo-local `.codex/hooks.json` ships with the plugin so Codex can pick up hooks automatically when working inside this repo

### Notable design choices
- **File-based extension protocol as the public API.** This is an unusual but interesting choice: any agent can integrate by writing to a file. The README explicitly says: "Integrate any AI coding tool with either of these approaches: Write `working`, `done`, or `wait` to `~/.cache/tmux-agent-status/<session>.status`."
- **Session and per-pane status are separate.** The session-level status is computed from per-pane status files using a small reducer (`working` if any pane is working, `wait` if any pane is waiting and none is working, `done` otherwise).
- **Park/wait as user-managed states.** "Park" means "I'm not done with this but stop showing it as urgent." This is presenter-level UX state, not agent state.
- **Smallest vocabulary of any tool surveyed.** Three states. Compare to OpenPets's 11 or the Pixel Agents/tmux-agent-sidebar event unions.

### Net assessment as a presenter
Trivially fits the design. Maps to a tiny sidebar that subscribes to session state changes and renders three values. The file-based protocol idea is interesting and probably should be supported as a *low-friction ingest source* on the daemon — any tool that just wants to set a status without writing code can drop a file in a watched directory.

---

---

## Summary table

| Tool | Data source | State model | Storage | Install footprint | Multi-session |
|------|-------------|-------------|---------|-------------------|---------------|
| OpenPets | MCP + hooks | Reaction enum (11 values) | In-memory + IPC discovery file | MCP + optional hooks | One pet, but per-project pet windows |
| Pixel Agents | Hooks + JSONL | `AgentState` + normalized `AgentEvent` union | VS Code state + layout file | HTTP server + hooks | Yes (per-terminal characters, sub-agents) |
| disler observability | Hooks only | Passthrough event log | SQLite | 12 Python hooks + uv | Yes (via swim-lane filter on `source_app`) |
| claude-code-tamagotchi | Statusline + transcript poll | Pet stats + behavior score | JSON + SQLite | Statusline + optional PreToolUse hook | No |
| ccpet | Statusline only | Pet token-energy stats | JSON | Statusline only | No |
| ccam | Hooks + JSONL | Full relational (`sessions`, `agents`, `events`) | SQLite | 7 hooks + Node handler | Yes (sessions and parent/child agents) |
| agents-observe | Hooks only | Passthrough events | SQLite | Hooks via bash wrapper | Yes (via projects table) |
| claude-team-dashboard | Filesystem watch on `~/.claude/teams/` | Read directly from teams/ files | None of its own | None — zero hooks | Teams only |
| tmux-agent-sidebar | Hooks + ps tree scan | `PaneStatus` enum + 16-variant `AgentEvent` | tmux pane options + globals | 16 hooks + Rust binary | Yes (per-pane, per-tmux-session) |
| opensessions | JSONL/JSON/SQLite tail + HTTP API | `AgentStatus` (6) + `AgentEvent` shape | In-memory tracker | Zero hooks (pure file tail) | Yes (per-thread instances within mux session) |
| tmux-agent-status | Hooks + status files | 3-state (`working`/`done`/`wait`) | Files in `~/.cache/tmux-agent-status/` | Hooks + TPM plugin | Yes (per-pane, per-tmux-session) |

---

## Cross-cutting analysis: session vs. process

A Claude Code session is a `~/.claude/projects/<encoded-cwd>/<session-id>.jsonl` file plus its `session_id`. The file persists indefinitely. You can `claude --resume <session-id>` and pick up where you left off. So the natural lifecycle states are actually:

1. **Session exists, process running** (live)
2. **Session exists, no process** (resumable, paused)
3. **Session exists, abandoned** (resumable but probably not coming back)
4. **Session ended** (SessionEnd received — `/exit` clean shutdown)

**None of the surveyed tools cleanly distinguish these states. One — tmux-agent-sidebar — actively scans process trees, but conflates pane-PID with session-PID rather than modeling them separately.** The rest collapse session and process into a single concept in different ways:

### How each tool conflates the two

**ccam** is the closest to noticing the distinction. A comment in `server/db.js`:

> *"Legacy sessions (created before SessionEnd hook) will never receive a SessionEnd event, so they stay 'active' forever. Complete any active session whose last event is older than 1 hour — the CLI process is certainly gone by then."*

And in `server/routes/hooks.js`:

> *"SessionEnd is the definitive signal that the CLI process exited."*

ccam's model intends `session.status = 'active'` to mean "a process is currently running this session," but the only signal it has is the `SessionEnd` hook. If that hook never fires (crash, `kill -9`, older Claude Code versions, force-quit terminal), the session sits as `active` indefinitely. Mitigation is a startup sweep that flips any `active` session with no events in 1 hour to `completed`. There is no PID column, no run ID, and no notion of "session exists, no process attached" — every such session gets demoted to `completed` after an hour.

**Pixel Agents** uses `vscode.Terminal` as a process proxy. When VS Code fires `onDidCloseTerminal`, the agent is removed. This works for terminals it spawned itself, but externally launched sessions (`isExternal: true`, e.g. a `claude --resume` from another VS Code window) have no real lifecycle signal — they persist in the agent registry until manually cleaned up. Sessions and processes are effectively conflated through the terminal handle, with a fallback heuristic for the external case.

**disler observability and agents-observe** have no state model at all — they're event logs. They don't ask "is this session active." A consumer reading the database has to invent its own definition based on event timestamps and `SessionEnd` events. Multiple processes attached to the same session would just appear as interleaved events in one session's stream with no distinguishing key.

**OpenPets** has the most interesting design here, though applied to a different problem. Its **lease + heartbeat** system is exactly the right shape for "process is running":

- `lease.acquire` when an MCP client connects (one Claude Code process)
- `lease.heartbeat` every 5 seconds while alive
- lease expires if heartbeats stop
- `lease.release` on clean shutdown

This is a per-process attachment primitive. If the Claude process dies (cleanly or not), the lease expires and the pet falls back to default. **OpenPets has the right mechanism but applies it to pet ownership, not session liveness.** It also has no session concept — only pet windows attached to leases.

**ccpet** has no process awareness — pure statusline-driven, infers activity from token deltas only.

**claude-code-tamagotchi** uses pid only for internal locking (its own analysis workers), never for tracking the Claude Code process.

**claude-team-dashboard** infers liveness from filesystem mtimes on team config and inbox files. No notion of process, but also no notion of session — only "team."

**tmux-agent-sidebar** is the **only tool surveyed that scans process trees directly.** It runs `ps -eo pid,ppid,comm,args`, builds a parent→children map, and walks descendants from each tmux pane's pid looking for an agent process. The README explicitly says: "If an agent exits without a hook, the periodic pid scan removes the stale pane on the next refresh cycle." This directly addresses the gap — but the model is still pane-centric, not session-centric. It conflates "this pane has an agent process running" with "this session is live." If you `claude --resume <session-id>` in a new pane, that's tracked as a new entry, not a continuation of the same session.

**opensessions** has formal terminal states (`done | error | interrupted`) and an in-memory tracker that prunes stale `running` events after 3 minutes and seen terminal instances after 5 minutes. No process scanning; relies on file-tail watchers. Per-thread instance tracking via `agent:threadId` keys means a session can be tracked across multiple processes — the closest any tool comes to modeling the process/session distinction explicitly.

**tmux-agent-status** has three states (`working | done | wait`) written to per-session files. A per-pane file system layered underneath, but session state is computed from per-pane state via a small reducer. No process awareness.

### Why the conflation happens

Claude Code's hooks emit `SessionStart` and `SessionEnd`, which are the **agent's** lifecycle events. There is no hook that fires when the **process** is dying non-cleanly:

- Crash: no event
- `kill -9`: no event
- Terminal force-closed: no event
- Container/VM going away: no event
- Network disconnect for SSH-attached terminals: no event

So tools that want to know "is the agent alive right now?" have only two tools available:

1. **Wait for an event and time out** (ccam's 1-hour sweep, agents-observe's per-event cwd update, Pixel Agents' permission/idle timers)
2. **Bind to a host signal that mirrors the process** (Pixel Agents → `vscode.Terminal`, OpenPets → MCP transport close)

Neither captures the Claude Code process directly. Nobody polls `~/.claude/projects/<cwd>/<session-id>.jsonl` for which files are currently being written to. Nobody walks the process tree looking for `claude` processes. Nobody models a heartbeat as a first-class entity.

### What a proper model would need

A clean session-vs-process model needs two related but distinct concepts:

```
Session                          Attachment (process)
-------                          --------------------
session_id (PK)                  attachment_id (PK)
project_dir                      session_id (FK)
created_at                       process_token  -- pid+starttime, or run-id from hook
last_event_at                    started_at
lifecycle_status:                last_heartbeat_at
  live | paused | ended          ended_at
                                 end_reason: clean | crash | timeout | replaced
```

Then derive:

- Session is **live** if it has at least one attachment with a fresh heartbeat
- Session is **paused** if it has no fresh attachments but the JSONL file exists and is recent
- Session is **abandoned** if no fresh attachments and the JSONL is older than some threshold
- Session is **ended** if `SessionEnd` was received

A session can have a sequence of attachments over time (resume → process → exit → resume → process). The current tools give you, at best, the most recent of these conflated with the session itself.

### Why this matters for a generalized state layer

The hooks API gives you `SessionStart` and `SessionEnd`, which are the agent's lifecycle, not the process's. A unified state layer that wants to give correct answers to "what's running right now?" needs additional inputs the hook stream doesn't provide:

1. **A heartbeat from the running process** — could be:
   - A long-running statusline process (statusline ticks are deterministic, ~once per turn)
   - An MCP keepalive (OpenPets-style)
   - A periodic `PreToolUse` or hook-based ping
   - Just observing JSONL file mtime changes
2. **A process identity** the layer can correlate hooks against — Claude Code doesn't expose a stable run ID per process today, but `(session_id, hook_first_seen_at)` is a reasonable substitute
3. **A clear "no process" state** that's distinct from "process not yet seen" and "process ended cleanly"

Without these, every consumer of the state layer reimplements the same staleness heuristic in slightly different ways (1 hour for ccam, terminal-close for Pixel Agents, 5s lease for OpenPets, never for disler). And every consumer gets some edge cases wrong: ccam loses sessions that crash and aren't cleaned up before an hour passes; Pixel Agents misses externally launched sessions; OpenPets has no concept that the same session might be resumed from a different process later.

This is a strong argument for shipping the generalized layer as the canonical answer to liveness. It's a piece of plumbing that genuinely benefits from being shared.

---

## Cross-cutting analysis: worktrees

Git worktrees have become the dominant pattern for running parallel agents — every agent gets its own checkout of the same repo on a different branch, so they don't stomp on each other's edits. The orchestrator tools (dmux, vibe-kanban, conductor, ccmanager, claude-squad) treat worktrees as a first-class concept because they create them. **The observability tools mostly don't track worktrees at all.**

This is a real gap. When a developer is running 5 parallel sessions in 5 worktrees against the same repo, "what is each session doing right now?" is much more useful when grouped by repo/branch than by `cwd` alone.

### What "tracking worktrees" actually means

Three different things, often conflated:

1. **Awareness** — does the tool know this `cwd` is a worktree at all, vs. a normal repo checkout?
2. **Repo grouping** — does the tool understand that `~/proj/.worktrees/feature-foo` and `~/proj/main` are two views of the same repo on different branches?
3. **Lifecycle management** — does the tool create, remove, or merge worktrees itself?

Almost no observability tool does (3) (that's an orchestrator concern). A useful state layer needs (1) and (2). Most tools surveyed do neither.

### How each tool currently handles worktrees

**Observability / state tools:**

| Tool | Worktree awareness | Repo grouping | Notes |
|------|--------------------|---------------|-------|
| **OpenPets** | None | None | `cwd` flows through hooks but isn't decomposed. `docs/release.md` mentions worktree only as a build/dev concern, not in the runtime data model. |
| **Pixel Agents** | None | None — sessions group by `cwd` string only. | The `getSessionDirs` provider method computes the JSONL location from `cwd` (`~/.claude/projects/<encoded-cwd>/`), but `cwd` for a worktree is the worktree path, not the main repo. So two sessions on the same repo in different worktrees show up as completely unrelated. |
| **disler observability** | None | None | `cwd` isn't even a typed field on `HookEvent` — it lives inside `payload`. |
| **agents-observe** | Some | Some — has explicit worktree-detection design docs (`2026-05-04-worktree-project-detection-design.md`). Project resolution walks back from `transcript_path` to find a sibling session with the same project ID, which catches some worktree cases. | Closest of the observability tools to handling worktrees, but the abstraction is "project," not "repo + branch." |
| **ccam** | None | None | Only `cwd` on the session row. No git awareness in the schema. |
| **ccpet, tamagotchi, claude-team-dashboard** | None | None | No git concept at all. |

**Tmux/sidebar tools:**

| Tool | Worktree awareness | Repo grouping | Notes |
|------|--------------------|---------------|-------|
| **tmux-agent-sidebar** | **First-class** | **Yes** | `WorktreeInfo` is metadata on most events. Has dedicated `WorktreeCreate` / `WorktreeRemove` event variants. `CwdChanged` event tracks cwd movement. Stores worktree state in tmux pane-options (`PANE_WORKTREE_NAME`, `PANE_WORKTREE_BRANCH`, `PANE_CWD`). Handles the **subagent-aware case**: when `WorktreeRemove` fires while subagents are still active, defers cleanup via a pending marker rather than wiping the parent's state. |
| **opensessions** | Yes (binary flag) | Yes (by branch) | `getGitInfo()` runs `git rev-parse --abbrev-ref HEAD --git-dir`; `isWorktree: true` if `--git-dir` contains `/worktrees/`. Emits `branch` and `dirty` flags. 5-second cache. Lighter than tmux-agent-sidebar but enough for "1 main, 3 worktrees on feature/X" rendering. |
| **tmux-agent-status** | Mentioned only | None in data model | README mentions worktree-isolated sessions as a use case but the state model is per-pane, not repo-aware. |

**Orchestrators (for reference — not primary observability scope):**

| Tool | Worktree role |
|------|---------------|
| **dmux** | The reason dmux exists. Each pane *is* a worktree+branch. `DmuxPane.worktreePath` is required. Has a `WorktreeCleanupService` for queued background deletion, a `worktreeDiscovery` utility that recursively scans for **nested** worktrees (worktrees within worktrees, created by hooks), and depth-ordered merge logic that merges deepest first. The most worktree-sophisticated tool surveyed. |
| **ccmanager, vibe-kanban, conductor, crystal, claude-squad** | All create per-task worktrees. Each tracks its own per-pane/card worktree state. None expose this to other observability tools. |

### Why the observability tools miss it

Two reasons:

1. **The hook payload doesn't say "worktree."** Claude Code's hook payload includes `cwd`, `transcript_path`, `session_id` — none of these declare "this is a worktree of repo X." A tool has to *derive* it by running `git rev-parse --git-dir` (the path will contain `/worktrees/`) and `git config core.worktree` or similar. Nobody bothers because the bare `cwd` "works" for displaying the path.

2. **The data model is `cwd`-keyed.** When sessions are grouped or filtered by `cwd`, sessions in different worktrees naturally fall into different buckets. There's no "repo" concept to roll them up under. The fix is a cheap derivation — the path a worktree's `.git` file points at is the canonical repo root — but it has to be explicit in the schema.

### What a proper model needs

Three derived fields on the session row, computable cheaply at session start (or on first hook event):

```sql
ALTER TABLE sessions ADD COLUMN repo_root  TEXT;   -- canonical repo path (the main worktree)
ALTER TABLE sessions ADD COLUMN worktree   TEXT;   -- this session's worktree path; same as cwd if it IS a worktree
ALTER TABLE sessions ADD COLUMN branch     TEXT;   -- HEAD branch at session start (may change later)
```

Computation, in priority order:

1. **From `cwd` at session start**: `git -C "$cwd" rev-parse --git-common-dir` gives the shared `.git` directory; its parent is the canonical repo root. `git -C "$cwd" rev-parse --show-toplevel` gives the *worktree* root (which may equal repo root if it's the main worktree, or differ if it's a linked worktree). `git -C "$cwd" rev-parse --abbrev-ref HEAD` gives the branch.
2. **Cache by repo root + cwd** so the lookup runs once per session, not per event.
3. **Re-derive on `CwdChanged`** if the agent moves between trees mid-session (rare but allowed).

This lets consumers do useful queries:

```
GET /sessions?repo_root=/Users/josh/proj    -> all sessions on this repo, across worktrees
GET /sessions?branch=feature/auth           -> all sessions on this branch
```

And derive aggregations:

```
"3 sessions on /Users/josh/proj"
   ├── main (1 session, idle)
   ├── feature/auth (1 session, working in Edit)
   └── feature/billing (1 session, waiting for input)
```

That's the kind of grouping users want when running parallel agents — it isn't currently possible to render in any of the observability tools.

### Worktree lifecycle events

Two additional events worth modeling explicitly, drawn from tmux-agent-sidebar's hook union:

```
worktreeCreate { worktree_path, branch, base_branch?, repo_root }
worktreeRemove { worktree_path, repo_root }
```

These come from orchestrators (dmux, ccmanager) — they can emit these into the daemon when they create/remove worktrees, so observability tools see worktree appearance/removal as first-class events rather than inferring it from cwd changes. The daemon doesn't *create* worktrees; it just records that a creation happened.

A subtlety from tmux-agent-sidebar's implementation: when a `worktreeRemove` fires while subagents are still active in that worktree, you can't safely wipe state — the children might still be running. Their solution is a pending marker that defers cleanup until subagents stop. The unified design should follow the same pattern: `worktreeRemove` events flag the affected worktree as "removing," and final cleanup waits for all attachments tied to that worktree to close.

### Implications for the unified design

Adding worktree as a first-class concept is cheap and high-value. Concrete additions to `03-design-sketch.md`:

1. Add `repo_root`, `worktree`, `branch` columns to the `sessions` table
2. Compute these at first hook event (the shim has `cwd`; one `git rev-parse` call answers all three)
3. Cache by `cwd` so the lookup runs once per attachment
4. Add `worktreeCreate` and `worktreeRemove` to the event vocabulary, emitted by orchestrators
5. Re-derive on `CwdChanged` events
6. In the read API: support `?repo_root=` and `?branch=` filters; expose `repo_root` and `worktree` on session response

The cost is one git invocation per session per attachment open. With caching, effectively free. The benefit is that every observability tool — pets, HUDs, dashboards — gets to render "3 worktrees on this repo" groupings without each reimplementing the logic.

---

## What this implies for a generalized state layer

### 1. There are essentially four coexisting state models

- **Reaction-style** (OpenPets, tmux-agent-status): single current state from a small enum, no history. Cheap to query, useless for analytics.
- **Event-log** (disler, agents-observe): timestamped raw events. Maximum fidelity, but every consumer reimplements the state machine.
- **Normalized relational** (ccam, Pixel Agents): explicit `sessions`/`agents`/`events` with state columns and parent/child links. Best for queries; requires a state-transition implementation per hook.
- **In-memory tracker with TTL pruning** (opensessions): per-thread instance state with stale-event pruning. No persistence, but precisely the right shape for live UIs.

A generalized layer should probably ship **all of them** as views over the same store: an event log as the source of truth, a derived state machine for "current state per session/agent," and a reaction-style projection for low-bandwidth consumers (statusline, pets).

### 2. Three independent tools converged on essentially the same `AgentEvent` abstraction

Pixel Agents (`server/src/provider.ts`), tmux-agent-sidebar (`src/event.rs`), and opensessions (`CONTRACTS.md`) all independently arrived at:
- A normalized event union over hook events
- A per-agent adapter abstraction that translates raw payloads into the union
- A multi-agent design (Claude + Codex + OpenCode at minimum)

The convergence is striking. **The abstraction is right.** What's missing is a shared package — three tools have re-implemented the same idea because no one shipped it as infrastructure.

The richest event union is tmux-agent-sidebar's (16 variants including `WorktreeCreate`, `CwdChanged`, `PermissionDenied`, `TaskCreated`/`TaskCompleted`). The most documented is opensessions (`CONTRACTS.md`). The most layered is Pixel Agents (separate provider/teamProvider).

### 3. OpenPets' reaction vocabulary is the right minimal external API

`idle | thinking | working | editing | running | testing | waiting | success | error | celebrating | waving` is small enough to be agent-agnostic and stable. A presenter (statusline, pet, HUD) only needs to subscribe to a stream of these per session.

opensessions's vocabulary is similar but smaller (`idle | running | done | error | waiting | interrupted`). tmux-agent-status is even smaller (`working | done | wait`). The right answer is probably the OpenPets one as the canonical projection, with explicit downsamplers for tools that want fewer states.

### 4. The hook collision problem can be solved with a single registered handler

Every hook-driven tool today writes its own line into `~/.claude/settings.json`. A unified daemon would:
- install **one** hook per event type
- the hook script POSTs to a local daemon
- the daemon fans out to registered consumers (presenters), each subscribing via WebSocket / SSE / IPC
- presenters never touch `settings.json`

This is structurally what ccam, Pixel Agents, disler, and tmux-agent-sidebar already do internally — just for their own consumers. The opportunity is to make the daemon the shared substrate. **opensessions has already done this for the read side** — its `POST /api/agent-event` endpoint is exactly the API; what it's missing is a hook-router on the front end.

### 5. Statusline is also single-tenant and needs a router

Same problem, different slot. A unified statusline shim could:
- be installed as the single statusline command
- read the JSON from stdin
- dispatch to N statusline presenters that each contribute a segment
- combine the segments and emit

This is pure presenter composition — no state needed.

### 6. JSONL transcript reading can be more than a fallback

Initial assumption: hooks are primary, JSONL is a fallback. **opensessions challenges this directly** — it ships *zero* hooks and gets all four agents working purely from file tails. Tradeoff: 2-3 second polling latency vs. instant hook delivery, in exchange for zero settings.json mutation and zero collision with other tools.

A generalized layer should support both:
- prefer hooks when installed (low latency, authoritative for tool ids)
- use JSONL/SQLite/JSON tail when hooks aren't or can't be installed
- merge both streams when both are present, deduping on event identity

### 7. Process-tree scanning is a real ingest source

tmux-agent-sidebar walks `ps -eo pid,ppid,comm,args` to detect dead agents whose hooks never fired. **This is the missing piece for the session-vs-process distinction.** No other tool does it. The unified daemon should include it as a fifth ingest source, run on the sweep cadence.

### 8. MCP is underused but well-suited

OpenPets is the only one using MCP for state ingest. It's the cleanest path for "agent self-reports state to the layer" — agents that don't have hooks (any external coding assistant) can still call MCP tools. A generalized state layer should accept input from:
- **hooks** (push, lossy on tool ids)
- **JSONL/JSON/SQLite tail** (pull, full fidelity but file-bound)
- **MCP self-report** (push, agent-driven, agent-agnostic)
- **statusline tap** (pull, deterministic cadence, includes tokens)
- **process tree scan** (pull via sweep, ground truth for liveness)
- **HTTP API** (push, for any tool to send events without writing a watcher — opensessions's pattern)
- **File-drop protocol** (push, lowest-friction, write a file in a watched dir — tmux-agent-status's pattern)

All seven feed the same event log.

### 9. Existing tools fall into clusters that map to the design

- **Already most of the design**: opensessions (multi-source ingest, formal `AgentEvent`, HTTP API, extension contracts) and tmux-agent-sidebar (richest event union, process-tree scanning, drift-free adapter table).
- **Right abstraction shape, not packaged**: Pixel Agents (`HookProvider` interface bundled in VS Code extension).
- **Right state model, presenters built into the same product**: ccam (relational schema, hook→state transitions, session reactivation logic).
- **Right minimal vocabulary**: OpenPets (11-value reaction enum), tmux-agent-status (3-value bash-friendly).
- **Pure event-log presenters**: disler, agents-observe.
- **Pure pet/statusline presenters**: ccpet, claude-code-tamagotchi, Claude HUD.
- **Filesystem-watch specialists**: claude-team-dashboard.

The unified daemon would be **the merge of opensessions and tmux-agent-sidebar**, with formal projections borrowed from ccam and an event-log persistence layer borrowed from disler/agents-observe. Pixel Agents's adapter abstraction becomes the per-agent extension interface. OpenPets's reaction enum becomes the canonical thin projection.



---

## Suggested next steps

1. Sketch a minimal protocol — events + state vocabulary + session/process model — that Pixel Agents and OpenPets could both adopt without breaking changes.
2. Prototype the unified hook router (single hook line in `settings.json`, fanout to localhost subscribers).
3. Prototype the heartbeat layer: figure out the cheapest reliable signal (statusline tap? JSONL mtime poll? a tiny `PreToolUse` ping?) and how it composes with hook-derived state.
4. Look at whether shipping it as a small Rust/Go binary (not Node) would dodge the "every dashboard ships its own runtime" problem.
5. Talk to Pablo De Lucca (Pixel Agents) and Alvin Unreal (OpenPets) about whether they'd consume a shared package — they're the two who already articulate the problem.
