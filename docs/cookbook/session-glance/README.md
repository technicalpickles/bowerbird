# session-glance

## What this is

One command that prints every live session, grouped by repository, with each session's state and age, and then exits. It is the fallback for a notification you missed: instead of cycling tmux panes to find out which agent is blocked, you ask once and get the whole picture.

The runnable code in [`src/index.ts`](src/index.ts) is the canonical consumer of the server-side session filter (`GET /sessions?state=`) and the first presenter to derive both of the Story 5.7 additions. That matters because of Axiom 4 in project-context.md: the daemon ships mechanical facts (a verbatim `cwd`, an epoch-ms `started_at`) and deliberately refuses to derive "repo" or "age" itself. Those derivations are the presenter's job, and this entry is where bowerbird writes them down.

Two things here are contracts other tools bind to, not implementation detail: the output shapes under `## Run it`, and the `deriveRepo` rule under `## How it works`.

## Run it

Requires Node 22.6+ for `--experimental-strip-types`. On Node before 22.18 that flag also prints an `ExperimentalWarning` pair on stderr; it is noise, not a problem, and nothing below shows it because everything below is stdout. Against your own daemon it is two lines:

```sh
export BOWERBIRD_TOKEN="$(bowerbird auth token | tr -d '\n')"
node --experimental-strip-types docs/cookbook/session-glance/src/index.ts
```

If you have no live sessions right now and want to see the shape, seed three. Do not reach for `bowerbird replay` with no argument: the bundled fixture carries no `cwd` and no `pid`, so every row lands in `(unknown repo)` and the liveness probe ends all of them within one 5s tick. Write a fixture that has both instead. Run this from the repo root:

```sh
bowerbird start
export BOWERBIRD_TOKEN="$(bowerbird auth token | tr -d '\n')"

# A worktree-shaped directory, so the second row has something to derive from.
# `.git` is a FILE here, exactly as git writes it in a real worktree.
mkdir -p /tmp/wt-feature
printf 'gitdir: /nowhere/.git/worktrees/wt-feature\n' > /tmp/wt-feature/.git

# Three sessions: Working in this repo, WaitingInput in the worktree, Idle with
# no cwd at all. The pids must be distinct and alive or the liveness probe ends
# the rows; `$$` (this shell), `$PPID` (its parent) and 1 (init/launchd) are.
now=$(($(date +%s) * 1000))
cat > /tmp/glance-demo.jsonl <<EOF
{"event_id":1,"source":"claude","session_id":"sess-alpha","kind":"PreToolUse","reaction":null,"payload":"{\"tool_name\":\"Read\"}","created_at":$now,"pid":$$,"cwd":"$PWD"}
{"event_id":2,"source":"claude","session_id":"sess-beta","kind":"Notification","reaction":null,"payload":"{\"notification_type\":\"permission_prompt\"}","created_at":$now,"pid":$PPID,"cwd":"/tmp/wt-feature"}
{"event_id":3,"source":"claude","session_id":"sess-gamma","kind":"Stop","reaction":null,"payload":"{}","created_at":$now,"pid":1,"cwd":null}
EOF
bowerbird replay /tmp/glance-demo.jsonl

node --experimental-strip-types docs/cookbook/session-glance/src/index.ts
```

That prints:

```
(unknown repo)
  claude/sess-gamma  Idle  0s
bowerbird
  claude/sess-alpha  Working  0s
wt-feature
  claude/sess-beta  WaitingInput  0s
```

One heading per repository, then one indented line per session. The middle heading is the name of the directory you cloned into, since `sess-alpha`'s `cwd` is `$PWD`; it reads `bowerbird` above because that is what the clone is usually called, and it reads whatever yours is called if you renamed it. The three ages are `0s` because the daemon stamps `started_at` when it ingests an event, so replayed sessions are newborn no matter what `created_at` says; against real sessions the column reads `4m12s`, `1h23m`, `3d4h`. These are demo rows in your real daemon: they go `Ended` on their own once those pids exit, or `rm ~/.bowerbird/bower.db` and restart to be rid of them.

The line format is `<source>/<session_id>  <current_state>  <age>`, two spaces between columns. Repos sort by name and sessions sort by `source/session_id`, so two runs against an unchanged daemon list the same repos and sessions in the same order. The age column is the one thing that moves between runs; it is recomputed against the wall clock every time. `current_state` is reproduced in its PascalCase wire spelling.

With no live sessions the default text mode prints `no live sessions (filter: state=...)` and exits 0. A text-mode attention surface that prints nothing reads as broken, so it never does. The machine modes answer the same question in their own vocabulary: `--count` prints `0`, and `--format=json` prints nothing at all, because zero lines is what NDJSON for an empty set is.

The output contract, which is a mini-API because the tmux status-line surface shells out to this entry:

- No flags: the grouped text above. The line format and the sort order are stable. The exact `no live sessions` wording is not; match on the prefix if you must match at all.
- `--count`: a single integer on stdout and nothing else, so `$(session-glance --count)` is shell-substitutable without parsing.
- `--format=text`: the grouped text above, stated explicitly. Same as no flag.
- `--format=json`: NDJSON, one object per session, one line each. The field set is fixed: `repo`, `source`, `session_id`, `current_state`, `age`, `age_seconds`, `started_at`, `cwd`. Adding or removing a key is a contract change. `repo`, `source`, `session_id` and `current_state` carry the same sanitized values text mode prints (see "How it works"); `cwd` and `started_at` are the untouched wire values, so the raw path is always recoverable from the row.
- `--state=<csv>`: passed through to the REST `?state=` filter. Accepted tokens are `idle`, `working`, `waitinginput`, `ended`, `unknown`, case-insensitive. The default is `idle,working,waitinginput,unknown`, which is how "every non-Ended session" is spelled: the filter grammar has no negation.
- `--help` / `-h`: this contract on stdout, exit 0.
- `--count` wins over `--format`. A consumer that asked for a bare integer never wants NDJSON instead.
- Repeating `--format` or `--state` with a second value is an error, not last-wins. Last-wins is a result that depends on argument order, and every other flag here is order-independent.
- `--help` / `-h` wins over every other argument, including a bad one. `session-glance --halp --help` prints the usage text and exits 0 rather than rejecting the typo, because the moment you most need the usage is the moment you got the arguments wrong.
- One env knob: `BOWERBIRD_GLANCE_TIMEOUT_MS` is how long the entry waits for the daemon, default `5000`. It must be a positive whole number of milliseconds; anything else is an error rather than a silent revert to the default. 5s is the right answer for a human at a prompt and a poor one for a tmux status line refreshing every 1-5s, which is what the knob is for. The value appears in the timeout message so you can tell which deadline fired.
- Exit codes: 0 on success, including the zero-sessions case. 1 on any failure: an unrecognized flag, an invalid state token, a bad `BOWERBIRD_GLANCE_TIMEOUT_MS`, an unreachable daemon, a daemon that accepts the connection and never answers, an HTTP error, a body that is not readable as JSON, a body that is not a JSON array, or an array whose elements are not session objects. Failures print one line on stderr and never a stack trace.

So `session-glance --count --state=waitinginput` is the whole of "how many agents are blocked right now."

Troubleshooting, by the message you got:

- `cannot reach the daemon at http://...` means `~/.bowerbird/server.json` is stale, which is what a crashed or `kill -9`'d daemon leaves behind (a clean `bowerbird stop` removes the file). Run `bowerbird start`.
- `... accepted the connection but did not answer within 5000ms` means something IS on that address and it is not answering: a wedged daemon, or an unrelated process that took the port. `bowerbird stop` then `bowerbird start`. The entry gives up rather than hanging, because a status line that shells out on an interval would otherwise accumulate stuck processes. The same message appears when the response headers arrive and the body then stalls, which is the same failure one step later. `BOWERBIRD_GLANCE_TIMEOUT_MS` changes the number in it.
- `... answered HTTP 200 but the body could not be read as JSON` means something answered that is not a bowerbird daemon: a proxy or dev server that took the port and returned HTML, or a connection reset mid-body. The underlying reason is quoted at the end of the line. `bowerbird stop` then `bowerbird start` re-binds the address.
- `... returned an array whose element N is ...` means the body parsed and was an array, but its elements are not session objects. Same diagnosis as above: whatever is on that address is not this daemon.
- `... is not valid JSON` for `server.json` means a daemon died mid-write. Delete the file and start again.
- Every session in `(unknown repo)` means the sessions predate the `cwd` field, or the source never reported one.
- `BOWERBIRD_GLANCE_TIMEOUT_MS=... is not a positive whole number of milliseconds` means exactly that; the value is milliseconds, so `1000`, not `1s`.

## How it works

Read `~/.bowerbird/server.json` for the daemon's address, `GET /sessions?state=idle,working,waitinginput,unknown` with a bearer token, render, exit. One request, no watch loop. The response is a bare JSON array of session objects, not an envelope.

Spelling "non-Ended" as a positive list of the other four tokens is forced, not stylistic: the filter grammar has no `!ended`. `unknown` is in that list on purpose. It is the decode-only catch-all reserved for future `current_state` values, so leaving it out would let a future daemon's new state vanish from the one surface whose job is to not let things vanish. Note also that the filter tokens are lowercase while the rendered field is PascalCase (`?state=waitinginput` returns `"current_state":"WaitingInput"`); a client-side label lookup keyed on the lowercase spelling silently produces an empty group.

`Ended` is not terminal. A session leaves `Ended` on its next hook event, typically a `claude --resume`. "Live" here means "not currently Ended," not "never ended."

**The canonical repo derivation.** `deriveRepo(cwd)` in [`src/index.ts`](src/index.ts) is the reference implementation that every other bowerbird attention surface conforms to rather than reinterpreting for itself. The rule:

1. An absent `cwd` goes into one named bucket, `(unknown repo)`. The session is never dropped. A session with no `cwd` is exactly the one you would otherwise never notice. "Absent" is generous on purpose: null, undefined, empty, or anything that is not a string. `cwd` arrives through an unchecked cast of the REST body, and a throw here would take down the whole run rather than bucket one session.
2. A **relative** `cwd` is the last path segment of `cwd`, with no filesystem walk at all. A relative path resolves against whoever is *reading*, not against the session that was recorded, so walking one makes the answer depend on where you invoked this from: run it from the repo root and every relative `cwd` collapses to a heading literally named `.`, run the same command from `/tmp` against the same daemon and the headings differ. Two runs of one surface disagreeing is the thing this rule exists to prevent, and nothing in the protocol or the daemon requires `cwd` to be absolute, so it is handled rather than assumed away.
3. Otherwise walk up from `cwd` to the nearest ancestor that contains a `.git` entry, and use that ancestor's name. The check is for existence, not for a directory: in a git worktree `.git` is a file, and testing `isDirectory()` would walk straight past the worktree into the main repo.
4. If no ancestor has a `.git`, use the last path segment of `cwd`. A directory that cannot be **read** counts as "no `.git` here" and the walk continues upward rather than stopping: an unreadable directory inside a real repo still resolves to that repo. Only a `cwd` with no readable `.git` anywhere above it, a since-deleted directory or one recorded on another machine, falls back to the last segment.
5. It never throws, for any input. Rule 1's guard is what makes that true.

Four edge cases worth naming rather than papering over:

- A **git worktree** resolves to the worktree directory's name. Under a `~/worktrees/{repo}/{branch}` layout that is the branch name, not the repo name. That is arguably the more useful grouping when you have four worktrees of one repo open, but it is a behavior, so it is written down instead of assumed.
- A `cwd` **below the repo root**, which is what you get when an agent is launched from a subdirectory, resolves correctly to the repo. That is the whole reason rule 3 exists instead of just taking the last path segment.
- A **symlinked `cwd`** groups under the link's own path, not the target's. `cwd` is verbatim off the wire and nothing here calls `realpath`, so one repo reached two ways (`/src/app` and a `~/app` symlink to it) splits into two headings.
- The derivation **touches the filesystem**, for an absolute `cwd`. Two consequences, both real: a `cwd` recorded on another machine finds nothing to walk and falls through to rule 4, and the function is not purely testable, which is why the path walking is confined to this one small helper and the formatting side stays pure.

Age comes from `started_at`, the epoch-ms timestamp of the session's first observed event, and renders as two units at most (`37s`, `4m12s`, `1h23m`, `3d4h`). A `started_at` that is null, or that is not a usable epoch-ms value at all, renders as `age unknown` rather than `NaN`, a 1970 date, or a day count in scientific notation.

**Every field that reaches a text line is sanitized, and it happens once, where the row is built.** The only thing separating a heading from a session row in this format is the two-space indent, so any line terminator inside a printed value splits one line into two and manufactures a heading out of whatever followed. `session_id`, `source` and `current_state` are as verbatim off the wire as `cwd` is, so all four are treated the same way: `U+0000`-`U+001F`, `U+007F`-`U+009F` (which is where `U+0085 NEL` lives), `U+2028` and `U+2029` become `U+FFFD`. The repo heading gets one extra step, because a leading space is the other way to forge a row: leading whitespace is stripped first, and a name that was nothing but whitespace collapses to `(unknown repo)` rather than printing a blank line.

Sanitizing at row-build time rather than at print time is the load-bearing part. The repo key rows are **grouped and sorted** by is the same string that gets **printed**, so there is one representation and not two. Sanitizing at print time gave a `cwd` of `/x/ foo` and one of `/x/foo` two separate groups that both printed `foo`, ordered by a key the reader never sees. Because of that, `--format=json` carries the sanitized `repo`, `source`, `session_id` and `current_state` too; `cwd` and `started_at` stay untouched, so a machine consumer that wants the raw path reads `cwd`.

Background: [`protocol.md` §GET /sessions](../../protocol.md#get-sessions), [`presenter-authoring.md` §Fetching a REST snapshot](../../presenter-authoring.md#fetching-a-rest-snapshot).

## How to apply it

- **Drive a status line.** `session-glance --count --state=waitinginput` prints a single integer, which is all a tmux status line, a menu bar item, or a shell prompt segment needs to render "2 blocked."
- **Filter to what you actually want interrupted for.** `--state` is a pass-through, so `--state=waitinginput,unknown` is a narrower attention surface than the default, and `--state=ended` answers "what finished while I was away."
- **Reuse the derivation instead of reinventing it.** Any tool that groups sessions by repo should match `deriveRepo`'s behavior, edge cases included, so two bowerbird surfaces never disagree about which repo a session belongs to.
- **Go live if you need to.** This entry is deliberately one-shot. For a continuously updating view, subscribe over WebSocket instead: [`state-session-fanout/`](../state-session-fanout/) shows the per-session routing, and [`dropped-frame-recovery/`](../dropped-frame-recovery/) shows how to catch up after a disconnect.
- **Not covered here:** mapping a session to its tmux pane via `last_pid`. The field is on the wire and the hop is real, but it is a separate pattern with its own failure modes.

## Files

- [`src/index.ts`](src/index.ts): the whole entry. `deriveRepo` (the canonical rule), `formatAge` / `ageSeconds`, `sanitizeTextField` / `sanitizeHeading` and `toRow` (where the text contract is made safe, once), `renderText` (grouping and sorting), `parseArgs` / `normalizeStates` (the flag contract), `requestTimeoutMs` (the deadline and its env override), and the fetch path with its four distinct failure messages: refused connection, no answer, an unreadable body, and a body that is not an array of sessions.
- [`tests/glance.test.ts`](tests/glance.test.ts): unit tests for the pure branches, including the worktree and unreadable-path cases a fixture-driven smoke cannot reach. Run with `npm test`; CI runs it too, on every PR.
- [`package.json`](package.json) / [`tsconfig.json`](tsconfig.json): Node 22.6+ project shape; `npm run typecheck` runs `tsc --noEmit` (CI does this on every PR).
