# rest-cursor-pagination

## What this is

Fetch a session's entire event history via REST (no WebSocket needed) and handle the case where the event log was truncated before your cursor. This is the canonical "fetch history, render, exit" pattern: a `GET /sessions/<id>/events?since=<cursor>` loop with gap-detection via `oldest_available_event_id`.

The runnable code in [`src/index.ts`](src/index.ts) demonstrates Story 1.7's cursor contract and illustrates Axiom 4 from project-context.md: mechanical facts (the cursor, the oldest-event-id) live in the protocol; semantics (what a gap means for your tool) live in the presenter. The live-WebSocket paths are owned by the sibling entries [`state-session-fanout/`](../state-session-fanout/) (fan-out) and [`dropped-frame-recovery/`](../dropped-frame-recovery/) (resilience).

## Run it

```sh
bowerbird start
bowerbird replay
export BOWERBIRD_TOKEN="$(bowerbird auth token | tr -d '\n')"
node --experimental-strip-types docs/cookbook/rest-cursor-pagination/src/index.ts session-alpha
```

Without a session-id argument, defaults to `session-alpha` (the first session in the bundled fixture). Requires Node 22.6+ for `--experimental-strip-types`. Expected output: tab-separated lines, one per event:

```
1	PreToolUse	Read	Continue
3	PostToolUse	Read	Continue
5	PreToolUse	Edit	Continue
7	PostToolUse	Edit	Continue
9	Notification	-	-
11	Stop	-	-
```

Pipe through `column -t -s$'\t'` for a pretty table.

Troubleshooting: if every reaction renders as `Unknown`, the daemon is missing `~/.bowerbird/adapters/claude/tool-reactions.toml` (see `INSTALL.md` for the placement step). If you get `session ... not found`, run `bowerbird export` with no argument to list the session ids actually in your event log; the bundled-fixture sessions are `session-alpha` and `session-beta`.

## How it works

Loop on `GET /sessions/<id>/events?since=<cursor>` until `cursor === null`. After the first response, compare the request's `since` against the response's `oldest_available_event_id`: if `since < oldest_available_event_id`, events were truncated, so log the actually-missing range (`since + 1 .. oldest - 1`) and continue with what survived. When the missing window collapses (e.g. `since = 0` and `oldest = 1` on a freshly-populated daemon where event 1 is the first one and nothing was dropped), the code suppresses the warning.

The substrate emits the mechanical fact and stays silent on what it means; the presenter interprets the gap. The `extractToolName(payload)` helper parses the JSON-string `payload` field defensively: payloads ride verbatim per Axiom 1, so a future binary-payload adapter would render as `-` rather than crash the viewer.

Background: [`presenter-authoring.md` §Fetching a REST snapshot](../../presenter-authoring.md#fetching-a-rest-snapshot), [`protocol.md` §GET /sessions/{id}/events](../../protocol.md#get-sessionsidevents).

## How to apply it

- **Stream to a renderer instead of collecting.** The code already prints each event as it arrives (one tab-separated line per event). For a richer renderer, replace the stdout write with your output sink; the pagination loop is unchanged.
- **Combine with WebSocket for live + history.** Use REST to load history up to your `lastEventId`, then open a WebSocket and subscribe to live events. The [`dropped-frame-recovery/`](../dropped-frame-recovery/) entry shows the complementary direction (WebSocket first, then REST to catch up after a disconnect); both directions share the same cursor mechanics.

## Files

- [`src/index.ts`](src/index.ts): the complete viewer; the fetch-loop (cursor initialization, loop body, cursor update, gap-detection branch) plus the defensive payload parsing.
- [`package.json`](package.json) / [`tsconfig.json`](tsconfig.json): Node 22.6+ project shape; `npm run typecheck` runs `tsc --noEmit` (CI does this on every PR).
