# dropped-frame-recovery

## Problem

My long-running presenter just received a `dropped` frame from the WebSocket, or a `close` frame, or the socket disconnected unexpectedly. How do I catch up without losing events or duplicating ones I already have?

## Approach

Track `last_event_id` from every `event` frame you successfully process. On disruption — `dropped`, `close`, or unsolicited socket close — call a recovery function that:

1. Fetches `GET /sessions` to discover the universe of known sessions.
2. For each session, pages `GET /sessions/<id>/events?since=<last_event_id>` until `cursor === null`.
3. Detects unrecoverable gaps via `oldest_available_event_id` (if your cursor predates it, history was truncated; log the lost range and continue with what survived).
4. Returns the total events recovered so the caller can log progress.

After recovery, reconnect the WebSocket and re-subscribe. The substrate guarantees `event_id` is monotonic per session (`crates/daemon/src/db/queries.rs` `AUTOINCREMENT`), so deduplication is trivial: discard any event whose id you've already seen.

The dropped frame's `first_dropped_event_id` / `last_dropped_event_id` are NOT cursors — they're best-estimate upper bounds. Always recover from the cursor YOU tracked from prior `event` frames (Story 2.4 contract).

Background: [`presenter-authoring.md` §The dropped-frame recovery loop](../presenter-authoring.md#the-dropped-frame-recovery-loop), [`protocol.md` §`dropped`](../protocol.md#dropped).

## Code

<!-- cookbook-include: ../../examples/reconnect-recovery/src/index.ts cookbook-begin:dropped-frame-recovery -->

```ts
/**
 * Catch up missed events after a Close, Dropped, or unsolicited socket close.
 *
 * Fetches `GET /sessions` to discover known sessions, then for each session
 * pages through `GET /sessions/<id>/events?since=<cursor.lastEventId>` until
 * the cursor goes null. Returns the total event count recovered. Updates
 * `cursor.lastEventId` in place so the calling reconnect loop resumes from
 * the right position.
 *
 * Per the Story 2.4 contract (docs/protocol-changelog.md): the recovery
 * cursor is the `last_event_id` the presenter authoritatively tracked from
 * prior `EventFrame`s — NOT the ids inside a `DroppedFrame`, which are
 * best-estimate upper-bound values.
 */
export async function recover(
  reason: string,
  deps: RecoveryDeps,
): Promise<number> {
  const { bind_addr, token, cursor } = deps;
  const auth = `Bearer ${token}`;

  console.error(`recover(${reason}): fetching session list`);
  const listRes = await fetch(`http://${bind_addr}/sessions`, {
    headers: { Authorization: auth },
  });
  if (!listRes.ok) {
    throw new Error(`GET /sessions returned HTTP ${listRes.status}`);
  }
  const sessions = (await listRes.json()) as SessionListItem[];

  let recovered = 0;
  for (const s of sessions) {
    let since = cursor.lastEventId;
    while (true) {
      const url = `http://${bind_addr}/sessions/${encodeURIComponent(s.session_id)}/events?since=${since}`;
      const res = await fetch(url, { headers: { Authorization: auth } });
      if (!res.ok) {
        // 404 is plausible if the session was just removed; skip rather than abort.
        if (res.status === 404) {
          console.error(`session ${s.session_id} not found during recovery`);
          break;
        }
        throw new Error(
          `GET /sessions/${s.session_id}/events returned HTTP ${res.status}`,
        );
      }
      const body = (await res.json()) as EventListResponse;

      // Gap-detection: if our cursor sits before the daemon's oldest
      // available event, some events are gone for good. The recovery
      // function reports the unrecoverable gap and continues with what's
      // still on disk.
      if (since > 0 && since < body.oldest_available_event_id - 1) {
        console.error(
          `gap unrecoverable for session ${s.session_id}: ` +
            `cursor ${since} predates oldest_available ${body.oldest_available_event_id}`,
        );
      }

      for (const ev of body.events) {
        recovered++;
        if (ev.event_id > cursor.lastEventId) {
          cursor.lastEventId = ev.event_id;
        }
      }

      if (body.cursor === null) {
        break;
      }
      since = body.cursor;
    }
  }
  return recovered;
}
```

## Variants

**Resume from disk.** Persist `cursor.lastEventId` after every write to your local model. On cold-start, REST-catch-up from that cursor before opening the WebSocket. The recovery function is identical; only the cursor source differs (disk vs. in-memory).

**Bounded retry.** The reference example reconnects forever; production tools should bound retries with exponential backoff and bail out with an alert after N consecutive failures. The recovery *function* is independent of the retry *policy* — wrap it in your scheduler of choice (e.g. `p-retry`, a hand-rolled exponential backoff, or a circuit breaker) without modifying the function itself.
