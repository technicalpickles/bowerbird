# Design sketch: unified state + event bus

A working sketch of the generalized state layer described in `01-findings.md` and `02-detailed-inventory.md`. This is not a spec — it's a thinking document. The purpose is to surface design decisions early, before any code.

## Core idea

Two API surfaces over a single store:

1. **State view** — the aggregated current truth, queryable by tools that just want to know "what's running right now." Cheap, read-mostly, eventually consistent.

2. **Event stream** — the append-only log of everything that happened, with both live subscription and historical query. The source of truth from which the state view is derived.

The state view is a projection of the event stream plus periodic reconciliation by a sweep. Tools that previously installed hooks, tailed JSONL, and reimplemented the same state machine get to subscribe instead.

```
                                ┌────────────────────────────┐
   Claude hooks  ──┐            │                            │
                   │            │   State view               │
   Statusline tap ─┼─► daemon ──┼─► (derived projection)     │
                   │            │                            │
   JSONL tail   ───┘            │   queryable: GET /sessions │
                                │                            │
   MCP self-report ────────────►│                            │
                                │                            │
   Sweep (periodic) ───────────►│   Event log                │
                                │   (canonical, append-only) │
                                │                            │
                                │   subscribable: WS /events │
                                │   queryable: GET /events?  │
                                └────────────────────────────┘
                                          │
                                          │
                            ┌─────────────┼──────────────┐
                            ▼             ▼              ▼
                          pets          dashboards     statuslines
                       (read-only,    (read + tail)  (read-only,
                        no hooks)                     no hooks)
```

## The two surfaces

### State view — "what's true right now"

This is what most consumers actually want. A pet doesn't care about the 47 `PreToolUse` events that led to the current `working` state. A statusline just wants `idle | thinking | working | waiting | done | error`.

```
GET /sessions
  -> [
       {
         session_id: "abc-123",
         project_dir: "/Users/josh/code/foo",
         repo_root: "/Users/josh/code/foo",        // canonical repo (null if not in a git repo)
         worktree: "/Users/josh/code/foo/.worktrees/feature-auth",  // null if not a worktree
         branch: "feature/auth",                   // current HEAD; null if not git
         lifecycle_status: "live" | "paused" | "abandoned" | "ended",
         current_state: "working",   // the OpenPets-style reaction enum
         current_tool: "Edit",
         model: "claude-sonnet-4-7",
         started_at: "...",
         last_event_at: "...",
         attachments: [
           { attachment_id: "...", started_at: "...", last_heartbeat_at: "...", alive: true,
             location: {
               host: "joshs-laptop",
               terminal: { program: "iTerm.app", session_id: "w0t1p0:F0…" },
               multiplexer: { kind: "tmux", session: "dev", pane: "%42" },
               ide: null,
               ssh: false
             }
           }
         ],
         agents: [
           { agent_id: "...", type: "main" | "subagent" | "teammate",
             status: "working" | "waiting" | "idle" | "completed" | "error",
             current_tool: "Edit", parent_agent_id: null },
           ...
         ],
         tokens: { input, output, cache },
         cost_usd: 0.42
       },
       ...
     ]

GET /sessions/:id           -> one session, same shape
GET /sessions/:id/agents    -> agent list for one session
GET /sessions?status=live   -> filter by lifecycle
GET /sessions?repo_root=/Users/josh/code/foo  -> all sessions on a repo, across worktrees
GET /sessions?branch=feature/auth             -> all sessions on a branch
```

Idempotent. Polling-friendly. ETag/If-None-Match for cheap "has anything changed."

### Event stream — "what happened"

Append-only log of normalized events. Source of truth.

```
GET /events?since=<cursor>&limit=N
  -> {
       events: [...],
       cursor: "next-cursor-here"
     }

WS /events                       # firehose of all events, live
WS /sessions/:id/events          # live tail of one session
GET /sessions/:id/events?since=<cursor>   # replay one session's history
```

The same `cursor` shape works for both historical query and live subscription. A consumer that disconnects can reconnect with the last cursor it saw and not miss anything. This is the Kafka / Postgres-logical-replication pattern: live tail is just a query that keeps streaming.

## Event vocabulary

A normalized union, drawing from Pixel Agents' `AgentEvent` plus the session/process model from `02-detailed-inventory.md`. Each event has:

```ts
{
  event_id: number,            // monotonic, the cursor key
  session_id: string,
  attachment_id: string | null,
  agent_id: string | null,
  timestamp: number,
  source: "hook" | "jsonl" | "mcp" | "statusline" | "sweep",
  kind: <see below>,
  ...kind-specific fields
}
```

The `kind` discriminator:

```
sessionStart       { source_hint?: string }                 // maps from SessionStart hook
sessionEnd         { reason: "clean" | "crash" | "timeout" | "replaced" }
attachmentOpen     { process_token: string }                // a process began driving this session
attachmentClose    { reason }                               // its heartbeat went stale or it called close
heartbeat          { source }                               // any signal of liveness, source = where it came from
userTurn                                                    // UserPromptSubmit
toolStart          { tool_id, tool_name, input? }
toolEnd            { tool_id, ok: boolean }
turnEnd                                                     // Stop
permissionRequest                                           // PermissionRequest or matching Notification
notification       { message }                              // catch-all for non-permission notifications
subagentStart      { parent_agent_id, agent_type, ... }
subagentEnd        { parent_agent_id, agent_id }
subagentTurnEnd    { parent_agent_id }
tokens             { input, output, cache, total }
cost               { usd }
preCompact         { reason? }
cwdChanged         { old_cwd, new_cwd, repo_root, worktree, branch }   // agent moved trees mid-session
worktreeCreate     { worktree_path, branch, base_branch?, repo_root }  // emitted by orchestrators (dmux etc.)
worktreeRemove     { worktree_path, repo_root }                        // emitted by orchestrators
agentMessage       { from_agent_id, to_agent_id, message_summary?, tool_id? }  // Agent Teams inboxes
reconcile          { what_changed }                          // emitted by sweep, see below
```

This is roughly Pixel Agents' `AgentEvent` plus explicit attachment events, worktree-lifecycle events, agent-team messages, and reconciliation events. The vocabulary is small enough to be agent-agnostic (Codex, OpenCode, Cursor map their hook formats into the same shape) and structured enough that the state view is mechanically derivable.

The worktree events come from **orchestrators** (dmux, ccmanager, vibe-kanban) — they emit them when they create or remove worktrees so observability tools see worktree appearance and removal as first-class events. The daemon doesn't manage worktrees; it just records events about them.

## State machine

The state view is computed by folding events. Logic outline:

**Session lifecycle:**
- `sessionStart` → row exists, `lifecycle_status = live` (assuming an attachment is also opening)
- `sessionEnd` → `lifecycle_status = ended`
- No attachment alive AND last event recent → `live` (still — this case shouldn't happen, but handle it)
- No attachment alive AND last event stale (e.g. >1min) AND JSONL still recent → `paused`
- No attachment alive AND last event very stale AND JSONL stale → `abandoned`

**Attachment lifecycle:**
- `attachmentOpen` → row exists, `alive = true`
- `heartbeat` → bump `last_heartbeat_at`
- `attachmentClose` → `alive = false`
- No heartbeat for >threshold → sweep marks `alive = false`, emits synthetic `attachmentClose { reason: "timeout" }`

**Agent state (per agent within session):**
- `userTurn` → main agent → `working`
- `toolStart` → agent → `working`, set `current_tool`
- `toolEnd` → clear `current_tool`
- `turnEnd` → agent → `idle` (or `completed` if session is ending)
- `permissionRequest` → agent → `waiting`
- Any event → clears `waiting` (heuristic: an active agent isn't waiting on you)

**Reaction enum (the small projection):**

The 11-value OpenPets vocabulary (`idle | thinking | working | editing | running | testing | waiting | waving | success | error | celebrating`) is a further projection of agent state + last tool name. Useful for pets and statuslines that want one word per session.

## The sweep

The sweep is what makes the state view trustworthy when hooks lie or go silent. It runs periodically (5–10s) and reconciles the projection against ground truth.

### What the sweep checks

In rough order of cost:

1. **JSONL mtime poll** — for each known session, stat its `~/.claude/projects/<encoded-cwd>/<session-id>.jsonl`. If mtime is fresher than `last_event_at`, the session is alive but events aren't reaching the daemon (hook crashed? handler down at the time?). Emit synthetic `reconcile` event noting the gap. If mtime is older than threshold, no process is writing — demote `lifecycle_status` accordingly.

2. **Heartbeat staleness check** — any attachment whose `last_heartbeat_at` is older than 2× the heartbeat interval is dead. Emit `attachmentClose { reason: "timeout" }`.

3. **Process check (optional, OS-specific)** — walk `ps` or `/proc` for `claude` processes. Match against attachment `process_token` if available. Detects crashes that don't get to fire `SessionEnd`.

4. **Tool-call timeout** — any `toolStart` without a corresponding `toolEnd` after Y seconds is suspect. Emit `reconcile` noting it.

### Sweep emits events, doesn't mutate state directly

This is the crucial discipline. The sweep doesn't `UPDATE sessions SET status = 'paused'` directly. It writes a `reconcile` or `attachmentClose` event into the event log. The same projection logic that handles real events handles synthetic ones.

Reasons:
- **Replayability** — you can reconstruct state from the log alone; sweep findings are visible in history
- **Source attribution** — every state change has a `source` field, so consumers can distinguish "the hook said this" from "the sweep inferred this"
- **Single state machine** — there's only one place that derives state from events; sweep is just another event source
- **Debuggability** — when a session shows up as `paused` and shouldn't, you grep the event log for the reconcile event and see why

### What the sweep does *not* do

- Does not delete data
- Does not "fix" sessions retroactively (no rewriting history)
- Does not interfere with live processes (read-only on the host)

## Hook installation: one line, fanout in the daemon

Today, every dashboard installs its own hook line in `~/.claude/settings.json`. Running ccam + Pixel Agents + tamagotchi means three handlers on the same `PreToolUse`. They don't conflict but they add latency and complexity.

The unified daemon installs **one hook entry per event type**, pointing at a tiny shim:

```jsonc
{
  "hooks": {
    "PreToolUse":       [{ "command": "claude-state-bus emit PreToolUse" }],
    "PostToolUse":      [{ "command": "claude-state-bus emit PostToolUse" }],
    "SessionStart":     [{ "command": "claude-state-bus emit SessionStart" }],
    "SessionEnd":       [{ "command": "claude-state-bus emit SessionEnd" }],
    "Stop":             [{ "command": "claude-state-bus emit Stop" }],
    "SubagentStop":     [{ "command": "claude-state-bus emit SubagentStop" }],
    "Notification":     [{ "command": "claude-state-bus emit Notification" }],
    "UserPromptSubmit": [{ "command": "claude-state-bus emit UserPromptSubmit" }],
    "PreCompact":       [{ "command": "claude-state-bus emit PreCompact" }]
  }
}
```

The `claude-state-bus emit <type>` shim:
- Reads the JSON payload from stdin
- POSTs to the local daemon (discovery via `~/.claude-state-bus/server.json`, same pattern as Pixel Agents)
- Exits 0 in <5ms regardless of daemon state (failsafe)

Subscribers register with the daemon over a local socket / WS / REST. They **never** touch `~/.claude/settings.json` themselves. This is the key architectural move: the single-tenant hook slot becomes a multi-tenant pub/sub.

Same idea for the statusline:

```jsonc
{
  "statusLine": {
    "type": "command",
    "command": "claude-state-bus statusline"
  }
}
```

The statusline shim:
- Reads the per-tick JSON
- Sends it to the daemon as a `heartbeat` + `tokens` event (statusline payload includes token usage)
- Asks the daemon "what segments should I render?"
- Each subscribed statusline-presenter contributes a segment
- Daemon composes them and the shim emits the combined line

## Heartbeat sources

The cleanest signal that a process is alive is the **statusline tick**. Claude Code calls the configured statusline command on every turn, with a fresh JSON payload, deterministically. If we register the unified statusline shim, we get a free heartbeat per turn per process — and the JSON includes session_id, model, cwd, and token counts.

Other heartbeat sources, in declining preference:
- **Statusline tick** (best — deterministic, free, includes useful payload)
- **Any hook event** (good — every PreToolUse is implicitly a heartbeat)
- **MCP keepalive** (good — works for agents we self-instrument with an MCP server, OpenPets-style)
- **JSONL mtime poll from the sweep** (fallback — works even when nothing is reporting)

The interesting consequence: if you install the unified statusline shim and a single `PreToolUse` hook, you get heartbeats on every turn and on every tool call, which is plenty of resolution to detect process death within ~10s.

## Where is this session running? — terminal & multiplexer attribution

A question presenters genuinely want to answer: *given a session, which terminal window / tmux pane / IDE is it actually running in right now?* This matters because:

- A desktop notification ("session X needs your attention") is more useful if you can click through to the right tmux pane, or at least display "iTerm window 2, pane 3"
- A pet/HUD that surfaces "you have 4 sessions running" is better when you can show "1 in VS Code, 3 in tmux session `dev`, pane %42"
- An orchestrator routing follow-up commands to the right session needs to know where to send them

**None of the surveyed tools currently capture this.** Pixel Agents has the closest equivalent — it binds to `vscode.Terminal` because it's a VS Code extension and gets the API for free. cmux organizes terminals but doesn't tie that organization back to which Claude session each one runs. Everyone else is blind to it.

The good news: the signals are abundant. Claude Code hooks inherit the parent shell's environment, so a hook script can read every relevant env var directly. The shim that POSTs to the daemon can capture an attribution fingerprint and attach it to the `attachmentOpen` event.

### Available signals

**Terminal emulator identification:**

| Variable                   | Set by                                          | Stable across reattach? |
|----------------------------|-------------------------------------------------|-------------------------|
| `TERM_PROGRAM`             | iTerm, vscode, Apple_Terminal, ghostty, WezTerm, Hyper, etc. | n/a (per session) |
| `TERM_PROGRAM_VERSION`     | same                                            | n/a |
| `TERM_SESSION_ID`          | iTerm                                           | **Yes** — persists across detach/reattach |
| `ITERM_SESSION_ID`         | iTerm (older)                                   | Yes |
| `KITTY_WINDOW_ID`          | kitty                                           | Yes (per kitty server) |
| `WEZTERM_PANE`             | WezTerm                                         | Yes |
| `WT_SESSION`               | Windows Terminal                                | Per-session UUID |
| `TERMINAL_EMULATOR`        | JetBrains IDEs                                  | n/a |
| `VSCODE_INJECTION` / `VSCODE_PID` | VS Code integrated terminal              | Per-window |

**Multiplexer identification:**

| Variable          | Set by  | Notes                                          |
|-------------------|---------|------------------------------------------------|
| `TMUX`            | tmux    | Path to the tmux socket — disambiguates multiple tmux servers |
| `TMUX_PANE`       | tmux    | Pane id like `%42`. Stable for pane lifetime; *not* across `tmux kill-server` |
| `STY`             | screen  | Screen session name like `12345.pts-0.host`   |
| `ZELLIJ`          | zellij  | Zellij session name                            |
| `ZELLIJ_PANE_ID`  | zellij  | Pane id                                        |

**Remote / SSH context:**

| Variable          | Notes                                   |
|-------------------|-----------------------------------------|
| `SSH_TTY`         | Set if shell is in an SSH session      |
| `SSH_CONNECTION`  | Format: `client_ip client_port server_ip server_port` |
| `SSH_CLIENT`      | `client_ip client_port server_port`    |

**OS-level:**

- `ttyname(0)` of stdin — the controlling tty, e.g. `/dev/ttys001`
- ppid chain — walk `/proc/<pid>/status` (Linux) or `ps -o ppid=` (macOS) up from the Claude process and look for recognizable ancestors (`tmux`, `zellij`, `Terminal`, `iTerm2`, `code`, `ghostty`)
- Hostname

### What to capture

Define an `AttachmentLocation` struct, captured once at `attachmentOpen` (the shim has full env access there) and stored on the attachment row:

```jsonc
{
  "host": "joshs-laptop",
  "tty": "/dev/ttys001",
  "ssh": false,
  "ssh_client_addr": null,         // populated when ssh=true
  "terminal": {
    "program": "iTerm.app",        // TERM_PROGRAM
    "version": "3.5.10",           // TERM_PROGRAM_VERSION
    "session_id": "w0t1p0:F0…"     // TERM_SESSION_ID — the stable handle for iTerm
  },
  "multiplexer": {
    "kind": "tmux",                // tmux | screen | zellij | null
    "socket": "/private/tmp/tmux-501/default",  // $TMUX, server-disambiguating
    "session": "dev",              // tmux session name
    "window": "1",                 // window index
    "pane": "%42"                  // $TMUX_PANE
  },
  "ide": {
    "kind": "vscode",              // vscode | jetbrains | null
    "pid": 12345,                  // VSCODE_PID where applicable
    "workspace": "/Users/josh/code/foo"  // best-effort from VSCODE_GIT_*, etc.
  },
  "ppid_chain": [
    { "pid": 71234, "name": "claude" },
    { "pid": 71200, "name": "zsh" },
    { "pid": 71198, "name": "tmux: server" },
    { "pid": 71001, "name": "iTerm2" }
  ]
}
```

Some fields will be null — that's fine. The fingerprint is best-effort. The presenter decides what to surface.

### Schema impact

The attachments table grows a `location` JSON column:

```sql
CREATE TABLE attachments (
  attachment_id     TEXT PRIMARY KEY,
  session_id        TEXT NOT NULL REFERENCES sessions(session_id),
  process_token     TEXT,
  location          TEXT,         -- JSON, the AttachmentLocation struct above
  started_at        INTEGER NOT NULL,
  last_heartbeat_at INTEGER NOT NULL,
  ended_at          INTEGER,
  end_reason        TEXT
);
```

The location is **per-attachment**, not per-session, because a single session can be resumed in different terminals at different times. `claude --resume <session-id>` from a different tmux pane next week is a new attachment with a new location, same session_id.

### Reattachment / stability gotchas

- **`TMUX_PANE` is not stable across `tmux kill-server`**, so a pane that was `%42` yesterday might be `%7` today. The location captures the pane id at attachment time — it's a *snapshot*, not a stable handle. If the presenter wants to "click through to that pane," it needs to verify the pane still exists when the click happens. tmux can answer: `tmux list-panes -aF '#{pane_id}'`.
- **`TERM_SESSION_ID` (iTerm) is stable across detach/reattach** — that's the handle to use if you want a presenter to "open the iTerm tab where this session is" and have it work after a laptop sleep cycle.
- **VS Code terminal handles are not externally addressable** — you can't tell VS Code from outside "focus terminal 3 of window 2." You can only know the session was started from VS Code.
- **Zellij and modern multiplexers** generally have stable IDs but vary; a per-multiplexer adapter knows the rules.

### "Click through to my session" — how presenters use this

Once the location is on the attachment row, presenters can do useful things:

- **Desktop notification** with "Open in iTerm" → shells out to `osascript` with `TERM_SESSION_ID`, or runs `tmux switch-client -t <session>:<window>.<pane>` with the multiplexer fields
- **Statusline / pet "you have 3 active sessions"** → groups by `multiplexer.session` and `terminal.program` to render "1 vscode, 2 in tmux:dev"
- **Orchestrator routing** — "send this follow-up to the same pane the user was working in"
- **Cross-machine view** (future, multi-host) — `host` + `terminal.program` distinguish sessions running on different workstations

### Implementation note: the shim does the work

The unified hook shim and statusline shim have access to the agent's full environment (they're spawned by Claude Code). When the shim sees the *first* event for a session_id (no existing attachment), it:

1. Reads the env vars listed above
2. Walks the ppid chain (cheap on Linux/macOS — three or four `stat` calls)
3. Constructs the `AttachmentLocation`
4. POSTs `attachmentOpen { location: {...} }` to the daemon

Subsequent events for the same `(session_id, process_token)` reuse the existing attachment and don't repeat the env capture — the shim just sends a `heartbeat` event. Cheap.

### Edge cases worth flagging

- **Subagents and teammates run in the same process** as the parent — they share the location. The attachment is per-process, not per-agent.
- **Background agents** (`run_in_background`) similarly share the parent process.
- **`claude` invoked from a script or CI** has no terminal — fingerprint is mostly null, which is also fine information.
- **Agents inside Docker / devcontainers** — the env will reflect the container, not the host. That's accurate, even if it might be confusing. Surface the hostname so the user can tell.
- **SSH inside tmux inside iTerm** — all three layers populate. `ssh: true` plus `multiplexer.kind: tmux` plus `terminal.program: iTerm.app` is the full picture.

## Worktrees, repos, and branches

Parallel agents in git worktrees are now the dominant pattern (every orchestrator — dmux, vibe-kanban, conductor, ccmanager, claude-squad — assumes it). The daemon needs to understand worktrees as first-class concepts because:

- A user running 5 sessions in 5 worktrees on the same repo wants them grouped as "5 sessions on `proj`," not as 5 unrelated `cwd` strings
- "Show me all sessions on the `feature/auth` branch" is a useful filter
- Orchestrators that create/remove worktrees should be able to broadcast that fact so observability tools track it without polling

**No surveyed observability tool currently does this.** Every state tool keys on `cwd` alone, so a worktree at `~/proj/.worktrees/feature-foo` and the main checkout at `~/proj` look like unrelated sessions. Only the tmux sidebar tools (tmux-agent-sidebar especially) handle worktrees in their data model, and only because they also create them.

### Derivation at session start

When the shim sees the first event for a new session, alongside the `AttachmentLocation` capture, it derives:

```
repo_root  = parent of `git -C "$cwd" rev-parse --git-common-dir`
worktree   = `git -C "$cwd" rev-parse --show-toplevel`
branch     = `git -C "$cwd" rev-parse --abbrev-ref HEAD`
```

`worktree == repo_root` when `cwd` is the main checkout. `worktree != repo_root` when it's a linked worktree. All three are null for sessions outside any git repo.

This is one git invocation per attachment (the three rev-parses can be combined into a single `git rev-parse --git-common-dir --show-toplevel --abbrev-ref HEAD` call). Cached by `cwd`; effectively free in the steady state.

### `cwdChanged` re-derivation

If the agent moves between trees mid-session (rare, but Claude Code allows it via `Bash` tool `cd`), the shim re-runs derivation and emits a `cwdChanged` event. Consumers update their grouping.

### Orchestrator-emitted worktree events

The daemon does *not* create or remove worktrees. It just records that someone did:

```
worktreeCreate { worktree_path, branch, base_branch, repo_root }
worktreeRemove { worktree_path, repo_root }
```

These come from orchestrators that spawn agents (dmux on `dmux n`, ccmanager on new-session, vibe-kanban on card-create). The orchestrator POSTs the event before launching the agent, so observers see "new worktree" → "new session in that worktree" in causal order.

### Subagent / pending-removal handling

A subtlety from tmux-agent-sidebar's implementation, worth borrowing: when `worktreeRemove` fires while subagents are still active in that worktree, the parent's pane-scoped state can't be safely wiped because children might still be writing. Their solution is a pending marker that defers cleanup until subagents stop.

The daemon should adopt the same pattern: a `worktreeRemove` event flags the worktree as "removing." Final cleanup of session/attachment rows tied to that worktree waits until all attachments close. This avoids races where an orchestrator races ahead and removes a worktree while subagents inside it are still emitting events.

### What this enables

```
# the kind of grouping that becomes possible
GET /sessions?repo_root=/Users/josh/proj

→ {
  repo: "/Users/josh/proj",
  worktrees: [
    { path: "/Users/josh/proj",                          branch: "main",         sessions: [{...}] },
    { path: "/Users/josh/proj/.worktrees/feature-auth",  branch: "feature/auth", sessions: [{...}] },
    { path: "/Users/josh/proj/.worktrees/feature-bill",  branch: "feature/billing", sessions: [{...}] }
  ]
}
```

Pets, HUDs, dashboards can render "3 sessions on `proj` — 1 on main, 2 on feature branches" without each reimplementing git plumbing. Orchestrators that already track their own worktrees can stop maintaining a parallel registry and read from the daemon. Observability tools gain awareness they never had.

### Edge cases

- **Submodules** — `git rev-parse --git-common-dir` on a submodule points into the parent's `.git/modules/<name>`. The "repo_root" will be the submodule's working dir, which is what users expect — sessions inside a submodule are sessions on the submodule, not the parent.
- **Bare repos** — `cwd` won't be a working tree. Derivation returns null for `worktree`. That's fine.
- **Non-git directories** — all three fields are null. Sessions outside any repo still work; they just don't get grouped.
- **Branch detached** — `--abbrev-ref HEAD` returns `HEAD`, which is honest. Consumers can render "(detached)" for that case.
- **Worktree moved/renamed mid-session** — rare, but possible. The session's recorded `worktree` is the path at session start; a `cwdChanged` event would emit a new path. Whether to update the historical row or just emit the event is a design call (probably: leave the historical row, trust the event log).

## Storage

SQLite, WAL mode. One database. Schema sketch:

```sql
CREATE TABLE sessions (
  session_id    TEXT PRIMARY KEY,
  project_dir   TEXT,                   -- the cwd at session start
  repo_root     TEXT,                   -- canonical repo path (parent of .git common-dir); same as project_dir for non-git
  worktree      TEXT,                   -- this session's worktree root; same as repo_root if it's the main worktree
  branch        TEXT,                   -- HEAD branch at session start; updated on CwdChanged
  model         TEXT,
  started_at    INTEGER NOT NULL,
  last_event_at INTEGER NOT NULL,
  ended_at      INTEGER,
  lifecycle     TEXT NOT NULL CHECK(lifecycle IN ('live','paused','abandoned','ended')),
  metadata      TEXT
);

CREATE INDEX idx_sessions_repo_root ON sessions(repo_root);
CREATE INDEX idx_sessions_branch    ON sessions(branch);

CREATE TABLE attachments (
  attachment_id     TEXT PRIMARY KEY,
  session_id        TEXT NOT NULL REFERENCES sessions(session_id),
  process_token     TEXT,         -- (pid, starttime) hash if available, otherwise null
  location          TEXT,         -- JSON: AttachmentLocation (terminal/multiplexer/ide/host fingerprint)
  started_at        INTEGER NOT NULL,
  last_heartbeat_at INTEGER NOT NULL,
  ended_at          INTEGER,
  end_reason        TEXT          -- 'clean' | 'crash' | 'timeout' | 'replaced' | null while live
);

CREATE TABLE agents (
  agent_id        TEXT PRIMARY KEY,
  session_id      TEXT NOT NULL REFERENCES sessions(session_id),
  parent_agent_id TEXT REFERENCES agents(agent_id),
  type            TEXT NOT NULL CHECK(type IN ('main','subagent','teammate')),
  status          TEXT NOT NULL CHECK(status IN ('idle','working','waiting','completed','error')),
  current_tool    TEXT,
  started_at      INTEGER NOT NULL,
  ended_at        INTEGER,
  metadata        TEXT
);

CREATE TABLE events (
  event_id     INTEGER PRIMARY KEY AUTOINCREMENT,  -- the cursor
  session_id   TEXT NOT NULL,
  attachment_id TEXT,
  agent_id     TEXT,
  timestamp    INTEGER NOT NULL,
  source       TEXT NOT NULL CHECK(source IN ('hook','jsonl','mcp','statusline','sweep')),
  kind         TEXT NOT NULL,
  payload      TEXT NOT NULL  -- JSON
);

CREATE INDEX idx_events_session_event ON events(session_id, event_id);
CREATE INDEX idx_events_timestamp     ON events(timestamp);
CREATE INDEX idx_sessions_lifecycle   ON sessions(lifecycle);
```

`event_id` doubles as the cursor. WS subscribers receive events in `event_id` order; reconnection with `since=<event_id>` resumes from there.

The `events` table is the canonical source. The `sessions`, `attachments`, and `agents` tables are derived projections — could be regenerated from events alone (write a `rebuild-projections` command for debugging and migrations).

## Open questions

These are intentionally not answered yet. They're the design forks where reasonable people would disagree.

1. **Multi-host?** A single laptop is one daemon. SSH-attached terminals running Claude Code on a remote host: do their hooks ship events to the local daemon, or does each host run its own? Marc Nuri's dashboard solved this with a heartbeat model and a central server. We could go either way; local-only is simpler.

2. **Auth and permissions?** OpenPets has a per-run token in the discovery file. Pixel Agents has an HTTP auth header. ccam has same-origin guards. For a localhost daemon talking to localhost subscribers, a per-run token (rotates per daemon start) is probably enough.

3. **Event retention?** Forever (event log as system of record), some window (90 days like ccpet), or configurable? Forever is fine for personal use; bounded retention matters for shared deployments.

4. **Where do consumers register?** Two options: a daemon-side config file listing subscribers, or runtime registration via WS/socket. Runtime is more flexible (subscribers come and go) but harder to make survive daemon restarts. Probably both — config file for static subscribers, runtime registration for ephemeral ones.

5. **What about Claude Code's native `~/.claude/teams/` directory?** claude-team-dashboard reads it directly. The daemon could also watch it and emit synthetic events for team membership changes, so teams become first-class without needing new hook events.

6. **Cross-tool consensus on the event vocabulary.** Pixel Agents has `AgentEvent`. OpenPets has the reaction enum. ccam has its hook→state transitions. None of these are identical. Adopting the daemon means converging on a shared vocabulary, which is a coordination problem more than a technical one.

7. **What language/runtime?** Node is the path of least resistance (matches Pixel Agents, ccam). Rust or Go avoid every-dashboard-ships-its-own-runtime. Probably worth the cost for the daemon if this is supposed to be infrastructure that lots of tools depend on.

## What this would let go away

If the daemon existed and Pixel Agents, ccam, OpenPets, ccpet, tamagotchi, and the rest adopted it as a backend:

- **Hook collision** — gone. One shim. Daemon does fanout.
- **Statusline collision** — gone. One shim. Composed segments.
- **State machine reimplementation** — gone. One projection.
- **Stale-session heuristics in 5 different places** — gone. One sweep.
- **JSONL tailing in 3 different places** — gone. One source.
- **Multi-window cooperation logic** (Pixel Agents' `server.json` PID dance) — gone. There's only one daemon per machine.
- **Per-tool installer scripts that mutate `~/.claude/settings.json`** — gone. Daemon owns the slot.

What presenters become:

- **Pets** — subscribe to `WS /sessions/:id/events`, filter for state changes, render. ~50 lines of code.
- **Statuslines** — register a segment provider with the daemon, return a string per tick. ~30 lines.
- **Dashboards** — `GET /sessions` for the list view, `WS /events` for the live feed. Like writing a frontend against any backend.
- **Analytics** — `GET /events?since=...` paginated, run whatever you want over the log.

That's the prize.