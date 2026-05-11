# Two test cases: TUI grouped by remote, and an 8-bit web app

These two cases stress different parts of v2.1. The exercise is to walk each through the current design and find where it works cleanly, where it stretches, and where it has actual gaps.

## Case 1: TUI grouped by git remote

Spec, restated:

- Display agents grouped by git remote URL (so all sessions on `github.com/foo/bar` appear together, regardless of which worktree they're in)
- Show per-agent state: idle, waiting for input, or working
- Live updates as state changes

### What v2.1 provides

The presenter's natural shape:

```
on startup:
  GET /sessions                       → populate initial list
  SUBSCRIBE state.sessions.added      → new sessions appear
  SUBSCRIBE state.sessions.removed    → sessions end

for each session in the list:
  SUBSCRIBE state.session.<id>.current_state

render loop:
  group sessions by <some-remote-key>
  for each group, render header + agent rows
  each row: indicator color based on current_state
```

The reaction enum maps to the three display states cleanly:

| reaction enum value | TUI display |
|---|---|
| `idle`, `success`, `celebrating` | idle (green/grey) |
| `waiting` | waiting for input (yellow/blinking) |
| `thinking`, `working`, `editing`, `running`, `testing`, `waving` | working (blue/animated) |
| `error` | error (red) |

Eleven daemon values collapse to four presenter states. The presenter does the collapse; the substrate doesn't try to anticipate it. This is the principle from `07-agent-type-and-foundations.md` working as designed.

### The gap: remote URL isn't in the schema

The substrate derives `repo_root`, `worktree`, `branch` (per M3). It does *not* derive remote URL. Two presenters in these cases both need it; without it they each `git config --get remote.origin.url` themselves, redundantly, against the same repos.

This is foundational by the test:

> Is this native git data that presenters need but currently can't access without reimplementing ingestion?

Yes. The substrate has already committed to running `git` once at session start to derive `repo_root`/`worktree`/`branch`. Adding `remote_url` to that same call costs nothing.

**Proposed addition** to the v2.1 design:

```sql
ALTER TABLE sessions ADD COLUMN remote_url TEXT;
CREATE INDEX idx_sessions_remote_url ON sessions(remote_url);
```

Derivation logic at session start, in the same git invocation as the other fields:

```rust
// Combine into one git call for efficiency
let output = git_rev_parse(cwd, &[
    "--git-common-dir",
    "--show-toplevel",
    "--abbrev-ref", "HEAD",
])?;
let remote = git_config(cwd, "remote.origin.url")?;  // separate call, also cached
```

Normalization: convert `git@github.com:foo/bar.git`, `https://github.com/foo/bar.git`, `https://github.com/foo/bar` all to the same canonical form. tmux-agent-sidebar's `normalize_git_url` is the reference implementation. Either drop the protocol/credentials or keep both — open question, probably canonical `<host>/<owner>/<repo>` is the right key for grouping.

API additions:

```
GET /sessions?remote_url=<canonical>
```

Plus a useful aggregation that comes naturally:

```
GET /remotes
  → [
      { remote_url: "github.com/foo/bar", session_count: 3,
        repo_roots: ["/Users/josh/foo-main", "/Users/josh/foo-wt"] },
      { remote_url: "github.com/foo/baz", session_count: 1,
        repo_roots: ["/Users/josh/baz"] },
      { remote_url: null, session_count: 1,    // sessions outside any git repo
        repo_roots: [] }
    ]
```

The TUI hits `/remotes` once for grouping headers, then per session it subscribes to `state.session.<id>.current_state`.

### The presenter, in full

With the addition above, the TUI is roughly:

```ts
type Session = {
  session_id: string,
  remote_url: string | null,
  worktree: string,
  branch: string,
  current_state: ReactionState,
  // ...
};

const sessions: Map<string, Session> = new Map();
const stateSubscriptions: Map<string, () => void> = new Map();

// Initial load
const initial = await fetch("/sessions").then(r => r.json());
for (const s of initial) {
  sessions.set(s.session_id, s);
  subscribeToState(s.session_id);
}

// Subscribe to new/removed
ws.subscribe("state.sessions.added", (frame) => {
  sessions.set(frame.session.session_id, frame.session);
  subscribeToState(frame.session.session_id);
  render();
});

ws.subscribe("state.sessions.removed", (frame) => {
  sessions.delete(frame.session_id);
  stateSubscriptions.get(frame.session_id)?.();  // unsubscribe
  stateSubscriptions.delete(frame.session_id);
  render();
});

function subscribeToState(id: string) {
  const unsub = ws.subscribe(`state.session.${id}.current_state`, (frame) => {
    const s = sessions.get(id);
    if (s) {
      s.current_state = frame.change.new;
      render();
    }
  });
  stateSubscriptions.set(id, unsub);
}

function render() {
  const grouped = groupBy([...sessions.values()], s => s.remote_url ?? "(no remote)");
  for (const [remote, rows] of grouped) {
    printHeader(remote);
    for (const r of rows) printRow(r);
  }
}
```

That's ~60 lines of presenter code. The state-machine, the projection, the git derivation, the cross-session correlation — all done by the substrate.

### Verdict for case 1

**Works with one schema addition** (`remote_url`). The pub/sub model is exactly the right shape. The reaction enum collapses cleanly to the four display states.

The addition is small and foundational — adding it to the design is the right call.

---

## Case 2: 8-bit animated web app

Spec, restated:

- One 8-bit sprite per agent
- Colored by git remote
- Animation rate scales with how active the agent is
- Animation rate also reflects context window fill
- Sleep when idle long enough
- Die when the process dies

This case is harder. It surfaces three distinct questions:

### Question 1: What's an "agent" here?

The spec says "each agent running." Two readings:

- **One sprite per Claude session.** The main agent. Subagents come and go too quickly for stable sprite identity.
- **One sprite per logical agent, including persistent subagent identities.** PAI-style — a researcher sprite, an engineer sprite — but tied to the `agent_type`, not per-invocation.

The substrate supports either. The session-level reading is simpler and matches every existing pet-style tool (OpenPets, ccpet, claude-lamp). The agent-level reading is what Outworked and AgentDeck use for their employee/creature visualizations.

For this exercise I'll assume **session-level** — one sprite per running Claude session. If subagents need their own sprites, the same pattern works with `state.session.<id>.agent.<agent_type>` topics.

### Question 2: Color by remote — same as case 1

Same gap, same fix. Once `remote_url` is on the session row, the sprite colorizer is a hash function from URL to color palette. Presenter side; no daemon involvement beyond exposing the field.

### Question 3: Animation rate by activity + context window

"How active is this agent right now" isn't a derived field — and after surveying actual tools (see `14-activity-survey.md`), it shouldn't be.

The initial instinct was to expose `recent_event_count_60s` as a derived projection because "8 inventoried tools want some form of 'how active is this.'" That turned out to be wrong on closer inspection. The survey of 8 pet/sprite/dashboard tools (ccpet, tamagotchi, OpenPets, Pixel Agents, Outworked, AgentDeck, claude-quest, claude-status) showed:

- **Only 1 tool** (tamagotchi) actively uses a window-based event-count rate
- **5 of 8** key off `last_event_at` alone (claude-status, Outworked, Pixel Agents, AgentDeck, claude-lamp)
- **1** (claude-quest) uses a leaky-bucket scalar — computes its own from events
- **1** (ccpet) uses tokens-per-tick, not event-rate
- **OpenPets and AgentDeck** are purely state-driven, no rate at all

Exposing `recent_event_count_60s` as a substrate primitive would be single-tool convenience, not a foundational aggregation. The right shape: expose `last_event_at` as the universal signal, expose the EVENTS channel for subscription, let presenters compute their own rate in ~15 lines when they need one.

**What the substrate exposes:**

- `last_event_at` on the session row (single timestamp, updated on every event)
- `events.session.<id>.*` subscription for presenters that want fine-grained rate computation
- `context_percentage` on the session row (from statusline data, M3 scope)
- `state.session.<id>.context` topic emitting on bucket changes (10%, 25%, 50%, 75%, 90%)

**What this case's presenter does:**

Subscribes to `events.session.<id>.*` and maintains a deque of timestamps (tamagotchi pattern), or a leaky-bucket scalar (claude-quest pattern). About 15 lines.

```ts
// Sliding-window pattern (tamagotchi-style)
const recentEvents: number[] = [];
const WINDOW_MS = 60000;

ws.subscribe(`events.session.${id}.*`, () => {
  const now = Date.now();
  recentEvents.push(now);
  while (recentEvents[0] < now - WINDOW_MS) recentEvents.shift();
});

function activityScore(): number {
  // recentEvents.length / 60 events/sec, normalized to 0..1
  return Math.min(1, recentEvents.length / 20);  // 20+ events in 60s = full speed
}
```

Or:

```ts
// Leaky-bucket pattern (claude-quest-style)
let flow = 0;
let lastEventAt = 0;

ws.subscribe(`events.session.${id}.*`, () => {
  flow = Math.min(1, flow + 0.05);
  lastEventAt = Date.now();
});

// Called on render frame
function decayFlow(dt: number) {
  if (Date.now() - lastEventAt > 5000) {
    flow = Math.max(0, flow - dt * 0.03);
  }
  return flow;
}
```

Both work without daemon-side computation. Pick whichever feels right; the substrate doesn't impose.

### Question 4: Sleep on idle, die on process death

These map cleanly:

**Idle policy (sleep):** presenter side. Subscribe to `state.session.<id>.current_state`. When state goes to `idle`, start a presenter-side timer (say 5 minutes). When timer fires, transition the sprite to sleeping. If state changes back to anything non-idle before the timer fires, cancel the timer.

The substrate provides the *signal*; the policy is presenter's. Good design — different presenters want different idle thresholds, and the substrate stays opinion-free.

**Process death (die):** subscribe to `state.session.<id>.attachment`. When `liveness` goes `alive` → `exited`, trigger the death animation. The liveness split from v2.1 (separate from lifecycle) is what makes this clean.

There's a subtlety: what if Claude crashes (process dies) but the user might `claude --resume`? `liveness: exited` doesn't necessarily mean "dead forever." For the sprite, "dead" might mean both `liveness: exited` AND time elapsed (say 5 minutes with no resume).

This is again presenter policy. The substrate provides:
- `state.session.<id>.attachment` (liveness changes)
- `state.session.<id>.lifecycle` (live → paused → abandoned → ended)

The sprite presenter could die on `lifecycle: ended`, ghost on `lifecycle: abandoned`, sleep on `lifecycle: paused` + extended idle. Or any other policy. The substrate doesn't dictate.

### The presenter, in full

```ts
type Sprite = {
  session_id: string,
  remote_url: string | null,
  state: ReactionState,
  context_pct: number,
  recent_events: number[],       // sliding window of event timestamps, last 60s
  liveness: "alive" | "exited" | "unknown",
  idle_since: number | null,     // ms timestamp when state went idle
  animation_phase: "moving" | "slowing" | "sleeping" | "dying" | "dead",
};

const sprites: Map<string, Sprite> = new Map();
const palette = colorPaletteFromUrl;  // hash fn, presenter-side
const ACTIVITY_WINDOW_MS = 60000;
const FULL_SPEED_EVENT_COUNT = 20;

function colorFor(s: Sprite): Color {
  return palette(s.remote_url ?? "default");
}

function activityScore(s: Sprite): number {
  // Drop events outside the window before scoring
  const cutoff = Date.now() - ACTIVITY_WINDOW_MS;
  while (s.recent_events.length && s.recent_events[0] < cutoff) {
    s.recent_events.shift();
  }
  return Math.min(1, s.recent_events.length / FULL_SPEED_EVENT_COUNT);
}

function animationSpeedFor(s: Sprite): number {
  if (s.animation_phase === "dead") return 0;
  if (s.animation_phase === "sleeping") return 0.1;  // gentle breathing
  const base = activityScore(s);                     // 0..1, presenter-computed
  const contextDamper = 1 - s.context_pct * 0.5;     // slow as context fills
  return base * contextDamper;
}

// ── subscribe ─────────────────────────────────────────────────────────

ws.subscribe("state.sessions.added", frame => {
  const s = makeSprite(frame.session);
  sprites.set(s.session_id, s);
  subscribeAll(s.session_id);
});

ws.subscribe("state.sessions.removed", frame => {
  // session ended cleanly; sprite has already died via lifecycle change
  // sweep stale subscriptions
  sprites.delete(frame.session_id);
});

function subscribeAll(id: string) {
  ws.subscribe(`state.session.${id}.current_state`, frame => {
    const s = sprites.get(id);
    if (!s) return;
    s.state = frame.change.new;
    if (s.state === "idle") {
      s.idle_since = Date.now();
    } else {
      s.idle_since = null;
      s.animation_phase = "moving";
    }
  });

  // Activity rate computed presenter-side from the event firehose
  // (~6 lines for the sliding-window pattern, see 14-activity-survey.md)
  ws.subscribe(`events.session.${id}.*`, () => {
    const s = sprites.get(id);
    if (!s) return;
    s.recent_events.push(Date.now());
  });

  ws.subscribe(`state.session.${id}.context`, frame => {
    const s = sprites.get(id);
    if (!s) return;
    s.context_pct = frame.change.new / 100;
  });

  ws.subscribe(`state.session.${id}.attachment`, frame => {
    const s = sprites.get(id);
    if (!s) return;
    s.liveness = frame.change.new.liveness;
    if (s.liveness === "exited") {
      s.animation_phase = "dying";
      setTimeout(() => { s.animation_phase = "dead"; }, DEATH_ANIM_MS);
    }
  });
}

// ── presenter-side idle policy ────────────────────────────────────────

setInterval(() => {
  const now = Date.now();
  for (const s of sprites.values()) {
    if (s.animation_phase === "dead" || s.animation_phase === "dying") continue;
    if (s.idle_since && now - s.idle_since > IDLE_THRESHOLD_MS) {
      s.animation_phase = "sleeping";
    }
  }
}, 5000);

// ── render loop (requestAnimationFrame) ───────────────────────────────

function frame() {
  for (const s of sprites.values()) {
    drawSprite(s, colorFor(s), animationSpeedFor(s));
  }
  requestAnimationFrame(frame);
}
```

That's ~120 lines. Five subscriptions per session, four per-session state fields, presenter-side animation logic, presenter-side rate computation.

### Verdict for case 2

Works with two additions:

1. **`remote_url` on the session row** (same as case 1)
2. **`context_percentage` on the session row** (already implied by v2 from statusline data; M3 timeline)

Plus the existing `attachment.liveness`, `current_state`, `lifecycle`, and `last_event_at` signals, which were already in v2.1.

The presenter's animation logic, idle threshold, death policy, **and activity rate computation** all stay client-side. The substrate provides clean transition signals and the event firehose; the presenter decides what they mean visually.

The activity-rate question was the most interesting one in this case, and the initial instinct (expose a counter) didn't survive contact with the survey of 8 actual tools — see `14-activity-survey.md`. Only tamagotchi wants a window-based rate; most tools key off `last_event_at` or use state transitions directly. Daemon-side counters would be single-tool convenience.

---

## What both cases surfaced

### One clear addition: `remote_url` on the session row

Both presenters need it. Multiple inventoried tools would too (any per-repo aggregation, including tmux-agent-sidebar which already reimplements it). It's pure git derivation, same family as `repo_root` and `branch`. Adding it to the M3 worktree-derivation work is the right call. ~30 lines including normalization.

### One non-addition: activity rate counters

The initial proposal in this document was to expose `recent_event_count_60s` and `recent_tool_count_60s` as derived projections. After surveying actual usage in `14-activity-survey.md`, this was reversed:

- Only 1 of 8 surveyed tools (tamagotchi) actively wants a window-based rate
- 5 of 8 key off `last_event_at` (single timestamp) — claude-status, Outworked, Pixel Agents, AgentDeck-class, claude-lamp
- 1 (claude-quest) uses a leaky-bucket scalar computed from its own JSONL tail
- 1 (ccpet) uses tokens-per-tick from JSONL, not event rate
- 2 (OpenPets, AgentDeck) use no rate at all — pure discrete state

Exposing the counter would have been single-tool convenience disguised as a multi-tool primitive. The substrate exposes `last_event_at` (universal signal) and the EVENTS channel (for the one tool that wants windowed rate). Presenters compute their own rate in ~6-15 lines using documented patterns.

### One non-addition: idle threshold and death policy

These came up but stay presenter-side. The substrate provides `current_state`, `lifecycle`, `attachment.liveness`, `last_event_at`. What "long enough" idle means and what "dies" means are presenter decisions. The 8-bit sprite app might sleep at 5 minutes idle; a more aggressive Tamagotchi might decay at 1 minute. The substrate doesn't pick.

### What the cases didn't surface

Things I expected to surface but didn't:

- **HITL backflow.** Neither case needs it. The TUI shows waiting state; doesn't approve. The web app animates a sprite; doesn't take input.
- **LAN reachability.** Both presenters are local (TUI runs in terminal, web app runs in browser against localhost daemon).
- **Cross-source presentation.** Neither case explicitly says "across Claude and Codex." If they did, they'd work — the `source` field is already a top-level event column, and the schema is `(source, session_id)` keyed.
- **Capabilities checks.** Neither presenter renders agent-specific UI like mode switching or permission options. The capabilities matrix matters more for richer presenters (AgentDeck-class).

### What this exercise validated

The v2.1 design's load-bearing claims all held:

1. Pub/sub topics are the right shape (`state.session.<id>.current_state`, `.context`, `.attachment` all map directly to what these presenters subscribe to)
2. The reaction enum projection is correct — both presenters collapse the 11 values cleanly to their display states without disagreement
3. Liveness separated from lifecycle is meaningful — the sprite app's "die when process dies" policy is exactly what the split is for
4. Snapshot-on-subscribe handles startup-population without separate `GET` calls (though the TUI still needs `GET /sessions` for the initial *list*; per-session state comes via snapshot frames)

And the substrate stayed out of presenter decisions: idle thresholds, death policy, animation curves, color palettes, grouping logic, **activity rate computation**. All client-side.

One addition surfaced and held: `remote_url`. One nearly surfaced and was rejected on further inspection: activity counters (see `14-activity-survey.md`). The discipline of testing proposals against actual tool behavior before committing is what made the difference.

## Updates to the MVP / milestone plan

These cases don't change MVP scope — both can wait for M3. They tighten what M3 needs to deliver:

- M3 adds `repo_root`, `worktree`, `branch`, **and `remote_url`** to the sessions table. One git derivation pass; ~30 extra lines of code.
- M3 adds `context_percentage` to the session row (was already implied for v2 from statusline data; pinning it here as part of M3 scope).
- M3 **does not** add activity counters. Originally proposed in an earlier draft of this document; reversed after the activity survey (`14-activity-survey.md`) showed only 1 of 8 surveyed tools actually wants a window-based rate. Presenters compute their own rate from the EVENTS firehose in 6-15 lines.

After M3, both test cases are buildable in ~60-120 lines each. That's the bar — if presenters of this complexity take more than ~200 lines, the substrate is too low-level. If they take much less, the substrate may be doing too much.

A third test case worth running through later: a multi-agent (Claude + Codex) version of the same TUI. Would surface the cross-source ordering and capability negotiation concerns more sharply, and validate M4. Worth doing once M4 is sketched concretely.