# Dogfooding feedback

Raw observations from running bowerbird against the maintainer's own Claude Code
sessions. This is the catch-net *before* the formal process: an entry here is a
real-usage signal, not a decision. Entries graduate into a
[sprint-change-proposal](bmad/planning-artifacts/), a
[deferred-work](bmad/implementation-artifacts/deferred-work.md) item, or the
[no-list](no-list.md) once they've been weighed. Until then they live here so
the signal isn't lost.

Distinct from `deferred-work.md`, which captures code-review and story-scoped
follow-ups with file references. This file captures things that only show up
when you actually *use* the thing.

## Entry format

Each entry: a date, the session it came from, what happened, and what (if
anything) the code says about it. Findings are framed as observations, not
prescriptions — the fix is a downstream decision.

---

## 2026-06-01 — daemon down after a reboot

**Session:** `ad3eaed4-af27-4bb0-9844-f0e237defbc1` · **Branch:** main

The workstation rebooted. On the first Claude Code session afterward, every tool
call stacked a hook error in the transcript:

```
PreToolUse:Bash hook error    Failed with non-blocking status code: No stderr output
PostToolUse:Bash hook error   Failed with non-blocking status code: No stderr output
PreToolUse:Read hook error    Failed with non-blocking status code: No stderr output
PostToolUse:Read hook error   Failed with non-blocking status code: No stderr output
```

`bowerbird-daemon` was not running, so `~/.bowerbird/ingest.sock` did not exist.
`shim.log` showed a burst of `connect to ingest socket ... failed: No such file
or directory (os error 2)` spanning ~90 seconds (14:32:20 → 14:33:46 UTC) until
the daemon came back (fresh pid file + socket at 14:33). During that window
every Pre/PostToolUse event was dropped. The tool calls themselves ran fine —
the shim does not block — but nothing was recorded.

Two distinct findings came out of this.

### Finding 1 — nothing supervises the daemon, so a reboot opens a silent drop window

The daemon was not under launchd or pitchfork. After the reboot it came back
only because something restarted it manually; there is no automatic recovery.
Every event in the gap between boot and that restart is gone.

The [no-list](no-list.md) does not cover daemon supervision or
start-on-login, so this is not an intentional V1 cut — just unhandled. Open
question for whoever picks this up: is a launchd LaunchAgent (installed by
`bowerbird install`) the right shape, or does the shim lazily spawn the daemon
on a failed connect? The shim is currently a pure thin client
(`crates/shim/src/socket.rs`) and does not spawn anything.

Related: Story 1.6 (hook-unreliability-tolerance) makes the *projection* robust
to dropped events, but robustness-to-drops is not the same as
not-dropping-on-every-boot.

### Finding 2 — the surfaced hook error is alarming and causeless

This is the one that actually bit the session. The shim's exit-code contract
(`crates/shim/src/error.rs::exit_code`) is deliberate:

- `Error::Connect` (daemon unreachable) → **exit 1**, "surfaces a real failure"
- mid-write / daemon-answered-with-error → **exit 0**, fire-and-forget (NFR20)
- exit 2 is forbidden (it would block the tool call)

So exiting 1 when the daemon is down is *intentional* — the author wants Claude
Code to surface that something is wrong (`main.rs:16`: exit 1 is "the closest
signal Claude Code will pick up"). The intent is right. The execution is the
problem: the **only** diagnostic the shim writes is one line to
`~/.bowerbird/shim.log` (`main.rs:28`). It never writes stderr. Claude Code, on
a non-zero hook exit with no stderr, renders the generic
`hook error / Failed with non-blocking status code: No stderr output` — which
names neither bowerbird nor the daemon, and repeats on *every* tool call for the
whole outage.

Worst of both worlds: noisy enough to alarm, mute enough to be useless.

Likely fix direction (a decision, not a commitment): on the exit-1 path, also
emit one human line to stderr — e.g. `bowerbird: daemon not running, event
dropped (see ~/.bowerbird/shim.log)`. That keeps the deliberate exit-1 surface
while making the surfaced message name the cause. A secondary question is
whether a per-call error for an outage that spans dozens of calls should be
rate-limited or coalesced, or whether exit 0 + stderr is the better contract
once the daemon-down case is distinguishable from a genuine shim bug.

### For reference — two failure modes seen in shim.log history

- **`os error 2` (No such file or directory)** — daemon down, socket gone. The
  reboot case above.
- **`os error 35` (EAGAIN / resource temporarily unavailable)** — daemon up but
  the socket write timed out / was busy. Transient, scattered across prior days.

---

## 2026-06-01: presenters can only triage on what the wire carries

**Session:** pickletown web `/sessions` triage-radar build · **Branch:** main (pickletown web, with bowerbird-deck as the reference presenter)

Built the pickletown web `/sessions` page into a triage radar (state filter
chips, attention-first ordering, hide-ended-by-default) and used bowerbird-deck's
live output as the reference. Three asks surfaced, all the same shape: the
presenter wants to answer "what needs me, and where," but the wire only carries
mechanical state. None of these are bugs. They are the edges you feel when you
actually try to triage from the substrate rather than just display it.

Concrete signal from a live deck snapshot: five `WaitingInput` sessions aged 7m
to 24m sitting *below* two ~50s `Working` sessions (pure newest-first buries the
session that has waited longest), and a `… 134 ended hidden` footer.

### Finding 1: no per-session cwd / repo, so the most natural triage filter is impossible

The first question a multi-session human asks is "which repo or directory is the
one waiting on me?" The wire (`SessionState`, `SessionListItem`, `Event`) carries
`source`, `session_id`, `current_state`, `last_event_kind`, `last_event_at_ms`,
`last_pid`, and the verbatim event payload, but nothing about where the session
runs. A presenter cannot group or filter by repo because the daemon never sees
the cwd. Not on the [no-list](no-list.md), so this reads as unhandled rather than
an intentional cut. Open question for whoever picks it up: does a cwd field
belong on the state projection (it could ride the same envelope path that already
carries `bowerbird_ppid`), or is it a `PreToolUse`-payload-derived value a
presenter should extract itself?

### Finding 2: sessions are only identifiable by an 8-char id hash

Cards and rows key on the last 8 characters of the `session_id` UUID. There is
nothing human-recognizable to tell two sessions apart at a glance. This brushes
against the no-list's "No personas / agent-roles abstraction" cut, and rightly
so: a role or persona model is presenter-level interpretation, not a daemon
responsibility. The distinction worth recording is narrower. A presenter can
only label by data that is on the wire, and today there is none a human
recognizes (no cwd, no first prompt, no branch). So this is Finding 1 seen from
the labeling side: the intentional cut is the persona model, not the raw fact a
label could be built from.

### Finding 3: Ended never evicts, so every presenter re-implements hide-ended client-side

`Ended` is non-terminal by design (ADR 0004: `claude --resume` revives a
session), so the daemon legitimately cannot delete on death, and
`SELECT_NON_SENTINEL_SESSIONS` returns the full history. bowerbird-deck hides
ended behind its `a` toggle; the web page now does the same with a default-off
filter chip. That is two presenters independently solving the same problem, and
the "134 ended hidden" count shows it is real, not theoretical. Already tracked
as pickletown bean gt-3cnt. The part that bites presenters directly: a
server-side `?state=` / `?since=` filter on `GET /sessions` would let presenters
stop re-implementing this client-side and would bound the snapshot-on-subscribe
burst at reconnect; a retention sweep is the other half. Whether it is one, the
other, or both is the gt-3cnt decision.
