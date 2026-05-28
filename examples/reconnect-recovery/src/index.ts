// reconnect-recovery
//
// Resilience pattern every long-running tool needs. Tracks `last_event_id`
// per `EventFrame`. On `Close`, `Dropped`, or unsolicited socket close,
// fetches REST snapshot of missed events via
// `GET /sessions/<id>/events?since=<last_event_id>`, then reconnects WS
// and resubscribes. Demonstrates Story 2.4's `DroppedFrame` recovery
// contract plus Story 2.5's graceful-shutdown semantics.
//
// The recovery function is exported so the Node-built-in `--test` runner
// can drive it with a synthetic fake daemon (see `tests/recover.test.ts`).
// That covers the `Dropped` branch as a compiled assertion without
// engineering a real lag burst (per Epic 3 retro Discovery #1's
// "structural guardrail over chaos test" framing).
//
// Run it:
//
//     bowerbird start
//     bowerbird replay
//     node --experimental-strip-types examples/reconnect-recovery/src/index.ts
//     # then in another shell: bowerbird stop && bowerbird start && bowerbird replay
//
// Stdout: one `{event:"recovered",recovered_count:N}` JSON per catch-up.
// Stderr: WS frame counter + reconnection diagnostics.
// Exit codes: 0 on graceful Close/SIGTERM or post-recovery idle timeout;
// 1 on auth failure or fatal socket error.

import { homedir } from "node:os";
import { readFileSync } from "node:fs";
import { join } from "node:path";

// Named `BowerbirdEvent`, not `Event`, so it does not shadow the DOM global
// `Event` — the WebSocket `error` listener below (`addEventListener("error",
// (ev: Event) => ...)`) needs the DOM `Event` type, and a local `interface
// Event` would silently capture that annotation.
interface BowerbirdEvent {
  event_id: number;
  source: string;
  session_id: string;
  kind: string;
  reaction: string | null;
  payload: string;
  created_at: number;
}

interface EventListResponse {
  events: BowerbirdEvent[];
  cursor: number | null;
  oldest_available_event_id: number;
}

interface SessionListItem {
  source: string;
  session_id: string;
}

interface EventFrame {
  op: "event";
  event: BowerbirdEvent;
}

interface DroppedFrame {
  op: "dropped";
  count: number;
  first_dropped_event_id: number;
  last_dropped_event_id: number;
}

interface CloseFrame {
  op: "close";
  reason: string | null;
}

type ServerMessage =
  | EventFrame
  | DroppedFrame
  | CloseFrame
  | { op: string; [k: string]: unknown };

interface ServerInfo {
  bind_addr: string;
}

interface RecoveryDeps {
  bind_addr: string;
  token: string;
  /** Mutable cursor — updated as catch-up fetches advance through history. */
  cursor: { lastEventId: number };
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

// cookbook-begin:dropped-frame-recovery
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
// cookbook-end:dropped-frame-recovery

async function runConnectionLoop(
  initialBindAddr: string,
  token: string,
  cursor: { lastEventId: number },
): Promise<void> {
  let bind_addr = initialBindAddr;
  const maxIdleMsEnv = process.env.BOWERBIRD_EXAMPLE_MAX_IDLE_MS;
  const maxIdleMs =
    maxIdleMsEnv && /^\d+$/.test(maxIdleMsEnv)
      ? parseInt(maxIdleMsEnv, 10)
      : null;

  let running = true;
  let recoveryCount = 0;
  let lastActivityMs = Date.now();
  const markActivity = (): void => {
    lastActivityMs = Date.now();
  };

  // The active WS is captured here so the shutdown handler can close it
  // — without that, SIGTERM during a quiet WS would set `running = false`
  // but the inner `await new Promise` would never resolve.
  let activeWs: WebSocket | null = null;

  // Global idle checker — runs independently of WS state. Exits cleanly
  // when at least one recovery has succeeded and no activity (WS frame OR
  // successful recover()) has happened in maxIdleMs. Used by the smoke
  // test to bound wall-clock without engineering an explicit "you're done"
  // message into the example.
  let idleChecker: NodeJS.Timeout | null = null;
  if (maxIdleMs !== null) {
    idleChecker = setInterval(() => {
      if (recoveryCount < 1) return;
      if (Date.now() - lastActivityMs > maxIdleMs) {
        console.error(`idle ${maxIdleMs}ms after recovery; exiting cleanly`);
        if (idleChecker !== null) clearInterval(idleChecker);
        process.exit(0);
      }
    }, 250);
  }

  const shutdown = (signal: NodeJS.Signals): void => {
    console.error(`received ${signal}; shutting down`);
    running = false;
    if (idleChecker !== null) clearInterval(idleChecker);
    // Close the active WS so the pending `await new Promise` resolves via
    // its `close` listener; then a short grace period lets stderr flush
    // before the hard exit. Without this, SIGTERM hangs on a quiet WS.
    if (activeWs !== null) {
      try {
        activeWs.close();
      } catch {
        // swallow: already closing
      }
    }
    setTimeout(() => process.exit(0), 100);
  };
  process.on("SIGTERM", shutdown);
  process.on("SIGINT", shutdown);

  while (running) {
    const ws = new WebSocket(`ws://${bind_addr}/ws`, {
      // @ts-expect-error -- Node's WebSocket accepts an options object with
      // `headers`; the DOM lib's constructor type does not.
      headers: { Authorization: `Bearer ${token}` },
    });
    activeWs = ws;

    const reconnect = await new Promise<{ reason: string } | null>(
      (resolve) => {
        let resolved = false;
        const finishWith = (reason: string | null): void => {
          if (resolved) return;
          resolved = true;
          try {
            ws.close();
          } catch {
            // swallow
          }
          resolve(reason === null ? null : { reason });
        };

        ws.addEventListener("open", () => {
          console.error("ws open; subscribing to events.*");
          ws.send(JSON.stringify({ op: "subscribe", topic: "events.*" }));
          markActivity();
        });

        ws.addEventListener("message", (ev: MessageEvent) => {
          markActivity();
          let msg: ServerMessage;
          try {
            msg = JSON.parse(String(ev.data)) as ServerMessage;
          } catch (e) {
            console.error(`parse error: ${(e as Error).message}`);
            return;
          }

          if (msg.op === "event") {
            const f = msg as EventFrame;
            if (f.event.event_id > cursor.lastEventId) {
              cursor.lastEventId = f.event.event_id;
            }
            console.error(`event ${f.event.event_id}: ${f.event.kind}`);
          } else if (msg.op === "dropped") {
            finishWith("dropped");
          } else if (msg.op === "close") {
            const c = msg as CloseFrame;
            finishWith(c.reason ?? "close");
          }
        });

        ws.addEventListener("error", (ev: Event) => {
          const detail = (ev as ErrorEvent).message ?? "(no detail)";
          console.error(`websocket error: ${detail}`);
          finishWith("error");
        });

        ws.addEventListener("close", (ev: CloseEvent) => {
          if (ev.code === 1008) {
            // Policy violation — bad subscribe shape, bad token, etc.
            console.error(
              `policy violation close (code ${ev.code}): ${ev.reason}`,
            );
            process.exit(1);
          }
          finishWith(`socket-close ${ev.code}`);
        });
      },
    );

    if (!running) {
      process.exit(0);
    }
    if (reconnect === null) {
      // Should not happen with the finish-with shape above; defensive exit.
      process.exit(0);
    }

    try {
      // Re-read server.json before each recover() attempt — the daemon
      // binds an ephemeral port that changes on restart, so a stale
      // bind_addr from the initial load would fail to connect after a
      // daemon restart. server.json is the discovery anchor (Story 3.2).
      try {
        const refreshed = loadServerInfo();
        bind_addr = refreshed.bind_addr;
      } catch {
        // server.json may not be readable mid-restart; keep prior value.
      }
      const n = await recover(reconnect.reason, { bind_addr, token, cursor });
      recoveryCount++;
      markActivity();
      console.log(JSON.stringify({ event: "recovered", recovered_count: n }));
    } catch (e) {
      console.error(`recovery failed: ${(e as Error).message}`);
      // Backoff briefly before retrying the WS connect to avoid a tight
      // failure loop if the daemon is still coming back up.
      await new Promise((r) => setTimeout(r, 500));
    }
  }

  process.exit(0);
}

async function main(): Promise<void> {
  const { bind_addr } = loadServerInfo();
  const token = resolveToken();
  const cursor = { lastEventId: 0 };
  await runConnectionLoop(bind_addr, token, cursor);
}

// Only run main() when this file is the entry point. Imports from the test
// suite (`tests/recover.test.ts`) re-use the `recover` export without
// triggering the WS connection loop.
const isEntry =
  process.argv[1] !== undefined &&
  import.meta.url === `file://${process.argv[1]}`;
if (isEntry) {
  main().catch((e: Error) => {
    console.error(e.message);
    process.exit(1);
  });
}
