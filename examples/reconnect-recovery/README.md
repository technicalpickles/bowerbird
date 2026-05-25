# reconnect-recovery

## What this example demonstrates

The resilience pattern every long-running tool needs. Tracks `last_event_id` from every received `EventFrame`. On `Close`, `Dropped`, or unsolicited socket close, fetches REST snapshot of missed events via `GET /sessions/<id>/events?since=<last_event_id>`, then reconnects WS and resubscribes. Demonstrates Story 2.4's `DroppedFrame` recovery contract plus Story 2.5's graceful-shutdown semantics.

Illustrates project-context.md Axiom 4: mechanical facts in the protocol (the daemon emits `DroppedFrame { count, first_dropped_event_id, last_dropped_event_id }` and `oldest_available_event_id` in REST responses); the presenter derives the recovery decision (which cursor to use, when a gap is unrecoverable, whether to keep retrying).

## Run it

```sh
bowerbird start
bowerbird replay
node --experimental-strip-types examples/reconnect-recovery/src/index.ts
# in another shell:
bowerbird stop && bowerbird start && bowerbird replay
```

The example connects, receives events, and on `Close` (triggered by `bowerbird stop`) runs `recover()` against the REST surface to catch up. After the second daemon comes up, it reconnects and resumes.

Requires Node 22.6+ for `--experimental-strip-types`.

## Two recovery branches

The recovery flow handles three triggering events:

1. **`Close` frame.** Graceful daemon shutdown (Story 2.5). The smoke test in `tests/cli_examples.rs` exercises this branch deterministically via `bowerbird stop`.
2. **`Dropped` frame.** Lagged-consumer recovery (Story 2.4). The smoke test does NOT engineer a real lag burst — `tests/recover.test.ts` covers the recovery LOGIC directly with a synthetic fake daemon (Node-built-in `http.createServer`). This mirrors Epic 3 retro Discovery #1's "structural guardrail over chaos test" framing.
3. **Unsolicited socket close.** Network partition, OS-level kill, etc. Handled identically — the WebSocket `close` listener triggers the same recovery flow.

In all three cases, the recovery cursor is `last_event_id` (tracked from prior `EventFrame`s), **not** the ids inside a `DroppedFrame`, which are best-estimate upper-bound values per the Story 2.4 protocol-changelog entry.

## Run the recovery unit test

```sh
cd examples/reconnect-recovery
npm test
```

`npm test` invokes the `test` script from `package.json` (`node --experimental-strip-types --test 'tests/**/*.test.ts'`). The bare-directory form (`--test tests/`) is not supported by Node 22.6+; use the glob (or pass an explicit file: `--test tests/recover.test.ts`).

The test spins up a tiny `http.createServer` fake daemon, calls `recover()`, and asserts the cursor advances to the highest event_id returned.

## Anatomy

The cookbook anchor `// cookbook-begin:dropped-frame-recovery` … `// cookbook-end:dropped-frame-recovery` wraps the `recover` function definition. The connection loop around it (subscribe + on-message + reconnect-on-close) is plumbing; the recovery logic is the canonical resilience recipe.

## Smoke-test mode

Setting `BOWERBIRD_EXAMPLE_MAX_IDLE_MS=2000` makes the example exit cleanly after 2 seconds of no WS frames *post-recovery*. The Rust smoke test uses this to bound the wall-clock without engineering an explicit "you're done" message into the example.

## Authentication

```sh
export BOWERBIRD_TOKEN="$(bowerbird auth token | tr -d '\n')"
```
