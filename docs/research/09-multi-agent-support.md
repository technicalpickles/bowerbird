# Multi-agent support: convergence, gaps, and challenges

The design has casually claimed to be "agent-agnostic" without examining what that means in practice. This document looks at what each major coding-agent CLI actually exposes — hooks, transcripts, session models, subagent concepts — and where the substrate's abstraction holds vs. where it breaks.

The headline finding: **Claude Code's hook model has become the de-facto standard.** Codex, Gemini CLI, and Cursor CLI all converged on JSON-over-stdin command hooks with matchers and exit-code semantics. Gemini CLI even ships a `CLAUDE_PROJECT_DIR` alias for compatibility. So "multi-agent support" is much less of a stretch than the v1 design implied — at least for the agents that adopted the convention. The agents that didn't (OpenCode, Aider, Copilot) are where the abstraction has to work harder.

## Per-agent breakdown

### Claude Code (Anthropic)

The reference implementation. Settings-installed shell command hooks, ~12 lifecycle events, JSON-over-stdin payload, exit-code semantics for blocking. JSONL transcripts at `~/.claude/projects/<encoded-cwd>/<session-id>.jsonl`. Native `subagent_type` and `agent_id` in hook payloads. Statusline command for per-turn JSON. Built-in OpenTelemetry.

Already exhaustively documented in `02-detailed-inventory.md`. This is what the v2 design targets.

### Codex CLI (OpenAI)

Hooks are **explicitly Claude-style**. The DeepWiki page on Codex's hook system literally calls it "a Claude-style engine." Config lives in `~/.codex/config.toml` (TOML inline tables) or `hooks.json` files next to active config layers, with project/user/system precedence. Hook events:

- `SessionStart`, `UserPromptSubmit`, `Stop` — stable as of v0.124.0
- `PreToolUse`, `PostToolUse` — proposed and being shipped (issue #14882, March 2026; landed in v0.130 May 2026)
- `PermissionRequest`

Same `matcher` regex, same JSON-over-stdin contract, same `continue`/`stopReason`/`systemMessage` output schema. The only material differences:

- TOML config instead of JSON
- Tool name vocabulary differs (`shell_command`, `apply_patch`, `update_plan`, `unified_exec`, `view_image`)
- Built-in OpenTelemetry with `gen_ai.*` span conventions
- `--json` flag on `codex exec` emits NDJSON of `thread.started`, `turn.started`, `item.started`, `item.completed`, `turn.completed`, `turn.failed`
- Subagents exist (`[agents]` in config.toml) but the surface is less mature
- Sessions stored as JSONL rollouts at `~/.codex/sessions/<id>/rollout-*.jsonl` (different path, similar shape)
- Plugins can bundle hooks via `plugin manifest` or `hooks/hooks.json`
- Hooks are *disabled* during Guardian review sessions (v0.121.0)
- `notify` config triggers an external program on `agent-turn-complete` (less granular than hooks, used for desktop toasts)

**Substrate fit: excellent.** A Codex adapter is mostly the same shim with TOML config writing and a tool-name table. The `payload`-passthrough principle from v2 handles the vocabulary differences automatically.

### Gemini CLI (Google)

**Also explicitly Claude-style**, with a stated migration goal. The official Google blog post from January 2026 introducing hooks describes them as "middleware for your AI assistant." Issue #9070 from September 2025 says outright: *"It mirrors the JSON-over-stdin contract, exit code semantics and matcher syntax used by Claude Code."*

The compatibility goes further than Codex:

- `~/.gemini/settings.json` or `.gemini/settings.json` — JSON config like Claude
- Environment variables include `CLAUDE_PROJECT_DIR` as an **explicit alias** for `GEMINI_PROJECT_DIR`
- Hook events: `SessionStart`, `UserPromptSubmit`, `BeforeTool`, `AfterTool`, `Notification`, `Stop`, `PreCompress`
- Same matcher syntax (regex for tools, exact for lifecycle, `*` or `""` for all)
- Same JSON-over-stdin stdin payload shape (`session_id`, `transcript_path`, `cwd`, `hook_event_name`, `timestamp`)
- Same output schema (`continue`, `stopReason`, `systemMessage`, `suppressOutput`)
- Same exit-code semantics
- **`gemini hooks migrate --from-claude`** is a planned CLI command for converting Claude Code hooks

The main differences:

- Event name conventions: `BeforeTool`/`AfterTool` instead of `PreToolUse`/`PostToolUse`
- Additional advanced events: `BeforeModel`, `BeforeToolSelection` (intercept/replace LLM request, adjust candidate tool list)
- Stronger OpenTelemetry integration (export to GCP Cloud Trace, Jaeger, Prometheus, Datadog)
- Two hook types: `command` (shell) and `plugin` (npm package, `geminicli-plugin` tag)
- Project-hook fingerprinting: if a hook's name or command changes via `git pull`, it's treated as new and untrusted

**Substrate fit: excellent.** A Gemini adapter is essentially a Claude adapter with a small event-name translation table. The shim layer is identical.

### Cursor CLI

Has shipped hooks, but in fits and starts. The current state:

- Hooks live in `~/.cursor/hooks.json`
- Events: `sessionStart`, `sessionEnd`, `beforeSubmitPrompt`, `stop`, `PreToolUse`, `PostToolUse` (and others added incrementally)
- Cursor v2.4 changelog (March 2026): *"Added compatibility with Claude Code hooks in CLI"*
- Subagents exist in `.cursor/agents/<name>.md` (same Markdown + YAML frontmatter pattern as Claude Code), can run **asynchronously and recursively** (subagents spawn subagents — newer than Claude Code's model)
- Sessions saved as JSONL transcripts (added recently, applies to headless mode too)
- Plugins package skills + subagents + MCP + hooks + rules in one install

**Substrate fit: good but unstable.** Cursor's hook surface keeps changing. The shape is convergent but the event names and field locations have drifted across versions. An adapter needs version-awareness.

### Aider

**No native hook system.** Aider was designed before hooks became standard. AiderDesk (the GUI fork by hotovo) ships 30+ lifecycle hooks (`onTaskCreated`, `onPromptFinished`, `onToolCalled`, `onFileAdded`, etc.) but those are React/TypeScript callbacks inside the desktop app, not external shell command hooks.

Aider's observable surface:

- Chat history files in `.aider.input.history` and `.aider.chat.history.md`
- No JSONL transcript format
- No subagent concept (Aider is a single-agent system)
- No statusline interface
- Stdout/stderr only, intended for direct terminal use

**Substrate fit: poor.** An Aider adapter would have to be **transcript-only**, tailing `.aider.chat.history.md` and parsing markdown-formatted turns. State signals are coarse — you can tell *whether* Aider is in a turn but not *what tool* it's invoking, because Aider's "tools" are deeply integrated (apply diffs, run tests, ask user) rather than dispatched as separate calls.

### OpenCode

Hooks exist but the model is **fundamentally different**: plugin-as-code, not hook-as-config.

- Plugins are TypeScript packages (`@opencode-ai/plugin`) loaded from `opencode.json` config
- Plugin export shape: `{ event: async ({ event }) => {...} }`
- Events: `session.created`, `session.idle`, `session.error`, `tool.execute.before`, `tool.execute.after`, `chat.message`, `experimental.chat.messages.transform`, plus more
- No JSON-over-stdin contract — plugins run in-process in the OpenCode node runtime
- Sessions stored differently (less standardized path)
- Subagents (primary/subagent dichotomy) are first-class with two built-in primary agents (Build, Plan) and two built-in subagents (General, Explore)
- Two competing third-party hook-config systems: `KristjanPikhof/OpenCode-Hooks` adds Claude-style YAML hooks on top, `oh-my-opencode` ships 52 hooks across four tiers

**Substrate fit: requires a different ingest path.** An OpenCode adapter would be a plugin, not a shim. The daemon would either:
- Ship an OpenCode plugin that pushes the same `AgentEvent` shape over HTTP to the daemon, or
- Tail OpenCode's session storage as a file-based provider

The first is more reliable; the second is what `opencode-observability` does today, copying disler's pattern.

### Copilot CLI (GitHub)

Less public information available, but per the Agent Sessions tool (which supports 7 agents) Copilot CLI does have **JSONL session storage**. The hook surface is undocumented publicly and probably nonexistent. Substrate fit: file-tail only, like Aider.

### OpenClaw

Has its own hook system (`types.hooks.ts`) with event-based extensibility. Proposed extension would add 10 lifecycle hook phases (5 pre-action, 5 post-action: `preRequest`, `preRecall`, `preResponse`, `preToolExecution`, `preCompaction`, `postRequest`, `postResponse`, `postToolExecution`, `postCompaction`, `onError`). Plugin model is registered-provider-based. Substrate fit: similar to OpenCode — plugin layer required.

## The convergence pattern

Looking across the seven major coding-agent CLIs, the picture is:

| Agent | Hook model | Convention compatibility | Subagent concept | JSONL transcripts | Statusline |
|---|---|---|---|---|---|
| Claude Code | Shell commands in `settings.json` | Reference | `.claude/agents/*.md` + `subagent_type` | `~/.claude/projects/.../*.jsonl` | Yes |
| Codex CLI | Shell commands in `config.toml` or `hooks.json` | Claude-style by design | `[agents]` in config.toml | `~/.codex/sessions/.../rollout-*.jsonl` | No (uses `notify`) |
| Gemini CLI | Shell commands in `settings.json` | Claude-style by design, `CLAUDE_PROJECT_DIR` alias | Less prominent | Yes (path varies) | No |
| Cursor CLI | Shell commands in `~/.cursor/hooks.json` | Recently added Claude compat | `.cursor/agents/*.md` (recursive, async) | Yes (recent) | No |
| OpenCode | TypeScript plugins | Different abstraction; community Claude-compat layers exist | Built-in Build/Plan/General/Explore | Yes (different path) | No |
| Aider | None | N/A | None | Markdown only | No |
| Copilot CLI | None public | N/A | Unknown | Yes | No |
| OpenClaw | TypeScript plugins | Different abstraction | Unknown | Unknown | No |

**Three "tiers" of substrate compatibility emerge:**

**Tier 1 — Shell-hook compatible.** Claude Code, Codex, Gemini CLI, Cursor CLI. The same shim binary handles all four with small per-agent translation tables (event-name aliases, config file format, env var names). 80% drop-in.

**Tier 2 — Plugin-required.** OpenCode, OpenClaw. The daemon ships an in-process plugin that converts native events to the canonical `AgentEvent` shape and POSTs to the daemon. More work per-agent, but the daemon's wire protocol is the same.

**Tier 3 — Transcript-only.** Aider, Copilot CLI (probably). Pure file tailing. State signals are coarser — start/stop/idle but not necessarily per-tool. The `current_state` projection degrades gracefully (likely just `idle` ↔ `working`).

## What the substrate has to do for each tier

### For tier 1 (Claude / Codex / Gemini / Cursor)

The shim handles four flavors of the same conversation. Per-agent config:

```yaml
# bowerbird/adapters/<agent>.yaml
agent_name: codex
config_path: ~/.codex/config.toml
config_format: toml-inline-tables
event_aliases:
  PreToolUse: PreToolUse        # codex uses same name
  PostToolUse: PostToolUse
  SessionStart: SessionStart
  Stop: Stop
env_var: CODEX_PROJECT_DIR
transcript_dir: ~/.codex/sessions
transcript_pattern: "{session_id}/rollout-*.jsonl"
tool_name_format: snake_case   # codex uses shell_command not Bash
```

A new agent CLI joins by dropping a yaml file. The hook payload normalization in the shim reads it, translates event names, and POSTs the same canonical envelope to the daemon.

### For tier 2 (OpenCode / OpenClaw)

Ship a small plugin package:

```ts
// @bowerbird/opencode-plugin
export default async ({ app, client }) => ({
  event: async ({ event }) => {
    await fetch('http://127.0.0.1:9876/events', {
      method: 'POST',
      body: JSON.stringify({
        source: 'opencode',
        kind: translateEventKind(event.type),
        payload: event,    // passthrough, like the v2 hook shim
      })
    });
  }
});
```

The wire protocol is identical to what the hook shim sends. From the daemon's perspective, an OpenCode event is just another `source: 'opencode'` row in the events table.

### For tier 3 (Aider / Copilot CLI)

A tail-only provider runs alongside the daemon, watching the relevant file:

- Aider: watches `.aider.chat.history.md` in the cwd, parses turn boundaries
- Copilot CLI: watches its JSONL session directory

The provider emits coarser events (`userTurn`, `turnEnd`, sometimes `toolStart` if the format is parseable). The daemon's `current_state` projection falls back to a smaller vocabulary: `idle | working | error`. Pets and lamps still work, just with less variety.

## Gaps that prevent honest multi-agent support today

These are the places where "agent-agnostic" needs more than waving the hand.

### Gap 1 — Subagent concepts differ in nontrivial ways

Claude Code: declarative file (`.claude/agents/<name>.md`), explicit `Task` tool, synchronous (subagents can't spawn further subagents), `agent_type` and `agent_id` in hook payload.

Codex CLI: `[agents]` config table, less mature surface, exact subagent invocation mechanism in flux.

Cursor CLI: declarative file (`.cursor/agents/<name>.md`), but **async** by default, **recursive** (subagents spawn their own subagents). Different lifecycle.

OpenCode: primary agents (`Build`, `Plan`) vs. subagents (`General`, `Explore`). Tab-switching between primaries. The "main vs subagent" distinction doesn't map onto Claude's "main + Task-spawned subagent" model directly.

Aider/Copilot: no subagent concept.

The v2 design's `agents` table has `parent_agent_id` and `type: main | subagent | teammate`. For Cursor's recursive subagents this works (parent pointer up the chain). For OpenCode's primary/subagent split, `type=main` for primaries is a reasonable mapping but loses the "Build vs. Plan" distinction.

**The substrate's response should be:** keep `agent_type` (preserves OpenCode's `Build`/`Plan` distinction, Claude's subagent name, Codex's `[agents]` entry name) and `parent_agent_id` as the two normalized concepts. Don't try to normalize "primary vs subagent" further — let presenters key on `agent_type`.

### Gap 2 — Tool-name vocabularies are completely different

Claude Code: `Edit`, `Write`, `MultiEdit`, `Bash`, `Read`, `Glob`, `Grep`, `Task`, `WebFetch`, `WebSearch`, etc.

Codex CLI: `shell_command`, `apply_patch`, `update_plan`, `unified_exec`, `view_image`, MCP tools as `mcp__<server>__<tool>`.

Gemini CLI: `read_file`, `write_file`, `replace`, `run_shell_command`, MCP tools as `mcp_<server>_<tool>`.

Cursor CLI: largely Claude-compatible (the changelog calls out compatibility).

OpenCode: tool names from its plugin tool registry; varies by plugin.

The `current_state` reaction enum (which maps tool names to `editing`/`running`/`testing`) has to know about all of these. The v2 design said this normalization was the *only* thing the daemon does on top of passthrough, and that justification holds. But the daemon needs a per-agent tool-name → reaction table:

```yaml
# canonical reaction projection
adapters/claude/tool-reactions.yaml:
  Edit: editing
  Write: editing
  MultiEdit: editing
  Bash: running
  Task: working
adapters/codex/tool-reactions.yaml:
  apply_patch: editing
  shell_command: running
  unified_exec: running
adapters/gemini/tool-reactions.yaml:
  write_file: editing
  replace: editing
  run_shell_command: running
```

This is ~50 lines of YAML per agent. Trivial. But it does mean the "11-value reaction enum" projection isn't free across agents — it requires per-adapter mapping.

### Gap 3 — Permission model semantics differ

Claude Code: `PermissionRequest` hook event with structured payload (question, options for `AskUserQuestion`, tool details).

Codex CLI: `PermissionRequest` event exists; payload less documented; sandbox-policy-driven auto-allow rules.

Gemini CLI: confirmation is built into the streaming UI (`WaitingForConfirmation` state); hook intercept is `BeforeTool` with `decision: deny` output to block.

Cursor CLI: approval modes (Run Everything, Auto-Run in Sandbox, Ask Every Time) — more configurable than a hook payload.

OpenCode: per-agent `permissions` config in `opencode.json`; pre-execute hooks can block.

The v2 design has `permissionRequest` as a first-class event with the full native payload. This works for Claude and Codex. For Gemini, the equivalent fires as `BeforeTool` with metadata flagging confirmation required. For Cursor, the approval is handled by Cursor's own flow and may not surface as an event the daemon sees.

**The substrate's response:** `permissionRequest` is a normalized kind, but the *fact that confirmation is needed* is the only canonical bit; the rest rides in `payload`. Presenters that want to render hardware approval prompts read `payload`; the data shape will be agent-specific. This is fine — the daemon does its job (preserves native data) and presenters that target multi-agent need a per-agent renderer.

### Gap 4 — Session identity isn't portable

Claude Code session ID: UUID in `session.jsonl` filename.

Codex CLI session ID: UUID in `~/.codex/sessions/<id>/`.

Cursor CLI session ID: format different, exposed via headless mode.

OpenCode session ID: different again.

No two agents use the same ID scheme. The daemon's `session_id` is already namespaced per-source (a hook event carries the originating agent's session ID, and the daemon stores it). The hidden assumption was that `session_id` is the natural key; **it isn't** — it's only unique per agent. If a presenter wants to query "all sessions across all agents," the key is `(source, session_id)`, not `session_id` alone.

**The substrate's response:** sessions are keyed on `(source, session_id)` internally. Externally, the `session_id` field in API responses is the agent-native ID; the `source` field qualifies it. This is already shaped correctly in the schema; just call it out explicitly.

### Gap 5 — Process attribution is harder across agents

The terminal/multiplexer attribution work in v2 assumes a single agent process. Multi-agent reality:

- A user can run Claude Code in one terminal and Codex in another simultaneously, in the same tmux session
- agent-flow already visualizes both side-by-side
- AgentDeck renders creature sprites for Claude, Codex, OpenCode, OpenClaw on one Stream Deck

`AttachmentLocation` already includes `host` and `terminal.session_id`. Adding `source` to the attachment row is the only schema change needed to disambiguate. The daemon then knows: "this iTerm session has one Claude attachment + one Codex attachment running in two different panes."

### Gap 6 — Hook timing semantics

Claude Code hooks are synchronous — the hook blocks the agent until it exits. The v2 shim exits in <5ms to avoid being a slowdown.

Gemini CLI says explicitly: *"Hooks run synchronously as part of the agent loop—when a hook event fires, Gemini CLI waits for all matching hooks to complete before continuing."*

Codex: same, but Guardian-review sessions skip hooks entirely.

Cursor: *"Hook commands now start 40x faster"* in a recent changelog — hints that hook overhead matters and is being optimized.

OpenCode plugins: in-process, no separate exit — synchronous within the plugin runtime.

All four converged on synchronous blocking semantics. **The shim has to remain fast** — sub-5ms is the design target, and it's not just a Claude Code thing.

### Gap 7 — Authoritative vs. observational hooks

Most agents' hook contracts let a hook *block* an operation (Claude's exit-code-2, Gemini's `decision: deny`, Codex's `continue: false`). The v2 design declared hook blocking out of scope — the unified shim always exits 0.

This is fine for observability but means a tool that wants to enforce policy (block dangerous Bash commands across all agents the user runs) still has to install its own hook *in addition to* the unified one. The collision is back, but only for the policy-enforcement use case.

There's no clean way around this without making the shim synchronous and stateful (waits for a registered policy subscriber to weigh in), which immediately blows past the failsafe budget. v2 keeps this as out-of-scope; policy enforcement remains per-agent and parallel to the daemon.

### Gap 8 — Statusline coverage is uneven

Only Claude Code has a true statusline command interface. Codex uses `notify` for `agent-turn-complete` only (much coarser). Gemini, Cursor, OpenCode have no equivalent — they handle UI internally in their TUI.

The v2 design uses statusline as one of three heartbeat sources. For non-Claude agents, heartbeats come *only* from hooks. That's fine — every tool call is a heartbeat — but it means context-percentage and token usage are Claude-specific data the daemon will have on Claude sessions and lack on others.

**The substrate's response:** the `context_percentage` and other statusline-only fields are flagged as nullable in the session row. Presenters that build HP-bars for context degrade to "unknown" for non-Claude sessions, or use a different signal (token count proportional to model max-context, if model is known).

### Gap 9 — Agent IDs don't reliably correlate across the stack

When Claude Code spawns a subagent via `Task`, the parent and child are linked by `parent_agent_id`. When Cursor CLI spawns subagents asynchronously and recursively, the linkage is the same shape but the spawning mechanism is different (and async).

When a user runs **two different agents on the same task** — e.g., Claude Code on the main branch and Codex CLI on a worktree to compare implementations — these have no relationship in any agent's hook event stream. They're independent sessions that happen to share `repo_root`.

The v2 schema already handles this: filter `GET /sessions?repo_root=...` returns both, grouped on the daemon side. But there's no "this is a comparison run" concept, and there shouldn't be in the daemon — that's a presenter or orchestrator concern.

### Gap 10 — Agent-to-agent communication

The "Agent Teams" feature in Claude Code (experimental, `CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS=1`) has agents sending messages to each other via `SendMessage` and shared inboxes at `~/.claude/teams/<team>/inboxes/<agent>.json`. This was discussed in `02-detailed-inventory.md`.

OpenCode and Cursor have their own forms of agent-to-agent coordination (OpenCode's primary-with-subagent calls, Cursor's recursive subagents) but the data lives in their own state, not in a shared inbox file.

**The substrate's response:** the v2 design dropped the `agentMessage` event. For Claude Code's Agent Teams specifically, a teams-aware adapter could watch `~/.claude/teams/*/inboxes/*` and emit a `kind: 'message'` event with `payload: { from, to, content }`. This is Claude-specific; other agents would surface message-like content inside their existing event payloads (Cursor) or not at all (Codex, Gemini).

## How the substrate's claim of "agent-agnostic" actually holds up

The honest summary:

**Tier 1 (Claude, Codex, Gemini, Cursor): the substrate's design holds well.** The hook-router shim is one binary with a config table per agent. The event/state model genuinely transfers. The Codex adapter sketch the v2 design promised is real and easy.

**Tier 2 (OpenCode, OpenClaw): different ingest model, same wire protocol.** A daemon-shipped plugin pushes the same events the shim would. More work to ship the plugin; same daemon-side schema.

**Tier 3 (Aider, Copilot): file-tail-only.** Coarser state, smaller event vocabulary, narrower reaction enum. Pets and lamps still work; sophisticated dashboards (per-tool timing, permission rendering) don't.

The "agent-agnostic" claim is **honest for tier 1, plausible for tier 2, partial for tier 3.** Worth saying so in the design rather than waving.

## Specific corrections to the v2 design

These come out of the multi-agent analysis:

1. **Sessions are keyed `(source, session_id)`**, not `session_id` alone. Mostly already shaped this way; call it out.

2. **`AttachmentLocation` adds `source`** to disambiguate when a user runs Claude + Codex + Gemini side-by-side.

3. **The reaction-enum projection requires a per-adapter tool-name → reaction table.** Not free across agents. List it as a per-adapter requirement.

4. **Statusline-only fields (`context_percentage`, token rollups derived from statusline)** are nullable and Claude-specific in practice. Note in the design that non-Claude sessions won't populate them.

5. **The provider abstraction has three classes, not one:**
   - HookProvider (Claude, Codex, Gemini, Cursor): shim-based, settings-file install
   - PluginProvider (OpenCode, OpenClaw): in-process plugin pushes events
   - TranscriptProvider (Aider, Copilot, possibly others): file-tail only, coarse vocabulary

   Pixel Agents' README anticipated exactly this with its "FileProvider and StreamProvider will be added alongside the first real second provider" note.

6. **The "agent-agnostic" claim should specify *what's preserved across agents*:**
   - Reliable: session start/end, tool start/end, current state enum, agent_type-based routing
   - Best-effort: permission payload shape (varies)
   - Claude-only: statusline-derived data, Agent Teams inbox messages

7. **`agent_type` is universal but the values are agent-specific.** Claude's `code-reviewer`, Codex's `[agents]` entry name, Cursor's `.cursor/agents/<name>.md`, OpenCode's `Build`/`Plan` are all valid `agent_type` values. Presenters that key on `agent_type` for voice/sprite/color need to register mappings per (source, agent_type) pair.

## What this leaves unanswered

A few questions the multi-agent analysis surfaces but doesn't settle:

1. **Should the daemon ship adapters for non-Claude agents in v1?** The argument for *yes*: validates the abstraction; lets the daemon claim "agent-agnostic" honestly. Against: adds maintenance surface and risks tying release cadence to other agents' API stability. Probably ship Claude in v1, document the abstraction precisely enough that a Codex adapter is a contributor PR.

2. **What's the responsibility split for "two agents on the same task"?** Worktrees + branches already group them. Anything richer (compare runs, swap models) is orchestrator concern, not daemon concern.

3. **How does the daemon handle agents it has never seen?** A `(source, ...)` row arrives with `source: 'gpt-engineer'`. Without an adapter config, the daemon stores the events but can't project to the reaction enum or normalize hook event names. Behavior should be: store with `current_state: 'unknown'`, emit events with `source: <unknown>` and raw payloads. Forward-compatible.

The point of this detour was to stress-test the design against the actual diversity of coding agents. Result: the design holds, but the "agent-agnostic" framing should be sharpened to "tier-1 native, tier-2 via plugin, tier-3 via file-tail." The single biggest architectural correction is that the substrate ships *three* provider types, not one; everything else is per-adapter table data.