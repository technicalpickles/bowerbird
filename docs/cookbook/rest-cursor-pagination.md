# rest-cursor-pagination

## Problem

I want to fetch a session's entire event history via REST (no WebSocket needed for this use case) and handle the case where the event log was truncated before my cursor.

## Approach

Loop on `GET /sessions/<id>/events?since=<cursor>` until `cursor === null`. After the first response, compare the request's `since` against the response's `oldest_available_event_id` — if `since < oldest_available_event_id`, events were truncated, log the actually-missing range (`since + 1 .. oldest - 1`) and continue with what survived. When the missing window collapses (e.g. `since = 0` and `oldest = 1` on a freshly-populated daemon where event 1 is the first one and nothing was dropped), suppress the warning.

The substrate emits the mechanical fact — `oldest_available_event_id` — and stays silent on what it means; the presenter interprets the gap (architecture.md Axiom 4).

Background: [`presenter-authoring.md` §Fetching a REST snapshot](../presenter-authoring.md#fetching-a-rest-snapshot), [`protocol.md` §GET /sessions/{id}/events](../protocol.md#get-sessionsidevents).

## Code

<!-- cookbook-include: ../../examples/event-log-viewer/src/index.ts cookbook-begin:rest-cursor-pagination -->

```ts
  let since = 0;
  let firstResponse = true;
  while (true) {
    const url = `http://${bind_addr}/sessions/${encodeURIComponent(sessionId)}/events?since=${since}`;
    const res = await fetch(url, { headers: { Authorization: auth } });
    if (!res.ok) {
      if (res.status === 404) {
        throw new Error(
          `session ${sessionId} not found (try \`bowerbird export\` to see available session ids)`,
        );
      }
      if (res.status === 401) {
        throw new Error(
          "daemon rejected bearer token; check BOWERBIRD_TOKEN env var",
        );
      }
      throw new Error(`daemon returned HTTP ${res.status}`);
    }
    const body = (await res.json()) as EventListResponse;

    // Gap-detection: on the first response, compare the request's `since`
    // against the daemon's globally oldest available event_id. A presenter
    // interpreting the mechanical fact (Axiom 4) — the daemon emits
    // `oldest_available_event_id` and stays silent on what it means.
    //
    // The trigger condition (`since < oldest_available_event_id`) matches
    // the daemon contract documented at crates/protocol/src/rest.rs:15-16.
    // The printed range describes only the *actually missing* event_id
    // window (`since + 1 .. oldest - 1`); when that window collapses
    // (e.g. `since = 0, oldest = 1` — a freshly-populated daemon where
    // event 1 is the first available event and no events were dropped),
    // no warning is printed.
    if (firstResponse && since < body.oldest_available_event_id) {
      const missingFrom = since + 1;
      const missingTo = body.oldest_available_event_id - 1;
      if (missingFrom <= missingTo) {
        console.error(
          `gap detected: events ${missingFrom}..${missingTo} are no longer available`,
        );
      }
    }
    firstResponse = false;

    for (const ev of body.events) {
      const tool = extractToolName(ev.payload);
      const reaction = ev.reaction ?? "-";
      console.log(`${ev.event_id}\t${ev.kind}\t${tool}\t${reaction}`);
    }

    if (body.cursor === null) {
      break;
    }
    since = body.cursor;
  }
```

## Variants

**Stream to a renderer instead of collecting.** Print each event as it arrives (the example already does this — one tab-separated line per event to stdout). For a richer renderer, replace the `console.log` line with a write to your output sink. The pagination loop is unchanged.

**Combine with WebSocket for live + history.** Use REST to load history up to your `lastEventId`, then open a WebSocket and subscribe to live events. The [`dropped-frame-recovery.md`](dropped-frame-recovery.md) entry shows the complementary direction (WebSocket first, then REST to catch up after a disconnect); both directions share the same cursor mechanics.
