---
stepsCompleted:
  - step-01-init
  - step-02-discovery
  - step-02b-vision
  - step-02c-executive-summary
  - step-03-success
  - step-04-journeys
inputDocuments:
  - docs/bmad/planning-artifacts/product-brief-bowerbird-distillate.md
  - docs/bmad/planning-artifacts/product-brief-bowerbird.md
  - docs/bmad/brainstorming/brainstorming-session-2026-05-11-0849.md
  - docs/bmad/project-context.md
workflowType: 'prd'
briefCount: 2
researchCount: 0
brainstormingCount: 1
projectDocsCount: 1
classification:
  projectType: developer_tool
  domain: general
  complexity: medium
  projectContext: brownfield
---

# Product Requirements Document - bowerbird

**Author:** pickles
**Date:** 2026-05-11

## Executive Summary

bowerbird is a local substrate for AI coding agent activity. It captures events from Claude Code (and future agents) into a persistent SQLite event log and streams them to any tool, display, or automation a developer wants to build — via WebSocket and REST — without the substrate ever interpreting what those events mean.

The problem it addresses: every developer who wants visibility into what their AI coding agent is doing must first work out the instrumentation — hooking into the agent reliably, collecting events consistently, without slowing the agent down. bowerbird takes on that layer so tool builders don't have to. A status light, a team dashboard, a git annotator, a Slack notifier — these can all run simultaneously against the same event stream. The bower collects; you decide what it means.

**Target users (V1):** Developers who want to build their own tools on top of AI coding agent activity. Their users follow when those tools exist.

**V1 scope:** Claude Code adapter (reference implementation), WebSocket pub/sub event stream, REST snapshot API, health/readiness endpoints, and reference example tools demonstrating multi-tool simultaneous operation.

### What Makes This Special

The instrumentation layer for AI coding agents requires careful design: the shim must run in Claude's process without adding perceptible latency (< 5ms p95), events must be collected atomically with their projection state, and the protocol must be stable enough that tools built against v1 don't break when the daemon updates.

bowerbird takes on that complexity once, with a clean stable protocol and explicit performance contracts, so developers building on top never have to think about hook delivery, WAL-mode SQLite durability, WebSocket fan-out, or protocol versioning. They subscribe to an event stream and build.

The separation of concerns is the differentiator: bowerbird owns the collection layer; the tool author owns the meaning layer. This keeps the substrate small, legible, and stable — and makes every tool built on it cheap to experiment with and easy to throw away.

**Why now:** AI coding agents are capable enough that developers feel the loss of legibility — the agent is doing something, but not what. The Claude Code hook mechanism is the first first-class tap point in a widely-used agent. Building the substrate while the ecosystem is young means the protocol can be designed right before it has to be designed around.

## Project Classification

- **Project type:** Developer tool — multi-crate Rust workspace (protocol library, shim binary, daemon binary, reference adapter) with WebSocket + REST API surface
- **Domain:** Developer tooling / AI agent activity
- **Complexity:** Medium — technically nuanced (performance contracts, protocol stability, pub/sub architecture) but no regulatory overhead
- **Project context:** Brownfield pre-MVP — significant design corpus established; implementation not yet started

## Success Criteria

### User Success

The primary success signal for V1 is experiential: a developer running Claude Code can open a locally-built visualization tool and see live agent activity immediately — no hook configuration changes, no agent restarts, nothing to reconfigure. When they want to change what the tool shows or how it looks, they edit the display layer and the data keeps flowing. bowerbird is invisible.

The two properties that define "working":
- **Live iteration without disruption:** changes to the visualization don't require touching the instrumentation layer or restarting any agent
- **Multiple simultaneous tools:** two or more tools can run against the same event stream independently, each unaware of the other

V1 success gate: pickles can build and iterate on local example tools against live Claude Code sessions, with multiple tools running simultaneously, without any instrumentation changes between experiments.

### Business Success

**V1:** The reference example tools demonstrate the full flow end-to-end. The substrate works well enough for the first user (pickles) to run real experiments and find it genuinely useful.

**Post-V1:** Someone pickles has never met or suggested to builds a tool on bowerbird independently. This is the honest signal that the protocol is clean, the documentation is sufficient, and the substrate is legible enough to build on without hand-holding.

### Technical Success

Performance bars from the design corpus, treated as success criteria with the expectation that exact numbers will be validated against implementation reality:

| Bar | Target | Notes |
|---|---|---|
| Shim exit (warm cache) | < 5ms p95 | Marginal cost of our code; measured separately per platform |
| Hook → projection | < 50ms p95 | WAL + `synchronous=NORMAL` |
| Hook → presenter (end-to-end) | < 100ms p95 | Full pipeline including WS delivery |
| Daemon idle CPU | < 0.5% | Precise definition of "idle" TBD |
| Daemon RSS | < 50MB | Sample from day one; track for drift |
| Core LOC | 5K–7K | Alarm at 10K |

All 10 required contract tests passing before MVP ships (WS drop behavior, PRAGMA invariants, state+event atomicity, graceful shutdown, cursor-gap detection, atomic settings.json install, hook unreliability tolerance, outbound envelope additive-compat, shim fuzz, connection factory enforcement).

### Measurable Outcomes

- Multiple tools running simultaneously against a live Claude Code session: **✓ or ✗** (binary gate)
- Reference example tools ported and smoke-tested in CI: **✓ or ✗**
- All 10 contract tests passing: **✓ or ✗**
- All performance bars met on macOS + Linux CI: **✓ or ✗** (with per-platform baselines)
- Post-V1: independent tool author with no prior contact: **✓ or ✗**

## Product Scope

### MVP — Minimum Viable Product

- Claude Code adapter (reference implementation): shim, daemon, `adapter-claude` crate, TOML tool-reactions
- WebSocket pub/sub event stream with `events.*` and `state.*` topics
- REST snapshot API with cursor-based pagination (`/sessions`, `/sessions/:id/events?since=`)
- Health and readiness endpoints (`/healthz`, `/readyz`)
- Reference example tools (TypeScript/Node), CI smoke-tested, demonstrating simultaneous multi-tool operation
- `bowerbird install` / `bowerbird uninstall` with atomic `~/.claude/settings.json` rewrite
- All 10 required contract tests
- CI matrix: macOS + Linux, performance regression gating

### Growth Features (Post-MVP)

- Second agent adapter (Codex, Gemini, or Cursor) — validates the adapter model with a real external contributor
- `/metrics` endpoint (Prometheus-compatible, path reserved at MVP)
- `bowerbird gc` for event-log truncation (policy decision deferred)
- arm64 CI runner
- `@bowerbird/presenter` SDK if presenter boilerplate ratio justifies it (revisit after first external tool)

### Vision (Future)

- bowerbird as the vendor-neutral observability substrate for AI coding agents — adapter-per-agent, single stable protocol, any tool in any language can subscribe
- Community-maintained adapter ecosystem
- Protocol versioning (v2+) with full backward-compat guarantee for existing tools

## User Journeys

### Journey 1 — Tool Builder: First Tool (Happy Path)

**Marcus wants to see what Claude is doing while he works.**

Marcus has been using Claude Code for a few weeks. He's productive with it, but he's developed a habit he finds annoying: alt-tabbing to the terminal to check if Claude is still running, finished, or waiting on him. He wants a signal he can see without context-switching — a status dot in his menu bar.

He finds bowerbird in the README of a Claude Code community thread. He runs `bowerbird install`. It modifies his `~/.claude/settings.json` to add the hook shim, tells him the daemon is running, and shows a health check URL. Ten seconds.

He opens the bowerbird docs, reads the WebSocket event format, and spends an afternoon writing ~80 lines of TypeScript. His tool subscribes to the `state.session.*` topic and maps `current_state` values to a menu bar icon: green for idle, yellow for working, red for waiting on input. He runs it. The dot appears. He triggers a Claude Code tool call. The dot goes yellow. It goes green when Claude finishes.

He didn't configure a hook. He didn't restart Claude. He didn't touch anything except his 80-line TypeScript file and the one install command.

**This journey requires:** `bowerbird install` CLI, daemon auto-start, WebSocket `state.*` topic, health check endpoint, clear event format documentation.

---

### Journey 2 — Tool Builder: Iterating Without Disruption (The Core Value Prop)

**Marcus wants to extend his tool without losing his flow.**

A week after shipping his menu bar dot, Marcus wants more. He's noticed that Claude sometimes makes a burst of tool calls in quick succession, and he wants to see a running count for the current session — how many tool calls so far, which tools were used most. He wants this as a tooltip on the menu bar icon.

He opens his TypeScript file. He adds a counter that increments on each `events.tool_use` event and formats it as tooltip text. He saves the file, restarts his tool process. That's it.

Claude Code is still running. The daemon is still running. The event stream reconnects immediately on tool restart — the daemon doesn't care that the subscriber disappeared for two seconds. The counter starts from the session snapshot (fetched via REST on startup) and picks up from there. Marcus sees the tool call count ticking up in real time within about 30 seconds of starting his edit.

He changed the display layer. He never thought about the instrumentation layer.

**This journey requires:** WebSocket reconnect with snapshot re-fetch via REST, `events.*` topic, cursor-based `/sessions/:id/events?since=` endpoint, session snapshot on connect.

---

### Journey 3 — Tool User: Installing a Shared Tool

**Priya wants the lamp, not the plumbing.**

Priya's colleague messages her a GitHub link: "bowerbird-lamp — turns a Govee smart bulb green when Claude is idle, yellow when working, red when blocked on input." Priya has never heard of bowerbird. She clicks the link, reads the README: "requires bowerbird running locally."

She runs `brew install bowerbird` (or `cargo install bowerbird`), then `bowerbird install`. The README says "that's it for setup." She clones the lamp repo, runs `npm install && npm start`. The bulb changes color. She opens Claude Code, asks it to run a task. The bulb goes yellow. It goes green when Claude finishes waiting.

Priya has no idea what WebSocket pub/sub is. She didn't write code. She didn't read the bowerbird protocol docs. She got value in under five minutes.

The next day she installs a second tool — a terminal dashboard a different colleague shared. Both tools run simultaneously. Neither affects the other. Neither required her to change anything about bowerbird.

**This journey requires:** Simple install story (`brew` / `cargo install`), daemon auto-start on install, stable protocol so community tools don't break on daemon updates, clear "what does bowerbird install do to my system" documentation.

---

### Journey 4 — Tool Builder: Troubleshooting

**Marcus's tool goes blank mid-session.**

Marcus is deep in a Claude Code session. His menu bar dot has been working for two weeks. Mid-session, it goes gray — no data. He checks his tool's log: `WebSocket disconnected. Reconnecting...` followed by `Reconnected. Received dropped frame (lagged 47 events).`

His tool handles this: on receiving a `dropped` frame it fetches a fresh snapshot from the REST API (`/sessions/:id/events?since=<cursor>`) and rehydrates its state. Within a few seconds the dot is back, showing the correct current state. He lost no session history — the SQLite log on the daemon side has everything. His tool just temporarily fell behind.

He checks: what caused the lag? His tool was doing something CPU-intensive (rendering a heavy chart) that slowed its WebSocket read loop. The daemon's broadcast channel filled up and started dropping slow consumers. bowerbird didn't crash. Claude Code never noticed. His tool recovered automatically.

Later that week the daemon does crash — a disk-full edge case during a long session. Claude Code keeps running; the shim gets a connection-refused error, logs it to `~/.bowerbird/shim.log`, and exits cleanly (no hook timeout visible to Claude). Marcus sees his tool disconnect. He frees disk space, restarts the daemon with `bowerbird start`. His tool reconnects. The session events from before the crash are gone (the daemon restarted clean), but his tool handles this gracefully — it resets its state on reconnect.

**This journey requires:** `dropped` frame on WS lag with lag count, REST snapshot re-fetch on reconnect, shim failure logging to file (never stdout/stderr), daemon restart CLI, clear documentation of what survives a daemon crash vs. what doesn't.

---

### Journey Requirements Summary

| Capability | Required By |
|---|---|
| `bowerbird install` / `bowerbird uninstall` CLI | Journeys 1, 3 |
| Daemon auto-start; `bowerbird start` / `bowerbird status` | Journeys 1, 3, 4 |
| WebSocket `state.*` and `events.*` topics | Journeys 1, 2 |
| REST snapshot API (`/sessions`, `/sessions/:id/events?since=`) | Journeys 2, 4 |
| WS reconnect with snapshot re-fetch | Journeys 2, 4 |
| `dropped` frame on lag with count | Journey 4 |
| Health check endpoint (`/healthz`, `/readyz`) | Journey 1 |
| Shim failure logging to file (never stdout/stderr) | Journey 4 |
| Stable protocol (community tools survive daemon updates) | Journey 3 |
| Clear "what does install do" documentation | Journeys 1, 3 |
| Clear "what survives a crash" documentation | Journey 4 |
