# Activity measurement across pet/sprite/dashboard tools

This document examines how seven existing tools measure, surface, and use "activity level" — to inform whether the substrate should expose activity counters as a derived projection (proposed in `13-test-cases.md`) and if so, what shape.

The tools examined:

- **ccpet** — statusline pet, energy + decay model
- **claude-code-tamagotchi** — statusline pet with sliding-window activity tracking
- **OpenPets** — discrete-reaction pet system, no activity rate
- **Pixel Agents** — VSCode pet visualizer with multi-timescale staleness detection
- **Outworked** — Animal Crossing-style office, two-tier stuck detection
- **AgentDeck** — physical Stream Deck creature display, discrete state model
- **claude-quest** — RPG sprite tool with flow meter
- **claude-status** — macOS menu bar, single-timestamp activity model

Each is examined for what mechanism it uses, what timescales it operates on, and what visual or behavioral signal it drives.

## Findings per tool

### ccpet — token-based feeding + exponential time decay

ccpet's "activity" isn't event-rate; it's **token consumption**. Tokens read directly from the Claude JSONL transcript at every statusline tick. The pet's `energy` increases as tokens are consumed and decreases via time-based decay.

Mechanism:
- `Pet.feed(tokens)` increments `accumulatedTokens`; converts to energy at `TOKENS_PER_ENERGY` rate (config). Excess accumulates for the next feeding.
- `Pet.applyTimeDecay()` runs every statusline tick. Decay rate: `100 / (3*24*60)` ≈ **0.0231 energy/minute** by default (3 days from 100 to 0). Configurable via `TIME_DECAY` config.
- `getTokenMetrics(transcriptPath)` reads the JSONL transcript and an external `~/.claude-pet/global-tracker.json` to only count *new* tokens since the last tick. This is "incremental activity since last update."

Surface: an emoji + numeric energy in the statusline. Also `contextLength` from the JSONL parsed and exposed as `contextPercentage` (against 200k limit) and `contextPercentageUsable` (against 160k limit).

What this implies for the substrate: ccpet doesn't need a daemon to expose activity rate — it just wants reliable per-tick token counts. The substrate's `total_tokens` and `recent_tokens_since_<cursor>` (deltas) would replace ccpet's need to maintain `global-tracker.json` and re-tail JSONL.

### claude-code-tamagotchi — sliding window of timestamps + intensity tiers

The most sophisticated activity model surveyed. Tamagotchi has an explicit `ActivitySystem.ts`. State fields:

```ts
sessionUpdateCount: number;        // Updates this session
totalUpdateCount: number;          // Lifetime updates
lastUpdateTimestamp: number;       // For gap detection
recentUpdateTimestamps: number[];  // Last 30 statusline-update timestamps
sessionStartTime: number;
previousSessionEnd: number;
sessionsToday: number;
```

Three timescales of activity inference:

1. **Per-tick (each statusline call):** `applyActivityUpdate()` increments counters, appends current timestamp, trims to last 30.

2. **Per-minute (rate):** `calculateActivityIntensity()` filters `recentUpdateTimestamps` to the last 60 seconds and returns the count.
   - intensity > 20 → "intense coding" (extra energy + hunger drain)
   - intensity > 10 → "active coding" (some extra drain)
   - else → normal

3. **Session-gap (idle):** `SESSION_GAP_THRESHOLD = 5 * 60 * 1000` (5 min). If `now - lastUpdateTimestamp > threshold`, treat as a new session — the pet "slept" and gets sleep-recovery energy:
   ```ts
   const sleepHours = Math.min(8, sessionGap / (1000 * 60 * 60));
   state.energy = Math.min(100, state.energy + (sleepHours * 10));
   ```
   8 hours capped, 10 energy per hour of sleep, max +80 energy.

Decay happens every `UPDATE_DECAY_INTERVAL = 20` updates. Sleep recovery: 3% energy per update while sleeping.

Mood derives from activity + state:
- intensity > 20 OR sessionUpdateCount > 200 → `focused`
- energy < 20 → `tired`
- otherwise → `normal` or `debugging`/`celebrating`/`tired` based on keywords

Keyword detection looks at user prompt text for `error`/`bug`/`fixed`/`works`/`?` for mood transitions.

Surface: animated emoji-style pet in statusline + thought bubbles + stats line.

What this implies: tamagotchi's `recentUpdateTimestamps` array (sliding window of last 30 ticks) is doing what the substrate's proposed `recent_event_count_60s` would do — but tamagotchi conflates "statusline ticks" with "events." If the substrate exposes both `recent_event_count_60s` (from real events) and `recent_tool_count_60s`, tamagotchi could read either and trust it.

### OpenPets — discrete reactions, no activity rate

OpenPets uses 11 discrete reactions (the canonical list lifted in v2/v2.1): `idle, thinking, working, editing, running, testing, waiting, waving, success, error, celebrating`. Each is a state, not a rate.

Hook events trigger reactions directly. The hook speech categories are deliberately small (`thinking | success | error | permission`) — they reflect *what just happened*, not *how busy the agent is*.

No activity counters, no sliding windows, no decay. The pet's mood is "what reaction is it currently performing." A lease/heartbeat system handles single-pet ownership across multiple Claude sessions, but that's about coordination, not activity.

What this implies: OpenPets-style consumers don't need activity rate. They want clean reaction signals — `current_state` transitions. The substrate's STATE channel topic `state.session.<id>.current_state` is sufficient.

### Pixel Agents — multi-timescale staleness

Pixel Agents has three timescales of "staleness," each for a different purpose:

- **TEXT_IDLE_DELAY_MS = 5,000** — 5 seconds of no transcript activity → text-idle (animation slows, status indicator updates). This is the "is the agent currently working" timescale.
- **EXTERNAL_ACTIVE_THRESHOLD_MS = 120,000** — 2 minutes. An "external" agent (one Pixel Agents discovered via global JSONL scan, not via hooks) is considered "active" if its JSONL file changed within this window.
- **GLOBAL_SCAN_ACTIVE_MAX_AGE_MS = 600,000** — 10 minutes. The window for "should this session even appear in the global discovery scan." Older than 10 min and it's not shown.

Plus polling intervals: `JSONL_POLL_INTERVAL_MS = 1000`, `FILE_WATCHER_POLL_INTERVAL_MS = 500`, `EXTERNAL_SCAN_INTERVAL_MS = 3000`, `EXTERNAL_STALE_CHECK_INTERVAL_MS = 30,000`.

Notable comment from their fileWatcher:

```
HOOKS MODE (preferred): Claude Code Hooks API delivers instant, reliable events
for session lifecycle. When hooks work, heuristic scanners and timers are suppressed.

HEURISTIC MODE (fallback): For environments without hooks. Uses:
- Per-agent 500ms JSONL polling for tool activity and /clear detection
- 1s main scanner for terminal adoption
- 3s external scanner for external session detection
- 30s stale check for orphaned external agents
```

They explicitly built dual-mode because hooks are unreliable in some environments. The substrate's hook-router approach inherits this fragility unless the daemon can also tail JSONL as a fallback.

What this implies: the substrate needs to expose multiple "staleness windows" to match the pattern (short-idle for animation, medium for is-this-alive, long for cleanup). Or — simpler — expose `last_event_at` and let presenters compute the windows they need. The cleanup window (10 min) is plausibly a daemon-side sweep concern (deferred to M7+ in the milestones).

### Outworked — two-tier stuck detection

Outworked tracks a per-agent `lastActivity` timestamp and runs a `setInterval` that flags two thresholds:

- **SLOW_TIMEOUT_MS = 300,000** (5 min) — fires `onSlow` callback. Soft warning; visual cue (?).
- **STUCK_TIMEOUT_MS = 600,000** (10 min) — fires `onStuck` callback. Enables an abort button in the UI.

Agent status enum: `working | done | waiting-input | waiting-approval | slow | stuck`. The `slow` and `stuck` values are **timing-derived states**, not event-derived.

No sliding-window rate. Activity tracking is a single timestamp per agent + threshold checks every N seconds.

Status-to-animation mapping is simple:
- thinking → `think` anim
- working → `type` anim
- speaking → `type` anim
- collaborating → `walk` anim
- background → `type` anim
- default → `idle` anim

Animation speed is fixed per anim type. The animation *changes* with state, not the speed.

What this implies: Outworked's two-tier "slow / stuck" pattern is a useful presenter-side projection that the substrate doesn't need to model. Presenters check `lastActivity` against their own thresholds.

### AgentDeck — discrete state machine with priority

AgentDeck's state model is purely discrete: `IDLE | PROCESSING | AWAITING_PERMISSION | AWAITING_OPTION | AWAITING_DIFF | DISCONNECTED`. No rate, no sliding window, no activity intensity.

Transitions trigger:
- TTS announcements (PROCESSING→IDLE plays a "done" sound)
- Voice assistant responses
- LED/screen color changes (`IDLE:'#22c55e' (green), PROCESSING:'#3b82f6' (blue), AWAITING_PERMISSION:'#f59e0b' (amber), DISCONNECTED:'#ef4444' (red)`)
- Stream Deck button updates

The closest to "activity" is that PROCESSING is a *state*, not a level. A creature on the LED matrix is either "doing something" or "not."

Timeline entries (for the Android relay) record discrete events with timestamps; the relay shows them as a scrolling feed but doesn't compute rates.

What this implies: AgentDeck is the simplest model — purely state-driven. The substrate's `current_state` projection is exactly what it consumes.

### claude-quest — flow meter (decay-based scalar)

The simplest *rate* model surveyed. claude-quest has a single `FlowMeter` ∈ [0, 1]:

```go
if hadActivity {
    s.FlowMeter += 0.05
    // clamp at 1.0
    s.FlowDecayTimer = 0
} else {
    s.FlowDecayTimer += dt
    if s.FlowDecayTimer > 5.0 {   // 5 second grace
        s.FlowMeter -= dt * 0.03  // decay at 0.03/sec
    }
}
```

Activity increment: +0.05 per event. Decay: 0.03/sec after a 5-second grace period. Effectively a leaky bucket / EWMA where steady activity keeps the meter near 1.0 and a 30-second pause drops it to zero.

Used for: a "flow peak reached" event that triggers in-game rewards. Tied into an RPG progression system with bash streaks, levels, and per-biome bonuses.

What this implies: the leaky-bucket model is a cleaner mathematical formulation than the sliding-window approach. The substrate could expose either, but a sliding window is more directly inspectable ("how many events in the last 60 seconds?") whereas a leaky bucket is a continuous scalar requiring tuning.

### claude-status — single-timestamp activity, sort by elapsed

claude-status has only `lastActivityAt: Date` per session. No rate, no sliding window, no decay model. State is 4-value: `active | waiting | idle | compacting`.

Two uses of `lastActivityAt`:
1. **Display:** `timeSinceActivity` formats it as a relative string ("3m ago").
2. **Sort:** `sortedByStateAndActivity` first sorts by state priority (waiting > active > compacting > idle), then by `lastActivityAt` descending.

No threshold-based transitions. State changes come from upstream events; activity timestamps are just metadata.

What this implies: the substrate's `last_event_at` on the session row (which is implicit in v2.1 — sessions get updated on every event) is the only signal a claude-status-class menu bar needs.

## Patterns across the tools

### Three distinct mechanisms in use

The seven tools collectively use three measurement approaches:

| Mechanism | Tools | Captures |
|---|---|---|
| **Single timestamp** | claude-status, Outworked | Time since last activity |
| **Sliding window of events** | tamagotchi (last 30 timestamps) | Events in last 60s, classified into tiers |
| **Leaky-bucket scalar** | claude-quest | Continuous 0..1 flow score |
| **Discrete state only** | OpenPets, AgentDeck, Pixel Agents (hooks mode), Outworked | No rate at all; state transitions are the signal |
| **Token consumption** | ccpet | Tokens/tick as a proxy for "is the user using Claude" |

Most tools (5 of 7) primarily use discrete states. Two compute a rate; one uses tokens as a proxy. Discrete-state is the dominant paradigm.

### Three timescales matter

When tools do compute rates or staleness, the timescales cluster:

| Timescale | Purpose | Tools using it |
|---|---|---|
| **5 seconds** | "Is the agent currently working right now?" | claude-quest grace period, Pixel Agents text-idle, tamagotchi update cadence |
| **1 minute** | "How intense is the activity?" | tamagotchi intensity calculation |
| **2-5 minutes** | "Has the user wandered off? Soft-warn." | Pixel Agents external-active, Outworked slow-threshold, tamagotchi session-gap |
| **10 minutes** | "Process is probably stuck or abandoned." | Pixel Agents global-discovery cutoff, Outworked stuck-threshold |

Five seconds, one minute, five minutes, ten minutes. These are remarkably consistent across tools.

### What gets driven by activity

Looking at what tools *do* with activity measurements:

| Use | Tools |
|---|---|
| Animation speed / sprite frame rate | tamagotchi (mood-driven anim), claude-quest (flow → effects) |
| Pet stat decay (hunger/energy) | tamagotchi, ccpet |
| Sleep / wake transitions | tamagotchi |
| Sort order in a list | claude-status |
| Display ("3m ago") | claude-status |
| Soft warning UI | Outworked (slow flag) |
| Abort button enablement | Outworked (stuck flag) |
| Pet death (none observed) | — |

Notably, **no tool surveyed makes the pet die from inactivity.** Sleeping yes (tamagotchi); decay yes (ccpet, tamagotchi); death no. Test case 2 in `13-test-cases.md` proposed dying-on-process-death (liveness signal), which is a different mechanism than "idle too long."

## Implications for the substrate

Walking these patterns through the v2.1 design and the `13-test-cases.md` proposal:

### Activity rate as a derived projection: is it really worth exposing?

The proposal in `13-test-cases.md` was to expose `recent_event_count_60s` and `recent_tool_count_60s` as derived columns + a `state.session.<id>.activity` topic. The argument was "8 inventoried tools want some form of 'how busy right now'."

Looking at what those 8 tools actually do, the picture is different:

- **ccpet**: doesn't want a rate; it wants token deltas since last tick. Already serviceable via `total_tokens` minus a presenter-held cursor.
- **tamagotchi**: maintains its own sliding window of timestamps; would consume `recent_event_count_60s` directly. **Yes, would use it.**
- **claude-quest**: maintains a leaky-bucket scalar; would use the substrate's event firehose directly and run its own bucket. The substrate's coarser counter is less useful to it.
- **OpenPets, AgentDeck**: don't use rate at all. Discrete state is sufficient.
- **Pixel Agents**: uses time-since-last-event, not rate. `last_event_at` is sufficient.
- **Outworked**: uses time-since-last-event with threshold checks. `last_event_at` is sufficient.
- **claude-status**: uses time-since-last-event. `last_event_at` is sufficient.
- **claude-lamp**: doesn't use rate (reacts to `current_state` transitions).

Of 8 candidates, **only tamagotchi actively wants a rate**. The rest want `last_event_at`, which v2.1 already implies. Tamagotchi could compute its own rate from event subscriptions if needed.

**Conclusion:** the proposed `recent_event_count_60s` derived projection is not foundational by the test from `07-agent-type-and-foundations.md`. It's a *single-tool convenience*, not a multi-tool primitive. Drop it from the v2.1 schema.

What stays in v2.1:
- `last_event_at` on the session row (already implicit; make it explicit)
- The events log itself, queryable with `since=<cursor>` and `kind=` filters
- Subscribe-able via `events.session.<id>.*` for tools that want to compute their own rate

What presenters do client-side:
- Sliding-window rate (tamagotchi pattern): maintain a deque of timestamps from event subscriptions
- Leaky-bucket scalar (claude-quest pattern): increment on event, decay on a render-frame timer
- Time-since-activity (claude-status pattern): just `Date.now() - last_event_at`
- Threshold checks (Outworked pattern): `setInterval` against `last_event_at`

All four are cheap client-side. The substrate doesn't need to pick one and impose it.

### The "context window fill" channel still matters

Even though activity rate doesn't survive the test, `context_percentage` does. Three tools surveyed read it (ccpet directly, tamagotchi indirectly via decay, the test-case sprite app for animation damping). It's a single derived field from statusline data — keep it as proposed.

Topic: `state.session.<id>.context` emitting on meaningful changes (10%, 25%, 50%, 75%, 90% buckets) is fine. Or just: emit on every change, presenters debounce client-side if they care. Simpler.

### `last_event_at` as the universal "is this active" signal

Five of eight tools key off this single field. v2.1 already maintains it (the session row updates on every event); just confirm it's exposed.

Pub/sub: there's no topic for "last_event_at changed" — it changes on every event by definition. Presenters that care use `events.session.<id>.*` directly, or fall back to polling `/sessions/:id` if they want a debounced version.

### Three threshold patterns the substrate could expose as configuration but shouldn't

Tempting additions that should stay out:

- **"Slow" threshold** (Outworked's 5min). Presenter-side. Different tools want different thresholds; the substrate doesn't pick.
- **"Stuck" threshold** (Outworked's 10min, Pixel Agents' 10min cleanup). Presenter-side, with one exception: a daemon sweep that promotes long-idle sessions to `lifecycle: abandoned` is reasonable. That's M7+, not v2.1.
- **"Sleep" threshold** (tamagotchi's 5min session-gap). Definitively presenter-side. Tamagotchi happens to use 5min; the 8-bit sprite app in case 2 used 5min; AgentDeck doesn't sleep at all. Each tool picks.

## Revised proposal: drop activity counters from v2.1

The original `13-test-cases.md` proposed adding `recent_event_count_60s` and `recent_tool_count_60s` to the schema. The survey shows this is single-tool convenience, not foundational. Drop the addition.

What `13-test-cases.md`'s case 2 (the 8-bit sprite app) actually needs:

1. ~~Activity counters~~ → **Event firehose subscription, presenter computes its own rate**
2. `remote_url` on session row — still needed (case 1's gap, real)
3. `context_percentage` on session row — still needed (3 surveyed tools use it)
4. `last_event_at` exposed — already in v2.1, just clarify it
5. Attachment liveness for process-death — already in v2.1

The presenter for case 2 grows by ~15 lines (the sliding window or leaky bucket implementation) but the substrate's surface stays smaller and more honest about what's foundational.

## What the survey didn't surface

Things I expected to find but didn't:

- **Per-tool weighting** — no tool surveyed weights different tool calls differently (a 30-second Bash run counting more than a 100ms Read). Activity is purely event-count-based across the board.
- **Activity-driven *workload* alerts** — no tool surveyed says "you've been at it too hard, take a break." All the decay/sleep mechanics are pet-side metaphors, not user-facing wellness features.
- **Cross-session aggregate activity** — no tool surveyed shows "total activity across all your sessions in the last hour." Each pet/sprite is per-session.

These are presenter ideas that no one has built yet. The substrate enables them but shouldn't pre-empt them.

## Specific edits this triggers

To `13-test-cases.md`:

- Remove the section proposing `recent_event_count_60s` as a foundational addition
- Reword case 2's "what it needs" to put activity rate as presenter-computed (with reference to this survey)
- Keep the `remote_url` and `context_percentage` additions

To `11-design-sketch-v2-1.md`:

- No changes — v2.1 didn't actually commit to activity counters; the proposal was in `13-test-cases.md`
- Reinforce `last_event_at` as part of the session row in section 5

To `12-mvp-and-milestones.md`:

- M3 scope unchanged in size (drops activity counters, gains `remote_url` — roughly even)
- Add a note that "activity rate" is explicitly presenter-side, with the survey as justification

## Closing observation

The survey clarifies an instinct that was right but unstated: **the substrate's job is to make event consumption cheap, not to compute every possible derived signal**. Pub/sub already makes event consumption cheap. Presenters that want activity rate get it for ~15 lines of code; presenters that want time-since-activity get it for one line. Both work without the substrate making a choice.

The original temptation in `13-test-cases.md` — "8 tools want this; expose it" — was based on a survey that hadn't happened yet. Now that it has, it turns out 7 of 8 want something simpler. The substrate stays smaller.