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
//     --format=text   the grouped text above, stated explicitly
//     --format=json   NDJSON, one object per session, fixed field set
//     --state=<csv>   pass-through to the REST `?state=` filter
//     --help, -h      the same contract on stdout, exit 0
//
// One env knob: `BOWERBIRD_GLANCE_TIMEOUT_MS` overrides how long the entry
// waits for the daemon (default 5000). See `requestTimeoutMs`.
//
// Exit codes: 0 on success (including zero live sessions), 1 on any failure
// (bad flag, bad state token, daemon unreachable, daemon unresponsive, HTTP
// error, a response body that is not an array of session objects).
// README.md "Run it" is the authoritative statement of the contract.

import { homedir } from "node:os";
import { existsSync, readFileSync, realpathSync } from "node:fs";
import { basename, dirname, isAbsolute, join } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

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
  help: boolean;
}

/// How long to wait for the daemon to answer before giving up. A third
/// daemon-down mode: the address in `server.json` is bound by something that
/// accepts the connection and never responds (a wedged daemon, a stale port
/// grabbed by an unrelated listener). Without a deadline the entry hangs
/// forever with no message and no exit, and the tmux status-line surface
/// (`6-tmux-ambient`) shells out to it on a repeating interval, so it would
/// accumulate hung processes rather than print one bad status.
const DEFAULT_REQUEST_TIMEOUT_MS = 5000;

/// The env var that overrides the deadline, and the reason it exists.
///
/// 5000ms is a fine default for a human typing the command, and a poor one for
/// the surface this entry was built to feed: a tmux status line refreshes on
/// `status-interval` (commonly 1-5s), so a wedged daemon can stall one refresh
/// past the next. The default stays 5s because that is the right answer for
/// the interactive case; a status line that wants to fail faster sets
/// `BOWERBIRD_GLANCE_TIMEOUT_MS=1000` instead of forking the entry.
const TIMEOUT_ENV = "BOWERBIRD_GLANCE_TIMEOUT_MS";

/**
 * Resolve the request deadline in ms.
 *
 * Unset or empty is the default. Anything else must be a positive safe
 * integer, and a value that is not gets a hard error rather than a silent
 * fallback: a typo'd `BOWERBIRD_GLANCE_TIMEOUT_MS=1s` that quietly reverts to
 * 5000 is exactly the kind of "configured it and it did nothing" that the
 * flag parser refuses to do for arguments.
 *
 * Pure and exported so the default and the override are unit-testable without
 * mutating `process.env` (which is a race in a parallel test runner).
 */
export function requestTimeoutMs(env: Record<string, string | undefined>): number {
  const raw = env[TIMEOUT_ENV];
  if (raw === undefined || raw.trim().length === 0) {
    return DEFAULT_REQUEST_TIMEOUT_MS;
  }
  const parsed = Number(raw);
  if (!Number.isSafeInteger(parsed) || parsed <= 0) {
    throw new Error(
      `${TIMEOUT_ENV}=${JSON.stringify(raw)} is not a positive whole number of ` +
        `milliseconds (default: ${DEFAULT_REQUEST_TIMEOUT_MS})`,
    );
  }
  return parsed;
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
  // Inside its own try for the same reason the read is: a daemon killed
  // mid-write leaves a truncated `server.json` behind, which is the SAME
  // `kill -9` scenario the fetch-error path below documents. A bare
  // `SyntaxError: Unexpected end of JSON input` names neither the file nor
  // the fix.
  let parsed: Partial<ServerInfo>;
  try {
    parsed = JSON.parse(body) as Partial<ServerInfo>;
  } catch (e) {
    throw new Error(
      `${path} is not valid JSON: ${(e as Error).message}. A daemon killed mid-write ` +
        `leaves a truncated file behind; delete it and run \`bowerbird start\`.`,
    );
  }
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
 *   1. `cwd` is absent (`null`, `undefined`, empty, or any non-string that
 *      slipped through the wire cast) -> the single named bucket
 *      `(unknown repo)`. The session is never dropped.
 *   2. `cwd` is RELATIVE -> `basename(cwd)`, with no filesystem walk at all.
 *      A relative path resolves against the READER's working directory, not
 *      the recorded session's, so walking it makes the answer depend on where
 *      this entry was invoked from: run from the repo root, every relative
 *      `cwd` collapses to a heading literally named `.`; run the same command
 *      from `/tmp` against the same daemon and the headings differ. Two runs
 *      of one surface disagreeing is the failure AC 3 exists to prevent, and
 *      nothing in the protocol or the daemon validates `cwd` as absolute, so
 *      this is a rule rather than an assumption. `basename` is
 *      machine-independent and needs no filesystem.
 *   3. Otherwise walk up from `cwd` itself to the nearest ancestor containing
 *      a `.git` ENTRY, and render that ancestor's basename. Existence, not
 *      `isDirectory()`: in a git worktree `.git` is a FILE, and an
 *      `isDirectory()` check would walk straight past the worktree into the
 *      main repo.
 *   4. No `.git` ancestor found -> `basename(cwd)`. A directory that cannot
 *      be READ counts as "no `.git` here" and the walk CONTINUES upward:
 *      `existsSync` returns `false` on EACCES rather than throwing, so an
 *      unreadable directory inside a real repo still resolves to that repo.
 *      Only a `cwd` with no readable `.git` anywhere above it (a since-
 *      deleted directory, a `cwd` recorded on another machine) falls back to
 *      the basename.
 *   5. Never throws, for any input. Rule 1's guard is what makes that true:
 *      `cwd` reaches here through an unchecked cast of the REST body, so a
 *      `null`-vs-`undefined` slip or a non-string would otherwise take down
 *      the whole run rather than bucket one session.
 *
 * Known imprecisions, named rather than papered over:
 *
 *   - A **git worktree** resolves to the worktree directory's basename, which
 *     under a `~/worktrees/{repo}/{branch}` layout is the BRANCH name, not the
 *     repo name. That is arguably the more useful grouping, but it is a
 *     behavior, so it is stated rather than assumed.
 *   - A `cwd` **below the repo root** (an agent launched from a subdirectory)
 *     resolves correctly to the repo. That is why rule 3 exists instead of a
 *     bare `basename(cwd)`.
 *   - A **symlinked `cwd`** groups under the link's own path, not the link
 *     target's. `cwd` is verbatim off the wire and nothing here calls
 *     `realpath`, so one repo reached through two paths (`/src/app` and a
 *     `~/app` symlink to it) splits into two headings.
 *   - This touches the filesystem. Two consequences, both real: the function
 *     is not purely testable (so the path walking is kept in this one small
 *     helper and the formatting side stays pure), and a `cwd` recorded on
 *     ANOTHER machine finds nothing to walk and falls through to rule 4.
 */
export function deriveRepo(cwd: string | null): string {
  // `== null` catches `undefined` as well as `null`, and the `typeof` check
  // catches everything else: `cwd` arrives through an unchecked
  // `as SessionListItem[]` cast of the response body, so its declared type is
  // a claim about the daemon, not a guarantee about this value.
  if (cwd == null || typeof cwd !== "string" || cwd.length === 0) {
    return UNKNOWN_REPO;
  }
  // Rule 2. Checked BEFORE any `existsSync`, because the whole point is that a
  // relative path must never be resolved against this process's working
  // directory.
  if (!isAbsolute(cwd)) {
    return basename(cwd) || cwd;
  }
  let dir = cwd;
  // Bounded by the filesystem root: `dirname("/") === "/"`, so the
  // parent-equals-self check always terminates the walk. No try/catch:
  // `existsSync` swallows every stat error (including EACCES) into `false`,
  // and `join`/`dirname`/`basename` on a string cannot throw. A catch here
  // would be unreachable, and an unreachable catch reads as coverage it is
  // not.
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
 *
 * Anything that is not a POSITIVE SAFE INTEGER is `age unknown` too, not just
 * non-finite values, and the two halves of that are separate bugs:
 *
 *   - A `started_at` of `-1e30` is finite, so a `Number.isFinite` guard passes
 *     it through, and the day count then formats in scientific notation
 *     (`1e+22d`) -- which escapes the documented two-unit shape a consumer
 *     parses. Every real epoch-ms value is a safe integer.
 *   - A `started_at` of `-1` or `0` IS a safe integer, and renders a
 *     ~57-year age (`20667d21h`) off a value that means "at or before the
 *     epoch". No session started in 1969. Non-positive is unusable, not old.
 *
 * `nowMs` is guarded on the same terms. It is a caller argument rather than a
 * wire value, but `formatAge(1000, NaN)` rendered `NaNdNaNh` -- the exact
 * output the null branch exists to prevent, reached through the other
 * parameter.
 */
export function formatAge(startedAt: number | null, nowMs: number): string {
  if (!usableEpochMs(startedAt) || !Number.isSafeInteger(nowMs)) {
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

/**
 * Age in whole seconds, or null when `started_at` is null or not a usable
 * epoch-ms value. Same guard as `formatAge`, so the `age` and `age_seconds`
 * fields of a `--format=json` row can never disagree about whether the age is
 * known.
 */
export function ageSeconds(startedAt: number | null, nowMs: number): number | null {
  if (!usableEpochMs(startedAt) || !Number.isSafeInteger(nowMs)) {
    return null;
  }
  return Math.max(0, Math.floor((nowMs - startedAt) / 1000));
}

/// The shared guard behind `formatAge` and `ageSeconds`, spelled once so the
/// `age` and `age_seconds` fields of a `--format=json` row cannot disagree
/// about whether the age is known.
function usableEpochMs(startedAt: number | null): startedAt is number {
  return startedAt !== null && Number.isSafeInteger(startedAt) && startedAt > 0;
}

/**
 * Flatten every character that could forge a LINE in the text format.
 *
 * The text format's ONLY structural discriminator is the two-space indent: an
 * unindented line is a repo heading, an indented one is a session under it,
 * and `6-tmux-ambient` parses exactly that. Any line terminator inside a
 * printed value splits one line into two and manufactures a heading out of
 * whatever followed it.
 *
 * The set is wider than the ASCII controls, because "what ends a line" is not
 * only a newline:
 *
 *   - U+0000-U+001F, U+007F: the ASCII controls, LF and CR among them.
 *   - U+0080-U+009F: the C1 controls, which contain U+0085 NEL. NEL ends a
 *     line for a terminal and for several parsers, and it is NOT in
 *     JavaScript's `\s`, so no whitespace-based guard catches it.
 *   - U+2028 LINE SEPARATOR and U+2029 PARAGRAPH SEPARATOR: line terminators
 *     in the ECMAScript grammar itself.
 *
 * All of them become U+FFFD, which is one visible character and terminates
 * nothing.
 */
export function sanitizeTextField(value: string): string {
  return value.replace(/[\u0000-\u001f\u007f-\u009f\u2028\u2029]/g, "\uFFFD");
}

/**
 * Make a derived repo name safe to emit as a text-mode HEADING.
 *
 * A heading carries one hazard a session row does not: a LEADING SPACE makes
 * it shape-identical to a session line. Both a leading space and an embedded
 * newline are legal POSIX path components and `cwd` is verbatim off the wire,
 * so both are reachable from a real session.
 *
 * Order matters. Leading whitespace is stripped FIRST, then the line-forging
 * characters are flattened. The other order breaks the "whitespace-only
 * collapses to the named bucket" promise for a TAB: a lone `\t` would flatten
 * to U+FFFD and then survive the strip as a one-character heading. Stripping
 * first, a name of nothing but JavaScript `\s` (spaces, tabs, newlines, NBSP,
 * U+2028 ...) collapses to the empty string and then to the bucket, so the
 * promise holds for all of them rather than for the space alone.
 */
export function sanitizeHeading(repo: string): string {
  const deindented = repo.replace(/^\s+/, "");
  const flattened = sanitizeTextField(deindented);
  return flattened.length === 0 ? UNKNOWN_REPO : flattened;
}

/**
 * Project one wire item into the rendered/serialized row shape.
 *
 * **Every field that reaches a text line is sanitized HERE, at construction,
 * so there is exactly one representation of each afterwards.** Two reasons,
 * and the second is why this is not done at print time:
 *
 *  1. `session_id`, `source` and `current_state` are as verbatim off the wire
 *     as `cwd` is. Sanitizing the heading alone left the session line
 *     forgeable through any of them: a `session_id` carrying a newline printed
 *     a real-looking unindented heading AND a real-looking session row under
 *     it.
 *  2. Sanitizing at print time gives one value two spellings. `renderText`
 *     groups and sorts on `row.repo`; if the printed heading were
 *     `sanitizeHeading(row.repo)` instead, a `cwd` of `/x/ foo` and one of
 *     `/x/foo` would be two distinct GROUPS that both PRINT `foo`, ordered by
 *     a key the reader never sees. Measured: the headings came out
 *     `foo, aaa, foo`, contradicting the README's "Repos sort by name".
 *
 * So `--format=json` carries the same sanitized `repo` / `source` /
 * `session_id` / `current_state` that text mode prints -- one representation,
 * not two. The raw path is still recoverable from the row: `cwd` and
 * `started_at` are untouched wire values, because neither is printed as
 * text-mode structure.
 */
export function toRow(item: SessionListItem, nowMs: number): GlanceRow {
  return {
    repo: sanitizeHeading(deriveRepo(item.cwd)),
    source: sanitizeTextField(item.source),
    session_id: sanitizeTextField(item.session_id),
    // PascalCase, verbatim from the wire apart from the line-forging flatten.
    // No re-spelling, no re-filtering.
    current_state: sanitizeTextField(item.current_state),
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
 *
 * **Nothing is sanitized here.** Every field this prints was made display-safe
 * in `toRow`, which is what makes the group KEY, the sort key and the printed
 * heading the same string. Re-sanitizing at this point would reintroduce the
 * two-spellings bug that doc comment describes: rows grouped and ordered by
 * one value, printed as another.
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

/** The accepted set, spelled once so the two error messages cannot drift. */
const ACCEPTED_FLAGS = "--count, --format=text, --format=json, --state=<csv>, --help, -h";

/**
 * The `--help` / `-h` text. Same contract the README states, on stdout, exit
 * 0. A CLI that answers `--help` with "unrecognized argument --help" and exit
 * 1 teaches the reader that it has no discoverable surface.
 */
export const USAGE = [
  "session-glance: one-shot print of every live session, grouped by repository.",
  "",
  "usage: node --experimental-strip-types src/index.ts [flags]",
  "",
  "  (no flags)     grouped text: a repo heading, then one indented line per",
  "                 session, `<source>/<session_id>  <state>  <age>`",
  "  --count        a single integer on stdout and nothing else",
  "  --format=text  the grouped text above, stated explicitly",
  "  --format=json  NDJSON, one object per session, fixed field set",
  "  --state=<csv>  pass-through to the REST `?state=` filter; tokens are",
  `                 idle, working, waitinginput, ended, unknown (default: ${DEFAULT_STATES})`,
  "  --help, -h     this text on stdout, exit 0; wins over any other argument",
  "",
  `env: ${TIMEOUT_ENV} sets the daemon deadline in ms (default: ${DEFAULT_REQUEST_TIMEOUT_MS}).`,
  "",
  "`--count` wins over `--format`. Exit 0 on success (including zero live",
  "sessions), 1 on any failure. Requires BOWERBIRD_TOKEN and a running daemon.",
];

/**
 * Parse the entry's flags. Order-independent; unknown arguments are a hard
 * error rather than a silent ignore, because a silently-ignored `--format`
 * typo would hand a consumer text where it expected JSON.
 *
 * "Order-independent" is enforced, not merely intended: a REPEATED `--format`
 * or `--state` is rejected rather than resolved last-wins, because last-wins
 * is precisely a result that depends on argument order. Repeating `--count`
 * is fine; it is a boolean with no second value to disagree with.
 *
 * `--help` / `-h` WINS over every other argument, including a bad one. The
 * rationale for answering `--help` at all is that a CLI which replies
 * "unrecognized argument --help" teaches the reader it has no discoverable
 * surface -- and `session-glance --halp --help` did exactly that, because the
 * single pass rejected the typo before it ever saw the request for help. The
 * one moment a reader most needs the usage text is the moment they got the
 * arguments wrong.
 */
export function parseArgs(argv: string[]): Options {
  const options: Options = {
    count: false,
    format: "text",
    states: DEFAULT_STATES,
    help: false,
  };
  // Scanned in its own pass, ahead of any validation, so no other argument can
  // out-rank it.
  if (argv.some((arg) => arg === "--help" || arg === "-h")) {
    return { ...options, help: true };
  }
  const seen = new Map<string, string>();
  const once = (family: string, arg: string): void => {
    const prior = seen.get(family);
    if (prior !== undefined) {
      throw new Error(
        `${family} given twice (${JSON.stringify(prior)} then ${JSON.stringify(arg)}); ` +
          `resolving that last-wins would make the result depend on argument order`,
      );
    }
    seen.set(family, arg);
  };
  for (const arg of argv) {
    if (arg === "--count") {
      options.count = true;
    } else if (arg === "--format=json") {
      once("--format", arg);
      options.format = "json";
    } else if (arg === "--format=text") {
      once("--format", arg);
      options.format = "text";
    } else if (arg.startsWith("--state=")) {
      once("--state", arg);
      options.states = normalizeStates(arg.slice("--state=".length));
    } else {
      throw new Error(
        `unrecognized argument ${JSON.stringify(arg)}; accepted flags are ${ACCEPTED_FLAGS}`,
      );
    }
  }
  return options;
}

// ---------------------------------------------------------------------------
// Fetch + render.
// ---------------------------------------------------------------------------

/**
 * The mode-(c) message: something IS on the address, it accepted the
 * connection, and it did not finish answering inside the deadline.
 *
 * Shared by the two places a stall surfaces -- the `fetch` call (no response
 * headers) and the body read (headers, then the stream stops) -- because they
 * are one failure from the reader's side and deserve one message.
 */
function unansweredError(bindAddr: string, timeoutMs: number): Error {
  return new Error(
    `daemon at http://${bindAddr} accepted the connection but did not answer within ` +
      `${timeoutMs}ms. ~/.bowerbird/server.json points there; the daemon may be ` +
      `wedged, or an unrelated process may have taken the address. Try \`bowerbird stop\` ` +
      `then \`bowerbird start\`. Set ${TIMEOUT_ENV} to change the deadline.`,
  );
}

/**
 * Is this the abort the request deadline fired?
 *
 * `AbortSignal.timeout` rejects with a `DOMException` named `TimeoutError`
 * from `fetch` itself; the same signal aborting a body read surfaces as an
 * `AbortError` on some paths. Both mean "the deadline fired", so both route to
 * the same message rather than to the generic one.
 */
function isTimeout(e: unknown): boolean {
  const name = (e as Error | undefined)?.name;
  return name === "TimeoutError" || name === "AbortError";
}

/**
 * Reject a response body that is an array of things which are not sessions.
 *
 * `Array.isArray` is not enough on its own. `[1,2,3]` passes it, and then the
 * failure is silent in both machine modes: text renders
 * `  undefined/undefined  undefined  age unknown`, and `--format=json` DROPS
 * documented keys (`JSON.stringify` omits `undefined` values), emitting
 * `{"repo":...,"age":...,"age_seconds":null}` against the README's "the field
 * set is fixed". `--count` reports a plausible integer for junk.
 *
 * Only the three fields the text contract PRINTS are required to be strings.
 * `cwd` and `started_at` have their own guards downstream (`deriveRepo` rule 1
 * and `formatAge`'s usable-epoch check), which is what keeps a single odd row
 * a named bucket rather than a whole-run failure, and additive fields a future
 * daemon adds are ignored rather than rejected.
 */
function checkRowShape(body: unknown[], bindAddr: string): SessionListItem[] {
  for (const [index, item] of body.entries()) {
    const bad =
      typeof item !== "object" || item === null || Array.isArray(item)
        ? "not a JSON object"
        : (["source", "session_id", "current_state"] as const)
            .filter((f) => typeof (item as Record<string, unknown>)[f] !== "string")
            .map((f) => `no string field "${f}"`)
            .join(", ");
    if (bad.length > 0) {
      throw new Error(
        `daemon at http://${bindAddr} returned an array whose element ${index} is ${bad}, ` +
          `where GET /sessions must return session objects. Something other than a bowerbird ` +
          `daemon may be listening on that address.`,
      );
    }
  }
  return body as SessionListItem[];
}

async function fetchSessions(
  bindAddr: string,
  auth: string,
  states: string,
): Promise<SessionListItem[]> {
  const url = `http://${bindAddr}/sessions?state=${encodeURIComponent(states)}`;
  const timeoutMs = requestTimeoutMs(process.env);
  // ONE signal for the whole exchange, headers and body alike. A per-call
  // deadline would let a server that dribbles the body forever hold the entry
  // open indefinitely while never breaching any single timeout.
  const signal = AbortSignal.timeout(timeoutMs);
  let res: Response;
  try {
    res = await fetch(url, { headers: { Authorization: auth }, signal });
  } catch (e) {
    // Daemon-down failure mode (c): something IS listening on the address, it
    // accepted the connection, and it never answered. A one-shot glance that
    // never exits is worse than one that fails, because the status-line
    // surface that shells out to it on an interval would pile up hung
    // processes instead of showing one bad status.
    if (isTimeout(e)) {
      throw unansweredError(bindAddr, timeoutMs);
    }
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
  // Reading the body is its OWN failure mode, and it was the one path left
  // outside a try. The status line is 200 by this point, so nothing above has
  // fired, and Node's three shapes here all name neither the address nor the
  // fix:
  //
  //   - a non-JSON body (an HTML error page from a proxy that took the port):
  //     `Unexpected token '<', "<html>not "... is not valid JSON`
  //   - headers, then the stream stalls past the deadline:
  //     `The operation was aborted due to timeout`
  //   - the connection reset mid-body: `terminated`
  //
  // The stall is mode (c) arriving one step later, so it gets mode (c)'s
  // message. The other two are the same "that is not a bowerbird daemon"
  // diagnosis as the non-array check below.
  let body: unknown;
  try {
    body = await res.json();
  } catch (e) {
    if (isTimeout(e)) {
      throw unansweredError(bindAddr, timeoutMs);
    }
    throw new Error(
      `daemon at http://${bindAddr} answered HTTP ${res.status} but the body could not be ` +
        `read as JSON: ${(e as Error).message}. GET /sessions must return a bare JSON array. ` +
        `Something other than a bowerbird daemon may be listening on that address; ` +
        `\`bowerbird stop\` then \`bowerbird start\` re-binds it.`,
    );
  }
  // A bare array, not an envelope -- CHECKED, not assumed. The cast below is
  // the only thing standing between the wire and every downstream `.length` /
  // `.map`, and the failure it lets through is silent in the one mode that
  // matters most: `--count` on a non-array body prints the literal string
  // `undefined` and exits 0, which a tmux status line renders as a plausible
  // status forever. Text and JSON mode fail loudly on the same input; this
  // makes all three agree.
  if (!Array.isArray(body)) {
    throw new Error(
      `daemon at http://${bindAddr} returned ${typeof body === "object" && body !== null ? "a JSON object" : JSON.stringify(body)} ` +
        `where GET /sessions must return a bare JSON array. Something other than a bowerbird ` +
        `daemon may be listening on that address.`,
    );
  }
  return checkRowShape(body, bindAddr);
}

async function main(argv: string[]): Promise<void> {
  const options = parseArgs(argv);
  if (options.help) {
    for (const line of USAGE) {
      console.log(line);
    }
    return;
  }
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
// tests in tests/ can import the pure helpers without firing a fetch.
//
// Compared on REALPATHS, not on URLs. `import.meta.url` is what the ESM
// loader resolved, and the loader realpaths the module it loads;
// `process.argv[1]` is the path the user typed, verbatim. Invoke the entry
// through a symlink -- which is exactly what the README's status-line
// guidance implies (`ln -s .../src/index.ts ~/bin/session-glance`) -- and the
// two differ, `isEntry` is false, and the process exits 0 having printed
// nothing at all. A silent no-op is the worst possible failure for an
// attention surface. `pathToFileURL` is kept as the fallback so a path
// needing percent-encoding (a space, a non-ASCII directory name) still
// compares equal when a realpath cannot be taken.
function resolveIsEntry(): boolean {
  const invoked = process.argv[1];
  if (invoked === undefined) {
    return false;
  }
  try {
    return realpathSync(fileURLToPath(import.meta.url)) === realpathSync(invoked);
  } catch {
    return import.meta.url === pathToFileURL(invoked).href;
  }
}

const isEntry = resolveIsEntry();

if (isEntry) {
  // Message only, never the stack. This is what makes "a clear message, not a
  // stack trace" true by construction for every failure path above.
  main(process.argv.slice(2)).catch((e: Error) => {
    console.error(e.message);
    process.exit(1);
  });
}
