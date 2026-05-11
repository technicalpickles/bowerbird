# Design vs. inventory: how well does the sketch fit?

A walkthrough comparing `03-design-sketch.md` against each tool in `02-detailed-inventory.md`. For each tool, two questions:

1. **Could this be rebuilt as a thin presenter on the daemon?**
2. **What does it currently do that the design doesn't capture, or captures worse?**

The goal is to surface places where the abstraction is wrong, missing pieces, or wishful — before we commit to it.

---

## OpenPets

**As a presenter, would it work?** Yes, very cleanly. The design's reaction enum *is* OpenPets's reaction enum (deliberately — that's where it came from). OpenPets becomes: subscribe to `WS /sessions/:id/events`, filter for state transitions, set the pet reaction. ~50 lines.

**What it does that the design doesn't capture:**

- **Pet ownership / multiplexing.** OpenPets has the lease mechanism so that "this Claude Code process owns this specific pet window." The daemon's state model says "session X has state Y" — it has no opinion about which window/output renders that state. This is the right call (presenters multiplex), but it means OpenPets keeps its own lease layer on top. The lease is no longer about session liveness (the daemon handles that) but about output binding.

- **Speech / "say" messages.** OpenPets carries arbitrary short text bubbles ("Done!", "Let me check"). These are *agent intent* messages, not state transitions. The design has no event kind for this. Two options: (a) add `kind: 'message'` to the event vocabulary, (b) say it's out of scope and OpenPets keeps its MCP `pet.say` path independent of the daemon. Probably (b), but worth being explicit. The daemon is for *state*, not for *expression*.

- **Speech validation rules.** OpenPets's `validateHookSpeech` rejects URLs, paths, secrets, code-like syntax. That's presenter-side hardening. Stays in OpenPets.

**Net assessment:** Excellent fit. OpenPets becomes ~50 lines of presenter plus its existing pet-window IPC. The lease/say/validate logic stays where it is.

---

## Pixel Agents

**As a presenter, would it work?** Mostly yes, but with caveats. The design adopted Pixel Agents's `AgentEvent` shape almost verbatim, so the event vocabulary maps. The state model maps. The hook router replaces Pixel Agents's HTTP server.

**What it does that the design doesn't capture:**

- **Terminal binding.** Pixel Agents knows "this VS Code terminal runs this Claude Code session" and removes the agent on `vscode.window.onDidCloseTerminal`. The daemon doesn't know about VS Code terminals. The terminal→session binding stays Pixel Agents's job. The session→state binding moves to the daemon. Cleaner separation, actually — but Pixel Agents still owns half the picture.

- **Per-tool detailed status strings.** `formatToolStatus("Edit", { file_path: "auth.ts" })` → "Editing auth.ts". The design carries `tool_name` and `input` in events but doesn't compute the human-readable string. Each presenter reimplements `formatToolStatus`. **This is a real duplication**: ccam has its own version, Pixel Agents has its own, openpets has its own. Worth either:
  - Shipping a shared `format-tool-status` package alongside the daemon
  - Having the daemon compute and cache a default formatted version per `toolStart` event, accessible via the read API
  - Both — daemon emits a default, presenters can override

- **`permissionExemptTools` / `subagentToolNames` knowledge.** Pixel Agents knows that `Task`/`Agent` spawn subagents and that `AskUserQuestion` doesn't trigger permission timers. In the unified design, the daemon's adapter for Claude Code knows these mappings — the daemon emits `subagentStart` events directly when those tools are seen. So Pixel Agents doesn't need this list anymore. **The design is actually a strict improvement here.**

- **JSONL polling for token usage.** Hooks don't carry token counts; statusline does. The design's heartbeat-via-statusline picks up tokens for free. **Improvement over Pixel Agents's current dual-source approach.**

- **Multi-window cooperation logic.** Pixel Agents has a `server.json` PID dance so a second VS Code window doesn't start a second server. With the daemon, there's already only one. **The design eliminates this entirely.**

**Net assessment:** Strong fit, and the design eliminates several pieces of complexity Pixel Agents currently carries. The two real costs are (a) Pixel Agents has to refactor to be a consumer rather than a server, and (b) `formatToolStatus` duplication needs a story.

---

## disler/claude-code-hooks-multi-agent-observability

**As a presenter, would it work?** Yes, almost trivially. disler's UI is a pure event log viewer. It becomes: `GET /events?since=` for backfill, `WS /events` for live tail.

**What it does that the design doesn't capture:**

- **HITL backflow.** disler has a `humanInTheLoop` field on `HookEvent`. The dashboard surfaces a question, and the user's response posts back to a WebSocket URL — and on the agent side, the hook is presumably blocking on that response. **The design is one-way.** Consumers read events; events don't flow consumer→agent. Adding HITL would require:
  - A bidirectional channel (the hook script blocks on a response from the daemon, daemon waits for a consumer's reply)
  - State for "this event is awaiting a response"
  - Auth model for who can answer

  This is genuinely a different abstraction. Either the design adds it as a clear extension surface, or HITL stays as a separate per-tool thing on top.

- **AI summarization in the hook.** disler runs an Anthropic call inside `send_event.py` to add a `summary` field. That's a hook-side enrichment. In the unified design, the hook is a thin shim — adding LLM calls there would slow it down past the failsafe budget. Better path: presenters can subscribe and write summaries back into the event log as a separate `summary` event, or maintain their own enrichment store. The daemon keeps the hook fast.

- **`source_app` namespace.** disler scopes by `source_app` so multiple projects's hooks share one server. The design uses `session_id` and `project_dir` directly. Different mental model — disler is "lots of projects emitting to one observer," the design is "one machine has these sessions running." For single-developer use the design is fine; for team/cluster deployments, the disler model is actually more useful.

**Net assessment:** Excellent fit for the read path. The HITL extension is the real gap — it's a feature the design doesn't accommodate, and adding it changes the daemon from a one-way bus to a full RPC system. Worth deciding whether HITL is in scope.

---

## agents-observe (simple10)

**As a presenter, would it work?** Yes, same shape as disler. Pure event log, becomes a consumer.

**What it does that the design doesn't capture:**

- **Project auto-detection** by walking back from `transcript_path` to find a sibling session with a project ID. The design has `project_dir` directly; auto-detection isn't needed because hooks carry `cwd`. **Improvement.**

- **Bash hook wrapper for speed.** agents-observe's bash wrapper exists because the Node CLI is too slow. The unified design's shim should be either Rust/Go or a tiny Node script that just forwards to a Unix socket — same backgrounding pattern, but the wrapper is shipped with the daemon. **Improvement, with the caveat that the daemon needs to ship a fast shim.**

**Net assessment:** Excellent fit. Same as disler structurally.

---

## ccam (hoangsonww)

**As a presenter, would it work?** Most of ccam's *backend* would be replaced by the daemon. ccam-the-product becomes a sophisticated read-side: kanban board, run controls, plugin integrations.

**What it does that the design doesn't capture:**

- **Agent control plane.** ccam has a "Run Claude" feature where it spawns `claude` itself with specific flags. That's an *agent control plane*, not state observability. The design says nothing about spawning. Two paths: (a) ccam keeps its spawner and the daemon picks up state from the spawned process via hooks, or (b) the daemon adds a control plane (`POST /sessions { project_dir, model }` → spawns claude). (a) is cleaner — the daemon stays focused on state.

- **Plugin marketplace.** ccam ships plugins (`ccam-analytics`, `ccam-productivity`, `ccam-insights`) as Claude Code plugins. These are *Claude Code-side extensions* (slash commands, hooks, MCP) that emit events ccam consumes. In the unified design, those plugins emit to the daemon, and ccam reads from the daemon. The plugins remain ccam's; the data plumbing is shared.

- **Session reactivation logic.** ccam has nontrivial logic for "when does a Stop event reactivate a completed session?" (only if the session wasn't in error). This is an interesting case: the daemon's projection logic needs to be at least as sophisticated, or consumers like ccam will reimplement it. **Worth porting ccam's reactivation rules into the daemon's projection.**

- **`WAITING_INPUT_PATTERN` regex** for parsing Claude Code Notification messages. This is dialect-specific text-matching — "permission" / "waiting for your input" / etc. The daemon's Claude adapter needs to do this parsing. ccam already has good rules; should be lifted directly.

**Net assessment:** ccam-as-consumer is feasible but it's a real refactor — the most complex of any tool surveyed. ccam currently *is* a state engine + a UI; only the UI half remains. The session reactivation rules and notification parsing should move *into* the daemon, not get reimplemented in each consumer.

---

## ccpet

**As a presenter, would it work?** Yes, perfectly. ccpet is a statusline-only token-energy tracker. It becomes a registered statusline segment provider that reads tokens from the daemon's session state.

**What it does that the design doesn't capture:**

- **Pet-specific persistent state** (pet UUID, name, animal type, lifetime tokens, graveyard). This is *presenter state*, not session state. ccpet keeps its own JSON file. **The design should be explicit that presenter persistence is the presenter's responsibility** — the daemon doesn't store "Josh's pet is named Luna."

- **Global leaderboard.** ccpet uploads to `ccpet.surge.sh`. That's a per-tool feature, totally orthogonal.

**Net assessment:** Trivial fit. ccpet becomes ~30 lines of statusline segment provider plus its existing pet-state file.

---

## claude-code-tamagotchi

**As a presenter, would it work?** Yes for the pet/statusline parts. Mostly yes for the violation system.

**What it does that the design doesn't capture:**

- **Pet-specific persistent state** — same as ccpet, presenter responsibility.

- **Violation detection (LLM-as-classifier).** Tamagotchi reads transcripts and asks Groq "is Claude doing what the user asked?" That's enrichment, presenter-side. Lives in tamagotchi.

- **PreToolUse blocking.** Tamagotchi can *block* tool calls when violations are detected (the hook returns non-zero). The unified hook router exits 0 unconditionally as a failsafe, which means **the daemon-routed model can't directly support tool blocking**. To preserve this:
  - Either tamagotchi installs its own `PreToolUse` hook *in addition* to the unified one (collision returns)
  - Or the daemon supports synchronous subscribers that can veto, with strict timeout (defeats the failsafe)
  - Or tool blocking is declared out of scope for the unified design

  This is a real tension. The daemon's failsafe-always-exit-0 makes it strictly observability. Anything that needs to *intervene* in agent behavior has to bypass it. **Probably the right answer is: the daemon is observability, intervention is a separate concern, intervention tools install their own hooks.** Worth saying so.

**Net assessment:** Pet portion is trivial. Violation system stays separate because of the blocking requirement. The design needs to be explicit that it's read-only / observability-only, not a control plane.

---

## claude-team-dashboard

**As a presenter, would it work?** Partially. The team dashboard reads `~/.claude/teams/*` filesystem state directly — config files, inbox files. The design doesn't currently watch that directory.

**What it does that the design doesn't capture:**

- **Agent-to-agent messages** (Agent Teams inboxes). These are messages *between agents* in a team, surfaced as a D3 communication graph. **There is no `agentMessage` event kind** in the current vocabulary. This is a real omission. Adding it:
  ```
  agentMessage  { from_agent_id, to_agent_id, message_summary?, tool_id? }
  ```
  And the daemon's Claude adapter would emit these from the `SendMessage` tool's PreToolUse + the inbox file watcher.

- **Team configuration as state.** Teams have members, roles, task assignments. Not currently in the data model. Could add a `team` projection or treat teams as just another agent grouping.

- **Filesystem-watch as a fifth ingest source.** The design lists hook / jsonl / mcp / statusline / sweep. The team dashboard adds **directory watching** — a sixth. It could fold into the sweep ("sweep also watches `~/.claude/teams/`") or be its own ingest.

**Net assessment:** Reasonable fit if we add `agentMessage` to the vocabulary and a teams adapter to the daemon. Otherwise team-dashboard stays parallel.

---

## tmux-agent-sidebar (hiroppy)

**As a consumer or as a piece to absorb?** Both. tmux-agent-sidebar is structurally the closest existing tool to the design. Its `AgentEvent` enum is the richest (16 variants). Its adapter abstraction (`HookRegistration` table with bidirectional drift tests) is the most disciplined. Its process-tree scanning is the *only* implementation of the session-vs-process gap.

**What it does that the design strictly improves:**

- **Tool storage in tmux pane options.** This is a specific solution to "let other tools read state without integrating with our API" via tmux's existing variable system. The unified design solves the same problem with a documented HTTP/WS API. Different mechanism, similar goal — but the daemon's API works for non-tmux consumers too.
- **Per-tool hook installation.** tmux-agent-sidebar installs 16 hooks; the daemon needs only the unified router shim per event type.

**What it does that the design currently doesn't:**

- **The richer event union.** Worktree events (`WorktreeCreate`, `WorktreeRemove`), task events (`TaskCreated`, `TaskCompleted`), `CwdChanged`, `PermissionDenied`, `StopFailure`, `TeammateIdle`, `WorktreeInfo` as event metadata. **Almost all of these should be added to the design's vocabulary.** The current design's `AgentEvent` is closer to Pixel Agents's, which doesn't model worktrees or task lifecycle. Worktrees especially are first-class user concerns.

- **Drift-free hook registration tables.** The compile-time enum + bidirectional table-vs-parser test is a good pattern that the daemon should adopt for its adapters.

- **Process-tree walking as ground truth for liveness.** The design has this as a sweep ingest source. Should explicitly cite tmux-agent-sidebar's approach.

- **Idle-state metadata.** `meta_only` flag on Notification events distinguishes "this notification carries metadata but doesn't change visible state" from "this is a real status change." The daemon should emit similar markers.

**As a refactor target:**

If the daemon existed and tmux-agent-sidebar adopted it, the daemon takes:
- Hook routing
- Process-tree scanning (the daemon does it once, tmux-agent-sidebar reads results)
- Event log persistence
- Multi-window cooperation

tmux-agent-sidebar keeps:
- The tmux UI rendering
- tmux pane option publishing (for other tmux-aware tools)
- The pet animation (it has one too)
- Worktree spawning UI

That's a clean split. The daemon's biggest borrow from tmux-agent-sidebar is the event vocabulary and the adapter discipline.

**Net assessment:** Strong fit, with the design needing to absorb tmux-agent-sidebar's richer event vocabulary. The bidirectional adapter-table-vs-parser pattern is worth adopting wholesale.

---

## opensessions (Ataraxy-Labs)

**As a foundation, would it work?** Possibly more than as a presenter — opensessions might be the **starting point** for the daemon, with extensions, rather than something built from scratch.

**What it already has:**
- A formal `AgentEvent` shape and `AgentStatus` enum (6 values)
- A documented `AgentWatcher` extension interface (`CONTRACTS.md`)
- A capability-based mux abstraction (`MuxProviderV1` + `WindowCapable` + `SidebarCapable`)
- An HTTP API (`POST /api/agent-event`) for any tool to push events
- WebSocket broadcast to subscribers (`server.publish("sidebar", ...)`)
- Per-thread instance tracking (a session can host multiple threads)
- TTL-based pruning (3 min for `running`, 5 min for terminal states)
- Built-in watchers for four agents (Amp, Claude Code, Codex, OpenCode)
- Session resolution via project-dir matching with parent-prefix fallback
- A `markPluginOwned` mechanism for plugin/watcher coexistence on the same thread

**What the design adds that opensessions lacks:**

1. **Persistent event log.** opensessions's tracker is in-memory only — events are projected to current state and pruned. No historical query API. **The biggest missing piece.** SQLite append-only events table.

2. **Hook-router as a named ingest source.** opensessions deliberately avoids hooks (its README says: "hook-based... fragile process polling"). The daemon should accept hooks too — they're authoritative for tool-call boundaries that file polling can't match. Adding them shouldn't displace the file-tail watchers.

3. **Process-tree scanning** (steal from tmux-agent-sidebar).

4. **Statusline shim composition** for pets and HUDs that want to render to the agent's own statusline.

5. **The 11-value reaction enum** as an external projection (opensessions has 6).

**As a refactor target:**

opensessions becomes the daemon. Adds:
- An events table + cursor-based query/subscribe API
- A hook-router shim and entries in `~/.claude/settings.json`
- A process scanner module
- A statusline composition layer
- The OpenPets reaction enum as a downsampler over `AgentStatus`

The watchers, mux abstraction, HTTP API, sidebar UI flow all stay.

**Net assessment:** opensessions is the most mature realization of the design's architecture today. **Building on opensessions probably saves ~70% of the work** vs. starting from scratch. The remaining 30% is the event log + hook router + process scanner + statusline composition.

---

## tmux-agent-status (samleeney)

**As a presenter, would it work?** Trivially. ~30 lines of code subscribing to session state and rendering three values into a tmux sidebar.

**What it does that's worth absorbing:**

- **File-drop integration protocol.** Any agent can integrate by writing `working`, `done`, or `wait` to a file path. **This is the lowest-friction ingest source possible.** No HTTP, no MCP, no daemon connection — just a file. The daemon should support a file-drop directory (`~/.config/claude-state-bus/inbox/<session_id>.json`?) for tools that don't want to write code.

- **Per-pane vs. session reduction.** Session status is computed from per-pane status via a small reducer. Generalizes to: a session can have multiple "instances" (panes, windows, processes), and the session-level state is a function of instance states. This matches opensessions's per-thread instance tracking and the design's attachment model.

**Net assessment:** Trivial presenter. The file-drop protocol is a good idea worth lifting into the design as a seventh ingest source.

---

## cmux family (manaflow + craigsc)

**As a consumer, would it work?** Marginally. cmux is fundamentally a terminal/workspace organizer, not a state observer. It has notification surfacing where it could benefit from "agent entered `waiting` state" subscriptions.

**What it does that the design doesn't capture:**

- **Process and tab management.** cmux's primary value is workspace structure — vertical tabs, splits, embedded browser. None of that is state-related.

- **OSC escape sequence handling.** cmux reads OSC 9/99/777 from terminal output. That's a *different* heartbeat-like signal: terminal emulators surface notifications via in-band escape codes. Could be a seventh ingest source if we wanted. Probably not worth it; cmux can listen on its own and post to the daemon if it wants its notifications visible to other tools.

**Net assessment:** Mostly orthogonal. cmux could become a *publisher* (it knows things about terminal state the daemon doesn't) and a light *consumer* (subscribing to waiting-state notifications), but it's not a presenter.

---

## Vibe Kanban / Conductor / Crystal / Opcode (orchestrators)

**As consumers, would they work?** Yes, lightly. They subscribe for "session done?" / "ok or error?" / "waiting for input?" The heavy lifting (worktree creation, Git operations, PR creation) is theirs.

**What they do that the design doesn't capture:**

- **Spawning agents.** All of these orchestrators *create* sessions. The design doesn't include a control plane. Same answer as ccam: spawning lives in the orchestrator, the daemon picks up state through hooks once the session starts.

- **Cross-session task coordination.** Vibe Kanban sees a kanban card move and spawns an agent against it. That's task→session correlation. Could be modeled in the daemon as session metadata (`metadata.task_id`), but that's optional.

**Net assessment:** Light consumers. The orchestration layer stays where it is.

---

## Claude HUD and statusline-only tools

**As a presenter, would it work?** Trivially. Statusline segment provider, ~30 lines.

---

## Dynatrace / OpenTelemetry path

**As an ingest source, would it work?** Yes. OTel is a strict superset of the hook event types — Claude Code emits OTel directly. The daemon could ingest OTel as a fifth source. **Worth adding to the design.**

**As a consumer:** If you're using Dynatrace for enterprise observability, you don't need the daemon. Different audience.

---

## Anthropic /buddy and Managed Agents

**Out of scope.** Buddy is in-process (it knows things hooks don't). Managed Agents is a hosted control plane. Neither maps to the daemon model.

---

## Cross-cutting findings

### The design captures well

- **Hook collision** → single router. Confirmed for all 8 hook-using tools.
- **Statusline collision** → segment composition. Confirmed for ccpet, tamagotchi, Claude HUD.
- **Session vs. process** → lease/heartbeat/sweep. Resolves the gap identified in `02-detailed-inventory.md`.
- **State machine reimplementation** → one projection. Confirmed for ccam, Pixel Agents.
- **Reaction enum as cheap projection** → confirmed for OpenPets, ccpet, tamagotchi, statusline tools.
- **Event log + state view duality** → confirmed for disler, agents-observe (consume the log), ccam (consume the projection).
- **Terminal / multiplexer attribution** → captured at attachment time, gives presenters click-through ("open in iTerm") and grouping ("3 sessions in tmux:dev"). **No surveyed tool currently does this** — Pixel Agents has the closest equivalent only because it's a VS Code extension and gets the API for free. Adding this as a daemon-level capability is a strict new feature.
- **Worktree / repo / branch grouping** → derived once at attachment open via a single `git rev-parse` call, plus `worktreeCreate`/`worktreeRemove` events from orchestrators. **No observability tool surveyed currently tracks this** — every state tool keys on `cwd` alone, so worktrees on the same repo look like unrelated sessions. tmux-agent-sidebar is the only surveyed tool with first-class worktree awareness, and only because it also creates them. Adding it to the unified design is a strict new capability for observability tools.
- **`agentMessage` events** for Agent Teams inboxes — added to the vocabulary so claude-team-dashboard can be a clean consumer.

### The design is missing

These came out of the walkthrough as real gaps:

1. **`tokens` and `cost` events as first-class.** The sketch lists them but they're worth being explicit because they're not hook-derived; they come from JSONL or statusline. **Already in the sketch but worth highlighting.**

2. **Default formatted tool status strings.** Every presenter reimplements `formatToolStatus`. **The daemon should compute a default and consumers can override.** Otherwise we ship the same logic three more times.

3. **OTel as a fifth ingest source.** Currently only listed as an alternative to the daemon. **Should be a recognized ingest path.**

4. **Explicit out-of-scope statements.** The design implicitly assumes:
   - Read-only / observability-only (no tool blocking, no HITL backflow, no agent control plane)
   - Single-machine local deployment (multi-host is a future extension)
   - Presenter persistence is presenter responsibility
   
   These need to be **explicit** in the design doc, because tools like tamagotchi (blocking), disler (HITL), ccam (control plane), and Marc Nuri's dashboard (multi-host) all exceed those bounds. Saying "this isn't that" up front avoids confusion.

5. **`message` / speech event kind** (OpenPets, Pixel Agents bubbles). Harder call — these are agent expression, not state. Probably out of scope, but should be explicit.

6. **HITL extension surface.** disler's HITL is real and valuable. The current design can't accommodate it. Three options:
   - Out of scope (disler stays separate)
   - Extension point: a "blocking subscriber" tier with a strict 1-second timeout, separate from observability subscribers
   - Full bidirectional RPC (much bigger scope)
   
   Worth picking one explicitly.

### Tools that do not fit the design and shouldn't

- **cmux family** — terminal organizer, mostly orthogonal
- **/buddy** — first-party, in-process
- **Managed Agents** — hosted control plane
- **Orchestrators** (Vibe Kanban etc.) — spawn agents, control plane

These are fine; not every tool should be a presenter.

### Tools that fit the design with refactoring

- **OpenPets** — ~50 lines presenter + existing IPC
- **ccpet, tamagotchi (pet half), Claude HUD** — trivial statusline segments
- **disler, agents-observe** — pure event log readers

### Tools where the design is a strict improvement

- **Pixel Agents** — eliminates multi-window cooperation, JSONL token polling, permission/idle timers, hook installation logic. The provider abstraction was already there; the design just lifts it to be shared.
- **ccam** — the session reactivation logic + Notification regex parsing belong *in* the daemon, not reimplemented per tool.

### Tools that exceed the design's scope

- **tamagotchi violation blocking** — needs synchronous PreToolUse, daemon is async-only
- **disler HITL** — needs bidirectional flow, daemon is one-way
- **ccam Run Claude / orchestrators** — control plane, daemon is observability-only

These tools either keep their non-observability features parallel to the daemon, or the daemon grows to accommodate them. The minimum viable daemon doesn't need to.

---

## Updates to make to the design sketch

Concrete changes to `03-design-sketch.md` based on this walkthrough:

1. Add `agentMessage` to the event vocabulary (for Agent Teams)
2. Add a section on `formatToolStatus` — daemon computes default, consumers override
3. Add OTel as a fifth ingest source explicitly
4. Add an explicit "out of scope" section listing:
   - Tool blocking / synchronous interception
   - HITL / agent backflow
   - Agent spawning / control plane
   - Presenter-side persistent state
   - Multi-host
5. In open questions, sharpen the HITL question — it's not "should we add it" but "is this an extension surface or out of scope"
6. Note that single-developer (one project, one machine, no `source_app` namespace) is the assumed deployment, and team deployments are a future extension

---

## Conclusion

The design fits the inventory better than I expected, and the places where it doesn't fit are mostly tools that genuinely exceed observability scope (blocking, HITL, control plane). The core claim — that 80% of these tools could be thin presenters on a shared state + event bus — holds up.

The two real missing pieces are:
- `agentMessage` for team inboxes
- An explicit position on HITL / blocking / control plane (in scope or not)

Both are addressable. Worth doing a v2 of the design sketch that incorporates these, then thinking about a minimum viable prototype.
