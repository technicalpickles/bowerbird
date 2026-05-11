
You said: There are tools for claude code that do things like show visualizations of what agents are doing, so you can get a glance at what agents are idle, working etc.
There are tools for claude code that do things like show visualizations of what agents are doing, so you can get a glance at what agents are idle, working etc. some times they take the form of plugins with hooks, to track. The problem i see is they are all doing the same hooks, and it’s hard to seitch around

I had this idea of making a generalized version. Instead of every tool doing its own thing, we do one general way that tracks the state, and exposes an api others can use. This could be both thought as a state query api and an event system

I want to do an inventory of these types of apps out there. Cmux is one, i’ve seen onew that try to emulate video games. I’ve written my own (agenticpets).

I want to build this list first, then review the features they off, and then look at how they are implemented

May 10
Claude responded: I'll help you build this inventory.
I'll help you build this inventory. Let me search for what's out there in this space.


Searched the web
Searched the web
Good start. Let me search for more, including the specific ones you mentioned and the gamified ones.


Searched the web
Searched the web
Two different things called cmux — the Manaflow native macOS terminal and Craig Sc's tmux-for-Claude-Code. Good to note. Let me search for the gamified ones.


Searched the web
Searched the web
Let me look for more variety — the video game style, observability tools, and any orchestration/swarm dashboards.


Searched the web
Searched the web
Let me search for the video game / arcade style ones specifically, plus ccmanager and other parallel session managers.


Searched the web
Searched the web
Excellent finds. Pixel Agents is a major one. Let me look for a few more — ccmanager, the orchestrators, and any TUI dashboards.


Searched the web
Searched the web
Let me also check on agenticpets (your project) and a few more orchestration tools — claude squad, opcode, vibe-kanban.


Searched the web
Searched the web
Let me check on agenticpets specifically (your project) and look for a few more specific things — including any state-API or middleware approaches that might already exist.


Searched the web
Searched the web
Excellent — found OpenPets which is very interesting because it uses MCP, plus ccpet. Let me find your agenticpets specifically by searching past chats and the web.


Relevant chats
Relevant chats
The search didn't surface your agenticpets project directly — let me check the web more broadly and also look for agent state APIs / middleware concepts.


Searched the web
Searched the web
Your agenticpets project doesn't surface in web search — likely it's personal/private. I have enough to build the inventory now. Let me put it together.

I have a solid inventory now — 25+ tools across the major categories. Here's the list, organized by approach:

Inventory: Claude Code agent state/visualization tools
Hook-driven dashboards (web UI, observability-style)
These all consume Claude Code's hook events (PreToolUse, PostToolUse, Stop, SubagentStart/Stop, SessionStart/End, Notification, UserPromptSubmit) and stream them somewhere.

disler/claude-code-hooks-multi-agent-observability — the canonical one, often referenced as the prior art. Bun/TypeScript server, swim-lane filtering, live pulse chart, SQLite WAL.
hoangsonww/Claude-Code-Agent-Monitor — Node/Express/React/SQLite/WebSockets, Kanban board, sessions list, analytics, "Run Claude" live stream with --include-partial-messages, separate VS Code extension, separate plugin marketplace pieces.
simple10/agents-observe — Docker-based, installed as a plugin, has /observe slash commands for status/debug.
mukul975/claude-team-dashboard — D3 force-directed graph of inter-agent comms, task dependency graph, watches ~/.claude/teams/ directory.
nexus-labs-automation/agent-observability — more of a guidance/instrumentation skill pack, but worth listing.
Marc Nuri's coding agent dashboard — heartbeat model, enricher pattern, WebSocket terminal relay; written up as a blog post architecture.
Cloud / OTel-based observability
Dynatrace Claude Code Monitoring — uses Claude Code's built-in OpenTelemetry export. Different lane entirely from hooks.
Anthropic Managed Agents dashboard — Anthropic's own first-party control plane.
Statusline pets / single-session companions
Ido-Levi/claude-code-tamagotchi — statusline pet with violation detection via PreToolUse hook + Groq LLM analysis of transcript.
terryso/ccpet — simpler statusline pet, energy decays/grows from token consumption, has a global leaderboard at ccpet.surge.sh.
Anthropic /buddy (Claude Buddy) — first-party, deterministic generation from user ID, "bones vs soul" architecture, observes conversations.
Game / character-based visualizations
pablodelucca/pixel-agents — VS Code extension, agents as pixel-art office workers, reads JSONL transcripts directly (not hooks). Explicitly architected to be agent-agnostic and platform-agnostic — closest to your idea in spirit.
alvinunreal/openpets — desktop app, MCP-based integration (openpets_set_state, openpets_say), supports Claude Code, OpenCode, Cursor, Zed, Windsurf, etc. This is the most architecturally interesting one for your purposes — it's pull-based via MCP rather than push-based via hooks.
DaniloTrebjesanin/claude-pixel-quest — VS Code extension, agents as pixel characters mining/fishing/chopping.
Terminal / workspace orchestrators (state visible as a side effect)
These aren't primarily monitoring tools but expose state via UI:

manaflow-ai/cmux — native macOS terminal, OSC 9/99/777 + cmux notify CLI for hook integration, sidebar shows git branch, PR status, listening ports, latest notification.
craigsc/cmux — different project, "tmux for Claude Code," worktree wrapper.
kbwo/ccmanager — TUI, multi-agent (Claude/Gemini/Codex/Cursor/Copilot/Cline/OpenCode/Kimi).
smtg-ai/claude-squad — tmux + worktrees.
BloopAI/vibe-kanban — kanban, multi-agent, now community-maintained after Bloop shutdown.
Conductor, Crystal, Nimbalyst, Opcode, dmux, VibeTree, agentree, Code Conductor — variants on the kanban/worktree theme.
HUD / statusline-only
Claude HUD — pure statusline, native token data via Claude Code's statusline API + transcript parsing, no network/background process.
melodic-software/claude-code-observability — logs 14 hook events to JSONL, queries via subcommands.
What strikes me looking at this list
Three observations relevant to your generalized-API idea:

1. There are essentially three data sources in use, and almost everyone picks one:

Hooks (push, requires settings.json modification) — dominant approach
JSONL transcript files at ~/.claude/projects/... (pull, polling) — used by Pixel Agents, Claude HUD partially, the tamagotchi pets
Statusline command (pull, on-demand) — used by ccpet, Claude HUD, tamagotchi
OTel — used by Dynatrace, the "right" answer in some ways but rare
MCP — used by OpenPets; this is the outlier and most aligned with your "expose an API" framing
2. Pixel Agents and OpenPets are the two prior arts that already articulate your problem. Pixel Agents' README explicitly says "Agent-agnostic: Claude Code today, but built to support Codex, OpenCode, Gemini, Cursor, Copilot, and others through composable adapters." OpenPets goes further: it defines a small state vocabulary (thinking, working, editing, running, testing, waiting, success, error) and exposes it as MCP tools that any agent can call. That's pretty close to what you're describing, just packaged as a pet rather than a generic state bus.

3. The hook-collision problem you identified is real and not theoretical. Every dashboard tells you to install its own hooks into ~/.claude/settings.json. Running disler's observability + tamagotchi + Claude HUD + ccpet would mean four hook handlers fighting for the same PreToolUse slot. Some of them (claude-code-tamagotchi, openpets) explicitly call out hook installation as friction.

Want me to do the next pass — pull each one's repo and inventory their actual feature surface (what state they track, what events they emit, what their config/install model looks like)? That'd be the right input for designing the unified API. We could also look at whether something like an OTel collector pattern, a single hook router, or an MCP-based state server is the cleanest abstraction.

You said: Write a markdown doc capturing the findinfs so far and then go with pulling and inventory
Write a markdown doc capturing the findinfs so far and then go with pulling and inventory

May 10
Claude responded: I'll create the findings doc first, then start pulling repos for the deeper inventory.
I'll create the findings doc first, then start pulling repos for the deeper inventory.


Ran a command, created a file
Ran a command, created a file
Now let me start pulling repos. I'll batch this efficiently — clone the priority ones, then inventory each.


Ran 3 commands
Ran 3 commands
Let me start with OpenPets since it's the most architecturally interesting (MCP-based).


Ran 2 commands, viewed a file
Ran 2 commands, viewed a file
Excellent — there's a packages/agent-events package that suggests a normalized event abstraction. Let me dive in.


Viewed 2 files
Viewed 2 files
Smaller than expected — this package is just speech message validation. The state vocabulary lives elsewhere. Let me find it.


Ran a command, viewed a file
Ran a command, viewed a file
This is the canonical OpenPets state vocabulary. Let me check what happens on the Claude side — how do hooks map to these states?


Viewed 2 files
Viewed 2 files
This is very clean — the mapClaudeHookEvent and classifyToolReaction functions are essentially the canonical translation from Claude hook events to a normalized state vocabulary. Now let me peek at the MCP server.


Ran 2 commands
Ran 2 commands
Got it. OpenPets is fully done. Now Pixel Agents.


Ran 7 commands, viewed a file
Ran 7 commands, viewed a file
Pixel Agents has its own internal "agent event vocabulary" — let me note it. Now let me look at the server piece since the README mentioned "HookProvider" abstraction.


Ran a command, viewed a file
Ran a command, viewed a file
Big find — Pixel Agents has formalized exactly the abstraction Josh is describing. The AgentEvent union and HookProvider interface are essentially a generalized state model. Let me check the server entry.


Pixel-agents server pieces
Pixel-agents server pieces
This is gold. Pixel Agents already implements:

A normalized AgentEvent union
A HookProvider interface (with normalizeHookEvent)
A discovery file (~/.pixel-agents/server.json) with port + auth token
Multi-window reuse: second VS Code instance detects the running server and reuses it
Let me look at the Claude provider implementation, then move on to the others.


Ran 3 commands, viewed 2 files
Ran 3 commands, viewed 2 files
Now disler observability — the canonical reference everyone cites.


Ran 3 commands, viewed a file
Ran 3 commands, viewed a file
Disler's model is much simpler — HookEvent is just a passthrough container. The hook event type is a string field, not a normalized union. Worth noting. Let me peek at the hook scripts to confirm.


Ran 2 commands
Ran 2 commands
So disler's pattern: each hook installs a send_event.py that POSTs the raw payload + --event-type X flag to the Bun server. The server stores it as-is. Hook event types are stringly-typed. No normalization. This is the simplest possible approach.


Disler send_event hook
Disler send_event hook
Got the picture. Now claude-code-tamagotchi.


Ran 3 commands
Ran 3 commands
So tamagotchi reads the statusline JSON as its primary input — it's not hook-driven for the pet itself, it's statusline-driven. Pet state lives in a JSON file. The violation detection is a separate PreToolUse hook. Let me peek at that.


Ran 2 commands
Ran 2 commands
Now ccpet (simpler).


Ran 3 commands
Ran 3 commands
Got it — ccpet is purely statusline-driven, no hooks. State is a JSON file. Now let me look quickly at hoangsonww (the comprehensive Node monitor), agents-observe, and team-dashboard.


Ran 3 commands
Ran 3 commands
You said: Continue
Continue

May 10
Claude responded: Same pattern as disler — passthrough hook handler.

Ran 2 commands
Ran 2 commands
Same pattern as disler — passthrough hook handler. Now let me check the schema/state model in the server.


Ccam server lib + db
Ccam server lib + db
Excellent — ccam has a normalized SQL schema:

sessions with status active|completed|error|abandoned
agents with status working|waiting|completed|error and type=main|subagent
events (free-form)
This is itself a reasonable canonical state model. Let me peek at how hook events get translated into these state transitions.


Ran 4 commands
Ran 4 commands
Now agents-observe.


Ran 4 commands
Ran 4 commands
Same passthrough pattern. Hooks are stored raw as hookName + payload. Let me look at one more — claude-team-dashboard since it watches the .claude/teams/ directory rather than using hooks.


Ran 3 commands
Ran 3 commands
Good — claude-team-dashboard watches ~/.claude/teams/* filesystem (no hooks at all). It's a fundamentally different data source. This is similar to how Pixel Agents tails JSONL transcripts.

I have enough material now. Let me write the inventory doc.


Created a file, ran a command, read a file
Created a file, ran a command, read a file
Two docs ready:

01-findings.md — the broad inventory (categories, tools, observations from research).

02-detailed-inventory.md — the deep dive from cloning eight repos and reading the source. The key finds:

Pixel Agents already has the abstraction you're describing. server/src/provider.ts defines a normalized AgentEvent union and a HookProvider interface with normalizeHookEvent, installHooks, formatToolStatus, etc. Comments explicitly say "FileProvider and StreamProvider will be added alongside the first real second provider." They've designed for it but haven't packaged it standalone — it's locked inside the VS Code extension.
OpenPets has the right reaction vocabulary. Eleven values: idle | thinking | working | editing | running | testing | waiting | waving | success | error | celebrating. Plus a clean lease + token IPC model and speech validation that rejects URLs/paths/code/secrets.
ccam has the most complete relational schema. sessions (status: active|completed|error|abandoned), agents (type: main|subagent, status: working|waiting|completed|error, parent_agent_id), events. The hook→state transition logic includes nontrivial reactivation rules and a WAITING_INPUT_PATTERN regex for parsing Notification payloads.
disler and agents-observe are pure passthrough event logs — no normalization, no state machine. Just store-what-you-receive.
claude-team-dashboard uses zero hooks — pure filesystem watch on ~/.claude/teams/*. Different data source entirely.
Three coexisting state models in the wild: reaction-enum (OpenPets), event-log (disler/agents-observe), normalized relational (ccam/Pixel Agents). A generalized layer probably needs all three as views over the same store.
The hook-collision and statusline-collision problems are solvable with router patterns: a single registered hook + statusline that fan out to subscribed presenters. Nothing in the wild does this yet.
The doc ends with a suggested-next-steps list, including talking to Pablo De Lucca and Alvin Unreal — they're the two authors who already articulate the agent-agnostic problem. Want me to push further in any direction — sketch the protocol, look at how the OTel/MCP paths would fit in, or pull a few more repos (Claude HUD, melodic-software, the bash-driven hooks toolkits)?


02 detailed inventory
Document · MD 

01 findings
Document · MD 
You said: I’m curous if any of these distinguished between sessions and processes.
I’m curous if any of these distinguished between sessions and processes. Like a session can persist between across multiple processes, and isn’t “active” if no process is running it

May 10
Claude responded: Good question — let me check across the repos.
Good question — let me check across the repos. This is the kind of thing that's easy to conflate, and the distinction matters a lot for state correctness (is "session" your unit, or is "process running session" your unit?).


Ran 3 commands
Ran 3 commands
Good, ccam understands the distinction at the comment level: "SessionEnd is the definitive signal that the CLI process exited." But it doesn't track PIDs — it infers liveness from event timestamps. Let me check the others.


Ran 3 commands
Ran 3 commands
OpenPets only uses pid for the desktop-app discovery file (locking) and atomic file writes. No notion of "the agent's process is alive."


Pixel-agents pid
Pixel-agents pid
Same — Pixel Agents tracks pid only for its own server (multi-window discovery), not for Claude Code processes. Let me check how Pixel Agents actually decides "is this agent alive."


Ran 3 commands, viewed a file
Ran 3 commands, viewed a file
Pixel Agents uses vscode.Terminal as its proxy for liveness. When VS Code says the terminal closed, the agent is gone. That's a process proxy in disguise — it's actually correct for the case where you spawned the agent yourself, but it doesn't help with externally launched processes (which it tracks via isExternal: true).

Let me check if claude-team-dashboard handles this — it watches files, so it has a unique view.


Ran 2 commands
Ran 2 commands
Same pattern — claude-team-dashboard infers liveness from filesystem mtimes (change events) rather than tracking processes. Now tamagotchi and ccpet.


Tamagotchi + ccpet pid
Tamagotchi + ccpet pid
Tamagotchi uses pid for its own internal locking and worker tracking — never for tracking the Claude Code process. Confirmed.

Let me also peek at how OpenPets' lease system handles "agent went away" — that's structurally similar to your question.


Openpets lease lifecycle
Openpets lease lifecycle
Got it. Here's the answer:

Short answer: no — none of them model session vs. process as separate concepts. They all collapse the distinction in different ways.

Here's what each one actually does:

ccam is the closest to noticing. There's a comment in db.js:

"Legacy sessions (created before SessionEnd hook) will never receive a SessionEnd event, so they stay 'active' forever. Complete any active session whose last event is older than 1 hour — the CLI process is certainly gone by then."

And in hooks.js:

"SessionEnd is the definitive signal that the CLI process exited."

So ccam's model is: session.status = 'active' should mean "a process is currently running this session," but the only way it knows the process exited is through a SessionEnd hook, and if that hook never fires (crash, kill -9, pre-SessionEnd Claude Code), the session sits as active forever. Mitigation: a heuristic 1-hour stale sweep at startup. There's no PID column. There's no notion of "session exists, no process attached" — the system shoves every such session to completed after an hour.

Pixel Agents uses vscode.Terminal as a process proxy. When VS Code says the terminal closed, Pixel Agents removes the agent. This works for terminals it spawned, but for externally launched sessions (isExternal: true, e.g. an claude --resume from another VS Code window), there's no real lifecycle signal — the agent persists until something cleans it up. Sessions and processes are conflated through the terminal handle.

disler observability and agents-observe don't have any state model at all — they're event logs. They don't even ask "is this session active." A consumer reading the database has to make its own definition based on event timestamps.

OpenPets has the most interesting design here, although it's solving a different problem. Its lease + heartbeat system is exactly the right shape for "process is running":

lease.acquire when an MCP client (one Claude Code process) connects
lease.heartbeat every 5 seconds
lease expires if heartbeats stop
lease.release on clean shutdown
This is a per-process attachment to the pet. If the Claude process dies, the lease expires, and the pet falls back to default. OpenPets has the right primitive but applies it to pet ownership, not session liveness. It also doesn't have a session concept at all — only pet windows attached to leases.

ccpet, tamagotchi, claude-team-dashboard have no notion of process — they read state passively (statusline, files) and infer activity from staleness.

The fundamental problem none of them solve:

A Claude Code session is a ~/.claude/projects/<cwd>/<session-id>.jsonl file plus a session_id. That file persists indefinitely. You can claude --resume <session-id> and pick up where you left off. So the natural states are actually:

Session exists, process running (active)
Session exists, no process (resumable, paused)
Session exists, abandoned (resumable but probably not coming back)
Session ended (SessionEnd received — /exit clean shutdown)
The conflation happens because Claude Code's hook events don't signal "process exiting non-cleanly." There's SessionEnd for clean shutdowns, and silence for everything else. None of the tools attempt to detect process death directly — no one polls ~/.claude/projects/... for which JSONL files are currently being written to, no one watches for the process tree, no one models a heartbeat.

A proper session-vs-process model would need:

A session table keyed by session_id with last_event_at, lifetime status (live | paused | ended)
A process attachment table keyed by some process token (PID + start time, or a hook-provided run ID) with heartbeat timestamps
Sessions with at least one fresh attachment → live. Sessions with no fresh attachment but a recent JSONL file → paused. Sessions where SessionEnd was received → ended.
The closest thing in the wild to a "process is attached" primitive is OpenPets' lease, and the closest thing to "session has continuity" is ccam's session_id row. Nobody combines them.

This is a real gap and a strong argument for the generalized state layer. The hooks API gives you SessionStart / SessionEnd, but those are the agent's lifecycle, not the process's. A heartbeat — either via a long-running statusline process, an MCP keepalive, or a periodic Notification-style hook — would let you actually distinguish "session paused, process gone" from "session still chugging." That's information none of these tools currently surface.

You said: Capture those details onto the inventory
Capture those details onto the inventory

May 10
Claude responded: I'll add a section to the detailed inventory capturing the session-vs-process analysis.
I'll add a section to the detailed inventory capturing the session-vs-process analysis.


Edited a file, ran a command, read a file
Edited a file, ran a command, read a file
Added a new top-level section, "Cross-cutting analysis: session vs. process", slotted in just before the next-steps list. It captures:

The four natural lifecycle states (live, paused, abandoned, ended) and that none of the surveyed tools cleanly distinguish them
Per-tool breakdown of how each one collapses session and process — ccam's stale-sweep heuristic, Pixel Agents' vscode.Terminal proxy, OpenPets' lease-but-applied-to-pet-ownership, the event-log tools that punt entirely
Why the conflation happens — the hooks API only emits agent lifecycle events, not process death signals (crash, kill -9, force-close all silent)
A proposed Session + Attachment schema sketch that separates the two concerns
A revised next-steps list with the heartbeat layer added as item 3
The argument lands at the end: this is plumbing every consumer of the state layer reimplements badly, with each tool getting different edge cases wrong. Genuine shared substrate.


02 detailed inventory
Document · MD 
You said: One angle i’m thinking of is having an aggregsted state that reflects aggregation if current state, informed by the hooks, but also having a sweep that makes s…
One angle i’m thinking of is having an aggregsted state that reflects aggregation if current state, informed by the hooks, but also having a sweep that makes sure it’s accurate 
My thought is tools could query this to know what is running now without having to di its own event/hook listening and trying to keep record of the state

Adjacent to this wojld be an an hook that captures all the events that something can subscribe to get live notifications of, and be able to query historically

May 10
Claude responded: That's a well-formed split — separating "what is true right now" from "what happened over time" is the right architectural decomposition, and it matches what d…
That's a well-formed split — separating "what is true right now" from "what happened over time" is the right architectural decomposition, and it matches what database systems learned the hard way (the materialized view vs. the log).

Let me think through this with you a bit before capturing it, because there are a few design forks worth surfacing:

The two surfaces you're describing

State view — current truth. Read-side. "What sessions exist, what's each one doing right now?" Idempotent queries. Updated by hooks (push) and reconciled by sweep (pull). This is what 90% of consumers actually want — a pet, a HUD, a dashboard's "active sessions" list. They don't care about the 47 PreToolUse events that led to the current working state.
Event stream — append-only log. Subscribe for live, query historically. "Replay everything that happened in session X." This is what observability tools, debuggers, and analytics want.
The state view is derived from the event stream plus the sweep. That's important: it means the event stream is the source of truth, the state view is a projection, and the sweep is reconciliation when the projection gets out of sync.

The sweep is where this gets interesting

A few questions worth pinning down:

What does the sweep actually check? Some candidates, in order of cost:
Process check: walk /proc (Linux) or ps for claude processes — cheap, but may miss containerized agents
JSONL mtime: poll ~/.claude/projects/*/*.jsonl for files modified in the last N seconds — cheap and reliable for "is something writing to this session"
Heartbeat staleness: any session whose latest event is older than threshold X gets demoted from live to paused
Statusline tick: each statusline call is a deterministic per-process pulse that could double as a heartbeat
What's the sweep cadence? Too frequent and it eats CPU; too infrequent and the state view lags reality. JSONL mtime polling at 5–10s is probably the sweet spot.
Does the sweep correct, or just flag? If the sweep notices session X has had no events for 5 minutes but the JSONL is still being written, does it update the row, emit a synthetic event, or just expose a confidence field on the read API? I'd argue: the sweep emits synthetic reconciliation events into the same event log, and the state view is recomputed from those. That keeps the event log canonical.
The hook router question

You said "an hook that captures all the events that something can subscribe to" — there's an important design choice embedded in that: does the daemon install one hook line per event type, or one hook line per event that fans out? I'd vote for the latter:

jsonc
// ~/.claude/settings.json — one entry per hook event, all pointing at the daemon
{
  "hooks": {
    "PreToolUse":  [{ "command": "claude-state-bus emit PreToolUse" }],
    "PostToolUse": [{ "command": "claude-state-bus emit PostToolUse" }],
    // ...
  }
}
The claude-state-bus emit <type> shim:

reads stdin (the hook payload)
POSTs to the daemon
exits 0 in <5ms (same failsafe pattern as ccam, Pixel Agents, disler)
Subscribers register with the daemon, not with settings.json. They never touch ~/.claude/settings.json themselves. This is the key move — it converts the single-tenant hook slot into a multi-tenant pub/sub.

A subtle point about the API surface

The state-query API and the event-subscription API want to be the same shape:

GET  /sessions                              -> current state of all
GET  /sessions/:id                          -> one session
GET  /sessions/:id/events?since=<cursor>    -> historical replay
WS   /sessions/:id/events                   -> live tail (continuation of replay)
WS   /events                                -> all live events (firehose)
The events?since=<cursor> shape with WS continuation lets a consumer reconnect after a network blip without losing events — they just resume from the last cursor they saw. This is the same pattern as Kafka, Postgres logical replication, etc. It also means a "live notifications" subscriber and a "historical query" reader use the same endpoint with different parameters.

Let me capture all of this in the doc as a third file — a design sketch — rather than overloading the inventory.


Created a file, ran a command, read a file
Created a file, ran a command, read a file
New doc: 03-design-sketch.md. Captures your two surfaces (state view + event stream) with the sweep as the reconciliation glue, and pushes on a few design decisions worth surfacing early.

Key shape:

State view is a derived projection. Reads are idempotent and polling-friendly. The 11-value reaction enum is a further projection of agent state for the simplest consumers.
Event stream is the canonical source. Same cursor shape works for historical query (GET /events?since=) and live tail (WS) — disconnect/reconnect doesn't lose events.
Sweep emits events into the log, not direct state mutations. Means state is always derivable from the event log alone, including reconciliations. Makes "why is this session marked paused?" a grep instead of a debugging session.
Hook router: one shim per event type, daemon does fanout to subscribers. Subscribers never touch ~/.claude/settings.json.
Statusline tick is a free heartbeat: deterministic per-turn, includes session_id and tokens. Combined with hook events, you get sub-10s resolution on process death without polling.
Open questions section calls out the genuine forks: multi-host vs. local-only, retention, runtime/language choice, and the social problem of getting Pixel Agents and OpenPets authors to converge on a vocabulary.

Three docs now in outputs:

01-findings.md — broad inventory
02-detailed-inventory.md — per-tool deep dive plus session-vs-process gap
03-design-sketch.md — what the unified layer could look like
Worth pushing further on any of: a small protobuf/JSON schema for the event vocabulary, a concrete prototype of just the hook router (smallest viable piece), or a doc summarizing the "what to convince Pixel Agents and OpenPets authors to share" pitch?


03 design sketch
Document · MD 
You said: Let’s take a look at the design sketch and compare/contrast to the inventory to think about how well the design captures them
Let’s take a look at the design sketch and compare/contrast to the inventory to think about how well the design captures them

May 10
Claude responded: Good idea — this is the "does the abstraction actually fit the cases" check.
Good idea — this is the "does the abstraction actually fit the cases" check. Let me do it carefully, going tool by tool. The honest version of this exercise involves looking for places where the design is wrong or missing pieces, not just confirming the happy path.


Verify docs present
Verify docs present
Let me think this through before writing it up.

For each tool, two questions:

Could it be rebuilt as a thin presenter on top of the design?
What does it currently do that the design doesn't account for, or does worse?
Going tool by tool:

OpenPets — the design's reaction enum is OpenPets's reaction enum. So as a consumer that subscribes to per-session reactions, OpenPets becomes trivial: subscribe → render. But OpenPets has the lease mechanism for "which pet does this process own" — that's project/process binding for output (which window). The design doesn't model that. The design says "session X has reaction Y," and presenters decide what to do with it. OpenPets has a multiplexing concern (one app, multiple pet windows, each bound to one agent process) that's a presenter concern, not a state concern. So OK — but the design needs to explicitly leave that to presenters.

Also — OpenPets has speech/say which is agent intent, not state. Agent says "Done!" or "Let me check." That's not a state transition, it's a side-channel message. Does the event log carry these? Currently no — the design only captures state-relevant events. Either we add a message event kind, or we say "out of scope, presenters handle this through MCP directly." Probably the latter.

Pixel Agents — the design borrows heavily from Pixel Agents's AgentEvent shape. So most of it maps. But Pixel Agents has terminal-binding — it knows "this VS Code terminal is running this Claude Code session" and can dispose the agent when the terminal closes. The daemon doesn't know about VS Code terminals. Pixel Agents would still need its own VS Code terminal lifecycle handling. The daemon gives it the session→state mapping, but the terminal→session mapping is Pixel Agents's job.

Also Pixel Agents tracks per-tool detailed status ("Reading foo.ts", "Editing auth.ts") — that's formatToolStatus running on each toolStart. Where does that live in the design? The event stream carries tool_id, tool_name, input. Presenters call their own formatter. That's fine — but it means each presenter that wants pretty status strings reimplements formatToolStatus. Maybe the daemon should compute and cache a default formatted version? Or ship a shared formatter library? Worth noting.

Pixel Agents's subagentToolNames / permissionExemptTools — these are Claude-specific knowledge embedded in the provider. Where do they live in the unified design? They belong in the daemon's Claude adapter, but the daemon needs a way to expose "this tool spawned a subagent" semantically, not just by name. The subagentStart event covers it. But the exemption lists — those are presenter logic (Pixel Agents uses them to decide "don't start a permission-waiting timer for this tool because this tool is itself a subagent spawner"). The design's heuristic-killer is that hooks are authoritative; the timers exist because Pixel Agents also tails JSONL and uses heuristics in the absence of hooks. So in the unified design, those timers should mostly go away — replaced by the daemon's own state machine. Worth checking.

disler observability and agents-observe — both are pure event logs with no state model. The design covers them perfectly: they become consumers of GET /events?since= for backfill and WS /events for live. Their UIs are pure history-replay. But disler has the HITL extension — the dashboard sends approval prompts back to the agent through a custom endpoint. That's not in the design at all. The design is read-only from the consumer side. Adding "agent <- consumer" backflow is a different abstraction. Probably out of scope, but worth flagging.

Also disler attaches an AI summary to each event via a hook flag. Where does that go? Two options: it lives in the raw payload (hook decided to add it), or it's a presenter concern (analytics tool calls Claude on each event). Probably the former — anything the hook adds is just part of the payload.

ccam — has its own normalized state model (sessions, agents, events). It would replace its DB layer with the daemon. Its UI subscribes. But ccam has plugin marketplace integrations and a "Run Claude" feature where it spawns Claude Code itself. That's an agent control plane, not just observability. The design has nothing to say about that. ccam-as-consumer of the daemon would mean the daemon handles state and ccam handles spawning. That's a clean split but requires ccam to refactor.

ccam also runs an MCP server that exposes its own data — interesting because that means ccam already implements consumer-side query API. So if the daemon exposes a similar query surface, ccam's MCP server could trivially front the daemon.

ccpet and tamagotchi — both are statusline-driven pets. The design's "statusline tick is a free heartbeat" + "statusline shim composes segments from registered providers" handles them perfectly. But they both maintain pet-specific persistent state (pet name, energy, animal type, mood history, leaderboard tokens). That state is not in the daemon. It's the presenter's own state — which is fine, but worth being explicit about.

Tamagotchi has the violation detection layer — that's an LLM-as-classifier reading transcripts. Presenter-side concern. Fine.

claude-team-dashboard — watches ~/.claude/teams/ directly. The design's "daemon could watch teams dir and emit synthetic team events" is in the open-questions section. If we did that, the team dashboard becomes a consumer. If we don't, it stays parallel infrastructure. The Agent Teams data model is genuinely different — teams have messages between agents (inboxes), which aren't well-captured by the current event vocabulary. There's no agentMessage event. Worth adding.

cmux family — the design is tool-agnostic but most cmux work is terminal multiplexing and notification surfacing, which is orthogonal. cmux uses OSC escape sequences and cmux notify CLI calls. Those don't have a session_id. Could cmux benefit from the daemon? Maybe — it could subscribe to "any session entered waiting state" and surface a desktop notification automatically. But cmux's primary job is workspace organization, not state observability. Mostly orthogonal.

Vibe Kanban / Conductor / Crystal / Opcode — the orchestrators. They spawn worktrees, run commands, manage Git state. The daemon gives them session liveness, but most of their work is workflow management. They'd be light consumers — "is this session done?" "did it succeed or error?" "is it waiting for input?" — but the heavy lifting (worktree creation, branch management, PR creation) is theirs.

Claude HUD and statusline-only tools — statusline shim handles these natively.

Dynatrace / OTel — totally different lane. Claude Code emits OTel directly. The daemon could ingest OTel as a fifth source (alongside hooks/jsonl/mcp/statusline) but it's also fine to leave OTel as the enterprise path and the daemon as the local-developer path.

Anthropic's Managed Agents and /buddy — first-party, not really in scope. Buddy is in-process, daemon is out-of-process. They serve different audiences.

Now let me think about what the design is missing that came out of this:

Agent-to-agent messages (Agent Teams inboxes) — no event kind for this
Speech/say events (OpenPets, Pixel Agents speech bubbles) — agent intent messages, not state
HITL backflow (disler) — consumer → agent, the design is one-way
Default formatted status strings — every presenter reimplements formatToolStatus
Pet/presenter persistent state — explicitly out of scope, but worth saying so
Agent control plane (ccam Run Claude, orchestrators spawning) — out of scope
OTel as ingest source — viable fifth source but not currently called out
What the design does capture well:

The session vs. process problem (lease + heartbeat + sweep)
The hook collision problem (single router)
The statusline collision problem (segment composition)
The reaction enum projection for cheap consumers
The event log + state view duality
The same cursor shape for live and historical
OK — I think this is enough material for a real critique doc. Let me write it.


Created a file, ran a command, read a file
Created a file, ran a command, read a file
New doc: 04-design-vs-inventory.md. Walks each tool against the design and asks "could it be a thin presenter, and what's missing if not."

The honest summary:

The design fits better than I expected. OpenPets, ccpet, tamagotchi (pet half), Claude HUD, disler, agents-observe become straightforward consumers. Pixel Agents and ccam fit, with refactoring — and the design is actually a strict improvement in places (Pixel Agents loses multi-window cooperation logic, JSONL token polling, and permission/idle timers; ccam's session reactivation rules belong in the daemon rather than reimplemented per tool).

Two real missing pieces emerged:

agentMessage event kind for Agent Teams inbox messages. Without it, claude-team-dashboard can't be a clean consumer. Should add to the vocabulary.
An explicit position on the boundary. Three tools genuinely exceed observability scope: tamagotchi (blocks tool calls synchronously), disler (HITL backflow), ccam/orchestrators (spawn agents). The current design implicitly punts on all of them. Better to be explicit: the daemon is read-only observability, intervention/control are separate. The design should say so.
Smaller refinements:

formatToolStatus is reimplemented in 3+ places — the daemon should compute a default and let consumers override
OTel deserves recognition as a fifth ingest source
Presenter-side persistent state (pet name, leaderboard tokens) is explicitly the presenter's responsibility
Single-developer-machine deployment is the assumed scope; multi-host and team-shared servers are future
Tools that genuinely don't fit and shouldn't: cmux family (terminal organizer), /buddy (first-party, in-process), Managed Agents (hosted control plane), Vibe Kanban / Conductor / Crystal (orchestrators). That's fine — not every tool should be a presenter. The point is that the observability tools converge.

The conclusion lands at: the core claim — "80% of these tools could be thin presenters on a shared state + event bus" — holds up. The next concrete move would be a v2 of the design sketch incorporating these findings, then a minimum viable prototype (probably just the hook router as the smallest valuable piece).

Want me to do that v2 of the design sketch, or pivot to thinking about what a minimum viable prototype would look like?


04 design vs inventory
Document · MD 
You said: One angle i did want to consider was attaching a procdss/session back to the terminal is in.
One angle i did want to consider was attaching a procdss/session back to the terminal is in. Tmux has env variables, and one mentioned vscide. I want to make sure to capture this feature specically. I’m not sure how detailed we csn get, like which terminal process / multiplexer window/pane/ etc

May 10
Claude responded: Good instinct.
Good instinct. This is a real feature — "where am I looking at this session" is a surprisingly hard question that nobody currently solves cleanly, and it has implications for how a presenter decides which window to surface a notification in.

Let me dig into what's actually available, then capture it.


Brainstorm available signals
Brainstorm available signals
Let me check what Pixel Agents and cmux already do here, since they're most likely to have grappled with it.


Ran 2 commands
Ran 2 commands
Confirmed: nobody in this inventory captures terminal/multiplexer attribution. Pixel Agents has its closest analog (binding to vscode.Terminal), but that's because it is the VS Code extension — it has direct API access to the terminal handle. None of them try to figure out "this Claude session is running in iTerm session X, tmux pane %42, on this physical workstation."

Let me also peek at Claude Code's hook payloads to see what env actually reaches them.


Ran 2 commands
Ran 2 commands
Right — Claude Code hook payloads include session_id, transcript_path, cwd, tool_name, tool_input, hook_event_name. Hooks don't natively carry environment variables — but they inherit the process's environment, so the hook script sees $TMUX, $TERM_PROGRAM, etc. as its own env. That's important: the hook script can capture this info itself.

Now let me write this up properly as a new section in the design sketch and reference it from the critique doc.


Viewed 2 files
Viewed 2 files
Now I'll insert a new section "Where is this session running? — terminal & multiplexer attribution" between Heartbeat sources and Storage, and update the attachments schema to include the location fingerprint.


Insert terminal attribution section after heartbeat sources
Insert terminal attribution section after heartbeat sources
Now I need to remove the duplicate attachments table from the Storage section and replace with the location-augmented version, since I introduced it in the new section.


Viewed a file, edited a file
Viewed a file, edited a file
Now update the State view sample shape so the API surface reflects this too — that's where consumers actually see it.


Add location to attachments in the read API sample
Add location to attachments in the read API sample
Now let me also note this in the "what the design captures" section of the critique doc, since it's a feature the inventory revealed nobody handles.


Edited a file, ran a command, read a file
Edited a file, ran a command, read a file
Captured. New section in 03-design-sketch.md: "Where is this session running? — terminal & multiplexer attribution", sitting between Heartbeat sources and Storage. Also threaded into the schema (location JSON column on attachments), the read API sample, and the critique doc's "captures well" list.

The key points covered:

What signals exist — a table of env vars by category:

Terminal emulators (TERM_PROGRAM, TERM_SESSION_ID, KITTY_WINDOW_ID, WEZTERM_PANE, WT_SESSION, VSCODE_INJECTION, TERMINAL_EMULATOR for JetBrains)
Multiplexers (TMUX+TMUX_PANE, STY for screen, ZELLIJ/ZELLIJ_PANE_ID)
SSH (SSH_TTY, SSH_CONNECTION, SSH_CLIENT)
OS-level (ttyname, ppid chain via /proc or ps, hostname)
The fingerprint shape — an AttachmentLocation JSON struct with host, terminal{program,version,session_id}, multiplexer{kind,socket,session,window,pane}, ide{kind,pid,workspace}, ssh, and a ppid_chain. Best-effort, fields can be null.

Per-attachment, not per-session — because claude --resume from a different pane next week is a new attachment with a new location, same session_id. This is the right granularity.

Stability gotchas worth flagging:

TMUX_PANE is not stable across tmux kill-server — it's a snapshot
TERM_SESSION_ID (iTerm) is stable across detach/reattach — that's the handle for "open this iTerm tab"
VS Code terminals aren't externally addressable — you can know the session was started from VS Code, but can't programmatically focus that specific terminal
How presenters use it — desktop notification click-through ("Open in iTerm"), grouping for status displays ("1 vscode, 2 in tmux:dev"), orchestrator routing, future cross-machine views.

Implementation note — the shim already has access to the agent's full env. On first event for a new (session_id, process_token) it captures the fingerprint and emits attachmentOpen { location: {...} }. Subsequent events for the same attachment skip the capture and just send heartbeats. Cheap.

Edge cases — subagents share the parent's location, background agents share, headless/CI has mostly-null fingerprint, Docker reflects the container (accurate but worth surfacing the hostname), SSH-inside-tmux-inside-iTerm populates all three layers.

The critique doc now lists this under "captures well" with the explicit note that no surveyed tool currently does this — Pixel Agents has the closest only because it gets the VS Code Terminal API for free. So this isn't just a parity feature; it's a strict new capability the unified layer enables.

Want to keep going? Some natural next moves: a v2 design sketch incorporating all the critique findings into one clean doc, or sketching the smallest viable prototype (probably just the hook router + event log + the terminal attribution capture, since those are the most novel pieces).


03 design sketch
Document · MD 

04 design vs inventory
Document · MD 
You said: Make sure we captured details about worktrees across these tools and how they are tracked, made visible and tracked.
Make sure we captured details about worktrees across these tools and how they are tracked, made  visible and tracked.

May 10
Claude responded: Good catch — worktrees came up repeatedly in the inventory but I didn't pull on the thread.
Good catch — worktrees came up repeatedly in the inventory but I didn't pull on the thread. Let me audit what each tool actually does with them, then capture it properly.


Find worktree mentions across all repos
Find worktree mentions across all repos
I have new repos here that I didn't clone — dmux and tmux-agent-status. Those weren't in my original priority list but came in earlier. Let me check what's actually here and what I'm missing.


Check what repos exist
Check what repos exist
Interesting — there are repos here I don't remember cloning (dmux, opensessions, tmux-agent-sidebar, tmux-agent-status). The filesystem must have persisted from a previous session, or these came in as part of the earlier batch and I missed them. Let me verify and learn what they are.


Identify the unfamiliar repos
Identify the unfamiliar repos
These are clearly repos I cloned in an earlier session that persisted on disk — they're directly relevant to your question. Let me start with dmux and the worktree-focused tools, then go back through my known set.


Ran 3 commands
Ran 3 commands
Excellent — dmux is the worktree-native tool. Worktree path is a first-class field, and they actually have logic for nested worktrees (worktrees within worktrees, created by hooks). Let me check tmux-agent-sidebar and tmux-agent-status.


Ran 2 commands
Ran 2 commands
tmux-agent-sidebar uses tmux pane-options (PANE_WORKTREE_NAME, PANE_WORKTREE_BRANCH, PANE_CWD) as its state store — that's a really clever approach. State lives in tmux itself, attached to the pane. Now tmux-agent-status:


Tmux-agent-status worktree
Tmux-agent-status worktree
tmux-agent-status mentions worktrees but doesn't track them as a primary concept — they're just one of several deployment patterns. Now let me check opensessions and recheck my known repos.


Ran 2 commands
Ran 2 commands
Good — opensessions has isWorktree: boolean and branch per session, derived from git rev-parse --git-dir. Lighter than dmux but does track it. Now let me sweep through the original priority repos:


Worktree mentions in priority repos
Worktree mentions in priority repos
Striking finding. Of the seven primary observability/state tools:

Pixel Agents: zero matches
ccam: zero matches
OpenPets: only in release docs
disler observability: only in skills (helper skills for worktree management — not in the dashboard's data model)
tamagotchi, ccpet, team-dashboard: zero matches
None of the observability/state tools track worktrees. They're invisible to all of them. The session_id is a UUID, the cwd is a path — and if cwd happens to be ~/proj/.worktrees/feature-foo, the dashboard just shows the path string. There's no understanding that "this session and that session are working on the same repo, just different branches."

Meanwhile every orchestrator (dmux, vibe-kanban, conductor, ccmanager, claude-squad — per the broad inventory) treats worktree as a first-class concept because they create them.

Let me verify cwd is the only thing in the disler/ccam state model:


Cwd/repo/branch tracking in ccam and disler
Cwd/repo/branch tracking in ccam and disler
Confirmed. ccam stores cwd only — no repo, no branch, no worktree-vs-main distinction. disler doesn't even have it as a typed field. Now I have enough material. Let me write this up properly.


Ran 2 commands
Ran 2 commands
The existing inventory already includes tmux-agent-sidebar, opensessions, and tmux-agent-status as numbered tools — Josh and I covered them in earlier sessions that I just didn't have in my immediate context. Good. Let me see what was already captured about worktrees to avoid duplicating, then add a focused cross-cutting section.


Existing worktree mentions in inventory
Existing worktree mentions in inventory
Already good — tmux-agent-sidebar's hook envelope includes a worktree field on most events and has dedicated WorktreeCreate / WorktreeRemove events. That's the design pattern. Let me look at the full envelope shape to confirm:


Tmux-agent-sidebar event vocabulary section
Tmux-agent-sidebar event vocabulary section
Now let me also check what's already captured about dmux:


Dmux mentions
Dmux mentions
Not in the inventory yet. Let me add a worktree cross-cutting analysis section, touch the design sketch to add worktree as a first-class field, and add dmux as a reference orchestrator (since the inventory's primary focus is observability, dmux can be a brief addition).


Find the cross-cutting analysis section start
Find the cross-cutting analysis section start
Good, there's already a cross-cutting section. I'll add a new cross-cutting section right after the session-vs-process one for worktrees.


Ran a command, viewed a file, edited a file
Ran a command, viewed a file, edited a file
Now update the design sketch to incorporate the worktree fields and events.


Ran a command, viewed a file, edited a file
Ran a command, viewed a file, edited a file
Now add worktree fields to the read API sample and add the worktree events to the event vocabulary.


Viewed 2 files, edited 2 files
Viewed 2 files, edited 2 files
Now add the worktree events to the event vocabulary.


Viewed a file, edited a file
Viewed a file, edited a file
Now add a worktree section after the terminal attribution one in the design sketch.


Ran a command, viewed a file, edited a file
Ran a command, viewed a file, edited a file
Now update the design-vs-inventory critique to acknowledge worktrees as a captured feature.


Edited 3 files, ran 2 commands, viewed 2 files
Edited 3 files, ran 2 commands, viewed 2 files
Now copy to outputs and present.


Ran a command, read a file
Ran a command, read a file
Captured across all three docs. The most striking finding from the audit:

None of the observability/state tools track worktrees. Pixel Agents, ccam, OpenPets, disler, claude-code-tamagotchi, ccpet, claude-team-dashboard — zero. Only the tmux sidebar tools (especially tmux-agent-sidebar, which has dedicated WorktreeCreate/WorktreeRemove events and WorktreeInfo metadata on most events) and the orchestrators (dmux being the most sophisticated, with nested-worktree discovery and depth-ordered merging) treat worktrees as first-class.

Everyone else stops at cwd. So a developer running 5 parallel sessions in 5 worktrees on the same repo gets 5 unrelated rows in their dashboard, with no way to see "3 sessions on proj, across 3 branches."

Three things added:

02-detailed-inventory.md — new "Cross-cutting analysis: worktrees" section with a per-tool matrix of awareness/grouping/lifecycle, an explanation of why the observability tools miss it (hook payloads don't say "worktree"; data models are cwd-keyed), the proposed schema additions, and the subagent-aware pending-removal pattern from tmux-agent-sidebar.
03-design-sketch.md — three concrete additions:
repo_root, worktree, branch columns on the sessions table, plus indexes
cwdChanged, worktreeCreate, worktreeRemove events in the vocabulary
New "Worktrees, repos, and branches" section explaining derivation (one combined git rev-parse call per attachment, cached by cwd), orchestrator-emitted lifecycle events, the subagent-aware pending-removal handling, and useful edge cases (submodules, bare repos, detached HEAD)
?repo_root= and ?branch= filters on the read API
04-design-vs-inventory.md — added worktree handling to the "captures well" list, with the explicit observation that this is a strict new capability for observability tools (none of them have it today, only the tmux sidebars and orchestrators do).
The cost is one git invocation per session (combined --git-common-dir --show-toplevel --abbrev-ref HEAD), cached. The benefit is that pets, HUDs, dashboards all gain "3 worktrees on this repo" grouping for free, and orchestrators that already track their own worktree state can stop maintaining a parallel registry.

Want to keep going? Natural next moves: a concrete event vocabulary spec (could be JSON Schema or TypeScript types extracted to a shared package), a sketch of the v2 design doc that incorporates all the critique findings into one clean canonical version, or pivoting to "what's the minimum viable prototype" — probably the hook router + sqlite event log + the worktree/terminal derivation, since those are the most novel pieces.


02 detailed inventory
Document · MD 

03 design sketch
Document · MD 

04 design vs inventory
Document · MD 
You said: I want to spend some time doing another survey of tools in this space, now that we have a better context.
I want to spend some time doing another survey of tools in this space, now that we have a better context. Focus on things like game-inspited displays and desktop integration /visibility. We can put it in a new file for ease if editing

May 10
Claude responded: Two quick clarifications to make this survey land where you want it:
Two quick clarifications to make this survey land where you want it:

Scope boundary — narrow to game-inspired/desktop-visibility tools only (pets, sprite-based, ambient widgets, system tray, notifications), or also include adjacent novelty categories like AR/voice/audio cues and unusual physical-world displays (e-ink, smart bulbs, etc.)?
Agent ecosystem — Claude Code only, or also surface comparable tools for Codex/OpenCode/Cursor CLI/Aider since the patterns may transfer?
You said: Novelty adjacent, and clauxe primary but look for things eith multi Gent support
Novelty adjacent, and clauxe primary but look for things eith multi Gent support

May 10
Claude responded: Got it.
Got it. Let me launch into research — Claude Code primary but flagging multi-agent support, with the novelty/desktop-visibility lens kept wide enough to catch the unusual stuff.


Novelty Claude Code UX tools



Research complete
•
424 sources
•
17m 55s
Your survey of game-inspired and novelty desktop-visibility tools for Claude Code agents is ready. It catalogs 30+ projects across sprite/RPG visualizations, Tamagotchi-style pets, menu-bar apps, ambient hardware (smart bulbs, e-ink, thermal printers, Pixoo matrices), wearables (Apple Watch, AR glasses), voice/TTS systems, Discord Rich Presence, and multi-agent office metaphors. The report highlights standout newcomers (Outworked, Claude Quest, AgentDeck, claude-lamp, shobhit99/claude-watch), identifies architectural patterns around hooks and JSONL transcripts, flags ecosystem gaps (Windows tray apps, LIFX/WLED/Nanoleaf integrations, Garmin/Pebble wearables, Pokémon-party multi-agent visualizers), and provides prioritized recommendations for inventory additions plus caveats on README claims, beta-OS dependencies, and the /buddy leak.


Game-Inspired and Novelty Desktop Visibility Tools for Claude Code Agents
Document 
You said: With that research, are there any concepts we are missing as a working base for these other tiols?
With that research, are there any concepts we are missing as a working base for these other tiols?

May 10
Claude responded: Good question.
Good question. Let me work through the new survey systematically against the design and find what's missing.


Current design sketch sections
Current design sketch sections
Let me think through each tool category from the new research and ask: what data does it consume that the daemon doesn't currently expose?

Going through the survey carefully:

1. Outworked / claude-office / pixel-agents — agents-as-employees with names, roles, sprites. They want agent identity / persona (display name, role like "researcher" / "engineer" / "architect", maybe an avatar key). PAI's per-agent voice mapping (Bella for engineer, Domi for researcher) is the same need. Currently the daemon has agent_id and type (main | subagent | teammate) but no human-meaningful name or role. Gap: agent persona metadata.

2. claude-receipts (thermal printer at SessionEnd) — needs final cost/token breakdown per model. Daemon has tokens and cost events but doesn't aggregate per-model. Gap: per-model cost rollup.

3. ccpet / tamagotchi / Anthropic /buddy — pets need cumulative stats: total tokens this session, lifetime tokens, lifetime sessions, time-since-last-fed. Daemon has events but no rollup view. Gap: aggregated per-session metrics.

4. Claude Quest / claude-pixel-quest — character reacts to specific tools (Edit → casts spell, Bash → swings sword). Already covered: tool_name is in events. But — they also use a context window % drained as HP bar. Statusline carries contextPercentageUsable. Currently the daemon doesn't expose context window utilization. Gap: context window state.

5. AgentDeck (13 surfaces) — needs to support N concurrent presenters subscribing to the same data. Already covered.

6. claude-lamp (Moonside BLE) — color per state. Already covered (current_state reaction enum).

7. m5-paper-buddy / claude-buddy-pico — physical hardware approval of permission prompts. Buttons on the device approve/deny AskUserQuestion and tool calls. This is HITL backflow — already flagged as a tension in 04-design-vs-inventory.md. The new research strengthens the case: AgentDeck, m5-paper-buddy, Redlight Greenlight for Claude Code, and claude-watch all want approval-from-elsewhere. Gap: HITL backflow surface — should be revisited.

8. Redlight Greenlight / claude-watch / Handwave — also approval-from-elsewhere. Same pattern.

9. claude-watch (Apple Watch) — also wants AskUserQuestion option-list rendering. Pixel Agents had this too. The daemon's permissionRequest event currently just signals waiting — not the choices. Gap: choice/options data on permission events.

10. Outworked / Pixel Agents / Claude Quest — render agent activity descriptors (not just tool name). formatToolStatus was already flagged. Confirmed gap.

11. AgentDeck — uses mDNS auto-discovery to find the bridge daemon. The design currently has a fixed discovery file. mDNS would let LAN devices (a phone, a Pi, an ESP32) find the daemon without configuration. Gap: LAN discovery, not just localhost.

12. Stream Deck integrations / TerminalDeck — bidirectional: device buttons fire actions toward Claude Code. Same HITL family.

13. claude-watch / Happy Coder / m5-paper-buddy — all run across LAN (phone talking to Mac, watch talking to phone talking to Mac, ESP32 talking to Mac). Gap: the daemon's API needs to be LAN-reachable, not just localhost. Currently the design assumes localhost-only. With these tools that's a hard constraint.

14. claude-music — /play, /vibe (DJ reads session and picks genre). This needs session content awareness, not just lifecycle. Today it'd tail JSONL itself. The daemon could expose recent assistant text via events for content-aware presenters. Slight gap, marginal value.

15. AR glasses / Apple Watch / iOS Happy Coder — present-mode operation: user is not at the laptop. Tools want to deliver notifications to the right surface. Today this is per-tool config (each tool decides where it lives). The daemon could expose user-presence signals — "the user has been at the laptop for 5 mins" vs "no input for 30 mins" — to help presenters decide whether to escalate. Possible gap but probably presenter-side.

16. PAI per-agent voice (Bella/Domi/Antoni) — assigns a voice to a role, not an agent_id (since agent_ids are ephemeral). They map by subagent type (researcher, engineer, architect). Currently the daemon's agents table has metadata (free-form). The Pixel Agents subagent_type field captured this. Gap: subagent_type as a first-class column.

17. Bobby-Gray/claude-dnd-skill cinematic display — streams typewriter narration to a TV. Different deployment topology — the display is on a Chromecast/tablet, the brain is on the laptop. Same LAN-reachability story.

18. agent-flow (cross-runtime) — visualizes Claude Code and Codex simultaneously. The daemon's design says "agent-agnostic" but only details Claude Code adapters. Codex tails ~/.codex/sessions/**/rollout-*.jsonl — different format, different events. Gap: explicit Codex / OpenCode / Cursor adapter sketch to validate the abstraction.

19. Outworked has a soundtrack and per-agent furniture — purely presenter state. Out of scope. ✓

20. Multiple per-process attachment scenarios I hadn't thought about: claude-watch has the Mac running claude and a Node bridge; Happy Coder replaces claude with happy to encrypt sessions; Cardputer pages have BLE peers. The "process attached to a session" concept may need to also accommodate proxies (a wrapping process that itself wraps claude). The process_token is currently a single value — what if there's a chain? Probably fine for v1; flag as a future consideration.

21. claude-status's "click to focus the right session" — works because it knows the terminal+multiplexer+IDE attribution. Already captured by the terminal attribution section. ✓

22. ZeldOcarina, clarvis, etr/bells-and-whistles — voice notifications. They want summarized last-message text to speak. clarvis does the summarization itself (LLM call). Daemon could optionally expose the last assistant message; today they'd tail JSONL. Marginal.

23. Discord Rich Presence (cc-discord-presence, claude-rpc) — needs project name, current activity, elapsed time. All covered, plus they want per-model pricing display (computed cost). Cost rollup gap (#3) covers this.

Summary of real gaps surfaced by the new survey:

Agent persona / display metadata — name, subagent_type as a first-class field, optional avatar/role
Per-model cost and token rollups — for receipts, dashboards, Discord
Aggregated per-session metrics — for pet stat decay, lifetime counters
Context window utilization — for HP bars, countdown displays
Permission request payload — choices, question text, harm potential (so watches/lamps/decks can render and approve)
HITL backflow — was flagged as out-of-scope, but the volume of evidence (AgentDeck, m5-paper-buddy, claude-watch, Redlight Greenlight, terminaldeck) suggests this should be a deliberate extension surface, not just "out of scope"
LAN-reachability + mDNS discovery — phone/watch/ESP32 needs to find the daemon
Codex / OpenCode adapter sketch — to validate the abstraction is genuinely agent-agnostic
Subagent_type as a first-class column — for per-role voice/avatar mapping
Plus a smaller observation: AskUserQuestion choices and harm-potential field need to ride on permission events for hardware-approval surfaces to render anything meaningful.

Let me capture these in a new doc.


Created a file, ran a command, read a file
Created a file, ran a command, read a file
New doc: 06-missing-concepts.md. The survey surfaces nine concrete gaps. Briefly, in priority order:

Strong gaps (the design serves these badly today):

Agent persona metadata — subagent_type, display_name, description columns. Outworked maps sprites to roles, PAI maps voices to roles (Bella/Domi/Antoni for engineer/researcher/architect), AgentDeck maps creatures to agents. They all need a stable key per role; agent_id rotates per run.
Per-model cost/token rollups — claude-receipts (thermal printer, per-model line items), Discord Rich Presence, every menu-bar usage tracker, Apple Watch usage app. All re-derive this from JSONL or ccusage today. Should be a derived projection.
Aggregated per-session metrics — pets shouldn't run aggregations across the event firehose every statusline tick. Roll counters into the session row, expose GET /stats/today and /stats/lifetime.
Context window utilization — Claude Quest's HP bar, claude-status's gauge, claude-pulse's burn-rate alerts. Statusline carries contextPercentage; daemon should expose it.
Permission request payload — m5-paper-buddy renders option cards on e-ink, claude-watch shows them on the wrist, Redlight Greenlight floats them, AgentDeck D200H displays on the LCD. The current permissionRequest event is empty. Needs question text, options, tool name + input, harm-potential flag.
The big architectural one:

HITL backflow — promote from "out of scope" to extension surface. The original critique flagged this for disler alone. The survey shows at least seven independently developed tools want approval-from-elsewhere (m5-paper-buddy, AgentDeck, claude-watch, Redlight Greenlight, terminaldeck, Bobby-Gray cinematic, Cardputer pager). It's a category, not a feature. If the daemon doesn't do this, every HITL tool installs its own blocking PreToolUse hook — collision returns. Proposed: a "permission decision" channel where exactly one subscriber registers as authoritative answerer per session; others can display informationally.
Topology gaps:

LAN reachability + mDNS discovery — every wearable/hardware tool runs across LAN, not localhost. AgentDeck explicitly uses mDNS + QR pairing. Daemon should bind two listeners (localhost always-on, LAN opt-in) with _claude-state-bus._tcp.local advertisement and per-device tokens.
Codex / OpenCode adapter sketch — the design claims agent-agnostic but only details Claude Code. agent-flow, AgentDeck, claude-rpc, ccmanager, opensessions all visualize Codex too. Codex has no hooks at all — pure JSONL tail under ~/.codex/sessions/. If the abstraction holds for a hook-less provider, it's real; if it doesn't, it's "Claude Code with extra steps." Should ship a Codex provider in v1 to validate.
Marginal:

Last-assistant-message event — clarvis, claude-music, Handwave, Bobby-Gray cinematic all tail JSONL for assistant text. Optional assistantMessage event would help, but JSONL tailing isn't actually broken.
The doc closes with the three design decisions worth getting explicit Y/N on:

HITL as extension surface (not just out-of-scope)?
Codex adapter in v1 to validate the abstraction?
LAN-reachability default-off but mDNS-discoverable?
Going from current design to current-design-plus-six-gaps is the difference between "covers most observability tools" and "covers most observability and novelty tools." HITL alone changes what the daemon fundamentally is.

Want me to fold these into a v2 design sketch, or keep this doc as the diff and let the design sketch stay clean?


06 missing concepts
Document · MD 
You said: Okay, so i don’t think i was specific enough.
Okay, so i don’t think i was specific enough. The idea is to be a good baseline that others can build on easily.  “Gaps” are intentional if they aren’t foundational enoigh of a concept. Persona/role, for example, should be buildable with it. This is one that might be close enough to a common thread. Subagent type seems adjacent to this.

The hitl is interesting, but i don’t know if it is the use case i was most interested in, and if there are enough examples. Would make this a later concept it we find more examples.

Lan reachable seems like another stretch. Potentially interesting, but narrower scope.

For persona/role/subagent, how are they being detected and/or detected? Is the tool ochesttating so it can tell the process the thing to assume?

May 10
Claude responded: Good corrective — you're right, I was bundling "the daemon should support this" with "the daemon should be this.
Good corrective — you're right, I was bundling "the daemon should support this" with "the daemon should be this." Let me re-examine with the right lens: what's foundational vs. what should be buildable on top.

And the persona question is sharp — let me actually answer it from the evidence rather than guessing.


How pixel-agents handles subagent type
How pixel-agents handles subagent type
So Claude Code's hook payload carries agent_type directly — "web-researcher", "general-purpose", etc. — and Pixel Agents just reads it. Let me check tmux-agent-sidebar and Outworked too.


Tmux-agent-sidebar agent_type usage
Tmux-agent-sidebar agent_type usage
Confirmed — agent_type is a Claude Code-native hook payload field. Same data, second consumer. Now Outworked:


Ran 3 commands
Ran 3 commands
Outworked invents the persona itself — name, role, personality are user-defined and passed into the prompt. It's not reading anything from Claude Code; it's being the orchestrator that assigns roles. The persona is an Outworked concept, not a Claude Code concept.

Now PAI:


Searched the web
Searched the web
Now I have the full picture. Let me check AgentVibes briefly since it's the cleanest example of "voice keyed to subagent":


Searched the web
Searched the web
Now I have the complete answer. Let me think through this carefully before writing it up.

The actual mechanism, end to end:

The user defines named subagents as Markdown files with YAML frontmatter at .claude/agents/<name>.md (project) or ~/.claude/agents/<name>.md (user). The frontmatter has name, description, tools, model. The body is the system prompt.

When the main Claude session decides to delegate work, it calls the Task tool (now also called Agent) with subagent_type: "<name>". Claude Code spawns a subagent process with that file's system prompt and an isolated 200k-token context.

Both SubagentStart and PreToolUse/PostToolUse hooks fire with agent_id and agent_type populated in the payload. This is first-class native data — straight from Anthropic's own SDK docs ("agent_id and agent_type are populated when the hook fires inside a subagent"). So agent_type is the canonical key. Not a daemon invention.

How tools use it:

PAI / AgentVibes / Benny Cheung's voice system: read subagent_type from hook payload, look it up in voices.json, route TTS to that voice. Pure presenter side; no orchestration.
Pixel Agents: reads agent_type to route to the right teammate seat.
tmux-agent-sidebar: stores agent_type:agent_id in pane options.
Outworked: invents its own persona system because it is the orchestrator — it generates its own agents on the fly with custom system prompts. Outworked's name/role/personality is not Claude Code's subagent_type. It's a layer above.
So the answer to your question:

There are two distinct things that the survey conflated:

agent_type / subagent_type — the native Claude Code concept. Defined declaratively in .claude/agents/*.md files. Naturally orchestrated by Claude itself when it decides to delegate. Comes through hooks. This is data the daemon should just pass through — it's already on the payload, costs nothing.
Persona/role/voice/sprite mapping — the presenter's concept. PAI maps researcher → Domi. Outworked maps its made-up agent to a sprite. These are presenter-side configuration files. The daemon shouldn't define what a "persona" is.
The foundational primitive is agent_type — and it's already in the hook payload. What the daemon needs to do is preserve and surface it. That's a one-field schema addition. The personas, voices, sprites, names — all of that is built on top using configuration files the presenters own.

Let me write this up properly, and also rework the "missing concepts" doc to use the foundational-vs-buildable lens you articulated.


Created a file, ran a command, read a file
Created a file, ran a command, read a file
Direct answers to your questions:

How are personas detected/defined? Is the tool orchestrating?

Three distinct patterns in the wild, and the survey conflated them:

Native subagent_type — the user declares subagents as Markdown files at .claude/agents/<name>.md with YAML frontmatter (name, description, tools, model) and a system-prompt body. When the main agent calls the Task tool with subagent_type: "code-reviewer", Claude Code spawns that subagent. Hooks fire inside the subagent with agent_type and agent_id populated in the payload — straight from Anthropic's SDK docs ("agent_id and agent_type are populated when the hook fires inside a subagent"). No orchestration on the tool side. Just read the field.
Higher-layer orchestration (BMAD via PAI's BmadBridge) — a skill detects "the user wants Mary the Business Analyst," wraps Claude with the right system prompt, and the hook does pattern-matching on output to recover the persona. This is still consuming hook data, but the persona lives in a layer above Claude Code's native subagents.
Pure invention (Outworked) — Outworked is the orchestrator. It generates its own employee personas with name/role/personality, builds the system prompt itself, and launches Claude. Outworked's "agents" are not Claude Code's subagent_type; they're Outworked-defined personas applied to whole sessions.
So subagent_type and persona aren't the same thing. subagent_type is native Claude Code data — declarative, in hook payloads, already there. Persona is a presenter-side concept that varies per tool.

What's foundational, then?

Applying your lens: foundational means "the daemon doesn't expose enough data to build this on top." Buildable means "the daemon exposes enough; the rest is presenter-side."

The corrected list of what's actually foundational comes down to one principle: faithfully preserve native hook/statusline/JSONL payload data on events. The original design quietly dropped some of it (agent_type, full permission question/options, full statusline payload). That's the gap.

Concretely:

agent_type and agent_id on every event where the hook payload carries them. Schema/passthrough only. Without it, every voice/sprite/teammate tool reimplements hook ingestion.
Full permission request payload (question text, options, tool input, harm-potential when available). Hardware approval surfaces literally can't render anything without this. Pure preservation of native data.
Statusline payload preserved on heartbeat events (tokens, context %, model). The only source for some of these.
Everything else from the missing-concepts doc — persona names, display names, voice maps, sprite maps, lifetime cost rollups, HP bars, LAN reachability — is buildable on top once those three are in place. PAI's voices.json keyed on agent_type is the existence proof: the daemon doesn't need a persona concept; it needs to preserve agent_type so PAI's config file works.

HITL backflow stays out of scope (different abstraction, evidence is real but modest). LAN reachability is presenter-side (AgentDeck's bridge is the LAN listener; it can subscribe to a localhost daemon). Codex adapter is worth documenting for abstraction validation but not shipping in v1.

The general principle that emerged, and that the doc closes on:

The daemon's job is to preserve and expose the underlying data, not to define application-level concepts on top of it.

The test for "should this be in the daemon":

Native data that presenters need but can't access without reimplementing ingestion → daemon preserves it
Derived aggregation many presenters compute redundantly → optional convenience query
Application-level concept layered on agent state → presenter responsibility
By that test, the original design dropped (1) in a few places. Most of the rest of the survey's "gaps" are actually (3) or (2). The right move is a tighter design that resists normalizing away native fields, not a broader one that adds persona/voice/persona-display concepts.

Want me to do a clean v2 of the design sketch that applies this principle systematically — basically rewriting the event shape to be "passthrough by default, normalize only where multiple sources need merging" — or keep this as a guiding-principles doc that informs future edits?


07 agent type and foundations
Document · MD 
You said: Yes, write as a new document
Yes, write as a new document

May 10
Claude responded: Let me write the v2 design — a clean rewrite applying the preserve-native-data principle systematically.

List current docs
List current docs
Let me write the v2 design — a clean rewrite applying the preserve-native-data principle systematically.


Created a file, ran a command, read a file
Created a file, ran a command, read a file
08-design-sketch-v2.md is the clean rewrite. The throughline: v2 commits harder to being a substrate.

The principle the doc opens on:

The daemon's job is to preserve and expose underlying data, not to define application-level concepts on top of it.

Three-rung test for "does this belong in the daemon":

Native data presenters can't access without reimplementing ingestion → daemon preserves
Derived aggregation many presenters compute redundantly → optional convenience
Application-level concept → presenter's job
What v1 kept: three data sources fanning in, single hook router, single statusline composer, event log + state view duality, session-vs-process via attachments + heartbeats, sweep-emits-events-not-mutations, worktree derivation, terminal attribution.

The six concrete changes:

Events are passthrough by default. payload field carries the full native hook/JSONL/statusline body verbatim. No tidy per-kind union that strips fields presenters might want. The cost: two consumers may handle the same logical event slightly differently. The benefit: nobody re-tails JSONL to recover a field the daemon "didn't think was important."
agent_type and agent_id as first-class indexed event columns. Pulled out of payload, exposed in filters. The single most important change — it's what lets PAI, AgentVibes, Pixel Agents, and tmux-agent-sidebar stop installing their own hooks.
Event kinds shrink from 18 to ~17 with clearer meanings. tokens, cost, agentMessage, assistantMessage are gone — that data lives in payloads of other events. Presenters that want it read it from payload.
permissionRequest carries the full native payload. Question text, options, tool input, harm signals. Hardware approval surfaces can render meaningful prompts.
Explicit out-of-scope statements as a section, not footnotes. Read-only observability. No tool blocking, no HITL backflow, no spawning, no personas, no LAN reachability, no multi-host, no presenter-side state.
The current_state reaction enum is the only non-passthrough normalization the daemon performs, justified because every cheap presenter (statusline, pet, lamp, LED matrix) reimplements the same five rules otherwise. Eleven values straight from OpenPets.
Tier-2 convenience queries stay (/sessions/:id/stats, /sessions/:id/usage, /stats/today, /stats/lifetime) but explicitly marked as folds the daemon does for convenience, not as primary architecture.

The doc closes with two useful pieces:

A v1→v2 diff table covering 10 design decisions.

A "what this enables" tour walking every tool in the inventory through v2 — what subscribes to what, what each presenter looks like in lines of code. The shape is consistent: register as a subscriber, react to events or query state, never touch ~/.claude/settings.json, never reimplement hook ingestion, never re-tail JSONL for fields the daemon already has.

The four open questions explicitly called out: current_state storage strategy, auth model (probably per-daemon-run token, Pixel Agents pattern), event retention, statusline composition order, language/runtime choice (Rust/Go for the shim specifically), and whether to expose payload filtering.

Want to push further? Three natural next moves: (a) sketch the minimum viable prototype — probably the hook router + sqlite event log + the reaction enum projection, ~500 lines; (b) write the protocol spec as JSON Schema or TypeScript types for a @claude-state-bus/protocol package; or (c) put together the "pitch to Pablo De Lucca and Alvin Unreal" memo — they're the two who already articulate the substrate problem.


08 design sketch v2
Document · MD 
You said: Let’s take a detour into how multi-agents are supported, and what the gaps are between coding agents, and what challenges there are for supporting them
Let’s take a detour into how multi-agents are supported, and what the gaps are between coding agents, and what challenges there are for supporting them

May 10
Claude responded: Good detour — this is the part of the design that's most "assumed to work" without close inspection.
Good detour — this is the part of the design that's most "assumed to work" without close inspection. Let me research what each agent ecosystem actually exposes before writing.


Searched the web
Searched the web
Excellent — Codex's hooks system is now intentionally Claude-compatible. That's a major finding. Let me check OpenCode and other agents now.


Searched the web
Searched the web
OpenCode has hooks but the model is plugin-based (TypeScript factories, not config-installed shell commands), and event names differ (session.created, session.idle, tool.execute.before/after, chat.message, experimental.chat.messages.transform). Let me also check Aider and Cursor CLI briefly:


Searched the web
Searched the web
Aider doesn't have native hooks — but AiderDesk (the GUI) does. The CLI doesn't. Let me check Cursor CLI and Gemini CLI:


Searched the web
Searched the web
Huge finding: Gemini CLI explicitly aliases CLAUDE_PROJECT_DIR for compatibility and uses a Claude-style JSON-over-stdin contract. So Gemini CLI hooks are 80% drop-in compatible with Claude Code hooks. The event names differ slightly (BeforeTool/AfterTool vs PreToolUse/PostToolUse) but the shape is the same. Let me check Cursor CLI briefly:


Searched the web
Searched the web
Cursor CLI added "Compatibility with Claude Code hooks in CLI" explicitly. So Cursor is also converging on Claude-shaped hooks. And there's a tool called Agent Sessions that already supports JSONL transcripts across 7 agents (Codex, Claude, Cursor CLI, Gemini, Copilot, OpenCode, OpenClaw). I have enough — let me write this up.


Created a file, ran a command, read a file
Created a file, ran a command, read a file
09-multi-agent-support.md — the multi-agent stress test of the design.

The headline finding: Claude Code's hook model has become the de-facto standard. Codex, Gemini CLI, and Cursor CLI all explicitly converged on JSON-over-stdin command hooks with matchers and exit-code semantics. Gemini CLI even ships a CLAUDE_PROJECT_DIR env-var alias for compatibility, and the Gemini hooks v1 spec literally says: "It mirrors the JSON-over-stdin contract, exit code semantics and matcher syntax used by Claude Code." Cursor CLI v2.4 changelog: "Added compatibility with Claude Code hooks in CLI." So "agent-agnostic" is far less of a stretch than the v1 design implied — but only for the four that adopted the convention.

Three tiers of substrate compatibility emerge:

Tier 1 — Shell-hook compatible (Claude, Codex, Gemini, Cursor). Same shim binary with a per-agent config table (event name aliases, config file format, env var names). The Codex adapter is mostly the same shim with TOML config writing and a tool-name table. ~80% drop-in.

Tier 2 — Plugin-required (OpenCode, OpenClaw). Different ingest model (TypeScript plugins running in-process, not config-installed shell commands), but the wire protocol to the daemon is the same. The daemon ships a small plugin per agent that POSTs the canonical envelope.

Tier 3 — Transcript-only (Aider, Copilot CLI). No hook system at all. Pure file tailing. Coarser state vocabulary — likely just idle ↔ working. Pets and lamps still work; sophisticated per-tool dashboards don't.

Ten concrete gaps the analysis surfaced:

Subagent concepts differ nontrivially (Cursor's recursive-async vs. Claude's synchronous-flat vs. OpenCode's primary-vs-subagent split)
Tool-name vocabularies are completely different — requires a per-adapter tool-name → reaction table (Claude's Edit vs. Codex's apply_patch vs. Gemini's replace)
Permission model semantics differ — Claude has structured payloads, Gemini has streaming-UI confirmation states, Cursor has approval modes
Session IDs aren't portable across agents — keying must be (source, session_id), not session_id alone
Process attribution needs source in the AttachmentLocation to disambiguate side-by-side Claude+Codex+Gemini runs
Hook timing semantics all converged on synchronous-blocking — the <5ms shim target isn't Claude-specific
Hook blocking (policy enforcement) stays out of scope; collision returns for that single use case
Statusline coverage is uneven — only Claude has it; context_percentage is effectively Claude-specific
agent_type values are agent-specific — presenters key on (source, agent_type) for voice/sprite mapping
Agent-to-agent communication only exists in Claude's experimental Agent Teams; doesn't generalize
Specific corrections back to the v2 design:

Sessions keyed (source, session_id)
AttachmentLocation adds source
Reaction enum requires per-adapter tool-name table (~50 lines of YAML per agent)
Statusline-only fields are nullable and Claude-specific in practice
The provider abstraction has three classes, not one: HookProvider, PluginProvider, TranscriptProvider (matches Pixel Agents' original "FileProvider and StreamProvider" foresight)
The "agent-agnostic" claim should be precisely scoped: "tier-1 native, tier-2 via plugin, tier-3 via file-tail"
The reaction enum normalization is the one thing the daemon does on top of passthrough, and the cost is per-agent tool-mapping tables
The honest summary: the design holds for tier 1, holds with plugin work for tier 2, holds partially for tier 3. The "agent-agnostic" framing should be sharpened rather than waved.

Worth following up with: (a) sketching what the per-adapter config file actually looks like (event aliases, tool-reaction tables, transcript path patterns); (b) examining the existing tools that already do multi-agent (agent-flow, AgentDeck, opensessions, ccmanager, Agent Sessions) to see what abstractions they converged on; or (c) folding these corrections into a v2.1 design sketch.


09 multi agent support
Document · MD 
You said: Examine tools doing multi agent
Examine tools doing multi agent

May 10
Claude responded: Let me look at how the existing multi-agent tools actually structure their abstractions — what they did concretely, since they've already grappled with this.
Let me look at how the existing multi-agent tools actually structure their abstractions — what they did concretely, since they've already grappled with this.


Check what we already have cloned
Check what we already have cloned
Good — I have opensessions, tmux-agent-sidebar/status, dmux. Let me clone the explicitly multi-agent ones: agent-flow, AgentDeck, ccmanager, Agent Sessions.


Clone multi-agent tools
Clone multi-agent tools
Let me start with the one I already have a fair amount of context on — opensessions, since it explicitly supports multiple agents — to see how it abstracts them.


Opensessions agent abstraction files
Opensessions agent abstraction files
This is exactly what I'm looking for — opensessions has explicit per-agent "watchers" plus a contract abstraction. Let me read the contract.


Opensessions agent contract
Opensessions agent contract
Now the watcher contract:


Opensessions watcher contract
Opensessions watcher contract
This is exactly the abstraction I sketched for v2 — and they already have 5 concrete implementations (amp, claude-code, codex, opencode, pi). Let me look at the per-watcher implementations to see how they handle agent differences.


Size of each watcher
Size of each watcher
~500 lines per agent average. That's the cost. Let me look at the Codex watcher specifically to see how the per-agent translation looks in practice:


Codex watcher head
Codex watcher head
This is gold. Look at the comment block — they document exactly the kind of agent-specific translation work that has to happen. New vs old format, event types, phase distinctions. ~80 lines of documentation before the code starts. This is the cost of supporting a single agent properly.

Let me check the Claude Code watcher briefly to compare:


Claude-code watcher head
Claude-code watcher head
Notice — opensessions chose JSONL tailing for Claude Code too, not hooks. Let me see why and check the OpenCode and amp watchers briefly to spot the pattern.


Opencode watcher head
Opencode watcher head
Different data source entirely for OpenCode — SQLite, not JSONL. Polls a database with bun:sqlite in readonly mode. Now amp:


Amp watcher head
Amp watcher head
A fourth ingest model: cloud REST API + WebSocket. Amp doesn't run locally — it's a remote service.

So in opensessions: Claude (file watch), Codex (file watch), pi (file watch), OpenCode (SQLite poll), Amp (HTTP + WebSocket). Now let me look at ccmanager and agent-flow:


Ccmanager structure
Ccmanager structure
ccmanager supports 8 agents (Claude Code, Gemini CLI, Codex CLI, Cursor Agent, Copilot CLI, Cline CLI, OpenCode, Kimi CLI). The README mentions "Configurable state detection strategies for different CLI tools" — let me find that:


Ran 2 commands
Ran 2 commands
Per-agent state detector files. Let me see how this is structured:


Ccmanager state detector base/types
Ccmanager state detector base/types
ccmanager doesn't tail JSONL or read hooks — it reads the terminal viewport directly. That's a fundamentally different approach. Let me confirm with the claude detector:


Ccmanager claude state detector
Ccmanager claude state detector
Pattern-matching the rendered terminal output. Spinner characters, "thinking..." labels, prompt boxes. ccmanager is a TUI wrapper that PTY-multiplexes the agent CLIs — so it reads the rendered output for state because it owns the terminal. Let me look at agent-flow now since it's more recent:


Agent-flow structure
Agent-flow structure
Two ingest paths: hooks for Claude, JSONL tail for Codex. Let me find the runtime detection:


Ran a command, viewed a file
Ran a command, viewed a file
Even cleaner abstraction — AgentSessionWatcher with onEvent, onSessionDetected, onSessionLifecycle. Each runtime is a factory. Let me see the protocol — the canonical event shape:


Ran a command, viewed a file
Ran a command, viewed a file
Different normalized event vocabulary from opensessions! Now let me check AgentDeck briefly since it's the most ambitious:


AgentDeck structure
AgentDeck structure
Looks like the prompt repeated — continuing the deep-dive into AgentDeck and the remaining tools. Let me see how AgentDeck handles multi-agent specifically.


AgentDeck multi-agent handling
AgentDeck multi-agent handling
AgentType = 'claude-code' | 'openclaw' | 'codex-cli' | 'opencode' | 'monitor'. Let me see the adapter:


AgentDeck adapter abstraction
AgentDeck adapter abstraction
This is the most sophisticated abstraction I've seen — AgentDeck explicitly has a capabilities matrix per agent. They don't pretend all agents are equivalent. They enumerate what features each agent supports and presenters check capabilities.hasModeSwitching before rendering UI for it. Now Agent Sessions:


Ran 2 commands
Ran 2 commands
Agent Sessions: per-agent indexer + parser files, with a UnifiedSessionIndexer on top. Let me check the unifier:


Agent-sessions unified indexer
Agent-sessions unified indexer
Notice — even refresh intervals are per-agent. Codex refreshes every 4s, Claude every 6s. Different agents have different update frequencies. Let me also check ccmanager's command preset approach:


Ran a command, viewed a file
Ran a command, viewed a file
ccmanager's SessionState is much smaller — just 4 values (idle | busy | waiting_input | pending_auto_approval) vs. opensessions' 8. Different abstraction choice. I have enough to write this up properly.


Created a file, ran a command, read a file
Created a file, ran a command, read a file
10-multi-agent-tool-patterns.md — examines five tools that actually shipped multi-agent: opensessions (5 agents), ccmanager (8 agents), agent-flow (2), AgentDeck (4 across 13 surfaces), Agent Sessions (7).

The headline finding: every multi-agent tool surveyed uses at least two different ingest paths for different agents. No tool found a single mechanism that works everywhere. Five distinct ingest models surfaced:

Hooks (config-installed shell commands) — agent-flow's Claude path, AgentDeck's Claude path
File-tail (filesystem watch + JSONL parse) — opensessions for Claude/Codex/pi, agent-flow's Codex path, Agent Sessions for everyone
SQLite polling — opensessions for OpenCode (bun:sqlite against ~/.local/share/opencode/opencode.db)
Cloud REST + WebSocket — opensessions for Amp (it's a cloud-hosted agent, no local files)
Terminal viewport scraping — ccmanager pattern-matches spinner chars (✱✲✳✴...) and prompt-box borders against the rendered PTY output, with per-agent regex packs for 8 agents
The abstractions tools converged on, despite the ingest variety:

Every tool landed on per-agent adapter + canonical event consumer. Interface details differ:

opensessions AgentWatcher: 8-value status enum + 3-value liveness; 5 watcher files totaling 2,488 lines (~500 lines per agent average)
ccmanager StateDetector: 4-value state enum (smaller because it's a session manager, not a visualizer); 8 detector files of pattern-matching constants
agent-flow AgentSessionWatcher: 12-event vocabulary oriented toward graph rendering; separate channels for events / session-discovery / lifecycle
AgentDeck AgentAdapter + AgentCapabilities: the most sophisticated abstraction — explicitly enumerates what each agent supports (hasModeSwitching, hasDiffReview, hasOptionLists, etc.); presenters check capabilities before rendering UI
Agent Sessions: per-agent indexer + parser + per-agent refresh rates (Codex polls every 4s, Claude every 6s — different agents update at different paces)
Three corrections to the v2 design surfaced:

AgentDeck's AgentCapabilities matrix is a real gap in v2. The design treats agents as interchangeable; AgentDeck doesn't. Adding per-source capabilities config (has_permission_payload, has_subagents, has_statusline, has_mode_switching) lets presenters check before rendering UI. Lift this pattern directly.
opensessions splits liveness from status. They're orthogonal — done + alive (waiting for prompt) vs. done + exited (session ended). The v2 design folds liveness into lifecycle; should be separate.
Per-agent refresh rates as config, not hard-coded. Push-based sources (Claude/Codex/Gemini hooks) don't need it; poll-based ones (OpenCode SQLite, Amp REST) do.
Three patterns the surveyed tools themselves didn't crack — places the substrate could win:

Cross-agent identity correlation (Claude on main + Codex on a worktree, same task — no tool groups them; v2's repo_root derivation already does)
Cross-source event ordering (each tool emits with the watcher's own clock; v2's monotonic event_id gives total order)
A shared data layer — every consumer of opensessions/agent-flow/AgentDeck has to integrate with that specific tool. The substrate's pitch sharpens: not another multi-agent visualizer, but the data layer the existing tools could consume from, eliminating their per-agent watcher code
Cost-of-an-agent benchmark: ~500 lines of code + ~80 lines of documentation per new agent. The Codex watcher in opensessions has 80 lines of header comments documenting new-vs-old JSONL formats, event types, phase distinctions, and lifecycle flows before any code runs. That's the realistic ongoing maintenance cost.

The v2 design's abstraction is correct in shape — the surveyed tools converged on essentially the same pattern. What v2 needs to do is ship the abstraction cleanly enough that these tools could adopt it as their data layer instead of writing their own watchers. That's the real value proposition; not "supports many agents" but "you stop having to."

Where to go from here? Three natural directions: (a) write the actual capabilities YAML for the four tier-1 agents based on AgentDeck's matrix; (b) draft a v2.1 design incorporating these corrections; or (c) pivot to a concrete "what's the minimum viable prototype" given everything we now know — probably hook router + sqlite event log + Claude adapter + the AgentWatcher-shaped interface that future contributors can hang Codex/Gemini/OpenCode adapters off of.


10 multi agent tool patterns
Document · MD 
You said: B first.
B first. I also want to think about how to expose something more pub sub, that is easy for any of these tools to consume. We could have it for specific events, but also a way to emit changes to agents so others don’t have to poll

May 10
Claude responded: Good direction — pub/sub is the right shape for this.
Good direction — pub/sub is the right shape for this. Let me think through what events presenters actually want before writing.

Looking at the inventory through the pub/sub lens:

OpenPets, claude-lamp, ccpet — care about current_state transitions (idle→working, working→waiting). They don't want every tool event; they want a small derived signal.
Pixel Agents, Outworked, AgentDeck — care about session/agent lifecycle (new session, new subagent, session ended) plus state transitions per agent.
claude-receipts — cares about exactly one event (sessionEnd) per session.
agent-flow, disler, ccam — want the firehose (every event, for the timeline view).
claude-watch, m5-paper-buddy — care about permissionRequest only when waiting for their user.
Statusline tools (ccpet, tamagotchi-statusline) — don't subscribe; they're polled per-tick. Different model.
claude-status — wants both: session list (snapshot) + state transitions (live).
So the patterns I see:

Firehose subscribers want raw events with filters.
State-change subscribers want derived transitions — "session X went from working to waiting" — not the underlying events that caused it. The daemon already computes this projection; it should be a publishable signal.
Single-event-kind subscribers want exactly one kind (just permissionRequest, just sessionEnd).
Snapshot+delta subscribers want a one-shot snapshot then live updates.
The "emit changes to agents" framing is interesting. Two reads of it:

(a) emit state change signals (not raw events) — derived transition notifications
(b) the substrate emits to agents (back-channel) — but that's HITL, out of scope
I'll go with (a) — that's what makes the most sense for "others don't have to poll."

Let me think about the pub/sub mechanics. WebSocket is the obvious choice (already in v2), but the topic structure is what matters. Topics need to be:

Hierarchical (subscribe to sessions.* or sessions.<id>.state)
Cheap on the broker side (no per-subscriber state machine)
Filterable without server-side compute (presenters express what they want declaratively)
The two-channel split: raw events (firehose with filters) vs. state changes (computed deltas on the projection). Different consumers, different shapes.

Let me sketch this in a v2.1 doc. I'll keep it focused — corrections from doc 10, plus the pub/sub design.


Created a file, ran a command, read a file
Created a file, ran a command, read a file
11-design-sketch-v2-1.md — incremental from v2 with the three corrections plus the pub/sub design as the headline change.

The pub/sub design, briefly:

Two channels, conceptually distinct:

EVENTS — raw events as ingested, append-only, ordered, replayable. The log.
STATE — derived transitions (old → new), last-write-wins, snapshotable. The projection.
The split matters because confusing them produces bad presenter code. A lamp subscribing to events.toolStart gets every tool call from every subagent, ones the user doesn't care about. A lamp subscribing to state.session.<id>.current_state gets one signal per state transition, debounced and deduped by the projection layer.

Hierarchical topics, NATS-style:

events.<kind>.<source>              # e.g. events.toolStart.claude
events.session.<session_id>.<kind>
events.agent.<agent_type>

state.session.<session_id>.current_state
state.session.<session_id>.lifecycle
state.session.<session_id>.attachment
state.sessions.added / removed
state.agents.<agent_type>
The topic shape is the filter — no query parameters, no server-side compute, just prefix-and-glob routing.

Snapshot-on-subscribe solves the snapshot+delta pattern in one round trip. When you subscribe to a STATE topic, the broker sends the current value first, then live updates. No separate GET before subscribing.

State changes carry old/new + back-pointer:

ts
type StateChange = {
  topic: string,
  changed_at: number,
  old: any,
  new: any,
  caused_by_event_id?: number,  // back-pointer for debuggers
};
Cheap to emit — the daemon already computes the projection; the change emitter is a diff on every update.

The four consumer patterns from the multi-agent analysis now have clean answers:

Pattern	Examples	v2.1 shape
Firehose	disler, ccam, agent-flow	SUBSCRIBE events.*
State-change	OpenPets, claude-lamp, ccpet	SUBSCRIBE state.session.<id>.current_state
Single-event-kind	claude-receipts, m5-paper-buddy	SUBSCRIBE events.sessionEnd.*
Snapshot+delta	claude-status, Outworked	SUBSCRIBE state.session.<id> (snapshot frame first)
The two corrections from doc 10 also folded in:

AgentCapabilities per source via GET /sources and per-source capabilities.yaml. Lifted directly from AgentDeck. Presenters check has_permission_payload, has_context_telemetry, has_subagents before rendering UI. The reaction enum gets a reaction_enum_subset field per source — Aider collapses to [idle, working, success, error]; Claude has all 11.
Liveness separated from lifecycle. Lifecycle is per-session (live | paused | abandoned | ended); liveness is per-attachment (alive | exited | unknown). Their combinations carry meaning — live + exited = crashed mid-turn; paused + unknown = transcript-only source we can't probe; ended + alive = diagnostic flag.
Backpressure handling added: bounded queue (default 1,000), dropped frames with event_id range for refetch, opt-in durable subscriptions with disk backing.

The "what this enables" section walks 9 presenters through v2.1. The common shape: subscribe to one or two topics, register a callback per topic, never poll. The substrate gives them changes they care about — not the events that caused them, not the full projection, not snapshots-on-request. Just transition signals.

The v2 → v2.1 diff table at the end captures the changes:

Aspect	v2	v2.1
Live consumption	WS firehose + query filters	Two channels, hierarchical topics
State changes	Implicit (poll)	First-class state.* with old/new
Snapshot+delta	Two separate calls	One subscribe + snapshot frame
Agent capabilities	Implicit	AgentCapabilities per source
Liveness	Folded into lifecycle	Separate per-attachment
Polling rates	Not modeled	Per-adapter runtime.yaml
Backpressure	Unspecified	Bounded queue + dropped frames
Five open questions surfaced for the design itself:

Should state-changes back-pointer to the causing event? (probably yes)
Are state changes guaranteed to follow their causing event in delivery order? (probably yes, single-tx emission)
Wildcards: * only or full glob? (probably * only)
Durable subscriptions: ship now or wait for evidence? (probably wait)
Per-attachment liveness topic granularity? (probably session-level, filter client-side)
Explicitly still out of scope — HITL backflow, cross-machine pub/sub, tool blocking, application-level concepts, cross-agent normalization beyond the reaction enum. The substrate's pitch holds: be the data layer the existing tools could adopt, freeing them from per-agent watcher code. Pub/sub is what makes that adoption easy.

Want to push further? Three natural moves: (a) write the actual capabilities YAML files for the tier-1 agents based on AgentDeck's matrix plus what we found in opensessions/agent-flow; (b) sketch the wire protocol as TypeScript types in a shared @claude-state-bus/protocol package; or (c) do the "minimum viable prototype" memo — what's the smallest version that's useful (probably hook router + sqlite event log + Claude adapter + WS pub/sub + the reaction projection, in Rust or Go for the shim binary).


11 design sketch v2 1
Document · MD 
You said: Let’s start thinking about c, what an mvp would look like, and maybe a few milestones out from that
Let’s start thinking about c, what an mvp would look like, and maybe a few milestones out from that

May 10
Claude responded: Good move.
Good move. Before sketching, let me think about what "minimum" actually means here — what's the smallest version that's useful (not just demoable) and validates the architecture (not just runs).

The temptation with this kind of substrate is to ship the whole shape with nothing real plugged in. That fails because nobody can actually use it. The opposite temptation — ship one full feature end-to-end — fails because it doesn't prove the abstraction holds.

What I think the MVP needs to prove:

The hook router actually replaces fighting over settings.json. This is the single biggest pain point and the entry-cost reason a presenter would adopt.
The reaction enum projection is right. If the 11-value vocabulary serves OpenPets, claude-lamp, and ccpet without forks, the projection is correct. If two of them disagree, it's wrong.
Pub/sub is cheaper than polling for presenters. Measure: how many lines of presenter code, how many event handlers, how many round-trips per second.
agent_type passthrough actually lets persona/voice tools work without their own hook installs. PAI-style voice mapping is the cleanest test.
What it doesn't need to prove in v1:

Multi-agent (one adapter is enough to validate the abstraction shape)
Worktree derivation (cheap to add but not the core proof point)
Statusline composition (different problem; can come later)
Capabilities (only matters once there's a second adapter)
HITL backflow (out of scope anyway)
Multi-host (out of scope)
The "validate without scope creep" question is: what's the smallest set that lets 2-3 real presenters drop their hook code? Let me think about who'd be the test consumers.

OpenPets — already runs MCP-based, would need adapting. Maybe wrong test.
claude-lamp — only ~150 lines. Easy to fork to "subscribe instead of install hook." Good test.
ccpet or tamagotchi — statusline tools. Different consumption pattern; might want them in a later milestone.
PAI / AgentVibes — voice mapping. Tests the agent_type passthrough cleanly.
So the MVP probably proves: hook router + Claude-only adapter + pub/sub for state.* + raw events for events.subagentEnd (for voice) + the reaction enum projection. That's it. No tier-2/3 adapters, no capabilities, no statusline composer.

Let me think about milestones beyond MVP. What's the natural ordering?

M1: MVP (one adapter, hooks + pub/sub + reactions)
M2: Codex as the second adapter (proves the abstraction)
M3: Capabilities + worktrees (the schema additions that pay off when there are 2+ adapters)
M4: Statusline composer (different problem, larger scope)
M5: Plugin provider (OpenCode) — proves the second ingest model
M6: ...
Beyond M5 things get speculative — depends on what users want.

Let me also think about language/runtime. The shim has to be sub-5ms cold-start. That eliminates Node (~50-100ms cold), eliminates Python without compilation, eliminates anything with a JIT warmup. Rust or Go for the shim is the realistic call. The daemon itself could be anything — long-running, doesn't need fast startup. Rust everywhere is probably the simplest call (no language switching across the codebase). Go is the runner-up.

One more thing: I should be honest about what I'm not sure about in the MVP. The reaction enum being correct is an empirical claim — we won't know if it's right until someone actually maps tools to reactions. The pub/sub backpressure model is theoretical. The hook router's failsafe behavior (exit 0 always, never block) needs measurement under real Claude Code load.

Let me also think about what would constitute success vs. failure at MVP. Concretely:

Success: claude-lamp and PAI both work via the daemon, with their hook installation files deleted. Both run in production for a week. Latency from PostToolUse firing to lamp color change is <100ms p95.
Partial: works for one but not the other (uncovers a real abstraction gap)
Failure: shim adds >5ms to every hook invocation (slows Claude perceptibly), or reaction enum doesn't fit (forks needed)
OK, I have enough. Let me write this.


Created a file, ran a command, read a file
Created a file, ran a command, read a file
12-mvp-and-milestones.md — what to build first and how it sequences.

Framing: an MVP isn't a demo. It's the smallest version that one or two real presenters can adopt, validates the load-bearing architectural claims, and leaves a clean shape for everything after.

Four claims the MVP has to prove:

Hook router replaces fighting over ~/.claude/settings.json — the entry-cost pain point
The 11-value reaction enum projection is correct (lamp + sprite agree on meaning)
Pub/sub is cheaper for presenters than polling (measured in lines + round-trips)
agent_type passthrough lets persona/voice tools work without their own hook installs
If those four pass, the substrate has earned its existence.

Explicitly deferred from MVP: multi-agent, capabilities, worktree derivation, statusline, HITL, LAN. Every one is "obviously useful" and every one is a way to spend three months not shipping.

MVP scope (a single Claude Code adapter):

Hook router for 10 hook events (PreCompact and edge cases come later)
Events + sessions + agents tables, no attachments yet
The reaction enum projection (hard-coded Claude tool→reaction table)
Pub/sub WS with events.* and state.* channels, snapshot-on-subscribe, bounded queue + dropped frames
REST polling endpoints as fallback
Per-daemon-run auth token (Pixel Agents' pattern)
Rust shim, statically linked, sub-5ms exit. Node and Python don't meet the bar.
Rust daemon (tokio + axum/hyper + rusqlite). Could be Go; cross-language friction not worth it.
Two real presenters as success criteria — claude-lamp forked to subscribe (~150 lines) and a PAI-style voice presenter (~80 lines). Both in production for a week.

Hard performance bars:

Shim cold start: <5ms p95
Hook → projection update: <50ms p95
Hook → presenter: <100ms p95
Daemon idle: <0.5% CPU, <50MB resident
If any miss by 2x, treat as MVP failure and revisit.

Six milestones beyond MVP — each 2-6 weeks of focused work:

Milestone	Focus	What it validates
M2	Codex adapter	Tier-1 abstraction is correct
M3	Worktrees + attachments + terminal attribution	Schema serves multi-session users
M4	Capabilities + Gemini + Cursor adapters	Tier-1 coverage complete; presenters negotiate features
M5	OpenCode plugin provider	Tier-2 ingest model is correct
M6	Statusline composer	Pulled-data presenters coexist (different problem)
M7+	Sweep / TranscriptProvider / MCP ingest / durable subscriptions	Depends on what users want
Each milestone has a specific presenter that validates it (agent-flow for M2, claude-status for M3, AgentDeck-style for M4, opensessions or AgentDeck for M5, ccpet + state segment for M6).

Risk register, each load-bearing claim with its failure mode:

Shim latency exceeds 5ms (mitigation: profile, mimalloc, allocation-free hot path)
Reaction enum doesn't fit some presenter's need (mitigation: enum is starting point, not gospel)
Pub/sub backpressure overflows under bursty sessions (mitigation: dropped frames + auto-resnapshot in SDK)
agent_type isn't actually used (mitigation: trivial cost, easy to leave in)
Cross-source ordering breaks under clock skew (mitigation: monotonic at daemon ingest, integration tested)
What this isn't: not a product, not Anthropic-affiliated, not a runtime competitor, not Universal Anything. Honest about being plumbing.

Closing principle: the substrate is a piece of plumbing. Plumbing earns trust by doing one job reliably before being trusted with more. Skip M1 in favor of "building it right first" and the project dies.

Where to push next?

(a) Wire-protocol spec — write the JSON Schema/TypeScript types for the pub/sub frames + event envelope as a @claude-state-bus/protocol package so contributors have a fixed target
(b) Adapter-author guide — a single doc explaining "how to add an agent" with Claude as the worked example, so M2/M4/M5 contributors don't have to reverse-engineer the design
(c) Architectural decision records — capture the load-bearing decisions (Rust, monotonic event_id at daemon, two-channel pub/sub, etc.) with the alternatives considered, so the design isn't relitigated each time a new contributor shows up
(d) An honest pitch memo — to nudge specific developers (Pablo De Lucca / Pixel Agents, Alvin Unreal / OpenPets, patoles / agent-flow, puritysb / AgentDeck) on whether this substrate would actually help them. They've each solved part of the problem; would they trade their per-agent watcher code for a shared one?
Any of those, or push on a different thread?


12 mvp and milestones
Document · MD 
You said: I want to think through a few cases to see if this would cover them.
I want to think through a few cases to see if this would cover them.

First is a TUI app that resembles what tmux-agent-sidebar shows, ie a display with agents grouped by git remote, and then items for each agent underneath it, with the ability to show what is idle or waiting for input vs what is working

Second would be a webapp/page that shows 8bit art for each agent running, anf they are colored based on the git remote, and are animated or not based on how much activity tbey are and how full their context window. They stop moving if idle, and start sleeping if the agent is idle long enough, and dies if the process dies

May 10
Claude responded: Good test cases — both stress different parts of the design.
Good test cases — both stress different parts of the design. Let me walk through each carefully, looking at what the substrate provides vs. what each presenter would have to do.


Check how tmux-agent-sidebar handles git remotes
Check how tmux-agent-sidebar handles git remotes
Confirmed — tmux-agent-sidebar gets the remote URL via git remote get-url origin and normalizes it. Let me look at how they normalize:


Tmux-agent-sidebar remote URL normalization
Tmux-agent-sidebar remote URL normalization
OK — they fetch the remote URL as part of a broader "fetch all git data" pass. The substrate (M3 in my milestones) derives repo_root, worktree, branch — but not remote_url. That's a gap for both of your test cases since you want grouping by remote.

Let me think through each case carefully.

Test case 1: TUI grouped by remote, agents underneath, idle vs waiting vs working

What this presenter needs:

Per-session: remote URL. Not currently in the schema. Adding it is cheap — same git rev-parse-adjacent call (git config --get remote.origin.url), cache by repo_root. Doesn't need to be a hot-path field; can be derived once per session-start.
Per-session: state (idle/waiting/working). This is the reaction enum. Eleven values is more than the TUI needs — it'd collapse working/editing/running/testing to "working" itself. Three states out of 11 is fine; collapsing down is presenter-side work.
Live updates when state changes. Pub/sub state.session.<id>.current_state. Already in v2.1.
Session list with snapshot+delta semantics. When the TUI starts, it needs the current list of sessions. When a new session starts, it appears. When one ends, it disappears.
The presenter's loop:

On startup: GET /sessions to populate, also SUBSCRIBE state.sessions.added and state.sessions.removed
Per session: SUBSCRIBE state.session.<id>.current_state for the live status indicator
Group rendering: presenter-side, keyed on remote_url from session row
The gap: remote_url isn't in the schema. Need to add it.

But wait — is this foundational or application-level? Let me apply the test from 07-agent-type-and-foundations.md:

Is this native data presenters need but currently can't access without reimplementing ingestion?

The remote URL isn't really "native data from hooks" — it's derived from git, just like repo_root and branch. The substrate already commits to deriving git fields. Adding remote_url to that derivation is a natural extension, not new responsibility.

The test that confirms it's foundational: would multiple presenters re-derive this? Yes — your TUI wants it for grouping, the 8-bit web app wants it for coloring, and probably any per-repo aggregation tool would. Better to derive once.

Test case 2: 8-bit web app, animation tied to activity + context window, sleep on idle, die on process death

What this presenter needs:

Per-agent (not per-session) rendering. "Each agent running" — does that mean each Claude session, or each agent including subagents? Probably each session (main agents), because subagents are short-lived and you wouldn't have a stable sprite for them.
Color by remote URL. Same gap as case 1.
Animation rate by activity level. This is interesting. "How much activity" isn't currently a derived field. It could mean:
Tool calls per minute
Tokens per minute
Time since last event
The substrate has the events but doesn't expose "rate." Presenter computes from the event stream? Or daemon exposes a smoothed activity rate?
Animation modifier by context window fill. Already covered — context_percentage in the session row (M-something, not in MVP but probably M3 territory since statusline data is available).
Sleep when idle "long enough." "Long enough" is presenter policy. The substrate provides current_state === "idle" plus last_event_at; the presenter decides what "long enough" means. Probably 5 minutes for sleep.
Dies when process dies. This is the liveness signal from v2.1. The substrate emits state.session.<id>.attachment on attachment liveness change. Process death → attachment goes alive → exited → emit state change. Presenter listens and triggers death animation.
The presenter's loop:

GET /sessions to populate; SUBSCRIBE state.sessions.added, state.sessions.removed
Per session: SUBSCRIBE state.session.<id>.current_state, state.session.<id>.context, state.session.<id>.attachment
For activity rate: either compute from events.session.<id>.* firehose, or daemon exposes state.session.<id>.activity_rate
The activity rate question is genuinely interesting. Let me think about it.

If every presenter that wants "how active is this agent" has to subscribe to the event firehose and compute a rolling average, that's a lot of redundant computation across presenters. But baking a specific rate (5-minute exponentially-weighted moving average?) into the substrate is making a policy choice that presenters might disagree with.

A middle ground: the substrate maintains a coarse rolling counter (events in the last 1 minute) updated as events flow in, exposed as recent_event_count on the session row. Presenters that want sophisticated rate computation can still do it from the firehose; presenters that just want "is this agent busy right now" use the coarse counter.

This is borderline application-level. But "is this active right now" comes up in enough presenters (your animated sprite, claude-status's menu bar dot, OpenPets' energy bar, the pet idle detector) that it's worth considering.

Let me write up the walkthrough.


Created a file, ran a command, read a file
Created a file, ran a command, read a file
13-test-cases.md — walked both cases against v2.1, with concrete presenter sketches.

Case 1 — TUI grouped by remote, three-state agent display:

The substrate works with one schema addition: remote_url on the session row.

The natural shape is GET /sessions for the initial list, then SUBSCRIBE state.sessions.added/removed for population changes, then per-session SUBSCRIBE state.session.<id>.current_state for the live status indicator. The 11-value reaction enum collapses cleanly to four display states (idle / waiting / working / error) — presenter does the collapse, substrate stays opinion-free.

The presenter is ~60 lines of code. The grouping logic, the state machine, the projection — all done by the substrate.

Case 2 — 8-bit sprite web app with activity-driven animation:

Three additions surfaced:

remote_url (same as case 1) for sprite color palette
Activity counters (recent_event_count_60s, recent_tool_count_60s) as derived projections, with state.session.<id>.activity topic emitting on bucket changes
context_percentage on the session row (already implied from statusline data)
Plus the existing attachment.liveness (for death), current_state (for sleep transitions), and lifecycle (for the longer "abandoned" state). The v2.1 liveness/lifecycle split is what makes "die when process dies" cleanly expressible.

The presenter is ~120 lines. Five subscriptions per session, presenter-side animation curves, idle thresholds, death policies.

What the exercise surfaced about the design:

One clear addition is foundational: remote_url on the session row. Both presenters need it. The substrate already commits to running git at session start for repo_root/worktree/branch — adding remote_url to the same derivation is ~30 extra lines. Multiple inventoried tools would benefit (tmux-agent-sidebar already has its own implementation; the substrate could replace it).

One addition is borderline tier-2: activity counters. At least 8 inventoried tools (OpenPets, ccpet, Tamagotchi, claude-quest, claude-status, AgentDeck, Outworked, this 8-bit web app) want some form of "how busy right now." Each computes it differently today. Worth exposing as a sliding-window counter — small daemon cost, real savings across presenters. But presenters could compute it from the firehose, so it's not strictly foundational.

Two things stayed presenter-side, validating the principle:

Idle threshold for "sleep long enough" — presenter policy (5 min vs. 1 min varies by tool)
Death policy — liveness: exited + how-long-to-wait-for-resume is the presenter's call
What didn't surface but I expected to:

HITL backflow — neither case needs it (TUI shows waiting state, doesn't approve; web app animates, doesn't take input)
LAN reachability — both presenters are localhost
Cross-source presentation — both implicitly Claude-only; the source field would handle it if added
Capabilities checks — only matter for richer presenters that render agent-specific UI (mode switching, permission options)
Updates to the MVP/milestone plan:

MVP scope unchanged — both test cases can wait for M3. But they tighten what M3 needs to deliver: remote_url derivation in the M3 git pass, activity counters as the same era of derived projections, and context_percentage from statusline data pinned in.

After M3, both presenters are buildable in 60-120 lines. That's the right bar — if presenters of this complexity take more than ~200 lines, the substrate is too low-level.

Three observations worth keeping:

The v2.1 design's load-bearing claims all held — pub/sub topics matched what the presenters naturally wanted, the reaction enum projection was correct, liveness/lifecycle split was meaningful, snapshot-on-subscribe worked for startup population.
The substrate stayed out of presenter decisions consistently. No daemon-side opinions about idle thresholds, animation curves, color palettes, grouping logic, or death timing. This is the principle from 07-agent-type-and-foundations.md working as designed.
The natural M3 scope tightened: not just repo_root/worktree/branch/attachment-liveness/terminal-attribution, but also remote_url and activity counters. Still a coherent ~3-6 week milestone; just slightly fuller.
A third test case worth running later: a multi-agent version of the same TUI (Claude + Codex). Would stress the cross-source ordering and capability negotiation concerns and validate M4. Worth doing once M4 is sketched concretely.

What's next — more test cases (the multi-agent one, or one that stresses HITL like a deck/wearable approval interface), or push back into the actual design/code work?


13 test cases
Document · MD 
You said: Let’s look at some of the tools that wiuld benefit from activity level, to see what they do to measure/surface/etc them
Let’s look at some of the tools that wiuld benefit from activity level, to see what they do to measure/surface/etc them

May 10
Claude responded: Good — let me actually look at what these tools do rather than guess.
Good — let me actually look at what these tools do rather than guess. The novelty survey mentioned several but I never dug into how they measure activity.


Check what I have for activity-measurement tools
Check what I have for activity-measurement tools
Good — I have ccpet, tamagotchi, openpets, pixel-agents, outworked, AgentDeck, and could clone claude-quest. Let me start with ccpet which is statusline-tick-based:


Ran 2 commands, viewed a file
Ran 2 commands, viewed a file
This is exactly what I needed. Key findings:

ccpet's "activity" is token-based — feed(tokens) adds energy; applyTimeDecay() removes it over time
Token-to-energy conversion: TOKENS_PER_ENERGY config (every N tokens = 1 energy)
Decay: ~0.0231 per minute by default (3 days from 100→0)
Inputs come from somewhere — let me find where feed() is called

Ran a command, viewed a file
Ran a command, viewed a file
Important — ccpet reads the JSONL transcript directly via getTokenMetrics(transcript_path, ...). Statusline tick triggers everything. Let me see what gets returned:


Ran a command, viewed a file
Ran a command, viewed a file
Interesting — ccpet maintains a global-tracker.json file at ~/.claude-pet/global-tracker.json with lastProcessedTimestamp. It only counts new tokens since last statusline tick. This is incremental activity measurement via JSONL re-tail. Now let me look at tamagotchi:


Ran a command, viewed a file
Ran a command, viewed a file
This is the key one:

ts
// Activity tracking (NEW)
sessionUpdateCount: number;     // Updates in current session
totalUpdateCount: number;       // Lifetime update counter
lastUpdateTimestamp: number;    // For gap detection
recentUpdateTimestamps: number[]; // Last 30 updates for activity rate
Tamagotchi tracks "last 30 timestamps" as a sliding window for activity rate. Plus a gap-detection field for idle inference. Let me see how they compute the rate:


Tamagotchi activity computation
Tamagotchi activity computation
There's an actual ActivitySystem.ts. Let me read it:


Tamagotchi ActivitySystem
Tamagotchi ActivitySystem
I had started on this before the conversation compacted. I have ccpet and tamagotchi data already. Let me continue with the others I flagged as pending: openpets, pixel-agents, outworked, AgentDeck, and claude-status.


Ran 3 commands
Ran 3 commands
OpenPets doesn't have activity tracking — its reaction enum collapses everything to the four hook speech categories. It's a discrete-event tool, not an activity-rate tool. Let me check the client protocol — that's where state lives:


Ran a command, viewed a file
Ran a command, viewed a file
Confirmed: openpets uses discrete reactions, not activity rates. The 11 reactions are pure event-derived. Idle is a state, not a derived rate. Now pixel-agents:


Ran 3 commands, viewed a file
Ran 3 commands, viewed a file
Pixel Agents' idle thresholds:

TEXT_IDLE_DELAY_MS: 5000 — 5s of no transcript activity → idle
EXTERNAL_ACTIVE_THRESHOLD_MS: 120,000 — 2 minutes is the line for "active vs. stale" external agents
GLOBAL_SCAN_ACTIVE_MAX_AGE_MS: 600,000 — 10 minutes is "considered alive for global discovery"
Three different timescales for three purposes: per-agent idle (5s), external alive-or-not (2min), global active discovery (10min). Now Outworked:


Ran 2 commands, viewed a file
Ran 2 commands, viewed a file
Outworked maps statuses (which come from event hooks) to animation states — no rate computation. Status changes are themselves the trigger; the animation runs at a fixed speed appropriate to its anim type. No "activity rate" concept. Let me see how agents become idle vs. working:


Ran 2 commands, viewed a file
Ran 2 commands, viewed a file
Outworked has 6 statuses (working | done | waiting-input | waiting-approval | slow | stuck) — and notably "slow" and "stuck" are derived from timing, not from events. Let me find where:


Outworked slow/stuck detection
Outworked slow/stuck detection
Two-tier stuck detection: 5 min = "slow" (soft warning), 10 min = "stuck" (allows abort). Tracked per-agent via lastActivity timestamp + setInterval. Now AgentDeck:


Ran 3 commands
Ran 3 commands
AgentDeck uses discrete state names with no activity rate: IDLE | PROCESSING | AWAITING_PERMISSION | AWAITING_OPTION | AWAITING_DIFF | DISCONNECTED. State transitions trigger TTS, sprites, voice. Not a rate-based system.

Now let me clone claude-quest since the RPG sprite tool was mentioned:


Clone claude-quest
Clone claude-quest
Go-based. Let me look for activity/HP/idle logic:


Ran 2 commands, viewed a file
Ran 2 commands, viewed a file
UpdateFlow — a "flow meter" that increases on activity and decays after 5 seconds of no activity. Decay rate 0.03/sec. Activity increment +0.05 per event. This is the simplest formulation I've seen — a single 0..1 number representing "is the user in flow." Let me see what defines hadActivity:


Ran 3 commands
Ran 3 commands
Only defined, not yet called from elsewhere — looks like work-in-progress. The shape is informative though. Let me also do a quick check at claude-status (gmr's menu bar):


Ran 3 commands, viewed a file
Ran 3 commands, viewed a file
claude-status uses 4 discrete states (active | waiting | idle | compacting) and tracks lastActivityAt (a single timestamp). No rate. Activity is "what's the elapsed time since lastActivityAt?" — used for both sort order and for showing "5 minutes ago" labels.

This is a different model again: a single timestamp for activity, not a sliding window or rate.

I have enough to synthesize. Let me write it up.


Created a file, ran a command, read a file
Created a file, ran a command, read a file
14-activity-survey.md — examined 8 tools (ccpet, tamagotchi, OpenPets, Pixel Agents, Outworked, AgentDeck, claude-quest, claude-status) for activity measurement mechanisms.

The headline finding flips the proposal from 13-test-cases.md. I'd argued activity counters were a tier-2 worth-exposing primitive because "8 tools want this." After looking at what those tools actually do, the picture changed: only 1 of 8 actively wants a rate (tamagotchi). The rest want simpler things.

The three mechanisms tools actually use:

Mechanism	Tools	What it captures
Single timestamp (last_activity_at)	claude-status, Outworked, Pixel Agents	Time elapsed since last event
Sliding window of timestamps	tamagotchi (only)	Events in last 60s, classified into tiers
Leaky-bucket scalar	claude-quest (only)	Continuous 0..1 flow score
Discrete state only	OpenPets, AgentDeck	No rate at all; transitions are the signal
Token consumption as proxy	ccpet	Tokens/tick read from JSONL
5 of 7 tools use discrete state. 3 of 7 use a single timestamp. Only tamagotchi computes a window-based rate. claude-quest computes a scalar.

The three timescales that consistently appear:

Timescale	Purpose	Where
5 seconds	"Currently working right now?"	claude-quest grace, Pixel Agents text-idle
1 minute	"How intense?"	tamagotchi intensity tiers (>20 / >10 / else)
2-5 minutes	"Wandered off? Soft warn"	Pixel Agents external-active, Outworked slow, tamagotchi session-gap
10 minutes	"Stuck or abandoned"	Pixel Agents cleanup, Outworked stuck
Specific tool details worth keeping:

tamagotchi's ActivitySystem.ts: maintains recentUpdateTimestamps: number[] (last 30 statusline ticks), computes intensity = count_in_last_60s. Three tiers (>20 intense, >10 active, else normal) drive extra energy/hunger drain. 5-min session gap triggers "sleep recovery" of sleepHours * 10 energy per hour, max 8 hours.
ccpet: not really activity at all — it's tokens. Pet.feed(tokens) reads from JSONL, applyTimeDecay() removes energy at 0.0231/min (3 days from 100→0). Maintains ~/.claude-pet/global-tracker.json to only count new tokens since last tick.
Outworked: two-tier stuck detection with setInterval against lastActivity. SLOW_TIMEOUT_MS = 5min (soft warning), STUCK_TIMEOUT_MS = 10min (abort button). slow and stuck are status enum values derived from timing, not events.
claude-quest: simplest rate model. FlowMeter += 0.05 per event; decay 0.03/sec after 5-second grace. Effectively a leaky bucket.
claude-status: just lastActivityAt: Date. Used for sort order and "3m ago" labels. No rate, no thresholds.
Pixel Agents: three timescales explicitly (TEXT_IDLE_DELAY_MS = 5s, EXTERNAL_ACTIVE_THRESHOLD_MS = 2min, GLOBAL_SCAN_ACTIVE_MAX_AGE_MS = 10min) for animation, alive-or-not, cleanup.
The revised proposal:

Drop recent_event_count_60s and recent_tool_count_60s from the v2.1 schema. They're single-tool convenience, not foundational primitives.

What stays:

last_event_at on the session row (already implicit; make it explicit)
The events log itself, subscribable for tools that want to compute their own rate
context_percentage on the session row — 3 tools use this; keeps its proposed status
remote_url on the session row — case 1 in 13-test-cases.md, still valid
What presenters do client-side (all cheap):

Sliding window (tamagotchi pattern) → 15 lines, deque of timestamps from events subscription
Leaky bucket (claude-quest pattern) → 10 lines, increment + render-frame decay
Time-since-activity (claude-status pattern) → 1 line, Date.now() - last_event_at
Threshold checks (Outworked pattern) → 5 lines, setInterval on last_event_at
Edits this triggers:

13-test-cases.md: remove activity counters from case 2's needs; replace with "presenter computes rate from event firehose"
11-design-sketch-v2-1.md: no schema changes (v2.1 didn't commit); just confirm last_event_at in section 5
12-mvp-and-milestones.md: M3 scope unchanged in size (drops counters, gains remote_url)
The closing observation that crystallizes the principle: the substrate's job is to make event consumption cheap, not to compute every possible derived signal. Pub/sub already makes consumption cheap. The instinct in 13-test-cases.md to expose counters was based on a survey that hadn't happened yet. Now that it has, the substrate stays smaller and more honest.

This is exactly the kind of correction the design benefits from — a hypothesis stress-tested against real evidence and adjusted. Worth doing the same for the other "tempting addition" candidates (capabilities matrix, statusline composer, etc.) as a sanity check before committing.

Want to:

(a) Apply the corrections back to docs 11/12/13 explicitly so they're internally consistent
(b) Run the same survey-against-evidence test on capabilities (which 4 tools actually need negotiation vs. which would just key on source)
(c) Push on the wire-protocol spec as concrete TypeScript types so the abstraction is testable
(d) Pivot to looking at concrete implementation choices (Rust crate structure, the actual hook router state machine, the storage schema migrations)

14 activity survey
Document · MD 
You said: Apply corrections
Apply corrections

May 10
Claude responded: I'll apply the corrections from the activity survey back into the affected docs.
I'll apply the corrections from the activity survey back into the affected docs. Three places need updates: 11-design-sketch-v2-1.md (confirm last_event_at is in the schema), 12-mvp-and-milestones.md (M3 scope adjustment), and 13-test-cases.md (revise case 2's "what it needs" section).

Let me check each first to make precise edits.


Ran 2 commands, viewed a file
Ran 2 commands, viewed a file
Now let me update v2.1 section 5 to explicitly call out last_event_at:


Add last_event_at clarification to v2.1 schema
Add last_event_at clarification to v2.1 schema
Now the v2 → v2.1 diff table needs updating too — let me add the last_event_at clarification:


Viewed a file, edited a file
Viewed a file, edited a file
Now 12-mvp-and-milestones.md. Find the M3 description:


Ran a command, viewed a file
Ran a command, viewed a file
Now update M3 to add remote_url, context_percentage, and last_event_at, with a note about activity rate:


Update M3 with corrections from test cases and activity survey
Update M3 with corrections from test cases and activity survey
Now let me update 13-test-cases.md — the "what it needs" section for case 2 needs to drop the activity counters and replace with "presenter-computed":


Find activity counters section in 13
Find activity counters section in 13
I need to revise sections 198-250 (the activity-rate discussion within case 2), the presenter sample code that uses recent_event_count, the summary section, and the M3 update note at the bottom. Let me look at each region:


Viewed a file, edited a file
Viewed a file, edited a file
Now let me fix the presenter sample code that uses state.session.<id>.activity and activity_score:


Viewed a file, edited a file
Viewed a file, edited a file
Now update the verdict + summary section for case 2:


Viewed a file, edited a file
Viewed a file, edited a file
Now the closing M3 update notes:


Viewed a file, edited a file
Viewed a file, edited a file
Let me verify the changes are clean and consistent:


Verify no stale references to dropped activity counters
Verify no stale references to dropped activity counters
The remaining references are intentional — they're in the "explicitly not in M3" and "why this was reversed" passages. Let me also check that the v2.1 doc has the last_event_at clarification flowing into the right places and the diff table is consistent:


Verify last_event_at appears in v2.1
Verify last_event_at appears in v2.1
Now copy the updated docs to outputs and confirm:


Ran 2 commands
Ran 2 commands
Sizes consistent with the edits. Let me do a final consistency check to make sure no other docs reference the dropped concepts (especially earlier docs that might be linked from the MVP/milestone view):


Check earlier docs for stale references
Check earlier docs for stale references
Clean — no stale references in earlier docs. Let me present the three updated files:


Presented 3 files
Presented 3 files
Corrections applied across three docs. Summary of what changed:

13-test-cases.md — the source of the original (incorrect) proposal, rewritten honestly:

Case 2's "Question 3" section rewritten — drops the Option A/B framing. Acknowledges directly that the original instinct was wrong, cites the survey, shows two presenter-side patterns (sliding window and leaky bucket) in actual code.
Sample presenter code updated — activity_score field replaced with recent_events: number[] (sliding window); state.session.<id>.activity subscription replaced with events.session.<id>.* subscription that simply pushes timestamps; activityScore(s) computed from the deque inline. Total line count unchanged (~120 lines); presenter does ~6 more lines for the window, daemon does zero.
Verdict revised from "three additions" to "two additions" (remote_url, context_percentage).
Summary section rewritten — "One borderline addition: activity counters" replaced with "One non-addition: activity rate counters" explaining the reversal with the per-tool breakdown.
Closing M3 update notes revised — removes the activity-counter line, replaces with "M3 does not add activity counters" and a one-sentence explanation pointing to the survey.
12-mvp-and-milestones.md — M3 milestone scope tightened:

Section title: "Worktrees, attachments, terminal attribution" → "Worktrees, attachments, terminal attribution, derived session fields"
Added to M3: remote_url derivation (~30 lines), context_percentage on session row, last_event_at documented as universal activity signal
Explicitly not in M3: paragraph stating activity counters were considered and rejected after the survey, with cross-reference to 14-activity-survey.md
Success bar updated to reference both test cases being buildable in 60-120 lines
11-design-sketch-v2-1.md — section 5 (schema) and section 9 (diff table) updated:

New subsection in §5: last_event_at is the universal "is this active" signal. Explains why it's exposed but no STATE topic for it (changes on every event = noisy). Points presenters to the EVENTS channel for fine-grained activity.
§9 diff table: new row for last_event_at ("Implicit" → "Documented as the universal activity signal; presenters compute rate client-side")
Consistency verified:

The only mentions of recent_event_count left in any doc are the three intentional ones explaining the reversal (one in each of docs 12, 13, 14)
No stale references in earlier docs (07, 08, 09, 10) — they pre-dated the proposal so nothing to update
last_event_at appears in v2.1 schema, M3 scope, and is referenced in both test-case presenters
What this actually demonstrates is the design discipline working as intended — the principle from 07-agent-type-and-foundations.md ("the daemon preserves and exposes underlying data, doesn't define application-level concepts") was used to test a specific proposal, and the test surfaced that the proposal failed by the principle's own criteria. The substrate stays smaller. Presenters stay slightly more responsible. The trade is fair.

Where to next?

(a) Apply the same survey-against-evidence discipline to the capabilities matrix proposal from doc 11 — which tools actually need negotiation vs. which would key on source directly?
(b) Write the wire-protocol spec as concrete TypeScript types (@claude-state-bus/protocol package) — turn the abstraction into something testable
(c) Adapter-author guide as a single doc with Claude as the worked example — what M2 contributors would need to write a Codex adapter
(d) Pivot to concrete implementation choices — Rust crate structure, the hook router state machine, storage schema migrations

13 test cases
Document · MD 

12 mvp and milestones
Document · MD 

11 design sketch v2 1
Document · MD 
You said: Now That we have gotten this far, lets take another at opensession specifically, and think about what gaps exist between it and what we’ve designed, as well as…
Now That we have gotten this far, lets take another at opensession specifically, and think about what gaps exist between it and what we’ve designed, as well as how much effort it would be to contribute to that

May 10
Claude responded: Good move — opensessions is the closest thing to what we've designed that already exists.
Good move — opensessions is the closest thing to what we've designed that already exists. Let me look at it carefully rather than going from memory.


Ran 2 commands, viewed a file
Ran 2 commands, viewed a file
This is illuminating. Let me check the server architecture and the WS protocol:


Ran 2 commands
Ran 2 commands
Now let me look at the server proper to see what API surface it exposes:


Opensessions server index
Opensessions server index
Massive file (2,615 lines). Let me check what shape they push over WebSocket and what the HTTP API looks like:


Ran a command, viewed a file
Ran a command, viewed a file
Critical insight: opensessions broadcasts the entire ServerState on every change — no pub/sub topics, no diff-based delivery, no snapshots-on-subscribe. Every TUI client gets the full session list every time anything changes (the broadcast is microtask-coalesced but otherwise sends the whole state).

This is a substantial architectural difference. Let me check the HTTP endpoints:


Ran 2 commands
Ran 2 commands
No GET endpoints. Only POSTs (for actions) and WebSocket (for receiving state). The TUI is the only consumer. No polling REST surface for other tools. Now let me check the WS handshake and look for any auth:


Ran a command, viewed a file
Ran a command, viewed a file
So the WS API:

One channel: "sidebar"
On open: client subscribes; gets full last ServerState snapshot
Every state change → full ServerState rebroadcast to all clients
Client → server messages are ClientCommand actions (focus, switch session, etc.)
No auth. Bind is to 127.0.0.1 by default, but the constants suggest it can be 0.0.0.0 for LAN
Now let me check what an external "presenter" would have to do. There's a programmatic-api.md referenced:


Programmatic API docs
Programmatic API docs
Programmatic API is write-only metadata, not read-side event consumption. Now the key question — what's the agent ownership / canonicalization logic:


Agent ownership logic
Agent ownership logic
Now let me check the tracker, which is the projection layer equivalent:


Opensessions tracker
Opensessions tracker
Important details:

8-value status enum (the 6 from CONTRACTS plus tool-running and stale)
Priority-based aggregation (when a session has multiple agent instances, pick the highest-priority status)
MAX_EVENT_TIMESTAMPS = 30 — keeps last 30 timestamps per session
TERMINAL_PRUNE_MS = 5 * 60 * 1000 — 5min cleanup of terminal states
Now look at storage and persistence:


Opensessions persistence
Opensessions persistence
No SQLite, no event log. opensessions is fully in-memory. The only on-disk state is session ordering preferences and metadata (status pills set via the programmatic API). Events flow through, get tracked, expire from memory after 5 minutes.

This is a major architectural difference. Let me check the issues/PRs to see what the maintainers think is missing:


Searched the web
Searched the web
Important: 0 PRs merged over the lifetime of the repo, 5 open issues, 1 open PR. This is significant for the "contribute to it" question. Let me look at the open issues:


Searched the web
Searched the web
431 stars, 25 forks, but 0 PRs merged. The maintainers ship their own changes but haven't yet integrated outside contributions. This is critical context for "how much effort to contribute." Let me also note: their thesis statement ("the primary user is an agent, not a person") is well-aligned with our design philosophy.

I have enough. Let me write this up.


Created a file, ran a command, read a file
Created a file, ran a command, read a file
15-opensessions-gap-analysis.md — a careful walk through what opensessions already has, what it lacks vs. our design, and what contributing would actually look like.

Short answer: opensessions has built 60-70% of the substrate's job. The remaining 30-40% is structural enough that contributing is roughly equivalent in effort to building standalone — and there's a complication that materially changes the calculus.

What opensessions has that matches the design:

AgentWatcher interface — essentially identical to what we sketched. Five built-in watchers (Amp, Claude Code, Codex, OpenCode, pi) totaling ~2,500 lines of agent-specific parsing. This is real engineering investment we'd otherwise duplicate.
AgentEvent shape — 8-value status enum, 3-value liveness, threadId/threadName for per-instance tracking, paneId for terminal attribution
Tracker / projection with priority-based aggregation (tool-running > running > error > stale > ...) — they thought through aggregation we hadn't pinned down
Multi-watcher merging into a single state payload
Programmatic POST API for metadata (status pills, progress, logs) — the write-side equivalent of our orchestrator-event POSTs
Mux abstraction with capability gates — the exact pattern we lifted for AgentCapabilities
What opensessions lacks (the gap):

The big ones:

No persistence. Entirely in-memory. Events expire from the tracker after 3-5 minutes. No SQLite, no event log. Restart → history gone.
No pub/sub topics. One WebSocket channel called "sidebar". Every state change broadcasts the entire ServerState to all connected clients. No filtering, no per-presenter subscriptions, no snapshot-on-subscribe.
No GET endpoints. None. Only POST (write metadata) and WS (read full state mirror). No way to query event history, sessions, or sources.
No hook router. Claude Code is consumed via JSONL tail with 2s polling — they sidestepped the hook collision problem entirely rather than solving it.
The smaller ones:

Narrower reaction enum (8 vs. our 11)
No subagent_type / agent_type as first-class field
No repo_root, worktree, or remote_url derivation (only branch and isWorktree boolean)
No AgentCapabilities per source
What opensessions has that we don't:

Unseen-state semantics — per-instance unseen tracking with TUI-visible markers. We hadn't modeled this.
canonicalizeAgentEvent for resolving thread-bound events to the actual pane/session
Mux-native hooks (tmux session-created, client-attached) for instant state changes rather than polling
Stale-process detection server-side (pruneStuck marks runs as stale after no updates)
Three places where worldviews are incompatible, not just gapped:

opensessions is N:1 (many sessions → one TUI client). The substrate is N:M (many sessions → many heterogeneous presenters). Adding pub/sub + REST event API changes the architecture, not just adds features.
opensessions persists nothing. Adding SQLite wouldn't break the TUI but would change what the project is — the database becomes source of truth, the in-memory tracker becomes a cache.
opensessions models a sidebar. Status pills have tones (info/success/warn), logs are capped at 50 entries for display, status text truncates at 100 chars. The metadata API is shaped like "things you pin to a sidebar row" rather than "raw events you can render however."
The "would the PRs land" question is the kicker: 0 merged PRs over the project's history. 25 forks, 431 stars, 5 open issues, 1 open PR. The project ships actively from the maintainers but hasn't yet integrated outside contributions.

Possible reads — they may be solo-maintained and in "shipping fast" mode, they may be philosophically aligned but architecturally protective, they may accept small features but not structural changes, or they may simply be young-project inertia.

The effort table (~5-8 weeks of focused work to close the gap):

Change	Effort	Risk
repo_root / worktree / remote_url	half day	Low
Hook router for Claude	2-3 days	Medium
agent_type as first-class	~day per watcher	Medium
Capabilities matrix	1-2 days	Low
SQLite persistence	1-2 weeks	High
Hierarchical pub/sub	1-2 weeks	High
GET endpoints (depends on persistence)	~week	Medium
Snapshot-on-subscribe + backpressure	~week	Medium
For comparison: building standalone Rust per the MVP was estimated at 6-8 weeks.

The architectural read — even if all PRs landed, the result would be a hybrid project heavier than either pure goal. opensessions optimizes for "TUI is fast, server is light." Layering persistence + pub/sub onto it makes the TUI's data path more complex than it needs to be, while still not being as clean as a substrate built for many presenters.

The honest path forward:

Engage maintainers directly with a discussion or issue: "I'm building agent-state infrastructure with goals X/Y/Z. Would opensessions be receptive to additions like persistent events + pub/sub topics, or is that scope-divergent?"
Start with remote_url as a small low-risk PR to test the contribution loop
If structural contributions are off the table — propose factoring the per-agent watchers (packages/runtime/src/agents/watchers/*) into a standalone npm package (@agent-watchers/*) that both opensessions and the substrate could consume. This is the "library extraction" pattern that lets two projects share infrastructure without one absorbing the other.
Build standalone if neither lands — using opensessions' patterns as reference where applicable
The discipline: don't fork, don't reimplement what's working, don't pretend opensessions doesn't exist — but also don't bet the substrate's existence on a maintainer interaction that hasn't happened yet.

Where to push next?

(a) Draft the actual GitHub discussion/issue to maintainers — what would the message look like, what specifically to ask
(b) Sketch the watcher-extraction proposal in more depth — what would @agent-watchers/* actually look like as a published package
(c) Continue with the wire-protocol spec or implementation choices, treating opensessions as "good reference, not a host"
(d) Look at the one open opensessions PR + 5 issues directly to see what they're already working on

15 opensessions gap analysis
Document · MD 
You said: Want to take a different angle.
Want to take a different angle. I want to think about how to maintain this as an open source project, while still allowing feedback and input, but having pretty well grounded avout what is and isn’t in scope.
Another piece of that is how to structure/architect to make it easier to maintain ofer time. There is a python library pocketflow, and one of it’s philosophies is that it is tiny, has good agent memory/instructions and lots of cookbooks and examples. Pi-mono is another thing with a similar philosophy

May 10
Claude responded: Let me research these two before writing — I should ground in what they actually do rather than guess.
Let me research these two before writing — I should ground in what they actually do rather than guess.


Searched the web
Searched the web
Now pi-mono:


Searched the web
Searched the web
Now I have grounding on both. Let me also quickly check pi-mono's actual contribution model:


Searched the web
Searched the web
I have a lot of grounding now. Both projects share a striking philosophy and have very explicit "what's in scope vs. out of scope" stances. Let me distill what matters for the substrate project.

The most striking patterns:

PocketFlow:

100 lines of core code, zero dependencies
"If we need a feature, build it as a node on top of the core" — not as a core change
.cursorrules / .windsurfrules files included for the AI to learn the framework
Cookbook directory with ~30 example apps
The framework is documented well enough for AI to extend it without human help
pi-mono:

4 tools only (read, write, edit, bash)
"No MCP, no sub-agents, no permission popups, no plan mode, no built-in to-dos, no background bash"
Explicit "Why?" answers in the README for each thing they don't include
"If you need X, build it as an extension or install a pi package"
"New issues and PRs from new contributors are auto-closed by default" — maintainers review daily
Aggressively extensible via hooks (event-driven extension system with ~20+ lifecycle hooks)
Extensions distributed via npm/git — no registry, no gatekeeping
AGENTS.md for project-specific rules (humans and agents both)
Project ships its own coding agent sessions publicly as documentation
The "auto-close PRs from new contributors" pattern is genuinely interesting. It looks hostile at first but it's actually a clear signal: "this is opinionated, we control direction." And it's paired with aggressive extensibility — if you want something we don't have, you don't change the core, you write an extension.

Let me write this up.


Created a file, ran a command, read a file
Created a file, ran a command, read a file
16-maintainership-and-scope.md — how to maintain this as an open source project.

The two reference models distilled:

PocketFlow (100 lines of core, zero dependencies): the entire abstraction is "graph + shared store." Vendor-specific code is out of core — users implement their own call_llm. Patterns like multi-agent / RAG / supervisor are documented in a cookbook of ~30 examples, not built into the framework. Ships .cursorrules / .windsurfrules / .clinerules to teach AI tools how to use the framework. The author explicitly says: "the more complex the framework, the harder it is for AI to help."

pi-mono (4 tools, ~1000-token system prompt, 44k+ stars): the README lists what's deliberately absent — no MCP, no sub-agents, no permission popups, no plan mode, no built-in to-dos, no background bash. Each "no" comes with a "why" and an alternative. Contribution policy: "New issues and PRs from new contributors are auto-closed by default. Maintainers review auto-closed issues daily." Aggressively extensible via 20+ lifecycle hooks; third-party packages distributed via npm/git with no central registry. The "awesome-pi-agent" repo curates ~40 community packages. Core doesn't grow; ecosystem does.

Five principles both share:

Core small enough to fit in one head (PocketFlow 100 lines, pi-mono 4 tools)
What's out of scope is named, justified, and given alternatives
Extensibility is first-class and unbounded — extensions ship via standard package mechanisms, no gatekeeping registry
AI agents are intended consumers of the documentation (AGENTS.md, .cursorrules)
The author publicly demonstrates use of their own tool (cookbook entries, published sessions)
How this maps to the substrate, concretely:

Scope discipline — write the no-list explicitly. The substrate already has an implicit no-list across the design docs; making it explicit is the leverage. Nine items drafted, each with the "why" and the alternative pathway:

No HITL backflow (sibling service)
No tool blocking (per-agent hook installs)
No cross-machine pub/sub (build your own relay)
No persona / voice / sprite (presenter responsibility)
No statusline composer in MVP (M6+)
No multi-agent in MVP (M2+)
No agent runtime competitor (we observe, don't run)
No on-disk presenter state (presenters own that)
No durable subscriptions in MVP
Contribution model — borrow pi-mono's auto-close pattern. PRs from new contributors auto-closed with a templated message pointing to: the no-list, the extension/adapter pathway, the discussion forum. Maintainer reviews weekly. Adapter PRs treated differently (additive, low-architectural-risk, welcomed). Documentation PRs always reviewed.

Repository structure optimized for AI extension authors:

claude-state-bus/
├── README.md           # single page, philosophy + no-list
├── AGENTS.md           # project rules for humans + AI
├── docs/               # reference (design, protocol, adapter-authoring)
├── crates/             # protocol, shim, daemon, adapter-claude
├── adapters/           # capabilities.yaml + tool-reactions.yaml per agent
├── examples/           # tested in CI — lamp, voice, grouped-tui, sprite-web
└── cookbook/           # recipes; each entry references a working example
Split: docs/ is reference, cookbook/ is recipe. Every cookbook entry references a tested example. CI runs examples against a real daemon, so docs can't drift silently.

Sizing the core:

protocol/: ~500 lines (types + serialization)
shim/: ~500 lines (hot path; complexity is anti-feature)
daemon/: ~3,000-5,000 lines (storage, pub/sub, projection, hook router)
adapter-claude/: ~1,000 lines (reference adapter)
Total: ~5,000-7,000 lines of Rust. Comparable to opensessions' TypeScript core, much smaller than LangChain (>400k). If the core grows past 10k lines, something is wrong — we've either absorbed a "no" into the core, over-engineered an abstraction, or scope has expanded silently.

Four extension surfaces that don't require core changes:

New adapters — third-party agent adapters, distributed however the author wants, discovered via config directory
Presenters — anyone subscribes to the documented pub/sub protocol
Capability extensions — community-supplied capabilities.yaml declares custom flags; daemon passes through without interpreting
Event kinds — third parties POST kinds the substrate hasn't seen; daemon stores and routes; doesn't interpret
What requires core changes:

New STATE topics (projection layer decides)
Reaction enum changes (would require coordination with all presenters)
New persistence
Wire protocol changes (versioned, careful evolution)
Versioning split: protocol@v1 is stable; breaking changes require v2 with parallel support. Internal implementation (Rust struct layout, SQLite schema, function signatures) is not stable — daemon can refactor freely as long as wire protocol stays compatible.

Documentation aimed at AI maintainers — three concrete practices:

AGENTS.md with project-specific rules ("hot-path code must not allocate on success path"; "wire protocol changes require a new version package and an ADR")
Architecture Decision Records (ADRs) for load-bearing decisions — Rust choice, two-channel pub/sub, reaction enum, etc. Alternatives considered and why rejected. When a future AI agent considers reverting one, they read the ADR first.
Examples must be tested in CI — no silent documentation drift
Feedback flow that doesn't dilute scope:

GitHub Discussions = front door; maintainer responds with one of: "yes file an issue," "this is on the no-list," "this is adapter/presenter concern not core," or "speculative — come back in a month"
Issues = confirmed work only
Adapter contributions = explicitly welcomed, reviewed faster
No-list updated quarterly based on what came up
Eight concrete actions for the project's first month:

README.md as a single ~200-line page
AGENTS.md with conventions
GitHub Discussions enabled with templated auto-close
Ship MVP (two reference presenters in production)
Write first three ADRs (Rust choice, two-channel pub/sub, reaction enum)
Write four cookbook entries (subscribe to state, subscribe to events, compute activity client-side, handle dropped frames)
Write the adapter-authoring guide using Codex as worked example
Start a public "agent state journal" — weekly post about real use
Honest risks acknowledged:

Auto-closing can feel hostile → friendly templated message + clear pointers
Small core means slower feature growth → that's the trade; not for everyone
Extension ecosystem may not materialize → ship Codex adapter yourself at M2 to seed it
AI-readable docs are a moving target → treat as living document
No-list can ossify → quarterly review pass; if reversing a no requires reverting an ADR, you'll see the cost
The closing principle: PocketFlow and pi-mono both demonstrate a counter-intuitive truth: the more aggressively you constrain the core, the more useful the ecosystem becomes. The substrate's "one thing" is observe agent state, expose it via pub/sub, never get in the way. Everything else is extension. The maintainer's job is to defend the smallness as fiercely as they champion the usefulness.

Where next?

(a) Draft the actual README.md and AGENTS.md as concrete artifacts you'd put in the repo
(b) Write the first three ADRs (Rust, pub/sub, reaction enum) with alternatives considered
(c) Draft the no-list as a standalone doc with all nine items expanded
(d) Sketch the adapter-authoring guide using Claude as the worked example, since Codex doesn't exist yet
(e) Look at one of the inventoried tools through this maintenance lens — what would they have done differently if they'd followed this discipline from the start?

16 maintainership and scope
Document · MD 
You said: Draft no-list, but for the ones in scope but not yet, let’s call them not yet, so waiting for the right opportunity
Draft no-list, but for the ones in scope but not yet, let’s call them not yet, so waiting for the right opportunity 

And let’s try drafting those readme ajd agents.md

May 10
Claude responded: Good — three artifacts to draft.
Good — three artifacts to draft. Let me think briefly about each before writing.

The no-list with two categories: "never" (out of scope by principle) and "not yet" (in scope but waiting for the right opportunity). The split matters because "not yet" items signal where contributions could land later, while "never" items signal hard architectural commitments. Let me re-examine the original list:

HITL backflow → never (synchronous wait blows shim budget; different project)
Tool blocking → never (vetoing requires sync blocking)
Cross-machine pub/sub → not yet (LAN reachability is plausibly useful for AgentDeck-style bridges; just complex)
Application-level concepts (persona, voice, sprite) → never (presenter responsibility by design)
Statusline composer → not yet (M6 in roadmap; different problem but in scope)
Multi-agent in MVP → not yet (M2+ is explicit)
Agent runtime competitor → never (we observe, don't run)
On-disk presenter state → never (presenters own their state)
Durable subscriptions → not yet (waiting for evidence)
Activity rate counters → never (14-activity-survey.md settled this)
Other things that surfaced through the docs that should be on the not-yet list:

TranscriptProvider for Aider/Copilot → not yet (M7+ speculative)
MCP-based ingest → not yet (could be a 4th provider type)
Sweep / abandoned-session detection → not yet (M7+)
Statusline polling endpoint for current state segment → not yet (part of M6 composer)
README.md — single page, ~200 lines max per the maintainership doc. The pi-mono README is a good model: clear what-this-is, prominent no-list, install instructions, philosophy section. I should make it concrete enough that someone unfamiliar with the design history can land and understand what it is in two minutes.

AGENTS.md — project rules for humans and AI agents. The pi-mono and PocketFlow conventions are different. pi-mono's AGENTS.md is hierarchical (global, project, package). It includes specific rules like "hot-path code must not allocate." Should include the project's coding conventions, architectural rules, when to write tests, when to update docs alongside code.

Let me write all three.


No-list with never vs not-yet split
No-list with never vs not-yet split
Now the README. The principle is: someone unfamiliar with the project lands here and understands what it is in two minutes. The pi-mono README is a model — clear what-this-is, prominent what-it-doesn't-do, install, philosophy.


README.md draft
README.md draft
Now AGENTS.md. The pi-mono pattern is hierarchical (global, project, package). For our project, this is the conventions doc that both human contributors and AI agents working in the codebase read first.


Created a file, ran a command
Created a file, ran a command
All three drafted. README is 221 lines (close to the 200-line target), AGENTS is 242 (compact for what it covers), no-list is 161.


Presented 3 files
Presented 3 files
Three artifacts. Brief summary of each:

17-no-list.md — 10 items split into two categories.

Never (6):

Synchronous HITL backflow (sync wait blows shim budget)
Tool-call blocking and policy enforcement (use parallel hook installs)
Application-level concepts: personas, voices, sprites, moods, color palettes (presenter responsibility)
Acting as an agent runtime (observation-only by design)
On-disk presenter state (presenters own their state)
Computing activity rate daemon-side (14-activity-survey.md settled this)
Not yet (9):

Cross-machine pub/sub (auth + transport + NAT is enough work to block MVP)
Multi-agent support beyond Claude (M2 = Codex, M4 = Gemini/Cursor, M5 = OpenCode)
Statusline composer (different consumption model; M6+)
TranscriptProvider for hook-less agents (M7+)
MCP-based ingest as fourth provider type (waiting for a use case)
Durable subscriptions with disk-backed queues (waiting for evidence of need)
Sweep / abandoned-session detection (presenter-side coverage works)
Capability extensions declared by community packages (bake the surface first)
Persistent agent identity across daemon restarts (reconstruction works for now)
Web UI for the daemon (debug-only; if built, ships as examples/debug-dashboard/)
Each entry has a "why" and a pathway. The "not yet" entries include what evidence would move them into milestone planning. Closing section on how to propose new entries and how to promote not-yet items.

README-draft.md — 221 lines, single-page overview.

Structure:

ASCII diagram showing shim → daemon → pub/sub → 3 example presenters
Pre-MVP status acknowledgment
"Why this exists" — the hook collision pain point
"What it does" / "What it doesn't do" (no-list summary with link)
Install (homebrew + cargo)
Quick taste — 3 code samples (state subscription, event subscription with agent_type, REST snapshot)
Philosophy (preservation principle, only-one-normalization, presenter sizing target)
Contributing — pi-mono auto-close pattern, adapter PRs welcomed faster
Project layout
License + related projects (opensessions, AgentDeck, PocketFlow, pi-mono, OpenPets)
AGENTS-draft.md — 242 lines, conventions for both human and AI contributors.

Structure:

What this project is (~1 paragraph + cross-links)
Before writing code (no-list check, ADR check, protocol check, cookbook check)
Coding conventions broken out by crate:
Hot-path rules for shim (no allocation, no async runtime, no retry, fire-and-forget always)
Daemon code style (tokio, rusqlite WAL, module sizing, error handling)
Protocol crate (versioned, JSON wire format, deny_unknown_fields, no extra deps)
Tests (unit alongside code, examples tested in CI, benchmarks gate the shim)
Adapter code structure (crate + data files layout)
Documentation conventions (when to update, cookbook shape, ADR format)
Working with AI agents — two sections, one for humans directing agents and one for agents themselves
Common pitfalls — 6 specific traps (transactional state/event emission, (source, session_id) keying, presenter concerns in projection, STATE topic explosion, hook delivery reliability, atomic settings.json writes)
Repository-wide commands
Decision authority (maintainer has final say; ADR/no-list-first; no committees)
The pieces fit together:

The no-list is the artifact contributors check before filing things.
The README is the first impression — what is this, what's it not, how to install, what's the philosophy.
AGENTS.md is the in-repo guide that both humans and AI agents read before changing code.
All three reinforce the same discipline: small core, explicit boundaries, presenter-side extensibility, no committee or consensus required for direction.

A few editorial notes I'd flag for review:

The README's "quick taste" section assumes JavaScript/Node clients. Could be worth a parallel Rust example for the audience that wants to embed the substrate.
The AGENTS.md pitfalls list will need to grow over time — I seeded six but real mistakes will produce more. Worth pinning it as a living document.
The auto-close contribution model is fine for pi-mono because it's a household name with 45k stars; in a young project it might feel especially abrupt. Consider softening the wording in the templated auto-close message itself when that exists.
The README cites Doc 14 and Doc 16 as "useful starting points" for design rationale. Once the project is real, those should probably be lifted up into the docs directory as numbered design docs rather than referenced by inventory number.
Where to push next?

(a) Draft the templated auto-close message that ships with the contribution model
(b) Write the first three ADRs (Rust, two-channel pub/sub, reaction enum) with alternatives considered
(c) Sketch the adapter-authoring guide using Claude as the worked example (since Codex doesn't exist yet)
(d) Draft the wire protocol spec as concrete TypeScript types
(e) Take a step back and review the full doc set (17 docs + 3 drafts) for consistency and identify what's missing

17 no list
Document · MD 

Readme draft
Document · MD 

Agents draft
Document · MD 
You said: Let’s make a list of cookbooks we could include, based on all the surveying we’ve done.
Let’s make a list of cookbooks we could include, based on all the surveying we’ve done. Probably keep it more general purpose tho, ie don’t require hardware or APIs we can’t validate on

May 10
Claude responded: Good direction.
Good direction. The constraint matters: cookbooks need to be self-contained, runnable in CI, no external API dependencies, no hardware. They should demonstrate substrate patterns through plausible examples that anyone can validate.

Let me think about this systematically. Looking back at all the inventoried tools and the patterns they exercise:

The presenter patterns from 12-mvp-and-milestones.md:

Subscribe to one session's state
Subscribe to specific event kinds
Subscribe to firehose (raw events)
Snapshot + delta consumption
Multi-session aggregation
Per-agent-type routing
The signals from 13-test-cases.md and 14-activity-survey.md:

Reaction enum collapse (11 → fewer states)
remote_url grouping
last_event_at for elapsed-time displays
Activity rate (sliding window vs. leaky bucket)
liveness / lifecycle distinction
context_percentage
The structural patterns:

Reconnect / dropped frames / resnapshot
Auth token handling
Backpressure
Polling vs. pub/sub trade-offs
What presenters in the wild actually do — distilled into patterns:

claude-lamp → state-driven color (3-4 states)
ccpet → token-based decay metaphor + statusline tick
tamagotchi → activity-driven mood + sliding window
PAI / voice → event-kind filter + agent_type routing
claude-receipts → single-event-kind, batch processing
claude-status menu bar → snapshot + lifecycle topics
agent-flow → firehose for visualization
Outworked → multi-session display with stuck detection
tmux-agent-sidebar → group by remote, status indicator
AgentDeck → multi-presenter coordination
Now, the constraint: no APIs we can't validate. So:

No "voice via ElevenLabs" → fine to mention voice as a concept, but the cookbook should output to stdout or a logfile
No "BLE bulb color" → could be a CSS color string written to a file
No "Stream Deck button" → terminal output instead
No "Discord notification" → maybe echo or webhook to localhost
The 8-bit web app from case 2 actually works as a pure HTML+JS thing — that's validatable
I want to also think about what teaches the substrate, not just what's a cool tool. Cookbooks should each teach one distinct concept clearly. Some natural categories:

Subscription mechanics (the basics, foundational)
State patterns (how to consume the projection)
Event patterns (how to consume raw events)
Derived computation client-side (rate, gaps, elapsed)
Multi-session and grouping (aggregate views)
Resilience (dropped frames, reconnection, daemon restart)
Adapter authoring (building your own ingest)
REST polling (the alternative to pub/sub)
Let me think about which specific recipes within those make sense. I'll aim for ~15-20 recipes total — enough variety without being overwhelming.

I should also think about which ones are "minimum viable cookbook" (essential for shipping) vs. "fills out the picture" (nice to have, can wait).

Let me draft this.


Created a file, ran a command, read a file
Created a file, ran a command, read a file
18-cookbook-catalog.md — 24 recipes across 8 categories, each tagged for which milestone it ships in.

The constraints applied: nothing depends on external APIs we can't validate (no TTS, no Discord, no LLMs), no hardware (no BLE bulbs, no Stream Deck, no e-ink), no HITL backflow (per the no-list). Every recipe outputs to stdout, a file, or browser HTML. Every recipe corresponds to an example in examples/ that runs in CI.

The eight categories:

Subscription basics (3 recipes) — hello-state, hello-events, auth-and-reconnect. The minimum viable presenter knowledge.
State consumption (4 recipes) — collapsing the 11-value enum into 4 display states, snapshot+delta for session lists, rendering permission requests without answering them (which is on the no-list), lifecycle vs. liveness combinations.
Event consumption (3 recipes) — single-event-kind subscribers (the claude-receipts pattern), routing on agent_type (the PAI voice pattern, but to stdout instead of TTS), client-side filtering by tool name.
Derived computation (4 recipes) — sliding-window activity (tamagotchi), leaky-bucket activity (claude-quest), time-since displays (claude-status), stuck detection (Outworked). The recipes that demonstrate "the substrate exposes signals, you compute the rate" from 14-activity-survey.md.
Multi-session and grouping (3 recipes) — grouping by remote_url (test case 1 from 13-test-cases.md), aggregating tool counts across sessions, single highest-priority indicator (claude-status menu bar).
Resilience (3 recipes) — handling dropped frames with refetch, surviving daemon restart, REST polling fallback for tools that don't want WebSocket.
Visual and integration patterns (3 recipes) — single-file HTML sprite display (test case 2, browser-only, no hardware), TUI sidebar, markdown daily summary.
Adapter authoring — separate longer-form guide (docs/adapter-authoring.md), not a cookbook recipe per se, but referenced from the cookbook.
Milestone breakdown:

18 recipes at MVP (M1) — everything that doesn't need M3 schema additions
+5 recipes at M3 — the ones that need remote_url, context_percentage, attachment liveness, or stuck-detection signals
+1 at M2 — the adapter-authoring guide using Codex as the worked example
That's 24 total. Feels appropriately dense without being overwhelming.

The deliberate exclusions:

External-API recipes (TTS, Discord, OpenAI) — each user installs their own
Hardware recipes (BLE, Stream Deck, e-ink) — each user wires their own
HITL backflow (answering permission prompts) — out of scope per the no-list
Multi-agent recipes in MVP — Codex doesn't exist in M1
Statusline composition — different problem, M6+
Persistent presenter state — would suggest the substrate provides it (it doesn't)
Three patterns I made explicit that I think are doing real work:

The CI requirements section. Every example must build, run against a fresh daemon, produce expected output, and have its README excerpt match the cookbook. This is what prevents documentation drift — exactly the discipline pi-mono and PocketFlow both follow.
The branching reading order. Different presenter types follow different paths through the cookbook. Notifier/logger → 3.1, 3.2, 7.3. Visualizer → 2.1, 2.2, 5.3 + 7.1/7.2. Analytics → 6.3, 7.3. Pet/sprite/lamp → 2.1, 4.1/4.2, 7.1. This means a new presenter author doesn't read all 24 recipes; they read 4-6 in a focused path.
Source attribution per recipe. Each pattern credits the tool it was distilled from (tamagotchi, claude-quest, claude-status, Outworked, etc.). This both gives credit and lets readers trace back to fuller implementations if they want.
One thing I'm uncertain about:

The single-file HTML sprite display (7.1) is more polish than the other recipes. It would require some non-trivial SVG/animation work. Worth thinking about whether the simpler "TUI sidebar" version (7.2) is enough to demonstrate the multi-session-visualization pattern, with the HTML version reserved as a "showcase example" that ships later. Decision: keep both, but mark 7.1 as M3 so it doesn't block MVP.

The big things this catalog accomplishes:

It makes the substrate's value concrete. A potential adopter reading this catalog sees: "I want to build X — they have a recipe that's 90% of the way there." That's the difference between "interesting framework" and "I should try this."

It validates the substrate's interface against actual presenter patterns. If a recipe is awkward to write, the substrate's API is wrong. The exercise of drafting these recipes is itself a design review.

It establishes the substrate's pedagogy. The cookbook entries teach the patterns; the design docs teach the rationale; the protocol doc teaches the contract. Three layers of documentation, each targeting a different reader.

Where next?

(a) Draft one cookbook entry in full as an example (probably 1.1 Hello current state, with the full code, the README, and the examples/hello-state/index.js)
(b) Draft the wire protocol spec — what the JSON messages actually look like, what fields each event kind has, what the STATE topic frames carry
(c) Draft the first three ADRs (Rust choice, two-channel pub/sub, reaction enum) with alternatives considered
(d) Look at the full doc set (now 18 design docs + 3 drafts) and identify gaps or contradictions before going to implementation

18 cookbook catalog
Document · MD 
You said: Let’s make one last document about everything we covered, what we we have written, and options we haven’t explored (ie when given 3 choices we only chose one o…
Let’s make one last document about everything we covered, what we we have written, and options we haven’t explored (ie when given 3 choices we only chose one or none)

May 10
Claude responded: Good move — a synthesis document is the right way to close this thread.
Good move — a synthesis document is the right way to close this thread. Let me think about what should be in it.

The doc should serve two readers: someone (possibly future-me, possibly an AI agent picking this up later) who wants to know the state of the design without reading 18 docs, and someone who wants to find the unexplored threads to push on. So structure:

What we built (the design we converged on)
What we wrote (the doc inventory, with one-line summaries)
The major decisions and what alternatives we didn't take
The threads we left unpulled (when given options, the ones we deferred)
What's actually unresolved vs. just unwritten
Let me also think about what kinds of unexplored options there are:

Times you gave me explicit menus (a/b/c/d) and we picked one
Architectural choices where alternatives were rejected with reasoning (Rust vs Go, SQLite vs other)
Test cases proposed but not run (multi-agent TUI)
Tools mentioned but not deeply examined
Documents drafted but not finalized (README/AGENTS as drafts)
Patterns surfaced but not turned into design
And I should be honest about what's actually a real "we didn't explore" vs. what's just "we made a choice and moved on." The first is fertile ground; the second is closed.

Let me scan back through to make sure I capture the choice-points accurately.


Ran 2 commands
Ran 2 commands
OK — 17 design docs (01-18, with 05 missing since I never created it), plus README-draft and AGENTS-draft. Let me write the synthesis doc.


Synthesis doc — state of the design and unexplored options
Synthesis doc — state of the design and unexplored options
The synthesis document already exists from earlier in our conversation — let me copy it to outputs and present it, then quickly verify it's the right scope for what you're asking.


Ran a command, read a file
Ran a command, read a file
19-synthesis.md — 347 lines, the closing document for the design thread.

Structure:

What we set out to do — the premise from doc 01 (hook collision problem, the substrate hypothesis)
What we converged on — the five load-bearing components plus the doc 07 principle that governed every other decision
Artifacts produced — table of all 17 numbered design docs (01-18, 05 unused) plus the README and AGENTS drafts, with one-line purposes
Major decisions and what we didn't pick — eight load-bearing choices walked through with the alternatives considered and what each rejection closed off:
Language/runtime (Rust vs. Go vs. Node/Bun vs. Python)
Storage (SQLite vs. LMDB vs. append-only log vs. Postgres)
Pub/sub model (two channels vs. one + filters vs. multi-WS vs. gRPC vs. SSE)
Reaction enum (OpenPets 11 vs. opensessions 8 vs. ccmanager 4 vs. claude-status 4 vs. invented)
Provider abstraction (three classes vs. one vs. five)
Capabilities matrix (booleans + enum-subset vs. none vs. arbitrary types vs. runtime discovery)
Activity rate (client-side only, reversed from doc 13)
Maintainership (pi-mono auto-close vs. default-accept vs. maintainer-only vs. tiered)
When given options, what we deferred — eight inflection points across the conversation where there were 3-5 explicit choices and we picked one. The unpulled threads, by doc:
Doc 01's (c) — apply design lens to a specific tool
Doc 06's (c) — adversarial review of the substrate premise itself
Doc 10's (a) — sketch actual capabilities.yaml for tier-1 agents
Doc 11's (b) — wire-protocol spec as TypeScript types (flagged as highest leverage)
Doc 12's (d) — pitch memo to specific tool authors
Doc 13's (b) — survey-test capabilities the way we survey-tested activity
Doc 15's (a) — draft GitHub discussion to opensessions maintainers
Doc 17's (b) — first three ADRs
Actually unresolved (vs. just unwritten) — six things that would shift the design if surfaced:
Wire protocol stability promise (strict vs. loose vs. tiered)
Authority for lifecycle transitions over time
Two adapters claiming the same session
Auth model beyond MVP
What "in production for a week" concretely means for MVP success
Whether a single needs_user_attention boolean would actually serve everyone (the line between "presenter concern" and "universal signal" is thinner than the no-list claims)
Next session priority order — five high-leverage actions:
Wire protocol spec
First three ADRs
Adapter-authoring guide using Claude as worked example
Survey-test the capabilities matrix
Implementation starting from the shim hot path
What I'd do differently — four retrospective observations:
Skip docs 03/04 (v1 design and critique); doc 06's principles should have come first
Do the activity survey before adding activity counters
Examine opensessions earlier (it's been the closest analog the whole time)
Pick a tool early and design against it; doc 13's test cases came too late
Closing observation — the two most load-bearing disciplines:
Doc 07's preserve-underlying-data principle (governs every correction)
Doc 16's smallness discipline from PocketFlow/pi-mono (constraining core makes ecosystem useful)
Both reinforce each other: smaller core, clearer boundaries, more capable presenters.

The bottom-line claim: the design is ready for implementation. The unwritten artifacts (wire protocol spec, ADRs, adapter guide) are valuable but not blocking. What's blocking is shipping the shim, daemon, Claude adapter, and two reference presenters — and seeing what survives contact with reality.

Now, looking at this synthesis with fresh eyes: a few honest notes about what it does and doesn't cover well.

What I think it captures well:

The "alternatives we didn't pick" section is genuinely useful as a record — it preserves the reasoning behind the choices
The "actually unresolved vs. just unwritten" distinction is important; conflating the two would create a misleadingly long todo list
The next-session priority order is grounded in the threads that came up repeatedly across multiple docs
What I'd flag for review:

The "what I'd do differently" section is risky to include — it's hindsight, and someone picking this up fresh could over-update on it. The docs we'd skip (03, 04) weren't wasted; they were the path to the principles. If you'd rather, I can soften that section or cut it.
The synthesis treats the implementation as the next phase but doesn't grapple with the "should we implement at all, or contribute to opensessions" question from doc 15. That's the most material strategic choice still open, and I underweighted it here. Worth surfacing more directly.
I didn't include anything about what success looks like at the project level (downloads, adoption, contributors). The MVP success criteria are technical, not adoption-based. That's probably fine for now but worth naming.
Want me to revise any of these, or are we ready to call the design phase done?


19 synthesis
Document · MD 
You said: Some followup thoughts:
Some followup thoughts:
I don’t think we ever specified how exactly the emitter sends messages to the daemon. There’s lots of options there like another http endpoint. One cocnern i have is eith latency on the emitters due to the daemon being unresponsive, slow, or just not running. Worst possible case is not having any kind if timeout and taking awhile, next worse is having an error that surfaces to rhe user as a hook failurr

May 10
Claude responded: You're right — this is a genuinely unspecified piece, and the failure modes you're naming are exactly the right ones to worry about.
You're right — this is a genuinely unspecified piece, and the failure modes you're naming are exactly the right ones to worry about. Let me think about it carefully before drafting anything.

The shim is invoked by Claude Code on every hook event. The hook config says something like command: claude-state-bus-shim. Claude runs it, captures stdout/stderr, looks at the exit code. If the shim exits non-zero or takes "too long," Claude either surfaces an error or stalls — both of which are unacceptable.

So what does the shim actually need to do? Receive a hook payload (from Claude, on stdin), forward it to the daemon, exit fast.

The transport options I can think of:

HTTP POST to localhost — straightforward but TCP setup has overhead (~1ms even on loopback), and HTTP failure modes (connection refused, slow read, timeout) are exactly what you're worried about
Unix domain socket — same shape as HTTP semantically but lower-level, faster, no TCP setup. Failure modes are similar.
Named pipe (FIFO) — write to a path; daemon reads. Blocks if no reader. Hard to non-block reliably across platforms.
Fire-and-forget UDP to localhost — no connection setup, no waiting for ack. Zero failure modes that surface to the caller, but no delivery guarantee.
File spool only — shim only ever writes a file; daemon watches a directory. Decouples completely but trades latency for reliability.
Hybrid: try socket with hard timeout, fall back to spool — most options have this shape
The constraints we've stated: <5ms p95, never block Claude, never surface error to user. The shim should be fire-and-forget from Claude's perspective regardless of daemon health.

Let me also think about what happens when:

Daemon isn't running at all (laptop just booted, daemon not yet started)
Daemon is running but overloaded (long GC pause, blocked on something)
Daemon is starting up (port not yet bound)
Daemon crashed mid-write
Network stack is weird (loopback firewall, IPv6/IPv4 mismatch)
The spool fallback handles all of these uniformly — the shim writes to disk and exits; the daemon picks up spooled events on startup or whenever it's ready. The question is whether spool should be the only mechanism (simpler) or a fallback from a faster path (lower latency in the common case).

Let me also think about hard timeout enforcement. POSIX connect(2) with a non-blocking socket + select() with timeout is the right shape on Unix. But the failure case (timeout hit) means the shim has done work and learned nothing — should it spool on timeout? Yes, definitely.

There's also a question of whether the shim should fork-and-detach. If the parent exits immediately after writing to disk (or kicking off a background process), Claude sees a clean exit, and a child process handles the actual network send. This decouples Claude's wait from the network round-trip entirely.

OK, I have enough to write this up. The actual design space is:

Tier 1 (preferred path): Unix domain socket connect-with-timeout (1-2ms), write payload, close, exit
Tier 2 (fallback on any failure): write to spool directory, exit
Tier 3 (catastrophic): can't write to spool either? Log to stderr, exit 0 anyway (never fail the hook)
And the shim never actually waits for the daemon to acknowledge anything. Connect-with-timeout means we wait up to N ms for the connection to establish but we don't wait for any response after sending. UDS is connection-oriented like TCP, so there's a handshake, but it's local and fast.

Let me also consider: should the shim spawn a background detached process for the network send, so the parent can exit even faster? That's tempting but adds complexity (process management, signal handling, double-fork pattern) and the UDS round-trip on loopback should be sub-millisecond. Probably not worth it for v1.

One more thing: the spool format. Newline-delimited JSON in ~/.claude-state-bus/spool/. Each file is one event. Atomic file creation via O_CREAT | O_EXCL with <timestamp>-<random>.ndjson. Daemon picks them up, processes, deletes. This is the same pattern that systemd journal and similar tools use.

Let me write this up.


Created a file, ran a command, read a file
Created a file, ran a command, read a file
20-shim-daemon-transport.md — fills in the unspecified piece you flagged.

Headline answer: the shim emits over Unix domain socket in the common case, falls back to a disk spool on any failure, and always exits 0 regardless of what happened internally.

Six transport options considered:

Option	Verdict
HTTP POST to localhost	Workable but heavy — TCP handshake adds 0.5-2ms even on loopback, HTTP framing is overkill, many failure shapes to handle
Unix domain socket	Picked. Sub-millisecond connect, no TCP layer, trivial to write from any language, connection-refused is the only "daemon down" failure mode
UDP datagram	Tempting (cannot fail from sender's view) but silently drops under load — we'd lose events without knowing, which violates the "know when things are broken" principle
Named pipe / FIFO	Multi-writer interleaving above 4KB is a real problem; not worth the corner cases
File spool only	Essential as fallback, too slow as primary (10-50ms FS-event latency)
Hybrid: UDS preferred, spool fallback	The actual answer. Fast common path, reliable fallback, both exit clean
Latency budget (held in all cases):

Path	Median	p95
UDS happy path	~0.8ms	~1.5ms
UDS → spool fallback (daemon down)	~3ms	~5ms
Spool only	~2ms	~3ms
Catastrophic (spool write fails)	~3ms	~5ms
5ms p95 budget holds in every scenario, including the catastrophic one.

The crucial property restated: the shim's exit code is always 0, regardless of what happened internally. A separate diagnostic command (claude-state-bus diagnose) checks for accumulating spool files, malformed entries, recent stderr — users who want to know "is my pipeline healthy" run that. The shim never tells them. Claude never sees a failed hook.

Failure mode walkthrough — what happens in each scenario:

Daemon not running (first boot): UDS → connection refused (~50µs) → spool fallback → exit 0. When daemon starts, drains the spool.
Daemon starting up: Same path. No loss, just delayed delivery.
Daemon overloaded (long GC pause): UDS connects but write hangs → 2ms timeout fires → spool fallback → exit 0. Daemon catches up via FS watcher.
Daemon crashes mid-write: The one acknowledged loss scenario. Shim sent partial bytes over UDS before crash; bytes discarded; event not in spool because UDS succeeded from the shim's view. Fixable by writing spool first then UDS, but trades latency in happy path. Defer until real loss is observed.
Daemon crashes after ack, before commit: Also lossy, mitigated by daemon writing to SQLite WAL immediately.
Firewall weirdness on 127.0.0.1: UDS doesn't touch the network stack, so unaffected.
Spool unwritable: Shim exits 0 anyway, event lost, diagnostic catches it.
Three trade-offs explicitly named:

Delivery guarantee for never-blocking — spool events arrive late and out-of-order with UDS events. Daemon sorts by event_id (ULID with millisecond precision generated at receipt) so the log is well-ordered, but live pub/sub subscribers see late timestamps. Cookbook will document handling this.
Durability for latency — no fsync() on the spool write. Kernel crash between write and page-cache flush could lose events. Acceptable because rare, and observational data loss isn't a correctness problem.
Simplicity for two code paths — pure-spool would be simpler. Cost: ~70 lines of code total for both paths. Bounded complexity.
What this implies for the daemon (the other side of the contract):

UDS listener (Tokio UnixListener, ~30 lines)
Spool directory watcher (notify crate — inotify on Linux, FSEvents on macOS)
Startup spool drain before opening UDS — so events that accumulated during downtime are visible to subscribers in correct order
Periodic spool sweep every 30s (FSEvents has reliability quirks under load — belt-and-suspenders)
Malformed file quarantine (move to .malformed/ subdirectory rather than infinite-loop on a corrupt file)
Concurrent ingest from UDS and spool with serialized SQLite writes
Four open questions deferred to post-MVP:

Should the shim fork-and-detach for the network send? Would reduce wallclock to ~100µs but adds process management complexity. Revisit if hook latency complaints appear.
Should the daemon ack at the application layer? Currently "ack" is just connection close. Would catch the daemon-crashes-after-accept loss scenario but costs 0.5ms round-trip.
How aggressive should spool cleanup be? Malformed files accumulate forever; daily sweep at M3 if it becomes a problem.
Does the shim need a config file? Only if multi-daemon setups appear; defer until asked.
A few honest notes on this design:

The UDS choice depends on macOS/Linux only. If Windows ever matters (not in MVP per the design), we'd need to revisit. Windows 10+ supports AF_UNIX but with quirks, and named pipes are the more idiomatic Windows shape.

The "always exit 0" rule is load-bearing. Everything else in this design exists to make that rule hold without losing too much data. If we ever soften it (e.g., exit 1 on spool write failure to surface "your disk is full" to the user), the entire failure-handling story changes. Worth treating as a hard architectural commitment, probably an ADR.

The data-loss scenario (daemon crashes between UDS accept and SQLite commit) is small but real. Naming it explicitly is better than pretending it doesn't exist. The cost of fixing it is one extra disk write in the happy path; we can flip that later if loss matters more than the 1ms.

The spool-event-arriving-late property is a real complication for presenters. Cookbook recipe 6.1 (handling dropped frames) should probably grow a section on "handling out-of-order delivery from spool fallback" — though most presenters won't care because they're consuming current state, not strict event order.

This should probably go into docs/decisions/ as an ADR once the repo exists. Title would be something like "ADR-005: Shim-to-daemon transport via UDS with spool fallback." The alternatives section above is most of the content.

Where to go next?

(a) Continue with another unspecified piece — the daemon-to-presenter pub/sub wire format is the other major unwritten contract
(b) Promote this to ADR form and write the full first three ADRs
(c) Look at the implementation order — what's the smallest end-to-end slice that exercises both UDS and spool paths?
(d) Surface other "we didn't specify X" gaps you're aware of — better to surface them now than to discover them mid-implementation

20 shim daemon transport
Document · MD 
You said: How common is a spool directory?
How common is a spool directory? Specifically in rust?

May 10
Claude responded: Honestly, I don't know off the top of my head — let me check rather than guess.
Honestly, I don't know off the top of my head — let me check rather than guess.


Searched the web
Searched the web
That search didn't return what I was hoping. Let me try more specific angles — disk-backed queues and the term "spool" specifically.


Searched the web
Searched the web
Good — there are real Rust crates for this exact pattern (yaque, disk-backed-queue). Let me check how widely-used these actually are and look for one or two real-world precedents in the systems-tools space for the directory-of-files pattern specifically.


Searched the web
Searched the web
OK, I have enough grounding to give you an honest answer. The picture is more nuanced than my doc 20 implied.

Honest answer: "spool directory" specifically isn't a common idiom in Rust. It's a real pattern but the term and the file-per-event shape are more common in older Unix tooling than in modern Rust services. Let me break it down.

Where the pattern is genuinely common:

The classical Unix "directory of files where each file is one queued item" pattern is widely used in older systems software — print spoolers, mail queues (/var/spool/mail, /var/spool/postfix), at(1)/cron jobs (/var/spool/cron). The word "spool" specifically comes from this lineage.

In modern observability tools, the concept is alive but the shape has shifted: instead of one file per event, you tend to see a small number of chunked/buffered files (Fluent Bit's storage.path writes chunks of multiple records; Vector's disk buffers do something similar). The reasons are practical — tens of thousands of tiny files cause filesystem and inode pressure, directory listing gets slow, FS watchers struggle.

Where Rust specifically lands:

Searching crates.io, the file-per-event shape isn't an idiomatic Rust pattern. What exists instead:

yaque — disk-backed SPSC queue using filesystem semantics, but stores items in chunked segment files, not file-per-item. Built on mio and notify. Modest adoption.
disk-backed-queue — uses SQLite as backing storage. Channel-shaped API.
Embedded KV stores like sled or redb for durable queue-like patterns.
Most Rust services that need this pattern either: write directly to SQLite (the most common choice), use a real broker (NATS, Redis), or use one of the above crates.
I haven't found a popular Rust crate that does exactly "directory full of one-file-per-event" with the conventions I described in doc 20 (sortable filename, O_CREAT | O_EXCL, inotify-based pickup). Plenty of Rust code does this when needed, but it's hand-rolled per project rather than a library pattern.

Outside Rust, in modern systems:

systemd journal uses a binary format, not a spool directory
Fluent Bit's filesystem buffer writes chunks containing many records
Postfix /var/spool/postfix/ still uses the directory pattern (one file per queued message) — but Postfix is an old daemon
Nginx, syslog-ng, etc. all use append-to-log files, not directory-of-events
The honest assessment of my doc 20 design:

The file-per-event spool I described would work, but it's not idiomatic Rust and has real practical issues I underplayed:

Inode pressure under load. A burst of 10,000 hook events creates 10,000 inodes. macOS APFS handles this fine; ext4 fine; but it's wasteful.
Directory listing is O(n). A daemon doing 30-second sweeps would scan the entire directory each time. After enough accumulation, this gets slow.
FS watchers (inotify, FSEvents) have known scaling cliffs. FSEvents in particular coalesces events under load and can miss things.
O_CREAT | O_EXCL collision retry is fine but adds a code path.
The chunked-file shape (yaque-style) is genuinely better for this kind of fallback queue — fewer files, segment rotation, simpler to reason about.
What I'd revise:

The hybrid design (UDS preferred, disk fallback) is still right — that part isn't controversial; the principle of "fast IPC path, durable fallback" is universal.

What should change is the fallback shape. Three options worth weighing:

Single append-only log file with rotation (spool.ndjson, rotated when it hits a size threshold). Daemon reads, tracks a byte offset, deletes on full consumption. Simplest. Multiple shim processes writing to the same file means writes need to be atomic (POSIX guarantees this for writes under PIPE_BUF, which is 4KB; hook payloads can exceed this).
yaque crate for the spool, since it solves exactly this and is built for the use case. Pulls in a dependency but it's a small one.
SQLite for the spool (the same SQLite the daemon already uses). Shim writes a row to a pending_events table; daemon polls or wakes on a sentinel. Eliminates the disk-spool concept entirely — the durable store is the spool. The shim now has a SQLite dependency, which is heavy for a hot-path binary; rusqlite + sqlite static is ~1MB. But the operational simplicity is high.
I think option 3 might actually be the right answer for our case, and I should have considered it in doc 20. The shim can open a SQLite connection in milliseconds, INSERT one row with WAL on, and exit. The daemon reads from the same database. There's no "transport" between them at all — the database is the transport.

This trades the UDS-preferred / spool-fallback split for a single uniform path: every event goes through SQLite. The daemon is just the long-running process that also reads from the same database and emits pub/sub events.

Trade-offs of the SQLite-as-IPC approach:

Pro: No transport code. No socket. No spool directory. The durable store is the only path.
Pro: Multi-writer safe with WAL mode. SQLite handles concurrency well for low-frequency writes.
Pro: Shim doesn't need to know whether the daemon is running.
Con: Shim binary size grows (rusqlite + sqlite-bundled adds ~1MB). Not catastrophic but real.
Con: SQLite open + WAL write is ~1-2ms; UDS write is ~0.5ms. Slower happy path.
Con: Daemon needs to poll for new rows or use a NOTIFY mechanism (SQLite doesn't have native pub/sub; would need polling, or a sentinel file with FS watch, or hooking on wal-index changes — none ideal).
The polling-for-new-rows issue is the real catch. The daemon polling every 100ms means new events have up to 100ms latency to live subscribers. That violates the "<100ms p95 hook→presenter" target from doc 12.

So the path of least resistance is probably still UDS + small fallback, but with a smarter fallback than file-per-event. Append-only NDJSON file with rotation is the version I'd actually recommend, or just using yaque if we want a maintained library.

Want me to revise doc 20 with one of these alternatives? Three concrete paths:

(a) Keep UDS primary but switch the fallback to append-only rotated NDJSON file (simpler implementation, common pattern)
(b) Keep UDS primary, use the yaque crate for the fallback queue (offloads the complexity to a maintained library; adds one dep)
(c) Reconsider the whole transport: SQLite-as-IPC with the shim writing directly to the daemon's database, eliminating the transport layer entirely (cleaner architecturally, slower hot path, polling-vs-notify is a real concern)
I'd lean (a) for MVP — it's the simplest thing that could work — and note (c) as worth revisiting once we have real numbers.

You said: What about writing directly to the same db the daemon is using?
What about writing directly to the same db the daemon is using?

May 10
Claude responded: You're picking up the option I waved at and then walked away from.
You're picking up the option I waved at and then walked away from. Let me think about it seriously rather than dismiss it on the polling concern, because that concern might be overblown.

The fundamental question: can the shim's hot path of "open SQLite, INSERT one row, close, exit" hit the 5ms budget? And can the daemon notice new rows fast enough to keep pub/sub latency under 100ms?

Let me work through this honestly.

The hot-path cost of SQLite write from a fresh process:

A cold-start binary doing sqlite3_open_v2() on a WAL-mode database, one INSERT into a pre-existing table, and exit:

Process startup (Rust binary, statically linked, no deps): ~0.5-1ms
sqlite3_open_v2() on an existing WAL database: ~0.5-1ms (opens the main db file + WAL file + shared memory mapping)
INSERT with prepared statement: ~0.1-0.3ms
WAL append (no fsync by default in NORMAL sync mode): ~0.1ms
Close + exit: ~0.2ms
Total: roughly 1.5-3ms in the happy case. That's actually within budget — comparable to the UDS path I described earlier.

But there are gotchas worth surfacing:

The WAL file shared-memory region (-shm) must already exist or the first writer has to create it. If the daemon hasn't run yet, the shim is the first writer and has to do this work, which adds ~1-2ms. Acceptable.
Writer contention. SQLite WAL allows one writer at a time. If Claude fires multiple hook events nearly simultaneously (e.g., parallel tool calls), the second shim invocation blocks on SQLITE_BUSY until the first commits. With WAL the busy window is very short (~0.5ms typically) but it's non-zero. Setting busy_timeout to ~50ms covers it.
The "daemon isn't running yet" case is actually easier than UDS+spool. With the shim writing directly to SQLite, there's no separate code path. The database file exists or it doesn't. If it doesn't, the shim creates it (with schema). The daemon, when it starts, opens the same file and finds the events. This is genuinely cleaner.
The first-ever-shim-invocation case. If ~/.claude-state-bus/events.db doesn't exist, the shim has to create it and run schema migrations. That's ~5-10ms — over budget on that one invocation. Fixable: ship the database file in the daemon's install step, so by the time the shim runs the file exists.
Disk durability. WAL with synchronous=NORMAL doesn't fsync on every commit; it fsyncs at WAL checkpoint time. This is actually what we want — fast writes, durability on the order of seconds, no per-event sync penalty.
The polling concern, examined more carefully:

I dismissed this too quickly last response. Let me look at the actual options for daemon-side notification of new rows:

Option 1: Polling. Daemon runs SELECT * FROM events WHERE event_id > ? ORDER BY event_id LIMIT 100 every N ms. Sets event_id watermark, emits new rows over pub/sub.

100ms polling = 100ms p95 hook→presenter latency. Right at the budget ceiling.
50ms polling = 50ms p95. Comfortable.
25ms polling = 25ms p95. Plenty of room.
Polling cost is trivial — SELECT against an indexed event_id > ? is sub-millisecond. Even at 25ms intervals, that's ~40 queries/second when idle, well under 1% CPU. Polling isn't the disaster I implied.

Option 2: SQLite's update_hook mechanism. The C API has sqlite3_update_hook() that fires a callback on every INSERT/UPDATE/DELETE in the current connection. But it doesn't fire for changes from other connections. So the daemon wouldn't see shim writes through this. Useless for our case.

Option 3: WAL frame watching. SQLite WAL writes append to events.db-wal. The daemon could notify/inotify watch this file and read new frames when it changes. But parsing WAL frames directly is fragile (it's an internal SQLite format). Not recommended.

Option 4: A "wakeup" sentinel. Shim writes a small sentinel file (~/.claude-state-bus/wake) after the INSERT. Daemon watches this file with notify. On modification, daemon queries for new rows. Combines durable INSERT with fast notification.

Latency: file modify → kernel notify → daemon wake → SELECT → emit. Maybe 5-10ms.
Shim does one extra write (touch the sentinel). ~0.2ms overhead.
Works even when daemon and shim are different processes.
Option 5: Hybrid — INSERT + UDS notification. Shim INSERTs, then tries a fast UDS send with the new event_id as payload (just an integer, "wake up and check for >=N"). If UDS fails, the daemon catches it on its next poll. This is the best of both worlds — fast happy path, durable always, polling fallback.

Happy path latency: UDS ping triggers immediate daemon read. Hook→presenter <10ms.
Daemon down: shim's UDS write fails (instant connection-refused), shim exits successfully (the INSERT already happened). When daemon starts, it reads everything since its last watermark.
No spool directory, no file-per-event, no fallback file format. Just SQLite + a notification channel.
The architectural shift this enables:

If we go with SQLite-as-the-only-write-path, the daemon's role becomes much cleaner:

The daemon is no longer the receiver of events. It's a reader of the event log that also emits pub/sub.
"Shim writes to SQLite" is the canonical write path. UDS is just notification.
The daemon can crash and restart freely — it just resumes reading from its last watermark.
Backfill is automatic — events that arrived while the daemon was down are just there in the database when it starts.
Multiple daemons could read the same database (read-only) for cases like "running a debug subscriber alongside the main one." Doesn't matter for MVP but doesn't preclude future stuff.
This is genuinely a simpler architecture. The "transport" was always somewhat artificial — we have a durable event log anyway; why introduce a separate emit-then-transport-then-store pipeline when the shim can just write to the store directly?

What I want to verify before committing:

A few things I should sanity-check rather than reason about in the abstract:

Cold-start SQLite open latency in Rust. Need to actually benchmark this. My estimates above might be off.
SQLITE_BUSY behavior under realistic burst load. What does it look like when 5 shim invocations fire within 10ms?
WAL file growth. If the daemon is down for a long time, the WAL grows. SQLite checkpoints normally happen during connections; with only shim writers (no long-running daemon connection), checkpoints might not happen. Need to either have the shim opportunistically checkpoint, or have the daemon do an aggressive checkpoint on startup.
macOS file locking semantics. SQLite WAL has some platform quirks; we should verify the shim+daemon scenario specifically on macOS APFS.
Let me check a few of these.


Searched the web
Searched the web
Good — the macOS fsync-patched-out detail is important; I should mention it. And the broader SQLite-in-production picture supports the approach. Let me also quickly check whether the SQLite-as-IPC pattern is something other tools do.


Searched the web
Searched the web
This is fantastic — the Overstory finding is the exact pattern we'd be building. Let me look at that more carefully.


Searched the web
Searched the web
This is a major find. Overstory does exactly what we're proposing — hooks write directly to SQLite, multiple processes (agents, watchdog, dashboard) access the same databases, WAL mode + busy_timeout=5000ms. They have 5 different SQLite databases that hook commands write to in-band with Claude's tool execution. And the architecture is described as "Your Claude Code session IS the orchestrator. There is no separate daemon" — they completely removed the daemon as a transport.

Key data points from Overstory's real deployment:

WAL mode + busy_timeout=5000 — this is the proven config for the pattern
"~1-5ms per query" — that's the actual measured cost they're hitting
"Hook commands (which run in-band with Claude Code's tool execution) have enough headroom to succeed without failing the hook" — they explicitly state this works for the hook latency budget
Prepared statements created once per process lifetime — important optimization
Multiple databases for separation of concerns (events.db, mail.db, sessions.db, metrics.db, merge-queue.db) — they don't try to put everything in one schema
OK, now I can give you a much more grounded answer.

Direct SQLite writes are not just viable — they're already proven in production for this exact pattern. Overstory is a 1600+ test, actively-developed multi-agent orchestration system using exactly this approach with Claude Code hooks, and they explicitly say the latency budget holds.

The revised architecture:

The shim opens the SQLite database directly. WAL mode + 5-second busy_timeout. INSERT one row. Close. Exit. No UDS, no spool directory, no separate transport at all. The database file is the transport.

Why this is genuinely better than what I sketched in doc 20:

One code path instead of two. No UDS happy-path code, no spool-fallback code, no daemon-side spool-watcher code, no startup-drain logic. Just "open, write, close."
Daemon down = non-event. The database is the source of truth. If the daemon isn't running, the events are still being recorded. When the daemon starts, it just reads from its watermark forward. No "drain the spool" step because there's no spool.
No transport-level loss scenarios. The shim writes to durable storage in one atomic operation. The only loss case is kernel-level (power loss before WAL fsync at checkpoint), which is the same risk the daemon would have anyway.
Simpler operational model. Multiple shims writing concurrently is just "WAL mode + busy_timeout." Multiple daemons reading is just "WAL mode permits concurrent readers." All the concurrency complexity is handled by SQLite, not by code we write.
It's already a known-good pattern. Overstory has run it through 1600+ tests across multiple Claude Code agents writing to multiple SQLite databases concurrently. The pattern is field-validated.
The latency picture:

Process startup (Rust static binary): ~0.5-1ms
SQLite open (existing WAL database): ~0.5-1ms
Prepared INSERT execution: ~0.1-0.3ms
WAL append (no fsync, synchronous=NORMAL): ~0.1ms
Close + exit: ~0.2ms
Total: roughly 1.5-3ms p50, probably 3-5ms p95. Within budget.

The daemon-notification question, revisited:

Now that we're not using SQLite for transport, just storage, the daemon's job is:

On startup: open the database, read from its last event_id watermark
On notification: re-query for new rows since watermark, emit on pub/sub
The notification channel can be lightweight. Options:

Option A: Pure polling. Daemon polls SELECT MAX(event_id) FROM events WHERE event_id > ? every 50ms. Trivial CPU cost. ~50ms p95 latency hook → presenter. Within the 100ms budget from doc 12.

Option B: SQLite WAL file watch. Daemon watches events.db-wal via notify crate. On file modification, query for new rows. Native FS notification = ~5-10ms latency. Free CPU when idle.

Option C: Sentinel file. Shim writes the row, then touch ~/.claude-state-bus/wake. Daemon watches that one file. Same latency profile as B but using a smaller, simpler file.

Option D: Hybrid. Daemon polls slowly (200ms) as a backstop, plus watches the WAL file for fast path. Misses on the FS watch get caught by the next poll.

I'd lean Option B for MVP — watching the WAL file is one line of notify configuration, the daemon already has to manage the database file, and it doesn't require the shim to do anything extra. Option D is the production-hardened version.

The pieces I should call out:

The shim needs to open SQLite, which adds binary size. rusqlite + bundled SQLite adds ~1MB to the shim binary. Doc 20 estimated the shim at ~500 lines of Rust; this might push it to ~700 lines plus the SQLite dependency. Not catastrophic — Overstory's shim equivalent (ov log) runs in Bun and is fine — but worth naming.
The shim and daemon must agree on schema. If the shim writes a row with columns the daemon doesn't expect, or vice versa, things break. Mitigation: schema migrations run by the daemon on startup; shim only does INSERT into a known stable subset of columns. The shim never CREATE TABLEs.
The shim opening SQLite on a database that doesn't exist yet. First-ever-shim-run case. Two options:
Shim creates the database with schema if it doesn't exist (~5-10ms one-time cost; acceptable since it's once per install).
Daemon's install command creates the database; shim refuses to write if file doesn't exist (cleaner, but means the install must run before any Claude session).
I'd pick the second — it's the responsibility of claude-state-bus install to ensure the schema is ready.
Checkpoint behavior. WAL files grow until checkpoint. With many writers and no long-running daemon connection, checkpoint might not happen. Overstory handles this with a long-running daemon that maintains a connection. We'd do the same — the daemon's existence ensures periodic checkpoints. If the daemon is down for a long time, the WAL grows; on daemon startup it does a PRAGMA wal_checkpoint(TRUNCATE).
macOS fsync caveat from the SQLite-in-production benchmark. Apple's bundled SQLite has fsync patched to a weaker version; if we want true durability, we ship our own SQLite (rusqlite-bundled does this for us). Doc 20's "no fsync on the spool path" trade-off becomes moot — we just use SQLite's durability model and it's correct.
What this means for documentation:

Doc 20 ("shim-to-daemon transport") becomes less correct. The replacement framing is: "the database is the substrate; the shim writes, the daemon reads, pub/sub is layered on top of the read path." That's actually the design we converged on in doc 11 but I muddled it in doc 20 by introducing transport-as-separate-concept.

Want me to draft a doc 21 that revises the transport story with this approach? It would:

Open with the architectural shift (no transport — the DB is the bus)
Lay out the shim's actual operation (open, INSERT, close)
Specify the daemon's notification mechanism (WAL file watch + polling backstop)
Cite Overstory as field-validated precedent for the pattern
Address the concrete concerns (binary size, schema management, checkpoint behavior, macOS fsync)
Note what doc 20 got wrong and supersede it
Honestly, I think this is the right answer and we should write it up. The Overstory precedent is the kind of evidence I should have looked for before writing doc 20 — a real production system doing exactly this, with measured latency, gives much more confidence than reasoning about UDS connect times in the abstract.

Three things worth flagging as actual unknowns we'd want to verify before committing:

Cold-start SQLite open time on macOS specifically (not Linux benchmark numbers, since macOS APFS has different file locking semantics)
Behavior under burst: 5+ hook invocations within 10ms (parallel tool calls from Claude — does busy_timeout=5000 hold the latency budget?)
WAL file size limits in practice when daemon is down for hours (Overstory's pattern of a long-running daemon handling checkpoints needs to be replicated)
Want the doc 21 revision?





Claude is AI and can make mistakes. Please double-check cited sources.
Chats using advanced research can't be shared

