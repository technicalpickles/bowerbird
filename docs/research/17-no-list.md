# Out-of-scope register (the "no-list")

This document lists what `bowerbird` deliberately does not do, with the reasoning for each entry and — where applicable — the right pathway for getting that capability another way.

The list is split into two sections:

- **Never** — out of scope by design. These are architectural commitments. Reversing one would change what the project is.
- **Not yet** — in scope conceptually, but deliberately deferred. These wait for either evidence that they're needed or for the right milestone. Contributions welcome, but file a discussion first.

If you're considering a contribution, read this first. If your idea is on the "never" list, the project is probably not the right home for it. If it's on the "not yet" list, file a discussion describing your use case and what evidence supports building it now.

---

## Never

### Synchronous human-in-the-loop approval

The substrate emits `events.permissionRequest` events with the full native payload (question text, available options, tool input details). Presenters can subscribe and render approval UI on wearables, lamps, terminals, or anywhere else.

What it doesn't do: accept the user's *answer* and route it back to the originating agent. The substrate is fire-and-forget; the hook shim exits in under 5ms and never blocks Claude.

**Why never:** Synchronous wait points turn the substrate into a coordination service with very different reliability requirements. A wearable approval interface that misses a tap stalls a real coding session. The substrate would need request-response correlation, timeout handling, fallback policies, and an authority model for "which presenter wins when two answer simultaneously." That's a different project.

**The pathway:** Build a sibling service that subscribes to `events.permissionRequest.*` from the substrate, handles approval flow with its own state machine, and writes answers back to the agent through whatever mechanism that agent supports (Claude has no remote approval API today; this is a hard problem at the agent layer, not at the substrate layer).

### Tool-call blocking and policy enforcement

The substrate observes tool calls. It does not veto them. The hook shim always exits 0; it never returns the `decision: deny` payload that would block a tool from running.

**Why never:** Vetoing requires the shim to wait for a policy subscriber to weigh in, which immediately blows the 5ms exit budget. Policy enforcement also needs deterministic ordering, authority resolution, audit trails — none of which the substrate's pub/sub model provides.

**The pathway:** Install your policy hook in parallel with the substrate's hook. Both fire on `PreToolUse`. The substrate observes (and exits 0); your policy hook decides (and exits 2 to block). Tools like `maybe-dont` and Claude's own security skills work this way today; the substrate doesn't compete with them.

### Application-level concepts: personas, voices, sprites, moods, color palettes

The substrate exposes `agent_type` (preserved verbatim from the source) and `current_state` (the 11-value reaction enum). It does not map them to anything.

If you want a researcher subagent to have a calm voice and a debugger subagent to sound urgent: presenter responsibility. If you want a particular reaction to render as red on your Stream Deck: presenter responsibility. If you want sessions on `github.com/foo/bar` to be blue: presenter responsibility (using `remote_url`).

**Why never:** Eight surveyed pet/sprite/dashboard tools each made different choices. The substrate doesn't pick. The `14-activity-survey.md` finding generalizes: when a presenter concern varies across tools, the substrate exposes the raw signal and stays silent on interpretation.

**The pathway:** Build whatever mapping you want, client-side. Publish it as a presenter package. The substrate's job is making sure your presenter has clean data to map *from*.

### Acting as an agent runtime

The substrate observes Claude Code, Codex CLI, Gemini CLI, Cursor CLI, OpenCode, and others. It does not spawn, control, or replace them.

**Why never:** Being agent-neutral and observation-only is the substrate's value. Building a runtime would put us in competition with the tools we observe and dilute the focus that makes the substrate useful.

**The pathway:** If you want a coding agent, use one of the ones the substrate already supports. If you want to write a new coding agent, that's a different project — and we'll happily write an adapter for it once it's running.

### On-disk presenter state

The daemon stores events, sessions, agents, attachments, session_usage. It does not store presenter UI state — which session is focused in some terminal-grouper, which sprite has been clicked, what color a user set their lamp to last week.

**Why never:** Two presenters watching the same daemon should not interfere with each other. The substrate is the source of truth for *what happened*; presenters are the source of truth for *how they show what happened*. Mixing those creates ordering problems that aren't worth solving here.

**The pathway:** Persist your presenter state wherever makes sense for your presenter (localStorage, your own SQLite, a config file). The substrate's pub/sub will faithfully replay state changes so your presenter can rebuild its view on reconnect.

### Computing activity rate as a substrate-side projection

The substrate exposes `last_event_at` on every session row and `events.session.<id>.*` subscriptions for the firehose. It does not maintain a sliding-window event count, a leaky-bucket flow scalar, or any other rate-based aggregation.

**Why never:** The `14-activity-survey.md` study of 8 pet/sprite/dashboard tools found that only 1 (tamagotchi) actually wants a window-based rate. The other 7 either use `last_event_at` alone, derive their own rate from the event stream (~6-15 lines client-side), or don't use a rate at all. Daemon-side rate counters would be single-tool convenience.

**The pathway:** Subscribe to `events.session.<id>.*` and maintain whatever rate model you want in the presenter. Three common patterns are documented in `docs/cookbook/computing-activity.md`.

---

## Not yet

### Cross-machine pub/sub

The broker binds to `127.0.0.1` only. The wire protocol is local. There's no LAN reachability, no authentication model that survives multi-host, no clock-skew handling.

**Why not yet:** AgentDeck-class bridges that need LAN distribution are real and worth supporting eventually. But solving auth, transport reliability, NAT traversal, and discovery is enough work that it would block MVP. For now, those bridges build their own relay that consumes from the localhost substrate.

**Pathway when it's time:** A separate `relay` crate that subscribes locally and re-publishes over TLS-authenticated WebSocket. Probably M8 or later — file a discussion if you have a use case that makes this urgent.

### Multi-agent support (Codex, Gemini, Cursor, OpenCode, others)

MVP is Claude Code only. The abstraction is designed for multi-agent (per `09-multi-agent-support.md` and `10-multi-agent-tool-patterns.md`) but only one adapter ships in v1.

**Why not yet:** Shipping the abstraction with two adapters before validating one is overkill. M2 (Codex) is the second adapter, scheduled after MVP success criteria are met. Each adapter is ~500 lines of code per `10-multi-agent-tool-patterns.md`; doing five before validating one is wasted work.

**Pathway when it's time:** M2 adds Codex (validates the tier-1 abstraction). M4 adds Gemini and Cursor (proves the convergent Claude-style hook surface). M5 adds OpenCode plugin provider (validates tier-2 ingest). Contributor PRs for adapters are explicitly welcomed and reviewed faster than core PRs. See `docs/adapter-authoring.md` once it exists.

### Statusline composer

Multiple tools want to contribute statusline segments. Today they collide — only one can be the configured statusline command at a time. A composer would let multiple presenters register segments that get composed per tick.

**Why not yet:** Statusline is pulled (Claude invokes per tick), not pushed. It's a fundamentally different consumption model from the pub/sub the rest of the substrate is built around. Shipping it in MVP would triple the surface area for marginal benefit. M6 in the milestone plan.

**Pathway when it's time:** A `statusline` subcommand the user wires Claude to invoke. Segment providers register via WS or local socket. Composition per tick with caching. Cookbook recipe in `docs/cookbook/statusline.md` once shipped.

### TranscriptProvider for hook-less agents (Aider, Copilot CLI)

The substrate's three provider types (HookProvider, PluginProvider, TranscriptProvider) are designed in `10-multi-agent-tool-patterns.md`. Only HookProvider ships in MVP. PluginProvider validates at M5 with OpenCode. TranscriptProvider is M7+.

**Why not yet:** Aider has no hook system; its only observable surface is a markdown chat history file. The state vocabulary is coarse (~4 values instead of 11). Building this would gate the design on the lowest-fidelity ingest path, which would inappropriately constrain richer adapters. Better to validate the abstraction with tier-1 and tier-2 first.

**Pathway when it's time:** M7 or later. Implements `state.session.<id>.current_state` with a smaller subset of the reaction enum (`reaction_enum_subset` on the source's capabilities, per `11-design-sketch-v2-1.md`). The capabilities surface degrades gracefully for tier-3 adapters.

### MCP-based ingest as a fourth provider type

Some tools may prefer to push events via MCP rather than installing hooks or shipping plugins. The wire protocol could trivially accept this; it's just another POST source.

**Why not yet:** No inventoried tool currently asks for it. Building it before there's a concrete use case risks shipping an unused surface. The substrate's POST ingest endpoint already accepts events from any source that speaks the protocol; an MCP wrapper would be a thin shim.

**Pathway when it's time:** A minimal MCP server crate (`bowerbird-mcp`) that exposes a single `emit_event` tool and forwards to the daemon. Probably <200 lines. File a discussion if you have a use case.

### Durable subscriptions with disk-backed queues

Presenters that want guaranteed delivery across daemon restarts (think: a cost-tracker that absolutely must not miss a single `sessionEnd` event) need durable subscriptions with delivery offsets.

**Why not yet:** Disk-backed queues add complexity for a benefit no inventoried presenter has demonstrated need for yet. Ephemeral pub/sub with snapshot-on-reconnect handles the common case (presenter restarts, re-subscribes, gets fresh snapshot, resumes). For the rare presenter that genuinely needs every event delivered exactly once, polling `GET /sessions/:id/events?since=<cursor>` with a stored cursor is sufficient.

**Pathway when it's time:** Opt-in `durable: true` on subscribe. Daemon tracks per-subscriber delivery offset in SQLite. Resume on reconnect using the last acked event_id. Probably <500 lines of additional daemon code; gated by evidence of real need.

### Sweep and abandoned-session detection

Sessions that go silent for hours are still marked `lifecycle: live` until an explicit `SessionEnd` arrives. A sweep loop could promote them to `lifecycle: abandoned` after a configurable timeout.

**Why not yet:** Presenter-side idle detection (using `last_event_at`) covers the same need without daemon-side policy. The substrate exposes the signal; presenters interpret it. If multiple presenters want consistent "abandoned" semantics, that's when to fold the logic into the daemon.

**Pathway when it's time:** Configurable timeouts per source (Pixel Agents uses 10 minutes for cleanup, Outworked uses 10 minutes for stuck). Background tokio task; runs every minute; promotes `live` → `abandoned` based on `last_event_at` age. Probably <100 lines.

### Capability extensions declared by community packages

The current `capabilities.yaml` model is daemon-defined (the keys are known to the daemon). Third parties could in theory declare additional capability flags that their adapters support and that presenters could query against — e.g., `has_voice_announcement_support`, `has_streaming_diff_view`.

**Why not yet:** The capabilities surface should bake in MVP before being extended. Once we have 2-3 adapters and 5-10 presenters consuming capabilities, the right shape for community extensions will be obvious. Forcing it now risks designing an over-general surface.

**Pathway when it's time:** Daemon passes unknown capability keys through verbatim; presenters that don't recognize them ignore them. The `capabilities.yaml` namespace becomes implicitly open. Documented contract: official keys are `has_*`; community keys should be `<vendor>_has_*` to avoid collisions.

### Persistent agent identity across daemon restarts

Today the `agents` table is in SQLite, so identity does persist. But the relationship between an `agent_id` (Anthropic-generated UUID) and a logical "Researcher" subagent type is reconstructed on every daemon restart from event history. A more robust model would explicitly track persistent agent identities.

**Why not yet:** Reconstruction from events works for current presenters. No inventoried tool has shown a use case where it doesn't. PAI's voice mapping keys on `agent_type`, which is already first-class.

**Pathway when it's time:** If a presenter needs cross-session agent identity (think: "this is the same Researcher that worked on this PR yesterday"), the substrate could expose `agent_type_history` per repo. Speculative until needed.

### Web UI for the daemon

A read-only dashboard at `http://localhost:9876/` showing live sessions, events, and state. Useful for debugging the substrate itself.

**Why not yet:** Not the substrate's job to provide UI. But a debug-only web view is plausibly useful for the maintainer and contributors. Low priority; would compete with cookbook examples for "show how to consume the data."

**Pathway when it's time:** Either a tiny embedded HTML page that subscribes to the same WebSocket every other presenter uses, or a separate companion package. Probably <300 lines of vanilla JS. If built, ship as `examples/debug-dashboard/` rather than baking into the daemon binary.

---

## How to propose adding to "not yet" or moving from "not yet" to in-development

The no-list is reviewed quarterly. The maintainer's question for each "not yet" item: does it still feel right to defer, given what's been learned?

For new "not yet" items: file a discussion describing the use case, the alternatives you've considered, and what evidence supports building this rather than building it as an extension. The maintainer either accepts it onto the list with a "not yet" status or onto the "never" list with reasoning.

For promoting a "not yet" item: similar — file a discussion. What changed? What concrete presenter wants this? What's the scope? If the answer is solid, the item moves into milestone planning.

The no-list is not a roadmap. Items here may stay deferred forever. The point is to set expectations, not to make promises.