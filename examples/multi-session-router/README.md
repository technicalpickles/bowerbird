# multi-session-router

## What this example demonstrates

Subscribes to `state.session.*`, routes each `State` frame to a per-`(source, session_id)` in-memory map, treats a previously-unseen key as a "new session appeared" event. Exercises Story 2.3's snapshot-on-subscribe semantics (existing sessions arrive as a burst of state frames before any live frame) and Story 2.2's live state-frame fan-out.

Illustrates project-context.md Axiom 1: the substrate emits mechanical facts (`state.session.*` frames); the presenter interprets them ("a new key means a new session").

## Run it

```sh
bowerbird start
bowerbird replay
node --experimental-strip-types examples/multi-session-router/src/index.ts
```

Requires Node 22.6+ for `--experimental-strip-types`.

## Expected output

```
{"event":"state","source":"claude","session_id":"session-alpha","current_state":"Idle","last_event_kind":"Stop"}
{"event":"state","source":"claude","session_id":"session-beta","current_state":"Idle","last_event_kind":"Stop"}
```

Stderr carries `new session: claude/session-alpha` log lines for the first sighting of each key.

## Anatomy

The cookbook anchor `// cookbook-begin:state-session-fanout` … `// cookbook-end:state-session-fanout` wraps the canonical pattern (subscribe + per-session map + new-session detection). The rest of the file is plumbing every TypeScript bowerbird tool needs: token resolution from `BOWERBIRD_TOKEN`, `server.json` discovery, SIGTERM handling.

## Adapting it

- **Filter to a single session.** Replace `state.session.*` with `state.session.<id>` to scope the subscription. The snapshot-on-subscribe behavior still fires for the matching session.
- **Record state transitions to disk.** Replace `console.log(JSON.stringify(out))` with an append to a JSONL file for offline analysis.
- **Render as a live dashboard.** Pipe stdout into a TUI library; the canonical JSON shape is stable enough to parse directly.

## Authentication

The example reads `BOWERBIRD_TOKEN` from the environment. Retrieve your token:

```sh
export BOWERBIRD_TOKEN="$(bowerbird auth token | tr -d '\n')"
```
