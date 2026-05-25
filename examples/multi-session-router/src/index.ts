// multi-session-router
//
// Subscribes to `state.session.*`, maintains an in-memory map keyed by
// `(source, session_id)`, treats first appearance of a key as a "new session
// appeared" event. Demonstrates Story 2.3's snapshot-on-subscribe behavior
// (existing sessions arrive as a burst of state frames before any live frame)
// plus Story 2.2's live state-frame fan-out.
//
// Run it:
//
//     bowerbird start
//     bowerbird replay
//     node --experimental-strip-types examples/multi-session-router/src/index.ts
//
// Stdout: one canonical JSON object per state update.
// Stderr: `new session: <source>/<session_id>` log lines + diagnostics.
// Exit codes: 0 on graceful Close / SIGTERM; 1 on socket error or auth failure.

import { homedir } from "node:os";
import { readFileSync } from "node:fs";
import { join } from "node:path";

interface HelloFrame {
  op: "hello";
  protocol_version: string;
  daemon_version: string;
  oldest_available_event_id: number;
  daemon_started_at: number;
  history_begins_cleanly: boolean;
}

interface StateFrame {
  op: "state";
  source: string;
  session_id: string;
  state: {
    current_state: "Idle" | "Working" | "WaitingInput";
    last_event_kind: string;
    last_event_at_ms: number;
  };
}

interface CloseFrame {
  op: "close";
  reason: string | null;
}

// Permissive union: covers the variants we act on plus a catch-all for
// future-additive ops (per `ServerMessage::Unknown` in crates/protocol/src/ws.rs).
type ServerMessage =
  | HelloFrame
  | StateFrame
  | CloseFrame
  | { op: string; [k: string]: unknown };

interface ServerInfo {
  bind_addr: string;
}

function loadServerInfo(): ServerInfo {
  const path = join(homedir(), ".bowerbird", "server.json");
  let body: string;
  try {
    body = readFileSync(path, "utf8");
  } catch (e) {
    throw new Error(
      `cannot read ${path}: ${(e as Error).message}. Is the daemon running? Try \`bowerbird start\`.`,
    );
  }
  const parsed = JSON.parse(body) as Partial<ServerInfo>;
  if (typeof parsed.bind_addr !== "string" || parsed.bind_addr.length === 0) {
    throw new Error(`${path} missing string field "bind_addr"`);
  }
  return { bind_addr: parsed.bind_addr };
}

function resolveToken(): string {
  const t = process.env.BOWERBIRD_TOKEN;
  if (!t || t.length === 0) {
    throw new Error(
      "BOWERBIRD_TOKEN env var not set. Retrieve your token with `bowerbird auth token`.",
    );
  }
  return t;
}

async function main(): Promise<void> {
  const { bind_addr } = loadServerInfo();
  const token = resolveToken();

  // Node 22's undici-backed WebSocket constructor accepts a per-request
  // options bag as the second arg (Node-specific extension; the browser
  // signature is `WebSocket(url, protocols)`). We use the `headers` field
  // to send `Authorization: Bearer <token>`. If a future Node release
  // narrows the constructor signature, fall back to the `?token=<token>`
  // query parameter form (supported by the daemon since Story 2.1).
  const url = `ws://${bind_addr}/ws`;
  const ws = new WebSocket(url, {
    // @ts-expect-error -- Node's WebSocket accepts an options object with
    // `headers`, but the DOM lib's WebSocket constructor type doesn't.
    headers: { Authorization: `Bearer ${token}` },
  });

  // cookbook-begin:state-session-fanout
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
  // cookbook-end:state-session-fanout

  ws.addEventListener("error", (ev: Event) => {
    const detail = (ev as ErrorEvent).message ?? "(no detail)";
    console.error(`websocket error: ${detail}`);
    process.exit(1);
  });

  ws.addEventListener("close", (ev: CloseEvent) => {
    // Code 1008 is the daemon's "Policy Violation" close (bad subscribe shape,
    // unknown topic, etc.). 1000 is the normal close.
    if (ev.code !== 1000) {
      console.error(`websocket closed with code ${ev.code}: ${ev.reason}`);
      process.exit(1);
    }
    process.exit(0);
  });

  const shutdown = (signal: NodeJS.Signals): void => {
    console.error(`received ${signal}; closing`);
    try {
      ws.close();
    } catch {
      // swallow
    }
    // Give the close handler a tick to flush; the close event will exit(0).
    setTimeout(() => process.exit(0), 100);
  };
  process.on("SIGTERM", shutdown);
  process.on("SIGINT", shutdown);
}

main().catch((e: Error) => {
  console.error(e.message);
  process.exit(1);
});
