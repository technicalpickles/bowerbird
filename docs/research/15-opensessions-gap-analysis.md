# opensessions: gap analysis and contribution viability

opensessions is the closest existing project to what the substrate design proposes. Both are agent-state aggregators with per-agent adapters, both target multi-agent CLIs, and both have first-class `AgentWatcher`-style abstractions. The honest question this document answers: how much of our design already exists in opensessions, where do they materially differ, and how much effort would it take to contribute changes that close the gap vs. build standalone?

The short answer: opensessions has done 60-70% of the substrate's job, but the 30-40% gap is structural enough that the work to close it is roughly comparable to building standalone — with one critical complication: **0 PRs have been merged in opensessions' history**, against 1 currently open and 5 open issues. This isn't a project taking contributions yet.

The rest of this document is what that gap actually contains.

## What opensessions has that matches the design

### Per-agent watcher abstraction (matches)

opensessions' `AgentWatcher` interface from `CONTRACTS.md` is essentially identical to what we sketched:

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

Five built-in watchers exist today (Amp, Claude Code, Codex, OpenCode, pi). Three ingest models in use: filesystem watch + JSONL tail (Claude, Codex, pi), SQLite polling (OpenCode), cloud REST + WebSocket (Amp). Documented in `CONTRACTS.md` with a minimal example for new watcher authors.

This matches our HookProvider / PluginProvider / TranscriptProvider split conceptually, though opensessions doesn't classify them — they all implement the same interface and the ingest detail is internal.

### Canonical event shape (mostly matches)

```ts
interface AgentEvent {
  agent: string;
  session: string;
  status: AgentStatus;
  ts: number;
  threadId?: string;
  threadName?: string;
  unseen?: boolean;
  paneId?: string;
  liveness?: AgentLiveness;
}

type AgentStatus = "idle" | "running" | "tool-running" | "done"
                 | "error" | "waiting" | "interrupted" | "stale";
type AgentLiveness = "alive" | "exited" | "unknown";
```

Eight-value status enum, three-value liveness, separation of `threadId` (instance key) from `agent` (watcher identifier). Liveness is per-event (not per-attachment), but that's a small structural difference.

Our 11-value reaction enum (from OpenPets) is broader. opensessions' enum is narrower because it's optimized for "what does the session row show in the sidebar," not for "what should the pet sprite do." Both are valid; ours subsumes theirs (`thinking`, `editing`, `running`, `testing`, `waving`, `success`, `celebrating` are absent in opensessions because the TUI doesn't need them).

### Tracker / projection layer (matches)

`AgentTracker` is opensessions' equivalent of our projection. It:

- Keys instances by `agent:threadId` when `threadId` exists, else by `agent`
- Stores per-session, per-instance `AgentEvent`s
- Computes a per-session aggregate status using a priority order (`tool-running` > `running` > `error` > `stale` > `interrupted` > `waiting` > `done` > `idle`)
- Tracks unseen state per instance, derives session-level unseen
- Prunes stale `running` events after 3 minutes
- Prunes seen terminal events after 5 minutes
- Maintains last 30 event timestamps per session (for the activity-timestamp display)

The aggregate-status priority is something we hadn't pinned down explicitly. opensessions has thought it through.

### Multi-watcher architecture (matches)

The server merges sessions from all registered providers into a single state payload. Sessions from Amp, Claude Code, Codex, OpenCode interleave correctly. The mux abstraction is separate from agent watchers, so the same agent events are routed to the right tmux session regardless of which agent produced them.

### Programmatic POST API (matches partially — write side only)

`POST /set-status`, `POST /set-progress`, `POST /log`, `POST /clear-log`, `POST /notify` on `127.0.0.1:7391`. Lets external scripts and CI push metadata into session rows. Status/progress are in-memory only; logs are capped at 50 entries per session.

This is roughly equivalent to our `worktreeCreate`/`worktreeRemove` events, but for arbitrary annotation rather than orchestrator events.

### Session metadata (matches partially)

Sessions carry:
- name, createdAt, dir, branch, dirty, isWorktree
- panes count, ports detected, windows
- agentState (aggregated), agents (per-instance), eventTimestamps (last 30)
- metadata (status pill, progress, recent logs)

Branch is derived (`git rev-parse --abbrev-ref HEAD`). Working directory and dirty state come from `git status`. There's an in-process git cache with 5s TTL.

## What opensessions doesn't have that the design requires

### Persistence — they have none

opensessions is **entirely in-memory.** No SQLite, no event log. The only on-disk state is:
- `session-order` (a few KB of JSON for preferred session ordering)
- Plugin/mux config (also JSON)

Events flow through, get tracked, and expire from memory after 3-5 minutes. The history disappears at server restart. There's no way to ask "what events happened on this session over the last hour" because they're gone.

Our design's SQLite event log with `event_id` ordering, `since=<cursor>` resume, and bounded retention is absent. **This is the biggest gap.**

### Pub/sub topics — they broadcast full state

The WebSocket protocol is one channel called `"sidebar"`. On every state change, the server publishes the **entire `ServerState` payload** to all subscribers via `server.publish("sidebar", JSON.stringify(lastState))`.

There are no topics, no subscriptions, no per-presenter filtering. Every TUI client gets the full session list with every update (microtask-coalesced but otherwise complete). For 5 sessions with metadata, that's roughly 3-10 KB per broadcast, fired on every state transition across any session.

Our design's two-channel pub/sub (`events.*` and `state.*`), hierarchical topics, snapshot-on-subscribe, bounded queue with drop-frame backpressure — none of this exists. The opensessions WS is fundamentally a "current state mirror," not an event stream.

A consequence: a presenter that only cares about one session (or one type of event) still receives every byte of state from every session on every change. Doesn't scale to 50 sessions or many presenters.

### Event log queryable via REST — absent

opensessions has no GET endpoints. None. Only POST (write metadata) and WebSocket (read state mirror). There's no way to query "what tools did session X use today" or "give me the last 100 events filtered by kind." That information isn't stored.

Our design's `GET /sessions`, `GET /sessions/:id/events?since=<cursor>`, `GET /sources`, `GET /remotes` surface doesn't exist.

### Reaction enum projection — narrower vocabulary

opensessions' 8-value status enum (`idle / running / tool-running / done / error / waiting / interrupted / stale`) maps to TUI presentation needs. Our 11-value reaction enum (with `thinking, editing, running, testing, waving, success, celebrating`) is for pet/sprite/lamp presenters that want more variety.

Adding the missing values is mechanical — but it would change the contract of all five existing watchers. opensessions' watchers don't emit `editing` because the TUI doesn't display it.

### Hook router for shipping a unified shim — absent

opensessions doesn't install hooks into `~/.claude/settings.json`. The Claude Code watcher reads JSONL transcripts directly via `fs.watch`. This means:

- No collision with other tools' hooks (good)
- Higher latency — JSONL polling at 2s vs. zero-latency hook firing (less good)
- The hook collision problem the substrate solves doesn't even exist for opensessions because they sidestepped it

Adding a hook router to opensessions would be net-additive: hooks for fast updates, JSONL tail as fallback (like pixel-agents' dual mode).

### Persistent agent identity — absent

When the OpenCode watcher polls SQLite, it sees an `agent` field on each message (like "librarian", "Sisyphus"). It surfaces it but doesn't normalize: each call's `agent_type` is per-message metadata, not a tracked entity. Our `agents` table with `agent_type`, `parent_agent_id`, `started_at`, `ended_at` is absent.

For Claude Code subagents specifically, opensessions tracks `threadId` per agent instance but doesn't model `subagent_type` as a first-class field. PAI-style voice-per-subagent presenters would have to parse it out of `threadName`.

### Worktree / remote URL derivation — partial

opensessions derives `branch` and `isWorktree` (boolean) but **not** `repo_root` (canonical), `worktree` (path), or `remote_url`. The test case from `13-test-cases.md` of "group sessions by git remote" doesn't work today — every session shows its tmux session name and branch, but nothing ties together multiple worktrees of the same repo.

This is mechanical to add but not present.

### Capabilities surface — absent

`AgentCapabilities` per source (from AgentDeck's matrix) isn't exposed. opensessions treats all watchers as equivalent; presenters can't negotiate features per agent. For its purpose (a sidebar showing status), this is fine. For our purpose (a substrate that multiple kinds of presenters consume), it's a gap.

### Activity rate — absent (correctly)

opensessions stores last-30 event timestamps and shows them in the detail pane, but doesn't compute or expose a rate. This matches our v2.1 decision after `14-activity-survey.md` — let presenters compute their own. opensessions reached the same conclusion independently.

### Multi-host, durable subscriptions, statusline composer — all absent

These are all explicitly out of scope in our v1/MVP too. The fact that opensessions doesn't have them isn't a gap; it's the right call.

## What opensessions has that the design doesn't

A handful of things worth noting where opensessions has solved a problem we hadn't addressed:

### Mux abstraction with capability gates

opensessions' `MuxProviderV1` with optional `WindowCapable`, `SidebarCapable`, `BatchCapable` capabilities is exactly the pattern we lifted for agent capabilities. Their tmux provider implements all four; their zellij provider is incomplete. Type guards (`isWindowCapable`, etc.) gate features per provider. This is a more mature version of what we propose for agent capabilities.

### Unseen-state semantics

Per-instance unseen tracking, derived to session level. When an agent transitions to a terminal state (`done`/`error`/`interrupted`) on a session the user isn't currently looking at, the row gets an unseen marker. When the user focuses the session, unseen clears. The substrate design has no equivalent — it doesn't model "what the user has seen."

This is arguably presenter concern (each presenter tracks its own seen-state per consumer), but opensessions made it server-side because the unseen marker shows up identically across multiple TUI clients viewing the same daemon.

### Agent ownership / canonicalization

`canonicalizeAgentEvent()` resolves a thread-bound event to its actual pane and session when those are derivable, even if the watcher emitted a different `session` initially. Handles the case where two tmux sessions could plausibly own the same agent thread (cwd matches both). The substrate design hadn't grappled with this.

### Mux-native hooks for state changes

opensessions registers tmux hooks (`session-created`, `session-renamed`, `client-attached`, etc.) so the server gets notified instantly when tmux state changes — no polling. The mux provider's `setupHooks()` method handles installation; `cleanupHooks()` reverses it.

Our design assumed terminal attribution would be derived from env-var fingerprints at agent-start. opensessions' approach is more reactive and more accurate for tmux specifically.

### Stale-process detection

`tracker.pruneStuck()` runs on every broadcast, marking events as `stale` if they've been `running` for too long without updates. The substrate design has `liveness` for this but doesn't explicitly model "stuck" — that was deferred to presenter side. opensessions does it server-side because the TUI wants a visible "stale" badge.

### Per-thread unseen markers

When multiple Claude Code threads run in the same tmux session (a main session plus subagents), opensessions tracks unseen state per thread. The session row shows aggregated unseen, but the detail panel shows per-thread.

## Material differences in worldview

A few places where opensessions and the substrate design have **incompatible** views, not just gaps:

### opensessions assumes a TUI consumer; the substrate assumes many

opensessions' WebSocket is bound to its TUI client. Other tools can POST metadata in, but they can't read state out (no GET endpoints, no event stream API). The architecture is "one server, one type of consumer, many sessions."

The substrate's architecture is "one daemon, one type of session, many presenters." It's the inverse — the daemon serves data to a heterogeneous set of presenters, each of which only cares about a slice.

Adding pub/sub topics + an event-log REST surface to opensessions would change the architecture from N:1 to N:M. That's a substantial reframing.

### opensessions persists nothing; the substrate persists everything

Events are ephemeral in opensessions. They flow through the tracker, drive state updates, and expire after 3-5 minutes. Restart the server and history is gone.

The substrate's event log is the durable record. Sessions, events, agents, attachments, session_usage all live in SQLite. A presenter that connects after the fact (think: claude-receipts running once per day to generate cost summaries) can read history.

Adding SQLite persistence to opensessions wouldn't change its TUI behavior — but it would change what the project *is*. The TUI is currently a side-effect of an in-memory state machine; persistence would make the database the source of truth and the state machine a cache.

### opensessions models a sidebar; the substrate models a system

The metadata API (`/set-status`, `/set-progress`, `/log`) is shaped like "things you can pin to a sidebar row." Tones are visual (`info`/`success`/`warn`/`error`). Logs are capped at 50 entries for display reasons. Status text truncated at 100 chars.

The substrate's events have no display semantics. A `permissionRequest` payload contains the full native data; it's up to a presenter to truncate, color, or pin it. The substrate doesn't take a position on visual presentation.

These aren't incompatible per se — opensessions could add a parallel raw-event API alongside the metadata API — but the metadata API's existence shows the design center is "TUI sidebar," not "general substrate."

## How much effort to contribute the gap

The pure code-change estimate for closing the gap, assuming the maintainers would accept the PRs:

| Change | Effort | Risk |
|---|---|---|
| Add `repo_root`, `worktree`, `remote_url` derivation alongside `branch` | ~half day | Low — straightforward git wrapping |
| Add `last_event_at` (already implicit), expose in `SessionData` | ~hours | Low — already partially there |
| Extend `AgentStatus` enum with `thinking`, `editing`, `testing`, etc. | ~half day in interface; ~half day per watcher to populate | Medium — semantic decisions in each watcher |
| Add a hook router for Claude Code (shim + settings.json install) | 2-3 days | Medium — new dependency, new code path, has to coexist with JSONL watcher |
| Add `subagent_type` / `agent_type` as first-class field on `AgentEvent` | ~half day in shape; ~day per watcher to populate | Medium — depends on what each agent actually exposes |
| Add `capabilities` per source via a registry | 1-2 days | Low — straightforward addition |
| Add SQLite persistence (events, sessions, agents, attachments tables) | 1-2 weeks | High — fundamental architectural change, has to coexist with in-memory tracker |
| Add hierarchical pub/sub on the WebSocket (subscribe to topics) | 1-2 weeks | High — backwards-compatibility with current "sidebar" channel, full broadcast semantics need to keep working for TUI |
| Add GET endpoints for event log, sessions, sources, remotes | ~week (depends on persistence) | Medium — has to ride on persistence |
| Add snapshot-on-subscribe semantics | ~half week | Medium — depends on pub/sub |
| Backpressure, drop-frame protocol | ~half week | Low — once the pub/sub is in place |
| **Total** | **roughly 5-8 weeks of focused work** | |

For comparison: the substrate MVP plan estimated similar order of magnitude for ground-up Rust (~6-8 weeks for the four load-bearing claims, in `12-mvp-and-milestones.md`).

## The harder question: would the contributions land?

**0 merged PRs over the project's history.** 25 forks, 431 stars, 5 open issues, 1 open PR. This is significant.

A few plausible reads:

- **The project is solo-maintained and the maintainers ship what they want.** The Ataraxy Labs team has a manifesto, a thesis, a stack of related projects (sem, weave, inspect). They have a strong vision. Their commit cadence is regular; they're shipping. They may simply be in "shipping fast, not yet ready for collaboration" mode.

- **They're philosophically aligned but practically protective of architecture.** The manifesto says "the primary user is an agent, not a person." That's compatible with the substrate's framing — but a sidebar TUI is a *human* consumer, and changing the architecture to serve many automated presenters might be off-thesis for them right now.

- **They might say yes to small features and no to architectural changes.** Adding `remote_url` derivation? Plausible PR. Replacing the in-memory tracker with SQLite-backed events? Less plausible. The PR most likely to land is one that doesn't change the shape of the project.

- **The "0 merged PRs" might just be young-project inertia.** The project has been public for months, not years. Maintainers often don't accept external PRs until they have time to review thoroughly.

Without engaging directly (an issue, a discussion, an email), we can't know which of these is operative. The disciplined move is:

1. Open a discussion or issue describing the design intent: "I'm building agent-state infrastructure with goals X, Y, Z. Would opensessions be receptive to additions like persistent events + pub/sub topics, or is that scope-divergent?"
2. Wait for a maintainer signal.
3. If positive, start with the smallest contribution (probably `remote_url` derivation, which is unambiguously useful and architecturally cheap) to test the contribution loop.
4. If negative or no response after ~2 weeks, build standalone with opensessions' patterns as inspiration where applicable.

## The architectural read

Beyond the "would they merge it" question, the more substantive issue: **even if all the contributions landed, the result would be a hybrid project that's heavier than either pure goal.**

opensessions' current design optimizes for "the TUI is fast, the state is current, the server is light." A pet visualizer or a wearable presenter doesn't need 90% of what that gives them; they need a small slice of events delivered cheaply. Layering pub/sub + persistence + event log into opensessions makes the TUI's data path more complex than it needs to be, while still not being clean enough for a presenter that just wants `state.session.<id>.current_state`.

The substrate design starts from "many small presenters" and builds out. opensessions starts from "one TUI client" and would have to be reframed. Contributing the gap is *possible* but not necessarily the right architectural call even if it's accepted.

A more honest framing:

- **opensessions has solved per-agent watching extremely well.** The five watchers represent significant per-agent engineering investment (~500 lines each). Re-implementing them is wasteful.
- **opensessions has not solved many-consumer event delivery.** That's the substrate's contribution.
- **The two could collaborate by sharing the watcher layer** rather than merging codebases.

A possible shape: extract opensessions' `packages/runtime/src/agents/watchers/*` into a standalone npm package (`@agent-watchers/{claude-code,codex,opencode,amp,pi}`) that both opensessions and the substrate consume. Each watcher remains responsible for understanding one agent's format; what *opens* the consumer side (TUI vs. pub/sub) becomes a host concern. This is the "library extraction" pattern that lets two projects share infrastructure without one absorbing the other.

This is a question worth asking the maintainers directly: would they be open to factoring out the watchers as a published package?

## Summary

opensessions has built a very good piece of the substrate. The five watchers, the agent model, the tracker, the canonicalization logic, the mux abstraction with capability gates — these are all worth reusing.

What it hasn't built: durable event log, pub/sub topics, GET endpoints for event querying, hook router for fast updates, per-source capabilities surface, broader reaction enum, agent-type as first-class field, repo grouping by remote URL.

The contribution cost to close the gap is **5-8 weeks of focused work** — roughly the same as building standalone — with high uncertainty about whether the architectural pieces (SQLite persistence, pub/sub) would be accepted given the project's 0/many PR merge ratio and tight TUI-centric focus.

The pragmatic path forward:

1. **Engage maintainers directly** to gauge appetite for both small additions and structural changes.
2. **Start with `remote_url` derivation** as a low-risk PR that tests the contribution loop.
3. **If structural contributions are off the table**, propose factoring the per-agent watchers into a reusable package. Either project can host or co-maintain.
4. **Build standalone if neither lands**, using opensessions as the reference for watcher design but with our own architecture for persistence and pub/sub.

The discipline: don't fork; don't reimplement what's working; don't pretend opensessions doesn't exist; but also don't bet the substrate's existence on a maintainer interaction that hasn't happened yet.