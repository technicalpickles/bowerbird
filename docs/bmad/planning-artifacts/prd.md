---
stepsCompleted:
  - step-01-init
  - step-02-discovery
  - step-02b-vision
  - step-02c-executive-summary
  - step-03-success
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
