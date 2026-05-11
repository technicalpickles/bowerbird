---
title: "Product Brief: bowerbird"
status: "complete"
created: "2026-05-11"
updated: "2026-05-11"
inputs:
  - docs/bmad/project-context.md
  - docs/bmad/brainstorming/brainstorming-session-2026-05-11-0849.md
---

# Product Brief: bowerbird

## Executive Summary

Coding agents — Claude Code, GitHub Copilot (Codex), Gemini, Cursor — emit rich signals about what they're doing: which tool they called, which file they edited, whether they're waiting or working. Those signals disappear into log files, or nowhere at all.

bowerbird is a local observability substrate that captures those signals and makes them available — in real time, via WebSocket — to any tool that wants to react to them. An ambient lamp that glows amber when Claude is thinking. A sprite that animates when a tool call lands. A dashboard that tracks session history and state transitions over time. bowerbird doesn't build those tools. It's the substrate they stand on.

The core philosophy: **observe and preserve, never interpret.** bowerbird captures agent hook events verbatim, stores them locally, and broadcasts them through a lean pub/sub protocol. What those events *mean* is the presenter's problem. bowerbird's job is to make sure nothing gets lost — and to stay out of the interpretation business.

## The Problem

Coding agents are increasingly ambient — they run for minutes or hours, making hundreds of tool calls, moving through states (thinking, editing, waiting, blocked) that developers intuitively care about but can't easily observe. The information exists: Claude Code fires `PreToolUse` and `PostToolUse` hooks with full payload on every tool call. But acting on it requires:

- Hooking into each agent's specific hook mechanism without slowing it down
- Writing a shim fast enough to be invisible (< 5ms) in the agent's process
- Building infrastructure to store events, reconstruct state, and broadcast to multiple subscribers
- Repeating this entire stack for every agent you want to observe

Nobody builds ambient coding-agent tools today because the plumbing cost is prohibitive. The few that exist are one-off, agent-specific, and fragile. There's no shared substrate — and so the space of possible tools doesn't get built.

## The Solution

bowerbird installs a lightweight shim into a coding agent's hook system. The shim relays hook events — with < 5ms p95 overhead — to a local daemon that stores them (SQLite, WAL mode), projects current agent state, and broadcasts to any subscribers via WebSocket pub/sub.

Presenter authors — the people building lamps, pets, sprites, dashboards — subscribe to state topics with 80–150 lines of code in any language that speaks WebSocket+JSON. They get clean, real-time agent state without touching hook configuration, without managing SQLite, and without implementing pub/sub. The reference quickstart targets a working lamp presenter in under five minutes.

**The protocol is the product.** bowerbird's value is a stable, observable surface. Presenters build to the protocol, not to any specific agent. When a new agent gets a bowerbird adapter, all existing presenters work without changes.

All data stays on the developer's machine. The daemon binds to `127.0.0.1` — nothing leaves the local environment.

## What Makes This Different

**Substrate-not-actor.** bowerbird never adds application-level semantics to the protocol — no `is_user_attention_needed`, no derived sentiment flags, no presenter-specific interpretations. It emits mechanical facts; presenters draw conclusions. This constraint is what keeps the protocol stable and the presenter ecosystem open-ended.

**Shim performance is a hard contract.** The shim runs inside the coding agent's process. A stall there is a stall for the user's entire session. The < 5ms target is non-negotiable in spirit, enforced by CI benchmarks, not claimed in documentation. This rules out async runtimes, dynamic allocation on the hot path, and any enrichment that can happen daemon-side.

**Adapter model.** One daemon, many agents. Claude Code today; Codex, Gemini, Cursor via adapters. Each adapter normalizes tool names to a stable 11-value reaction enum — that's the only normalization bowerbird performs. Existing presenters don't break when a new adapter ships.

**Local-first by design.** No cloud dependency, no account required, no telemetry. For developers who are rightly cautious about what their AI coding sessions reveal, this is a meaningful property.

## Who This Serves

**Primary: Developer tool authors** who want to build ambient or reactive tools around coding agent sessions. They're comfortable writing code, want a clean protocol and a working reference example, and will implement presenters in TypeScript, Python, Go, or whatever they reach for first. They need bowerbird to handle the plumbing so they can focus on the feature — not on WS connection management and SQLite schema design.

**Secondary: Individual developers** who want to run community-built presenters (lamps, status widgets, voice hooks) without authoring their own. For them, bowerbird is install-once infrastructure.

## Success Criteria

- A developer can clone the reference lamp example and see it react to live Claude Code events in under five minutes
- The reference TypeScript presenter stays under 150 lines of feature code, excluding connection boilerplate
- Shim overhead holds at < 5ms p95, measured per-platform in CI (macOS and Linux separately — process spawn timing differs)
- The adapter model validates: a second agent adapter ships and no existing presenters require changes
- The protocol is stable enough that the community ships presenters against it without upstream guidance

## Scope

**In for v1:**
- `bowerbird install` / `bowerbird uninstall` — hooks Claude Code atomically; never corrupts `~/.claude/settings.json`
- Daemon: event ingestion, SQLite storage (WAL), state projection, WebSocket pub/sub, REST snapshot API with cursor-based pagination
- Claude Code adapter (reference implementation)
- TypeScript/Node example presenters with CI smoke tests
- macOS + Linux distribution via Homebrew and `cargo install`
- Health and readiness endpoints for presenter liveness checks

**Explicitly out:**
- Windows support (explicit scope cut; no way to test it locally)
- Any feature requiring the daemon to interpret agent events — no personas, priorities, sentiment, urgency
- HITL backflow, tool blocking, multi-host/LAN access
- Distro packaging (Debian, Arch, nixpkgs) — community-driven if it happens

## Vision

Two years from success: bowerbird is the substrate that ambient developer tooling is built on. Coding agents are as observable as web servers (Prometheus) or browsers (DevTools). A developer installing a new coding agent also runs `bowerbird install`, as automatically as they configure their terminal or editor.

A small ecosystem of presenters — lamps, pets, voice hooks, IDE widgets, team dashboards — speaks the bowerbird protocol, built by independent authors who never had to touch the daemon. Adapter authors can support a new coding agent without touching a single presenter. The substrate stays small, ferociously out of the interpretation business, and deliberately constrained — which is exactly what makes the ecosystem open.
