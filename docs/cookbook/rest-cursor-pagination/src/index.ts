// event-log-viewer
//
// REST cursor-pagination + gap-detection via `oldest_available_event_id`.
// Demonstrates Story 1.7's REST event-history surface (FR8, FR9, FR20).
// No WebSocket — the WS pattern is owned by multi-session-router and
// reconnect-recovery. This example is the canonical "fetch history via
// REST, render, exit" recipe.
//
// Run it:
//
//     bowerbird start
//     bowerbird replay
//     node --experimental-strip-types docs/cookbook/rest-cursor-pagination/src/index.ts [session-id]
//
// Default session id is `session-alpha` (matches the bundled fixture).
//
// Stdout: one tab-separated line per event — `<event_id>\t<kind>\t<tool_or_dash>\t<reaction_or_dash>`.
// Stderr: gap warning (if `since < oldest_available_event_id`) + diagnostics.
// Exit codes: 0 on success; 1 on HTTP error.

import { homedir } from "node:os";
import { readFileSync } from "node:fs";
import { join } from "node:path";

interface Event {
  event_id: number;
  source: string;
  session_id: string;
  kind: string;
  reaction: string | null;
  payload: string;
  created_at: number;
}

interface EventListResponse {
  events: Event[];
  cursor: number | null;
  oldest_available_event_id: number;
}

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

function extractToolName(payload: string): string {
  // `payload` is a verbatim raw JSON string per architecture.md:404 / Axiom 1
  // ("native payloads ride verbatim"). Parse defensively — some future adapter
  // could emit a non-JSON payload (e.g. base64 binary), in which case we render
  // a dash rather than crash.
  try {
    const parsed = JSON.parse(payload) as { tool_name?: unknown };
    if (typeof parsed.tool_name === "string" && parsed.tool_name.length > 0) {
      return parsed.tool_name;
    }
  } catch {
    // fall through to dash
  }
  return "-";
}

async function main(sessionId: string): Promise<void> {
  const { bind_addr } = loadServerInfo();
  const token = resolveToken();
  const auth = `Bearer ${token}`;

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
}

const sessionId = process.argv[2] ?? "session-alpha";

main(sessionId).catch((e: Error) => {
  console.error(e.message);
  process.exit(1);
});
