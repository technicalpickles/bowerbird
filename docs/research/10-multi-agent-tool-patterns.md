# How existing tools support multi-agent

The v2 design and the multi-agent gap analysis (`09-multi-agent-support.md`) sketched the abstraction. The honest question is whether existing tools have already solved this in practice, and what abstractions they converged on. This document examines five tools that explicitly support multiple coding-agent CLIs and pulls out the patterns.

The tools examined:

- **opensessions** — Ataraxy Labs' tmux sidebar with first-class watchers for 5 agents
- **ccmanager** — Piebald AI's session manager supporting 8 agents
- **agent-flow** — patoles' real-time visualizer for Claude Code + Codex
- **AgentDeck** — puritysb's physical-controller bridge for 4 agents across 13 surfaces
- **Agent Sessions** — jazzyalex's macOS session browser for 7 agents

These are the tools that actually shipped multi-agent support, not just promised it.

## The four ingest models that emerged

Across all five tools, **every multi-agent system surveyed has at least two different ingest paths** for different agents. No tool found a single mechanism that works everywhere. The four models:

### 1. Hook ingest (config-installed shell commands)

Used by: agent-flow (for Claude), AgentDeck (for Claude Code).

Pattern: write to `~/.claude/settings.json` to install a hook command that POSTs events to a local HTTP server. Zero-latency streaming; full payload preservation. Works for Claude, Codex, Gemini, Cursor (the tier-1 agents from the previous analysis).

### 2. File-tail (filesystem watch + JSONL parsing)

Used by: opensessions (for Claude, Codex, pi), agent-flow (for Codex), Agent Sessions (everyone — it's a session *browser*, not a live visualizer).

Pattern: watch the relevant directory with `fs.watch` (recursive where available), parse JSONL line-by-line, derive state from message content. Works for any agent that persists transcripts. Cheaper to install (no settings.json mutation) but coarser state signals because content has to be interpreted.

### 3. SQLite polling (database read)

Used by: opensessions (for OpenCode).

Pattern: bun:sqlite in readonly mode against `~/.local/share/opencode/opencode.db`. Polls every N seconds. Reads tables (`session`, `message`, `part`) and JSON columns within them. Necessary because OpenCode doesn't persist JSONL transcripts and doesn't have config-installable hooks. The only choice if you want non-plugin observability.

### 4. Cloud REST + WebSocket

Used by: opensessions (for amp).

Pattern: poll Amp's cloud REST API every 10s for thread discovery, then open a WebSocket per thread for real-time status. Amp is a cloud-hosted agent — there's no local process or file to watch. The watcher reads `~/.local/share/amp/secrets.json` for the API key.

### 5. Terminal viewport scraping (PTY pattern-matching)

Used by: ccmanager.

Pattern: ccmanager PTY-multiplexes the agent CLIs — it owns the terminal that the agent runs in. Reads the rendered terminal output and pattern-matches for spinner characters (`✱✲✳✴✵✶✷✸✹✺...`), "thinking..." activity labels, prompt boxes (`─────` border detection). Per-agent regex packs:

- Claude: spinner chars + `(9m 21s · ↓ 13.7k tokens)` token-stats line
- Gemini: different spinner set + different prompt box layout
- Codex, Cursor, Copilot, Cline, OpenCode, Kimi: each with their own visual conventions

This is the *most* portable mechanism — it works for any agent that renders to a terminal. It's also the *most fragile* because it breaks the moment any agent changes its visual presentation.

## The abstractions tools converged on

Despite four ingest mechanisms, every tool surveyed landed on essentially the same abstraction shape: a **per-agent adapter that produces canonical events for a unified consumer**. The interfaces differ in detail but agree in structure.

### opensessions: `AgentWatcher` (5 implementations)

```ts
// packages/runtime/src/contracts/agent-watcher.ts (paraphrased)

interface AgentWatcher {
  readonly name: string;
  start(ctx: AgentWatcherContext): void;
  stop(): void;
}

interface AgentWatcherContext {
  resolveSession(projectDir: string): string | null;
  resolveThreadOwner?(agent, threadId?, threadName?): AgentThreadOwner | null;
  emit(event: AgentEvent): void;
}

type AgentStatus =
  | 'idle' | 'running' | 'tool-running' | 'done'
  | 'error' | 'waiting' | 'interrupted' | 'stale';

interface AgentEvent {
  agent: string;        // 'amp' | 'claude-code' | 'codex' | 'opencode' | 'pi'
  session: string;      // mux session name
  status: AgentStatus;
  ts: number;
  threadId?: string;
  threadName?: string;
  paneId?: string;
  liveness?: 'alive' | 'exited' | 'unknown';
}
```

The five watcher files (`amp.ts`, `claude-code.ts`, `codex.ts`, `opencode.ts`, `pi.ts`) total **2,488 lines** — an average of ~500 lines per agent. The Codex watcher alone has ~80 lines of header documentation describing the new vs. old JSONL formats, event types, phase distinctions, and lifecycle flows before the code starts. This is the cost of supporting one agent properly.

State vocabulary: 8 values. Liveness is separate from status (alive/exited/unknown) so a session can be `done + alive` (waiting for next prompt) or `done + exited` (process gone).

### ccmanager: `StateDetector` (8 implementations)

```ts
// src/services/stateDetector/types.ts

interface StateDetector {
  detectState(terminal: Terminal, currentState: SessionState): SessionState;
  detectBackgroundTask(terminal: Terminal): number;
  detectTeamMembers(terminal: Terminal): number;
  hasTransientRenderFooter(terminal: Terminal): boolean;
}

type SessionState =
  | 'idle' | 'busy' | 'waiting_input' | 'pending_auto_approval';

type StateDetectionStrategy =
  | 'claude' | 'gemini' | 'codex' | 'cursor'
  | 'github-copilot' | 'cline' | 'opencode' | 'kimi';
```

Eight detector files, each implementing pattern-matching against the rendered terminal viewport. State vocabulary: **4 values** — much smaller than opensessions' 8. ccmanager doesn't need to distinguish `running` from `tool-running` because it's a session manager, not a per-tool visualizer.

The Claude detector alone has constants like `SPINNER_CHARS = '✱✲✳✴✵✶✷✸✹✺✻✼✽✾✿❀❁❂❃...'`, `TOKEN_STATS_LINE_PATTERN = /\([^)]*\d[^)]*tokens\s*\)/i`, and `IDLE_DEBOUNCE_MS = 1500` (workaround for Claude appearing idle while still processing). Per-agent fragility is built into the detector.

### agent-flow: `AgentSessionWatcher` (2 implementations)

```ts
// extension/src/session-runtime.ts

type AgentRuntimeMode = 'claude' | 'codex';

interface AgentSessionWatcher extends TypedDisposable {
  readonly onEvent: TypedEvent<AgentEvent>;
  readonly onSessionDetected: TypedEvent<string>;
  readonly onSessionLifecycle: TypedEvent<SessionLifecycleEvent>;
  start(): void;
  isActive(): boolean;
  isSessionActive(sessionId: string): boolean;
  getActiveSessions(): SessionInfo[];
  replaySessionStart(sessionIds?: string[]): void;
}

type AgentEventType =
  | 'agent_spawn' | 'agent_complete' | 'agent_idle'
  | 'message' | 'context_update' | 'model_detected'
  | 'tool_call_start' | 'tool_call_end'
  | 'subagent_dispatch' | 'subagent_return'
  | 'permission_requested' | 'error';

interface AgentEvent {
  time: number;
  type: AgentEventType;
  payload: Record<string, unknown>;
  sessionId?: string;
}
```

Cleaner separation: event stream (`onEvent`), session discovery (`onSessionDetected`), and lifecycle transitions (`onSessionLifecycle`) are three different channels. The event vocabulary is **richer** (12 types) and oriented toward graph visualization (`subagent_dispatch`/`subagent_return` are paired events that form edges in the orchestration graph).

The Claude runtime starts an HTTP hook server and writes to `~/.claude/settings.json`; the Codex runtime tails `~/.codex/sessions/**/rollout-*.jsonl`. Same `AgentEvent` shape comes out either way.

### AgentDeck: `AgentAdapter` + `AgentCapabilities` (4 implementations)

```ts
// shared/src/adapter.ts

type AgentType = 'claude-code' | 'openclaw' | 'codex-cli' | 'opencode' | 'monitor';

interface AgentCapabilities {
  type: AgentType;
  displayName: string;
  hasTerminal: boolean;          // PTY proxy
  hasModeSwitching: boolean;     // Plan/AcceptEdits/Default
  hasDiffReview: boolean;
  hasOptionLists: boolean;
  hasNavigablePrompts: boolean;
  hasSuggestedPrompts: boolean;
  hasApiUsage: boolean;
  hasModelCatalog: boolean;
}

interface AgentAdapter extends EventEmitter {
  readonly capabilities: AgentCapabilities;
  start(options): Promise<void>;
  handleCommand(cmd): boolean;
  writeInput(data: string): void;
  isAlive(): boolean;
  attachTerminal(stdin, stdout): void;
  getTtyPath(): string | undefined;
  getProjectName(): string | null;
  getHttpServer(): Server;
  shutdown(): Promise<void>;
}

type AdapterEvent =
  | { source: 'hook';       event: string; data: Record<string, unknown> }
  | { source: 'parser';     event: string; data?: Record<string, unknown> }
  | { source: 'metadata';   event: 'cursor_update' | 'usage_info' | ... }
  | { source: 'activity' }
  | { source: 'connection'; status: 'connected' | 'disconnected' }
  | { source: 'timeline';   entry: TimelineEntry; upsert?: boolean };
```

This is the most sophisticated abstraction surveyed. **AgentDeck doesn't pretend agents are equivalent** — it enumerates capabilities and presenters check `capabilities.hasModeSwitching` before rendering UI for it. Per-agent capability constants:

```ts
CLAUDE_CODE_CAPABILITIES: {
  type: 'claude-code',
  hasTerminal: true, hasModeSwitching: true, hasDiffReview: true,
  hasOptionLists: true, hasNavigablePrompts: true, hasSuggestedPrompts: true,
  hasApiUsage: true, hasModelCatalog: false,
}

OPENCLAW_CAPABILITIES: {
  type: 'openclaw',
  hasTerminal: false, hasModeSwitching: false, hasDiffReview: false,
  hasOptionLists: true, hasNavigablePrompts: false, hasSuggestedPrompts: false,
  hasApiUsage: false, hasModelCatalog: true,
}

CODEX_CLI_CAPABILITIES: {
  type: 'codex-cli',
  hasTerminal: true, hasModeSwitching: false, hasDiffReview: false,
  hasOptionLists: true, hasNavigablePrompts: false, hasSuggestedPrompts: false,
  hasApiUsage: false, hasModelCatalog: false,
}

OPENCODE_CAPABILITIES: {
  type: 'opencode',
  hasTerminal: true, hasModeSwitching: false, hasDiffReview: false,
  hasOptionLists: true, hasNavigablePrompts: false, hasSuggestedPrompts: false,
  hasApiUsage: false, hasModelCatalog: false,
}
```

The event union has six `source` discriminators (`hook`, `parser`, `metadata`, `activity`, `connection`, `timeline`), each with its own shape. The adapter is bidirectional: it both *emits* events and *handles commands* from the bridge (`select_option`, `navigate_option`, `interrupt`, `switch_mode`).

### Agent Sessions: per-agent Indexer + Parser

Not a runtime watcher — Agent Sessions is a session *browser*, so it reads-only. But the per-agent abstraction is the same. The `AgentSessions/Services/` directory has:

```
CursorSessionParser.swift, CursorSessionIndexer.swift
ClaudeSessionIndexer.swift
GeminiSessionParser.swift, GeminiSessionIndexer.swift
OpenClawSessionParser.swift
HermesSessionParser.swift
CopilotSessionParser.swift
DroidSessionParser.swift
UnifiedSessionIndexer.swift  ← composes all the above
```

Plus per-agent refresh-rate constants:

```swift
private static let focusedSessionRefreshIntervalsBySource: [SessionSource: ...] = [
  .codex:   (activeOnAC: 4,  activeOnBattery: 8,  inactiveOnAC: 20, inactiveOnBattery: 60),
  .claude:  (activeOnAC: 6,  activeOnBattery: 10, inactiveOnAC: 25, inactiveOnBattery: 60),
  .gemini:  defaultFocusedSessionRefreshIntervals,
  // ...
]
```

Different agents update at different paces, and the unifier knows it. Codex polls every 4s on AC power; Claude every 6s. This isn't optimization — it's accommodating actual behavior differences.

## What this means for the v2 design

Walking the v2 design through each of these tools' lenses:

### The provider abstraction needs at least three classes — confirmed

The previous analysis (`09-multi-agent-support.md`) sketched HookProvider / PluginProvider / TranscriptProvider. The surveyed tools collectively use **five**:

- HookProvider (config-installed shell commands) — agent-flow, AgentDeck
- TranscriptProvider (file-tail + JSONL parse) — opensessions, agent-flow, Agent Sessions
- SQLiteProvider (database polling) — opensessions for OpenCode
- CloudProvider (REST + WebSocket) — opensessions for Amp
- TerminalScrapeProvider (PTY pattern-matching) — ccmanager

A v1 substrate doesn't have to ship all five. But the interface should leave room for them. opensessions' `AgentWatcher` interface (start/stop, emits events to a context) is the right shape — it doesn't constrain *how* the watcher gets its data, only that it produces canonical events.

### Capabilities matter, and the v2 design ignored them

AgentDeck's `AgentCapabilities` matrix is a real correction to the design. Not all agents have permission prompts. Not all have mode switching. Not all expose model usage. A presenter that renders permission-approval UI on a wearable should check `hasOptionLists` for the source agent before showing anything.

**Proposed addition to v2:** alongside the per-agent reaction-mapping config, ship a per-agent capability config:

```yaml
# adapters/claude/capabilities.yaml
has_permission_payload: true
has_mode_switching: true
has_subagents: true            # Task tool with agent_type
has_statusline: true
has_token_telemetry: true      # in statusline + JSONL
has_context_telemetry: true    # contextPercentage in statusline
permission_payload_shape: claude-v1

# adapters/codex/capabilities.yaml
has_permission_payload: true
has_mode_switching: false
has_subagents: partial          # [agents] in config.toml, less mature
has_statusline: false           # uses notify on agent-turn-complete only
has_token_telemetry: true       # via JSONL token_count events
has_context_telemetry: false
permission_payload_shape: codex-v1

# adapters/aider/capabilities.yaml
has_permission_payload: false
has_mode_switching: false
has_subagents: false
has_statusline: false
has_token_telemetry: false
has_context_telemetry: false
```

Presenters consume capabilities alongside state. The capabilities surface lives next to the sessions API:

```
GET /sources                  -> [{ source, capabilities }, ...]
```

This is a small schema addition that codifies what's actually possible per agent without leaking presenter UI concerns into the daemon.

### The state vocabulary should be as small as possible — confirmed

ccmanager picked 4 states; opensessions picked 8. The v2 design currently has 11 (the OpenPets reaction enum). The novelty-tool survey's pets and lamps need the larger vocabulary for variety; ccmanager's session manager doesn't.

**The right move:** keep 11 as the canonical reaction enum, but acknowledge that not all sources can populate all 11. The reaction projection should fall back to a smaller subset when the source can't distinguish (e.g., tier-3 transcript-only agents collapse to `idle | working | success | error`). The session row carries the projection as-is; presenters consuming a non-Claude session see a smaller-but-valid value space.

### Per-agent translation costs ~500 lines of code and ~80 lines of documentation

opensessions' Codex watcher is 590 lines. Its Claude Code watcher is 507. agent-flow's Codex rollout parser is similar. ccmanager's Claude detector is 246 lines plus a regex pack.

**This is the realistic cost of adding an agent.** Shipping the substrate with Claude only is the right v1 call; "agent-agnostic" should mean the *interface* is right, not that the *implementation* exists for every agent. A contributor-PR pathway for new agents is the practical model.

### opensessions split `liveness` from `status` — worth adopting

Status answers "what is this agent doing?" Liveness answers "is the process still alive?" They're orthogonal:

| status | liveness | meaning |
|---|---|---|
| running | alive | Currently working |
| done | alive | Finished a turn, idle, waiting for input |
| done | exited | Session ended cleanly |
| stale | alive | Status says working but file hasn't grown — probably hung |
| stale | exited | Session crashed |
| error | alive | Errored but still recoverable |
| error | exited | Errored and gone |
| anything | unknown | No pane info, watcher-only |

The v2 design folds liveness into `lifecycle: live | paused | abandoned | ended`. That's close, but opensessions' three-way `alive | exited | unknown` is more honest about uncertainty. The substrate should keep the distinction explicit — `lifecycle` for the session, `liveness` for the attachment (which is closer to what `last_heartbeat_at` already represents).

### Refresh rates and timing are per-agent

Agent Sessions tunes refresh intervals per agent: Codex every 4s on AC, Claude every 6s. The v2 design doesn't model refresh at all — it assumes events are pushed. For sources where events *are* pushed (Claude hooks, Codex hooks, Gemini hooks), this is fine. For sources that have to be polled (OpenCode SQLite, Amp REST API, transcript-only agents), the daemon needs per-source poll-rate config:

```yaml
# adapters/opencode/runtime.yaml
poll_interval_active_ms: 2000
poll_interval_idle_ms: 30000

# adapters/amp/runtime.yaml
discovery_poll_interval_ms: 10000
# WebSocket per-thread = push, no polling

# adapters/claude/runtime.yaml
# push-only via hooks; no polling needed
```

This is per-adapter implementation detail, not part of the wire protocol, but the daemon should expose it as configuration rather than hard-coding.

### Bidirectionality is real for some surfaces (AgentDeck) but not necessary as a core concept

AgentDeck's `AgentAdapter` is bidirectional — it emits events *and* accepts commands. The v2 design is explicitly one-way (no HITL). This holds. But the abstraction shape — an adapter that owns the connection in both directions — is what AgentDeck did to ship hardware approval UIs.

The v2 design's response: HITL is a presenter concern. AgentDeck's bridge is the LAN-reachable presenter. The substrate provides the read-only event stream; AgentDeck-class tools build their own bidirectional bridge on top. AgentDeck doesn't need the substrate to be bidirectional; it just needs to know what's happening, and have its own back-channel to the agent (which it does, via PTY proxy).

## Patterns the surveyed tools didn't solve

A few things even the most ambitious multi-agent tools didn't crack:

### Cross-agent identity correlation

If a user runs Claude on `main` and Codex on `worktree/feature-x` for the same task, no tool surveyed groups them. opensessions groups by tmux session; AgentDeck groups by attached PTY. Neither knows "these two sessions are working on related branches of the same repo."

The v2 design's `repo_root` + `branch` derivation actually gets this for free — `GET /sessions?repo_root=...` returns both. The substrate already does what the surveyed tools don't.

### Cross-agent event ordering

When five agents fire events at near-simultaneously, no surveyed tool orders them correctly across sources. opensessions emits per-watcher with the watcher's own clock; AgentDeck does the same. The v2 design's monotonic `event_id` counter (assigned by the daemon at ingest) gives a total order across all sources. Small but real win.

### Tool-name vocabularies aren't normalized

opensessions doesn't surface tool names at all — its event vocabulary is status-only. agent-flow does, but uses each agent's native names (Claude's `Bash`, Codex's `shell_command`). AgentDeck normalizes for parser events but not for hooks. Agent Sessions preserves verbatim for browsing.

**The right substrate behavior:** preserve verbatim in `payload.tool_name`. Maintain a per-adapter mapping to the canonical reaction enum (`editing`, `running`, etc.) for the `current_state` projection. Don't try to invent a cross-agent tool-name namespace; nobody else has.

### Subagent semantics differ in ways no tool normalizes

Claude Code subagents are synchronous and flat. Cursor subagents are async and recursive. OpenCode has primary-vs-subagent. agent-flow's `subagent_dispatch`/`subagent_return` paired events work for Claude but lose information for Cursor's parallel-tree case.

The v2 design's `parent_agent_id` pointer handles Claude and Cursor (recursion is just walking the chain). OpenCode's primary-vs-subagent is partially lost — both become "agent" rows with `agent_type` distinguishing them. This is fine as long as `agent_type` is preserved faithfully (which v2 already does).

## What no tool surveyed shipped

A handful of capabilities the v2 design or extensions could provide that none of the surveyed tools have:

- A genuinely uniform `current_state` projection across all five adapters (each tool either has a different vocabulary per agent, or normalizes only loosely)
- A capability negotiation surface (only AgentDeck has this, and only partially)
- A single subscriber that gets events from all sources without re-implementing the per-agent ingestion (every consumer of opensessions/agent-flow/AgentDeck has to integrate with that specific tool, not a generic substrate)
- Cross-source query support (`give me all sessions in this repo, regardless of which agent`)

The substrate's pitch sharpens: it's not "another multi-agent visualizer." It's the *data layer* that the existing multi-agent tools could consume from, eliminating their per-agent watcher code in favor of a shared one.

## Concrete additions for v2.1

From the survey:

1. **Three-class provider abstraction**, but enumerate five concrete types in documentation: HookProvider, PluginProvider, TranscriptProvider, SQLiteProvider, CloudProvider, TerminalScrapeProvider. Ship one in v1 (HookProvider for Claude); document the rest as contributor pathways.

2. **`AgentCapabilities` per source**, exposed via `GET /sources`. Lets presenters check before rendering UI. Lift from AgentDeck's matrix.

3. **Liveness separate from lifecycle.** Borrow opensessions' `alive | exited | unknown` for attachments. Lifecycle stays on the session.

4. **Per-adapter refresh rates as config**, not hard-coded. Match Agent Sessions' active/idle/AC/battery tuning if needed.

5. **The reaction enum projection has a per-adapter mapping table.** Tier-3 (transcript-only) adapters collapse to a smaller subset. Document this; ship the YAML.

6. **Cross-source ordering is the daemon's job** via monotonic `event_id`. The surveyed tools don't do this; the substrate can.

7. **Sources are first-class** in the schema. `(source, session_id)` as the natural key. Already in v2; reinforce with the multi-agent evidence.

The corrections are small. The bigger point is that the v2 design's abstraction is *correct in shape* — the surveyed tools converged on essentially the same pattern. What v2 needs to do is ship the abstraction cleanly enough that other tools can adopt it as their data layer instead of writing their own.