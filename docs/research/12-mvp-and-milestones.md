# MVP and milestones

The design has reached a point where it's clear what the substrate could be. The question this document answers: what's the smallest version that's actually useful, and how do later capabilities sequence on top of it?

The principle: an MVP isn't a demo. It's the smallest version that (a) one or two real presenters can adopt, (b) validates the load-bearing architectural claims, and (c) leaves a clean shape for everything that comes after. If it can't pass those bars, it's a sketch, not an MVP.

## What the MVP has to prove

Four claims that are load-bearing:

1. **The hook router replaces fighting over `~/.claude/settings.json`.** Presenters install one entry; the daemon fans out. If this works, the substrate has earned its existence — the rest is bonus.
2. **The reaction enum projection is correct.** Eleven values lifted from OpenPets. If two unrelated presenters (a lamp and a sprite) both consume `current_state` without disagreeing on what each value means, the projection is the right shape. If they fork, it's wrong.
3. **Pub/sub is meaningfully cheaper for presenters than polling.** Measured as lines of presenter code and round-trips per second under typical Claude Code load.
4. **`agent_type` passthrough lets persona/voice tools work without installing their own hooks.** PAI's voice-by-subagent-type mapping is the test.

If the MVP can't show these four, none of the later milestones matter.

## What the MVP does *not* need to prove

These are deliberately deferred:

- **Multi-agent.** The abstraction can be designed for it; shipping one adapter is enough to validate the shape. Codex is M2.
- **Capabilities matrix.** Only matters once there are two or more adapters to differ.
- **Worktree / repo / branch derivation.** Cheap to add; not on the critical path for v1 proof.
- **Statusline composer.** Different problem (pulled by Claude per tick, not pushed); larger scope.
- **HITL backflow.** Out of scope at the design level.
- **LAN reachability, multi-host, durable subscriptions.** Out of scope.

Resisting these is the discipline. Every one of them is "obviously useful," and every one of them is a way to spend three months not shipping.

## Scope of the MVP

```
                ┌─────────────────────────────────────────────┐
                │  ~/.claude/settings.json (single hook line) │
                └─────────────────┬───────────────────────────┘
                                  │
                          ┌───────▼────────┐
                          │  claude-state  │  ← static binary, <5ms exit
                          │   -bus emit    │
                          └───────┬────────┘
                                  │ POST events
                          ┌───────▼────────┐
                          │     daemon     │  ← long-running, single tenant
                          │                │
                          │   ┌────────┐   │
                          │   │ sqlite │   │  ← events + sessions tables
                          │   └────────┘   │
                          │                │
                          │   ┌────────┐   │
                          │   │project-│   │  ← state machine + reaction enum
                          │   │  ion   │   │
                          │   └────────┘   │
                          │                │
                          │   ┌────────┐   │
                          │   │ ws ps  │   │  ← pub/sub broker
                          │   │ rest   │   │  ← polling endpoints
                          │   └────────┘   │
                          └───────┬────────┘
                                  │
                  ┌───────────────┼────────────────┐
                  │               │                │
            ┌─────▼─────┐  ┌──────▼──────┐  ┌─────▼─────┐
            │claude-lamp│  │  PAI voice  │  │  claude-  │
            │  (lamp)   │  │             │  │  status   │
            └───────────┘  └─────────────┘  └───────────┘
                  SUBSCRIBE             SUBSCRIBE       SUBSCRIBE
                  state.session.*       events.         state.sessions
                  .current_state        subagentEnd.*   state.session.*
```

### Inclusions

**Single Claude Code adapter.** Hook router for these events: `SessionStart`, `SessionEnd`, `UserPromptSubmit`, `PreToolUse`, `PostToolUse`, `Stop`, `Notification`, `PermissionRequest`, `SubagentStart`, `SubagentStop`. That's 10 events; full hook set is 13 but `PreCompact`, `PostToolUseFailure`, and edge cases come later.

**Events table** with the v2 schema. `event_id` autoincrement, `(session_id, source='claude')` natural key, full payload preserved.

**Sessions table** with the v2 schema, minus the worktree/repo/branch columns (deferred to M3). Folded counters (`total_tokens`, `total_tool_calls`, `total_user_turns`, `last_user_turn_at`).

**Agents table** with `agent_id`, `agent_type`, `parent_agent_id`. The `subagent_type` from hook payload gets copied to `agent_type`.

**The reaction enum projection.** Eleven values. Hard-coded mapping table inside the daemon for Claude tool names (`Edit`→`editing`, `Bash`→`running`, etc.).

**Pub/sub WebSocket endpoint** with two channels (`events.*` and `state.*`), hierarchical topics, snapshot-on-subscribe for STATE topics, raw event delivery for EVENTS topics. Bounded queue, drop with `dropped` frame on overflow.

**Polling REST endpoints** for `GET /sessions`, `GET /sessions/:id`, `GET /sessions/:id/events?since=<cursor>`, `GET /sessions/:id/stats`. These exist primarily so a presenter can choose polling if they want; they also serve as the implementation backing for the WS snapshot frame.

**Auth via per-daemon-run token** written to `~/.bowerbird/server.json` on startup (Pixel Agents' pattern). WS subscribers and REST clients present the token.

**Shim binary** that exits in <5ms. Rust, statically linked. Takes hook event name and stdin payload, POSTs to the daemon, exits. Never blocks Claude even if the daemon is down (1s timeout, log to local file on failure).

### Exclusions

No statusline composition. No worktree derivation. No capabilities. No multi-agent. No sweep / heartbeat / lease for now — sessions go to `lifecycle='ended'` only on explicit `SessionEnd`; otherwise stay `live`. (Means abandoned sessions appear stuck; acceptable for MVP.) No attachments table — `process_token` and terminal attribution come in M3. No Codex, no Gemini, no anything-else.

### Two real presenters as success criteria

The MVP isn't done until two presenters work against it in production, ideally for a week each:

1. **claude-lamp**, forked to subscribe instead of installing hooks. Subscribes to `state.session.<my-session>.current_state`. Maps each of the 11 reaction values to a BLE bulb color. ~150 lines.
2. **A PAI-style voice presenter** (could be a slim fork of AgentVibes or a clean rewrite). Subscribes to `events.subagentEnd.*`. Reads `agent_type` from the event, looks up the voice in a config file, fires ElevenLabs TTS. ~80 lines.

If both work, all four load-bearing claims pass. If only one works, there's a real abstraction gap to find.

### Performance bars

- Shim exit time: <5ms p95. Measured: `time bowerbird emit PostToolUse < /tmp/payload.json` in a loop of 100.
- Hook-to-projection-update latency: <50ms p95. Measured: timestamp on shim exit vs. timestamp on STATE channel publish.
- Hook-to-presenter latency: <100ms p95. Measured: hook fires → lamp color changes.
- Daemon idle CPU: <0.5% on a typical laptop with one live session.
- Daemon memory: <50MB resident.

If any of these miss by 2x, treat as an MVP failure and revisit.

### Languages

**Shim: Rust.** Static binary, no runtime, no GC pause, sub-5ms cold start. Node and Python don't meet the bar.

**Daemon: Rust.** Same codebase, simpler dependency graph. Tokio for async, axum or hyper for HTTP/WS, rusqlite for storage. Could be Go for slightly easier contribution surface, but mixing two languages adds friction.

**Adapter config:** YAML. Read at daemon startup. One file per source.

### Repository layout

```
bowerbird/
  ├── crates/
  │   ├── shim/                  # the static binary that hooks invoke
  │   ├── daemon/                # long-running service
  │   ├── protocol/              # wire types, shared
  │   └── adapter-claude/        # the only adapter in MVP
  ├── adapters/
  │   └── claude/
  │       ├── capabilities.yaml  # placeholder for M3
  │       └── tool-reactions.yaml
  ├── examples/
  │   ├── lamp-presenter/        # claude-lamp fork
  │   └── voice-presenter/       # PAI-style voice
  └── docs/
      ├── pubsub-protocol.md
      └── adapter-guide.md       # how to write a new adapter (used in M2)
```

### Distribution

- Homebrew tap for macOS (the primary target — most novelty tools are macOS-first)
- `cargo install bowerbird` for source builds
- A single `bowerbird install` command that writes the hook entry to `~/.claude/settings.json` non-destructively (merges with existing hooks)
- A `bowerbird uninstall` command that removes the entries

## Milestones beyond MVP

These are sized to roughly two-to-six weeks of focused work each. The order reflects what each one validates and which presenters it unblocks.

### M1 — MVP

As described above. Two presenters in production. Four load-bearing claims tested.

### M2 — Codex adapter

Adds the second tier-1 adapter. Validates the abstraction shape — if the same shim binary handles Codex with a config table change, the abstraction is real; if it needs significant code changes, the abstraction needs work.

**What's in M2:**

- `adapters/codex/runtime.yaml` describing Codex's hook config format (TOML inline tables in `~/.codex/config.toml`)
- `adapters/codex/tool-reactions.yaml` mapping `shell_command`→`running`, `apply_patch`→`editing`, etc.
- Event-name aliasing layer in the shim — same hook events, possibly different names on Codex side
- A small additional logic path for Codex's TOML config writing (vs. Claude's JSON)
- `bowerbird install --agent codex` writes the right config to the right place
- Documentation: adapter-guide.md fleshed out, contributor pathway documented

**Success bar:** an existing Codex presenter (probably agent-flow if patoles is willing to try it) consumes from the daemon for both Claude and Codex sessions side-by-side. Cross-source event ordering works (events from both agents interleave correctly via monotonic `event_id`).

**Risk:** Codex's hook surface is still evolving (`PreToolUse`/`PostToolUse` only stable as of v0.130 in May 2026). The adapter has to pin a Codex version range.

### M3 — Worktrees, attachments, terminal attribution, derived session fields

The schema additions from v2 that pay off once there's more than one session running. Mostly mechanical; gets a lot of value with low risk.

**What's in M3:**

- `repo_root`, `worktree`, `branch`, **`remote_url`** columns on `sessions`, derived at first event via combined `git rev-parse --git-common-dir --show-toplevel --abbrev-ref HEAD` plus `git config --get remote.origin.url`. Cached by cwd. `remote_url` normalized to canonical `<host>/<owner>/<repo>` for cross-worktree grouping (the gap surfaced in case 1 of `13-test-cases.md`).
- `worktreeCreate` and `worktreeRemove` event kinds; orchestrators (dmux, etc.) can POST them.
- `attachments` table with `process_token` (pid + starttime), `liveness` column (alive/exited/unknown), heartbeat-driven `last_heartbeat_at`.
- Terminal attribution fingerprint via env vars (`TERM_PROGRAM`, `TMUX`, `TMUX_PANE`, `KITTY_WINDOW_ID`, `WEZTERM_PANE`, `VSCODE_INJECTION`, `SSH_TTY`, etc.), captured once per attachment open, stored in `attachments.location` as JSON.
- `context_percentage` on the session row, populated from Claude statusline data and Codex token_count JSONL events where available (Claude-only in practice for now; nullable for sources without context telemetry).
- `last_event_at` on the session row, documented as the universal "is this active" signal (see `14-activity-survey.md`).
- `GET /sessions?repo_root=...`, `?branch=...`, `?remote_url=...` filters; new `GET /remotes` aggregate.
- New STATE topics: `state.session.<id>.attachment` for liveness changes; `state.session.<id>.context` for context-percentage bucket changes (10%, 25%, 50%, 75%, 90%).

**Explicitly not in M3** (after the activity-survey review): no `recent_event_count_60s` or `recent_tool_count_60s` counters. The survey of 8 pet/sprite/dashboard tools showed that only 1 (tamagotchi) actually wants a window-based rate. The other 7 either use discrete states, key off `last_event_at`, or compute their own rate from event subscriptions in ~15 lines. Exposing the counter would be single-tool convenience, not a foundational primitive. Presenters compute activity rate client-side using one of four documented patterns (`14-activity-survey.md`).

**Presenter that validates it:** claude-status (gmr's click-to-focus terminal-session menu bar) — already has its own implementation of terminal attribution and would benefit from a daemon-provided version. If claude-status's "click to focus" works through the daemon, the attribution data is right.

**Success bar:** a user with three worktrees on the same repo, each with a Claude session, sees them grouped under one repo in a dashboard or pet visualizer. Both test cases from `13-test-cases.md` are buildable in ~60-120 lines of presenter code against an M3 daemon.

### M4 — Capabilities surface + Gemini and Cursor adapters

The capabilities matrix (from AgentDeck) and the remaining tier-1 adapters. After this, the substrate covers four agents and presenters can negotiate features per source.

**What's in M4:**

- `GET /sources` returning the capabilities matrix
- Per-adapter `capabilities.yaml` files for Claude, Codex, Gemini, Cursor
- `reaction_enum_subset` published per source — presenters can validate they're only reacting to states the source can produce
- Gemini adapter (config file: `~/.gemini/settings.json`; event aliases: `BeforeTool`/`AfterTool` → `PreToolUse`/`PostToolUse`; `CLAUDE_PROJECT_DIR` alias already there)
- Cursor adapter (config file: `~/.cursor/hooks.json`; lifecycle events; some version handling for the moving target)

**Presenter that validates it:** an updated AgentDeck-style multi-agent visualizer that renders different UI per source based on capabilities (mode-switching shown for Claude, hidden for Codex; permission-payload-aware UI for Claude/Codex/Gemini, basic-only for Cursor where the payload shape varies).

**Success bar:** a presenter rendering 4-agent state simultaneously works without per-agent branching in the presenter code — branching is contained in capability checks against the substrate's response.

### M5 — Plugin provider (OpenCode)

The second ingest model — tier-2 from the multi-agent analysis. Validates that the wire protocol holds when the ingest path isn't a hook shim.

**What's in M5:**

- `@bowerbird/opencode-plugin` npm package, ~200 lines of TypeScript
- Plugin subscribes to OpenCode's internal events (`session.created`, `session.idle`, `tool.execute.before`, `tool.execute.after`, `chat.message`), translates to the canonical `AgentEvent`, POSTs to the daemon
- `adapters/opencode/capabilities.yaml`, `tool-reactions.yaml`, `runtime.yaml` (ingest: plugin)
- The daemon's auth model extended to accept events from a plugin process running in OpenCode's address space (probably: the plugin reads the same token from `~/.bowerbird/server.json`)
- Documentation: how to write a plugin-provider for an agent without config-installable hooks

**Presenter that validates it:** opensessions or AgentDeck consuming from the daemon for OpenCode sessions, side-by-side with Claude.

**Success bar:** the daemon's event log shows interleaved Claude and OpenCode events with the same shape; presenters that don't care about the source ingest path don't notice the difference.

### M6 — Statusline composer

Different problem; bigger scope. The statusline is pulled, not pushed — Claude Code invokes the configured statusline command per tick. Multiple presenters want to contribute segments. The composer reads registered segment providers, calls each (with caching), composes the result.

**What's in M6:**

- `bowerbird statusline` command Claude is configured to run
- Segment provider registration via WS or local socket (`register-segment {name, priority, command}`)
- Composition per tick: each registered segment called with current session JSON; output composed in priority order
- Built-in segments: `state` (the reaction enum, emoji-formatted), `tokens`, `context-percentage`, `model`
- Cache layer so segment providers aren't re-invoked every tick if their state hasn't changed
- Documentation: how to write a segment provider

**Presenter that validates it:** ccpet's statusline + a separate "current state" segment, composed together, without either tool fighting over the slot.

**Success bar:** two unrelated statusline segments coexist on the same line, contributed by two different presenters that each only register their own segment.

### M7+ — Speculative

Beyond M6, what to build depends on what users actually want. Plausible directions:

- **Sweep + abandoned-session detection** (the v2 design's heartbeat + lease + reconcile loop). Probably needed once power users have many sessions; not urgent until then.
- **Transcript-provider** (Aider, possibly Copilot CLI). File-tail-only ingest. Coarser state vocabulary. Two-day project per agent once the abstraction is established.
- **MCP-based ingest** as a fourth provider type. Some tools may want to push events via MCP rather than hook or plugin. Easy to add to the wire protocol; harder to define cleanly.
- **Per-presenter durable subscriptions.** Opt-in, disk-backed queue. Wait until evidence accumulates that a real presenter wants it.
- **Capabilities-driven UI generation.** Long shot. Presenters describe what they want to render in a schema; the daemon (or a sibling library) renders only the parts the source supports. Speculative; probably never.

## Risks and how to detect them early

Each load-bearing claim has a failure mode worth watching for.

**The hook router introduces measurable latency.** Risk: every Claude invocation now waits for the shim. If the shim's static binary cold-start exceeds 5ms p95 on a moderately loaded macOS laptop, users will notice. Detection: `hyperfine` benchmark in CI. Mitigation: profile aggressively, possibly use `mimalloc` or skip allocations on the hot path.

**The reaction enum doesn't fit.** Risk: a presenter we hadn't considered needs a state value we don't have (`compacting`? `searching`? `reviewing`?). Detection: review the first three real presenters that consume `current_state`; if any of them does `if (state === "working" && tool === "...") render as X`, the enum is missing a value. Mitigation: extending the enum is cheap; the OpenPets 11-value set is a starting point, not gospel.

**Pub/sub backpressure under bursty Claude sessions.** Risk: a session that fires 20 tool events per second overflows the WS queue and presenters miss state changes. Detection: load test with synthetic event stream. Mitigation: the `dropped` frame protocol is already designed; presenter SDK should automatically refetch snapshot on `dropped`.

**`agent_type` not actually used by real presenters.** Risk: PAI's pattern of voice-per-subagent-type is interesting but rare; if no other presenter uses it, the schema cost is unjustified. Detection: count adopters at M4. Mitigation: it's a single column and an event field; cost is trivial even if usage is low.

**Cross-source ordering breaks under clock skew.** Risk: events from two different agents arrive out-of-order at the daemon and get assigned `event_id`s in the wrong order. Detection: integration test with two simulated agents firing simultaneously. Mitigation: `event_id` is monotonic at the daemon (ingest time), not at the source — should be fine, but worth a test.

## What this isn't

A few clarifications on what the MVP and milestones are *not*:

- **Not a product.** No marketing site, no signup flow, no telemetry. Open source, local-first. Distributed as a Homebrew formula and a cargo install target.
- **Not an Anthropic project.** This is a substrate that *works with* Claude Code, not a fork of it. The hook integration is documented behavior; no internal access required.
- **Not a runtime competitor.** The substrate doesn't run agents. It observes them.
- **Not a Universal Anything.** "Agent-agnostic" means the abstraction is right for tier-1 hook-compatible agents, tier-2 plugin-required agents, and tier-3 transcript-only agents — with shrinking feature sets as the tier descends. Honest about its limits.

## The path through

The natural cadence is something like:

| Milestone | Focus | What it validates |
|---|---|---|
| MVP (M1) | Claude-only, hooks + pub/sub + reactions | Substrate is useful; abstractions hold |
| M2 | Codex adapter | Tier-1 abstraction is correct |
| M3 | Worktrees + attachments + attribution | Schema serves multi-session users |
| M4 | Capabilities + Gemini + Cursor | Tier-1 coverage complete; presenters can negotiate |
| M5 | OpenCode plugin provider | Tier-2 ingest model is correct |
| M6 | Statusline composer | Pulled-data presenters can coexist |

After M6 the substrate covers the bulk of the inventory's use cases. M7+ depends on what users do with it.

The thing to avoid: skipping M1 in favor of building "the right thing first." The substrate is a piece of plumbing. Plumbing earns trust by doing one job reliably before being trusted with more.