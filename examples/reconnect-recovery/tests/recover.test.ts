// Node-built-in `--test` runner. Run with:
//
//     node --experimental-strip-types --test tests/
//
// Covers the `Dropped` branch of reconnect-recovery's recovery flow without
// engineering a real lag burst against a live daemon. The smoke test in
// `tests/cli_examples.rs` exercises the `Close` branch deterministically
// via `bowerbird stop`; this test exercises the recovery LOGIC directly.

import { test } from "node:test";
import assert from "node:assert/strict";
import { createServer, type IncomingMessage, type ServerResponse } from "node:http";

import { recover } from "../src/index.ts";

interface FakeEvent {
  event_id: number;
  source: string;
  session_id: string;
  kind: string;
  reaction: string | null;
  payload: string;
  created_at: number;
}

function startFakeDaemon(
  sessions: { source: string; session_id: string }[],
  events: Record<string, FakeEvent[]>,
): Promise<{ bind_addr: string; close: () => Promise<void> }> {
  return new Promise((resolve, reject) => {
    const server = createServer((req: IncomingMessage, res: ServerResponse) => {
      const url = req.url ?? "";
      if (url === "/sessions") {
        res.writeHead(200, { "content-type": "application/json" });
        res.end(JSON.stringify(sessions));
        return;
      }
      const m = url.match(/^\/sessions\/([^/]+)\/events\?since=(\d+)$/);
      if (m) {
        const sessionId = decodeURIComponent(m[1]);
        const since = parseInt(m[2], 10);
        const all = events[sessionId] ?? [];
        const slice = all.filter((e) => e.event_id > since);
        const body = {
          events: slice,
          cursor: slice.length === 0 ? null : slice[slice.length - 1].event_id,
          oldest_available_event_id: all.length === 0 ? 0 : all[0].event_id,
        };
        res.writeHead(200, { "content-type": "application/json" });
        res.end(JSON.stringify(body));
        return;
      }
      res.writeHead(404);
      res.end();
    });
    server.on("error", reject);
    server.listen(0, "127.0.0.1", () => {
      const addr = server.address();
      if (typeof addr === "object" && addr !== null) {
        resolve({
          bind_addr: `127.0.0.1:${addr.port}`,
          close: () =>
            new Promise<void>((res) => {
              server.close(() => res());
            }),
        });
      } else {
        reject(new Error("server.address() returned unexpected shape"));
      }
    });
  });
}

test("recover fetches missed events and updates the cursor", async () => {
  const session = { source: "claude", session_id: "test-session" };
  const events: FakeEvent[] = [
    {
      event_id: 1,
      source: "claude",
      session_id: "test-session",
      kind: "PreToolUse",
      reaction: "Continue",
      payload: "{}",
      created_at: 1,
    },
    {
      event_id: 2,
      source: "claude",
      session_id: "test-session",
      kind: "Stop",
      reaction: null,
      payload: "{}",
      created_at: 2,
    },
  ];
  const fake = await startFakeDaemon([session], { "test-session": events });

  try {
    const cursor = { lastEventId: 0 };
    const n = await recover("dropped", {
      bind_addr: fake.bind_addr,
      token: "test-token",
      cursor,
    });
    assert.equal(n, 2, "should recover both events");
    assert.equal(
      cursor.lastEventId,
      2,
      "cursor should advance to the highest event_id seen",
    );
  } finally {
    await fake.close();
  }
});

test("recover returns 0 when no events past the cursor", async () => {
  const session = { source: "claude", session_id: "test-session" };
  const events: FakeEvent[] = [
    {
      event_id: 1,
      source: "claude",
      session_id: "test-session",
      kind: "PreToolUse",
      reaction: "Continue",
      payload: "{}",
      created_at: 1,
    },
  ];
  const fake = await startFakeDaemon([session], { "test-session": events });

  try {
    const cursor = { lastEventId: 5 };
    const n = await recover("dropped", {
      bind_addr: fake.bind_addr,
      token: "test-token",
      cursor,
    });
    assert.equal(n, 0, "should recover nothing past cursor");
    assert.equal(cursor.lastEventId, 5, "cursor should be unchanged");
  } finally {
    await fake.close();
  }
});

test("recover handles an unrecoverable gap (cursor predates oldest_available)", async () => {
  // Daemon's oldest available event_id is 10; cursor sits at 1, so events
  // 2..9 are gone for good. recover() should emit a `gap unrecoverable`
  // stderr warning and continue with what's still on disk (events 10..11).
  // This exercises the gap-detection branch in src/index.ts:167-172.
  const session = { source: "claude", session_id: "rotated-session" };
  const events: FakeEvent[] = [
    {
      event_id: 10,
      source: "claude",
      session_id: "rotated-session",
      kind: "PreToolUse",
      reaction: "Continue",
      payload: "{}",
      created_at: 10,
    },
    {
      event_id: 11,
      source: "claude",
      session_id: "rotated-session",
      kind: "Stop",
      reaction: null,
      payload: "{}",
      created_at: 11,
    },
  ];
  const fake = await startFakeDaemon([session], { "rotated-session": events });

  // Capture stderr writes to assert the gap warning fires. Restore on exit.
  const stderrChunks: string[] = [];
  const origWrite = process.stderr.write.bind(process.stderr);
  // @ts-expect-error -- Node's stderr.write overloads are awkward to shim
  // with the bound function's exact signature; the runtime behavior is
  // identical (write returns boolean, accepts string|Buffer).
  process.stderr.write = (chunk: string | Buffer): boolean => {
    stderrChunks.push(typeof chunk === "string" ? chunk : chunk.toString());
    return true;
  };

  try {
    const cursor = { lastEventId: 1 };
    const n = await recover("dropped", {
      bind_addr: fake.bind_addr,
      token: "test-token",
      cursor,
    });
    assert.equal(n, 2, "should recover the two available events past cursor");
    assert.equal(
      cursor.lastEventId,
      11,
      "cursor should advance to the highest event_id seen",
    );
    const stderr = stderrChunks.join("");
    assert.match(
      stderr,
      /gap unrecoverable for session rotated-session/,
      `expected gap warning in stderr; got: ${stderr}`,
    );
  } finally {
    process.stderr.write = origWrite;
    await fake.close();
  }
});

test("recover advances cursor to the max across multiple sessions", async () => {
  // Two sessions with non-overlapping event_id ranges. The cursor should
  // advance monotonically to the global max, NOT reset per session.
  const sessions = [
    { source: "claude", session_id: "session-one" },
    { source: "claude", session_id: "session-two" },
  ];
  const events: Record<string, FakeEvent[]> = {
    "session-one": [
      {
        event_id: 1,
        source: "claude",
        session_id: "session-one",
        kind: "PreToolUse",
        reaction: "Continue",
        payload: "{}",
        created_at: 1,
      },
      {
        event_id: 2,
        source: "claude",
        session_id: "session-one",
        kind: "Stop",
        reaction: null,
        payload: "{}",
        created_at: 2,
      },
    ],
    "session-two": [
      {
        event_id: 3,
        source: "claude",
        session_id: "session-two",
        kind: "PreToolUse",
        reaction: "Continue",
        payload: "{}",
        created_at: 3,
      },
      {
        event_id: 4,
        source: "claude",
        session_id: "session-two",
        kind: "Stop",
        reaction: null,
        payload: "{}",
        created_at: 4,
      },
    ],
  };
  const fake = await startFakeDaemon(sessions, events);

  try {
    const cursor = { lastEventId: 0 };
    const n = await recover("dropped", {
      bind_addr: fake.bind_addr,
      token: "test-token",
      cursor,
    });
    assert.equal(n, 4, "should recover events across both sessions");
    assert.equal(
      cursor.lastEventId,
      4,
      "cursor should reflect the global max event_id across all sessions",
    );
  } finally {
    await fake.close();
  }
});
