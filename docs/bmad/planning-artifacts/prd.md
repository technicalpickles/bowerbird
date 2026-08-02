---
stepsCompleted:
  - step-01-init
  - step-02-discovery
  - step-02b-vision
  - step-02c-executive-summary
  - step-03-success
  - step-04-journeys
  - step-05-domain
  - step-06-innovation
  - step-07-project-type
  - step-08-scoping
  - step-09-functional
  - step-10-nonfunctional
  - step-11-polish
releaseMode: phased
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
status: final
updated: 2026-08-02
---

# Product Requirements Document - bowerbird

**Author:** pickles
**Date:** 2026-05-11
**Updated:** 2026-08-02 (Phase 3 refresh: post-v0.1.0 cycle, attention surfaces + adoption; see decision log at `prds/prd-bowerbird-2026-08-02/.decision-log.md`)

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
- **Project context:** Brownfield, shipped: v0.1.0 tagged 2026-08-02 (Epics 1 through 5 complete). The original "pre-MVP, implementation not yet started" framing is retained in git history; this document now describes a live substrate entering its Phase 3 cycle.

## Success Criteria

### User Success

The primary success signal for V1 is experiential: a developer running Claude Code can open a locally-built visualization tool and see live agent activity immediately — no hook configuration changes, no agent restarts, nothing to reconfigure. When they want to change what the tool shows or how it looks, they edit the display layer and the data keeps flowing. bowerbird is invisible.

The two properties that define "working":
- **Live iteration without disruption:** changes to the visualization don't require touching the instrumentation layer or restarting any agent
- **Multiple simultaneous tools:** two or more tools can run against the same event stream independently, each unaware of the other

V1 success gate: pickles can build and iterate on local example tools against live Claude Code sessions, with multiple tools running simultaneously, without any instrumentation changes between experiments.

**Phase 3 success gate (2026-08-02):** bowerbird earns a place in the maintainer's actual working day. Concretely: the attention surfaces (notifications, glance, board, ambient) replace manual tmux pane-cycling as the way the maintainer discovers blocked and finished sessions. V1 proved the substrate works; Phase 3 proves it is worth using. The Phase 2 method ("build a presenter, use it daily") produced the tool but not the habit, so Phase 3 inverts the shape: instead of one destination TUI, several small surfaces that meet the maintainer where attention already lives (OS notifications, the tmux status line, a one-shot CLI glance).

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

**Phase 3 measurable outcomes (added 2026-08-02; the V1 table had no usage metric, which is how the dogfooding stall went unmeasured):**

- Daily-driver signal: across 10 consecutive working days, the maintainer acts on at least one bowerbird attention surface per day (a notification jumped to, a glance invoked, the ambient count checked) instead of discovering session state by pane-cycling: **✓ or ✗**, self-reported honestly
- Stranger time-to-first-prototype: an ecosystem-versed developer with a specific use case in mind goes from "never heard of bowerbird" to "my own prototype running against live sessions" within one hour, using a cookbook entry as the starting skeleton: **✓ or ✗** (measured when the first such person shows up; the counter-metric is where in the hour they bounced)
- Cookbook as accelerator: each new cookbook entry is validated by real daily use before it lands (the entry is harvested from a tool the maintainer runs, not authored speculatively): **✓ or ✗** per entry

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

### Phase 3 Scope (current cycle, 2026-08-02)

Four new cookbook entries, built in order, each dogfooded before it lands (see Phase 3 journeys and FR40 through FR44):

1. **`session-glance`**: one-shot CLI print of session states, repo-keyed via `cwd`, age via `started_at`. First consumer of the Story 5.8 REST `?state=` filters.
2. **`transition-alerts`**: OS notifications on blocked (`WaitingInput`) and done (`Working` to `Idle`) transitions, with the reason joined from the events stream and the repo named on every alert.
3. **`live-board`**: the long-lived TUI, absorbed from the bowerbird-deck sibling repo (deck is archived once this lands).
4. **`tmux-ambient`**: a status-line count (e.g. "2 blocked") wired from the glance query.

Substrate changes this cycle: none planned. All four entries run on the shipped v0.1.0 wire surface. Protocol maturation is a side effect, not a goal (maintainer decision, 2026-08-02).

### Growth Features (Post-MVP backlog)

Consolidated 2026-08-02 (this list previously appeared twice, here and under Project Scoping, with drift between the copies):

- Second agent adapter (Codex, Gemini, or Cursor) — validates the adapter model with a real external contributor
- `/metrics` endpoint (Prometheus-compatible, path reserved at MVP)
- `bowerbird gc` for event-log truncation (policy decision deferred)
- arm64 CI runner
- `@bowerbird/presenter` SDK if presenter boilerplate ratio justifies it (revisit after first external tool)
- Homebrew tap (deferred; audience does not justify tap maintenance yet)
- Promote `session-glance` to a built-in `bowerbird sessions` subcommand: triggered, not scheduled. Only if daily use shows the node-invocation friction of the cookbook entry actually bites.
- Live-only subscription flag (`subscribe` opting out of the snapshot burst): triggered, not scheduled. Build `transition-alerts` on the per-key map pattern first (the pattern is needed for reconnect correctness regardless); add the protocol flag only if daily use shows the pattern is clumsy. See addendum for the design analysis.
- `sync` frame activation: the frame is specced and reserved but never emitted; no journey currently demands it. Stays dormant.

### Vision (Future)

- bowerbird as the vendor-neutral visibility substrate for AI coding agents — adapter-per-agent, single stable protocol, any tool in any language can subscribe
- Community-maintained adapter ecosystem
- Protocol versioning (v2+) with full backward-compat guarantee for existing tools

## Project Scoping

### MVP Strategy & Philosophy

**Approach: Experience MVP — prove the substrate works by using it.**

The MVP is shaped around a single user (pickles) running real experiments. This is intentional and honest: the riskiest assumption is not "can we build the protocol correctly" but "does the separation of instrumentation from display layer actually make experiments cheap?" The only way to validate that is to run experiments.

V1 ships when pickles can build and iterate on multiple tools simultaneously against live Claude Code sessions without instrumentation friction. The reference examples serve dual purpose: they demonstrate the pattern to future tool authors, and they are the experiments that validate the substrate.

**Resource requirements:** Solo maintainer (pickles). All core crates in Rust; reference examples in TypeScript/Node. No coordination overhead — one person decides what ships.

**MVP philosophy carve-out:** the contribution model (auto-close by default, weekly triage) is explicitly designed for a solo maintainer. V1 does not need community adoption to succeed — it needs one user (the maintainer) to find it genuinely useful.

**Phase 2 — Release Readiness (Epic 5):** V1 ships in two beats. Phase 1 (Epics 1–4, complete) produced the capture + streaming + install + replay substrate. Phase 2 validates and hardens it: the maintainer installs bowerbird on their main machine, builds a first-party presenter in a sibling repository, uses it daily, and harvests the friction. Bugs found in that loop become hotfix stories folded into the Epic 5 cadence; the planned Epic 5 stories then convert the CI gates from aspirational to load-bearing, exercise the release pipeline end-to-end, polish the install UX, and rewrite the README and quickstart for the first-time reader who has not been in the room while bowerbird was built. The v0.1.0 tag is the closing event of Phase 2.

**Phase 2 outcome, recorded honestly (2026-08-02):** the method half-worked. The dogfood window (five working days in late May) found the epic's realest bugs, and every correctness story in Epic 5 traces to it. But the daily-use habit did not survive past that window: bowerbird-deck did not stick as a daily driver, and by epic close the maintainer was not running bowerbird day to day. The stall was never measured because no success metric counted usage (fixed in Measurable Outcomes, above). Diagnosis from the Phase 3 discovery session: a destination TUI demands a permanent pane it has not yet earned, while the actual attention loop is interrupt-shaped (notifications) and glance-shaped (a one-shot query), not dwell-shaped.

**Phase 3, Attention Surfaces and Adoption (current cycle):** restart the experiment with surfaces that meet attention where it lives, then convert each proven surface into a cookbook entry that doubles as a prototype accelerator for the first outside adopters. Stakes are calibrated as "still an experiment": everything reversible, no substrate changes planned, adoption pursued but not at announcement rigor. Goals in order: (1) the maintainer as genuine daily user, (2) outside adoption via the cookbook, (3) protocol maturation only as a side effect.

### MVP Feature Set

**All four user journeys supported at v1:**
- Journey 1 (tool builder, first tool) — full happy path
- Journey 2 (iterating without disruption) — core value prop validated
- Journey 3 (tool user, installing a shared tool) — enabled by prebuilt binaries + stable protocol
- Journey 4 (troubleshooting) — `dropped` frame + reconnect + shim failure logging

**Must-have capabilities (confirmed):**
- Claude Code adapter (shim + daemon + `adapter-claude` + TOML tool-reactions)
- WebSocket pub/sub (`state.*` + `events.*` topics, wildcard subscriptions, multi-session fan-out)
- REST snapshot API (cursor-paginated `/sessions/:id/events?since=`)
- Unix domain socket ingest path (shim → daemon)
- `bowerbird install` / `bowerbird uninstall` (atomic `~/.claude/settings.json`)
- `bowerbird replay` + `bowerbird export` (fake signal stream for Quickstart + session capture)
- Three reference examples (multi-session router, event log viewer, reconnect recovery)
- All 10 required contract tests
- Documentation path: Quickstart → presenter-authoring → protocol → cookbook
- CI: macOS + Linux, performance regression gating, `cargo build --examples`

### Post-MVP Features

Consolidated into **Growth Features (Post-MVP backlog)** under Product Scope (2026-08-02); this section previously duplicated that list with drift.

### Risk Mitigation Strategy

**Technical risks:**

- **Shim performance budget (< 5ms p95)** — most technically uncertain item; depends on Claude Code's hook execution model and platform-specific process spawn timing. Mitigation: bench from day one (`shim/benches/hot_path.rs`), measure separately on macOS and Linux. If the number can't be met cleanly, the right response is an ADR documenting the real number — not a silent miss.
- **Unix domain socket ingest** — new decision; simpler than TCP+auth, not more complex. Low risk.
- **Protocol stability guarantee** — committing to additive-only within v1.x is a design discipline constraint. Mitigation: the protocol crate is the enforcement mechanism; CI gates on changelog entries.

**Market risks:**

- **Primary: the substrate works but experiments aren't actually cheap.** If iterating on a tool still requires enough ceremony that it doesn't feel lighter than rolling your own, the core bet fails. Mitigation: pickles is the canary. If the experience isn't meaningfully better than ad-hoc, that's a V1 finding, not a V2 failure.
- **Secondary: no one finds it.** Acceptable for V1 — the post-V1 signal (stranger builds a tool without being asked) is the adoption gate, not V1 itself.

**Resource risks:**

- **Solo maintainer = single point of failure.** Mitigation: contribution model, maintainer status protocol, and auto-close tagging are explicitly designed for this. The risk is acknowledged and designed around, not ignored.

## User Journeys

### Journey 1 — Tool Builder: First Tool (Happy Path)

**Marcus wants to see what Claude is doing while he works.**

Marcus has been using Claude Code for a few weeks. He's productive with it, but he's developed a habit he finds annoying: alt-tabbing to the terminal to check if Claude is still running, finished, or waiting on him. He wants a signal he can see without context-switching — a status dot in his menu bar.

He finds bowerbird in the README of a Claude Code community thread. He runs `bowerbird install`. It modifies his `~/.claude/settings.json` to add the hook shim, tells him the daemon is running, and shows a health check URL. Ten seconds.

He opens the bowerbird docs, reads the WebSocket event format, and spends an afternoon writing ~80 lines of TypeScript. His tool subscribes to the `state.session.*` topic and maps `current_state` values to a menu bar icon: green for idle, yellow for working, red for waiting on input. He runs it. The dot appears. He triggers a Claude Code tool call. The dot goes yellow. It goes green when Claude finishes the turn.

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

**This journey requires:** `dropped` frame on WS lag with lag count, REST snapshot re-fetch on reconnect, shim failure logging to file plus one cause line on stderr for exit-1 failures (the success and exit-0 daemon-answered paths stay stdout/stderr-silent — Story 5.10), daemon restart CLI, clear documentation of what survives a daemon crash vs. what doesn't.

---

### Phase 3 Journeys (added 2026-08-02)

Captured from the maintainer's narration in the Phase 3 discovery session. Journeys 1 through 4 above are the V1 record and stand unchanged; the `brew install` path in Journey 3 is historical (see Installation Methods for the real paths).

### Journey A: The Pane-Cycling Day (as-is, the journey this cycle exists to fix)

**pickles runs several projects, each a herdr workspace / tmux session with multiple panes: agents, editors, commands.**

An agent gets kicked off in one pane; attention moves to another project. The loop for discovering what needs attention is manual: cycle through the panes and look. The most common find is a session blocked on a permission prompt, because sessions start in accept-edits mode with the intention of switching to auto mode, and sometimes that switch never happens. The session has been sitting blocked for however long it took the cycle to come back around. The other find is an agent that went idle a while ago, with follow-up waiting.

The cost is not one big failure; it is a steady leak. Attention goes to polling. Blocked and finished sessions wait for the next sweep instead of for the person.

### Journey B: Interrupt-Driven With a Glanceable Fallback (intended daily-driver day)

Same environment, same projects. The daemon is already running (start-on-login, Story 5.9).

An agent blocks on permissions or finishes a turn. An OS notification fires immediately, naming the state change, the reason (permission prompt for which tool), and WHERE: which repo, and where feasible which tmux pane. pickles hops to that session and unblocks or responds. When a notification was missed live (away from the desk, heads-down), the fallback is at-a-glance: a one-shot CLI print of every session's state, grouped by repo, showing working / idle / needs-prompt with ages. For watching the flow of events over time, a longer-lived live board shows the eventing itself.

Pane-cycling becomes a thing that no longer happens. The hop (notification lands you in the right pane) is a stretch goal: pursued only if it turns out cheap; the floor requirement is that every surface names WHERE the session is.

**This journey requires:** `state.session.*` subscription with the per-key map pattern (snapshot absorbed silently, live frames treated as transitions), the events stream joined for notification reasons, `cwd` and `started_at` on the wire (shipped, Story 5.7), REST `?state=` filters (shipped, Story 5.8). Zero new substrate capabilities: everything in this journey runs on the shipped v0.1.0 wire surface.

### Journey C: The Stranger's First Hour (adoption)

**An ecosystem-versed developer, already burned by point tools.**

They have used existing agent-observability and status tools, and hit the wall every such tool builds: each one owns its own capture plumbing, so combining them means fighting them or rebuilding them. They want to do more, and they refuse to re-solve instrumentation to get it.

They arrive at bowerbird with a specific use case in mind, possibly a tool they already built and want to re-base onto a real substrate. They install (quickstart path, timed and falsified in Story 5.14). They find the cookbook entry nearest their use case and copy it as a starting skeleton: entries earn their keep as prototype accelerators, not demos. Within the first hour they have a working prototype of THEIR thing running against live sessions.

The keep-moment: their prototype runs alongside everything else with no instrumentation fight. The thing they came fed up about is the thing that just worked.

The bounce risk concentrates in the find-the-nearest-entry step: if no entry is near their use case, the hour dies in protocol documentation. Cookbook breadth-of-starting-points is therefore the load-bearing adoption deliverable.

### Journey Requirements Summary (Journeys 1 through 4)

Covers the V1 journeys only. Phase 3 journey requirements are inline in Journey B above; the load-bearing fact there is that they introduce zero new substrate capabilities.

| Capability | Required By |
|---|---|
| `bowerbird install` / `bowerbird uninstall` CLI | Journeys 1, 3 |
| Daemon auto-start; `bowerbird start` / `bowerbird status` | Journeys 1, 3, 4 |
| WebSocket `state.*` and `events.*` topics | Journeys 1, 2 |
| REST snapshot API (`/sessions`, `/sessions/:id/events?since=`) | Journeys 2, 4 |
| WS reconnect with snapshot re-fetch | Journeys 2, 4 |
| `dropped` frame on lag with count | Journey 4 |
| Health check endpoint (`/healthz`, `/readyz`) | Journey 1 |
| Shim failure logging to file; stderr cause line on exit-1 only (success/exit-0 stay silent) | Journey 4 |
| Stable protocol (community tools survive daemon updates) | Journey 3 |
| Clear "what does install do" documentation | Journeys 1, 3 |
| Clear "what survives a crash" documentation | Journey 4 |

## Innovation & Novel Patterns

### Detected Innovation Areas

**The substrate-not-actor paradigm for AI agent tooling**

bowerbird makes a deliberate bet that the most valuable layer in an AI coding agent visibility stack is the *thinnest* one. The instinct in most tooling is to add interpretation — derive insights, surface recommendations, prioritize signals. bowerbird refuses. A substrate that collects faithfully and routes without opinion gives every tool built on top of it complete creative freedom, and keeps the substrate small enough to be stable and trustworthy over time.

This is the Unix pipes play applied to AI agent observability. The value isn't features; it's a clean separation of concerns that didn't exist before.

**Protocol-first design in an emerging ecosystem**

The Claude Code hook mechanism is the first first-class tap point in a widely-used AI coding agent. bowerbird's bet is that building a stable, versioned, vendor-neutral protocol *now* — before the ecosystem has to design around an ad-hoc one — is worth more than any specific feature on top of it. The adapter model (one protocol, multiple agent sources) is the long-term play: community adapters for Codex, Cursor, Gemini can follow the same protocol without bowerbird changing.

**Instrumentation as a shared solved problem**

Every developer who wants visibility into AI coding agent activity today solves the instrumentation problem themselves: hook delivery, event consistency, performance contracts, protocol stability. bowerbird's bet is that treating this as shared infrastructure worth building once, correctly, lets the ecosystem focus on the display and reaction layer where the interesting ideas live.

### Market Context & Competitive Landscape

No direct competitor currently offers a stable, vendor-neutral, collection-only substrate for AI coding agent activity. The legibility gap is real: developers using AI coding agents have no low-friction way to see what their agent is doing. The space has:
- Agent-native observability (built into the agent platform, opinionated, vendor-locked)
- Log-scraping approaches (brittle, break on format changes)
- Ad-hoc hook scripts (no stable protocol, no multi-tool support)

bowerbird's position is the infrastructure layer none of these provide.

### Validation Approach

The protocol-first bet is validated by a specific signal: does a developer who has never spoken to pickles build a tool on bowerbird without asking a single question? That's the proof that the protocol is clean and the documentation is sufficient — the innovative design held under real use.

The deliberate restraint bet is validated by what *doesn't* appear in issues and PRs: feature requests to add derived fields, sentiment, priorities, or interpretation to the daemon. If those don't appear, the paradigm is holding. If they appear frequently, the market is providing signal worth examining.

### Risk Mitigation

- **Risk:** The deliberate restraint — bowerbird collects and routes but never derives — reads as a missing feature to users who haven't yet felt the pain of opinionated layers. **Mitigation:** Lead with the concrete value ("skip the plumbing, build the tool you want") rather than the design philosophy; let the philosophy be discoverable in the design docs for those who want to understand why.
- **Risk:** Claude Code changes its hook schema, breaking the reference adapter. **Mitigation:** The adapter pattern is explicitly designed for this — `adapter-claude` normalizes Claude's schema to the stable protocol; schema changes are adapter concerns, not protocol concerns.
- **Risk:** The ecosystem doesn't develop independently-authored tools. **Mitigation:** V1 ships reference examples that demonstrate the full pattern; these lower the barrier for the first external author.

## Developer Tool Specific Requirements

### Language Matrix

| Layer | Language | Notes |
|---|---|---|
| Protocol crate | Rust | Stable wire surface; public API; all changes need ADR |
| Shim binary | Rust | Static binary; no async runtime; < 5ms p95 hot path |
| Daemon binary | Rust | Tokio single-thread; axum; rusqlite |
| Reference adapter | Rust | `adapter-claude` — Claude Code hook normalization |
| Reference example tools | TypeScript / Node | Lives in `docs/cookbook/<name>/`; CI smoke-tested |
| Install/CI scripts | Shell | < 200 line budget; `shellcheck` strict mode in CI |

### Installation Methods

**1. Prebuilt binaries (GitHub Releases) — primary path**
Targets: macOS arm64, macOS x86_64, Linux x86_64 (Linux arm64 if CI budget allows). Pickles has prior art for the release pipeline. This is the path for users without a Rust toolchain.

**2. Source build**
`cargo install --git https://github.com/technicalpickles/bowerbird --tag vX.Y.Z` for any platform with a Rust stable toolchain. `Cargo.lock` committed; reproducible builds. (Corrected 2026-08-02: plain `cargo install bowerbird` does not exist; crates.io publishing is deferred per ADR 0011, and `brew install` was cut by the no-list. Journey 3's install wording above is the historical V1 record.)

**Hook installation (separate from binary install):**
`bowerbird install` atomically modifies `~/.claude/settings.json` — reads, parses, merges the hook entry, writes to `.tmp`, renames. On collision (concurrent write from Claude Code), the operation retries with backoff. The hook entry written to `settings.json` uses a **PATH-relative binary name** (`bowerbird`) rather than an absolute path, to survive Homebrew upgrades and `cargo install` updates. Version-mismatch between shim and daemon (different binary versions installed via different methods) logs a warning on daemon startup and is documented as unsupported.

### API Surface (v1 Stable)

#### Two Socket Surfaces

| Surface | Protocol | Auth | Callers |
|---|---|---|---|
| `~/.bowerbird/ingest.sock` | Unix domain socket | None (filesystem `0600`) | Shim only |
| `127.0.0.1:<port>` TCP | HTTP + WebSocket | Bearer token | Tools, REST clients |

The ingest path uses a Unix domain socket with filesystem-permission security — no bearer auth. The shim connects to the socket path (compiled-in default, optional config override). This eliminates the "how does the shim get the token" problem and removes auth overhead from the 5ms hot path.

#### REST Endpoints (TCP, Bearer Auth)

| Endpoint | Auth | Description |
|---|---|---|
| `GET /healthz` | none | Liveness — process up and responding |
| `GET /readyz` | none | Readiness — DB reachable, migrations applied, broadcasters live |
| `GET /status` | bearer | Version, uptime, connected tool count, last event time |
| `GET /sessions` | bearer | List known sessions. Optional filters (Story 5.8, ADR 0008): `?state=<csv>` (read-time current_state), `?since=<updated_at_ms>` (recency bound, not a cursor), `?limit=<n>`. Absent = unfiltered. |
| `GET /sessions/:id` | bearer | Session detail and current projection state |
| `GET /sessions/:id/events?since=<cursor>` | bearer | Cursor-paginated event log; response includes `oldest_available_event_id` |
| `GET /sessions/:id/stats` | bearer | Per-session event counts and tool-use breakdown. Fields are additive-only; clients must tolerate unknown fields. |

Reserved (not implemented in v1): `GET /metrics` — Prometheus text format; path reserved now.

**Ingest endpoint (Unix socket):**
Newline-delimited JSON over the Unix domain socket — one `{event-object}\n` request, one status-line response (`200\n` on accept, `503\n` under backpressure, `400 <reason>\n` on malformed payload). See [ADR-0002](../../decisions/0002-ingest-wire-framing-and-hook-kind.md) for the wire contract; supersedes the earlier "POST /ingest via HTTP/1.1" wording. The daemon returns the `200` synchronously after the event is accepted into the write queue — not after it is persisted to SQLite. The shim gets an ACK within the 5ms budget; actual persistence happens asynchronously. Under backpressure (`503`), the shim logs to `~/.bowerbird/shim.log` and exits cleanly (exit 0). If the daemon is unreachable (socket does not exist, `ECONNREFUSED`), the shim logs to `~/.bowerbird/shim.log` and exits non-zero — surfacing to Claude Code that the hook failed.

#### WebSocket (TCP, Bearer Auth)

Connect: `ws://127.0.0.1:<port>/ws` (bearer token in `Authorization` header or `?token=` query param).

**Subscribe / unsubscribe messages (client → daemon):**
```json
{ "op": "subscribe", "topic": "state.session.*" }
{ "op": "subscribe", "topic": "events.*" }
{ "op": "unsubscribe", "topic": "events.*" }
```

One topic per message; multi-topic subscription is "send multiple subscribe messages." Inbound messages use strict `deny_unknown_fields` parsing per `crates/protocol/src/ws.rs::ClientMessage`. Wire shape back-amended for Story 2.1 (2026-05-20). Subscribe additionally accepts an optional `states` array (Story 5.8, ADR 0008) scoping the snapshot-on-subscribe burst to sessions in the named states; valid only on `state.session.*` family topics, and it scopes only the snapshot, never the live stream.

**Server-sent frame types:**

| Frame | When sent |
|---|---|
| `hello` | Sent immediately on connect; includes `protocol_version` and daemon version |
| `state` | Snapshot on subscribe + on any state change for subscribed sessions |
| `event` | Each new event matching subscribed topics |
| `dropped` | Client broadcast slot lagged; includes lag count in events (not bytes). Client should re-fetch snapshot via REST. Socket stays open. |
| `close` | Daemon graceful shutdown |
| `sync` | Reserved; specced with a validated constructor but never emitted by any shipped daemon (added to this table 2026-08-02 to document reality; activation is a Growth-backlog item with no current demand) |

**Topics (v1):**
- `state.session.*` — all session state changes (wildcard; new sessions appear as `state` frames)
- `state.session.<id>` — one session's full state row
- `state.session.<id>.current_state` — current_state field only
- `events.*` — all events, all sources
- `events.<source>.*` — events from a specific source

**Multi-session behavior:** when a new session appears while a tool is subscribed to `state.session.*`, the daemon emits a `state` frame for the new session. Tools must handle new sessions arriving at any time.

#### What Is NOT Stable (Internal)

- SQLite schema — do not read the DB directly
- Internal daemon types not in `crates/protocol`
- Ingest socket wire format beyond what the shim uses

### CLI Surface (v1)

| Command | Description |
|---|---|
| `bowerbird install` | Write hook to `~/.claude/settings.json`, start daemon |
| `bowerbird uninstall` | Remove hook, stop daemon |
| `bowerbird start` / `bowerbird stop` | Daemon lifecycle without touching hook config |
| `bowerbird status` | Daemon liveness + version |
| `bowerbird replay <file>` | Replay a JSONL event file through the daemon's pub/sub path |
| `bowerbird export <session-id>` | Export a session's events from SQLite to JSONL replay format |
| `bowerbird auth token` | Print the current bearer token from keychain or configured fallback |

Replay file format is wire-format event envelopes in JSONL — no separate schema. Bundled demo fixtures ship with the binary for the Quickstart. `bowerbird export` enables capturing real sessions for replay and debugging.

### Migration Guide

**Within v1.x — schema guarantee (hard):**
Additive-only on the wire surface: new fields on outbound types, new topics, new endpoints. No fields removed, no types changed. Tools built for v1 work on any v1.x daemon release without changes.

**Within v1.x — behavioral compatibility (best-effort):**
The schema guarantee does not extend to all behavioral details. Bug fixes and security fixes may change observable behavior. Two explicit carve-outs:
- **Security fixes:** may change behavior without a v2 bump, always.
- **Bug fixes where the old behavior was incorrect per the spec:** may change behavior; must be noted in `protocol-changelog.md` with `type: behavioral`.

`protocol-changelog.md` entries use a structured header: `type: schema | behavioral | security`. CI validates that any change to `crates/protocol/src/*.rs` produces a corresponding changelog entry.

**v1 → v2 (breaking changes):**
v2 ships as a parallel `protocol@v2` module tree. v1 endpoints continue during a documented transition window. The changelog entry specifies the transition duration and the migration checklist — exact fields or behaviors that changed and what tools must update.

### Documentation Requirements (v1 Deliverables)

The reader path through docs must exist at launch:

| Document | Scope |
|---|---|
| Quickstart | Works against `bowerbird replay` with bundled demo fixture — no Claude Code required. Covers install → replay → run reference example → see output. Forward-pointer to `presenter-authoring.md` at the success moment. |
| `docs/presenter-authoring.md` | How to build a tool that consumes the WebSocket stream: connect, subscribe, handle state/event/dropped frames, snapshot on reconnect. Language-agnostic with TypeScript examples. |
| `docs/protocol.md` | Wire format reference: all endpoints, frame types, topic syntax, auth contract, ingest socket. Machine-readable enough to generate client stubs from. |
| `docs/cookbook/` | v1 ships at least three entries, each a self-contained directory under `docs/cookbook/<name>/` containing prose README + runnable code. Must exist at launch (not a post-launch deliverable). Phase 3 target: seven entries (the three V1 entries plus `session-glance`, `transition-alerts`, `live-board`, `tmux-ambient`), each validated by daily use before landing (FR40 through FR43). |
| `docs/protocol-changelog.md` | Structured changelog; CI-enforced; required entry for any `crates/protocol/src/*.rs` change. |

### Code Examples

Reference examples live in `docs/cookbook/<name>/`, CI smoke-tested against a live daemon. Each cookbook entry is a self-contained directory: the README explains what it is and how to apply the pattern; the colocated `src/`, `package.json`, and `tsconfig.json` are the runnable canonical code. There is no separate cookbook surface to keep in sync: the prose and code share the same directory, so a developer changing the code and updating the README is one PR, not a coupling problem.

**V1 reference examples:**

1. **`state-session-fanout`** (`docs/cookbook/state-session-fanout/`): subscribes to `state.session.*` (wildcard), routes events to per-session state, handles new sessions appearing mid-subscription. Demonstrates the core fan-out pattern every non-trivial tool needs.
2. **`rest-cursor-pagination`** (`docs/cookbook/rest-cursor-pagination/`): reads `events.*`, renders tool-call history. Shows cursor-based pagination via REST.
3. **`dropped-frame-recovery`** (`docs/cookbook/dropped-frame-recovery/`): demonstrates snapshot-on-connect + `dropped`-frame detection + REST re-fetch. The resilience pattern every tool that runs for more than a few minutes needs.

All three examples must run against `bowerbird replay` with bundled fixture files.

## Functional Requirements

### Hook Integration & Event Capture

- FR1: The shim can capture Claude Code hook events and deliver them to the daemon without adding perceptible latency to Claude Code's operation
- FR2: The shim can operate without network timeouts or blocking calls that could delay Claude Code's hook execution
- FR3: Tool builders can install and remove the bowerbird hook from Claude Code's configuration without manually editing configuration files
- FR4: The Claude Code adapter can normalize Claude Code hook payloads into the canonical protocol event format
- FR5: The shim logs failure information to a dedicated log file and stays stdout/stderr-silent on the success and exit-0 (daemon-answered) paths. On an exit-1 failure it additionally emits exactly one `bowerbird: <cause>` line to stderr so the failure is not causeless to Claude Code, appending a fixed `(see the shim log)` pointer (never the env-controlled path) only when the file-log append succeeded (Story 5.10)

### Event Storage & Persistence

- FR6: The daemon can persist incoming events to a local event log atomically with their associated session state projection
- FR7: The daemon can survive unexpected termination without leaving the event log in a corrupt or inconsistent state
- FR8: Tool builders can query the event log with a cursor to retrieve events from a specific point forward
- FR9: The daemon exposes the oldest available event identifier so tools can detect whether they have missed events

### Real-Time Event Streaming

- FR10: Tool builders can subscribe to a stream of agent activity events over a persistent connection
- FR11: Tool builders can filter their subscription to specific topics at session, source, or global scope
- FR12: Tool builders can subscribe to activity across all sessions simultaneously using a wildcard subscription
- FR13: The daemon can notify subscribed tools when new sessions appear without requiring reconnection
- FR14: The daemon can notify a tool when it has missed events due to slow consumption, including how many events were missed
- FR15: The daemon can deliver a current-state snapshot to a connecting tool without requiring a separate query
- FR16: Multiple tools can connect to and receive the same event stream simultaneously without affecting each other
- FR17: The daemon can send a shutdown notification to connected tools before terminating

### Event Query & History

- FR18: Tool builders can retrieve a list of known agent sessions
- FR19: Tool builders can retrieve the current projected state of a specific session
- FR20: Tool builders can retrieve paginated event history for a session from a given cursor position
- FR21: Tool builders can retrieve per-session event statistics
- FR22: Tool builders can check daemon liveness without authenticating
- FR23: Tool builders can check daemon readiness — including storage and broadcaster state — without authenticating

### Session Tracking

- FR24: The daemon can track multiple concurrent agent sessions, distinguishing them by both source and session identifier
- FR25: The daemon can maintain a current-state projection per session, updated in the same operation as event storage
- FR26: The daemon can tolerate missing hook events without entering an inconsistent or stuck state

### Installation & Configuration

- FR27: Tool builders can install bowerbird without a Rust development environment using prebuilt binaries from GitHub Releases
- FR28: Tool builders can install bowerbird from source using the Rust toolchain
- FR29: Tool builders can start and stop the daemon independently of the Claude Code hook configuration
- FR30: Tool builders can check the daemon's current status and version from the command line

### Developer Tools & Experience

- FR31: Tool builders can replay a recorded event sequence through the daemon's full pub/sub path without a live Claude Code session
- FR32: Tool builders can export a real session's events to a file for replay or debugging
- FR33: Tool builders can access reference implementations demonstrating event subscription, multi-session fan-out, and dropped-frame recovery
- FR34: Tool builders can run all reference implementations against bundled fixture data without a live agent session
- FR35: Tool builders can access documentation covering: quickstart (no live agent required), tool-building guide, protocol reference, and recipe cookbook

### Protocol & Compatibility

- FR36: The protocol guarantees that tools built against v1 continue to work on any v1.x daemon release without modification
- FR37: The daemon accepts inbound events via a socket accessible only to the current OS user
- FR38: Tool builders can authenticate REST and WebSocket connections using a bearer token
- FR39: Tool builders can access structured changelog information identifying the type and nature of any protocol changes between releases

### Attention Surfaces & Cookbook (Phase 3, added 2026-08-02)

All Phase 3 FRs are presenter-layer deliverables (cookbook entries): no new daemon or protocol capability is required, which is itself the Phase 3 bet. Each entry follows the cookbook's canonical shape (self-contained directory, five-section README, runnable TypeScript, CI typecheck + smoke) and lands only after it has been validated by real daily use.

- FR40: Tool builders can print a one-shot glance of every live session's state from the command line, grouped by repository (derived from `cwd`), with per-session age (derived from `started_at`), via the `session-glance` cookbook entry consuming REST `GET /sessions?state=`
- FR41: Tool builders can receive OS notifications when a session transitions into `WaitingInput` (blocked) or from `Working` to `Idle` (turn done), with the triggering reason joined from the events stream (e.g. which tool's permission prompt), via the `transition-alerts` cookbook entry; the entry documents the per-key map pattern that absorbs the snapshot burst silently and stays correct across reconnects
- FR42: Tool builders can run a long-lived live board showing per-session state and event flow across all sessions, via the `live-board` cookbook entry (the bowerbird-deck archival that accompanies this is a project task recorded in Phase 3 Scope and the addendum, not part of this capability)
- FR43: Tool builders can surface a session-attention count in the tmux status line via the `tmux-ambient` cookbook entry, wired from the same query as `session-glance`
- FR44: Every Phase 3 attention surface names WHERE the session is: at minimum the repository derived from `cwd`; optionally the tmux pane, derived presenter-side by matching the session's `last_pid` into tmux pane PIDs (the substrate ships location facts, never location interpretation)

**Open question carried into Phase 3 (not an FR):** the `Reaction` classification (`tool-reactions.toml`) has had zero consumers since Epic 1 and sits in tension with the restraint doctrine (the daemon "collects faithfully and routes without opinion", yet ships a classification layer). No Phase 3 journey demanded it. Decide during the cycle: keep as the one sanctioned interpretation hook (and give it a consumer), or deprecate it with an ADR. Do not force a consumer into existence to dodge the decision.

## Non-Functional Requirements

### Performance

- NFR1: The shim must add no more than 5ms at the p95 percentile to Claude Code's hook execution time (hard constraint; benchmarked from day one via `shim/benches/hot_path.rs`)
- NFR2: The daemon must introduce no perceptible lag under normal single-developer load on a modern laptop; performance is tuned when evidence warrants, not speculatively
- NFR3: The daemon must be ready to accept connections within 2 seconds of cold start on reference hardware; verified via the health endpoint (FR22)

### Reliability & Data Integrity

- NFR4: The event log is unbounded for V1; the documented V1 escape hatch is deleting or truncating `~/.bowerbird/bower.db` directly; a dedicated `bowerbird gc` command for managed truncation is post-V1
- NFR5: When the host filesystem is full (ENOSPC), the daemon logs the drop at error level and closes the ingest connection; the shim treats any write error as fire-and-forget and exits 0 without blocking Claude Code
- NFR6: The event log survives unexpected daemon termination; any event acknowledged to the shim is durable on restart (guaranteed by WAL-mode atomic writes)
- NFR7: The daemon accepts unbounded event ingest rate in V1 for single-developer workloads; no rate limiting or burst protection; this is a documented design limitation

### Compatibility & Portability

- NFR8: Prebuilt binaries target currently-supported macOS versions on both x86_64 and arm64
- NFR9: Linux prebuilts target glibc-based distributions; musl deferred post-V1
- NFR10: The `cargo install` path requires only the Rust stable toolchain; no nightly features

### Security

- NFR11: The daemon bearer token is a UUID4 value, stored in the system keychain (macOS Keychain / Linux Secret Service) and retrieved via `bowerbird auth token`
- NFR12: Fallback order when keychain unavailable: (1) environment variable, (2) on-disk config file in `~/.bowerbird/`; fallback mechanism is documented
- NFR13: If no token is resolvable via any fallback path, the daemon exits non-zero with a human-readable error to stderr
- NFR14: Token rotation requires a daemon restart; the daemon reads the token once at startup and does not hot-reload it
- NFR15: The shim failure log is created with mode `0600` regardless of the process umask

### Operability

- NFR16: The daemon logs at error level by default; `-v` and `-vv` flags expose progressively more detail; each log line follows the format `<ISO8601 timestamp> <LEVEL> <message>`; structured JSON logging deferred to V2
- NFR17: On unexpected crash, the daemon writes crash information to `~/.bowerbird/`; no external crash reporting
- NFR18: A daemon metrics endpoint is deferred until usage patterns justify it; health and readiness endpoints (FR22, FR23) are sufficient for V1

### Protocol & API Stability

- NFR19: No breaking changes to the REST or WebSocket protocol within any v1.x release series; tools built against v1.0 continue to work on any v1.x daemon without modification (anchors FR36)

### Implementation Constraints

- NFR20: The daemon's ingest socket listen backlog is at minimum 128; the shim exits non-zero on `ECONNREFUSED` or socket-not-found (daemon unreachable), and exits 0 on mid-write errors (transient daemon issues, backpressure)
- NFR21: The daemon auto-migrates the SQLite schema on startup; migration failures are fatal with a human-readable error to stderr
- NFR22: The V1 event log schema includes a timestamp column on all event rows to support future event-log management without schema changes
