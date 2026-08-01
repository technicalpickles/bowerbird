# state-session-fanout

## What this is

Subscribe to every session as it appears (no enumeration, no polling) and route each session's state changes to a per-session object you own. This is the canonical first pattern for any live presenter: a `state.session.*` subscription feeding a per-`(source, session_id)` map, where first sighting of a key means "new session appeared."

The runnable code in [`src/index.ts`](src/index.ts) exercises Story 2.3's snapshot-on-subscribe semantics (existing sessions arrive as a burst of state frames before any live frame) and Story 2.2's live state-frame fan-out. It also illustrates Axiom 4 from project-context.md: mechanical facts live in the protocol (`state.session.*` frames); semantics live in the presenter ("a new key means a new session" is your interpretation, not the wire's).

## Run it

```sh
bowerbird start
bowerbird replay
export BOWERBIRD_TOKEN="$(bowerbird auth token | tr -d '\n')"
node --experimental-strip-types docs/cookbook/state-session-fanout/src/index.ts
```

Requires Node 22.6+ for `--experimental-strip-types`. Expected stdout, one JSON line per state change:

```
{"event":"state","source":"claude","session_id":"session-alpha","current_state":"Idle","last_event_kind":"Stop"}
{"event":"state","source":"claude","session_id":"session-beta","current_state":"Idle","last_event_kind":"Stop"}
```

Stderr carries `new session: claude/session-alpha` log lines for the first sighting of each key.

## How it works

Subscribe to `state.session.*`. The daemon emits a snapshot of all currently known sessions on subscribe, then live state frames as they happen. Key your in-memory map by `(source, session_id)`; `source` matters because a future multi-adapter world (Codex + Claude) needs to disambiguate sessions that share a `session_id`. Treat first-sighting of a key as "new session appeared."

That's the whole pattern. The snapshot-on-subscribe semantics mean you never need a separate `GET /sessions` REST call to bootstrap: by the time the first live frame arrives, you've already seen one snapshot frame per session the daemon knows about.

The core of `src/index.ts` is the subscribe + per-session map + new-session detection flow. The rest of the file is plumbing every TypeScript bowerbird tool needs: token resolution from `BOWERBIRD_TOKEN`, `server.json` discovery, SIGTERM handling, and an ignore-unknown-ops branch mirroring the protocol crate's `Unknown` catch-all for future-additive frame variants.

Background: [`presenter-authoring.md` §Handling each ServerMessage frame](../../presenter-authoring.md#state), [`protocol.md` §Topic grammar](../../protocol.md#topic-grammar).

## How to apply it

- **Filter to a single session.** Subscribe to `state.session.<specific-id>` instead of `state.session.*`. You lose new-session discovery (the snapshot only covers the one session you named, and no future-session frame ever arrives) but gain a tighter event stream; useful when a presenter has a stable target session and doesn't care about the rest of the universe.
- **Persist transitions for audit.** Wrap the map update in a write to disk (SQLite, JSONL, whatever you have). The fan-out pattern is orthogonal to the persistence choice; the code shows the in-memory shape because that's the canonical thing every consumer does first.
- **Render as a live dashboard.** Pipe stdout into a TUI library; the JSON-per-line shape is stable enough to parse directly.

## Files

- [`src/index.ts`](src/index.ts): the complete presenter; the subscribe + route + detect flow plus connection plumbing.
- [`package.json`](package.json) / [`tsconfig.json`](tsconfig.json): Node 22.6+ project shape; `npm run typecheck` runs `tsc --noEmit` (CI does this on every PR).
