# Missing concepts: gaps surfaced by the second survey

The novelty/desktop-visibility survey (`05-novelty-survey.md`) covers ~30 tools that weren't in the original priority list. Walking each one against `03-design-sketch.md` surfaces nine gaps the daemon's data model, API, or topology doesn't currently cover. They're listed roughly in order of how universally they hit, so the first few are arguably required and the last few are nice-to-have.

The point of this doc isn't to add everything to the design. It's to be honest about which presenter classes the current design serves cleanly, which it serves with a stretch, and which it can't serve at all without changes.

---

## 1. Agent persona metadata

**Tools that need it:** Outworked (named pixel employees per agent), Pixel Office (boss + employees), paulrobello/claude-office, Claude Quest (character class), PAI per-agent voice mapping (Bella/Domi/Antoni for researcher/engineer/architect), AgentDeck (creature sprites differ per agent), bells-and-whistles voice slots, etr's themed voice packs.

**What's missing:** the daemon's `agents` table has `agent_id`, `type`, and `parent_agent_id`. There's no display name, no role/class, no avatar key. Pixel Agents had `subagent_type` for exactly this; the design dropped it.

**Why it matters:** these tools all map *something* (sprite, voice, color, room) to *something else* (the agent). The mapping needs a stable key, and `agent_id` rotates per run. The right key is the **subagent_type** (or for the main agent, just "main") — which is what Claude Code itself uses when registering teammate/subagent definitions.

**Proposed addition:**

```sql
ALTER TABLE agents ADD COLUMN subagent_type TEXT;     -- 'researcher', 'engineer', 'main', 'general-purpose', etc.
ALTER TABLE agents ADD COLUMN display_name TEXT;      -- human-readable; falls back to subagent_type
ALTER TABLE agents ADD COLUMN description TEXT;       -- one-line role description if known
```

`subagent_type` comes from the Claude Code subagent definition (which the hook payload carries). `display_name` is either the same or a presenter override. Presenters key their voice/sprite/color maps on `subagent_type`, not `agent_id`.

This unblocks PAI-style per-role voice maps, Outworked-style per-role sprites, and AgentDeck-style per-agent creatures.

---

## 2. Per-model cost and token rollups

**Tools that need it:** claude-receipts (thermal printer, per-model line items at SessionEnd), Discord Rich Presence (cc-discord-presence renders Dec 2025 API rates per model), ccusage-style menu bars (ClaudeBar, claude-monitor, ccseva all show per-model breakdowns), claude-status, Apple Watch "Usage for Claude" iOS app.

**What's missing:** the design's `tokens` and `cost` events are per-event. There's no roll-up by model, no per-session lifetime totals, no "how much has this session cost so far in Sonnet vs Haiku" breakdown.

**Proposed addition:** a derived projection alongside `sessions`:

```sql
CREATE TABLE session_usage (
  session_id      TEXT NOT NULL REFERENCES sessions(session_id),
  model           TEXT NOT NULL,
  input_tokens    INTEGER NOT NULL DEFAULT 0,
  output_tokens   INTEGER NOT NULL DEFAULT 0,
  cache_read      INTEGER NOT NULL DEFAULT 0,
  cache_creation  INTEGER NOT NULL DEFAULT 0,
  cost_usd        REAL    NOT NULL DEFAULT 0,
  PRIMARY KEY (session_id, model)
);
```

Updated by folding `tokens` and `cost` events. Read API exposes:

```
GET /sessions/:id/usage    -> [{ model, input, output, cache, cost_usd }, ...]
```

This is a strict superset of what every receipt/usage tool computes today. Each one currently re-derives it from JSONL or `ccusage` output.

---

## 3. Aggregated per-session metrics

**Tools that need it:** ccpet (energy from tokens, decay from time), Tamagotchi (mood from activity rate), /buddy (deterministic from user, but still wants session-tick counters), idle-game prototypes, the Pixoo64 family dashboards.

**What's missing:** pet-style tools want simple counters: total tokens this session, total tool calls, time since last user input, sessions today. The event log has all this latent, but a pet should not be running aggregations across an event firehose every statusline tick.

**Proposed addition:** roll the counters into the session row directly, updated incrementally:

```sql
ALTER TABLE sessions ADD COLUMN total_tokens         INTEGER NOT NULL DEFAULT 0;
ALTER TABLE sessions ADD COLUMN total_tool_calls     INTEGER NOT NULL DEFAULT 0;
ALTER TABLE sessions ADD COLUMN total_user_turns     INTEGER NOT NULL DEFAULT 0;
ALTER TABLE sessions ADD COLUMN last_user_turn_at    INTEGER;
```

Plus a daemon-level aggregate the pet can query without scanning:

```
GET /stats/today            -> { sessions, total_tokens, total_cost_usd, models_used: [...] }
GET /stats/lifetime         -> { sessions, total_tokens, total_cost_usd, first_seen_at }
```

Pets ask one cheap question per tick. They don't need the event firehose.

---

## 4. Context window utilization

**Tools that need it:** Claude Quest (200K-token context bar drains like an HP bar), claude-status (gauge in menu bar), CHUD floating overlay, Pixoo64 dashboards (rate-limit gauge), claude-monitor (rate-limit prediction), claude-pulse (predictive burn-rate alerts), the leeguooooo/claude-code-usage-bar (5h/7d countdowns).

**What's missing:** the statusline payload includes `contextLength`, `contextPercentage`, `contextPercentageUsable`. Hooks don't carry this. The daemon's design doesn't currently surface it.

**Proposed addition:** include context fields in the `tokens` event when the source is statusline (the only source that has it), and roll them into the session row:

```sql
ALTER TABLE sessions ADD COLUMN context_length            INTEGER;
ALTER TABLE sessions ADD COLUMN context_percentage        REAL;
ALTER TABLE sessions ADD COLUMN context_percentage_usable REAL;
```

Plus rate-limit fields when available (5h/7d quota windows from Anthropic's response headers). claude-monitor and claude-pulse already poll for these; the daemon could be the canonical aggregator.

---

## 5. Permission request payload

**Tools that need it:** m5-paper-buddy (renders AskUserQuestion option cards on e-ink with hardware approval buttons), claude-watch (renders option list on Apple Watch), Redlight Greenlight for Claude Code (floating overlay with Opt+Return / Opt+Esc to approve/reject), AgentDeck D200H 14-key HID + LCD, claude-buddy-pico (two physical buttons on Pimoroni display).

**What's missing:** the design's `permissionRequest` event signals only that a permission is being requested. It carries no payload. Hardware/wearable approval surfaces need:

- the question text (what is being asked)
- the option list (for AskUserQuestion: choices the user can pick)
- the tool name and tool input (for tool-use permissions)
- harm-potential signal if available (dmux/Outworked precompute this with an LLM)

**Proposed addition:** the `permissionRequest` event carries a structured payload:

```ts
permissionRequest {
  request_id: string,                        // for matching the response
  kind: "tool_use" | "ask_user_question",
  tool_name?: string,                        // for tool_use
  tool_input?: any,                          // for tool_use; redactable
  question?: string,                         // for ask_user_question
  options?: { id: string, label: string, description?: string }[],
  potential_harm?: { has_risk: boolean, description?: string }
}
```

The hook payload already carries most of this; the daemon's adapter just needs to pass it through. With `request_id` an answer can later be correlated, even if the daemon stays one-way (see #6).

---

## 6. HITL backflow — promote from "out of scope" to "extension surface"

**Tools that need it:** m5-paper-buddy, AgentDeck, claude-watch, Redlight Greenlight, terminaldeck, Bobby-Gray cinematic display (TV-side approval), Cardputer pager mode, disler observability HITL.

**Current design:** `04-design-vs-inventory.md` lists HITL as out of scope.

**Why this needs revisiting:** the current design treats HITL as a single feature disler asked for. The novelty survey shows **at least seven independently developed tools** want approval-from-elsewhere. It's not one feature; it's a category. The pattern is consistent:

1. Agent fires a permission request hook (which **blocks** by design — the hook script doesn't exit until the user decides)
2. Some surface (watch, lamp, deck button, e-ink button) renders the question
3. The user answers on that surface
4. The agent is unblocked

The daemon doesn't have to do the blocking itself, but if it doesn't, every HITL tool **has to install its own blocking PreToolUse hook**, which puts us right back in the hook-collision problem the daemon was supposed to solve.

**Proposed approach:** define a *single* extension surface — a "permission decision" channel — that the unified hook can route through:

- The unified hook script blocks waiting for either (a) an inline timeout (default: pass-through), or (b) a decision posted by an authorized subscriber via `POST /permissions/:request_id { decision: "approve" | "reject" }`
- At most one subscriber can be the "authoritative answerer" for a given session (registered explicitly: "I will answer permission requests for session X")
- If no authoritative answerer is registered, the hook exits 0 immediately and Claude's normal in-terminal prompt fires
- If multiple presenters want to *display* the question (watch + lamp + deck), they all subscribe; only one answers

This makes HITL a *capability*, not a special case. m5-paper-buddy registers as authoritative for sessions that opt in. claude-watch registers when the user explicitly hands off to the watch. Redlight Greenlight registers globally. The daemon stays single-tenant on the hook installation side, multi-tenant on the display side, and lets exactly one tool answer.

The watch/deck tools that don't want to be authoritative just subscribe to display permission events as informational signals. That's already covered by the read API.

---

## 7. LAN reachability and discovery

**Tools that need it:** claude-watch (iPhone bridge to Mac, watch to iPhone), Happy Coder (phone to Mac), AgentDeck (Pixoo64, ESP32, Android, iOS, all on one bridge — uses **mDNS + QR pairing**), m5-paper-buddy (BLE to Mac, but the same architectural need), claude-rpc (Discord shows status from another machine), Bobby-Gray cinematic display (Chromecast or tablet on LAN), Pixoo family dashboards (Pixoo64 polling LAN-side).

**Current design:** assumes localhost. The discovery file at `~/.bowerbird/server.json` carries `127.0.0.1:<port>`.

**Why this matters:** every wearable / hardware / second-screen tool needs to reach the daemon over LAN. Forcing each one to ship its own bridge process on the laptop reproduces the fragmentation we're trying to eliminate.

**Proposed addition:**

- Daemon binds **two listeners**: localhost (always on, no auth required) and LAN (off by default, opt-in)
- LAN listener uses **mDNS / Bonjour** (`_bowerbird._tcp.local`) for zero-config discovery
- Per-device pairing via a short-lived QR code containing `{host, port, paired_token}` — same pattern AgentDeck and Happy Coder already use
- Devices that pair get a long-lived per-device token; can be revoked from the daemon
- The auth model is per-device, not per-session: a paired Apple Watch can see all sessions; Cardputer can see all sessions; etc.

The localhost interface stays simple. The LAN interface is opt-in but uses standard discovery.

---

## 8. Codex / OpenCode / Cursor adapter sketch

**Tools that need it:** patoles/agent-flow (visualizes Claude Code **and** Codex simultaneously, OpenCode plugin in flight), AgentDeck (Claude Code, Codex, OpenCode, OpenClaw creature sprites), claude-rpc (Codex parallel support), tmux-agent-status / opensessions / ccmanager (multi-CLI from day one).

**Current design:** says it's agent-agnostic. Only details Claude Code adapters.

**Why this matters:** the abstraction either holds for Codex and OpenCode or it doesn't. If the same `AgentEvent` union and `HookProvider` shape work for Codex's `~/.codex/sessions/**/rollout-*.jsonl` format, the daemon is genuinely multi-agent. If it doesn't, the daemon is "Claude Code with extra steps."

**Proposed addition:** sketch the Codex adapter explicitly. Codex emits:

- Roll-out files in JSONL format under `~/.codex/sessions/`
- Different lifecycle: no SessionStart hook, just file appearance; no PreToolUse / PostToolUse hooks at all (Codex doesn't have hooks); state is *only* derivable from the JSONL stream
- Different tool vocabulary (`shell`, `apply_patch`, `update_plan`, etc.)

So Codex is a **file-only provider** — same `AgentEvent` union, but `parseTranscriptLine` is the only ingest path; `installHooks` is a no-op; `formatToolStatus` gets a Codex-specific tool name table. This is exactly what Pixel Agents's provider abstraction already anticipated ("FileProvider and StreamProvider will be added alongside the first real second provider"). The daemon should ship at least one non-Claude provider in v1 to validate the abstraction.

OpenCode is similar. Cursor CLI's situation is different (Cursor Composer is in the IDE, not a CLI tool), so probably out of scope.

---

## 9. Last-assistant-message exposure for content-aware presenters

**Tools that need it:** clarvis (LLM-summarizes Claude responses to 2 sentences before TTS), kennethleungty/claude-music (`/vibe` reads conversation and picks music genre), Bobby-Gray cinematic display (typewriter narration of last response), Handwave (Haiku-summarized response for spoken playback).

**What's missing:** the event log has tool events; assistant text isn't a first-class event. These tools all tail JSONL themselves.

**Proposed addition:** an optional `assistantMessage` event when the JSONL adapter sees a new assistant turn:

```ts
assistantMessage {
  message_id: string,
  text: string,                  // potentially long; bounded by config
  has_tool_use: boolean,
  is_final: boolean              // last assistant message before turnEnd
}
```

This is the lowest-priority gap because it's easy to ignore and JSONL tailing is already done well. Worth listing only because three independently developed presenters do the same tailing today.

---

## What the survey didn't surface that I expected to

A few things the daemon does cover that the survey confirms aren't gaps:

- **State enum (idle/working/waiting/...)** — every pet, lamp, sprite, and HUD uses some version of this. Already canonical.
- **Tool-name → human-readable string** — already flagged as a gap (formatToolStatus); confirmed.
- **Worktree / repo / branch grouping** — only dmux and the orchestrators care; novelty tools surveyed don't (yet). Already in the design, but not driven by these tools.
- **Session-vs-process distinction** — none of the novelty tools surfaced this need explicitly. Validated indirectly: every "is this session live" check in the surveyed tools is heuristic.
- **Terminal/multiplexer attribution** — claude-status uses it for click-to-focus; nothing else surveyed exploits it yet, but the survey reinforces that this is a strict capability gap nobody else fills.

## Updated scope summary

The current design serves cleanly:
- Pets, statuslines, HUDs, dashboards, observability tools (the original target)

The current design serves with stretch:
- Discord Rich Presence, menu-bar usage trackers (need the cost rollup)
- Multi-agent visualizers (Outworked, claude-office, AgentDeck) — need agent persona

The current design can't serve without changes:
- Wearables and hardware approval surfaces (need HITL backflow + LAN reachability)
- Cross-runtime visualizers (need a non-Claude provider sketch)
- Pet decay timers and lifetime stat displays (need aggregates as a first-class projection)

Adding gaps 1, 2, 4, 5, 7, 8 (six of nine) is the difference between "the design covers most observability tools" and "the design covers most observability *and* novelty tools." Gap 6 (HITL) is the one that genuinely changes what the daemon is, from a one-way bus to a one-way bus with a clearly-defined backflow extension. Gap 3 is small. Gap 9 is optional.

The decisions I'd flag for explicit Y/N:

- **HITL as extension surface, not "out of scope"?** The survey strongly suggests yes.
- **Ship a Codex adapter in v1?** The abstraction needs validation.
- **LAN-reachability default-off but mDNS-discoverable?** Required for wearables to be a real category.