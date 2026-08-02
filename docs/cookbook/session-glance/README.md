# session-glance

## What this is

One command that prints every live session, grouped by repository, with each session's state and age, and then exits. It is the fallback for a notification you missed: instead of cycling tmux panes to find out which agent is blocked, you ask once and get the whole picture.

The runnable code in [`src/index.ts`](src/index.ts) is the canonical consumer of the server-side session filter (`GET /sessions?state=`) and the first presenter to derive both of the Story 5.7 additions. That matters because of Axiom 4 in project-context.md: the daemon ships mechanical facts (a verbatim `cwd`, an epoch-ms `started_at`) and deliberately refuses to derive "repo" or "age" itself. Those derivations are the presenter's job, and this entry is where bowerbird writes them down.

Two things here are contracts other tools bind to, not implementation detail: the output shapes under `## Run it`, and the `deriveRepo` rule under `## How it works`.

## Run it

```sh
bowerbird start
bowerbird replay
export BOWERBIRD_TOKEN="$(bowerbird auth token | tr -d '\n')"
node --experimental-strip-types docs/cookbook/session-glance/src/index.ts
```

Requires Node 22.6+ for `--experimental-strip-types`. Default output is one heading per repository, then one indented line per session:

```
(unknown repo)
  claude/sess-gamma  Idle  0s
bowerbird
  claude/sess-alpha  Working  4m12s
wt-feature
  claude/sess-beta  WaitingInput  1h23m
```

The line format is `<source>/<session_id>  <current_state>  <age>`, two spaces between columns. Repos sort by name and sessions sort by `source/session_id`, so two runs against an unchanged daemon print identical output. `current_state` is reproduced verbatim in its PascalCase wire spelling.

With no live sessions the entry prints `no live sessions (filter: state=...)` and still exits 0. An attention surface that prints nothing looks broken, so it never does.

The output contract, which is a mini-API because the tmux status-line surface shells out to this entry:

- No flags: the grouped text above. The line format and the sort order are stable. The exact `no live sessions` wording is not; match on the prefix if you must match at all.
- `--count`: a single integer on stdout and nothing else, so `$(session-glance --count)` is shell-substitutable without parsing.
- `--format=json`: NDJSON, one object per session, one line each. The field set is fixed: `repo`, `source`, `session_id`, `current_state`, `age`, `age_seconds`, `started_at`, `cwd`. Adding or removing a key is a contract change.
- `--state=<csv>`: passed through to the REST `?state=` filter. Accepted tokens are `idle`, `working`, `waitinginput`, `ended`, `unknown`, case-insensitive. The default is `idle,working,waitinginput,unknown`, which is how "every non-Ended session" is spelled: the filter grammar has no negation.
- `--count` wins over `--format`. A consumer that asked for a bare integer never wants NDJSON instead.
- Exit codes: 0 on success, including the zero-sessions case. 1 on any failure: an unrecognized flag, an invalid state token, an unreachable daemon, an HTTP error. Failures print one line on stderr and never a stack trace.

So `session-glance --count --state=waitinginput` is the whole of "how many agents are blocked right now."

Troubleshooting: `cannot reach the daemon at http://...` means `~/.bowerbird/server.json` is stale, which is what a crashed or `kill -9`'d daemon leaves behind (a clean `bowerbird stop` removes the file). Run `bowerbird start`. If every session lands in `(unknown repo)`, the sessions predate the `cwd` field or the source never reported one.

## How it works

Read `~/.bowerbird/server.json` for the daemon's address, `GET /sessions?state=idle,working,waitinginput,unknown` with a bearer token, render, exit. One request, no watch loop. The response is a bare JSON array of session objects, not an envelope.

Spelling "non-Ended" as a positive list of the other four tokens is forced, not stylistic: the filter grammar has no `!ended`. `unknown` is in that list on purpose. It is the decode-only catch-all reserved for future `current_state` values, so leaving it out would let a future daemon's new state vanish from the one surface whose job is to not let things vanish. Note also that the filter tokens are lowercase while the rendered field is PascalCase (`?state=waitinginput` returns `"current_state":"WaitingInput"`); a client-side label lookup keyed on the lowercase spelling silently produces an empty group.

`Ended` is not terminal. A session leaves `Ended` on its next hook event, typically a `claude --resume`. "Live" here means "not currently Ended," not "never ended."

**The canonical repo derivation.** `deriveRepo(cwd)` in [`src/index.ts`](src/index.ts) is the reference implementation that every other bowerbird attention surface conforms to rather than reinterpreting for itself. The rule:

1. A null or empty `cwd` goes into one named bucket, `(unknown repo)`. The session is never dropped. A session with no `cwd` is exactly the one you would otherwise never notice.
2. Otherwise walk up from `cwd` to the nearest ancestor that contains a `.git` entry, and use that ancestor's name. The check is for existence, not for a directory: in a git worktree `.git` is a file, and testing `isDirectory()` would walk straight past the worktree into the main repo.
3. If no ancestor has a `.git`, or the path cannot be read, use the last path segment of `cwd`.
4. It never throws. An unreadable path is a bucket, not a crash.

Three edge cases worth naming rather than papering over:

- A **git worktree** resolves to the worktree directory's name. Under a `~/worktrees/{repo}/{branch}` layout that is the branch name, not the repo name. That is arguably the more useful grouping when you have four worktrees of one repo open, but it is a behavior, so it is written down instead of assumed.
- A `cwd` **below the repo root**, which is what you get when an agent is launched from a subdirectory, resolves correctly to the repo. That is the whole reason rule 2 exists instead of just taking the last path segment.
- The derivation **touches the filesystem**, which is fine because the entry runs on the same host as the sessions, but it means a `cwd` recorded on another machine falls through to rule 3.

Age comes from `started_at`, the epoch-ms timestamp of the session's first observed event, and renders as two units at most (`37s`, `4m12s`, `1h23m`, `3d4h`). A null `started_at` renders as `age unknown` rather than `NaN` or a 1970 date.

Background: [`protocol.md` §GET /sessions](../../protocol.md#get-sessions), [`presenter-authoring.md` §Fetching a REST snapshot](../../presenter-authoring.md#fetching-a-rest-snapshot).

## How to apply it

- **Drive a status line.** `session-glance --count --state=waitinginput` prints a single integer, which is all a tmux status line, a menu bar item, or a shell prompt segment needs to render "2 blocked."
- **Filter to what you actually want interrupted for.** `--state` is a pass-through, so `--state=waitinginput,unknown` is a narrower attention surface than the default, and `--state=ended` answers "what finished while I was away."
- **Reuse the derivation instead of reinventing it.** Any tool that groups sessions by repo should match `deriveRepo`'s behavior, edge cases included, so two bowerbird surfaces never disagree about which repo a session belongs to.
- **Go live if you need to.** This entry is deliberately one-shot. For a continuously updating view, subscribe over WebSocket instead: [`state-session-fanout/`](../state-session-fanout/) shows the per-session routing, and [`dropped-frame-recovery/`](../dropped-frame-recovery/) shows how to catch up after a disconnect.
- **Not covered here:** mapping a session to its tmux pane via `last_pid`. The field is on the wire and the hop is real, but it is a separate pattern with its own failure modes.

## Files

- [`src/index.ts`](src/index.ts): the whole entry. `deriveRepo` (the canonical rule), `formatAge` / `ageSeconds`, `renderText` (grouping and sorting), `parseArgs` / `normalizeStates` (the flag contract), and the fetch path including the two distinct daemon-down messages.
- [`tests/glance.test.ts`](tests/glance.test.ts): unit tests for the pure branches, including the worktree and unreadable-path cases a fixture-driven smoke cannot reach. Run with `npm test`.
- [`package.json`](package.json) / [`tsconfig.json`](tsconfig.json): Node 22.6+ project shape; `npm run typecheck` runs `tsc --noEmit` (CI does this on every PR).
