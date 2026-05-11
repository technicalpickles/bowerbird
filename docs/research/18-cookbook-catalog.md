# Cookbook recipe catalog

Catalog of cookbook entries to ship with the substrate. Every entry corresponds to a runnable example in `examples/` and is exercised in CI. The constraint: nothing depends on external APIs, hardware, or paid services we can't validate.

Each recipe teaches one thing. Where possible, the example does it in the simplest plausible form — `console.log`, a file write, an HTTP POST to a local mock, an HTML+JS page that opens in the browser. Real-world adaptations (writing to a BLE bulb, sending to Discord, driving a Stream Deck) belong in user-published presenter packages, not in our cookbook.

Total: 17 recipes across 7 categories. Marked **[MVP]** if it ships with M1, **[M3]** if it requires M3 schema additions, **[M4]** if it requires the capabilities surface, **[Later]** if it depends on milestones beyond that.

---

## Category 1: Subscription basics

These are the recipes a new presenter author reads first. They teach how to connect, authenticate, subscribe, and handle messages.

### 1.1 Hello, current state **[MVP]**

**Teaches:** WebSocket connection, auth, single-topic subscription, snapshot-on-subscribe semantics.

**The recipe:** Connect to the daemon. Subscribe to `state.session.<id>.current_state` for a single session. Log every transition to stdout.

**Why it matters:** This is the absolute minimum viable presenter. ~25 lines of JavaScript. Demonstrates that "subscribe and react" is genuinely one substrate call.

**Example:** `examples/hello-state/index.js`

```
[snapshot] working
[change] working → waiting
[change] waiting → working
[change] working → success
```

### 1.2 Hello, event firehose **[MVP]**

**Teaches:** Raw event consumption, wildcards in topic patterns, payload structure.

**The recipe:** Subscribe to `events.session.<id>.*`. Print each event with `kind`, `tool_name` (where present), and elapsed time since the previous event.

**Why it matters:** The other half of the pub/sub model. Shows what raw events look like and how to filter by topic pattern.

**Example:** `examples/hello-events/index.js`

```
[0ms]   userPromptSubmit
[140ms] preToolUse  Read
[180ms] postToolUse Read (success)
[210ms] preToolUse  Edit
[450ms] postToolUse Edit (success)
[460ms] stop
```

### 1.3 Authentication and reconnection **[MVP]**

**Teaches:** Reading the auth token, exponential backoff, snapshot refetch on resubscribe.

**The recipe:** Read `~/.claude-state-bus/server.json` for the token. Connect; on disconnect, reconnect with exponential backoff (1s, 2s, 4s, 8s, capped at 30s). On reconnect, all subscriptions are re-established and fresh snapshots arrive automatically.

**Why it matters:** Every presenter needs this. Doing it wrong is the source of most "presenter looks frozen after daemon restart" bug reports.

**Example:** `examples/auth-and-reconnect/index.js`

---

## Category 2: State consumption

These recipes show how to use the STATE channel — the projection layer's transition signals.

### 2.1 Collapsing the 11-value reaction enum **[MVP]**

**Teaches:** How to map the canonical enum to a smaller display vocabulary; the principle that presenters do the collapse.

**The recipe:** Subscribe to `state.session.<id>.current_state`. Map all 11 values to four display states: `idle`, `working`, `waiting`, `error`. Print the display state plus the underlying reaction for transparency.

```javascript
const collapse = {
  idle: "idle",
  success: "idle",
  celebrating: "idle",
  thinking: "working",
  working: "working",
  editing: "working",
  running: "working",
  testing: "working",
  waving: "working",
  waiting: "waiting",
  error: "error",
};
```

**Why it matters:** The reaction enum is intentionally rich. Most presenters want a smaller vocabulary. This recipe shows the right way to narrow it.

**Example:** `examples/collapse-reactions/index.js`

### 2.2 Snapshot + delta for a session list **[MVP]**

**Teaches:** Combining `GET /sessions` with `state.sessions.added/removed` subscription for a live session list.

**The recipe:** Fetch the initial session list via REST. Subscribe to `state.sessions.added` and `state.sessions.removed`. Render a live-updating list to the terminal using ANSI cursor codes.

**Why it matters:** Almost every multi-session presenter needs this pattern. The combination of one REST call (for the initial snapshot) plus two pub/sub subscriptions (for changes) is the canonical shape.

**Example:** `examples/session-list/index.js`

```
3 active sessions
  ▶ claude-state-bus/main          working
  ▶ blog/draft-post                idle
  ▶ scratch/poc                    waiting
```

### 2.3 Reacting to permission requests **[MVP]**

**Teaches:** Subscribing to a specific event kind, parsing the native payload, presenting the choices.

**The recipe:** Subscribe to `events.permissionRequest.*`. When one arrives, print the question text, list the options, and the underlying tool input — all from the native payload.

**Why it matters:** Permission rendering is one of the most common multi-presenter use cases. This recipe shows how to do it without trying to *answer* the prompt (which is out of scope per the no-list).

**Example:** `examples/permission-display/index.js`

```
[permission] session=abc-123 agent=main
  question: "Allow Bash to run: rm -rf /tmp/work"
  options:
    1. Allow once
    2. Allow always for this session
    3. Don't allow
  tool_input: {"command":"rm -rf /tmp/work"}
```

### 2.4 Lifecycle and liveness together **[M3]**

**Teaches:** The distinction between session lifecycle (`live` / `paused` / `abandoned` / `ended`) and attachment liveness (`alive` / `exited` / `unknown`).

**The recipe:** Subscribe to both `state.session.<id>.lifecycle` and `state.session.<id>.attachment`. Render a table showing every active session's lifecycle and liveness side by side. Demonstrate the various combinations (`live + alive` = working normally, `live + exited` = process died mid-turn, `paused + alive` = between turns).

**Why it matters:** The lifecycle/liveness split is one of the corrections from `10-multi-agent-tool-patterns.md` (borrowed from opensessions). This recipe makes it concrete.

**Example:** `examples/lifecycle-liveness/index.js`

---

## Category 3: Event consumption

These recipes show how to use the EVENTS channel — the raw event stream.

### 3.1 Single-event-kind subscriber **[MVP]**

**Teaches:** The "fire on one specific event" pattern. Subscribe to exactly one event kind across all sessions.

**The recipe:** Subscribe to `events.sessionEnd.*`. Print a one-line summary for each session that ends: session ID, duration, total events. Write to a logfile that grows over time.

**Why it matters:** This is the shape that tools like `claude-receipts` use. The substrate handles the per-event-kind filtering on its side; the presenter is trivial.

**Example:** `examples/session-end-log/index.js`

```
2026-05-12 14:32:11  claude/abc-123  duration=18m   events=147
2026-05-12 15:01:08  claude/def-456  duration=42m   events=312
2026-05-12 15:18:33  claude/abc-123  duration=4m    events=23   (resumed)
```

### 3.2 Routing on agent_type **[MVP]**

**Teaches:** Reading `agent_type` from event payloads, mapping to per-type behavior.

**The recipe:** Subscribe to `events.subagentEnd.*`. Maintain a config dict mapping `agent_type` → a label. Print a colored line per subagent end:

```
[code-reviewer] finished — 23 events in 4m
[researcher]    finished — 8 events in 1m
[code-reviewer] finished — 19 events in 3m
```

**Why it matters:** This is the substrate's `agent_type` story end-to-end. PAI-style voice routing, Outworked-style employee animations, and many other presenters key on this. The substrate guarantees `agent_type` is passed through verbatim; presenters do the mapping.

**Example:** `examples/agent-type-routing/index.js`

### 3.3 Filtering by tool name **[MVP]**

**Teaches:** Subscribing to `events.preToolUse.*` and filtering client-side by `tool_name`.

**The recipe:** Subscribe to `events.preToolUse.*`. Keep a tally of how many times each tool has been invoked across all sessions in the current daemon lifetime. Print the running tally every 5 seconds.

```
Tool counts (last 5s update):
  Read       147
  Edit       89
  Bash       42
  Grep       31
  Glob       12
```

**Why it matters:** Shows client-side filtering of the firehose. Also a small concrete example of "what would a usage analytics tool look like."

**Example:** `examples/tool-tally/index.js`

---

## Category 4: Derived computation

These recipes show patterns that presenters compute themselves from substrate signals — the things the substrate *won't* compute (per the no-list).

### 4.1 Computing activity rate, sliding window pattern **[MVP]**

**Teaches:** The tamagotchi pattern — deque of timestamps, count within a 60s window.

**The recipe:** Subscribe to `events.session.<id>.*`. Maintain a sliding window of event timestamps. Print the events-per-minute rate every second:

```
[14:32:00] activity: 12 events/min (low)
[14:32:01] activity: 14 events/min (low)
[14:32:02] activity: 18 events/min (active)
[14:32:03] activity: 24 events/min (intense)
```

Includes the threshold logic (>20 = intense, >10 = active) from tamagotchi.

**Why it matters:** One of the four activity patterns from `14-activity-survey.md`. ~15 lines including the threshold tiers.

**Example:** `examples/activity-sliding-window/index.js`

### 4.2 Computing activity rate, leaky bucket pattern **[MVP]**

**Teaches:** The claude-quest pattern — increment per event, decay over time.

**The recipe:** Same as 4.1 but using a leaky bucket. `flow += 0.05` per event; decay `0.03/sec` after a 5-second grace period. Print flow score (0..1) every second.

```
[14:32:00] flow: 0.42 ████░░░░░░
[14:32:01] flow: 0.57 █████░░░░░
[14:32:02] flow: 0.83 ████████░░  (peak nearing)
[14:32:03] flow: 1.00 ██████████  (peak reached!)
[14:32:08] flow: 0.97 █████████░  (no activity, decaying)
```

**Why it matters:** The other valid pattern. Different presenters prefer one or the other. Both are short.

**Example:** `examples/activity-leaky-bucket/index.js`

### 4.3 Time-since-activity displays **[MVP]**

**Teaches:** The claude-status pattern — render relative time from `last_event_at`.

**The recipe:** Fetch the session list. For each session, render `<session> — <last_event_at relative>`. Refresh display every second.

```
claude-state-bus/main    active now
blog/draft-post          2m ago
scratch/poc              17m ago
old-experiment           3h ago
```

**Why it matters:** The simplest activity signal — one timestamp, no computation. Five of eight tools surveyed in `14-activity-survey.md` use this approach.

**Example:** `examples/time-since/index.js`

### 4.4 Detecting stuck or slow sessions **[M3]**

**Teaches:** The Outworked two-tier pattern — threshold against `last_event_at` for "slow" (5min) and "stuck" (10min) badges.

**The recipe:** Every 10 seconds, check all sessions in `state.current_state === "working"`. For each, check `(now - last_event_at)`. Print badges:

```
[ok]    claude-state-bus/main    (30s)
[slow]  blog/draft-post          (5m12s)
[stuck] scratch/poc              (11m4s)  — abort recommended
```

**Why it matters:** Demonstrates derived state computation from `last_event_at`. Outworked uses this for its stuck-detection UI; many presenters could.

**Example:** `examples/stuck-detection/index.js`

---

## Category 5: Multi-session and grouping

These recipes show how to render or compute across many sessions.

### 5.1 Grouping by git remote **[M3]**

**Teaches:** Using `remote_url` to group sessions across worktrees of the same repo.

**The recipe:** Fetch sessions. Group by `remote_url`. Render a tree:

```
github.com/josh/blog
  ├ blog/main             working
  ├ blog/draft-feature    waiting
  └ blog/refactor-tags    idle

github.com/josh/scratch
  └ scratch/poc           working

(no remote)
  └ home/dotfiles         idle
```

Subscribe to `state.sessions.added/removed` and `state.session.*.current_state` to keep the tree live.

**Why it matters:** Test case 1 from `13-test-cases.md`. The TUI tool that resembles tmux-agent-sidebar. ~60 lines.

**Example:** `examples/grouped-tui/index.js`

### 5.2 Aggregating tool counts across sessions **[MVP]**

**Teaches:** Maintaining cross-session aggregates as events arrive.

**The recipe:** Subscribe to `events.preToolUse.*` across all sessions. Maintain a per-tool count per session. Print a table showing tool usage by session.

```
Session                          Read  Edit  Bash  Grep  Total
claude-state-bus/main              42    18    11     7     78
blog/draft-post                    23     5     2     0     30
scratch/poc                         8     1     0     0      9
```

**Why it matters:** Demonstrates that the same firehose subscription can drive many derived views. The aggregator is small (~30 lines).

**Example:** `examples/tool-counts-by-session/index.js`

### 5.3 Highest-priority session indicator **[MVP]**

**Teaches:** Aggregating state across multiple sessions into a single "what should the user pay attention to" signal.

**The recipe:** Subscribe to `state.session.*.current_state`. Maintain a priority map (matching opensessions' priority order):

```
waiting     (5)  ← needs user attention
error       (4)
working     (3)
testing     (3)
running     (3)
editing     (3)
thinking    (2)
celebrating (1)
success     (1)
idle        (0)
```

Output the single highest-priority state across all sessions. Useful for menu bar indicators that show one global status.

```
[!] waiting   ← claude-state-bus/main needs input
```

**Why it matters:** claude-status menu bar does this. So does any "single global indicator" tool. The aggregation is presenter logic, but the substrate provides all the inputs.

**Example:** `examples/global-priority-indicator/index.js`

---

## Category 6: Resilience and operations

These recipes cover the boring-but-essential failure handling.

### 6.1 Handling dropped frames **[MVP]**

**Teaches:** What to do when the substrate reports backpressure overflow.

**The recipe:** Subscribe to a high-volume topic (`events.*`). Artificially slow the consumer (`await sleep(100)` per message). After a few seconds the daemon sends a `dropped` frame indicating which event_ids were lost. On receiving `dropped`, refetch the latest state via REST and re-establish subscriptions.

**Why it matters:** Most presenters won't hit this in practice, but the pattern is part of the contract. Writing it once and pointing to it in the cookbook prevents every presenter from rediscovering it.

**Example:** `examples/dropped-frame-handling/index.js`

### 6.2 Surviving daemon restart **[MVP]**

**Teaches:** What "snapshot-on-resubscribe" actually buys you.

**The recipe:** Connect, subscribe to several topics. Manually kill and restart the daemon (the example script does this). Demonstrate that on reconnect, the presenter gets fresh snapshots for all STATE topics it had subscribed to, and resumes events from a cursor.

**Why it matters:** The daemon is a long-running service but it can crash, restart, or be upgraded. Presenters that fail gracefully across restarts are the only kind that survive in production.

**Example:** `examples/daemon-restart-resilience/index.sh` (mixes shell scripting to restart the daemon with a JS presenter)

### 6.3 REST polling fallback **[MVP]**

**Teaches:** How to consume the substrate without pub/sub, for tools that prefer simple HTTP.

**The recipe:** Every 5 seconds, `GET /sessions`. Diff against the previous result. Print added, removed, and changed sessions.

**Why it matters:** Some integrations (cron-driven scripts, CI pipelines, tools running on hostile networks) don't want WebSocket. The REST endpoints exist as a fallback; this recipe shows they're real.

**Example:** `examples/rest-polling/index.sh` (bash + jq, no Node)

```bash
#!/bin/bash
while true; do
  curl -s -H "Authorization: Bearer $(cat ~/.claude-state-bus/server.json | jq -r .token)" \
    http://127.0.0.1:9876/sessions | \
    jq -r '.[] | "\(.session_id) \(.current_state)"'
  sleep 5
done
```

---

## Category 7: Visual and integration patterns

These are end-to-end examples that combine multiple recipes into something close to a real presenter, while still being self-contained.

### 7.1 Single-file HTML sprite display **[M3]**

**Teaches:** Driving an animated visualization from substrate state.

**The recipe:** A single `.html` file that opens in a browser and renders one animated sprite per session. State changes update the sprite's animation. `remote_url` determines color palette. `last_event_at` drives animation speed (client-computed activity rate). `attachment.liveness=exited` triggers death animation.

The HTML+JS+CSS is self-contained, no build step, no framework. Connects to the daemon via WebSocket from the page.

**Why it matters:** Test case 2 from `13-test-cases.md`. Self-contained, browser-only, no hardware. Demonstrates the substrate's value to a frontend-curious audience. ~250 lines including HTML and SVG sprites.

**Example:** `examples/sprite-web/index.html`

### 7.2 TUI sidebar **[M3]**

**Teaches:** Building a terminal-resident multi-session view (similar shape to tmux-agent-sidebar or opensessions, but standalone).

**The recipe:** A Node script using a simple TUI library (built-in `readline` + ANSI escapes, no `blessed` or other heavy deps). Renders the session list grouped by remote, with per-session state indicators. Live updates.

**Why it matters:** Demonstrates that a 60-line presenter can replicate the core of much larger tools.

**Example:** `examples/grouped-tui/index.js`

### 7.3 Markdown daily summary **[MVP]**

**Teaches:** Batch consumption of the event log via REST for after-the-fact reporting.

**The recipe:** A script that runs `GET /events?since=<24hours-ago>` and aggregates into a markdown summary:

```markdown
# Coding activity — 2026-05-12

## Sessions
- **claude-state-bus/main** (4 hours, 312 events)
  - 89 tool calls (Edit: 32, Bash: 24, Read: 18, ...)
  - 2 subagent runs (code-reviewer, researcher)
- **blog/draft-post** (45 minutes, 67 events)
  - 19 tool calls
  - 1 permission prompt (Bash)

## Totals
- 5 sessions, 8h 12m total active time
- 421 events across all sessions
- 12 tool kinds used
- 0 errors
```

Writes to `~/claude-daily-<date>.md`. Could be run from cron or a launchd LaunchAgent.

**Why it matters:** Demonstrates the REST surface as the right answer for "I want a batch summary," not the WebSocket. Different consumption shapes for different needs.

**Example:** `examples/daily-summary/index.js`

---

## Category 8: Adapter authoring (separate page, referenced)

This isn't strictly a cookbook entry — it's a longer document — but it lives in the same conceptual space.

### 8.1 Writing an adapter for a new agent **[M2]**

**Teaches:** The full adapter contract. Worked example: how to add Codex.

This is a long-form guide rather than a recipe, so it lives in `docs/adapter-authoring.md` and not in `cookbook/`. Cross-referenced from there.

**Why it matters:** Adapter contributions are the highest-value PRs the project accepts (per the contribution model). The guide makes them easy.

---

## What's deliberately not in the cookbook

The same discipline as the no-list applies: some patterns are tempting but don't belong.

- **Anything that requires an external API.** ElevenLabs TTS, OpenAI for follow-up summaries, Discord webhooks, Slack notifications. Each user installs their own.
- **Anything that requires hardware.** BLE bulbs, Stream Deck buttons, e-ink displays. Each user wires their own.
- **Anything that does HITL backflow.** Subscribing to `permissionRequest` and rendering it is fine (recipe 2.3). Actually approving back is out of scope (per the no-list).
- **Multi-agent recipes during MVP.** Codex doesn't exist in M1; cross-source examples wait for M2.
- **Statusline composition.** That's a different problem; comes in M6.
- **Persistent presenter state.** A "remember which sessions I've seen" recipe would need the presenter to manage its own storage. We can show how, but not in the substrate's cookbook — it'd suggest the substrate provides this.

## Minimum viable cookbook for MVP launch

Out of the 17 entries above, **11 are tagged [MVP]** and would ship with v1:

- 1.1 Hello, current state
- 1.2 Hello, event firehose
- 1.3 Authentication and reconnection
- 2.1 Collapsing the reaction enum
- 2.2 Snapshot + delta for session list
- 2.3 Reacting to permission requests
- 3.1 Single-event-kind subscriber
- 3.2 Routing on agent_type
- 3.3 Filtering by tool name
- 4.1 Activity rate, sliding window
- 4.2 Activity rate, leaky bucket
- 4.3 Time-since-activity displays
- 5.2 Aggregating tool counts
- 5.3 Highest-priority indicator
- 6.1 Dropped frames
- 6.2 Daemon restart
- 6.3 REST polling fallback
- 7.3 Markdown daily summary

That's actually 18 — let me recount.

Recounting MVP entries: 1.1, 1.2, 1.3, 2.1, 2.2, 2.3, 3.1, 3.2, 3.3, 4.1, 4.2, 4.3, 5.2, 5.3, 6.1, 6.2, 6.3, 7.3 = 18 recipes. Add 2.4 (lifecycle/liveness needs M3 for attachments), 4.4 (stuck detection benefits from M3), 5.1 (grouping needs `remote_url`), 7.1 (sprite needs M3 for `remote_url` + `context_percentage`), 7.2 (TUI needs M3 for `remote_url`) = 5 more at M3.

Total: 18 at MVP, +5 at M3, +1 at M2 (adapter authoring) = **24 entries total at M3**.

That feels close to right. Some of the M3 entries could ship at MVP with caveats (the sprite example could work without `remote_url`, using session name for color hashing).

## Ordering for documentation

Recommended reading order for someone new to the substrate:

1. **Read README.md** (you are here)
2. **Read 1.1 Hello, current state** (the minimum viable presenter)
3. **Read 1.3 Authentication and reconnection** (the things every real presenter needs)
4. **Branch:**
   - If you're building a notifier or logger: 3.1, 3.2, 7.3
   - If you're building a live visualizer: 2.1, 2.2, 5.3, then 7.1 or 7.2
   - If you're building an analytics tool: 6.3, 7.3
   - If you're building a pet/sprite/lamp: 2.1, 4.1 or 4.2, then 7.1
5. **Eventually read 6.1 and 6.2** before going to production

The cookbook README should include this branching guide.

## CI requirements for cookbook entries

Each example must:

- Build with `cargo build` (if Rust) or `node --check` (if Node) or `bash -n` (if shell).
- Run against a fresh daemon in `examples/test.sh` and produce expected output within a timeout.
- Have its README excerpt match what's in the cookbook entry (no drift).

The example test rig spawns a daemon on a test port, injects synthetic events, runs the example, captures output, and diffs against an expected fixture. If the daemon's API changes, examples either keep working or block the PR.

## Source attribution

Many recipes are distilled from observed patterns in the inventory:

- 4.1 (sliding window) — tamagotchi
- 4.2 (leaky bucket) — claude-quest
- 4.3 (time-since) — claude-status
- 4.4 (stuck detection) — Outworked
- 5.1 (grouping by remote) — tmux-agent-sidebar
- 5.3 (priority indicator) — opensessions (priority order) + claude-status (single indicator)
- 7.1 (sprite display) — abstracted from openpets, pixel-agents, AgentDeck creatures
- 7.2 (TUI) — opensessions, tmux-agent-sidebar
- 7.3 (daily summary) — claude-receipts

The cookbook gives credit to where the patterns came from. Each entry can include a short "Inspired by" line linking back to the source tool.