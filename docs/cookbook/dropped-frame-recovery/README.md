# dropped-frame-recovery

## What this is

The resilience pattern every long-running presenter needs: your WebSocket just received a `dropped` frame, or a `close` frame, or the socket disconnected unexpectedly, and you need to catch up without losing events or duplicating ones you already have.

The runnable code in [`src/index.ts`](src/index.ts) tracks `last_event_id` from every received `event` frame and, on disruption, runs a REST catch-up before reconnecting. It demonstrates Story 2.4's `dropped`-frame recovery contract plus Story 2.5's graceful-shutdown semantics, and illustrates Axiom 4 from project-context.md: the daemon emits mechanical facts (`dropped { count, first_dropped_event_id, last_dropped_event_id }`, `oldest_available_event_id`); the presenter derives the recovery decision (which cursor to use, when a gap is unrecoverable, whether to keep retrying).

## Run it

```sh
bowerbird start
bowerbird replay
export BOWERBIRD_TOKEN="$(bowerbird auth token | tr -d '\n')"
node --experimental-strip-types docs/cookbook/dropped-frame-recovery/src/index.ts
# in another shell:
bowerbird stop && bowerbird start && bowerbird replay
```

The presenter connects, receives events, and on `close` (triggered by `bowerbird stop`) runs `recover()` against the REST surface to catch up. After the second daemon comes up, it reconnects and resumes. Requires Node 22.6+ for `--experimental-strip-types`.

Setting `BOWERBIRD_EXAMPLE_MAX_IDLE_MS=2000` makes it exit cleanly after 2 seconds of no WS frames post-recovery; the Rust smoke test uses this to bound wall-clock time.

To run the recovery unit test:

```sh
cd docs/cookbook/dropped-frame-recovery
npm test
```

`npm test` runs `node --experimental-strip-types --test 'tests/**/*.test.ts'` (the bare-directory form `--test tests/` is not supported by Node 22.6+; use the glob). The test spins up a tiny `http.createServer` fake daemon, calls `recover()`, and asserts the cursor advances to the highest event_id returned.

## How it works

Track `last_event_id` from every `event` frame you successfully process. On disruption, `recover()`:

1. Fetches `GET /sessions` to discover the universe of known sessions.
2. For each session, pages `GET /sessions/<id>/events?since=<last_event_id>` until `cursor === null`.
3. Detects unrecoverable gaps via `oldest_available_event_id` (if your cursor predates it, history was truncated; log the lost range and continue with what survived).
4. Returns the total events recovered so the caller can log progress.

After recovery, reconnect the WebSocket and re-subscribe. The substrate guarantees `event_id` is monotonic per session, so deduplication is trivial: discard any event whose id you've already seen.

The dropped frame's `first_dropped_event_id` / `last_dropped_event_id` are NOT cursors; they're best-estimate upper bounds. Always recover from the cursor YOU tracked from prior `event` frames (Story 2.4 contract).

Three triggering events share the one flow:

1. **`close` frame.** Graceful daemon shutdown (Story 2.5). The smoke test in `tests/cli_examples.rs` exercises this branch deterministically via `bowerbird stop`.
2. **`dropped` frame.** Lagged-consumer recovery (Story 2.4). The smoke test does NOT engineer a real lag burst; [`tests/recover.test.ts`](tests/recover.test.ts) covers the recovery logic directly with a synthetic fake daemon (structural guardrail over chaos test, per the Epic 3 retro).
3. **Unsolicited socket close.** Network partition, OS-level kill, etc. The WebSocket `close` listener triggers the same recovery flow.

Background: [`presenter-authoring.md` §The dropped-frame recovery loop](../../presenter-authoring.md#the-dropped-frame-recovery-loop), [`protocol.md` §`dropped`](../../protocol.md#dropped).

## How to apply it

- **Resume from disk.** Persist `cursor.lastEventId` after every write to your local model. On cold-start, REST-catch-up from that cursor before opening the WebSocket. The recovery function is identical; only the cursor source differs (disk vs. in-memory).
- **Bounded retry.** The reference code reconnects forever; production tools should bound retries with exponential backoff and bail out with an alert after N consecutive failures. The recovery function is independent of the retry policy: wrap it in your scheduler of choice (`p-retry`, a hand-rolled backoff, a circuit breaker) without modifying the function itself.

## Files

- [`src/index.ts`](src/index.ts): the complete presenter; `recover()` plus the connection loop (subscribe, on-message, reconnect-on-close) around it.
- [`tests/recover.test.ts`](tests/recover.test.ts): unit test for the recovery logic against a fake daemon, runnable via `npm test`.
- [`package.json`](package.json) / [`tsconfig.json`](tsconfig.json): Node 22.6+ project shape; `npm run typecheck` runs `tsc --noEmit` (CI does this on every PR).
