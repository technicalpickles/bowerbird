# agent_type, persona, and where the foundation ends

## How subagent identity actually works in Claude Code

Subagents in Claude Code are **declaratively defined**, not orchestrator-generated. The mechanism, end to end:

### 1. Definition

The user creates Markdown files with YAML frontmatter:

```
.claude/agents/code-reviewer.md       ← project scope
~/.claude/agents/researcher.md         ← user scope
```

The frontmatter carries the metadata, the body is the system prompt:

```yaml
---
name: code-reviewer
description: Expert code review specialist. Proactively reviews code
  for quality, security, and maintainability.
tools: Read, Grep, Glob, Bash
model: sonnet
---
You are a senior code reviewer ensuring high standards of code quality
and security. When invoked: 1. Run git diff...
```

There are also three built-in subagent types (`Explore`, `Plan`, `General-purpose`) that ship with Claude Code.

### 2. Invocation

The main Claude session decides to delegate and calls the `Task` tool (also called `Agent`) with `subagent_type: "code-reviewer"`. Either Claude picks the subagent based on task characteristics (auto-routing, imperfect), or the user invokes one explicitly with `@code-reviewer ...` or `/agents`.

The decision is made *by the agent itself*. The harness (Claude Code) reads the matching `.md` file, spawns a subagent with that system prompt, that tool set, that model, and an isolated 200K-token context.

### 3. Hook payload

When hooks fire **inside that subagent**, the payload carries both fields, straight from Anthropic's SDK documentation: *"agent_id and agent_type are populated when the hook fires inside a subagent."*

- `agent_type: "code-reviewer"` — the stable, declarative identifier
- `agent_id: "<uuid>"` — the per-run instance ID

`SubagentStart` and `SubagentStop` carry these. `PreToolUse`, `PostToolUse`, and `PostToolUseFailure` carry them when fired inside a subagent.

This means **`agent_type` is a first-class native field. The daemon doesn't have to invent it, derive it, or orchestrate anything.** It just needs to preserve it on the event and expose it through the read API.

### 4. How tools use it

Three distinct strategies show up in the wild:

**Strategy A — Read it directly (most common).** PAI and AgentVibes read `subagent_type` from the hook payload, look it up in `~/.claude/agent-voices/voices.json`, and route TTS to the configured voice. Pixel Agents reads it to route to the right teammate seat. tmux-agent-sidebar stores `agent_type:agent_id` in tmux pane options. None of these do orchestration; they only consume what hooks already provide.

**Strategy B — Layered orchestration.** PAI's "BmadBridge" detects when a *BMAD* agent (Mary, Winston, Carson) is being invoked through a higher-level skill, and overrides the voice mapping accordingly. The BMAD layer sits *above* Claude Code's native subagent system. The hook code does pattern matching on output text (`if (output.includes('Mary') || subagentType === 'mary')`) because BMAD-defined agents may or may not surface as native `subagent_type` values.

**Strategy C — Invent personas (Outworked, claude-office).** These tools *are* the orchestrator. Outworked generates its own employee personas with name + role + personality, builds the system prompt itself, and launches Claude with that prompt. The resulting Claude session may have its own subagents internally, but Outworked's "agents" are a layer above Claude Code's subagent concept — they're Outworked-defined personas applied to whole sessions, not Claude Code subagents.

**The key observation:** strategies A and B consume `agent_type` from native hooks. Strategy C invents persona at the orchestration layer. **The daemon only needs to support strategy A/B's data flow.** Strategy C tools (Outworked) bring their own persona system; the daemon doesn't need a persona concept of its own to serve them — they consume the daemon's session/agent state and overlay their own metadata on top.

---

## Foundational vs. buildable: the right lens

The earlier "missing concepts" doc bundled two questions:

1. Can the daemon expose enough data that this tool can be built on it?
2. Should the daemon itself implement this feature?

Those are different. The first is the foundational question. The second is a scope question that should usually be answered "no" — the design's goal is to be a substrate, not a finished UI stack.

Walking the nine gaps through this lens:

### Foundational (the daemon doesn't expose enough; can't be built on top)

These are cases where the data simply isn't reachable without re-implementing event ingestion, which is the thing the daemon exists to consolidate.

**1. `agent_type` and `agent_id` on events.** Already in the hook payload, but the design's event vocabulary doesn't preserve `agent_type` as a first-class field on the event. Adding `agent_type` and `agent_id` to every event (where present in the payload) lets every strategy-A and strategy-B tool consume from the daemon instead of installing hooks. **This is genuinely foundational — without it, every voice/sprite/teammate-routing tool reimplements the hook payload parsing.**

**2. Permission request payload (question text, options, tool input).** Hardware approval surfaces need this content to render anything meaningful. The payload is already on the hook event; the daemon needs to preserve it on the `permissionRequest` event. Without it, no presenter can show a meaningful approval prompt. **Foundational — same logic as agent_type. Pure preservation of native data.**

**3. Cumulative session metrics, exposed via a cheap query.** Pets, statuslines, and dashboards all want simple counters. The events are there; what's missing is a query interface that doesn't require scanning the firehose. **Foundational in shape, trivial in substance** — it's just a per-session row update. Whether the daemon exposes it as `GET /sessions/:id/stats` or a presenter computes it from the event stream is a thin line, but landing it in the daemon avoids N reimplementations.

### Buildable on top (no daemon change needed if the foundational data is exposed)

These are features where, once `agent_type` and tool events and tokens flow through, every presenter can build the feature themselves with a small config file.

**4. Persona / display name / voice mapping.** PAI's `voices.json`, Outworked's agent definitions, AgentVibes' voice slots, etr/bells-and-whistles' themed packs — all presenter-side mappings keyed on `agent_type`. The daemon shouldn't have a `display_name` or `description` column. Presenters maintain their own files. **The foundation needed is `agent_type` on every event** (item 1 above). Everything else is built on top.

**5. Per-model cost rollup.** Once `tokens` and `cost` events carry the model name, anyone can roll them up. Useful as a built-in query for convenience, but not foundational — claude-receipts, ccusage, Discord Rich Presence can all aggregate the event stream themselves if they want. Worth adding only if many presenters want it.

**6. Context window utilization.** Statusline tap emits the current context %. The daemon needs to preserve it on the heartbeat event (or whatever event carries statusline data). Once it's on an event, the HP-bar tools build their own bar. **The foundation is preserving the statusline payload faithfully**; the bars are presenter-side.

### Out of scope (legitimately not the daemon's job)

**7. HITL backflow.** Genuinely a different abstraction (bidirectional, blocking, auth-sensitive). If the survey turns up many more examples, revisit. The current evidence is real but the count is modest — keep it as an extension surface the design *anticipates* but doesn't *implement*. The daemon stays one-way.

**8. LAN reachability and mDNS discovery.** Narrower scope. A presenter that wants to be reached from a phone/watch/ESP32 can ship its own LAN listener that subscribes to the localhost daemon. The daemon doesn't need to be a LAN service to enable LAN tools. AgentDeck already does this — its bridge is the LAN listener; the daemon would just be one of its data sources.

**9. Codex / OpenCode adapter sketch.** Worth doing for *validation* — to prove the abstraction holds — but not because the daemon must ship the adapter in v1. Document the abstraction in enough detail that someone can write the adapter; ship it later.

---

## The corrected list of foundational additions

Boiling it down to what the daemon genuinely needs to support its stated goal of "be a good baseline that others can build on":

### Required to be a real baseline

- **`agent_type` and `agent_id` preserved on every event where the hook payload carries them.** Without this, every persona/voice/sprite tool reimplements hook ingestion. One-line schema change, one-line passthrough in the adapter.
- **Faithful preservation of native hook payload data** on every event. This is the principle, not a single feature. If Claude Code's hook gave it to us, the event should carry it: `agent_type`, `agent_id`, `tool_use_id`, `tool_input`, `tool_response`, the full permission question and options, `transcript_path`, `cwd`, `model`, etc. The daemon's `payload` field on events is the right place. The temptation to "normalize away" native fields should be resisted; that's how presenters end up re-tailing JSONL to recover what got dropped.
- **Statusline payload preserved** on heartbeat events. Token counts, context %, model, session ID. Statusline is the only source for some of this; throwing it away forces re-derivation.

That's it. Three things.

### Useful but optional

- Cheap per-session aggregation queries (`GET /sessions/:id/stats`) so pets don't scan the event log every tick. Implementation: trivial; convenience matters more than the API surface.
- Per-model rollup query. Same logic.

### Explicitly not the daemon's job

- Persona / display name / role / voice / sprite mapping (presenter config files)
- HITL backflow (different abstraction; revisit if evidence grows)
- LAN reachability (presenter-side, not daemon-side)
- Multi-runtime providers shipped in v1 (document the abstraction; ship later)

---

## The general principle

The daemon's job is to **preserve and expose the underlying data**, not to define application-level concepts on top of it. `agent_type` is foundational because it's native data that's currently being thrown away by the design. "Persona" is not foundational because it's a presenter concept that varies per tool and is best expressed as a config file the presenter owns.

The test for "should this be in the daemon" is roughly:

1. Is this *native data* (from hooks, JSONL, statusline) that presenters need but currently can't access without reimplementing ingestion? → Yes, daemon should preserve it.
2. Is this a *derived aggregation* that many presenters compute redundantly? → Convenience; worth a query API if many tools need it, otherwise let them compute.
3. Is this an *application-level concept* layered on top of agent state? → Presenter responsibility; daemon stays out.

By that test, the original design dropped item 1 (`agent_type`, full permission payload, full statusline) in places where it shouldn't have. Most of the rest of the "gaps" from the survey are item 3 (presenter concepts) or item 2 (nice-to-have aggregations) — buildable on top of a correctly-preserving daemon.