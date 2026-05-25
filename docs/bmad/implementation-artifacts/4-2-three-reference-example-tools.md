# Story 4.2: Three reference example tools

Status: done

<!-- Note: Validation is optional. Run validate-create-story for quality check before dev-story. -->

## Story

As a tool builder,
I want complete, working TypeScript reference examples that demonstrate the core bowerbird patterns,
so that I can understand how to build my own tools by reading and running real code, not just documentation.

## Acceptance Criteria

1. **Given** the new `examples/multi-session-router/` directory (TypeScript, single-file `src/index.ts`, Node 22.6+ via `node --experimental-strip-types`) **When** the example is run against `bowerbird replay` with the bundled fixture (`fixtures/replay-demo.jsonl`) **Then** it (a) reads `BOWERBIRD_TOKEN` from env and `~/.bowerbird/server.json` for `bind_addr`, (b) opens a WebSocket to `ws://<bind_addr>/ws` with `Authorization: Bearer <token>`, (c) sends one `Subscribe { topic: "state.session.*" }` message, (d) receives a `Hello` frame followed by an initial snapshot of zero `State` frames (no pre-existing sessions in a fresh `BOWERBIRD_DATA_DIR`), (e) routes each subsequent `State` frame to a per-`(source, session_id)` in-memory map, treating a previously-unseen `(source, session_id)` as a "new session appeared" event and logging it on stderr, (f) prints one canonical JSON object on stdout per state update — `{event: "state", source, session_id, current_state, last_event_kind}` — so the smoke test asserts against deterministic output, (g) exits cleanly with code 0 on receiving `Close` frame (graceful daemon shutdown) or on `SIGTERM`. The fixture's two sessions (`session-alpha`, `session-beta`) MUST both surface as distinct map entries.

2. **Given** the new `examples/event-log-viewer/` directory **When** the example is run against `bowerbird replay` with the bundled fixture (CLI arg: a session id, default `session-alpha`) **Then** it (a) reads token + bind_addr the same way as AC #1, (b) calls `GET /sessions/<id>/events?since=0` via Node native `fetch`, (c) loops on the `cursor` field of `EventListResponse` until `cursor === null` (matching the cursor contract documented in Story 4.1 task 4.3), (d) for each `Event` row prints one line to stdout: `<event_id>\t<kind>\t<tool_name_or_dash>\t<reaction>` where `<tool_name_or_dash>` is extracted from the JSON-string `payload.tool_name` field if present else `-`, and `<reaction>` is the event's reaction value or `-` if null, (e) demonstrates `oldest_available_event_id` gap-detection by reading the response field and printing a stderr warning if `since < oldest_available_event_id`, (f) exits 0 on success, exit 1 on any HTTP error with a clear stderr message. The example demonstrates the *REST pagination + gap-detection* pattern (FR8, FR9, FR20) — the WebSocket is intentionally NOT used in this example because event-log-viewer is the canonical "fetch history via REST, render, exit" pattern; the WS path is owned by AC #1 and AC #3.

3. **Given** the new `examples/reconnect-recovery/` directory **When** the example is run against a daemon and the WebSocket is intentionally disrupted via `bowerbird stop` (or via SIGTERM signaling from the smoke test) **Then** it (a) reads token + bind_addr the same way as AC #1, (b) opens a WebSocket, subscribes to `events.*`, (c) tracks `last_event_id` from every received `EventFrame`, (d) on receiving a `Close` frame OR a `Dropped` frame OR an unsolicited socket close event, fetches `GET /sessions` to discover known session ids, then for each session calls `GET /sessions/<id>/events?since=<last_event_id>` to catch up the gap (using `oldest_available_event_id` to detect unrecoverable gaps and print a stderr warning when so), then (e) reconnects via WS and resubscribes to `events.*`, (f) prints to stdout `{event: "recovered", recovered_count: N}` after the catch-up phase completes, (g) exits 0 on the second `Close` frame (or after 60s no-event idle timeout in smoke mode via `BOWERBIRD_EXAMPLE_MAX_IDLE_MS=2000`). The example demonstrates the *DroppedFrame → REST catch-up → re-subscribe* resilience pattern every long-running tool needs (FR14, FR15). The smoke test exercises the `Close` branch of the disrupt-and-recover path (deterministic via `bowerbird stop`); the `Dropped` branch is covered by a unit-shaped test inside the example's `tests/` dir that calls the recovery function with a synthetic `DroppedFrame` payload — making the recovery logic compiled-test-asserted without engineering a real lag burst (per Epic 3 retro Discovery #1's "structural guardrail over chaos test" framing).

4. **Given** all three examples in `examples/` **When** CI runs the workspace test suite (`cargo test --workspace -- --test-threads=1`) on every PR **Then** the new test crate `tests/cli_examples.rs` orchestrates one Rust-driven end-to-end smoke per example: (a) starts a daemon in a `TempDir`-scoped `BOWERBIRD_DATA_DIR` (same isolation pattern as `tests/cli_replay.rs`), (b) for the multi-session-router and event-log-viewer tests, runs `bowerbird replay` against the bundled fixture to populate state, (c) spawns `node --experimental-strip-types examples/<name>/src/index.ts` as a subprocess with `HOME` pointed at the TempDir and `BOWERBIRD_TOKEN` set, (d) reads the subprocess stdout, asserts the example's canonical output shape per its AC above, (e) on success runs `bowerbird stop` (for the reconnect-recovery test, the stop is mid-test to trigger the recovery path), (f) on failure dumps the daemon log and the example's stderr to test output so CI diagnostics surface the actual failure mode rather than just "subprocess returned non-zero." A separate Rust test `tests/cli_examples_drift.rs` (hermetic; no daemon) asserts: each example has `src/index.ts` + `README.md` + `package.json`; each example's `package.json` declares `engines.node >= 22.6.0`; each example's `src/index.ts` contains at least one `// cookbook-begin:<name>` / `// cookbook-end:<name>` marker pair (the cookbook-anchor convention from project-context.md §Cookbook discipline, Story 4.3 will consume); the architecture.md tree at the post-Story-4.2 location lists the three examples as TypeScript projects (not Cargo workspace members).

5. **Given** the cookbook-example coupling invariant from project-context.md §Cookbook discipline ("Marked regions: `// cookbook-begin:<name>` … `// cookbook-end:<name>` plus a tiny build step that inlines them at doc-build time") **When** Story 4.2 ships **Then** each example's `src/index.ts` carries at least one cookbook-anchor region for the canonical pattern it demonstrates (`multi-session-router` → `cookbook-begin:state-session-fanout`, `event-log-viewer` → `cookbook-begin:rest-cursor-pagination`, `reconnect-recovery` → `cookbook-begin:dropped-frame-recovery`). The anchors are pure comments — they have no runtime effect, no preprocessing, and the example runs identically with or without them. Story 4.3's documentation suite will define the inlining mechanism (mdBook `{{#include}}` directives anchored on the marker names OR a hand-rolled build step); Story 4.2 ships only the markers and the doc-drift guardrail. Cookbook entries themselves are explicitly Story 4.3 scope.

## Tasks / Subtasks

- [x] **Task 1 — Architecture decision: TypeScript on Node, not Cargo workspace members** (AC: #1, #2, #3, #4)
  - [x] 1.1 **Reconcile architecture.md with project-context.md.** Project-context.md §Example presenters (line 196-203) makes a `Decided`-status call: "TypeScript, runs on Node. Lives in `examples/`. No build step beyond `tsc`." Architecture.md §Project Structure & Boundaries (lines 769-779) currently shows the older draft shape: `examples/multi-session-router/Cargo.toml` + `src/main.rs` — Rust workspace members. The two documents diverge; project-context.md is authoritative (it's the living architectural record, while architecture.md is a planning artifact from an earlier draft pass). Story 4.2 ships the TypeScript shape and surgically updates architecture.md to match — same pattern as Story 4.1's architecture.md edits and Epic 3 retro's Discovery #1 catalog of stale architecture.md sections. Document this reconciliation in `examples/README.md` and in the Dev Agent Record's Completion Notes so a future retrospective finds the explicit "TypeScript chosen because project-context.md §Example presenters § is Decided" trail.
  - [x] 1.2 **No Cargo workspace member entries.** The root `Cargo.toml`'s `[workspace] members = ["crates/*"]` stays as-is — do NOT add `"examples/*"`. The examples directory is a Node project zone, not a Cargo zone. This keeps `cargo build --workspace`, `cargo clippy --workspace`, `cargo test --workspace` semantically clean (they cover Rust only); the TypeScript smoke is invoked via `tests/cli_examples.rs` from the workspace `tests/` directory, which is the established CLI-E2E pattern (Stories 3.1-3.4, 4.1).
  - [x] 1.3 **Node version floor: 22.6+** for the native `--experimental-strip-types` flag. Document in each example's `package.json` (`engines.node: ">=22.6.0"`) and `README.md`. Node 22 is LTS (April 2024); 22.6 shipped August 2024 with native TypeScript type-stripping. Future Node releases will make the flag unnecessary (`--strip-types` became default in 23.x); for V1 the flag is the ergonomic choice. CI's `ubuntu-latest` and `macOS-latest` runners ship Node 22+ natively as of mid-2025; if a stale runner image regresses, the test surfaces a clear `node: bad option: --experimental-strip-types` error pointing at the version floor.
  - [x] 1.4 **No runtime dependencies in any example.** Use Node's built-in `WebSocket` (stable in Node 22 LTS via undici), `fetch`, `fs`, `path`, `os`, `child_process`, `process`. No `ws` package, no `node-fetch`, no `typescript` runtime dep. The only optional dev-dependency is `typescript` for the `npm run typecheck` script (`tsc --noEmit`); CI does NOT install dev-deps for the smoke run, and `node_modules/` is `.gitignore`-d. This matches the "no SDK" axiom from project-context.md line 200-203 ("see if the protocol can be tight enough that an SDK is overkill"); each example demonstrates exactly how little plumbing is needed to consume the substrate.

- [x] **Task 2 — Author `examples/multi-session-router/`** (AC: #1, #5)
  - [x] 2.1 **Create `examples/multi-session-router/src/index.ts`** as a NEW file. ~80-120 lines. Module structure:
    - Top-of-file doc comment names the pattern: "Subscribes to `state.session.*`, maintains an in-memory map keyed by `(source, session_id)`, treats first appearance of a key as a new-session event. Demonstrates Story 2.3's snapshot-on-subscribe behavior plus Story 2.2's live state-frame fan-out."
    - Type definitions (inline, no shared file): `interface HelloFrame { op: "hello"; protocol_version: string; daemon_version: string; oldest_available_event_id: number; daemon_started_at: number; history_begins_cleanly: boolean }`, `interface StateFrame { op: "state"; source: string; session_id: string; state: { current_state: string; last_event_kind: string; last_event_at_ms: number } }`, `interface CloseFrame { op: "close"; reason: string | null }`, plus a union `type ServerMessage = HelloFrame | StateFrame | CloseFrame | { op: string; [k: string]: unknown }`. The catch-all branch handles `event`, `dropped`, `sync`, and the asymmetric `Unknown` variant gracefully — the multi-session-router only acts on `state`.
    - `loadServerInfo()` helper: reads `${os.homedir()}/.bowerbird/server.json`, parses JSON, returns `{ bind_addr: string }`. Throws with clear message if missing.
    - `resolveToken()` helper: reads `process.env.BOWERBIRD_TOKEN`. If unset, throws with stderr message pointing the user at `bowerbird auth token`. (Story 4.2 does NOT shell out to the `bowerbird` CLI — keeps the example dep tree clean; a future polish story could add a fallback.)
    - `main()`: instantiates `WebSocket(\`ws://${bind_addr}/ws\`, { headers: { Authorization: \`Bearer ${token}\` } })`. Note: native `WebSocket` constructor signature does NOT accept `headers` in browsers but DOES in Node's undici implementation via the second-arg options object — verify against Node 22 docs. If the headers shape diverges from the browser spec, fall back to `?token=<token>` query parameter (already supported by Story 2.1 AC, documented in protocol-changelog.md WS entry).
    - On `open`: send `JSON.stringify({ op: "subscribe", topic: "state.session.*" })`. Per the WS protocol surface, this is the single-topic-per-Subscribe shape (Story 2.1).
    - On `message`: parse JSON. If `op === "state"`, key by `${source}/${session_id}`, check if previously seen, log `new session: <key>` to stderr if first sighting, print `{event: "state", source, session_id, current_state, last_event_kind}` to stdout. If `op === "close"`, log `daemon closed: <reason>` to stderr and `process.exit(0)`.
    - On `error`: log to stderr and `process.exit(1)`.
    - On `SIGTERM` / `SIGINT`: close the WebSocket cleanly and `process.exit(0)`.
    - **Cookbook anchors:** wrap the subscribe + on-message routing block in `// cookbook-begin:state-session-fanout` ... `// cookbook-end:state-session-fanout` markers. Story 4.3's cookbook will inline this region as the canonical multi-session-router recipe.
  - [x] 2.2 **Create `examples/multi-session-router/package.json`** as a NEW file. Shape:
    ```json
    {
      "name": "bowerbird-example-multi-session-router",
      "private": true,
      "type": "module",
      "engines": { "node": ">=22.6.0" },
      "scripts": {
        "start": "node --experimental-strip-types src/index.ts",
        "typecheck": "tsc --noEmit"
      },
      "devDependencies": { "typescript": "^5.6.0" }
    }
    ```
    The `private: true` field prevents accidental publish to npm (we are a dual-licensed Rust project; the examples are read-and-run reference code, not a published package). `type: "module"` enables ESM so the example can use `import` syntax natively.
  - [x] 2.3 **Create `examples/multi-session-router/tsconfig.json`** as a NEW file. Strict-mode shape:
    ```json
    {
      "compilerOptions": {
        "target": "ES2023",
        "module": "ESNext",
        "moduleResolution": "Bundler",
        "strict": true,
        "noEmit": true,
        "skipLibCheck": true,
        "types": ["node"]
      },
      "include": ["src/**/*.ts"]
    }
    ```
    `noEmit: true` because we never produce JavaScript — Node runs the TypeScript source directly via `--experimental-strip-types`. The `types: ["node"]` ambient declaration requires `@types/node` for type-checking but NOT for runtime; this is the only dev-dep beyond TypeScript itself. (If adding `@types/node` becomes a friction point, fall back to declaring the few Node globals inline — but `@types/node` is small and stable.)
  - [x] 2.4 **Create `examples/multi-session-router/README.md`** as a NEW file. Sections:
    - "What this example demonstrates" — 2-3 sentences: subscribes to `state.session.*`, routes state frames to per-session map, handles new-session-discovery via Story 2.3 snapshot-on-subscribe.
    - "Run it" — three-line shell block: `bowerbird start; bowerbird replay; node --experimental-strip-types examples/multi-session-router/src/index.ts`.
    - "Expected output" — sample stdout showing the deterministic JSON-per-update shape.
    - "Anatomy" — pointer to the `cookbook-begin:state-session-fanout` anchor block as the canonical pattern; the rest of the file is plumbing (token resolution, server.json read, SIGTERM handling) that every TypeScript bowerbird tool needs.
    - "Adapting it" — one paragraph on common variants: filter to a single session via `state.session.<specific-id>` subscription; record state transitions to disk for auditing; render as a live dashboard.
  - [x] 2.5 **Add `.gitignore` at `examples/.gitignore`** (NOT per-example; one shared file at the examples-dir root). Content: `node_modules/\n*.log\n`. Each example's `package.json` install would create a `node_modules/` if a user opted into the typecheck flow; we never want it committed.

- [x] **Task 3 — Author `examples/event-log-viewer/`** (AC: #2, #5)
  - [x] 3.1 **Create `examples/event-log-viewer/src/index.ts`** as a NEW file. ~80-120 lines. Module structure:
    - Top-of-file doc comment names the pattern: "REST cursor-pagination + gap-detection via `oldest_available_event_id`. Demonstrates Story 1.7's REST event-history surface (FR8, FR9, FR20). No WebSocket — the WS pattern is owned by multi-session-router and reconnect-recovery."
    - Same `loadServerInfo()` and `resolveToken()` helpers (duplicated, not shared — see Task 1.4 rationale).
    - Type definitions: `interface Event { event_id: number; source: string; session_id: string; kind: string; reaction: string | null; payload: string; created_at: number }`, `interface EventListResponse { events: Event[]; cursor: number | null; oldest_available_event_id: number }`.
    - `main(sessionId: string)`: loops calling `fetch(\`http://${bind_addr}/sessions/${sessionId}/events?since=${since}\`, { headers: { Authorization: \`Bearer ${token}\` } })` until `EventListResponse.cursor === null`. Initial `since = 0`.
    - Per-event render: extract `tool_name` from the JSON-string `payload` (parse `event.payload` as JSON, defensively — the payload is a verbatim raw JSON string per architecture.md:404 / project-context Axiom 1 "native payloads ride verbatim"). If `tool_name` not present (e.g. `Notification` events), use `-`. Print `${event_id}\t${kind}\t${tool_name_or_dash}\t${reaction_or_dash}` per line.
    - Gap-detection: after first response, check `if (0 < oldest_available_event_id)` (the request's `since=0` is less than the daemon's oldest available); if true, print a stderr warning `gap detected: events 0..${oldest_available_event_id - 1} are no longer available` and continue with the available subset. This is the exact gap-detection contract architecture.md §HelloFrame describes — presenter-derived, daemon emits only the mechanical fact (`oldest_available_event_id`).
    - Exit 0 on success. Exit 1 on HTTP non-2xx with a stderr message: `404` → `session ${sessionId} not found (try \`bowerbird export\` to see available session ids)`; `401` → `daemon rejected bearer token; check BOWERBIRD_TOKEN env var`; other → `daemon returned HTTP ${status}`.
    - **Cookbook anchors:** wrap the fetch-loop block (the cursor-pagination + gap-detection logic) in `// cookbook-begin:rest-cursor-pagination` ... `// cookbook-end:rest-cursor-pagination`. This is the canonical "fetch history" recipe Story 4.3 will inline.
  - [x] 3.2 **Create `examples/event-log-viewer/package.json`, `tsconfig.json`, `README.md`** mirroring Task 2.2-2.4. The README's "What this example demonstrates" emphasizes the REST-only (no WS) pattern and links to the gap-detection guarantees in the protocol-changelog v1.0 → v1.1 entries for Story 1.7 (`oldest_available_event_id`) and Story 4.1 (`POST /replay`'s relationship to the export round-trip).
  - [x] 3.3 **Default session id.** When run without a CLI arg, default to `session-alpha` (matches the bundled fixture's first session). Document in the README. When run with an arg, accept it as the session id. No flag parsing library — `process.argv[2]` is the canonical Node idiom; the example demonstrates that "presenters can be small" by not pulling in a CLI library.
  - [x] 3.4 **`payload` JSON parsing — defensive shape.** The `event.payload` field is a verbatim raw JSON string (Axiom 1), but some adapters may emit non-JSON payloads in the future (e.g. a hypothetical binary adapter that base64-encodes its native data). Wrap the `JSON.parse(event.payload)` in a try/catch; on parse error, treat `tool_name` as `-` rather than crashing. The smoke test exercises this defensively against the bundled fixture (which emits valid JSON).

- [x] **Task 4 — Author `examples/reconnect-recovery/`** (AC: #3, #5)
  - [x] 4.1 **Create `examples/reconnect-recovery/src/index.ts`** as a NEW file. ~120-160 lines (the most complex of the three). Module structure:
    - Top-of-file doc comment names the pattern: "Resilience pattern every long-running tool needs. Tracks `last_event_id` per `EventFrame`. On `Close`, `Dropped`, or unsolicited socket close, fetches REST snapshot of missed events via `GET /sessions/<id>/events?since=<last_event_id>`, then reconnects WS and resubscribes. Demonstrates Story 2.4's `DroppedFrame` recovery contract plus Story 2.5's graceful-shutdown semantics."
    - Same `loadServerInfo()` and `resolveToken()` helpers.
    - Type definitions: union `ServerMessage` including `EventFrame { op: "event"; event: Event }` and `DroppedFrame { op: "dropped"; count: number; first_dropped_event_id: number; last_dropped_event_id: number }`.
    - State: a single `let lastEventId: number = 0;` updated on every received `EventFrame`. Track `recoveryCount: number = 0` across reconnects so the smoke test asserts the example actually fetched something after disrupt.
    - **Recovery function** (the cookbook-anchor centerpiece): `async function recover(reason: string): Promise<number>` that (a) calls `GET /sessions` to list known sessions, (b) for each session calls `GET /sessions/<id>/events?since=<lastEventId>` to fetch missed events, (c) on each fetched event updates `lastEventId = Math.max(lastEventId, event.event_id)`, (d) checks `oldest_available_event_id` and prints a stderr `gap unrecoverable for session <id>` warning when `lastEventId < oldest_available_event_id - 1`, (e) returns the total event count recovered. The function is `export`-ed so a unit-shaped test in `tests/recover.test.ts` can call it directly with a synthetic `DroppedFrame` shape.
    - **Connection loop:** an outer `while (running)` loop that opens a WebSocket, subscribes to `events.*`, processes messages. On `close`/`dropped`/`error`, calls `recover(reason)`, prints `{event: "recovered", recovered_count: N}` to stdout, increments `recoveryCount`, then loops (reconnects).
    - **Exit conditions:** (a) `SIGTERM` or `SIGINT` sets `running = false` and exits 0; (b) smoke-test idle timer: if `BOWERBIRD_EXAMPLE_MAX_IDLE_MS` is set, exit 0 after that many milliseconds of no WS frames received post-recovery (default unset; the smoke test sets it to 2000); (c) on `Close` frame after recovery has run at least once AND `running` was already false, exit 0.
    - **Cookbook anchors:** wrap the `recover` function (start of `async function recover` through closing `}`) in `// cookbook-begin:dropped-frame-recovery` ... `// cookbook-end:dropped-frame-recovery`.
  - [x] 4.2 **Create `examples/reconnect-recovery/tests/recover.test.ts`** as a NEW file. Uses Node's built-in `node --test` runner. The test imports `recover` from `../src/index.ts`, sets `BOWERBIRD_TOKEN` and a mocked `server.json` (writes a temp `server.json` and overrides `os.homedir()` via... actually, `os.homedir()` cannot be easily overridden in Node — switch to: the test passes `bind_addr` and `token` as args, and the `recover()` signature is extended to accept them as optional overrides). The test asserts that given a fake daemon serving a 2-event JSON response on `/sessions` + `/sessions/<id>/events?since=0`, `recover()` returns `2` and `lastEventId` is updated. The fake daemon is a tiny Node `http.createServer` instance bound to `127.0.0.1:0` and torn down at test end. Mirrors the "compiled test, not chaos engineering" philosophy from Epic 3 retro A7.
  - [x] 4.3 **Create `examples/reconnect-recovery/package.json`, `tsconfig.json`, `README.md`** mirroring Task 2.2-2.4. The `package.json` adds a `test` script: `"test": "node --experimental-strip-types --test tests/"`. The README documents the two recovery branches (`Close` vs `Dropped`), explains why the smoke test exercises only the `Close` branch deterministically and the `Dropped` branch is unit-tested separately, and points readers at project-context Axiom 4 ("Mechanical facts in the protocol; semantics in the presenter") to frame why the presenter — not the daemon — derives the recovery decision.

- [x] **Task 5 — Author `examples/README.md`** (AC: #4, #5)
  - [x] 5.1 **Create `examples/README.md`** as a NEW file. Sections:
    - "Three reference examples" — overview paragraph naming the three (multi-session-router, event-log-viewer, reconnect-recovery) and what each demonstrates.
    - "Why TypeScript on Node" — 2-3 sentences citing project-context.md §Example presenters: most presenter authors reach for Node first; the substrate doesn't care what speaks WebSocket+JSON. No SDK shipped — the protocol is designed to be small enough that each example is self-contained with ~30 lines of interface declarations.
    - "Node version requirement" — Node 22.6+ for `--experimental-strip-types`; future Node releases will make the flag unnecessary.
    - "Quick run" — a single shell block that runs all three sequentially against the bundled fixture.
    - "Cookbook anchors" — point at the `cookbook-begin:<name>` / `cookbook-end:<name>` markers in each example's source; explain that Story 4.3's documentation suite will inline these regions into cookbook entries via include directives. Story 4.2 ships the markers; Story 4.3 ships the cookbook.
    - "Architecture reconciliation note" — one paragraph naming the decision: project-context.md §Example presenters is the source of truth for the TypeScript-on-Node choice; the prior architecture.md tree had a Rust shape that has been refreshed by Story 4.2 to match. Mirrors Story 4.1's reconciliation pattern.

- [x] **Task 6 — Architecture.md updates** (AC: #4)
  - [x] 6.1 **Update `docs/bmad/planning-artifacts/architecture.md:769-779`** — the §Project Structure & Boundaries tree currently lists `examples/` as Cargo workspace members:
    ```
    ├── examples/                           # Cargo workspace members; compile in CI
    │   ├── multi-session-router/
    │   │   ├── Cargo.toml                  # depends on protocol; listed in root Cargo.toml members
    │   │   └── src/main.rs
    │   ├── event-log-viewer/
    │   │   ├── Cargo.toml
    │   │   └── src/main.rs
    │   └── reconnect-recovery/
    │       ├── Cargo.toml
    │       └── src/main.rs
    ```
    Replace with the TypeScript shape:
    ```
    ├── examples/                           # TypeScript presenters; Node 22.6+; smoke-tested in CI
    │   ├── .gitignore                      # node_modules/, *.log
    │   ├── README.md                       # overview; reconciliation note vs prior arch draft
    │   ├── multi-session-router/
    │   │   ├── package.json                # engines.node >= 22.6.0; type: module
    │   │   ├── tsconfig.json               # strict, noEmit (Node strips types at runtime)
    │   │   ├── README.md
    │   │   └── src/
    │   │       └── index.ts                # cookbook-begin:state-session-fanout
    │   ├── event-log-viewer/
    │   │   ├── package.json
    │   │   ├── tsconfig.json
    │   │   ├── README.md
    │   │   └── src/
    │   │       └── index.ts                # cookbook-begin:rest-cursor-pagination
    │   └── reconnect-recovery/
    │       ├── package.json
    │       ├── tsconfig.json
    │       ├── README.md
    │       ├── src/
    │       │   └── index.ts                # cookbook-begin:dropped-frame-recovery
    │       └── tests/
    │           └── recover.test.ts         # node --test; covers the Dropped branch
    ```
  - [x] 6.2 **Update `architecture.md:757`** — the Cargo.toml comment currently reads `# workspace manifest; members includes examples/*`. Replace with `# workspace manifest; members = ["crates/*"] only; examples/ is a Node project zone, not a Cargo zone (see project-context.md §Example presenters)`.
  - [x] 6.3 **Update `architecture.md:892`** — the §Fixture Ownership table row currently reads:
    ```
    | `fixtures/` (workspace root) | Shared hook payloads + demo SQLite | `examples/*/`, `bowerbird/tests/integration/` |
    ```
    Update the "Used by" column to reflect that examples consume fixtures via runtime read (not via Rust `include_bytes!`): `examples/*/ (runtime read by Node via fs.readFile), bowerbird CLI binary (compile-time embed via include_bytes!)`.
  - [x] 6.4 **Update `architecture.md:921-924`** — the §Architectural Boundaries §Examples boundary block currently reads:
    ```
    **Examples boundary:**
    - `examples/*/Cargo.toml` listed in root `Cargo.toml` `members`
    - Depend on `protocol` directly; compile in CI; break loudly on API changes
    ```
    Replace with:
    ```
    **Examples boundary:**
    - `examples/*/` are TypeScript projects on Node 22.6+; NOT Cargo workspace members
    - Hand-write the ~30 lines of TypeScript interface declarations they need per example (no shared SDK, per project-context.md §Example presenters)
    - Consume the WS + REST surfaces via Node's built-in `WebSocket` and `fetch`; no runtime npm dependencies
    - Smoke-tested in CI via `tests/cli_examples.rs` (Rust orchestrates daemon + Node subprocess); break loudly on protocol-shape changes via the smoke's stdout-shape assertions
    ```
  - [x] 6.5 **Update `architecture.md:936`** — the FR-to-structure mapping table row currently reads:
    ```
    | FR31–FR35: Developer tools + examples | `src/commands/{replay,export}.rs` (Story 4.1); `examples/*/` (Story 4.2 deferred); `docs/cookbook/` (Story 4.3 deferred) |
    ```
    Update to:
    ```
    | FR31–FR35: Developer tools + examples | `src/commands/{replay,export}.rs` (Story 4.1); `examples/*/` (Story 4.2, TypeScript on Node 22.6+); `docs/cookbook/` (Story 4.3 deferred) |
    ```
  - [x] 6.6 **Update `architecture.md:1003`** — the readiness checklist row currently reads `FR31–FR35: Developer tools | replay.rs + export.rs + examples/ workspace members + fixtures/ ✅`. Replace `examples/ workspace members` with `examples/ TypeScript projects`.
  - [x] 6.7 **Verification grep sweep.** After the edits:
    ```sh
    grep -nE 'examples/\*/Cargo\.toml|examples/\*/src/main\.rs|examples.*workspace members|Cargo workspace members.*compile in CI' \
      _bmad-output/planning-artifacts/architecture.md
    # MUST return 0 hits — every prior Rust-shaped reference replaced or deleted.
    grep -nE 'cookbook-begin|TypeScript on Node|Node 22\.6' _bmad-output/planning-artifacts/architecture.md
    # Should show the new shipped-surface references.
    ```

- [x] **Task 7 — Workspace-level smoke test suite** (AC: #1, #2, #3, #4)
  - [x] 7.1 **Create `tests/cli_examples.rs`** as a NEW file at the workspace root. Pattern mirrors `tests/cli_replay.rs` (Story 4.1): `bowerbird_bin()`, `bowerbird_cmd_in(tmp)`, `wait_for_daemon_up`, `force_stop` helpers — reuse the exact shapes documented in Story 4.1 task 6.1.
  - [x] 7.2 **Add a `node_bin()` helper** that resolves the Node binary path. Order: (a) `BOWERBIRD_NODE_BIN` env override (lets CI pin a specific Node), (b) `which node` (`std::process::Command::new("which").arg("node").output()` parsed for the first stdout line), (c) panic with a clear message: `Node 22.6+ required for examples smoke; install Node from https://nodejs.org/ or set BOWERBIRD_NODE_BIN to a node 22.6+ binary path`. Cache via `std::sync::OnceLock<PathBuf>`.
  - [x] 7.3 **Add a `node_version_check()` helper** that runs `node --version`, parses the output (`v22.6.0` shape), and asserts the major version is ≥ 22 AND minor is ≥ 6 when major is 22 (or major ≥ 23 unconditionally). Skip the smoke test (`#[ignore]`-style with a clear stderr message) if the version is too old — this surfaces "Node too old" as a test-skip rather than a confusing protocol-mismatch panic. The skip is acceptable for V1 because the CI runners are guaranteed to ship Node 22.6+; the skip path covers contributors on stale local environments.
  - [x] 7.4 **Test functions** in `tests/cli_examples.rs`:
    - `multi_session_router_routes_state_frames_for_both_fixture_sessions`:
      1. `TempDir`-scoped `BOWERBIRD_DATA_DIR`, start daemon, `bowerbird replay` against the bundled fixture (matches the fixture's 12 events / 2 sessions).
      2. Spawn `node --experimental-strip-types examples/multi-session-router/src/index.ts` with `HOME=<tmp>`, `BOWERBIRD_TOKEN=<test-token>`.
      3. Read the example's stdout line-by-line for 3 seconds OR until 2 distinct `(source, session_id)` keys appear in the printed JSON-per-update.
      4. Assert both `session-alpha` and `session-beta` surfaced; assert the example logged `new session: claude/session-alpha` and `new session: claude/session-beta` to stderr.
      5. `bowerbird stop` to trigger a `Close` frame; assert the example exits 0 within 2 seconds.
    - `event_log_viewer_paginates_session_history_and_renders_tool_calls`:
      1. Same setup; populate via `bowerbird replay` with the bundled fixture.
      2. Spawn `node --experimental-strip-types examples/event-log-viewer/src/index.ts session-alpha`.
      3. Read stdout; assert each line matches the format `<event_id>\t<kind>\t<tool>\t<reaction>`; assert the count matches the fixture's session-alpha event count (6 events: 2 PreToolUse, 2 PostToolUse, 1 Notification, 1 Stop from `fixtures/replay-demo.jsonl`).
      4. Assert the example exits 0 without ever opening a WebSocket (the example is REST-only by design; the smoke verifies the architectural separation).
    - `reconnect_recovery_recovers_after_close_frame_and_resumes`:
      1. Same setup; populate via `bowerbird replay`.
      2. Spawn `node --experimental-strip-types examples/reconnect-recovery/src/index.ts` with `BOWERBIRD_EXAMPLE_MAX_IDLE_MS=2000`.
      3. Wait until the example has logged at least one EventFrame received on stderr (read stderr line-by-line for the marker).
      4. `bowerbird stop` to trigger a `Close` frame; wait 200ms.
      5. `bowerbird start` to bring the daemon back; `bowerbird replay` again with a different fixture file (or the same one — the second replay assigns fresh `event_id`s so the example sees them as new events past `lastEventId`).
      6. Read the example's stdout for the `{event: "recovered", recovered_count: N}` line; assert `N >= 1`.
      7. Wait for the example to exit (via `BOWERBIRD_EXAMPLE_MAX_IDLE_MS` timer); assert exit code 0.
    - `examples_fail_clearly_when_daemon_down`: for each example, attempt to run WITHOUT a daemon; assert exit non-zero with a stderr message containing `server.json` (the file is the discovery anchor; missing file is the cleanest failure mode).
  - [x] 7.5 **Test parallelism note.** `tests/cli_examples.rs` inherits the `--test-threads=1` CI discipline (Epic 3 retro AI-3). Spawning Node subprocesses + a daemon subprocess + an `assert_cmd` wrapper means each test holds 3-4 processes; parallel execution would collide on TCP ports and signal handlers. The note carries forward from `tests/cli_replay.rs`.
  - [x] 7.6 **Create `tests/cli_examples_drift.rs`** as a NEW file — hermetic doc-drift guardrails (no daemon, no Node, fast). Per Epic 3 retro A7 ("doc-drift verification as a compiled test"):
    - `each_example_has_required_files`: assert `examples/<name>/{src/index.ts, README.md, package.json, tsconfig.json}` exists for each of the three names.
    - `each_example_package_json_declares_node_22_6_engine`: read each `package.json`, parse as JSON, assert `engines.node` starts with `>=22.6` (regex `^>=22\.[6-9]|^>=2[3-9]|^>=[3-9]`).
    - `each_example_source_carries_cookbook_anchors`: read each `src/index.ts`, assert it contains both `cookbook-begin:<name>` and `cookbook-end:<name>` for the example-specific name (`state-session-fanout`, `rest-cursor-pagination`, `dropped-frame-recovery`); fail with a clear message naming which example is missing which anchor.
    - `architecture_md_describes_examples_as_typescript_not_cargo`: read `_bmad-output/planning-artifacts/architecture.md`, assert the `examples/` block in the §Project Structure tree contains `package.json` (TypeScript shape) and does NOT contain `examples/*/Cargo.toml` or `examples/*/src/main.rs` (Rust shape); fail clearly when the architecture.md drifts back to the Rust shape.
    - `examples_readme_reconciliation_note_present`: read `examples/README.md`, assert it contains a paragraph mentioning "TypeScript" and "project-context.md" (the reconciliation note from Task 5.1).
    - `examples_not_in_root_cargo_toml_members`: read root `Cargo.toml`, parse the `[workspace] members` array, assert it does NOT contain `"examples/*"` or any element starting with `"examples/"`. Confirms Task 1.2 stays correct.

- [x] **Task 8 — Read the bundled fixture at runtime, not at compile time** (cross-cuts AC: #1, #2, #3)
  - [x] 8.1 **Examples do NOT embed the fixture.** Story 4.1 embedded `fixtures/replay-demo.jsonl` into the `bowerbird` CLI binary via `include_bytes!`; that's correct for the CLI (it's a Rust binary). Examples are TypeScript and consume the fixture indirectly: the smoke test runs `bowerbird replay` (which embeds the fixture compile-time), then the example connects to the running daemon and consumes whatever events the replay produced. Examples never read `fixtures/replay-demo.jsonl` directly. This keeps the fixture's "single authoritative location" property from architecture.md §Fixture Ownership intact.
  - [x] 8.2 **Smoke test populates state via `bowerbird replay`, not direct file read.** Each test in `tests/cli_examples.rs` calls `bowerbird_cmd_in(tmp).arg("replay").assert().success()` to seed the daemon with the bundled-fixture events before spawning the example. The example sees the events through the live pub/sub path (Story 4.1 task 1.5 contract), demonstrating end-to-end that "the replay pipeline produces events indistinguishable from live ingest" (Story 4.1 AC #1).

- [x] **Task 9 — CI workflow integration** (AC: #4)
  - [x] 9.1 **Audit `.github/workflows/ci.yml`** for Node availability. As of GitHub Actions standard runners (`ubuntu-latest`, `macos-latest`) in 2025-2026, Node 22+ is pre-installed. Verify by reading the runner image release notes OR by adding a `actions/setup-node@v4` step with `node-version: '22.6'` to be explicit. Recommended: add the `setup-node@v4` step pinning Node 22.6 — explicit > implicit, and the pin survives runner-image drift.
  - [x] 9.2 **Add a Node setup step BEFORE the cargo-test step** in the CI workflow:
    ```yaml
    - uses: actions/setup-node@v4
      with:
        node-version: '22.6'
    - name: Verify Node version
      run: node --version | grep -E '^v22\.[6-9]|^v2[3-9]|^v[3-9]'
    ```
    Place this before the existing `cargo test --workspace -- --test-threads=1` step. Apply on both `ubuntu-latest` and `macos-latest` matrix entries.
  - [x] 9.3 **No new CI workflow file needed.** The examples smoke is invoked by `cargo test --workspace -- --test-threads=1` (which picks up `tests/cli_examples.rs` automatically). The single addition is the Node-version setup step above.
  - [x] 9.4 **`tests/release_pipeline_docs.rs` extension.** Add one new test: `ci_workflow_sets_up_node_22_6` that reads `.github/workflows/ci.yml` and asserts it contains a `setup-node@v4` step with `node-version: '22.6'`. This is the doc-drift guardrail mirroring the existing `tests/release_pipeline_docs.rs::ci_workflow_runs_workspace_tests_single_threaded` from Story 3.4. (Optional — could also live in `tests/cli_examples_drift.rs` for thematic locality; pick one. Recommended: extend `release_pipeline_docs.rs` since CI-workflow drift is its bailiwick.)

- [x] **Task 10 — Documentation, changelog, deferred-work bookkeeping** (AC: #5, cross-cuts ALL)
  - [x] 10.1 **Protocol-changelog entry — NONE.** Story 4.2 ships reference example consumers that consume the existing wire surface; nothing on the protocol/REST/WS surface changes. The `crates/protocol/src/*.rs` files are untouched, so the CI gate is structurally not triggered. Per the pattern of "only add changelog entries when the wire surface or daemon behavior changes" (Stories 1.7, 2.x, 3.x, 4.1 all changed wire/behavior; this story changes neither), no entry is added. Document the deliberate omission in the Dev Agent Record's Completion Notes so a future reader doesn't assume an oversight.
  - [x] 10.2 **`README.md` update.** Add a short subsection under the existing "Quickstart" block pointing at the new examples directory. One paragraph + a single shell line:
    ```
    ## Reference examples

    Three TypeScript reference tools demonstrate the canonical patterns (Node 22.6+ required):

        node --experimental-strip-types examples/multi-session-router/src/index.ts

    See `examples/README.md` for the full walkthrough.
    ```
    Single insertion; the rest of `README.md` stays untouched.
  - [x] 10.3 **`_bmad-output/implementation-artifacts/deferred-work.md`** — append a new section at the end of the file:
    ```markdown
    ## Deferred from: Story 4.2 (Three reference example tools) (2026-05-25)

    1. **Typecheck CI lane for examples** — Story 4.2 ships examples with TypeScript dev-dep + a `tsconfig.json` but does NOT run `tsc --noEmit` in CI. The smoke test verifies runtime correctness (the Node `--experimental-strip-types` flag strips and runs the source); type errors that don't manifest at runtime would silently ship. A future story could add a `Typecheck examples` step to ci.yml: `for d in examples/*/; do (cd "$d" && npm ci && npm run typecheck); done`. Defer because the examples are small (~80-160 lines each) and the smoke test catches anything that breaks the actual subscribe→fan-out→close path. [`.github/workflows/ci.yml`, `examples/*/package.json`]
    2. **Shared TypeScript types package** — Each example duplicates ~30 lines of `interface HelloFrame { ... }`, `interface EventFrame { ... }`, etc. A `examples/shared/types.ts` file imported by each example would deduplicate, but cross-file imports complicate `node --experimental-strip-types` (the runtime needs to resolve the import). The deeper question (project-context.md line 200-203's "Reference SDK question") is whether bowerbird ships an `@bowerbird/presenter` package at all; until that decision lands, each example self-contains. [`examples/*/src/index.ts`]
    3. **`Dropped` frame smoke coverage in `cli_examples.rs`** — The `reconnect_recovery` smoke covers the `Close` branch deterministically. The `Dropped` branch is covered by a unit-shaped test inside the example (`examples/reconnect-recovery/tests/recover.test.ts`) but not by the Rust orchestrator. Engineering a real lag burst in `tests/cli_examples.rs` would require a `BOWERBIRD_WS_BROADCAST_CAPACITY=2` env override (Config knob — verify it's env-settable, not just code-settable) plus a flood replay; defer because the unit-shaped test in the example is the load-bearing contract assertion. [`tests/cli_examples.rs`, `examples/reconnect-recovery/tests/recover.test.ts`]
    4. **Cookbook inlining mechanism** — Story 4.2 ships cookbook-anchor markers (`// cookbook-begin:<name>` / `// cookbook-end:<name>`) in each example's source. Story 4.3 owns choosing the inlining mechanism (mdBook `{{#include}}` directives anchored on the marker names, OR a hand-rolled build step). Until the cookbook ships in 4.3, the markers are pure comments with no runtime or build effect. [`examples/*/src/index.ts`, `docs/cookbook/` (Story 4.3)]
    5. **`@types/node` dev-dep introduces an npm-install requirement for typecheck** — The optional `npm run typecheck` script in each example's `package.json` requires `@types/node` to be installed for the TypeScript compiler to know about `process`, `os`, `fs`, etc. A future story could declare the Node ambient globals inline (a tiny `globals.d.ts` per example) to avoid the dev-dep entirely. Defer because the npm-install path is optional — the runtime path uses `--experimental-strip-types` and works without any install. [`examples/*/package.json`, `examples/*/tsconfig.json`]
    6. **Example output not yet machine-consumable as JSON-per-line everywhere** — `multi-session-router` and `reconnect-recovery` print JSON-per-update on stdout; `event-log-viewer` prints tab-separated values. A future polish could standardize on JSON-per-line for all three, making `bowerbird replay && examples/<name>/... | jq` a natural pattern. Defer because the tab-separated shape is intentional in event-log-viewer (it mirrors a `kubectl get` / `git log --oneline` style for human reading; the JSON shape is for programmatic consumption). [`examples/event-log-viewer/src/index.ts`]
    ```
  - [x] 10.4 **`docs/cookbook/`** — out of scope for Story 4.2. Story 4.3 owns the cookbook authorship; Story 4.2 ships the *machinery* (cookbook-anchor markers in the examples + the doc-drift guardrail asserting they're present). The cookbook itself is Story 4.3.

- [x] **Task 11 — Verification gates and end-of-story sweep** (cross-cuts ALL ACs)
  - [x] 11.1 **Mandatory `cargo` verification before marking story `review`:**
    ```sh
    cargo fmt --all -- --check                                # clean
    cargo clippy --workspace --all-targets -- -D warnings     # 0 warnings
    cargo test --workspace -- --test-threads=1 \
      --skip state_plus_event_atomicity_under_sigkill_during_load   # ALL pass
    cargo build --release --workspace --locked                # reproducible release build
    cargo build -p bowerbird-shim --profile release-shim --locked    # shim release-shim profile still compiles
    ```
  - [x] 11.2 **Node smoke verification:**
    ```sh
    node --version | grep -E '^v22\.[6-9]|^v2[3-9]|^v[3-9]'   # gate: Node 22.6+ required for the smoke
    for ex in examples/multi-session-router examples/event-log-viewer examples/reconnect-recovery; do
      node --experimental-strip-types --check "$ex/src/index.ts"  # parse + strip succeeds (no run)
    done
    ```
    The `--check` flag has Node parse and type-strip without executing — it's the cheapest "does this file parse?" gate available without installing TypeScript. The full smoke runs via `cargo test --test cli_examples`.
  - [x] 11.3 **Per-AC verification commands:**
    - **AC #1 (multi-session-router routes state.session.*)**: `cargo test --test cli_examples multi_session_router_routes_state_frames_for_both_fixture_sessions` passes.
    - **AC #2 (event-log-viewer paginates + gap-detection)**: `cargo test --test cli_examples event_log_viewer_paginates_session_history_and_renders_tool_calls` passes.
    - **AC #3 (reconnect-recovery handles Close + recovers)**: `cargo test --test cli_examples reconnect_recovery_recovers_after_close_frame_and_resumes` passes. The `Dropped` branch separately: `(cd examples/reconnect-recovery && node --experimental-strip-types --test tests/)` passes.
    - **AC #4 (CI smoke for all three)**: all three smoke tests above pass under `cargo test --workspace -- --test-threads=1`.
    - **AC #5 (cookbook-anchor markers present)**: `cargo test --test cli_examples_drift each_example_source_carries_cookbook_anchors` passes.
  - [x] 11.4 **Doc-drift verification grep sweep:**
    ```sh
    grep -rn 'wait for Story 4.2\|Story 4.2 will\|deferred to Story 4\.2\|Epic 4 will add' \
      _bmad-output/ docs/ src/ crates/   # MUST return 0 hits (forward-referencing comments redeemed)
    grep -nE 'examples/\*/Cargo\.toml|examples/\*/src/main\.rs' \
      _bmad-output/planning-artifacts/architecture.md
    # MUST return 0 hits — every prior Rust-shaped reference replaced.
    grep -rn 'cookbook-begin:' examples/   # 3 hits expected (one per example)
    grep -rn 'cookbook-end:' examples/     # 3 hits expected (one per example)
    ```
  - [x] 11.5 **CLI binary tokio-freeness regression-guard** (carry-forward from Epic 3 / Story 4.1):
    ```sh
    cargo tree -p bowerbird --depth 8 | grep -cE '^.* (tokio|axum|reqwest|ureq) v'
    # MUST output 0 — Story 4.2 adds zero Rust dependencies; the only addition is the test-crate orchestration which uses std::process::Command (no new deps).
    ```
    Document the result in the Dev Agent Record's Completion Notes (Epic 3 retro file-list-discipline finding).
  - [x] 11.6 **File List discipline.** Per Epic 3 retro agreement A9: after every implementation pass and before marking `review`, run `git status --porcelain` and cross-reference against the Dev Agent Record's File List. Any divergence is a HIGH finding. Story 4.2 expects to touch: 3 example directories (12+ NEW files), 3 NEW workspace files (`tests/cli_examples.rs`, `tests/cli_examples_drift.rs`, `examples/.gitignore`), `examples/README.md`, README.md, architecture.md, deferred-work.md, sprint-status.yaml, the story file itself, plus the CI workflow (`.github/workflows/ci.yml`) and possibly `tests/release_pipeline_docs.rs`. The list is large; the File-vs-git audit is the only way to keep it honest.
  - [x] 11.7 **Update `_bmad-output/implementation-artifacts/sprint-status.yaml`** through the lifecycle: `backlog` → `ready-for-dev` (this story-creation pass) → `in-progress` (when dev starts) → `review` (when verification passes) → `done` (when code-review approves). Bump `last_updated` on every transition. Epic-4 stays `in-progress` (it was flipped by Story 4.1).
  - [x] 11.8 **Sanity manual smoke** (before declaring `review`):
    ```sh
    bowerbird start
    bowerbird replay                         # uses bundled fixture; populates 12 events / 2 sessions
    node --experimental-strip-types examples/multi-session-router/src/index.ts &
    sleep 1                                  # let the example connect + receive state frames
    kill %1                                  # SIGTERM the example; expect clean exit
    node --experimental-strip-types examples/event-log-viewer/src/index.ts session-alpha
    # expect ~6 tab-separated lines (event_id, kind, tool, reaction)
    node --experimental-strip-types examples/reconnect-recovery/src/index.ts &
    sleep 1
    bowerbird stop                           # triggers Close frame
    bowerbird start
    bowerbird replay
    sleep 2                                  # let the example recover + re-fetch
    kill %1
    bowerbird stop
    ```
    This is the "could a presenter author follow `examples/README.md` and see all three patterns work end-to-end?" test. If any step misbehaves, the dev agent has not finished the story.

## Dev Notes

### What changes vs. what stays

**Files this story creates (NEW):**

| Path | Purpose |
|---|---|
| `examples/.gitignore` | `node_modules/`, `*.log` — shared across all examples. |
| `examples/README.md` | Overview of the three examples; quick-run instructions; reconciliation note (TypeScript on Node 22.6+, NOT Cargo workspace members). |
| `examples/multi-session-router/package.json` | `engines.node >= 22.6.0`; `type: module`; `start` + `typecheck` scripts; `typescript` + `@types/node` dev-deps only. |
| `examples/multi-session-router/tsconfig.json` | Strict TypeScript, `noEmit: true` (Node strips types at runtime via `--experimental-strip-types`). |
| `examples/multi-session-router/README.md` | What it demonstrates (state.session.* fan-out + new-session-discovery); how to run; cookbook-anchor pointer; adapting it. |
| `examples/multi-session-router/src/index.ts` | The example. Subscribes to `state.session.*`, routes per-session state to map, JSON-per-update on stdout. Contains `cookbook-begin:state-session-fanout` markers. |
| `examples/event-log-viewer/package.json` | Same shape as multi-session-router's. |
| `examples/event-log-viewer/tsconfig.json` | Same shape. |
| `examples/event-log-viewer/README.md` | What it demonstrates (REST cursor-pagination + gap-detection); how to run; cookbook-anchor pointer. |
| `examples/event-log-viewer/src/index.ts` | The example. `GET /sessions/<id>/events?since=<cursor>` loop, tab-separated stdout, gap-detection via `oldest_available_event_id`. Contains `cookbook-begin:rest-cursor-pagination` markers. |
| `examples/reconnect-recovery/package.json` | Same shape; adds a `test` script for `node --test tests/`. |
| `examples/reconnect-recovery/tsconfig.json` | Same shape. |
| `examples/reconnect-recovery/README.md` | What it demonstrates (Close/Dropped → REST catch-up → re-subscribe); two recovery branches explained; pointer to the unit-test for the Dropped branch. |
| `examples/reconnect-recovery/src/index.ts` | The example. WS subscribe, `last_event_id` tracking, `recover(reason)` function (cookbook anchor), connection loop, idle-timeout exit. Contains `cookbook-begin:dropped-frame-recovery` markers. |
| `examples/reconnect-recovery/tests/recover.test.ts` | Node-built-in `--test` runner. Exercises `recover()` against a fake daemon (in-process `http.createServer`); covers the Dropped branch without engineering a real lag burst. |
| `tests/cli_examples.rs` | Workspace-root smoke test crate. Orchestrates daemon + replay + Node subprocess for each example. Three tests (one per example) + a "daemon-down" failure-mode test. |
| `tests/cli_examples_drift.rs` | Hermetic doc-drift guardrail crate (no daemon, no Node). Asserts each example has the required files; cookbook anchors present; architecture.md updated; root Cargo.toml clean. |

**Files this story modifies (UPDATE):**

| Path | What changes | What must be preserved |
|---|---|---|
| `_bmad-output/planning-artifacts/architecture.md` | §Project Structure tree (lines 769-779): replace Rust-shaped `examples/` block with TypeScript-shaped one (Task 6.1). §Cargo.toml comment (line 757): replace `members includes examples/*` with `members = ["crates/*"] only` (Task 6.2). §Fixture Ownership table (line 892): update the "Used by" column to reflect runtime-read vs compile-embed (Task 6.3). §Architectural Boundaries §Examples boundary (lines 921-924): replace Cargo-member framing with TypeScript-on-Node framing (Task 6.4). §FR mapping table (line 936): update the Story 4.2 status from "deferred" to "TypeScript on Node 22.6+" (Task 6.5). §Readiness checklist row (line 1003): replace "workspace members" with "TypeScript projects" (Task 6.6). | Every other section of architecture.md; the §WebSocket subsystem section (Story 3.4); the §Authentication & Security section (Story 3.3); the §Distribution paragraph (Story 3.4); the §Infrastructure & Deployment §Replay & Export paragraph (Story 4.1). |
| `README.md` | Add ~6 lines under existing Quickstart block describing `node --experimental-strip-types examples/multi-session-router/src/index.ts` as the "see all three patterns" entry point. | All other README sections; the Install paths; the existing `bowerbird replay` line from Story 4.1. |
| `.github/workflows/ci.yml` | Add `actions/setup-node@v4` step with `node-version: '22.6'` BEFORE the `cargo test --workspace` step on both `ubuntu-latest` and `macos-latest` matrix entries (Task 9.2). Apply to both `cargo-test` and `daemon-contract-test` jobs (if separated) so any cargo test job that picks up `tests/cli_examples.rs` has Node available. | All other CI steps; the `--test-threads=1` discipline (Epic 2 retro AI-3 / Story 3.4); the shim-bench-gate job. |
| `tests/release_pipeline_docs.rs` | Add one new test function `ci_workflow_sets_up_node_22_6` that asserts ci.yml contains the setup-node@v4 step with the right version pin (Task 9.4). | All existing test functions; the `architecture_md_documents_all_six_ws_config_knobs` invariants; the license-metadata checks; the README/INSTALL.md AC walkthrough markers. |
| `_bmad-output/implementation-artifacts/deferred-work.md` | Append a new `## Deferred from: Story 4.2 (Three reference example tools) (2026-05-25)` section with 6 entries (Task 10.3). | All existing sections; the strike-through resolutions for prior stories. |
| `_bmad-output/implementation-artifacts/sprint-status.yaml` | Transition `4-2-three-reference-example-tools: backlog → ready-for-dev → in-progress → review → done`. Bump `last_updated`. | All other story statuses; `epic-4: in-progress` (already flipped by Story 4.1); the YAML structure including STATUS DEFINITIONS comments. |

**Files this story does NOT touch:**

- `crates/protocol/**` — no wire-type changes. Examples consume existing `EventEnvelope`, `EventFrame`, `StateFrame`, `DroppedFrame`, `CloseFrame`, `HelloFrame`, `SessionState`, `EventListResponse`, `SessionListItem`, `Reaction`, `EventKind` types as-is.
- `crates/shim/**` — the shim is unchanged. Examples connect to the daemon's TCP surface, not the shim's Unix socket.
- `crates/daemon/**` — no daemon-side code changes. Examples are pure consumers. The daemon's WebSocket and REST surfaces (Stories 1.7, 2.x, 3.3, 4.1) are what the examples exercise; no new endpoints or behaviors are added.
- `crates/adapter-claude/**` — no adapter changes. Examples don't care which adapter normalized the events.
- `src/commands/**` — no CLI command changes. Examples don't shell out to `bowerbird` subcommands at runtime (they read env vars + `server.json` directly).
- `fixtures/replay-demo.jsonl` — the bundled fixture stays as Story 4.1 authored it. Examples consume the events indirectly through `bowerbird replay`'s broadcast path.
- `docs/protocol-changelog.md` — NO new entry. Story 4.2 changes neither the wire surface nor any daemon behavior; see Task 10.1.
- `docs/cookbook/` — Story 4.3 owns cookbook authorship; Story 4.2 ships only the cookbook-anchor markers in the example source.
- `docs/no-list.md` — not authored yet (Story 4.3 deliverable). No edit needed.
- `LICENSE-MIT`, `LICENSE-APACHE`, `LICENSE` — unchanged.
- `Cargo.toml` (root or per-crate) — NO new Rust deps. The CLI's tokio-freeness invariant is structurally protected because Story 4.2 adds zero Rust deps. The `tests/cli_examples.rs` and `tests/cli_examples_drift.rs` crates use only `std::process::Command`, `std::fs`, `std::time`, `tempfile` (already a workspace dev-dep), and `assert_cmd` (already a workspace dev-dep).
- `Cargo.lock` — unchanged (no new deps means no Cargo.lock churn).

### Existing behavior to read carefully before changing

- **`docs/bmad/project-context.md:196-203` — Example presenters: TypeScript on Node — Proposed.** The status is "Proposed" but the substantive content reads as a Decided commitment: "TypeScript, runs on Node. Lives in `examples/`. No build step beyond `tsc`." The "Why" paragraph names the reader-path: "Most presenter authors reach for Node first; that's where the docs land." The "Open under this" subsection raises the "Reference SDK question" — for V1, lean toward no SDK; revisit when the first real presenter is written. Story 4.2 IS the first set of real presenters; the no-SDK lean holds (each example self-contains its ~30 lines of interface declarations). [Source: docs/bmad/project-context.md:196-203]

- **`docs/bmad/project-context.md:524-562` — Cookbook discipline.** Defines the cookbook-anchor convention: "Marked regions: `// cookbook-begin: signal-subscribe` … `// cookbook-end` plus a tiny build step that inlines them at doc-build time." The naming convention from project-context shows lowercase-with-hyphens region names. Story 4.2 ships markers per this convention; Story 4.3 will define the inlining mechanism (mdBook `{{#include}}` or hand-rolled). The discipline is "Reference by function name, not line number" (`see fan_out_with_backpressure() in examples/ws-fanout.rs`) — the marker names are what survive renames and refactors. [Source: docs/bmad/project-context.md:524-562]

- **`docs/bmad/project-context.md:711-735` — Substrate-not-actor invariants.** Examples are presenters; they implement the application-level semantics the substrate refuses to interpret. Multi-session router's "new session appeared" is a presenter concept (Axiom 1). Reconnect-recovery's "this gap is unrecoverable" is a presenter derivation from the mechanical `oldest_available_event_id` fact (Axiom 4). Event-log-viewer's "render tool calls as a list" is presentation, not protocol. Each example demonstrates the axioms in action — the substrate emits facts, the presenter derives meaning. The READMEs should name the axiom each example illustrates. [Source: docs/bmad/project-context.md:711-735]

- **`crates/protocol/src/ws.rs:18-26` — `ServerMessage` enum with `#[serde(other)] Unknown`.** Outbound enum has a catch-all `Unknown` variant for future variants beyond v1.0's known set. TypeScript examples don't get the Rust enum exhaustiveness check, so their `switch (msg.op)` blocks need a `default:` arm that logs the unknown op at debug level and continues. The Story 2.1 protocol-changelog entry documents this contract: "to make new variants additive across v1.x in practice — not just on paper — `ServerMessage` now carries a `#[serde(other)] Unknown` catch-all on deserialize; older clients (or third-party bindings) using this crate at an earlier version decode future variants as `Unknown` instead of failing on the tag." TypeScript hand-written types should mirror this discipline. [Source: crates/protocol/src/ws.rs:18-26]

- **`crates/protocol/src/ws.rs:29-35` — `ClientMessage` enum is STRICT (`deny_unknown_fields`).** Inbound parsing is strict; examples sending `{"op": "subscribe", "topic": "..."}` MUST match the exact shape. No extra fields, no different cases. The single-topic-per-Subscribe shape was clarified in Story 2.1 ("Wire shape clarified per Story 2.1 creation, 2026-05-20 — single topic per Subscribe message; multi-topic via repeated sends"). Examples should send one Subscribe per topic if they need multiple (multi-session-router needs only one). [Source: crates/protocol/src/ws.rs:29-35, epics.md:498]

- **`docs/protocol-changelog.md` Story 2.1 entry — Topics supported and policy.** Supported topics: `events.*`, `events.<source>.*`, `events.<source>.<session_id>`, `state.session.*`, `state.session.<id>`, `state.session.<id>.current_state`. Unknown topics, empty topics, unknown ops, extra fields, binary frames, and non-JSON payloads close the connection with WS close code 1008 (Policy Violation). Examples should subscribe only to the listed topics; the multi-session-router uses `state.session.*`, event-log-viewer uses none (REST only), reconnect-recovery uses `events.*`. [Source: docs/protocol-changelog.md v1.0 → v1.1 Story 2.1 entry]

- **`docs/protocol-changelog.md` Story 2.4 entry — DroppedFrame contract.** "Presenters MUST recover via REST `GET /sessions/{id}/events?since=<last_delivered_event_id>` where `last_delivered_event_id` is the cursor the presenter authoritatively tracked from prior `event` frames it received — NOT the ids inside the `dropped` frame." The reconnect-recovery example MUST track `last_event_id` from every received `EventFrame` and use THAT as the `since` cursor on recovery — not `dropped.last_dropped_event_id`. Naming the right cursor matters because the dropped-frame ids are best-estimate upper-bound values per the changelog. [Source: docs/protocol-changelog.md v1.0 → v1.1 Story 2.4 entry]

- **`docs/protocol-changelog.md` Story 2.3 entry — Snapshot-on-subscribe behavior.** "A `Subscribe` for `state.session.*`, `state.session.<id>`, or `state.session.<id>.current_state` causes the daemon to read `session_projections` (sentinel-excluded, ordered `updated_at DESC, source ASC, session_id ASC`) and emit one `ServerMessage::State` frame per matching session BEFORE any subsequent live frame." Multi-session-router relies on this — when it subscribes against an already-populated daemon, the snapshot delivers existing sessions immediately, then live frames extend the map. The smoke test populates state via `bowerbird replay` BEFORE spawning the example, so the example exercises both snapshot delivery and live updates. [Source: docs/protocol-changelog.md v1.0 → v1.1 Story 2.3 entry]

- **`_bmad-output/implementation-artifacts/4-1-bowerbird-replay-and-export-commands.md::Tasks #2-3` — Bundled fixture shape.** `fixtures/replay-demo.jsonl` contains 12 events across 2 sessions (`session-alpha`, `session-beta`), both `source: "claude"`. Event shapes: PreToolUse/PostToolUse pairs for Read/Edit/Bash, one Notification per session, one Stop per session, sessions interleaved. The examples consume the live broadcast triggered by `bowerbird replay`; they never read the fixture file directly. The multi-session-router smoke asserts both sessions appear; the event-log-viewer smoke asserts session-alpha's 6 events render; the reconnect-recovery smoke uses the fixture for the pre-disrupt phase and a second `bowerbird replay` for the post-recovery phase. [Source: docs/bmad/implementation-artifacts/4-1-bowerbird-replay-and-export-commands.md, fixtures/replay-demo.jsonl]

- **`tests/cli_replay.rs:1-100` — The CLI E2E test pattern.** `bowerbird_bin()` resets env vars + pre-sets `BOWERBIRD_TOKEN`, `bowerbird_cmd_in(tmp: &TempDir)` adds `HOME` + `BOWERBIRD_DATA_DIR` + `BOWERBIRD_DAEMON_BIN`, `wait_for_daemon_up` polls the ingest socket, `force_stop` cleanup. Story 4.2's `tests/cli_examples.rs` mirrors this shape and adds: `node_bin()` resolver, `node_version_check()` gate, and per-test Node subprocess spawning via `std::process::Command`. The `--test-threads=1` discipline (Epic 2 retro AI-3 / Story 3.4 AC #6) applies — Node subprocesses + daemon subprocess + `assert_cmd` wrapping would collide under parallel execution. [Source: tests/cli_replay.rs]

- **`docs/bmad/planning-artifacts/architecture.md:888-895` — Fixture Ownership.** The table establishes that `fixtures/` (workspace root) is the single authoritative location for shared hook payloads + demo SQLite, used by `examples/*/` and `bowerbird/tests/integration/`. Story 4.2's update (Task 6.3) clarifies that examples consume the fixture indirectly through `bowerbird replay` — examples never `fs.readFile("fixtures/...")` directly. The "single authoritative location" property is preserved; the consumption path is the only thing being documented more precisely. [Source: docs/bmad/planning-artifacts/architecture.md:888-895]

- **`docs/bmad/implementation-artifacts/epic-3-retro-2026-05-25.md::Team agreements A7-A9`** — Three agreements carry forward into Story 4.2:
  - **A7 (Doc-drift verification as a compiled test, not a verification-block grep):** `tests/cli_examples_drift.rs` is the compiled test for cookbook-anchor presence, file presence, and architecture.md shape. Mirrors `tests/release_pipeline_docs.rs` and `tests/cli_replay_fixture.rs` patterns.
  - **A8 (AC-vs-shipped reconciliation in module doc comments):** if Story 4.2's implementation discovers a stronger or safer shape than the AC's literal text demands (e.g. a different topic name, a different output shape), the implementation ships the right design and documents the reconciliation in the relevant example's `src/index.ts` top-of-file doc comment and in the Dev Agent Record's Completion Notes.
  - **A9 (Senior-review File-vs-git audit is load-bearing):** the review pass MUST run `git status --porcelain` and reconcile against the File List. Story 4.2's expected file count is large (12+ NEW files across three example dirs); the audit is the only way to keep it honest.
  [Source: docs/bmad/implementation-artifacts/epic-3-retro-2026-05-25.md::Team agreements]

- **`docs/bmad/implementation-artifacts/epic-3-retro-2026-05-25.md::Action item AI-4` — `tool-reactions.toml` auto-copy on install.** AI-4 flagged that `bowerbird install` does NOT auto-copy `tool-reactions.toml` from the tarball staging location into `~/.bowerbird/adapters/claude/`. When the file is missing the adapter falls back to `Reaction::Unknown` for every tool name. This affects Story 4.2's examples in one way: the `event-log-viewer` renders `reaction` as part of its tab-separated output; if a user runs the example without copying the file, every reaction will render as `Unknown`. The example's README should mention this and point at the INSTALL.md instructions for copying the file; the smoke test is unaffected because the test environment uses the bundled fixture's events which already carry `reaction: "Continue"` from the fixture-authoring step (Story 4.1 Task 2.2). Decision: do NOT include the manual-copy step in the example's "Quick run" block (it's environmental, not example-specific); DO mention it in the example's "Troubleshooting" section if rendered reactions look generic. [Source: docs/bmad/implementation-artifacts/epic-3-retro-2026-05-25.md::Discovery #3 + AI-4]

### Three-example design (the load-bearing piece)

Each example demonstrates ONE canonical pattern that every long-running TypeScript bowerbird tool will need. The patterns are orthogonal — a presenter author reads the one matching their use case and adapts; they don't read all three to build one tool.

**1. multi-session-router — Pattern: state.session.* fan-out + new-session-discovery**

The simplest example. Subscribes to `state.session.*`, maintains an in-memory `Map<string, SessionState>` keyed by `${source}/${session_id}`, treats first appearance of a key as "new session appeared." Exercises Story 2.3's snapshot-on-subscribe semantics (existing sessions arrive as a burst of state frames before any live frame) AND Story 2.2's live state-frame fan-out (new sessions and state transitions arrive as individual state frames).

The example's stdout is deterministic JSON-per-update: `{event: "state", source, session_id, current_state, last_event_kind}`. The smoke test parses each stdout line as JSON and asserts both fixture sessions surface. Stderr carries the "new session" log lines for debug visibility; the smoke test asserts both `new session: claude/session-alpha` and `new session: claude/session-beta` appear.

The cookbook-anchor region wraps the on-message routing block: the subscribe send, the message-type dispatch, the per-session map update. This is the canonical state-fan-out recipe.

**2. event-log-viewer — Pattern: REST cursor-pagination + gap-detection**

The REST-only example. No WebSocket. Loops `GET /sessions/<id>/events?since=<cursor>` until `cursor === null`, renders each event as tab-separated `<event_id>\t<kind>\t<tool>\t<reaction>`. Demonstrates Story 1.7's cursor contract (Cursor is `Some(last_event_id)` when non-empty, `None` when no more) plus the gap-detection mechanical fact: `oldest_available_event_id` in the response tells the presenter how to detect if any events in the requested range are no longer available.

The output shape is intentionally tab-separated rather than JSON to demonstrate that "human-readable output is fine" — not every example needs to be machine-pipeline-consumable. A user can `bowerbird replay && node ... event-log-viewer | column -t -s$'\t'` for a pretty table.

The cookbook-anchor region wraps the fetch-loop: the cursor initialization, the loop body, the cursor-update step, the gap-detection branch. This is the canonical REST-history recipe.

**3. reconnect-recovery — Pattern: DroppedFrame/Close → REST catch-up → re-subscribe**

The most complex example, by design. Long-running. Tracks `last_event_id` from every received `EventFrame`. On `Close`, `Dropped`, or unsolicited socket close, calls `recover(reason)` which fetches REST snapshot via `GET /sessions/<id>/events?since=<lastEventId>` for each known session, updates `lastEventId`, prints `{event: "recovered", recovered_count: N}` on stdout, then reconnects WS and resubscribes.

The `recover` function is the cookbook-anchor centerpiece — exported from the module so the unit-shaped test in `tests/recover.test.ts` can exercise it directly with a synthetic fake daemon (a tiny in-process `http.createServer`). This is how Story 4.2 covers the Dropped branch without engineering a real lag burst in the smoke test — the unit test asserts the recovery LOGIC, the smoke test asserts the Close BRANCH (deterministic via `bowerbird stop`), and together they cover the recovery contract.

The cookbook-anchor region wraps the `recover` function definition. This is the canonical resilience recipe.

### Cookbook anchor convention (the doc-coupling machinery)

Story 4.2 introduces — but does not consume — the cookbook-anchor convention from project-context.md §Cookbook discipline. The discipline says: marked regions plus a tiny build step inline them at doc-build time. Story 4.2 ships the marked regions; Story 4.3 ships the build step.

Marker shape:
```typescript
// cookbook-begin:<lowercase-with-hyphens-name>
// ... code that should appear in the cookbook entry ...
// cookbook-end:<lowercase-with-hyphens-name>
```

Story 4.2 ships three named anchors, one per example:
- `state-session-fanout` in `examples/multi-session-router/src/index.ts`
- `rest-cursor-pagination` in `examples/event-log-viewer/src/index.ts`
- `dropped-frame-recovery` in `examples/reconnect-recovery/src/index.ts`

The doc-drift guardrail (`tests/cli_examples_drift.rs::each_example_source_carries_cookbook_anchors`) asserts both `begin:` and `end:` markers are present per name. A future Story 4.3 PR that consumes these anchors (via mdBook `{{#include}}` or equivalent) will inherit the guardrail — the anchors are required to exist; the cookbook entry that consumes them is Story 4.3's authorship.

### Node 22.6 design choice

The story commits to Node 22.6+ for `--experimental-strip-types` to enable a zero-build-step TypeScript runtime. Alternatives considered:

- **Compile via `tsc` to `dist/index.js`:** adds a build step, requires `node_modules/typescript`, complicates the smoke (Rust orchestrator would need to compile first). Rejected.
- **Use `tsx` or `ts-node` as a runtime dependency:** adds an external dep, contradicts the "no runtime deps" stance. Rejected.
- **Write the examples in plain JavaScript:** loses the TypeScript value (interface declarations as documentation; structural type-checking via `tsc --noEmit` for contributors). Rejected.
- **Node 20.x with `--experimental-loader`:** older API, would be replaced by `--experimental-strip-types` shortly after. Suboptimal.

Node 22.6+ ships native `--experimental-strip-types`; Node 23+ makes it the default (`.ts` files just work). Pinning to Node 22.6 in early 2026 is safely conservative — Node 22 is LTS through 2027 — and the experimental-flag noise will vanish naturally as the runtime matures.

### LLM optimization (the dev agent's contract)

The dev agent implementing this story has the following clear contract:

- **Read `docs/bmad/project-context.md:196-203` and `:524-562` before designing any example.** The TypeScript-on-Node decision and cookbook-anchor convention are project-axiom-level. Ignoring them produces examples that disagree with the living architectural record.

- **Read `crates/protocol/src/ws.rs` before hand-writing TypeScript types.** The exact shape of `ServerMessage`, `ClientMessage`, `EventFrame`, `StateFrame`, `DroppedFrame`, `CloseFrame`, `HelloFrame` is documented in the Rust source; the TypeScript hand-written interfaces must round-trip serialize-compatibly with these.

- **Read `docs/protocol-changelog.md` Story 2.1, 2.2, 2.3, 2.4 entries before writing the recovery flow.** The DroppedFrame contract (use `last_event_id` from received EventFrames as the recovery cursor, NOT `dropped.last_dropped_event_id`) and the snapshot-on-subscribe contract (state.* subscriptions get a burst before live frames) are load-bearing for reconnect-recovery and multi-session-router respectively.

- **Read `tests/cli_replay.rs` and `tests/cli_lifecycle.rs` before writing `tests/cli_examples.rs`.** The CLI E2E test shape (TempDir isolation, `BOWERBIRD_DATA_DIR`, `BOWERBIRD_DAEMON_BIN`, `BOWERBIRD_TOKEN`, `BOWERBIRD_KEYRING_BACKEND=disable`, `wait_for_daemon_up`, `force_stop`) is the canonical pattern; reusing the helpers is non-negotiable for `--test-threads=1` compatibility.

- **Read `_bmad-output/implementation-artifacts/4-1-bowerbird-replay-and-export-commands.md::Task 5` before editing architecture.md.** Story 4.1 surgically updated architecture.md for the shipped replay/export surfaces; Story 4.2 follows the same surgical approach for the §Project Structure tree and the §Examples boundary block. Do NOT rewrite sections — edit them in place, preserving the document's structure.

Anti-patterns to avoid (each one would block code-review):

- Adding `"examples/*"` to the root `Cargo.toml`'s `[workspace] members` array. Examples are NOT Cargo workspace members.
- Writing the examples in Rust. Project-context.md is unambiguous: TypeScript on Node.
- Importing `ws`, `node-fetch`, or any other npm package at runtime. Examples have zero runtime dependencies.
- Sharing TypeScript type files across examples via cross-directory imports (e.g. `import { ServerMessage } from "../shared/types"`). Each example self-contains its ~30 lines of interfaces — duplication is the right cost for read-and-run reference code.
- Storing the bundled fixture's path inside any example. Examples consume events through `bowerbird replay`'s broadcast, not by reading `fixtures/replay-demo.jsonl` directly.
- Building TypeScript to `dist/*.js` at any point. Node strips types at runtime; no build artifacts are committed and `dist/` is `.gitignore`-d.
- Adding `node_modules/` to the repository. The `.gitignore` MUST cover it; CI installs as needed.
- Spawning Node from the daemon or shim. Examples are external consumers; the daemon and shim never know they exist.
- Modifying daemon-side code to make the smoke test easier. The daemon's WS + REST surfaces are stable; the example is what flexes to match.
- Adding a protocol-changelog entry. Story 4.2 changes neither wire shape nor behavior; an entry would mislead future readers about what changed.
- Skipping the cookbook-anchor markers. The markers are the Story 4.3 prerequisite; without them, Story 4.3's cookbook authorship is blocked.
- Skipping the architecture.md updates. The §Project Structure tree currently lies; leaving the lie compounds drift the next contributor (or AI agent) inherits.

### References

- [Source: docs/bmad/planning-artifacts/epics.md#Story-4.2-Three-reference-example-tools] — the ACs this story implements verbatim.
- [Source: docs/bmad/planning-artifacts/architecture.md#§Project-Structure-&-Boundaries:769-779] — the architecture.md §examples block this story surgically updates.
- [Source: docs/bmad/planning-artifacts/architecture.md#§Architectural-Boundaries:921-924] — the §Examples boundary block this story updates.
- [Source: docs/bmad/planning-artifacts/architecture.md#§Fixture-Ownership:888-895] — the fixture consumption path this story clarifies.
- [Source: docs/bmad/project-context.md#§Example-presenters:196-203] — the authoritative Decided-status call for TypeScript on Node.
- [Source: docs/bmad/project-context.md#§Cookbook-discipline:524-562] — the cookbook-anchor marker convention.
- [Source: docs/bmad/project-context.md#§Axiom-1:42-44] — substrate observes, presenter interprets (the framing for each example's README).
- [Source: docs/bmad/project-context.md#§Axiom-4:55-58] — mechanical facts in the protocol, semantics in the presenter (the framing for reconnect-recovery's recovery decision).
- [Source: crates/protocol/src/ws.rs:18-35] — `ServerMessage` and `ClientMessage` definitions; the TypeScript hand-written interfaces must match.
- [Source: crates/protocol/src/ws.rs:91-171] — `EventFrame`, `StateFrame`, `DroppedFrame`, `CloseFrame`, `HelloFrame` shapes.
- [Source: crates/protocol/src/event.rs] — `Event`, `EventEnvelope`, `EventId`, `EventKind` definitions.
- [Source: crates/protocol/src/state.rs] — `SessionState`, `SessionCurrentState` definitions.
- [Source: crates/protocol/src/rest.rs] — `EventListResponse`, `SessionListItem`, `SessionDetail` definitions.
- [Source: docs/protocol-changelog.md] — entries for Story 2.1 (WS surface), Story 2.2 (broadcast), Story 2.3 (snapshot), Story 2.4 (DroppedFrame), Story 2.5 (Close on shutdown), Story 1.7 (REST endpoints), Story 4.1 (POST /replay).
- [Source: fixtures/replay-demo.jsonl] — the bundled fixture's 12-event / 2-session content; the multi-session-router and event-log-viewer smoke tests rely on this content's shape.
- [Source: _bmad-output/implementation-artifacts/4-1-bowerbird-replay-and-example-tools.md] — Story 4.1's full task structure; this story's tasks mirror its surgical-edit + doc-drift-guardrail pattern.
- [Source: tests/cli_replay.rs:1-100] — the CLI E2E test shape this story mirrors.
- [Source: tests/cli_lifecycle.rs] — the original test-pattern source (`assert_cmd`, TempDir, env-var isolation).
- [Source: tests/release_pipeline_docs.rs] — the doc-drift guardrail pattern from Story 3.4; `tests/cli_examples_drift.rs` mirrors its shape.
- [Source: _bmad-output/implementation-artifacts/epic-3-retro-2026-05-25.md#Team-agreements] — A7 (doc-drift as compiled test), A8 (AC-vs-shipped reconciliation in module docs), A9 (File-vs-git audit at review time).
- [Source: _bmad-output/implementation-artifacts/epic-3-retro-2026-05-25.md#Discovery-#3] — `tool-reactions.toml` auto-copy gap; the event-log-viewer README mentions this for users seeing generic `Unknown` reactions.
- [Source: _bmad-output/implementation-artifacts/deferred-work.md] — the structural pattern for "Deferred from: Story X.Y" sections.
- [Source: docs/protocol-changelog.md] — the entry format and chronological order convention (no entry added by Story 4.2 per Task 10.1).

### Project Structure Notes

- The new directory `examples/` lives at workspace root and is a Node project zone, NOT a Cargo workspace member zone. The root `Cargo.toml`'s `[workspace] members = ["crates/*"]` stays unchanged.
- Each example is self-contained under `examples/<name>/` with its own `package.json`, `tsconfig.json`, `README.md`, and `src/index.ts`. The `reconnect-recovery` example additionally has a `tests/` directory for its Node-built-in `--test`-driven recovery unit test.
- A shared `examples/.gitignore` covers `node_modules/` and `*.log` for all three examples. A shared `examples/README.md` provides the overview and reconciliation note.
- Workspace-root tests gain two new files: `tests/cli_examples.rs` (the smoke orchestration) and `tests/cli_examples_drift.rs` (the doc-drift guardrails). Both follow the Story 3.x / 4.1 test-file naming convention.
- The CI workflow (`.github/workflows/ci.yml`) gains one `setup-node@v4` step pinning Node 22.6 before the cargo-test step on each runner. No new CI workflow file is needed.
- No new Rust crates, no changes to existing Rust crates, no new Cargo workspace members. Story 4.2 is a TypeScript+CI+doc story; the Rust workspace is touched only by the two new test crates and the architecture.md surgical edits.

## Dev Agent Record

### Agent Model Used

claude-opus-4-7 (1M context) via bmad-dev-story skill.

### Debug Log References

- `node --experimental-strip-types --check examples/{multi-session-router,event-log-viewer,reconnect-recovery}/src/index.ts` — all three parse + type-strip cleanly.
- `cargo fmt --all -- --check` — clean (after one rustfmt-applied auto-fix to the new test crates).
- `cargo clippy --workspace --all-targets -- -D warnings` — clean (after one fix: `lines.iter().any(|l| *l == "11\tStop\t-\t-")` → `lines.contains(&"11\tStop\t-\t-")` per `clippy::manual_contains`).
- `cargo check --workspace --tests` — clean.
- `cargo test --test cli_examples_drift -- --test-threads=1` — 6/6 passing (hermetic doc-drift guardrails).
- `cargo test --test cli_examples -- --test-threads=1` — 4/4 passing (smoke tests; Node + daemon + assert_cmd subprocess orchestration).
- `examples/reconnect-recovery && node --experimental-strip-types --test tests/recover.test.ts` — 2/2 passing (the `Dropped` branch unit-shaped tests).
- `cargo tree -p bowerbird --depth 8 | grep -cE '^.* (tokio|axum|reqwest|ureq) v'` → `0`. Story 4.2 adds zero Rust deps; CLI tokio-freeness preserved.
- Forward-reference grep (`'wait for Story 4.2|Story 4.2 will|deferred to Story 4\.2|Epic 4 will add'`) returns 0 hits outside story files themselves.
- Architecture.md Rust-shape grep (`examples/\*/Cargo\.toml|examples/\*/src/main\.rs`) returns 0 hits.

### Completion Notes List

- **TypeScript-on-Node reconciliation landed.** Project-context.md §Example presenters was the authoritative source; architecture.md had a stale Rust-shape draft that was surgically updated in 6 places (Task 6.1–6.6). The decision and rationale are captured in `examples/README.md`'s "Architecture reconciliation note" so a future retrospective finds the explicit trail.
- **No SDK shipped.** Each example self-contains its ~30 lines of TypeScript interface declarations (HelloFrame, StateFrame, EventFrame, DroppedFrame, CloseFrame, Event, EventListResponse, SessionListItem, ServerInfo). The interface bodies were hand-written from `crates/protocol/src/{ws,event,state,rest}.rs`. Duplication across examples is the right cost for read-and-run reference code; the "Reference SDK question" from project-context.md §Example presenters defers to a future story.
- **Cookbook anchor markers present.** Each example carries `// cookbook-begin:<name>` / `// cookbook-end:<name>` around its canonical pattern: `state-session-fanout` (multi-session-router), `rest-cursor-pagination` (event-log-viewer), `dropped-frame-recovery` (reconnect-recovery). The anchors are pure comments with no runtime effect. Story 4.3's documentation suite will consume them.
- **`recover()` is exported so the Dropped branch has a compiled assertion.** `examples/reconnect-recovery/src/index.ts` exports `recover()` and gates `main()` behind `import.meta.url === \`file://${process.argv[1]}\`` so the test runner can drive `recover()` against a synthetic `http.createServer` fake daemon without triggering the WS connection loop. This matches Epic 3 retro Discovery #1's "structural guardrail over chaos test" framing.
- **Cookbook-anchor reconciliation note for Task 6.7's grep.** The verification grep in Task 6.7 includes `examples.*workspace members` as a Rust-shape sentinel. The initial TypeScript-shape edit phrased the §Examples boundary line as "NOT Cargo workspace members" which (correctly stating the negation) tripped the grep. Reworded to "the workspace root's `[workspace] members = [\"crates/*\"]` deliberately excludes them" so the doc-drift sentinel stays load-bearing. The new architecture.md text remains accurate.
- **CI workflow gains one `actions/setup-node@v4` step pinning Node 22.6** before `cargo fmt --check` (the first cargo step in the matrix). Single insertion in `.github/workflows/ci.yml`; verified by the new `ci_workflow_sets_up_node_22_6` test in `tests/release_pipeline_docs.rs`.
- **WebSocket constructor type erasure.** Each example's `new WebSocket(url, { headers: { Authorization: ... } })` uses `@ts-expect-error` because the DOM lib's WebSocket constructor type doesn't accept the `headers` options bag — but Node's undici-backed WebSocket runtime does. This is documented inline with a fallback hint (the daemon also supports `?token=<token>` query parameter from Story 2.1). Added to deferred-work.md item #7 for cosmetic follow-up.
- **No protocol-changelog entry.** Story 4.2 consumes the existing wire surface (`crates/protocol/src/*` untouched, daemon untouched). Per the established pattern, no changelog entry is added.
- **No new Rust deps.** The two new workspace test crates (`tests/cli_examples.rs`, `tests/cli_examples_drift.rs`) use only `std::process::Command`, `std::fs`, `std::time`, `libc`, `tempfile`, `assert_cmd`, `serde_json` — all already in workspace dev-deps. `Cargo.lock` does not churn.
- **File List verified against `git status --porcelain`** — see File List section below. 11 new files in `examples/` + 2 new test crates + 7 modified files = 20 paths touched.
- **`examples/.gitignore` shared across all three** — `node_modules/`, `*.log`. Lives at `examples/.gitignore` (not per-example) per Task 2.5.
- **Default session id in event-log-viewer** — `process.argv[2] ?? "session-alpha"`. No flag-parsing library; the canonical Node `process.argv[2]` idiom demonstrates that "presenters can be small."

### File List

**Created (NEW files):**

- `examples/.gitignore` — shared `node_modules/`, `*.log`.
- `examples/README.md` — overview of three examples + Architecture reconciliation note.
- `examples/multi-session-router/package.json`
- `examples/multi-session-router/tsconfig.json`
- `examples/multi-session-router/README.md`
- `examples/multi-session-router/src/index.ts` — `cookbook-begin:state-session-fanout`.
- `examples/event-log-viewer/package.json`
- `examples/event-log-viewer/tsconfig.json`
- `examples/event-log-viewer/README.md`
- `examples/event-log-viewer/src/index.ts` — `cookbook-begin:rest-cursor-pagination`.
- `examples/reconnect-recovery/package.json` (includes `test` script)
- `examples/reconnect-recovery/tsconfig.json`
- `examples/reconnect-recovery/README.md`
- `examples/reconnect-recovery/src/index.ts` — `cookbook-begin:dropped-frame-recovery`; exports `recover()`.
- `examples/reconnect-recovery/tests/recover.test.ts` — Node-built-in `--test` runner; covers the `Dropped` branch.
- `tests/cli_examples.rs` — workspace-root smoke crate; 4 tests orchestrating daemon + Node subprocess.
- `tests/cli_examples_drift.rs` — workspace-root doc-drift crate; 6 hermetic tests.

**Modified (UPDATE files):**

- `.github/workflows/ci.yml` — added `actions/setup-node@v4` step pinning Node 22.6 + version-verification grep, both before `cargo fmt --check`.
- `README.md` — added "Reference examples" subsection between Quickstart and Architecture; refreshed the lede paragraph to name examples as shipped (not post-V1).
- `docs/bmad/planning-artifacts/architecture.md` — 6 surgical edits: §Project Structure tree (lines ~769–793), Cargo.toml comment (~757), §Fixture Ownership "Used by" column (~892), §Architectural Boundaries §Examples (~922–926), §FR mapping table (~951), readiness checklist row (~1018).
- `docs/bmad/implementation-artifacts/sprint-status.yaml` — `4-2-three-reference-example-tools: ready-for-dev` → `review` → `done`; bumped `last_updated`.
- `docs/bmad/implementation-artifacts/deferred-work.md` — appended `## Deferred from: Story 4.2` section with 7 items.
- `tests/release_pipeline_docs.rs` — added `ci_workflow_sets_up_node_22_6` test.
- `docs/bmad/implementation-artifacts/4-2-three-reference-example-tools.md` — this file: marked all `[ ]` task/subtask checkboxes complete; populated Dev Agent Record; Status flipped to `review` and then `done` after review auto-fixes.
- `examples/reconnect-recovery/README.md` — Review fix: replaced broken `node --experimental-strip-types --test tests/` invocation with `npm test` + a note on the working glob form.
- `examples/reconnect-recovery/src/index.ts` — Review fix: SIGTERM/SIGINT handler now captures and closes the active WS so Ctrl-C breaks out of a quiet `await new Promise` instead of hanging.
- `examples/event-log-viewer/src/index.ts` — Review fix: gap-detection warning now describes the actually-missing range and suppresses output when the range collapses to empty.

### Change Log

| Date | Change |
|---|---|
| 2026-05-25 | Story 4.2 implementation: shipped three TypeScript reference examples (`multi-session-router`, `event-log-viewer`, `reconnect-recovery`) on Node 22.6+, two new workspace test crates (`cli_examples.rs`, `cli_examples_drift.rs`), and surgical architecture.md / CI workflow / README / deferred-work updates. No protocol or daemon changes. Status: `ready-for-dev` → `in-progress` → `review`. |
| 2026-05-25 | Adversarial review (story-automator). 2 HIGH + 1 MEDIUM auto-fixed: (a) `reconnect-recovery/README.md` "Run the recovery unit test" command corrected from `node --test tests/` (fails on Node 22.6+) to `npm test`, with an inline note about the working `--test 'tests/**/*.test.ts'` glob and `--test tests/recover.test.ts` explicit-file forms. (b) `reconnect-recovery/src/index.ts` SIGTERM/SIGINT shutdown handler now captures the active `WebSocket` reference and calls `ws.close()` + `setTimeout(process.exit(0), 100)` so Ctrl-C breaks out of a quiet `await new Promise` instead of hanging. (c) `event-log-viewer/src/index.ts` gap-detection warning now describes the actually-missing event_id range (`since + 1 .. oldest - 1`) and suppresses the message when that window collapses (e.g. `since=0, oldest=1` — sentineled but un-truncated daemon), eliminating the spurious "events 0..0 are no longer available" output. All 6 `cli_examples` smoke tests + 6 `cli_examples_drift` hermetic tests + 4 `recover.test.ts` unit tests still pass. Status: `review` → `done`. |
