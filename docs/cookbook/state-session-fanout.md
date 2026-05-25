# state-session-fanout

## Problem

I want to subscribe to every session as it appears (no enumeration, no polling) and route each session's state changes to a per-session object I own.

## Approach

Subscribe to `state.session.*`. The daemon emits a snapshot of all currently known sessions on subscribe (Story 2.3), then live state frames as they happen. Key your in-memory map by `(source, session_id)` — `source` matters because a future multi-adapter world (Codex + Claude) needs to disambiguate sessions that share a `session_id`. Treat first-sighting of a key as "new session appeared."

That's the whole pattern. The snapshot-on-subscribe semantics mean you never need a separate `GET /sessions` REST call to bootstrap — by the time the first live frame arrives, you've already seen one snapshot frame per session the daemon knows about.

Background: [`presenter-authoring.md` §Handling each ServerMessage frame → `state`](../presenter-authoring.md#state), [`protocol.md` §Topic grammar](../protocol.md#topic-grammar).

## Code

<!-- cookbook-include: ../../examples/multi-session-router/src/index.ts cookbook-begin:state-session-fanout -->

```ts
  const seen = new Map<string, StateFrame["state"]>();

  ws.addEventListener("open", () => {
    ws.send(JSON.stringify({ op: "subscribe", topic: "state.session.*" }));
  });

  ws.addEventListener("message", (ev: MessageEvent) => {
    let msg: ServerMessage;
    try {
      msg = JSON.parse(String(ev.data)) as ServerMessage;
    } catch (e) {
      console.error(`failed to parse server message: ${(e as Error).message}`);
      return;
    }

    if (msg.op === "state") {
      const s = msg as StateFrame;
      const key = `${s.source}/${s.session_id}`;
      if (!seen.has(key)) {
        console.error(`new session: ${key}`);
      }
      seen.set(key, s.state);
      const out = {
        event: "state",
        source: s.source,
        session_id: s.session_id,
        current_state: s.state.current_state,
        last_event_kind: s.state.last_event_kind,
      };
      console.log(JSON.stringify(out));
    } else if (msg.op === "close") {
      const c = msg as CloseFrame;
      console.error(`daemon closed: ${c.reason ?? "no reason given"}`);
      try {
        ws.close();
      } catch {
        // swallow: already closing
      }
      process.exit(0);
    } else if (msg.op === "hello") {
      const h = msg as HelloFrame;
      console.error(
        `connected to daemon ${h.daemon_version} (protocol ${h.protocol_version})`,
      );
    } else {
      // Future-additive variants (sync, event, dropped, or anything beyond
      // v1.x) are surfaced for visibility but not routed. The substrate's
      // `Unknown` catch-all (crates/protocol/src/ws.rs) is the wire-level
      // mirror of this branch.
      console.error(`ignoring unhandled op: ${msg.op}`);
    }
  });
```

## Variants

**Filter to a single session.** Subscribe to `state.session.<specific-id>` instead of `state.session.*`. You lose new-session discovery (the snapshot only covers the one session you named, and no future-session frame ever arrives) but gain a tighter event stream — useful when a presenter has a stable target session and doesn't care about the rest of the universe.

**Persist transitions for audit.** Wrap the `seen.set(key, s.state)` line in a write to disk (SQLite, JSONL, whatever you have). The fan-out pattern is orthogonal to the persistence choice; the example shows the in-memory shape because that's the canonical thing every consumer does first. Adding persistence is mechanical from there.
