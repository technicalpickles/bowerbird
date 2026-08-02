// session-glance
//
// One-shot print of every live session, grouped by repository, each with its
// current state and age. The glanceable fallback for a missed notification:
// fetch once, render, exit. No watch loop, no polling, no pane cycling.
//
// Consumes REST `GET /sessions?state=`, the Story 5.8 server-side filter's
// first consumer, plus the Story 5.7 `cwd` and `started_at` fields. Both of
// the interesting things this entry prints are PRESENTER derivations: the
// daemon ships mechanical facts (a verbatim `cwd`, an epoch-ms `started_at`)
// and deliberately refuses to derive repo or age itself (project-context.md
// Axiom 4, ADR 0006). `deriveRepo` below is the canonical derivation the rest
// of the Epic 6 attention surfaces conform to.
//
// Run it:
//
//     bowerbird start
//     export BOWERBIRD_TOKEN="$(bowerbird auth token | tr -d '\n')"
//     node --experimental-strip-types docs/cookbook/session-glance/src/index.ts
//
// Output contract (a mini-API: the tmux status-line surface shells out to
// this entry, so these shapes are stable, not incidental):
//
//     (no flags)      grouped text: a repo heading, then one indented line
//                     per session, `<source>/<session_id>  <state>  <age>`
//     --count         a single integer on stdout and nothing else
//     --format=json   NDJSON, one object per session, fixed field set
//     --state=<csv>   pass-through to the REST `?state=` filter
//
// Exit codes: 0 on success (including zero live sessions), 1 on any failure
// (bad flag, bad state token, daemon unreachable, HTTP error). README.md
// "Run it" is the authoritative statement of the contract.

import { homedir } from "node:os";
import { existsSync, readFileSync } from "node:fs";
import { basename, dirname, join } from "node:path";
import { pathToFileURL } from "node:url";

/// The five `?state=` tokens the daemon accepts, lowercase and
/// case-insensitive. Kept identical to
/// `crates/daemon/src/api/filter.rs::parse_state_token` on purpose: a
/// client-side pre-check that disagrees with the daemon's is worse than no
/// pre-check at all, because it rejects input the daemon would have accepted.
const ACCEPTED_STATE_TOKENS = [
  "idle",
  "working",
  "waitinginput",
  "ended",
  "unknown",
] as const;

// "Every non-Ended session" spelled as a positive CSV. The filter grammar has
// no negation (there is no `!ended`), so non-Ended is the other four tokens.
// `unknown` is included deliberately: it is the decode-only catch-all reserved
// for future additive `current_state` values, and omitting it would let a
// future daemon's new state silently vanish from an attention surface.
const DEFAULT_STATES = "idle,working,waitinginput,unknown";

/// The single bucket for sessions the daemon has no `cwd` for. Named rather
/// than dropped: a session with no `cwd` is exactly the one you would
/// otherwise never notice.
const UNKNOWN_REPO = "(unknown repo)";

/// Rendered in the age column when `started_at` is null (a projection row
/// written before Story 5.7). Named, so the column never reads `NaN` or a
/// 1970 timestamp.
const UNKNOWN_AGE = "age unknown";

/// One element of the `GET /sessions` response. The response is a BARE JSON
/// ARRAY of these, not an envelope object (`crates/protocol/src/rest.rs`).
interface SessionListItem {
  source: string;
  session_id: string;
  /// PascalCase on the wire (`Idle` / `Working` / `WaitingInput` / `Ended` /
  /// `Unknown`) even though the `?state=` filter tokens are lowercase. Do not
  /// assume the two spellings match: a client-side label lookup keyed on the
  /// lowercase token silently produces an empty group.
  current_state: string;
  last_event_kind: string;
  last_event_at_ms: number;
  updated_at: number;
  last_pid: number | null;
  /// Verbatim: no canonicalization, no `~` expansion, no symlink resolution.
  cwd: string | null;
  /// Epoch-ms of the session's first observed event. Nullable.
  started_at: number | null;
}

/// One rendered row. This IS the `--format=json` object: the field set is the
/// documented contract, so adding or removing a key is a contract change.
export interface GlanceRow {
  repo: string;
  source: string;
  session_id: string;
  current_state: string;
  age: string;
  age_seconds: number | null;
  started_at: number | null;
  cwd: string | null;
}

export interface Options {
  count: boolean;
  format: "text" | "json";
  states: string;
}

interface ServerInfo {
  bind_addr: string;
}

// ---------------------------------------------------------------------------
// Daemon discovery + auth. Identical to the sibling entries on purpose: the
// cookbook has exactly one discovery path and one token-resolution path.
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// The canonical FR44 repo-from-cwd derivation.
// ---------------------------------------------------------------------------

/**
 * Derive a repository name from a session's `cwd`. **This is the canonical
 * FR44 repo-from-`cwd` derivation.**
 *
 * Every other bowerbird attention surface (the tmux status line, transition
 * alerts, the live board) conforms to THIS function's behavior rather than
 * reinterpreting FR44 for itself. Downstream code cites it by name
 * (`deriveRepo`), never by line number. Changing the rule means changing this
 * doc comment AND the entry README's "How it works" section together. Those
 * two are the durable record, and every surface that groups by repo is bound
 * to whatever they say.
 *
 * The rule, in order:
 *
 *   1. `cwd` is `null` (or empty) -> the single named bucket `(unknown repo)`.
 *      The session is never dropped.
 *   2. Otherwise walk up from `cwd` itself to the nearest ancestor containing
 *      a `.git` ENTRY, and render that ancestor's basename. Existence, not
 *      `isDirectory()`: in a git worktree `.git` is a FILE, and an
 *      `isDirectory()` check would walk straight past the worktree into the
 *      main repo.
 *   3. No `.git` ancestor found, or the path is not readable (a session on a
 *      since-deleted directory, or a `cwd` recorded on another machine) ->
 *      `basename(cwd)`.
 *   4. Never throws. An unreadable path is a bucket, not a crash.
 *
 * Known imprecisions, named rather than papered over:
 *
 *   - A **git worktree** resolves to the worktree directory's basename, which
 *     under a `~/worktrees/{repo}/{branch}` layout is the BRANCH name, not the
 *     repo name. That is arguably the more useful grouping, but it is a
 *     behavior, so it is stated rather than assumed.
 *   - A `cwd` **below the repo root** (an agent launched from a subdirectory)
 *     resolves correctly to the repo. That is why rule 2 exists instead of a
 *     bare `basename(cwd)`.
 *   - This touches the filesystem, which is fine here (the entry runs on the
 *     same host as the sessions) but means the function is not purely
 *     testable. The path walking is kept in this one small helper so the
 *     formatting side stays pure.
 */
export function deriveRepo(cwd: string | null): string {
  if (cwd === null || cwd.length === 0) {
    return UNKNOWN_REPO;
  }
  try {
    let dir = cwd;
    // Bounded by the filesystem root: `dirname("/") === "/"`, so the
    // parent-equals-self check always terminates the walk.
    for (;;) {
      if (existsSync(join(dir, ".git"))) {
        return basename(dir) || dir;
      }
      const parent = dirname(dir);
      if (parent === dir) {
        break;
      }
      dir = parent;
    }
  } catch {
    // Rule 4: an unreadable path falls through to the basename bucket.
  }
  return basename(cwd) || cwd;
}

// ---------------------------------------------------------------------------
// Pure formatting. No filesystem, no network. Unit-tested in tests/.
// ---------------------------------------------------------------------------

/**
 * Render a session age from `started_at` (epoch-ms, nullable) as a compact
 * two-unit string: `37s`, `4m12s`, `1h23m`, `3d4h`.
 *
 * A null `started_at` renders as the named `age unknown` placeholder. A
 * future-dated `started_at` (clock skew between the recording host and this
 * one) clamps to `0s` rather than rendering a negative age.
 */
export function formatAge(startedAt: number | null, nowMs: number): string {
  if (startedAt === null || !Number.isFinite(startedAt)) {
    return UNKNOWN_AGE;
  }
  const seconds = Math.max(0, Math.floor((nowMs - startedAt) / 1000));
  if (seconds < 60) {
    return `${seconds}s`;
  }
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) {
    return `${minutes}m${seconds % 60}s`;
  }
  const hours = Math.floor(minutes / 60);
  if (hours < 24) {
    return `${hours}h${minutes % 60}m`;
  }
  return `${Math.floor(hours / 24)}d${hours % 24}h`;
}

/** Age in whole seconds, or null when `started_at` is null. */
export function ageSeconds(startedAt: number | null, nowMs: number): number | null {
  if (startedAt === null || !Number.isFinite(startedAt)) {
    return null;
  }
  return Math.max(0, Math.floor((nowMs - startedAt) / 1000));
}

/** Project one wire item into the rendered/serialized row shape. */
export function toRow(item: SessionListItem, nowMs: number): GlanceRow {
  return {
    repo: deriveRepo(item.cwd),
    source: item.source,
    session_id: item.session_id,
    // Verbatim from the wire, PascalCase. No re-spelling, no re-filtering.
    current_state: item.current_state,
    age: formatAge(item.started_at, nowMs),
    age_seconds: ageSeconds(item.started_at, nowMs),
    started_at: item.started_at,
    cwd: item.cwd,
  };
}

/**
 * Group rows by repo and render the text mode's lines. Returns the lines
 * rather than printing them so the shape is unit-testable.
 *
 * Empty input is a clear "nothing live" line, never blank output: a glance
 * that prints nothing reads as a broken glance.
 *
 * Ordering is plain codepoint sort on both axes (repo, then
 * `<source>/<session_id>`) so two runs against the same daemon agree. Not
 * `localeCompare`, which varies with the ambient locale.
 */
export function renderText(rows: GlanceRow[], states: string): string[] {
  if (rows.length === 0) {
    return [`no live sessions (filter: state=${states})`];
  }
  const groups = new Map<string, GlanceRow[]>();
  for (const row of rows) {
    const existing = groups.get(row.repo);
    if (existing) {
      existing.push(row);
    } else {
      groups.set(row.repo, [row]);
    }
  }
  const lines: string[] = [];
  for (const repo of [...groups.keys()].sort()) {
    lines.push(repo);
    const group = [...(groups.get(repo) ?? [])].sort((a, b) =>
      `${a.source}/${a.session_id}` < `${b.source}/${b.session_id}` ? -1 : 1,
    );
    for (const row of group) {
      lines.push(`  ${row.source}/${row.session_id}  ${row.current_state}  ${row.age}`);
    }
  }
  return lines;
}

// ---------------------------------------------------------------------------
// Argument parsing. Invalid input exits non-zero with a one-line message
// naming the bad input and the accepted set.
// ---------------------------------------------------------------------------

/**
 * Normalize a `--state=` CSV, rejecting any token the daemon would reject.
 *
 * The message deliberately reuses the daemon's own vocabulary from
 * `crates/daemon/src/api/filter.rs::parse_state_token` so the client-side
 * pre-check and the server-side rejection say the same thing about the same
 * input.
 */
export function normalizeStates(raw: string): string {
  const tokens = raw.split(",").map((t) => t.trim().toLowerCase());
  for (const token of tokens) {
    if (!(ACCEPTED_STATE_TOKENS as readonly string[]).includes(token)) {
      throw new Error(
        `invalid state token ${JSON.stringify(token)}; accepted tokens are ` +
          `idle, working, waitinginput, ended, unknown (case-insensitive)`,
      );
    }
  }
  return tokens.join(",");
}

/**
 * Parse the entry's flags. Order-independent; unknown arguments are a hard
 * error rather than a silent ignore, because a silently-ignored `--format`
 * typo would hand a consumer text where it expected JSON.
 */
export function parseArgs(argv: string[]): Options {
  const options: Options = { count: false, format: "text", states: DEFAULT_STATES };
  for (const arg of argv) {
    if (arg === "--count") {
      options.count = true;
    } else if (arg === "--format=json") {
      options.format = "json";
    } else if (arg === "--format=text") {
      options.format = "text";
    } else if (arg.startsWith("--state=")) {
      options.states = normalizeStates(arg.slice("--state=".length));
    } else {
      throw new Error(
        `unrecognized argument ${JSON.stringify(arg)}; accepted flags are ` +
          `--count, --format=text, --format=json, --state=<csv>`,
      );
    }
  }
  return options;
}

// ---------------------------------------------------------------------------
// Fetch + render.
// ---------------------------------------------------------------------------

async function fetchSessions(
  bindAddr: string,
  auth: string,
  states: string,
): Promise<SessionListItem[]> {
  const url = `http://${bindAddr}/sessions?state=${encodeURIComponent(states)}`;
  let res: Response;
  try {
    res = await fetch(url, { headers: { Authorization: auth } });
  } catch (e) {
    // The second daemon-down failure mode, and the one the "daemon stopped
    // mid-day" adversity actually produces. `server.json` is still on disk
    // (the daemon only removes it on a CLEAN shutdown, so a crash, an OOM
    // kill, or a `kill -9` leaves it behind), the address in it is stale, and
    // the connection is refused. Node reports that as a bare
    // `TypeError: fetch failed` whose message names neither the address nor
    // the fix, which is exactly the stack-trace-shaped failure this entry must not
    // produce. Name both.
    const cause = (e as { cause?: { code?: string } }).cause;
    const code = cause && typeof cause.code === "string" ? ` (${cause.code})` : "";
    throw new Error(
      `cannot reach the daemon at http://${bindAddr}${code}. ` +
        `~/.bowerbird/server.json points there but nothing is listening, which is what a ` +
        `stopped or crashed daemon leaves behind. Try \`bowerbird start\`.`,
    );
  }
  if (!res.ok) {
    if (res.status === 401) {
      throw new Error("daemon rejected bearer token; check BOWERBIRD_TOKEN env var");
    }
    if (res.status === 400) {
      // The daemon's `{"error": msg}` body is more specific than anything we
      // could reconstruct here, so surface it verbatim.
      const detail = await res.text().catch(() => "");
      throw new Error(`daemon rejected the query: ${detail || "HTTP 400"}`);
    }
    throw new Error(`daemon returned HTTP ${res.status}`);
  }
  // A bare array, not an envelope.
  return (await res.json()) as SessionListItem[];
}

async function main(argv: string[]): Promise<void> {
  const options = parseArgs(argv);
  const { bind_addr } = loadServerInfo();
  const auth = `Bearer ${resolveToken()}`;
  const sessions = await fetchSessions(bind_addr, auth, options.states);

  // `--count` wins over `--format`: it is the narrower contract, and the
  // consumer that asked for a bare integer never wants NDJSON instead.
  if (options.count) {
    console.log(String(sessions.length));
    return;
  }

  const nowMs = Date.now();
  const rows = sessions.map((s) => toRow(s, nowMs));

  if (options.format === "json") {
    for (const row of rows) {
      console.log(JSON.stringify(row));
    }
    return;
  }

  for (const line of renderText(rows, options.states)) {
    console.log(line);
  }
}

// Only run main() when this file is the process entry point, so the unit
// tests in tests/ can import the pure helpers without firing a fetch. Same
// guard as `dropped-frame-recovery`, resolved through `pathToFileURL` so a
// path needing percent-encoding (a space, a non-ASCII directory name) still
// compares equal.
const isEntry =
  process.argv[1] !== undefined && import.meta.url === pathToFileURL(process.argv[1]).href;

if (isEntry) {
  // Message only, never the stack. This is what makes "a clear message, not a
  // stack trace" true by construction for every failure path above.
  main(process.argv.slice(2)).catch((e: Error) => {
    console.error(e.message);
    process.exit(1);
  });
}
