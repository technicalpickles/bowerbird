# Claude Code Agent State & Visualization Tools — Inventory & Findings

## Motivation

The Claude Code ecosystem has grown a large number of tools that observe, visualize, or react to agent activity. Almost all of them tap into the same handful of data sources (hooks, JSONL transcripts, statusline) and reimplement the same plumbing — which means they collide when run together (every dashboard wants its own `PreToolUse` hook in `~/.claude/settings.json`) and switching between them is painful.

The hypothesis behind this inventory: there is a generalizable layer underneath all of these — a state model + event stream — and most of these tools could be rebuilt as thin presenters on top of it.

This document is the first-pass inventory. A second pass will dig into each tool's repo to characterize what state it tracks, what events it emits, and how it's installed/configured.

## Data sources currently in use

Every tool in this space pulls from one (or a small combination) of these:

1. **Hooks** — `~/.claude/settings.json` entries that run a script on `PreToolUse`, `PostToolUse`, `SessionStart`, `SessionEnd`, `Stop`, `SubagentStart`, `SubagentStop`, `UserPromptSubmit`, `Notification`, `PreCompact`. Push-based. Most common approach. Collides when multiple tools want the same hook.
2. **JSONL transcripts** at `~/.claude/projects/<encoded-path>/<session-id>.jsonl`. Pull-based, polled. The only source of full conversation content. Used when hooks are too coarse.
3. **Statusline command** — Claude Code calls a configured command on each turn with session JSON on stdin. Pull-based, deterministic cadence, but only one consumer slot.
4. **OpenTelemetry** — Claude Code has built-in OTel export. Underused outside enterprise.
5. **MCP** — agent calls tools defined by the observer. Pull-based from the observer's perspective; the agent self-reports.
6. **Terminal escape sequences** — OSC 9/99/777 for notifications. Used by terminal-level tools (cmux).

## Categories

### Hook-driven dashboards (web UI, observability-style)

- **disler/claude-code-hooks-multi-agent-observability** — the canonical reference; Bun/TS server, swim-lane filtering, live pulse chart, SQLite WAL.
- **hoangsonww/Claude-Code-Agent-Monitor** — Node/Express/React/SQLite/WebSockets, Kanban board, sessions list, analytics, "Run Claude" live stream, separate VS Code extension and plugin marketplace bundles.
- **simple10/agents-observe** — Docker-based plugin, `/observe` slash commands.
- **mukul975/claude-team-dashboard** — D3 force-directed graph of inter-agent comms, task dependency graph, watches `~/.claude/teams/`.
- **nexus-labs-automation/agent-observability** — guidance/instrumentation skill pack.
- **Marc Nuri's coding agent dashboard** — heartbeat model, enricher pattern, WebSocket terminal relay (blog-post architecture, not packaged).

### Cloud / OTel-based observability

- **Dynatrace Claude Code Monitoring** — uses Claude Code's built-in OTel export. Cost, tokens, sessions, tool activity, reliability signals.
- **Anthropic Managed Agents dashboard** — first-party control plane (different scope: managed agents API, not local CLI).

### Statusline pets / single-session companions

- **Ido-Levi/claude-code-tamagotchi** — statusline pet + violation detection via `PreToolUse` hook and Groq LLM analysis of transcript.
- **terryso/ccpet** — simpler statusline pet, energy from token consumption, global leaderboard.
- **Anthropic /buddy** (Claude Buddy) — first-party, deterministic per-user generation, "bones vs soul" architecture, observes conversations in-process.

### Game / character-based visualizations

- **pablodelucca/pixel-agents** — VS Code extension, pixel-art office. Reads JSONL transcripts directly. **Explicit architectural goal of being agent-agnostic and platform-agnostic.**
- **alvinunreal/openpets** — desktop app, **MCP-based** integration. Defines state vocabulary (`thinking`, `working`, `editing`, `running`, `testing`, `waiting`, `success`, `error`) exposed as MCP tools. Closest to a generalized state API.
- **DaniloTrebjesanin/claude-pixel-quest** — VS Code extension, agents as pixel characters mining/fishing/chopping.

### Terminal / workspace orchestrators (state visible as a side effect)

- **manaflow-ai/cmux** — native macOS terminal on Ghostty. OSC 9/99/777 + `cmux notify` CLI. Sidebar shows git branch, PR status, listening ports, latest notification.
- **craigsc/cmux** — separate project, "tmux for Claude Code," worktree wrapper.
- **kbwo/ccmanager** — TUI, multi-agent (Claude/Gemini/Codex/Cursor/Copilot/Cline/OpenCode/Kimi).
- **smtg-ai/claude-squad** — tmux + worktrees.
- **BloopAI/vibe-kanban** — kanban, multi-agent (now community-maintained).
- Variants on the kanban/worktree theme: **Conductor**, **Crystal**, **Nimbalyst**, **Opcode**, **dmux**, **VibeTree**, **agentree**, **Code Conductor**.

### HUD / statusline-only

- **Claude HUD** — pure statusline, native token data via Claude Code's statusline API + transcript parsing. No network or background process.
- **melodic-software/claude-code-observability** — logs 14 hook events to JSONL, queries via subcommands.

## Observations

### 1. The same data, reimplemented N times

Almost every dashboard installs the same hook handlers, parses the same JSON payloads, and emits the same logical events (tool started, tool finished, agent waiting, session ended). The differences are almost entirely in the presentation layer.

### 2. Three data sources dominate; one is interesting

- **Hooks** are dominant.
- **JSONL transcripts** are used when hooks are too coarse (Pixel Agents, Tamagotchi for context analysis).
- **MCP** is the outlier — only OpenPets uses it — and it's the most aligned with a generalized "state API" framing because the agent self-reports against a defined vocabulary.

### 3. Pixel Agents and OpenPets already articulate the problem

Pixel Agents' README explicitly says it aims to be "Agent-agnostic: Claude Code today, but built to support Codex, OpenCode, Gemini, Cursor, Copilot, and others through composable adapters."

OpenPets goes further: it defines a small state vocabulary (`thinking`, `working`, `editing`, `running`, `testing`, `waiting`, `success`, `error`) and exposes it as MCP tools that any agent can call. That's structurally close to a generalized state bus — packaged as a pet, but the abstraction is right.

### 4. The hook-collision problem is real

Running disler's observability + tamagotchi + Claude HUD + ccpet would mean four hook handlers fighting for slots in `~/.claude/settings.json`. Some tools (claude-code-tamagotchi, openpets) explicitly call out hook installation as friction. There's no standard for hook composition — no "hook router" pattern in common use.

### 5. The statusline is single-tenant

Only one statusline command can be configured. ccpet, Claude HUD, claude-code-tamagotchi, and game-studio statuslines all want it. Users have to pick one. This is another argument for a state bus that statusline tools can be presenters of.

### 6. Multi-agent state is poorly handled

Most tools assume one Claude Code session. A few (disler's, mukul975's team dashboard, Pixel Agents) handle multiple. The state model gets harder once you have parent/child sub-agents, parallel teammate agents (Claude Code's experimental teammate mode), or multiple independent sessions on the same machine.

## What a generalized state layer would need

Provisional, to be refined as the deeper inventory comes in:

- **A canonical state vocabulary**: per-session state (idle / working / waiting-for-input / waiting-for-permission / done / error), plus optional fine-grained sub-state (current tool, current file, last activity timestamp).
- **An event stream**: a superset of the hook events, normalized — so a consumer doesn't need to know whether the event came from a hook, a JSONL tail, or an MCP call.
- **A query API**: "what sessions exist right now and what is each doing?" — read-side, idempotent, cacheable.
- **A single, versioned hook installation**: one shim that fans out to registered consumers, instead of every tool installing its own hook line.
- **Multi-source ingest**: hooks, JSONL tail, MCP self-report, statusline tap, OTel — all feed the same internal state model.
- **Multi-agent awareness from day one**: sessions, sub-agents, teammates as first-class objects.
- **Pluggable presenters**: dashboard, statusline, pet, kanban, HUD — all read from the same store.

## Next step

Pull each of the prior-art repos and inventory:
- What state model do they implicitly use?
- What events do they emit/consume?
- How are they installed/configured?
- What's their data store (if any)?
- What's their integration surface (hooks / JSONL / MCP / OTel / statusline)?

Priority order for deep dive: OpenPets, Pixel Agents, disler observability, claude-code-tamagotchi, ccpet, Claude HUD, hoangsonww monitor, simple10/agents-observe, mukul975 team dashboard.
