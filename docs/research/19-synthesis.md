# State of the design and unexplored options

A single-page synthesis of where the design landed, what artifacts exist, what decisions were made, and what threads we left unpulled. Written so a future reader (human or AI) can pick this up without reading all 17 design docs.

## What we set out to do

The premise from `01-findings.md`: dozens of tools (pets, dashboards, statuslines, lamps, sprites, voice assistants) each independently install Claude Code hooks, tail JSONL transcripts, hijack the statusline, or maintain shadow state files. They collide. They re-implement the same plumbing. They each break differently when Claude's transcript format shifts.

The goal: design a substrate that ingests once and exposes agent state to many presenters cheaply, so the next person who wants a creature that paws at the screen when Claude is thinking writes ~150 lines of presenter code instead of a from-scratch hook system.

## What we converged on

A local daemon called `bowerbird` with five load-bearing components:

1. **A static-binary hook shim** (Rust, <5ms cold start) installed into `~/.claude/settings.json` once. Fans hook events out to the daemon. Never blocks Claude.

2. **A SQLite-backed event log** preserving native payloads verbatim. `(source, session_id)` is the natural key.

3. **A projection layer** that derives a small set of session-row fields (`current_state` from the 11-value OpenPets-derived reaction enum, `lifecycle`, `last_event_at`, derived git fields).

4. **A pub/sub WebSocket** with two channels:
   - `events.*` — raw events with hierarchical topic filters
   - `state.*` — derived state transitions with old/new values
   
   Snapshot-on-subscribe for STATE topics. Bounded queues with `dropped` frames for backpressure.

5. **A REST polling fallback** for tools that don't want WebSocket.

Plus an explicit no-list (`17-no-list.md`) and a contribution model borrowed from pi-mono (auto-close new contributor PRs, review weekly).

The substrate's job is to preserve native data and expose it cheaply. The principle from `07-agent-type-and-foundations.md` that governs every other decision: **the substrate doesn't define application-level concepts on top of underlying data.** Personas, voices, sprites, moods, color palettes, activity rates, idle thresholds — all presenter side.

## Artifacts produced

17 design docs, plus a README draft and an AGENTS.md draft. Roughly 6,500 lines total.

| Doc | Purpose |
|---|---|
| 01-findings | Initial inventory of ~30 tools, the hook-collision problem, the substrate hypothesis |
| 02-detailed-inventory | Deep dive per tool: ingest model, state vocabulary, hook installs, terminal attribution |
| 03-design-sketch | v1 of the design — events, sessions, agents, attachments, the API surface |
| 04-design-vs-inventory | Critique walkthrough: which inventoried tools are served by v1 and which aren't |
| 06-missing-concepts | Gaps from a novelty-tool survey (the 05 slot ended up unused) |
| 07-agent-type-and-foundations | The foundational-vs-buildable principle that became the design's North Star |
| 08-design-sketch-v2 | Clean rewrite after applying the preserve-native-data principle |
| 09-multi-agent-support | Tier 1 (Claude/Codex/Gemini/Cursor), Tier 2 (OpenCode/OpenClaw), Tier 3 (Aider/Copilot) analysis |
| 10-multi-agent-tool-patterns | Examined 5 existing multi-agent tools (opensessions, ccmanager, agent-flow, AgentDeck, Agent Sessions); five ingest models in the wild |
| 11-design-sketch-v2-1 | Pub/sub channels, capabilities surface, liveness/lifecycle split |
| 12-mvp-and-milestones | MVP scope and M2-M6 roadmap |
| 13-test-cases | Two test cases (TUI grouped by remote, 8-bit sprite app) walked against v2.1 |
| 14-activity-survey | Survey of 8 tools' activity-measurement approaches; reversed the activity-counter proposal |
| 15-opensessions-gap-analysis | What opensessions has, what it lacks, contribution viability assessment |
| 16-maintainership-and-scope | PocketFlow/pi-mono-style maintainership and scope discipline |
| 17-no-list | Out-of-scope register: 6 "never" + 9 "not yet" items |
| 18-cookbook-catalog | 24 cookbook recipes, no external APIs or hardware, all CI-testable |
| README-draft | Single-page project intro for the repo |
| AGENTS-draft | Project rules for human and AI contributors |

The numbered docs build on each other in order. Reading 01, 07, 08, 11, 12, and 17 gives the core design in ~5,000 lines of doc. The rest are supporting analyses and corrections.

## Major decisions and what we didn't pick

A few load-bearing choices where we had real alternatives:

### Language and runtime: Rust shim and daemon

**What we picked:** Rust everywhere. Static binary for the shim (sub-5ms cold start), tokio + axum + rusqlite for the daemon.

**Alternatives considered:** Go (also meets the cold-start budget, easier contribution surface, but mixing languages with the rest of the substrate adds friction); Node/Bun (cold-start budget violated by Node startup alone, ~50-100ms); Python with compilation (Mojo, RustPython embedding — fragile, novel, not justified by gain).

**Why we picked Rust:** the shim must be sub-5ms p95; Node and unembedded Python are out by an order of magnitude; Go works but mixing languages with the rest of the codebase adds drift. Rust everywhere keeps the codebase coherent.

**What this closed off:** the lower bar to entry that comes with Node/Bun (familiar to the JavaScript-heavy presenter ecosystem) or Go (popular for daemons). If contribution volume disappoints, this is worth revisiting — but the shim has to stay in something that doesn't have a runtime warmup.

### Storage: SQLite with WAL

**What we picked:** Embedded SQLite, WAL mode, `synchronous=NORMAL`.

**Alternatives considered:** LMDB (smaller, faster, but no SQL surface; harder to inspect from a curl command); a custom append-only log file (simplest, but reimplements indexing); Postgres (overkill, requires a separate process).

**Why we picked SQLite:** debuggable from `sqlite3` CLI; reasonable performance for tens-of-thousands events per session; well-understood crash semantics. The substrate doesn't need exotic storage.

**What this closed off:** the option of zero-process operation. A custom log file would let users do `tail -f` on event history without going through the daemon. Probably not worth the complexity.

### Pub/sub model: two channels, hierarchical topics

**What we picked:** Two conceptual channels (`events.*` and `state.*`) over one WebSocket. NATS-style hierarchical topics with `*` wildcards. Snapshot-on-subscribe for STATE.

**Alternatives considered:**

- **One channel, server-side query filters** (the v1 design's shape). Rejected because every consumer would re-implement projection logic to derive state transitions from raw events. The whole point of the projection layer is to do that once.
- **Multiple physical channels** (one WS for events, one for state, possibly more for different domains). Rejected because the operational complexity (presenters managing N connections) isn't justified by the conceptual split.
- **gRPC streams** instead of WebSocket JSON. Rejected because the debuggable-from-curl property is valuable for early users.
- **Server-Sent Events** (one-way push, simpler than WS). Rejected because some operations (subscribe, unsubscribe, auth) want a request-response shape.

**What this closed off:** the option of per-topic physical isolation (a slow consumer on `events.*` won't slow down `state.*` consumers under the current model, but it shares the connection's bounded queue). If backpressure becomes a real problem in practice, splitting the channels physically is the escape hatch.

### Reaction enum: 11 values from OpenPets

**What we picked:** The OpenPets canonical 11: `idle / thinking / working / editing / running / testing / waiting / waving / success / error / celebrating`.

**Alternatives considered:**

- **opensessions' 8** (`idle / running / tool-running / done / error / waiting / interrupted / stale`). Narrower, oriented toward TUI display, no `editing`/`testing`/`celebrating`.
- **ccmanager's 4** (`idle / busy / waiting_input / pending_auto_approval`). Minimal, would force pet-style presenters to hold their own richer projection.
- **claude-status' 4** (`active / waiting / idle / compacting`). Has the unique `compacting` value we don't expose.
- **Inventing our own.** Rejected because the OpenPets enum is already in use across a real ecosystem (OpenPets and derivative pet tools); adopting it preserves compatibility.

**Why 11:** richer presenters (pets, sprites, voice) want the variety. Simpler presenters (TUI dashboards) collapse it client-side — recipe 2.1 in the cookbook shows the canonical 4-state collapse.

**What this closed off:** the cleaner extensibility that comes with a smaller core enum. If a presenter ever wants `compacting` as distinct from `working`, that's a 12th value to negotiate with all consumers. We don't have a clear answer for how the enum evolves.

### Provider abstraction: three classes

**What we picked:** HookProvider (Claude/Codex/Gemini/Cursor), PluginProvider (OpenCode/OpenClaw), TranscriptProvider (Aider/Copilot). MVP ships only HookProvider.

**Alternatives considered:**

- **A single provider abstraction** that handles all ingest models internally. Rejected because the operational shape is genuinely different (hook = config installation + shim; plugin = in-process TypeScript; transcript = file watcher).
- **Five provider classes** (HookProvider, PluginProvider, TranscriptProvider, SQLiteProvider, CloudProvider) as opensessions effectively has. Rejected as overspecified for what's likely needed; we lifted opensessions' watcher contract but documented three tiers since SQLite and Cloud are special cases of "Plugin" with different transport.

**What this closed off:** explicit guidance for the SQLite-polling case (OpenCode) and the cloud-REST+WS case (Amp). Those folded under PluginProvider; if they need real differentiation, three becomes four becomes five.

### Capabilities matrix

**What we picked:** Per-source `capabilities.yaml` with boolean flags (`has_permission_payload`, `has_subagents`, etc.) plus a `reaction_enum_subset` array.

**Alternatives considered:**

- **No capabilities surface; treat all sources as equivalent.** Rejected because AgentDeck's experience shows presenters genuinely need to negotiate (mode-switching exists in Claude but not Codex; permission payloads vary in shape).
- **A capabilities schema with arbitrary key types** (numbers, ranges, structured config). Rejected as premature; booleans plus enum-subset cover every observed case.
- **Runtime capability discovery** (presenters probe each source for what it supports). Rejected because it duplicates effort; static config from the adapter is fine.

**What this closed off:** the ability for presenters to negotiate richer capabilities (e.g., "this source supports streaming diff updates"). Adding that later is additive, so the cost is small.

### Activity rate computation: client-side only

**What we picked:** No daemon-side activity counters. `last_event_at` exposed; presenters compute their own rate.

**Alternatives considered:**

- **A `recent_event_count_60s` derived field on session row** plus a `state.session.<id>.activity` topic. Originally proposed in `13-test-cases.md`. Reversed in `14-activity-survey.md` after surveying 8 tools and finding only one actually wants window-based rate.
- **Multiple rate windows** (`60s`, `5min`, `1hr`) for different presenter needs. Rejected for the same reason as above.
- **A leaky-bucket scalar instead of a window** (claude-quest's pattern, naturally smoothed). Rejected for the same reason; presenters that want this compute it.

**What this closed off:** out-of-the-box ergonomics for the one pet tool (tamagotchi) that actively wants a rate. That presenter pays ~6 lines for the sliding window. Cheap trade.

### Maintainership: pi-mono-style auto-close

**What we picked:** Auto-close PRs and issues from new contributors with a templated message. Maintainer reviews weekly. Adapter PRs treated differently (faster review).

**Alternatives considered:**

- **Default-accept-with-review** (the conventional model). Rejected because it creates an unsustainable maintenance load for a single-author project and dilutes architectural coherence.
- **Maintainer-only contributions** (PocketFlow's stricter stance). Rejected as too closed — adapter PRs in particular need community participation to scale.
- **Tiered access** (regular contributors get faster review). The auto-close model is effectively this; "regular contributor" is what you become after your first non-auto-closed interaction.

**What this closed off:** the "soft" feeling of an open-arms project. The signal sent is clear: this is opinionated and maintainer-led. Some potential contributors will move on. That's the trade.

## When given options, what we deferred

These are the times across the conversation when there were 3+ explicit choices and we picked one (or none). Each is fair game to revisit.

### After doc 01 (findings) — three directions to go

The options at the end of doc 01 were:
- **(a)** Push deeper on the inventory side
- **(b)** Move to the design side
- **(c)** Apply the design lens to a specific tool

We took **(a)** then **(b)**. **(c)** never happened in depth — we never picked a single tool and said "let's design exactly what would replace its implementation." The closest we got was the opensessions analysis in doc 15, but that was a gap analysis, not a "let's design exactly the substrate that would let opensessions delete 80% of its code."

**Worth revisiting:** picking 1-2 specific tools (probably claude-lamp, ccpet) and writing the "your tool against the substrate" walkthrough as a contributor recruitment artifact.

### After doc 06 (missing concepts) — three directions

- **(a)** Apply the missing concepts to redesign v2
- **(b)** Examine one of the surveyed novelty tools deeply
- **(c)** Step back and ask whether the substrate's framing is right at all

We took **(a)** (became doc 07, then doc 08). **(b)** got partially done through tool-by-tool examinations in later docs. **(c)** never happened — we didn't seriously challenge the substrate premise itself. The closest was doc 14's reversal of activity counters, but that was tactical.

**Worth revisiting:** an adversarial review of the whole premise. Is the hook-collision problem actually big enough to justify a daemon? Could it be solved with a smaller intervention (like a community hook merger script)?

### After doc 10 (multi-agent patterns) — four directions

- **(a)** Write capabilities YAML for tier-1 agents
- **(b)** Draft a v2.1 design with corrections
- **(c)** Pivot to "minimum viable prototype" given everything we now know
- **(d)** Look at one of the multi-agent tools through this maintenance lens

We took **(b)** (became doc 11). **(c)** became doc 12. **(a)** and **(d)** never happened.

**Worth revisiting:** **(a)** is the most concrete — sketching the actual `capabilities.yaml` for Claude / Codex / Gemini / Cursor based on AgentDeck's matrix would surface real edge cases. Could be a half-day exercise.

### After doc 11 (design sketch v2.1) — three directions

- **(a)** Capabilities YAML for tier-1 agents
- **(b)** Wire-protocol spec as TypeScript types
- **(c)** Adapter-author guide with Claude as worked example
- **(d)** Pivot to implementation choices (Rust crate structure, hook router state machine, schema migrations)

We took **(d)** partially via doc 12 (MVP plan touches Rust choices). **(b)** and **(c)** are unwritten.

**Worth revisiting:** **(b)** — the wire protocol spec is the contract that everything else hangs off. Drafting it as concrete TypeScript types (or JSON Schema) would make the substrate testable in isolation. This is probably the single most leveraged unwritten artifact.

### After doc 12 (MVP) — four directions

- **(a)** Wire-protocol spec
- **(b)** Adapter-author guide
- **(c)** Architectural decision records
- **(d)** Pitch memo for specific developers (Pablo at Pixel Agents, Alvin at OpenPets, patoles at agent-flow, puritysb at AgentDeck)

None of these happened directly. The closest was doc 13 (test cases) which validated the design but didn't reach to any of these four.

**Worth revisiting:** **(d)** is interesting strategically — at some point the project's success depends on convincing other tool authors to consume from it. Drafting the pitch (what would convince Alvin Unreal to make OpenPets a substrate consumer instead of standalone?) would force concreteness about the substrate's value proposition to existing tool authors.

### After doc 13 (test cases) — three directions

- **(a)** Apply corrections to docs 11/12/13
- **(b)** Survey-test capabilities the way we survey-tested activity
- **(c)** Wire-protocol spec
- **(d)** Implementation choices

We took **(a)** (which led to doc 14, the activity survey, and then re-corrections). **(b)** is unwritten — we never did a survey-against-evidence test for whether the capabilities matrix is the right shape.

**Worth revisiting:** **(b)** — given how productive doc 14's discipline was for activity counters, applying the same lens to capabilities is plausible. Which 4 tools genuinely need feature negotiation vs. which would just key on `source`?

### After doc 15 (opensessions analysis) — four directions

- **(a)** Draft the GitHub discussion/issue to opensessions maintainers
- **(b)** Sketch the watcher-extraction proposal as a published package
- **(c)** Continue with wire protocol or implementation choices
- **(d)** Look at the opensessions PRs/issues directly

None of these happened — we went to doc 16 (maintainership) instead, which was a different thread.

**Worth revisiting:** **(a)** is interesting because it would be cheap (drafting an issue is a paragraph) and high-leverage (a positive response could substantially change the contribution path).

### After doc 17 (no-list) — five drafts to write

- **(a)** Templated auto-close message
- **(b)** First three ADRs (Rust, two-channel pub/sub, reaction enum)
- **(c)** Adapter-authoring guide using Claude as worked example
- **(d)** Wire protocol spec
- **(e)** Full doc-set review for consistency

None of these happened. Doc 18 (cookbook) was a different thread.

**Worth revisiting:** **(b)** is structurally important — the ADRs anchor decisions so they don't get relitigated. The three we'd write first are exactly the three load-bearing choices from the "What we picked" section above. Each is ~80-150 lines.

### After doc 18 (cookbook) — five directions

- **(a)** Draft one cookbook entry in full as an example
- **(b)** Wire protocol spec
- **(c)** First three ADRs
- **(d)** Doc-set review

This synthesis is **(d)** as far as it goes. **(a)**, **(b)**, **(c)** remain.

## What's actually unresolved (vs. just unwritten)

Most of the unwritten artifacts above are mechanical — they'd just be more documents in the design tradition. A smaller list of things are *actually unresolved* and would shift the design if surfaced:

### Wire protocol stability promise

The design says "the wire protocol is versioned." It doesn't say what specifically falls under that promise. Does the *content* of `events.<kind>` payloads count? If we add an optional field to `permissionRequest` payloads, is that a breaking change?

**Why this matters:** the entire claim that "presenters and adapters can evolve independently" hinges on a clear answer here. Some sketches:

- **Strict:** every byte of every payload is contract; adding fields requires v2.
- **Loose:** the topic shape and frame structure is contract, payload is best-effort.
- **Tiered:** event envelope is strict, payload is loose (passes through verbatim) with documented schema-on-best-effort.

The third is probably right but we never picked it explicitly.

### Whether the daemon should own session lifecycle transitions

The design says `lifecycle` is `live | paused | abandoned | ended`. `live → ended` happens on `SessionEnd`. `live → paused` happens on `Stop`. But who transitions `paused → abandoned`? A daemon sweep, per the deferred M7+ work — but that means presenters today see sessions stuck in `paused` indefinitely.

**Why this matters:** test case 2 (sprite app) wanted death-on-process-death. Liveness handles that. But the conceptual question of "who is the authority on lifecycle transitions over time" isn't fully resolved.

### What happens when two adapters claim the same session

If somehow Claude and Codex both think they own session ID `abc-123` (different agents, same UUID), what does the substrate do? The current key is `(source, session_id)` so they don't actually collide at storage time. But cross-source presenters that group by `session_id` alone will see both. We never wrote down what the daemon does when this happens (the answer is probably "nothing; presenters group correctly using the natural key").

### Authentication model beyond MVP

MVP uses a per-daemon-run token written to `~/.bowerbird/server.json`. This is single-user; the daemon trusts anyone with the token.

**Future questions we haven't answered:** 
- Per-presenter tokens with capability scopes?
- Read-only vs. read-write distinction (orchestrators emit events; presenters only consume)?
- Auth for the LAN case if we ever ship cross-machine pub/sub?

### What "in production for a week" actually means for MVP success

The MVP success criteria are "claude-lamp and a PAI-style voice presenter both work for a week." But what counts as "works"? Number of dropped frames below some threshold? Number of presenter restarts? Number of times the user noticed the presenter was wrong?

This is the kind of thing that gets answered by actually shipping. But it's worth naming as unresolved before then.

### Whether the substrate should ever own user notification policy

"User attention needed" is presenter concern by design. But what if every presenter wants the same definition? A `permissionRequest` event always means "user needs to look here." A `current_state: waiting` always means "user input requested." We could expose a single derived `needs_user_attention` boolean. We deliberately don't, but the line is thinner than the no-list claims.

## What the next session of work should probably be

In order of leverage:

1. **Wire protocol spec** (doc 11's "(b)" — unwritten). The single artifact that anchors everything else. Probably 300-500 lines of TypeScript types or JSON Schema plus prose.

2. **First three ADRs** (Rust choice, two-channel pub/sub, reaction enum). The "alternatives considered" section above gives most of the content; formalizing into ADRs is a few hours.

3. **Draft an adapter-authoring guide** using Claude as the worked example (Codex doesn't exist yet at MVP). When M2 lands, the contributor can follow the guide rather than reverse-engineering.

4. **Survey-test the capabilities matrix** the way doc 14 survey-tested activity counters. Genuinely test whether multi-agent presenters need feature negotiation or whether they'd happily key on `source`.

5. **Implementation**, starting from the shim's hot path (it's the smallest piece and the part that's most performance-critical, so getting it right early de-risks the rest).

Lower-leverage but worth doing eventually:

6. Templated auto-close message
7. The actual `capabilities.yaml` files for Claude / Codex / Gemini / Cursor (which is partly content for the adapter-authoring guide)
8. A pitch memo to specific tool authors (whose buy-in would materially change the substrate's adoption)
9. The opensessions outreach (a discussion or issue) per doc 15's recommended path
10. A "what would change if the substrate didn't exist" devil's-advocate review

## What I'd do differently if starting over

A few things I'd revisit if walking the design path again:

- **Skip docs 03 and 04.** The v1 design was a useful waypoint but doc 08 (v2) almost entirely replaces it. The path could have been shorter if doc 06 had come first (foundational principles → design, rather than design → critique → principles → redesign).

- **Do the activity survey (doc 14) before adding activity counters in doc 13.** The reversal was cheap but writing the proposal in doc 13 anchored thinking in a direction that turned out to be wrong. The survey-first discipline should have been the default.

- **Examine opensessions earlier.** Doc 15 came at position 15 of 18 but the project has been the closest existing analog the whole time. Looking at it earlier would have surfaced the AgentWatcher abstraction shape before we sketched our own.

- **Pick a tool early and design against it.** Doc 13's test cases were the most productive validation exercise but came late. Two test cases up front (a pet, a multi-session TUI) would have anchored every intermediate design decision in concrete needs.

## Closing observation

The discipline that did the most work across the whole design is the one from doc 07 — **the substrate preserves underlying data and resists modeling application-level concepts on top.** Every major correction (dropping activity counters in doc 14, keeping idle thresholds presenter-side in doc 13, refusing HITL backflow throughout, treating personas/voices/sprites as off-design) traces back to this principle.

The second most useful discipline is from doc 16 — **constraining the core ferociously is what makes the ecosystem useful.** It's the PocketFlow and pi-mono lesson. The temptation to absorb capabilities into the core is constant; resisting that temptation is the maintainer's actual job.

Both disciplines reinforce each other: smaller core, clearer boundaries, more capable presenters.

The design is ready for implementation. The unwritten artifacts above are valuable but not blocking. What's blocking is shipping the shim, the daemon, the Claude adapter, and the two reference presenters, and seeing what survives contact with reality.