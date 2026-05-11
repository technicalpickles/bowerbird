# Maintainership, scope, and extensibility

The substrate's success depends as much on how it's maintained as on what's in it. Two projects model the discipline well: PocketFlow (100-line Python LLM framework) and pi-mono (TypeScript coding-agent toolkit, 44k+ stars, OpenClaw's engine). Both are explicitly minimal, explicitly extensible, and explicitly opinionated about what doesn't belong in core. The substrate should borrow heavily from both.

This document is about the *non-code* part of the project: how to set scope, how to handle contributions, how to structure for AI maintainability, and how to keep the core honest as community pressure mounts.

## The two reference models

### PocketFlow

100 lines of core in `pocketflow/__init__.py`. Zero dependencies. Zero vendor lock-in. The entire abstraction is "graph + shared store" — nodes with `prep / exec / post` methods, flows that connect them, a dict-based shared store for inter-node communication.

The discipline:

- **Vendor-specific code is out of core.** No built-in LLM wrappers, no embedding clients, no vector DB integration. The user implements their own `call_llm` function. The framework doesn't pick.
- **Patterns are documented, not built-in.** Multi-agent, RAG, workflow, map-reduce, supervisor — all implementable in a few hundred lines on top of the 100-line core. The cookbook directory contains ~30 of these as working examples.
- **AI agents are intended consumers.** Documentation includes `.cursorrules`, `.windsurfrules`, `.clinerules` files that teach AI coding tools how to use the framework correctly. The author explicitly says: "the more complex the framework, the harder it is for AI to help."
- **The framework is small enough to copy.** "Just copy the source code (only 100 lines)" is offered as an alternative to `pip install`.

The frame: humans design the high-level flow, AI agents implement the node logic. The framework's smallness is what makes that division of labor possible.

### pi-mono

A coding agent toolkit. The agent itself has exactly four tools (`read`, `write`, `edit`, `bash`) and a system prompt under 1,000 tokens. From the README:

> No MCP. Build CLI tools with READMEs (see Skills), or build an extension that adds MCP support.
>
> No sub-agents. There's many ways to do this. Spawn pi instances via tmux, or build your own with extensions.
>
> No permission popups. Run in a container, or build your own confirmation flow with extensions.
>
> No plan mode. Write plans to files, or build it with extensions.
>
> No built-in to-dos. They confuse models.

Each "no" comes with a *why* and an alternative. The list reads like a manifesto. The philosophy: "what you leave out matters more than what you put in."

The contribution model:

> **New issues and PRs from new contributors are auto-closed by default.** Maintainers review auto-closed issues daily.

This looks hostile but is actually a clear signal: this project is opinionated and maintainer-led. Auto-closing is paired with aggressive extensibility — if you want something pi doesn't have, you build a pi package and publish it to npm. The maintainers don't have to gatekeep your fork; you don't have to wait for them to merge.

The extension system has 20+ lifecycle hooks (`session_start`, `before_agent_start`, `tool_call`, `before_provider_request`, `after_provider_response`, etc.) and four extension types (extensions, skills, prompt templates, themes). The "awesome-pi-agent" repo curates ~40 third-party packages. The core doesn't grow; the ecosystem does.

## What both share

Five principles emerge from looking at the two together:

1. **The core is small enough to fit in one head.** PocketFlow: 100 lines. pi-mono: 4 tools, ~1000 token system prompt. The maintainer can hold the entire core in working memory and reason about every change.

2. **What's out of scope is named, justified, and given alternatives.** Not "we don't support X" but "we don't support X because Y; if you need X, do Z." This converts every "you should add X" into "you should build a Z and contribute it back via the extension mechanism."

3. **Extensibility is first-class and unbounded.** The extension API is rich. New extensions ship via standard package mechanisms (npm, pip, cargo). There's no centralized registry to gate-keep; discovery happens through awesome-lists and community curation.

4. **AI agents are intended consumers of the documentation.** PocketFlow ships `.cursorrules`. pi-mono ships `AGENTS.md`. Both projects expect that humans *and* AI agents will read their docs to understand how to extend or modify the code. This shapes how docs are written.

5. **The project author publicly demonstrates how they use their own tool.** PocketFlow's cookbook. pi-mono's published agent sessions on Hugging Face. The discipline of using your own tool publicly forces it to actually work end-to-end.

## How this maps to the substrate

The substrate is plumbing, not a framework — but the principles transfer.

### Scope discipline: the "no list"

The substrate's "no list" already exists implicitly across the design docs. Making it explicit is the maintainability lever. Draft:

> **No HITL backflow.** The substrate emits permission-request events. It does not accept permission-answer responses. If you need a wearable approval interface, build a sibling service that consumes from the substrate. Why? Synchronous wait points blow past the 5ms shim budget and turn the substrate into a coordination service, which is a different project.
>
> **No tool blocking.** The substrate observes tool calls. It does not veto them. Policy enforcement remains a per-agent hook install, parallel to the substrate. Why? Vetoing requires synchronous blocking; the shim is deliberately fire-and-forget.
>
> **No cross-machine pub/sub.** The broker binds to localhost only. LAN-distributed presenters build their own relay. Why? Network reliability, auth, and multi-host clock-skew are out-of-scope problems.
>
> **No persona, voice, sprite, or other application-level concepts.** The substrate exposes `agent_type` and `current_state`. Mapping those to voices, sprites, or moods is presenter responsibility. Why? Eight surveyed pet/sprite/dashboard tools each made different choices; the substrate doesn't pick one.
>
> **No statusline composer in MVP.** Statusline is pulled (Claude invokes per tick), not pushed. Different problem; larger scope; M6 or later. Why? Shipping it in MVP triples the surface area for marginal benefit; presenters that want statusline can build a small one against the polling REST surface.
>
> **No multi-agent in MVP.** Claude only at M1. Codex at M2. Why? Validating the abstraction with two adapters is overkill before validating it with one.
>
> **No agent runtime competitor.** The substrate observes Claude/Codex/Gemini/Cursor. It does not run agents. If you want an agent, use one of those. Why? The substrate's value is being agent-neutral and observation-only.
>
> **No on-disk session ordering, no UI preferences, no per-user view state.** Those are presenter concerns. Why? Two presenters viewing the same daemon should not interfere with each other.
>
> **No durable subscriptions in MVP.** Ephemeral pub/sub only. Why? Disk-backed queues add complexity for benefit that no inventoried presenter has demonstrated need for yet.

Each "no" answers a question that will be asked. The point isn't to be dismissive — it's to set expectations so contributors can self-select. Someone who needs HITL backflow will see "this isn't the right project for that need" and move on or build the sibling service. Someone whose pet visualizer needs `current_state` + `agent_type` will see "this exactly serves you."

The maintainer's job becomes: when a feature request comes in, check whether it's on the no-list. If yes, link the explanation. If it's a new "no," add it to the list and link. The no-list grows; the core stays small.

### Contribution model: borrow pi-mono's auto-close

The substrate is small. The maintainer (you, presumably) can review what they want to review. Default-accepting PRs from anyone creates an unsustainable maintenance load and dilutes architectural coherence. pi-mono's "new contributors auto-closed, maintainers review daily" pattern is sound discipline.

A concrete version for the substrate:

- **PRs from new contributors auto-closed** with a templated message pointing to:
  - The no-list (have you checked if this is intentionally out of scope?)
  - The extension/adapter pathway (this could be a third-party adapter)
  - The discussion forum (file a discussion first; PRs without a discussion are routinely declined)
- **Maintainer reviews auto-closed PRs once a week** and reopens any that warrant discussion.
- **Adapter PRs treated differently** because they're additive and low-architectural-risk. A new agent adapter that conforms to the adapter contract is much easier to accept than a change to the projection layer.
- **Documentation PRs always reviewed** because they cost the maintainer almost nothing and they signal community ownership.

The signal to contributors: "we will merge things, but we move deliberately. Build it as an adapter or extension first, prove it useful, then propose folding it into core."

### Repository structure: optimized for AI extension authors

The substrate's repo structure should let an AI agent — Claude, Codex, whoever — open the repo and immediately understand how to add an adapter or how to write a presenter SDK. This is the PocketFlow lesson: small, discoverable, well-documented for AI consumption.

Proposed structure:

```
bowerbird/
├── README.md                         # Single-page overview, philosophy, no-list
├── AGENTS.md                         # Project rules for humans and AI agents
├── docs/
│   ├── design/                       # The 14 design docs as they evolve
│   ├── adapter-authoring.md          # How to write a new adapter (M2 deliverable)
│   ├── presenter-authoring.md        # How to consume the pub/sub (with examples)
│   ├── protocol.md                   # Wire protocol spec — the public contract
│   ├── extension-points.md           # What's customizable, what's not
│   └── no-list.md                    # Explicit out-of-scope register
├── crates/
│   ├── protocol/                     # Wire types only. Stable surface.
│   ├── shim/                         # Static binary, hot path. ~500 lines.
│   ├── daemon/                       # The long-running service. ~3-5k lines.
│   └── adapter-claude/               # The reference adapter. ~1k lines.
├── adapters/
│   └── claude/
│       ├── capabilities.yaml
│       └── tool-reactions.yaml
├── examples/
│   ├── lamp-presenter/               # ~150 lines, validates one load-bearing claim
│   ├── voice-presenter/              # ~80 lines, validates another
│   ├── grouped-tui/                  # The case-1 test case from doc 13
│   └── sprite-web/                   # The case-2 test case from doc 13
└── cookbook/
    ├── reading-state.md              # "Subscribe to state.session.<id>"
    ├── reading-events.md             # "Subscribe to events.session.<id>.*"
    ├── computing-activity.md         # "Three patterns from 14-activity-survey"
    ├── grouping-by-remote.md         # "Use sessions.remote_url"
    ├── handling-disconnects.md       # "Dropped frames + resnapshot"
    └── building-an-adapter.md        # End-to-end walkthrough
```

The split between `docs/` and `cookbook/`: `docs/` is reference, `cookbook/` is recipe. A contributor or AI agent reads cookbook entries when they want to *do* something specific; they read docs when they want to *understand* something.

`AGENTS.md` at the root specifies project rules — coding conventions, what to use the shim for vs. the daemon, when to write a test, when to update docs alongside code. AI agents working on the codebase read this first.

Every example in `examples/` must be runnable and tested in CI. Every cookbook entry must reference a working example. This prevents the cookbook from drifting from the actual API.

### Sizing the core

Following PocketFlow and pi-mono: the core should be small enough that the maintainer can hold it in working memory and any AI agent can read it quickly.

Rough targets:

- `protocol/` crate: ~500 lines (just types and serialization)
- `shim/` crate: ~500 lines (fast path; complexity is anti-feature)
- `daemon/` crate: ~3,000-5,000 lines (the bulk; storage, pub/sub, projection, hook router)
- `adapter-claude/` crate: ~1,000 lines (one good reference implementation)

Total: ~5,000-7,000 lines of Rust for the substrate proper. Plus ~2,000 lines of documentation, ~1,000 lines of examples.

This is comparable to opensessions' core (which is ~5,000 lines of TypeScript not counting watchers) and to pi-mono's `pi-coding-agent` package. It's far smaller than LangChain (>400k) or LangGraph (>50k). The implication: anyone interested in the project — human or AI — can read the entire core in an afternoon.

If the core starts growing past 10k lines, something is wrong. Either we've absorbed a "no" into the core (and need to extract it), or we've over-engineered an abstraction (and need to simplify it), or scope has expanded silently (and we need to update the no-list).

### Extension surface: what can third parties build without touching core?

The substrate should be extensible via four channels:

1. **New adapters.** A third party can ship an adapter for an agent we don't support, distributed as their own package. The adapter contract is:
   - Implement the wire protocol (POST events to daemon's ingest endpoint)
   - Provide `capabilities.yaml` and `tool-reactions.yaml`
   - Optionally provide an install script that wires their agent's hook config
   
   Distribution: any mechanism (`cargo install`, `homebrew tap`, raw curl). No central registry. The daemon discovers adapters at startup by reading a config directory (`~/.config/bowerbird/adapters/*`).

2. **Presenters.** Anyone can write a presenter against the documented pub/sub protocol. The daemon doesn't know or care who's listening. Distribution is whatever the presenter author wants.

3. **Capability extensions.** A community-supplied `capabilities.yaml` can declare additional capability flags (e.g., `has_voice_announcement_support`, `has_streaming_diff_view`) that presenters can negotiate against. The daemon passes these through; doesn't interpret them. This lets the capability surface grow without core changes.

4. **Event kinds.** A third party can POST event kinds the substrate has never seen (e.g., `orchestrator.delegationStarted`, `ide.fileOpened`). The daemon stores them and emits them on `events.<kind>.*` topics. Subscribers that care receive them; the substrate doesn't interpret. This lets new event types flow without core changes.

What you *cannot* do without changing core:

- Add new STATE topics (the projection layer decides what gets emitted)
- Change the reaction enum (extension would require coordination with all presenters)
- Add new persistence (events table is what it is)
- Change the wire protocol (versioned, careful evolution required)

This split — extension surface for things that don't require coordination, core changes for things that do — is what PocketFlow and pi-mono both pattern. It's the discipline that lets the core stay small.

### Versioning and stability promise

The wire protocol is versioned. Once `protocol@v1` ships, it's stable; breaking changes require `v2` with parallel support. Presenters declare what protocol version they expect; the daemon serves multiple versions concurrently if needed.

Internal implementation details — Rust struct layout, internal function signatures, even the SQLite schema — are not stable. The daemon can refactor freely as long as the wire protocol stays compatible.

This is the same split PocketFlow draws (the 100-line graph is stable; the cookbook examples are not) and pi-mono draws (the extension API is stable; the agent loop internals are not). It gives the maintainer freedom to improve internals without breaking the ecosystem.

### Documentation aimed at AI maintainers

Every doc in the repo should be written assuming the reader might be an AI agent doing maintenance work. Three concrete practices:

1. **`AGENTS.md` at the root** with project-specific rules:
   - "When adding a new event kind, update `protocol/src/event_kinds.rs`, the kind table in `docs/protocol.md`, and at least one cookbook entry."
   - "Hot-path code in `shim/` must not allocate on the success path. Use the benchmark in `shim/benches/hot_path.rs` to verify."
   - "Wire protocol changes require a `protocol@v{N+1}` package and an ADR in `docs/decisions/`."

2. **Architecture Decision Records (ADRs)** for load-bearing decisions:
   - 001 — Rust for shim and daemon (vs. Go or Node)
   - 002 — SQLite vs. embedded LMDB
   - 003 — Two-channel pub/sub (events.* and state.*)
   - 004 — 11-value reaction enum (vs. opensessions' 8 or AgentDeck's 6)
   - 005 — Hook-route shim (vs. file-tail-only ingest)

   Each ADR includes alternatives considered and why rejected. When a future AI agent considers reverting one, it can read the ADR and see the reasoning.

3. **Examples must be tested.** Every cookbook entry references a working example in `examples/`. CI runs the examples against a real daemon. If an example breaks because the API changed, CI fails, and either the example or the API gets fixed. No silent documentation drift.

### How feedback flows in without diluting scope

The pattern most likely to keep the project healthy:

- **GitHub Discussions are the front door.** New ideas go there first. The maintainer responds with one of:
  - "Yes, this fits — please file an issue / PR"
  - "This is on the no-list because <reason>. Have you considered <extension pathway>?"
  - "This is an adapter / presenter concern, not a core concern. Here's the contract for that."
  - "Interesting but speculative; let it bake. Come back if it still seems important in a month."
  
- **Issues are for confirmed work**: bug reports with reproductions, accepted features from discussions, regressions.

- **PRs from established contributors** get normal review. PRs from new contributors auto-closed with the templated message pointing back to discussions.

- **Adapter contributions** (new agent adapters that conform to the contract) are explicitly welcomed and reviewed faster than core PRs. This rewards the highest-value contribution shape.

- **The no-list is updated quarterly** based on what came up in discussions. A "no" that gets asked five times gets a dedicated explanation in the no-list rather than being answered in five separate threads.

### What the substrate can publish to demonstrate use

Both PocketFlow (cookbook) and pi-mono (published agent sessions) publicly demonstrate the maintainer using their own tool. For the substrate:

- **Two reference presenters in production for at least a week** before declaring MVP complete (already in `12-mvp-and-milestones.md`)
- **A weekly "agent state journal"** — a short blog post or README update showing real data from one of those production presenters. Shows the project is alive and that the maintainer uses it.
- **Cookbook examples backed by real use**: when a pattern appears in real production use, lift it into the cookbook. When a cookbook entry doesn't reflect real use, it gets retired.

The discipline: dogfood publicly. If you wouldn't use it yourself, fix it before asking others to.

## Risks of this approach

A few honest concerns:

- **Auto-closing new contributors can feel hostile**, especially for one-off bug fixes. Mitigate with a clear, friendly templated message that explicitly invites them to file a discussion first and links to past successful contributions for examples.

- **A small core means slower feature growth.** Some users will go elsewhere. That's the trade — the project is for people who want a stable, small, opinionated substrate, not a full-featured Swiss-army-knife.

- **An extension ecosystem may not materialize.** If the project's network effect doesn't kick in, the substrate stays a one-author project. Mitigate by making the *first* third-party adapter a deliberate effort: ship Codex adapter at M2 yourself, write the adapter-authoring guide with that experience, then invite contributions.

- **AI-readable documentation is a moving target.** The "AGENTS.md" pattern is novel and not yet load-tested across model versions. Mitigate by treating it as a living document; revise when models miss the point.

- **The no-list can ossify.** A decision made early might be wrong later. Mitigate by reviewing the no-list quarterly with a "should any of these change?" pass. ADRs help here — if reversing a no would also require reverting an ADR, you'll see the cost.

## Concrete actions for the project's first month

1. **Write the README.md as a single page** that includes: what the substrate is, what it's not (top three no-list items), how to install, how to subscribe to events. Maximum ~200 lines.

2. **Write AGENTS.md** with project conventions for human and AI contributors.

3. **Open GitHub Discussions** and the templated auto-close message. Pin the no-list explanation.

4. **Ship MVP** per `12-mvp-and-milestones.md`. Two reference presenters working in production.

5. **Write the first three ADRs**: Rust choice, two-channel pub/sub, reaction enum. Date them. Show alternatives.

6. **Write four cookbook entries**: subscribe to state, subscribe to events, compute activity rate client-side, handle dropped frames. Each backed by a working example.

7. **Write the adapter-authoring guide** using the Codex adapter as the worked example. This is M2's actual deliverable from a contributor's perspective.

8. **Start the public "agent state journal"** — every Friday, a short post about something the maintainer noticed about their own use.

That's the first month of project hygiene work. Everything after is feature work against the milestones.

## The principle this serves

PocketFlow and pi-mono both demonstrate a counter-intuitive truth: **the more aggressively you constrain the core, the more useful the ecosystem becomes**. A framework that tries to be everything to everyone ends up being mediocre at most things. A framework that does one thing well and extends through clean boundaries can accumulate quality without accumulating complexity.

The substrate's "one thing" is: observe agent state, expose it via pub/sub, never get in the way. Everything else is extension. The contribution model and documentation model should reinforce that, every day.

The maintainer's job is to defend the smallness as fiercely as they champion the usefulness. That tension is the project's whole shape.