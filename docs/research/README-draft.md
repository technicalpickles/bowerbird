# claude-state-bus

A local daemon that watches your coding agents and exposes their state via pub/sub. Built for the dozens of pet/sprite/dashboard/lamp/voice tools that all want to know what Claude Code is doing — without each of them re-implementing hook ingestion.

```
┌──────────────────────────────────────┐
│  ~/.claude/settings.json             │
│  (one hook line installed by         │
│   claude-state-bus install)          │
└─────────────────┬────────────────────┘
                  │
          ┌───────▼────────┐
          │  shim binary   │   ← <5ms exit, never blocks Claude
          └───────┬────────┘
                  │ POSTs events
          ┌───────▼────────┐
          │     daemon     │   ← maintains sessions, agents, events
          │   (sqlite)     │   ← computes the reaction enum projection
          └───────┬────────┘
                  │
        ┌─────────┴──────────┐
        │   WebSocket pub/   │
        │   sub: events.*    │
        │           state.*  │
        └─────────┬──────────┘
                  │
   ┌──────────────┼──────────────┐
   │              │              │
┌──▼──┐      ┌────▼─────┐   ┌────▼────┐
│lamp │      │ voice    │   │ TUI /   │
│     │      │ TTS      │   │ sprite  │
└─────┘      └──────────┘   └─────────┘
```

The substrate observes; presenters render. Many presenters share one ingestion path.

## Status

Pre-MVP. The design is settled across [`docs/design/`](docs/design/); the implementation is in progress. See [milestones](docs/design/12-mvp-and-milestones.md) for the plan and [no-list](docs/no-list.md) for what's deliberately out of scope.

## Why this exists

Today, every pet, lamp, sprite, dashboard, and voice tool installs its own hook into `~/.claude/settings.json`. They collide. Each one re-implements JSONL transcript parsing or hook payload normalization. Some maintain shadow state files (`~/.claude-pet/global-tracker.json`) to avoid re-processing tokens. Others tail the JSONL on a polling loop. Half of them break when Claude's transcript format shifts.

`claude-state-bus` ingests once, exposes the data via pub/sub, and lets every presenter subscribe to the slice they care about. The lamp wants three reaction states. The voice tool wants `subagentEnd` events. The dashboard wants every session's `current_state`. They all get exactly that, no more, no less, with no hook installation of their own.

## What it does

- **Hook router**: one entry in `~/.claude/settings.json` fans out to many consumers
- **Event log**: every hook event preserved in SQLite with native payload intact
- **Reaction projection**: 11-value enum (`idle / thinking / working / editing / running / testing / waiting / waving / success / error / celebrating`) computed from tool names and event sequence
- **Pub/sub WebSocket** with two channels:
  - `events.*` — raw events with hierarchical topic filters
  - `state.*` — derived state transitions with old/new values
- **REST endpoints** for polling and snapshot reads
- **Per-session metadata**: `repo_root`, `worktree`, `branch`, `remote_url`, `context_percentage`, `last_event_at`, lifecycle, attachment liveness
- **Agent identity**: `agent_type` and `parent_agent_id` preserved from Claude's `subagent_type`

## What it doesn't do

Read this carefully before filing an issue. The full reasoning is in [docs/no-list.md](docs/no-list.md).

- **No HITL backflow.** Permission events flow out; answers don't flow back. The substrate is fire-and-forget.
- **No tool blocking.** Tools that want to enforce policy install their own hook in parallel.
- **No personas, voices, sprites, or color palettes.** Presenter concerns. The substrate exposes raw signals; you map them.
- **No runtime competitor.** We observe Claude / Codex / Gemini / Cursor / OpenCode. We do not replace them.
- **No multi-agent in MVP.** Claude only at v1. Codex at M2.
- **No statusline composer in MVP.** Different problem (pulled, not pushed). M6+.
- **No activity rate computed daemon-side.** Subscribe to events and compute your own; six lines of code.
- **No cross-machine pub/sub.** Localhost binding. Build your own relay if you need LAN.

If your use case is on the "no" list, the project is probably not the right home. If it's adjacent ("not yet"), file a discussion — some of those wait for the right moment.

## Install

```bash
# Homebrew (macOS, the primary target)
brew install claude-state-bus

# Or with cargo (any platform)
cargo install claude-state-bus

# Install the hook into ~/.claude/settings.json (non-destructive merge)
claude-state-bus install

# Start the daemon (typically launchd / systemd; manual for now)
claude-state-bus daemon
```

To uninstall:

```bash
claude-state-bus uninstall    # removes hook entries from settings.json
```

The daemon binds to `127.0.0.1:9876` by default. The auth token is written to `~/.claude-state-bus/server.json` on first start.

## Quick taste

Subscribe to one session's state changes:

```javascript
const ws = new WebSocket("ws://127.0.0.1:9876/subscribe");
const token = readToken("~/.claude-state-bus/server.json");
ws.send(JSON.stringify({ op: "auth", token }));

ws.send(JSON.stringify({
  op: "subscribe",
  topic: "state.session.abc-123.current_state",
}));

ws.onmessage = (msg) => {
  const frame = JSON.parse(msg.data);
  if (frame.op === "snapshot") {
    console.log("Current state:", frame.value);
  } else if (frame.op === "publish") {
    console.log("Changed:", frame.change.old, "→", frame.change.new);
  }
};
```

Subscribe to subagent completions across all sessions (for a voice tool):

```javascript
ws.send(JSON.stringify({
  op: "subscribe",
  topic: "events.subagentEnd.*",
}));

ws.onmessage = (msg) => {
  const frame = JSON.parse(msg.data);
  if (frame.op === "publish") {
    const agent_type = frame.event.agent_type;  // e.g., "code-reviewer"
    const voice = voiceConfig[agent_type] ?? defaultVoice;
    tts.speak(`${agent_type} finished`, voice);
  }
};
```

Get the current session list with snapshots:

```bash
curl -H "Authorization: Bearer $(cat ~/.claude-state-bus/server.json | jq -r .token)" \
  http://127.0.0.1:9876/sessions
```

For full presenter recipes, see [`docs/cookbook/`](docs/cookbook/).

## Philosophy

The substrate's job is to preserve and expose underlying data, not to define application-level concepts on top of it.

This means:

- **Native payloads are kept intact.** Hook event payloads ride in the `payload` field verbatim. The daemon doesn't strip or rename fields. Presenters that want full fidelity get it.
- **Only one normalization is applied**: tool names mapped to the 11-value reaction enum, per `adapters/<source>/tool-reactions.yaml`. Everything else passes through.
- **Sources are first-class.** `(source, session_id)` is the natural key. Claude is the only source in v1; Codex follows at M2. Future adapters configure rather than fork.
- **Presenters can be tiny.** The reference lamp presenter is ~150 lines. The reference voice presenter is ~80 lines. Two of the test cases from the design (TUI grouped by remote, animated 8-bit sprites) come in at 60-120 lines each. If your presenter exceeds ~200 lines for a single-purpose tool, the substrate may be too low-level — file a discussion.

For longer-form design rationale, read [the design docs](docs/design/) in order. Doc 14 (`activity-survey.md`) and Doc 16 (`maintainership-and-scope.md`) are the most useful starting points.

## Contributing

The contribution model is borrowed from [pi-mono](https://github.com/badlogic/pi-mono): **new issues and PRs from new contributors are auto-closed by default. Maintainers review auto-closed items weekly.**

This isn't hostile — it's how we keep the project from drifting. Here's how it works:

1. **For new features or behavior changes**: file a GitHub Discussion first. Describe the use case and any alternatives you've considered.
2. **For bug reports**: include reproduction steps. These get reopened quickly.
3. **For new adapters** (a new agent CLI): you're welcome to file a PR directly. New adapters are the highest-value contribution shape and get faster review. See [`docs/adapter-authoring.md`](docs/adapter-authoring.md).
4. **For documentation fixes**: always reviewed.
5. **For everything else**: discussion first. PRs without discussions are routinely declined regardless of code quality.

If you're building a presenter or an extension that doesn't require core changes, you don't need to contribute upstream at all — publish your own package and let people install it directly. See the [cookbook](docs/cookbook/) for examples.

The substrate is small on purpose. Each `no` is justified. If you think a `no` should be revisited, file a discussion with what's changed.

## Project layout

```
claude-state-bus/
├── AGENTS.md              # project rules for humans and AI agents
├── docs/
│   ├── design/            # how we got here (17 design docs)
│   ├── decisions/         # ADRs for load-bearing choices
│   ├── adapter-authoring.md
│   ├── presenter-authoring.md
│   ├── protocol.md
│   └── no-list.md         # what we don't do, with reasoning
├── crates/
│   ├── protocol/          # wire types and serialization (stable surface)
│   ├── shim/              # static binary called by hooks (<5ms exit)
│   ├── daemon/            # long-running service (sqlite + ws + rest)
│   └── adapter-claude/    # reference adapter
├── adapters/
│   └── claude/
│       ├── capabilities.yaml
│       └── tool-reactions.yaml
├── examples/              # tested in CI; cookbook entries reference these
│   ├── lamp-presenter/
│   ├── voice-presenter/
│   ├── grouped-tui/
│   └── sprite-web/
└── cookbook/              # how to do specific things
```

The total core (`crates/protocol`, `crates/shim`, `crates/daemon`, `crates/adapter-claude`) targets 5,000-7,000 lines of Rust. If it grows past 10,000, something's wrong — either we've absorbed something that should have been an extension, or scope has expanded silently.

## License

MIT.

## Related projects

The substrate borrows ideas from many places:

- **[opensessions](https://github.com/Ataraxy-Labs/opensessions)** — the closest existing project. Per-agent watchers, multi-agent state tracking. Different shape (TUI consumer, in-memory only) but the watcher abstraction is essentially identical. See [`docs/design/15-opensessions-gap-analysis.md`](docs/design/15-opensessions-gap-analysis.md).
- **[AgentDeck](https://github.com/puritysb/AgentDeck)** — the `AgentCapabilities` matrix per source. Lifted directly.
- **[PocketFlow](https://github.com/The-Pocket/PocketFlow)** and **[pi-mono](https://github.com/badlogic/pi-mono)** — the maintainership and scope discipline.
- **[OpenPets](https://github.com/openpets/openpets)** — the 11-value reaction enum. Canonical vocabulary.

The substrate isn't a replacement for any of these. They each serve different consumers. The goal is to be the data layer they could all share.