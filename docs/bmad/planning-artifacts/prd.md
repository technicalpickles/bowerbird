---
stepsCompleted:
  - step-01-init
  - step-02-discovery
  - step-02b-vision
  - step-02c-executive-summary
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
