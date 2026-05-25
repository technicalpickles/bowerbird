# Story 4.1: bowerbird replay and export commands

Status: done

<!-- Note: Validation is optional. Run validate-create-story for quality check before dev-story. -->

## Story

As a tool builder,
I want to replay a recorded event sequence through bowerbird's full pub/sub path and export real sessions to replay files,
so that I can develop and debug my tools against realistic event streams without needing a live Claude Code session.

## Acceptance Criteria

1. **Given** a JSONL file whose every non-blank, non-`#`-prefixed line deserializes as a `protocol::Event` (post-store form: `event_id`, `source`, `session_id`, `kind`, `reaction?`, `payload`, `created_at`) **When** I run `bowerbird replay <file>` against a running daemon **Then** each event is forwarded to a new daemon endpoint `POST /replay` (bearer-auth), which strips `event_id` + `created_at`, constructs a `protocol::EventEnvelope`, pushes it onto the existing `ingest_tx` channel, and the channel→`ingest::writer::run`→`projection::session::write` path persists + publishes the envelope **exactly as if it had arrived via the ingest socket** — subscribed WebSocket clients receive `EventFrame` + `StateFrame` pairs in the order the JSONL lines appear (per-session ordering preserved). The CLI reports `replayed N events from <path>` on stdout and exits 0; per-line JSON parse failures are reported as `line N: <error>` on stderr and the replay continues with the remaining lines (best-effort, not transactional — replay is a development tool).

2. **Given** a live session in the daemon's SQLite event log identified by `<session-id>` (matching the `session_id` column for `source = "claude"`; non-Claude sources arrive only when a second adapter ships and are out of V1 scope) **When** I run `bowerbird export <session-id>` (with optional `-o <path>` / `--output <path>`; default stdout) **Then** the CLI calls `GET /sessions/<session-id>/events?since=0` against the running daemon (bearer-auth, resolved via the existing `commands::auth::resolve_token_for_cli` chain), iterates the cursor-paginated response (looping on `cursor` until it is `None`; today the V1 endpoint returns the full history in a single page per `deferred-work.md` Story 1.7 entry, but the loop is structural so a future page-limit story does not break export), writes each `Event` as one JSON line followed by `\n` to the chosen sink, and exits 0. A missing or unknown `<session-id>` (daemon 404) produces a clear stderr message `session <id> not found` and exits non-zero; `401` produces `daemon rejected bearer token; check ~/.bowerbird/config.toml or BOWERBIRD_TOKEN` (no token-value printed); transport failure (`Unreachable`) produces `cannot reach daemon at <addr>; is it running? (try 'bowerbird start')` and exits non-zero.

3. **Given** the `bowerbird` binary distribution **When** I run `bowerbird replay` with no `<file>` argument **Then** the command uses a bundled demo fixture embedded into the binary via `include_bytes!("../../fixtures/replay-demo.jsonl")` (so the fixture is part of the compiled artifact and does not need to be shipped or located at runtime), the CLI prints `using bundled fixture (<N> events across <M> sessions)` on stdout before replaying, and the resulting pub/sub traffic exactly mirrors the file-input path of AC #1. The bundled fixture lives at workspace-root `fixtures/replay-demo.jsonl` per `architecture.md:760-765` (the canonical fixtures location) and is consumed by both the CLI binary (compile-time embed) and `examples/*/` smoke tests (Story 4.2; runtime file read of the same path).

4. **Given** a replay file (or the bundled fixture) whose events span at least two distinct `(source, session_id)` keys **When** `bowerbird replay` runs **Then** WS subscribers to `state.session.*` receive a `StateFrame` for each distinct session as the projection upsert for that session fires (one StateFrame per event, per session, matching normal ingest semantics from Story 2.2); the bundled fixture MUST contain events for at least two distinct sessions so the no-arg replay demonstrates multi-session fan-out without requiring a custom file; a compiled doc-drift guardrail in `tests/cli_replay_fixture.rs::bundled_fixture_spans_at_least_two_sessions` asserts `fixtures/replay-demo.jsonl` parses cleanly into `Vec<Event>` and `events.iter().map(|e| (&e.source, &e.session_id)).collect::<HashSet<_>>().len() >= 2`.

5. **Given** a replay file event whose `created_at` timestamp is in the past relative to wall-clock at replay time **When** `bowerbird replay` processes it **Then** the daemon does not attempt to preserve original inter-event timing — events are forwarded to the daemon as fast as the bounded `ingest_tx` channel accepts them (no `tokio::time::sleep`, no per-event throttle); the daemon assigns a fresh `created_at` via `current_unix_millis()` at `projection::session::write` time (matching normal ingest semantics — the original `created_at` from the JSONL line is silently dropped along with `event_id`), so the replayed rows in `events.created_at` reflect replay wall-clock, not source wall-clock; replay is for development, not performance reproduction, and `docs/protocol-changelog.md` carries this rationale verbatim per Task 9.4 below so a future presenter author building against the replay path understands the timing contract.

6. **Given** `docs/bmad/planning-artifacts/architecture.md` reflects the **shipped** CLI surface after Stories 3.1–3.4 (the §CLI framework block at line 503 currently says "`replay` and `export` arrive in Story 4.1 (Epic 4)") **When** Story 4.1 lands **Then** the line is updated to list `replay` and `export` alongside the other shipped subcommands (alphabetical order: `auth token, export, install, replay, start, status, stop, uninstall`); the §Implementation Order block at line 531 drops the "`replay`/`export` arrive in Epic 4" trailing sentence; the §Project Structure tree at line 875 drops the "# Epic 4 will add: src/commands/{replay,export}.rs" comment (the files now exist); the FR-to-structure mapping table at line 932 changes "Epic 4 — `src/commands/{replay,export}.rs`, `examples/*/` (deferred)" to "`src/commands/{replay,export}.rs` (Story 4.1); `examples/*/` (Story 4.2 deferred)" so the surface differentiation between this story and Story 4.2 is documented. A compiled doc-drift guardrail in `tests/cli_replay_fixture.rs::architecture_md_lists_replay_and_export_as_shipped` asserts that the §CLI framework block lists both subcommands and that no live `Epic 4 will add` / `arrive in Story 4.1` comment survives. Mirrors the Story 3.4 `tests/release_pipeline_docs.rs` pattern (Epic 3 retro agreement A7 — doc-drift as compiled test, not verification-block grep).

7. **Given** Story 4.1 introduces a new authenticated REST endpoint `POST /replay` **When** Story 4.1 ships **Then** `docs/protocol-changelog.md` carries one new v1.0 → v1.1 entry under the existing section, `type: schema` (the v1.x compatibility guarantee is additive on outbound types and on the *set of available endpoints* — adding endpoints does not break v1.0 presenters that never call them, so this is the correct categorization per `crates/protocol/src/` policy and architecture.md §Protocol stability); the entry names the endpoint, its bearer-auth requirement, the JSONL request body shape (`Event` per line, ignoring `event_id`+`created_at`), the `200 {"replayed_count": N, "parse_errors": [{"line": N, "error": "..."}]}` response shape, the per-line continue-on-error policy, the relationship to the existing `ingest_tx` channel, the absence of rate-limiting on replay, and the wall-clock-rewrite contract from AC #5; it also cross-references the bundled-fixture path and the new `bowerbird export` reader path. The CI gate at `.github/workflows/ci.yml` (the `crates/protocol/src/*.rs` change → protocol-changelog.md entry rule) is NOT triggered by this story directly (no `crates/protocol/src/` files change), but the discipline is preserved by hand: Story 4.1 adds the entry alongside the implementation.

## Tasks / Subtasks

- [x] **Task 1 — Add the daemon-side `POST /replay` endpoint** (AC: #1, #4, #5, #7)
  - [x] 1.1 **Create `crates/daemon/src/api/replay.rs`** as a NEW file. Module doc comment names the contract: "Accepts JSONL of `protocol::Event` records on the request body; strips `event_id` + `created_at`; constructs `EventEnvelope`; pushes to `state.ingest_tx`. Per-line parse failures are collected and returned in the response body rather than failing the entire request — replay is a development tool, not a transactional ingest path." The endpoint is `bearer-auth-required` (it joins the `authenticated` Router branch in `crates/daemon/src/api/mod.rs:101-110`).
  - [x] 1.2 **Add `ingest_tx: tokio::sync::mpsc::Sender<protocol::EventEnvelope>` to `AppState`** in `crates/daemon/src/state.rs`. Today `ingest_tx` is owned exclusively by `ingest::listener::run_bound`'s spawn closure (see `crates/daemon/src/main.rs:195-219`); the new REST endpoint needs to push to the same channel. The cleanest refactor: construct the `(ingest_tx, ingest_rx)` pair, clone `ingest_tx` into both `AppState` and the listener task. `ingest_rx` stays in the writer task. Verify the channel capacity (`config.ingest_channel_capacity`, default 1024) remains sufficient — a `bowerbird replay` of a 10k-event file will fan out within capacity because the writer task drains continuously. If `try_send` returns `TrySendError::Full` from the replay path, return that line's parse error as `{"line": N, "error": "channel full (replay too fast)"}` and continue; do NOT block the HTTP handler.
  - [x] 1.3 **Request body parsing.** Accept any `Content-Type` (we are not opinionated; `application/json-seq` and `application/x-ndjson` and `text/plain` are all valid for JSONL). Use axum's `Body` extractor with the existing `RequestBodyLimitLayer::new(1 MiB)` cap (architecture.md:124, `crates/daemon/src/api/mod.rs:28`). For replay files exceeding 1 MiB, document the limit in the endpoint's `400 body too large` response and recommend chunking via multiple `POST /replay` calls. (V1 acceptable; a future Story can lift the cap or introduce a streaming variant.)
  - [x] 1.4 **Per-line processing loop.** Split the body by `\n`. For each non-empty, non-comment (`#`-prefixed) line: attempt `serde_json::from_str::<protocol::Event>(line)`. On `Ok`, construct `EventEnvelope { source: event.source, session_id: event.session_id, kind: event.kind, reaction: event.reaction, payload: event.payload }` (drop `event_id` + `created_at` — the daemon's writer reassigns both via AUTOINCREMENT + `current_unix_millis()`). Sentinel kinds (`EventKind::RecordingStarted`, `EventKind::RecordingEnded`) are *rejected* with a parse-error entry — `projection::session::write` already runtime-guards against sentinels (see `crates/daemon/src/projection/session.rs:56-65`), but rejecting at the replay boundary gives a clearer error message than a propagated `Error::Projection`. On `Err`, record `{"line": N, "error": serde_json error chain}` and continue.
  - [x] 1.5 **Response shape.** Return `200 application/json` with body `{"replayed_count": N, "parse_errors": [{"line": M, "error": "..."}, ...]}` where `replayed_count` is the number of successfully forwarded envelopes (counted at `try_send` success, not at downstream commit — downstream commit failures are recorded only in the daemon's tracing output per the existing `projection::session::write` policy, not the HTTP response, because the response is sync but the writer task is async). `parse_errors` is `[]` when every line parsed cleanly. The handler returns BEFORE downstream commits finish; the AC #1 promise that "subscribed WS clients receive the frames" is satisfied by the existing writer→broadcaster path, not by waiting for it in the HTTP response.
  - [x] 1.6 **Add the route to `api::router`.** In `crates/daemon/src/api/mod.rs:101-110`, add `.route("/replay", post(replay::run))` to the `authenticated` Router. The bearer-auth middleware applies automatically. Import: `use axum::routing::post;` at the top of `mod.rs`.
  - [x] 1.7 **Contract tests in `crates/daemon/tests/contract_daemon.rs`** under a new module `story_4_1_replay`:
    - `replay_forwards_events_through_broadcast_path`: spawn daemon → connect WS subscriber to `events.*` → POST 3 events to /replay → assert WS receives 3 EventFrames in order with the same `(source, session_id, kind, payload)` shape, and that `event_id` and `created_at` are *newly assigned* (not the JSONL values).
    - `replay_emits_state_frames_for_each_session`: POST a JSONL with 4 events across 2 sessions → WS subscribed to `state.session.*` receives 4 StateFrames (one per event per session — matching Story 2.2 semantics).
    - `replay_continues_on_per_line_parse_error`: POST a body with `valid\nINVALID\nvalid` → response is `{"replayed_count": 2, "parse_errors": [{"line": 2, "error": "..."}]}` → WS receives exactly 2 events.
    - `replay_rejects_sentinel_kinds`: POST a `RecordingStarted` event → that line is in `parse_errors` with `error: "sentinel kind cannot be replayed"` → no broadcast.
    - `replay_requires_bearer`: POST without auth → `401`. POST with wrong token → `401`. POST with the correct token + valid body → `200`.
    - `replay_dropped_event_id_and_created_at_are_reassigned`: POST `{"event_id": 999999, "created_at": 1, ...}` → check the resulting events row → `event_id` is the next AUTOINCREMENT value (much smaller than 999999 unless the test runs against a populated DB), `created_at` is within ±5s of test wall-clock, NOT `1`.
  - [x] 1.8 **Tracing.** Wrap the handler in `#[tracing::instrument(skip_all, fields(content_length))]` and emit one `info!` line per request with `replayed_count` + `parse_errors_count`. Per-line errors stay at `debug!` so a fixture with hundreds of lines does not spam the log. The endpoint follows the same `skip_all` + opt-in-field discipline as the rest of `api/`.

- [x] **Task 2 — Author the bundled replay fixture** (AC: #3, #4)
  - [x] 2.1 **Create `fixtures/replay-demo.jsonl`** as a NEW file at the workspace root. The directory `fixtures/` already exists in the architecture.md tree (line 760-765) but is not yet populated; this is the first inhabitant. The file's purpose is twofold: (a) compile-time embed into the `bowerbird` CLI binary via `include_bytes!`, (b) runtime fixture for Story 4.2's reference examples and their CI smoke tests. Both consumers read it as JSONL of `protocol::Event` records.
  - [x] 2.2 **Fixture content design.** ~12-15 events across exactly TWO sessions (`session-alpha`, `session-beta`) demonstrating a realistic interaction shape: PreToolUse → PostToolUse pairs for tool calls (mix of `Read`, `Edit`, `Bash` payloads matching Claude Code's hook shape), one Notification per session, one Stop per session. Both sessions use `source: "claude"`. Sessions are interleaved (not all of alpha then all of beta) so multi-session fan-out is observably demonstrated. `event_id` and `created_at` in the file are placeholder values (`event_id: 1..15`, `created_at: 1700000000000..1700000015000`); the daemon discards both at replay per AC #5. The fixture lines do NOT carry a top-level comment header (JSONL has no comment convention beyond what the parser allows; the loop in Task 1.4 honors `#`-prefixed lines but the bundled fixture stays pure JSONL so it round-trips through `bowerbird export`).
  - [x] 2.3 **Validation.** The fixture MUST parse as `Vec<protocol::Event>` via `serde_json::from_str` on each line; the `tests/cli_replay_fixture.rs::bundled_fixture_is_valid_jsonl` guardrail (Task 6.2 below) is the structural check. Hand-edit the fixture by writing each line, then validate locally: `for line in $(cat fixtures/replay-demo.jsonl); do echo "$line" | jq -e . > /dev/null || echo "BAD: $line"; done`. Re-validate after any edit — a malformed fixture would make every `bowerbird replay` (no-arg) fail.
  - [x] 2.4 **Wire-shape conformance.** Each line is a `protocol::Event`, NOT an `EventEnvelope` (the difference: `Event` has `event_id` + `created_at`; `EventEnvelope` does not). This is the same shape `bowerbird export` emits (Task 3) and the same shape Story 4.2's `examples/event-log-viewer` will consume. Keeping export's output and replay's input on the same wire shape means a user can `bowerbird export <session-id> | bowerbird replay /dev/stdin` round-trip. Verify: pick one line from `fixtures/replay-demo.jsonl`, ensure `serde_json::from_str::<protocol::Event>` succeeds with no `deny_unknown_fields` complaint (the outbound `Event` type is permissive per the asymmetric serde policy).

- [x] **Task 3 — Wire `src/commands/replay.rs`** (AC: #1, #3, #5)
  - [x] 3.1 **Create `src/commands/replay.rs`** as a NEW file. Add `pub mod replay;` to `src/commands/mod.rs:1-7` (alphabetical placement: between `mod install;` and `mod start;`).
  - [x] 3.2 **Args definition.**
    ```rust
    #[derive(Args)]
    pub struct ReplayArgs {
        /// Path to a JSONL file of protocol::Event records. Omit to use the
        /// bundled demo fixture embedded in the binary.
        pub file: Option<PathBuf>,
    }
    ```
    No other flags in V1. Future flags (`--rate-limit`, `--filter-session=<id>`, `--dry-run`) are tracked in deferred-work per Task 9.5.
  - [x] 3.3 **Bundled fixture embed.** At module top:
    ```rust
    /// Bundled at compile time from fixtures/replay-demo.jsonl (workspace root).
    /// The CARGO_MANIFEST_DIR-relative path resolves at compile time, so the
    /// resulting binary is self-contained — no runtime file lookup needed
    /// and `bowerbird replay` (no arg) works against the installed binary
    /// regardless of cwd or whether the source tree is present.
    const BUNDLED_FIXTURE: &[u8] = include_bytes!("../../fixtures/replay-demo.jsonl");
    ```
  - [x] 3.4 **Run flow.** `run(args)`:
    1. Resolve the body: `args.file` present → `std::fs::read(&path)?`; absent → `BUNDLED_FIXTURE.to_vec()`. Print `using bundled fixture (<N> events across <M> sessions)` on stdout (compute N, M by parsing the bundled bytes locally before POST). Or: print `replaying <path>` for the explicit-file case.
    2. Resolve the daemon's HTTP address via `commands::daemon::read_server_info(&bowerbird_dir)` + parse `bind_addr` — same pattern as `commands::status::run` (`src/commands/status.rs:58-77`). Degrade with a clear stderr message if `server.json` is missing.
    3. Resolve the bearer token via `commands::auth::resolve_token_for_cli()` — same pattern as `status.rs:79-88`. Degrade with `daemon not reachable; token resolution failed: <err>` on `Err`.
    4. Hand-roll an HTTP POST against `/replay` via `TcpStream` (mirror `commands::daemon::http_get_status` but `POST`, with `Content-Type: application/x-ndjson`, `Content-Length: <body.len()>`, `Authorization: Bearer <token>`). Implement as a new helper in `commands::daemon`: `pub fn http_post_replay(addr: SocketAddr, bearer: &str, body: &[u8], per_attempt: Duration) -> ReplayResponse` where `ReplayResponse` is `Ok(body: Vec<u8>) | Status(u16) | Unreachable`. Keep the helper alongside the existing `http_get_status` so the "no reqwest, no tokio in CLI" invariant stays explicit.
    5. Parse the 200 response body as `{"replayed_count": N, "parse_errors": [...]}`. Print to stdout: `replayed N events from <path-or-"bundled-fixture">`. If `parse_errors` is non-empty, print each as `  line <N>: <error>` on stderr.
    6. Non-200 paths: `401` → `daemon rejected bearer token; check ~/.bowerbird/config.toml or BOWERBIRD_TOKEN`; `Status(code)` → `daemon returned HTTP <code>`; `Unreachable` → `cannot reach daemon at <addr>; is it running? (try 'bowerbird start')`. Exit non-zero on any non-200.
  - [x] 3.5 **Add `Replay(commands::replay::ReplayArgs)` to the `Command` enum in `src/main.rs:25-44`** (alphabetical between `Install` and `Start`). Add the match arm:
    ```rust
    Command::Replay(args) => commands::replay::run(args).context("bowerbird replay"),
    ```
    The clap derive `#[command(about = "...")]` doc-comment becomes the help text; mirror the existing pattern:
    ```rust
    /// Replay a JSONL file of recorded events through the daemon's pub/sub
    /// path. Omit the file argument to use the bundled demo fixture.
    Replay(commands::replay::ReplayArgs),
    ```
  - [x] 3.6 **Output discipline.** The CLI prints exactly one progress line on success: `replayed N events from <path-or-"bundled-fixture">`. Per-line errors go to STDERR (so a user piping `bowerbird replay file.jsonl | ...` does not contaminate the consumer with diagnostic noise). The stdout stream is reserved for the progress line only; future extensions (e.g. a `--json-output` flag) can opt into stdout-as-data shape. This mirrors `bowerbird status`'s stdout/stderr discipline.

- [x] **Task 4 — Wire `src/commands/export.rs`** (AC: #2)
  - [x] 4.1 **Create `src/commands/export.rs`** as a NEW file. Add `pub mod export;` to `src/commands/mod.rs:1-7` (alphabetical placement: between `mod daemon;` and `mod install;`).
  - [x] 4.2 **Args definition.**
    ```rust
    #[derive(Args)]
    pub struct ExportArgs {
        /// The session_id to export (matches the session_id column in
        /// `/sessions/{id}/events`). Source is "claude" in V1; multi-source
        /// disambiguation arrives when a second adapter ships per the
        /// deferred-work entry from Story 1.7.
        pub session_id: String,

        /// Output path; default stdout.
        #[arg(short, long)]
        pub output: Option<PathBuf>,
    }
    ```
  - [x] 4.3 **Run flow.** `run(args)`:
    1. Resolve daemon address + bearer token via the same `read_server_info` + `resolve_token_for_cli` chain as Task 3.4 steps 2-3.
    2. Loop: call `commands::daemon::http_get_events(addr, &bearer, &args.session_id, since, per_attempt)` (NEW helper — Task 4.4 below). Each response is a `protocol::EventListResponse`. Write each `Event` as one JSON line to the output sink. Update `since` to the response's `cursor`. Continue until `cursor` is `None`.
    3. On any non-200 response: 404 → `session <id> not found` exit 1; 401 → bearer-rejected message exit 1; other → `daemon returned HTTP <code>` exit 1; transport failure → unreachable message exit 1.
    4. On success: `exported N events from session <id> to <path-or-"stdout">` to STDERR (not stdout — stdout is reserved for the JSONL data when `-o` is absent so `bowerbird export <id> | bowerbird replay /dev/stdin` round-trips). Exit 0.
  - [x] 4.4 **New helper `commands::daemon::http_get_events`.** Mirror `http_get_status`'s shape, but the URL is `/sessions/<id>/events?since=<cursor>`. Returns a `Result<Vec<u8>, ExportError>` where ExportError covers 404, 401, transport, parse. Keep the helper alongside `http_get_status` so a future audit of CLI↔daemon HTTP touchpoints finds them in one place.
  - [x] 4.5 **JSONL emission discipline.** Each event is `serde_json::to_string(&event)?` + `\n`. Use `BufWriter` around the output sink (stdout or file) so we do not syscall per event. On file output (`-o <path>`), open with `OpenOptions::new().create(true).truncate(true)` (a re-export overwrites). Write through `BufWriter`, flush on close. No partial-file semantics — an incomplete export (process killed mid-write) leaves a truncated file the user can detect by tail-line check; explicit atomic-rename was considered but rejected as overkill for V1 (the user can re-run on failure).
  - [x] 4.6 **Add `Export(commands::export::ExportArgs)` to the `Command` enum in `src/main.rs`** (alphabetical between `Auth` and `Install`). Match arm:
    ```rust
    Command::Export(args) => commands::export::run(args).context("bowerbird export"),
    ```
    With clap doc:
    ```rust
    /// Export a session's event history as JSONL of protocol::Event records.
    /// Pipe to `bowerbird replay /dev/stdin` to round-trip through the
    /// daemon's pub/sub path on a different machine or after a fresh
    /// `bowerbird install`.
    Export(commands::export::ExportArgs),
    ```

- [x] **Task 5 — Architecture.md updates** (AC: #6)
  - [x] 5.1 **Update `_bmad-output/planning-artifacts/architecture.md:503`** — the §CLI framework block currently reads "Subcommands (top-level, alphabetical): `auth token`, `install`, `start`, `status`, `stop`, `uninstall`. `replay` and `export` arrive in Story 4.1 (Epic 4). `version` is provided by clap's built-in `--version` flag." Replace with: "Subcommands (top-level, alphabetical): `auth token`, `export`, `install`, `replay`, `start`, `status`, `stop`, `uninstall`. `version` is provided by clap's built-in `--version` flag." (Drop the deferral sentence; alphabetize `export` and `replay` into the list.)
  - [x] 5.2 **Update `architecture.md:531`** — the §Implementation Order block currently ends "...singleton, system-keychain token resolver. `replay`/`export` arrive in Epic 4." Replace the trailing sentence: "`replay`/`export` ship in Story 4.1." Same line — surgical edit.
  - [x] 5.3 **Update `architecture.md:875`** — the project-structure tree currently has a trailing comment `# Epic 4 will add: src/commands/{replay,export}.rs`. The files now exist; delete that comment line. Add the two files into the tree block above (around line 871-872, alphabetical with the existing `install.rs`, `start.rs`, etc.):
    ```
    │       ├── export.rs                   # `bowerbird export <session-id>` — fetch /sessions/{id}/events, write JSONL
    │       ├── install.rs                  # `bowerbird install` — settings.json merge + daemon spawn
    │       ├── replay.rs                   # `bowerbird replay [<file>]` — POST /replay with JSONL body (bundled fixture default)
    │       ├── start.rs                    # `bowerbird start`
    ```
  - [x] 5.4 **Update `architecture.md:932`** — the FR-to-structure mapping table row reads `| FR31–FR35: Developer tools + examples | Epic 4 — `src/commands/{replay,export}.rs`, `examples/*/` (deferred) |`. Replace with: `| FR31–FR35: Developer tools + examples | `src/commands/{replay,export}.rs` (Story 4.1); `examples/*/` (Story 4.2 deferred); `docs/cookbook/` (Story 4.3 deferred) |`. This makes the per-story split explicit so a future reader can find the responsible story by feature.
  - [x] 5.5 **Add a new short section under §Infrastructure & Deployment (around line 510, after the "Distribution" block)** named "Replay & Export":
    ```markdown
    **Replay & Export (Story 4.1):**
    - `bowerbird replay [<file>]` reads JSONL of `protocol::Event` records, POSTs them to the daemon's new `POST /replay` endpoint (bearer-auth). The daemon strips `event_id` + `created_at`, constructs `EventEnvelope`s, and pushes them onto the existing `ingest_tx` channel — so replayed events flow through the same `ingest::writer::run` → `projection::session::write` → broadcast path as live ingest. The CLI's no-arg form uses a bundled fixture embedded via `include_bytes!("../../fixtures/replay-demo.jsonl")`. Replay does NOT preserve original inter-event timing; events are forwarded as fast as the channel accepts them. (`crates/daemon/src/api/replay.rs`, `src/commands/replay.rs`, `fixtures/replay-demo.jsonl`)
    - `bowerbird export <session-id>` reads `/sessions/<session-id>/events?since=<cursor>` in a cursor-paginated loop and writes JSONL of `protocol::Event` records to stdout (or `-o <path>`). The output shape is the input shape for `bowerbird replay`, so `bowerbird export <id> | bowerbird replay /dev/stdin` round-trips an entire session through the pub/sub path on the same daemon (or, after `bowerbird export <id> > session.jsonl`, on a different machine after `bowerbird install`). (`src/commands/export.rs`)
    ```
    This is the single new top-level paragraph this story adds to architecture.md beyond the surgical edits above. It documents the design choice (replay-via-ingest-channel) so a future reader does not have to reverse-engineer it from `crates/daemon/src/api/replay.rs`.
  - [x] 5.6 **Verification grep sweep.** After the edits:
    ```sh
    grep -nE 'arrive in Story 4.1|Epic 4 will add|replay/export arrive' _bmad-output/planning-artifacts/architecture.md
    # MUST return 0 hits — all four legacy references replaced or deleted.
    grep -nE 'replay|export' _bmad-output/planning-artifacts/architecture.md | grep -v deferred
    # Should show the new shipped-surface references in §CLI framework, §Implementation Order, §Project Structure tree, §FR mapping, §Infrastructure & Deployment.
    ```

- [x] **Task 6 — Workspace-level CLI E2E test suite** (AC: #1, #2, #3, #4, #5, #6)
  - [x] 6.1 **Create `tests/cli_replay.rs`** as a NEW file at the workspace root. Mirror the structure of `tests/cli_lifecycle.rs` (Story 3.2 / 3.3 pattern): `bowerbird_bin()` helper resets `BOWERBIRD_*` env vars and pre-sets `BOWERBIRD_TOKEN` to a known test value, `bowerbird_cmd_in(tmp: &TempDir)` configures per-test isolation, `wait_for_daemon_up` polls the ingest socket, `force_stop` cleanup helper for panics.
  - [x] 6.2 **Test functions** (each `#[test]` per the workspace's `--test-threads=1` discipline; daemon contract-test job in CI inherits this — see `.github/workflows/ci.yml`):
    - `replay_with_explicit_file_forwards_events_to_subscribed_ws`: start daemon, connect a `tokio_tungstenite` WS client subscribed to `events.*`, write a small JSONL to a TempDir file, `bowerbird replay <file>`, assert the WS client receives the expected events in order. (Note: this test DOES need a `tokio` runtime to drive the WS client — keep it in `#[tokio::test(flavor = "current_thread")]` per the existing daemon contract-test pattern. The CLI itself stays tokio-free.)
    - `replay_with_no_argument_uses_bundled_fixture`: `bowerbird replay` (no arg), assert stdout starts with `using bundled fixture (`, exit 0, and the daemon's `/sessions` list grew by the fixture's session count.
    - `replay_continues_after_invalid_lines`: JSONL with mixed valid/invalid lines, `bowerbird replay <file>`, parse stdout for `replayed N events`, parse stderr for `line M: ...` per invalid line; assert N + invalid_count == total line count.
    - `replay_fails_clearly_when_daemon_down`: `bowerbird stop`; `bowerbird replay <file>` → exit non-zero with stderr `cannot reach daemon at <addr>`.
    - `replay_fails_with_401_when_token_wrong`: `BOWERBIRD_TOKEN=wrong bowerbird replay <file>` → exit non-zero with `daemon rejected bearer token`.
    - `bundled_fixture_is_valid_jsonl`: hermetic (no daemon). Parse `fixtures/replay-demo.jsonl` line-by-line as `protocol::Event`, assert ≥10 events, assert ≥2 distinct `(source, session_id)` keys, assert ≥1 event has `EventKind::PreToolUse` (round-trip canary; if the variant is renamed the test breaks loudly).
    - `bundled_fixture_spans_at_least_two_sessions`: hermetic. The AC #4 explicit guardrail. `HashSet<(String, String)>::from_iter(events.iter().map(|e| (e.source.clone(), e.session_id.clone()))).len() >= 2`.
  - [x] 6.3 **Create `tests/cli_export.rs`** as a NEW file. Tests:
    - `export_writes_jsonl_of_session_events_to_stdout`: start daemon → replay a fixture (priming the DB) → `bowerbird export session-alpha`, capture stdout, parse each line as `protocol::Event`, assert count matches the fixture's session-alpha events.
    - `export_writes_to_file_when_output_flag_given`: same but with `-o <tmp/out.jsonl>`; assert file exists and parses cleanly.
    - `export_returns_session_not_found_for_unknown_id`: `bowerbird export bogus-session-id` → exit non-zero with stderr `session bogus-session-id not found`.
    - `export_round_trips_through_replay`: replay fixture → export session-alpha to a file → wipe daemon state → start fresh daemon → replay the exported file → export session-alpha again → diff the two exports (modulo `event_id` + `created_at`, which the daemon reassigns; only `(source, session_id, kind, reaction, payload)` must match). The round-trip is the load-bearing invariant: export's output IS replay's input.
  - [x] 6.4 **Create `tests/cli_replay_fixture.rs`** as a NEW file — doc-drift guardrails (no daemon, fast, hermetic). Per Epic 3 retro agreement A7 ("doc-drift verification as a compiled test, not a verification-block grep"):
    - `bundled_fixture_is_valid_jsonl`: same as 6.2's hermetic test, duplicated here so the doc-drift guardrail does not depend on the CLI E2E suite compiling.
    - `bundled_fixture_spans_at_least_two_sessions`: same as 6.2; the AC #4 explicit invariant.
    - `architecture_md_lists_replay_and_export_as_shipped`: read `_bmad-output/planning-artifacts/architecture.md` (CARGO_MANIFEST_DIR-relative path), assert the §CLI framework block at line ~503 contains both `replay` and `export` in the subcommands list, and that no `arrive in Story 4.1` / `Epic 4 will add: src/commands/{replay,export}` strings survive. The test names what it asserts so a failure message points the dev directly at the stale paragraph.
    - `protocol_changelog_documents_post_replay_endpoint`: read `docs/protocol-changelog.md`, assert the v1.0 → v1.1 section contains an entry mentioning `POST /replay` with a `Resolves: 4.1` marker. (Mirrors Story 3.4's `tests/release_pipeline_docs.rs::protocol_changelog_documents_v0_1_release_pipeline_entry` shape.)
    - `cli_help_lists_replay_and_export`: run `bowerbird --help` as a subprocess, assert the output contains both `replay` and `export` subcommand summaries. (Sanity check that the clap derive surface matches the doc; the doc-drift between code and clap-help is structurally enforced by clap itself, but the test makes the binding explicit.)
  - [x] 6.5 **Test parallelism note.** `tests/cli_replay.rs` and `tests/cli_export.rs` spawn real `bowerbird-daemon` subprocesses and inherit the `--test-threads=1` CI discipline. `tests/cli_replay_fixture.rs` is hermetic and parallel-safe, but running under `--test-threads=1` is fine (no observable performance impact at this test count). Do NOT introduce a `[[test]] harness = false` override — the workspace policy is one test harness per file, serialized at the workspace level via CI's `--test-threads=1`.

- [x] **Task 7 — Hand-rolled HTTP POST helper in `commands::daemon`** (AC: #1, #2)
  - [x] 7.1 **Add `http_post_replay`** to `src/commands/daemon.rs` after `http_get_status` (around line 330). Mirrors `http_get_status`'s shape: `TcpStream::connect_timeout` → `set_read_timeout` + `set_write_timeout` → format the request (POST + headers + body) → `write_all` → `read_to_end` → parse status code → return `ReplayResponse::Ok(body) | ReplayResponse::Status(u16) | ReplayResponse::Unreachable`.
    Request shape:
    ```
    POST /replay HTTP/1.1
    Host: <addr>
    Connection: close
    Authorization: Bearer <token>
    Content-Type: application/x-ndjson
    Content-Length: <body.len()>

    <body bytes>
    ```
    `Content-Length` is mandatory for HTTP/1.1 POST without chunked encoding; axum's body extractor reads exactly that many bytes from the request body. Use the byte length, not the char count.
  - [x] 7.2 **Add `http_get_events`** to `src/commands/daemon.rs` for the export side. Same shape as `http_get_status` but with a path-and-query of `/sessions/<session_id>/events?since=<cursor>`. The handler returns the response body bytes; the caller (`commands::export::run`) parses it as `protocol::EventListResponse`. Same error shape as `StatusResponse`: `Ok(Vec<u8>) | Status(u16) | Unreachable`. Add a 404-specific arm if the test pattern shows we benefit from distinguishing 404 from other status codes; in V1, the body parsing in the caller can read `{"error": "session not found"}` from the 404 body to disambiguate.
  - [x] 7.3 **Unit tests for the new helpers** under the existing `#[cfg(test)] mod tests` block in `src/commands/daemon.rs:362`. Test the request-shape construction (write the formatted request to a `Vec<u8>` and `assert!` it contains the expected headers). No network — pure formatting tests. The integration coverage lives in `tests/cli_replay.rs` and `tests/cli_export.rs` per Task 6.

- [x] **Task 8 — CLI dep-tree invariant verification** (cross-cuts AC: #1, #2)
  - [x] 8.1 **`cargo tree -p bowerbird --depth 8 | grep -cE '^.* (tokio|axum|reqwest|ureq) v'` MUST output 0** after this story lands. The CLI gains `protocol`'s `Event` type re-export (already a dep), `serde_json` (already a dep), but NO new tokio/axum/reqwest/ureq dependency. The hand-rolled HTTP POST helper (Task 7.1) is the structural defense against this regression. Run this grep manually after `cargo build --workspace` and document the result in Dev Agent Record's Completion Notes.
  - [x] 8.2 **Workspace `cargo tree` audit.** Run `cargo tree -p bowerbird-daemon --depth 4 | grep -E '^[^├└]'` to confirm no new daemon-level deps land (we are reusing axum, tokio::mpsc, tracing — all already present). The new file `crates/daemon/src/api/replay.rs` does not bring in any new crate.
  - [x] 8.3 **`cargo bench --no-run`** still compiles. The shim hot-path bench (`shim/benches/hot_path.rs`) is untouched, but a workspace-wide compile failure would catch any accidental cross-crate API drift.

- [x] **Task 9 — Documentation, changelog, deferred-work bookkeeping** (AC: #5, #6, #7)
  - [x] 9.1 **Architecture.md updates** — covered by Task 5 (§CLI framework, §Implementation Order, §Project Structure tree, §FR mapping, plus the new §Infrastructure & Deployment "Replay & Export" paragraph). No additional architecture.md sections need touching — `bowerbird replay` and `bowerbird export` are CLI additions, not architectural shifts.
  - [x] 9.2 **CLAUDE.md** (workspace-root, if present) — not touched. The file is for cross-cutting AI-agent instructions, not feature documentation.
  - [x] 9.3 **README.md** — add ONE line to the Install section's Quickstart describing `bowerbird replay` against the bundled fixture:
    ```sh
    # Try it without setting up Claude Code — the bundled fixture demonstrates
    # the pub/sub path:
    bowerbird replay
    ```
    Single-line addition; the rest of README.md stays untouched. The full `bowerbird export` workflow gets a dedicated example in Story 4.2's reference-tools README contributions.
  - [x] 9.4 **`docs/protocol-changelog.md`** — append the new entry under the existing v1.0 → v1.1 section. Format:
    ```markdown
    - **type: schema** — New authenticated REST endpoint `POST /replay` (Story 4.1). The endpoint accepts a JSONL request body of `protocol::Event` records (post-store form, same shape `GET /sessions/{id}/events` returns); for each line, the daemon constructs a `protocol::EventEnvelope` (dropping the line's `event_id` and `created_at`, which the daemon reassigns at projection-write time via AUTOINCREMENT + `current_unix_millis()`), and pushes the envelope onto the existing `ingest_tx` channel that the Unix-socket ingest path already uses. Replayed events therefore flow through the same `ingest::writer::run` → `projection::session::write` path as live shim ingest — persisted to `events`, projected into `session_projections`, broadcast via `BroadcastHub::publish`. WS subscribers receive `EventFrame` + `StateFrame` pairs in JSONL line order; multi-session JSONL files demonstrate multi-session fan-out by virtue of the existing per-session UPSERT semantics. The response body is `{"replayed_count": N, "parse_errors": [{"line": M, "error": "..."}]}` where parse errors are per-line and the handler continues on failure — replay is a development tool, not a transactional ingest path. Sentinel kinds (`RecordingStarted` / `RecordingEnded`) are rejected at the replay boundary with a parse-error entry; they remain reserved for daemon-lifecycle emission. No rate-limiting on the endpoint (replay is for development; the existing `RequestBodyLimitLayer::new(1 MiB)` cap is the only structural limit, and a future story can add streaming if larger replays become routine). Original inter-event timing is NOT preserved — events are forwarded as fast as the bounded `ingest_tx` channel accepts them, and `created_at` reflects replay wall-clock rather than source wall-clock. The complementary CLI command `bowerbird export <session-id>` reads `/sessions/{id}/events?since=<cursor>` in a cursor-paginated loop and writes JSONL to stdout (or `-o <path>`) in the same wire shape, so `bowerbird export <id> | bowerbird replay /dev/stdin` round-trips a session through the pub/sub path. The bundled demo fixture at `fixtures/replay-demo.jsonl` is embedded into the `bowerbird` CLI binary via `include_bytes!` and used by `bowerbird replay` (no-arg) so new users can exercise the pub/sub path without a live Claude Code session. v1.0 presenters are unaffected — they do not call `/replay` and are not subscribed to a replay-specific broadcast topic. (`Resolves: 4.1`)
    ```
    This is the only changelog entry Story 4.1 lands. It is `type: schema` because the endpoint set expands (additive on the wire surface), per the architecture.md §Protocol stability framing.
  - [x] 9.5 **`_bmad-output/implementation-artifacts/deferred-work.md`** — append a new section at the end of the file:
    ```markdown
    ## Deferred from: Story 4.1 (bowerbird replay and export commands) (2026-05-25)

    1. **Replay rate-limiting / pacing flag** — V1 forwards events as fast as the bounded `ingest_tx` channel accepts them. Future flags: `--rate-limit <events-per-sec>` (token-bucket throttle in the CLI), `--preserve-timing` (`tokio::time::sleep` between events to match original `created_at` deltas). Defer until a presenter author asks for it. [`src/commands/replay.rs`]
    2. **Replay body-size cap** — The `POST /replay` endpoint inherits the global `RequestBodyLimitLayer::new(1 MiB)` cap. A replay file > 1 MiB requires chunking into multiple `POST /replay` calls; a future story could add streaming (`Transfer-Encoding: chunked`) or lift the cap for `/replay` specifically. The CLI currently does NOT chunk — a file > 1 MiB produces a `413` from the daemon and the CLI reports it as a daemon error. [`crates/daemon/src/api/replay.rs`, `src/commands/replay.rs`]
    3. **Source-disambiguation in `bowerbird export`** — Today `export <session-id>` queries `/sessions/{id}/events` which folds across all sources by natural key. The Story 1.7 deferred-work entry on multi-source disambiguation already tracks this; when a second adapter ships, both the REST endpoint and `bowerbird export` will need `--source <claude|codex|...>`. The change is in the export CLI; the export wire shape (per-line `Event`) already carries `source` per event, so no protocol change is needed. [`src/commands/export.rs`, `crates/daemon/src/api/sessions.rs::detail`]
    4. **`bowerbird replay --dry-run`** — Parse the JSONL, validate, report per-line counts, but do not POST to the daemon. Useful for fixture authors. Single flag; deferred for V1 because the fixture-validation guardrails in `tests/cli_replay_fixture.rs` cover the immediate need. [`src/commands/replay.rs`]
    5. **`bowerbird replay --filter-session=<id>`** — Replay only events whose `session_id` matches the filter, dropping others. Useful when a multi-session export needs to be replayed as a single-session smoke. Deferred — a user can pre-filter the JSONL with `jq` today. [`src/commands/replay.rs`]
    6. **Bundled fixture refresh tooling** — `fixtures/replay-demo.jsonl` is hand-authored. A future tooling story could add `cargo xtask refresh-replay-fixture` that exports an interesting session from a developer's `~/.bowerbird/bower.db` and writes it to the fixture path, with `git diff` showing the change for review. Deferred — hand-authored fixtures are stable enough for V1's modest event-shape coverage. [`fixtures/replay-demo.jsonl`]
    ```
  - [x] 9.6 **`docs/cookbook/`** — out of scope for Story 4.1. Story 4.3 owns the cookbook authorship; the replay/export commands will get cookbook entries there (likely "Replay a session through your tool" and "Export a session for sharing"). The current story documents the wire surface in protocol-changelog.md; the user-facing recipe lives one story later.
  - [x] 9.7 **`docs/no-list.md`** — not authored yet (Story 4.3 deliverable). No edit needed by this story.

- [x] **Task 10 — Verification gates and end-of-story sweep** (cross-cuts ALL ACs)
  - [x] 10.1 **Mandatory `cargo` verification before marking story `review`:**
    ```sh
    cargo fmt --all -- --check                                # clean
    cargo clippy --workspace --all-targets -- -D warnings     # 0 warnings
    cargo test --workspace -- --test-threads=1 \
      --skip state_plus_event_atomicity_under_sigkill_during_load   # ALL pass (per Epic 3 retro Discovery #2)
    cargo build --release --workspace --locked                # reproducible release build
    cargo build -p bowerbird-shim --profile release-shim --locked    # shim release-shim profile still compiles
    ```
  - [x] 10.2 **Per-AC verification commands.** Each AC has at least one explicit assertion:
    - **AC #1 (replay forwards through broadcast)**: `cargo test --test contract_daemon story_4_1_replay::replay_forwards_events_through_broadcast_path` passes.
    - **AC #2 (export to JSONL)**: `cargo test --test cli_export export_writes_jsonl_of_session_events_to_stdout export_round_trips_through_replay` passes.
    - **AC #3 (bundled fixture)**: `cargo test --test cli_replay replay_with_no_argument_uses_bundled_fixture` passes; `cargo test --test cli_replay_fixture bundled_fixture_is_valid_jsonl` passes.
    - **AC #4 (multi-session fan-out)**: `cargo test --test contract_daemon story_4_1_replay::replay_emits_state_frames_for_each_session` passes; `cargo test --test cli_replay_fixture bundled_fixture_spans_at_least_two_sessions` passes.
    - **AC #5 (no timing preservation)**: `cargo test --test contract_daemon story_4_1_replay::replay_dropped_event_id_and_created_at_are_reassigned` passes (asserts `created_at` reflects replay wall-clock, not the JSONL value).
    - **AC #6 (architecture.md updates)**: `cargo test --test cli_replay_fixture architecture_md_lists_replay_and_export_as_shipped` passes.
    - **AC #7 (protocol-changelog)**: `cargo test --test cli_replay_fixture protocol_changelog_documents_post_replay_endpoint` passes.
  - [x] 10.3 **Doc-drift verification grep sweep** (carried forward from Epic 3 retro discipline):
    ```sh
    grep -rn 'wait for Story 4.1\|Story 4.1 will\|arrive in Story 4.1\|Epic 4 will add' \
      _bmad-output/ docs/ src/ crates/   # MUST return 0 hits
    grep -nE 'replay|export' _bmad-output/planning-artifacts/architecture.md | \
      grep -vE 'deferred|Resolved by'   # All references point at shipped surfaces, not future stories
    grep -nE 'POST /replay|/replay' docs/protocol-changelog.md   # changelog entry present
    grep -nE 'license' Cargo.toml src/*/Cargo.toml crates/*/Cargo.toml   # license metadata still on every published crate
    ```
  - [x] 10.4 **CLI binary tokio-freeness regression-guard:**
    ```sh
    cargo tree -p bowerbird --depth 8 | grep -cE '^.* (tokio|axum|reqwest|ureq) v'
    # MUST output 0 — no new HTTP-client or async-runtime dependencies.
    ```
    Document the result in the Dev Agent Record's Completion Notes (per Epic 3 retro file-list-discipline finding).
  - [x] 10.5 **File List discipline.** Per Epic 3 retro agreement A9 ("the senior-review File-vs-git audit is load-bearing, not optional"): after every implementation pass and before marking `review`, run `git status --porcelain` and cross-reference against the Dev Agent Record's File List. Any divergence is a HIGH finding that blocks `review`. Include rustfmt reflows (`cargo fmt --all`) in the File List even though they look incidental — Story 3.3 and 3.4 both surfaced these as silent omissions.
  - [x] 10.6 **Update `_bmad-output/implementation-artifacts/sprint-status.yaml`** through the lifecycle: `backlog` → `ready-for-dev` (this story-creation pass) → `in-progress` (when dev starts) → `review` (when verification passes) → `done` (when code-review approves). The story-creation workflow handles the first transition; subsequent transitions are dev-agent and review-agent responsibilities. Bump `last_updated` on every transition.
  - [x] 10.7 **Sanity round-trip smoke** (manual, before declaring `review`):
    ```sh
    bowerbird start
    bowerbird replay                         # uses bundled fixture; expect "replayed 12 events" (or whatever the fixture has)
    bowerbird export session-alpha > /tmp/alpha.jsonl
    wc -l /tmp/alpha.jsonl                   # expect non-zero
    bowerbird replay /tmp/alpha.jsonl        # expect "replayed N events from /tmp/alpha.jsonl"
    bowerbird stop
    ```
    This is the "could a presenter author run this end-to-end on a fresh install?" test. If any step fails or produces unexpected output, the dev agent has not finished the story.

## Dev Notes

### What changes vs. what stays

**Files this story creates (NEW):**

| Path | Purpose |
|---|---|
| `crates/daemon/src/api/replay.rs` | Daemon-side `POST /replay` endpoint handler; parses JSONL of `Event` → drops `event_id` + `created_at` → constructs `EventEnvelope` → `ingest_tx.try_send` per line; returns `{replayed_count, parse_errors}`. |
| `src/commands/replay.rs` | CLI `bowerbird replay [<file>]`; bundled-fixture `include_bytes!` embed; hand-rolled HTTP POST via `commands::daemon::http_post_replay`. |
| `src/commands/export.rs` | CLI `bowerbird export <session-id> [-o <path>]`; cursor-paginated `GET /sessions/{id}/events?since=<cursor>` loop; JSONL emission to stdout or file. |
| `fixtures/replay-demo.jsonl` | Workspace-root bundled fixture; ~12-15 events across 2 sessions; consumed by `include_bytes!` in the CLI AND by Story 4.2's reference examples at runtime. |
| `tests/cli_replay.rs` | E2E suite: replay-with-explicit-file, replay-bundled-fixture, replay-continues-on-error, replay-fails-when-daemon-down, replay-401, fixture validation. |
| `tests/cli_export.rs` | E2E suite: export-to-stdout, export-to-file, export-not-found, export-round-trips-through-replay. |
| `tests/cli_replay_fixture.rs` | Hermetic doc-drift guardrails: fixture JSONL validity, fixture two-session invariant, architecture.md surface assertions, protocol-changelog entry assertion, CLI help-text assertion. |

**Files this story modifies (UPDATE):**

| Path | What changes | What must be preserved |
|---|---|---|
| `src/main.rs` | Add `Replay(commands::replay::ReplayArgs)` and `Export(commands::export::ExportArgs)` variants to the `Command` enum (alphabetical placement); add the two `match` arms in `main()` (each with `.context("bowerbird replay")` / `.context("bowerbird export")`); add `replay::ReplayArgs` and `export::ExportArgs` to the existing clap derive surface. | The existing six subcommands (`Auth`, `Install`, `Start`, `Status`, `Stop`, `Uninstall`); the `Cli` struct's `name`, `version`, `about`; the `anyhow::Context`-permitted-here-only doc comment. |
| `src/commands/mod.rs` | Add `pub mod export;` and `pub mod replay;` (alphabetical). | All existing `pub mod` declarations; the shared helpers `resolve_claude_settings`, `resolve_bowerbird_dir`, `home_dir`, `resolve_daemon_bin`, `daemon_is_up`. |
| `src/commands/daemon.rs` | Add two new helpers `http_post_replay(addr, bearer, body, per_attempt) -> ReplayResponse` and `http_get_events(addr, bearer, session_id, since, per_attempt) -> Result<Vec<u8>, ExportError>`; add the new response enums next to the existing `HealthzOutcome` and `StatusResponse`; add unit tests for the request-shape construction. | All existing functions (`start_daemon_detached`, `stop_daemon_via_pid_file`, `read_pid`, `pid_alive`, `spawn_detached`, `server_json_path`, `read_server_info`, `wait_for_server_json`, `http_get_healthz`, `http_get_status`, `parse_status_code`, `body_after_headers`, `find_subslice`); the existing `#[cfg(test)] mod tests` block (just add new tests, don't replace). |
| `crates/daemon/src/state.rs` | Add `pub ingest_tx: tokio::sync::mpsc::Sender<protocol::EventEnvelope>` to `AppState`. | All existing `AppState` fields (`db`, `migrations_complete`, `shutdown_requested`, `ws_close_requested`, `bearer`, `started_at_ms`, `broadcaster`, `ws_semaphore`, `ws_config`); the `WsConfig` struct; `wait_for_ws_connection_drain`. |
| `crates/daemon/src/main.rs` | Around line 195 where `(ingest_tx, ingest_rx) = mpsc::channel(...)` is created, clone `ingest_tx` into `AppState` (line 229 where `AppState { ... }` is constructed). | Every other line of `main.rs`; the singleton lock acquisition; the `init_pools` + `run_migrations` sequence; the recording-session lifecycle (`write_recording_started` / `write_recording_ended`); the ingest listener and writer task spawns; the WS semaphore; the axum serve + graceful shutdown sequence; the WAL checkpoint. |
| `crates/daemon/src/api/mod.rs` | Add `pub mod replay;` (alphabetical, between `pub mod health;` and `pub mod sessions;` — `r` falls between `h` and `s` alphabetically); add `.route("/replay", post(replay::run))` to the `authenticated` Router (alphabetical with the existing `/sessions/*` and `/status` routes); add `use axum::routing::post;` to the import block at line 14. | The unauthenticated `/healthz` and `/readyz` routes; the WS `/ws` route; the middleware stack (`SetRequestIdLayer`, `PropagateRequestIdLayer`, `RedactedSpan`, `timeout_middleware`, `RequestBodyLimitLayer`); `require_bearer` route-layered on the authenticated subset. |
| `README.md` | Add ONE Quickstart-section line demonstrating no-arg `bowerbird replay`. | All other README sections; the Install / Architecture / Protocol / Contributing pointers. |
| `_bmad-output/planning-artifacts/architecture.md` | §CLI framework (line 503): alphabetize `replay` and `export` into the subcommand list, drop the "arrive in Story 4.1" sentence. §Implementation Order (line 531): drop the trailing "`replay`/`export` arrive in Epic 4" sentence. §Project Structure tree (line 875): drop the "# Epic 4 will add" comment, add `export.rs` and `replay.rs` lines in alphabetical position. §FR mapping (line 932): change "Epic 4 — `src/commands/{replay,export}.rs`, `examples/*/` (deferred)" to "`src/commands/{replay,export}.rs` (Story 4.1); `examples/*/` (Story 4.2 deferred); `docs/cookbook/` (Story 4.3 deferred)". Add new §Infrastructure & Deployment "Replay & Export" paragraph around line 510. | Every other section of architecture.md; the existing §WebSocket subsystem section (Story 3.4 / Epic 2 retro AI-2); the §Authentication & Security section (Story 3.3); the §Distribution paragraph (Story 3.4); the full project-structure tree apart from the surgical line additions. |
| `docs/protocol-changelog.md` | Append the new `type: schema` entry under v1.0 → v1.1 documenting `POST /replay`. | All existing entries (the changelog grows; never edited in place). |
| `_bmad-output/implementation-artifacts/deferred-work.md` | Append a new `## Deferred from: Story 4.1 ... ` section with six entries (Task 9.5). | All existing sections; the strike-through resolutions for prior stories. |
| `_bmad-output/implementation-artifacts/sprint-status.yaml` | Transition `4-1-bowerbird-replay-and-export-commands: backlog → ready-for-dev → in-progress → review → done`. Set `epic-4: in-progress` on the `ready-for-dev` transition (Story 4.1 is the first story in Epic 4 — auto-flip per the workflow). Bump `last_updated`. | All other story statuses; the YAML structure including STATUS DEFINITIONS comments. |

**Files this story does NOT touch:**

- `crates/protocol/**` — no wire-type changes. `Event`, `EventEnvelope`, `EventKind`, `Reaction`, `EventListResponse` all stay as-is. The new `/replay` endpoint produces and consumes existing types.
- `crates/shim/**` — the shim is unchanged. Replay does NOT route through the ingest socket; it uses a parallel REST endpoint.
- `crates/adapter-claude/**` — the adapter normalizes Claude Code hook payloads; replay events are already normalized (the daemon strips `event_id` + `created_at` but does NOT re-normalize the `payload` string). No new adapter surface needed.
- `crates/daemon/src/ingest/**` — the ingest socket and writer/handler stay unchanged. Replay reuses the writer (via `ingest_tx`) but does not modify it.
- `crates/daemon/src/projection/**` — `session::write` is the sole owner of the write transaction; replay invokes it via the same path as live ingest. No projection-layer changes.
- `crates/daemon/src/db/**` — no schema migration, no new SQL strings. Replay's persistence is the existing `INSERT_EVENT` + `UPSERT_SESSION_PROJECTION`.
- `crates/daemon/src/broadcast/**` — fan-out unchanged. The broadcaster sees replayed envelopes as identical to live ones (post-projection-commit `BroadcastEnvelope::Event` + `BroadcastEnvelope::State`).
- `.github/workflows/ci.yml` — no CI change. The existing `--test-threads=1` discipline (Story 3.4 AC #6) covers the new test files. The protocol-changelog CI gate (architecture.md §CI requirements) is NOT triggered by Story 4.1 because no `crates/protocol/src/*.rs` file is edited.
- `.github/workflows/release.yml` — no release-pipeline change. The bundled fixture lives at workspace root and is embedded at compile time; no tarball-layout change needed.
- `LICENSE-MIT`, `LICENSE-APACHE`, `LICENSE` — unchanged.
- `Cargo.toml` (workspace root or per-crate) — no new dep additions. The CLI continues to use `serde`, `serde_json`, `protocol`, `clap`, `anyhow`, `libc`, `secrecy`, `keyring`, `toml` (plus `adapter-claude`). No `reqwest`, no `ureq`, no `tokio` in the CLI binary.

### Existing behavior to read carefully before changing

- **`crates/daemon/src/main.rs:195-219`** is the ingest channel setup site. Today `(ingest_tx, ingest_rx)` is constructed and `ingest_tx` is moved into the listener task spawn (`ingest::listener::run_bound(..., ingest_tx, ...)`). Story 4.1 needs to `clone` `ingest_tx` BEFORE the spawn (the channel sender is `Clone`) so a clone can land in `AppState` for the new `/replay` endpoint. The listener's clone and the AppState clone independently push to the same `ingest_rx`; the writer task drains them serially via the existing `tokio::select!` loop. Capacity (`config.ingest_channel_capacity`, default 1024) is unchanged; a replay of a 10k-line file will fan out within capacity because the writer drains continuously. [Source: `crates/daemon/src/main.rs:195-219`, `crates/daemon/src/state.rs::AppState`]

- **`crates/daemon/src/ingest/writer.rs:12-44`** is the writer task. It accepts `EventEnvelope`s on `rx` and calls `projection::session::write(&writer_pool, &broadcaster, envelope)`. Story 4.1's `/replay` endpoint produces `EventEnvelope`s from the parsed `Event`s — same wire shape, same writer, same broadcast. The shutdown-drain branch at lines 31-41 also covers replay (replay events queued before SIGTERM are drained alongside live-ingest events). [Source: `crates/daemon/src/ingest/writer.rs`]

- **`crates/daemon/src/projection/session.rs:43-65`** is the projection write entry point. It runtime-guards sentinel kinds (`RecordingStarted` / `RecordingEnded`) at lines 56-65 — Story 4.1's `/replay` endpoint MUST reject these at the parse boundary (returning a per-line parse error) so the runtime guard never fires for legitimate replay input; the guard remains as defense-in-depth. The `tracing::instrument(skip_all, fields(source, session_id))` span attribution at line 43 is inherited by replayed events. [Source: `crates/daemon/src/projection/session.rs:1-100`]

- **`crates/daemon/src/api/mod.rs:96-145`** is the router composition. The `authenticated` sub-router at lines 101-110 is where `/replay` lands. The middleware stack (timeout 30s, body limit 1 MiB) wraps `/replay` automatically; per AC #7 / Task 1.3, the 1 MiB cap is the only structural limit on replay body size. Importantly: the `auth::require_bearer` middleware is layered via `route_layer` on the `authenticated` sub-router (line 107-110), so it applies to `/replay` automatically. No new auth wiring is needed. [Source: `crates/daemon/src/api/mod.rs:101-145`]

- **`crates/daemon/src/api/events.rs`** is the existing handler for `GET /sessions/{id}/events?since=<cursor>` — the data source for `bowerbird export`. Per the Story 1.7 deferred-work entry, the endpoint returns the full history slice in V1 (no page-size limit). `EventListResponse { events, cursor, oldest_available_event_id }` is returned; the CLI's export loop uses `cursor` to decide whether to fetch again. Today `cursor` is `Some(events.last().event_id)` when non-empty and `None` when empty; the loop terminates correctly on the empty case AND on a future paginated implementation that returns `None` only at the actual end. [Source: `crates/daemon/src/api/events.rs:30-133`]

- **`src/commands/status.rs:35-117`** is the reference shape for "CLI hand-rolled HTTP probe with graceful degradation across missing-server.json, missing-token, 401, transport-failure." Story 4.1's `replay` and `export` follow the same pattern: resolve daemon address from `server.json`, resolve token from the env→keychain→config.toml chain, hand-roll the HTTP request via `TcpStream`, surface clear stderr messages for each failure mode. The architectural commitment is: the CLI binary stays tokio-free, axum-free, reqwest-free; verified by `cargo tree -p bowerbird --depth 8 | grep -cE '^.* (tokio|axum) v' == 0`. [Source: `src/commands/status.rs`, `src/commands/daemon.rs:236-329`]

- **`src/commands/daemon.rs:282-329`** is `http_get_status` — the hand-rolled HTTP GET pattern. Replay's `http_post_replay` (Task 7.1) is a structural twin with three differences: (a) verb is `POST`, (b) `Content-Type: application/x-ndjson` + `Content-Length: <body.len()>` headers, (c) body bytes follow the headers separated by `\r\n\r\n`. The 200-response-body extraction (`body_after_headers` at line 342) is reused as-is. [Source: `src/commands/daemon.rs:282-360`]

- **`crates/daemon/src/state.rs::AppState`** is shared via `axum::extract::State<AppState>`. Today it carries `db`, `migrations_complete`, `shutdown_requested`, `ws_close_requested`, `bearer`, `started_at_ms`, `broadcaster`, `ws_semaphore`, `ws_config`. Story 4.1 adds `ingest_tx: tokio::sync::mpsc::Sender<protocol::EventEnvelope>` as an additive field. Because `AppState` is `Clone` (the daemon clones it into route handlers via `with_state`), the mpsc Sender's `Clone` impl makes this free — every handler invocation gets its own sender clone, all pointing at the same channel. [Source: `crates/daemon/src/state.rs`]

- **`tests/cli_lifecycle.rs:1-120`** is the canonical CLI E2E test shape — `assert_cmd::Command::cargo_bin("bowerbird")`, env isolation (`HOME` + `BOWERBIRD_DATA_DIR` + `BOWERBIRD_DAEMON_BIN` + `BOWERBIRD_INGEST_SOCK`), token pre-set (`BOWERBIRD_TOKEN = LIFECYCLE_TEST_TOKEN`, with `BOWERBIRD_KEYRING_BACKEND=disable` for defense in depth), TempDir-scoped data, `wait_for_daemon_up` polling, `force_stop` cleanup. Stories 4.1's `tests/cli_replay.rs` and `tests/cli_export.rs` mirror this shape. [Source: `tests/cli_lifecycle.rs`]

- **`fixtures/`** is the architecture-defined workspace-root location for shared fixtures (`architecture.md:760-765`). Today the directory does NOT exist on disk — `ls fixtures/ → No such file or directory`. The architecture document lists `fixtures/hook_pre_tool_use.json` and `fixtures/event_log_sample.db` as planned contents; Story 4.1 creates the directory and authors the first inhabitant `replay-demo.jsonl`. Future stories (4.2's reference examples, possibly 4.4's contract suite) will add siblings. The directory's `mkdir -p fixtures/` happens implicitly when `git add fixtures/replay-demo.jsonl` runs. [Source: `architecture.md:760-765`; project's actual filesystem]

- **`docs/protocol-changelog.md`** structure: one top-level `## v1.0 → v1.1` section with a list of `- **type: <kind>** — <body>. (Resolves: X.Y)` entries in story-completion chronological order. Type kinds in use: `behavioral` (most common; functional addition without wire-format change) and `schema` (used by Story 4.1; wire-surface addition). The asymmetric forward-compat policy (`crates/protocol/src/` outbound permissive, inbound strict) is the protection — adding a `POST /replay` endpoint is `schema` because v1.x's "set of available endpoints" is part of the wire surface. v1.0 presenters that never call `/replay` are unaffected; that is the additive contract. [Source: `docs/protocol-changelog.md:1-23`]

- **`_bmad-output/implementation-artifacts/deferred-work.md`** uses a per-section pattern: `## Deferred from: <Story-X.Y> (<description>) (<date>)` headers, then numbered or hyphen-bulleted entries with bracketed file-path references. Resolved entries get struck through inline with `~~text~~ **Resolved by Story X.Y...**` followed by a backlink. Story 4.1 ADDS a new section (Task 9.5); it does NOT strike through any prior entries (Story 4.1 does not close any prior deferred work — `bowerbird replay` and `bowerbird export` are net-new features). [Source: `_bmad-output/implementation-artifacts/deferred-work.md`]

### Replay endpoint design (the load-bearing piece)

The `POST /replay` endpoint is the single non-trivial daemon addition. Three structural decisions shape it:

1. **Wire shape choice: `Event` per line, not `EventEnvelope` per line.** `protocol::Event` (with `event_id` + `created_at`) is the shape `GET /sessions/{id}/events` returns; reusing it for the replay input means `bowerbird export <id> | bowerbird replay /dev/stdin` round-trips through the same wire format with no transformation. The daemon discards `event_id` + `created_at` at the parse boundary (Task 1.4) because the writer reassigns both. An alternative — accepting raw `EventEnvelope` — would require the user (or the export CLI) to strip the assigned fields before replay, adding asymmetric work for no benefit. The "input shape == output shape" property is the design's key affordance.

2. **Channel reuse, not direct projection-write.** The endpoint pushes to `ingest_tx` rather than directly calling `projection::session::write`. Three reasons: (a) the existing writer task already serializes writes through the single-writer pool, so contention with live ingest is naturally handled; (b) the existing shutdown-drain semantics (writer task drains queued envelopes after `shutdown_requested.cancel()`) automatically apply to replay; (c) the existing tracing instrumentation and error handling stay in one place. The alternative — a separate `replay::write_directly` path — would duplicate ~40 lines of pool-checkout, transaction-construction, broadcast-publish logic. Story 4.1's design says: replay IS ingest, just on a different inbound surface.

3. **Per-line continue-on-error, not transactional batch.** A replay file may have stale or malformed lines; failing the whole replay on one bad line is hostile to the development-tool use case. The endpoint's `parse_errors` array makes the failures visible without aborting the success path. The transactional contract on a single line (its `INSERT INTO events` + `UPSERT INTO session_projections` inside one SQLite transaction) is preserved by reusing `projection::session::write`; we are explicit that the *replay request* is best-effort across lines, not transactional. The response body structure (`{replayed_count, parse_errors}`) makes this contract legible.

The endpoint is intentionally minimal: no streaming, no chunked encoding, no rate-limiting, no `?dry-run` query parameter, no `?max-events=N` cap. Each of these is a real future-V1.1 question (the deferred-work entries in Task 9.5 enumerate them). For V1, the 1 MiB body cap + the bounded `ingest_tx` channel capacity + the per-line continue-on-error policy are sufficient.

### Bundled-fixture shape (the demo-without-Claude-Code piece)

`fixtures/replay-demo.jsonl` is the single artifact a new user touches via `bowerbird replay` (no-arg). The fixture's job is to demonstrate (a) per-session event flow, (b) multi-session fan-out via two interleaved sessions, (c) a realistic mix of `EventKind` variants (`PreToolUse`, `PostToolUse`, `Notification`, `Stop`). A reasonable shape:

```
{"event_id":1,"source":"claude","session_id":"session-alpha","kind":"PreToolUse","reaction":"Continue","payload":"{\"hook_event_name\":\"PreToolUse\",\"session_id\":\"session-alpha\",\"tool_name\":\"Read\",\"tool_input\":{\"file_path\":\"/tmp/example.txt\"}}","created_at":1700000000000}
{"event_id":2,"source":"claude","session_id":"session-beta","kind":"PreToolUse","reaction":"Continue","payload":"{\"hook_event_name\":\"PreToolUse\",\"session_id\":\"session-beta\",\"tool_name\":\"Bash\",\"tool_input\":{\"command\":\"echo hello\"}}","created_at":1700000001000}
{"event_id":3,"source":"claude","session_id":"session-alpha","kind":"PostToolUse","reaction":"Continue","payload":"{\"hook_event_name\":\"PostToolUse\",\"session_id\":\"session-alpha\",\"tool_name\":\"Read\",\"tool_response\":{\"content\":\"file contents...\"}}","created_at":1700000002000}
... etc ...
```

Field values to verify before hand-authoring the file:
- `kind` must be one of the serialized `EventKind` strings: `"PreToolUse"`, `"PostToolUse"`, `"Stop"`, `"Notification"`. (`RecordingStarted` / `RecordingEnded` are sentinels and would be rejected.)
- `reaction` is `Option<Reaction>`; values: `null` or `"Continue"` / `"Pause"` / `"Block"` / `"Unknown"` / `"Vendor(N)"`. The reaction serialization uses the custom impl in `crates/protocol/src/reaction.rs`.
- `payload` is `String` (raw JSON as a string-escaped value, because `protocol::Event.payload: String` is the verbatim-raw policy from architecture.md:404).
- `created_at` is `i64` Unix milliseconds. Values are placeholder; the daemon reassigns at write time.

The fixture should NOT include sentinel events. The fixture should include at least one `Stop` event per session so the projection's "session ended" state is exercised by replay.

### LLM optimization (the dev agent's contract)

The dev agent that implements this story has the following clear contract:

- **Read `crates/daemon/src/main.rs:195-219`** before touching the ingest channel setup. Clone `ingest_tx` BEFORE the listener spawn.
- **Read `crates/daemon/src/projection/session.rs:43-65`** before designing the `/replay` parse-error policy. Sentinels are runtime-guarded already; the replay endpoint rejects them at the parse boundary for clearer error messages.
- **Read `src/commands/status.rs:35-117`** before writing `src/commands/replay.rs::run`. The status command is the structural reference for "CLI hand-rolled HTTP probe with graceful degradation."
- **Read `src/commands/daemon.rs:282-329`** before writing the new helpers `http_post_replay` and `http_get_events`. They are structural twins of `http_get_status`.
- **Read `tests/cli_lifecycle.rs:1-120`** before writing `tests/cli_replay.rs` and `tests/cli_export.rs`. Reuse `bowerbird_cmd_in(tmp)`, `wait_for_daemon_up`, `force_stop`.

Anti-patterns to avoid (each one would block code-review):
- Adding `reqwest`, `ureq`, `hyper`, or `tokio` to the CLI's `[dependencies]` block.
- Implementing replay by writing to the ingest Unix socket (the protocol there is hook-shaped, NOT EventEnvelope-shaped; the conversion would be lossy).
- Implementing replay as a CLI-only command that bypasses the daemon (it would not exercise the pub/sub path, defeating the AC #1 promise).
- Adding a new field to `protocol::Event` or `protocol::EventEnvelope` to mark "this is a replay" — the broadcast frames must be indistinguishable from live ingest per AC #1.
- Persisting replayed events to a separate table or with a marker column — the daemon's `events` table is the source of truth; replay events live there alongside live events.
- Preserving original `created_at` in the daemon row — AC #5 explicitly says the daemon assigns wall-clock at write time.
- Skipping the doc-drift guardrails in `tests/cli_replay_fixture.rs` — Epic 3 retro agreement A7 makes these mandatory for any cross-file invariant.

### References

- [Source: docs/bmad/planning-artifacts/epics.md#Story-4.1-bowerbird-replay-and-export-commands] — the ACs this story implements verbatim.
- [Source: docs/bmad/planning-artifacts/architecture.md#§API-&-Communication-Patterns] — the REST + WS surface this story extends.
- [Source: docs/bmad/planning-artifacts/architecture.md#§Project-Structure-&-Boundaries:760-765] — the `fixtures/` directory contract; Story 4.1 creates the first inhabitant.
- [Source: docs/bmad/planning-artifacts/architecture.md#§Infrastructure-&-Deployment:495-512] — the shipped CLI surface this story extends.
- [Source: docs/bmad/planning-artifacts/architecture.md#§Authentication-&-Security:432-446] — the bearer-auth policy the new endpoint inherits.
- [Source: crates/protocol/src/event.rs] — `Event`, `EventEnvelope`, `EventId`, `EventKind` definitions.
- [Source: crates/protocol/src/rest.rs#EventListResponse] — the response shape `bowerbird export` consumes.
- [Source: crates/daemon/src/main.rs:195-219] — the `ingest_tx` channel setup site.
- [Source: crates/daemon/src/ingest/writer.rs] — the writer task that drains `ingest_rx` and calls `projection::session::write`.
- [Source: crates/daemon/src/projection/session.rs] — the sole owner of the SQLite write transaction + broadcast publish.
- [Source: crates/daemon/src/api/mod.rs:96-145] — the router composition; the new `/replay` route plugs into the `authenticated` sub-router.
- [Source: crates/daemon/src/api/events.rs] — the `GET /sessions/{id}/events?since=<cursor>` handler; `bowerbird export` is its first systematic consumer.
- [Source: src/commands/status.rs] — the structural reference for CLI HTTP probes with graceful degradation.
- [Source: src/commands/daemon.rs:282-329] — the `http_get_status` helper; `http_post_replay` and `http_get_events` are structural twins.
- [Source: src/commands/auth.rs#resolve_token_for_cli] — the env→keychain→config.toml token resolver chain (Story 3.3).
- [Source: tests/cli_lifecycle.rs] — the CLI E2E test pattern (assert_cmd, TempDir, BOWERBIRD_DATA_DIR isolation, --test-threads=1).
- [Source: tests/release_pipeline_docs.rs] — the doc-drift guardrail pattern from Story 3.4; `tests/cli_replay_fixture.rs` mirrors its shape.
- [Source: _bmad-output/implementation-artifacts/epic-3-retro-2026-05-25.md#Team-agreements] — agreements A7 (doc-drift as compiled test), A8 (AC-vs-shipped reconciliation in module doc comments), A9 (File-vs-git audit at review time).
- [Source: _bmad-output/implementation-artifacts/deferred-work.md] — the structural pattern for "Deferred from: Story X.Y" sections.
- [Source: docs/protocol-changelog.md] — the entry format and chronological order convention.

### Project Structure Notes

- The CLI binary stays at workspace-root `bowerbird` package (per architecture.md:860). Story 4.1's new files land at `src/commands/replay.rs` and `src/commands/export.rs`, alphabetical with the existing six command files.
- The daemon-side endpoint lands at `crates/daemon/src/api/replay.rs`, alphabetical with the existing seven module files in `crates/daemon/src/api/`.
- The bundled fixture lands at workspace-root `fixtures/replay-demo.jsonl`, the architecture-canonical location for shared fixtures (used by both compile-time embed in the CLI and runtime read by Story 4.2's examples).
- Tests land at workspace-root `tests/cli_replay.rs`, `tests/cli_export.rs`, `tests/cli_replay_fixture.rs`, alongside the existing `tests/cli_install.rs`, `tests/cli_lifecycle.rs`, `tests/cli_auth.rs`, `tests/release_pipeline_docs.rs`. Daemon-side contract tests land in `crates/daemon/tests/contract_daemon.rs` under a new `story_4_1_replay` module, mirroring the Epic 3 `story_3_1_singleton` / `story_3_2_lifecycle` / `story_3_3_auth` modules.
- No new examples, no new crates, no new workspace members. The directory structure remains exactly as the Epic 3 retrospective recorded it; this story populates planned but-empty surfaces (CLI subcommands, daemon API endpoint, fixtures directory) without restructuring.

## Dev Agent Record

### Agent Model Used

claude-opus-4-7 (1M context) via Claude Code's BMAD dev-story workflow.

### Debug Log References

- `cargo test -p bowerbird-daemon --test contract_daemon story_4_1_replay -- --test-threads=1` — 8 passed (original 6 + QA pass added `replay_skips_blank_and_comment_lines`, `replay_with_only_comments_replays_zero_events`)
- `cargo test --test cli_replay -- --test-threads=1` — 5 passed
- `cargo test --test cli_export -- --test-threads=1` — 7 passed (original 4 + QA pass added `export_fails_clearly_when_daemon_down`, `export_fails_with_401_when_token_wrong`, `export_overwrites_existing_output_file`)
- `cargo test --test cli_replay_fixture -- --test-threads=1` — 5 passed
- `cargo test --workspace -- --test-threads=1 --skip state_plus_event_atomicity_under_sigkill_during_load` — 355 passed, 1 filtered out
- `cargo tree -p bowerbird --depth 8 | grep -cE '^.* (tokio|axum|reqwest|ureq) v'` — 0
- `cargo fmt --all -- --check` — clean
- `cargo clippy --workspace --all-targets -- -D warnings` — 0 warnings
- `cargo build --release --workspace --locked` — success
- `cargo build -p bowerbird-shim --profile release-shim --locked` — success
- `cargo bench --no-run` — success
- Sanity round-trip smoke (Task 10.7) — start → replay (12 events / 2 sessions from bundled fixture) → export session-alpha (6 events) → replay /tmp/alpha.jsonl (6 events) → stop. All steps emit the AC-specified output.

### Completion Notes List

- **AC #1 (replay forwards through broadcast)**: `POST /replay` accepts JSONL of `protocol::Event`, strips `event_id`+`created_at`, constructs `EventEnvelope` per line, and pushes via `state.ingest_tx.try_send`. Contract test `replay_forwards_events_through_broadcast_path` verifies the broadcast emits `Event` + `State` pairs in JSONL line order with newly assigned `event_id` and `created_at`.
- **AC #2 (export to JSONL)**: `bowerbird export <session-id>` resolves daemon address + bearer token, pre-checks `GET /sessions/{id}` (404 → "session not found"), then loops `GET /sessions/{id}/events?since=<cursor>` writing each `Event` as a JSONL line. Stdout for data, stderr for summary keeps stdout pipe-safe. Round-trip (`bowerbird export <id> | bowerbird replay /dev/stdin`) verified by `export_round_trips_through_replay`.
- **AC #3 (bundled fixture)**: `fixtures/replay-demo.jsonl` (12 events / 2 sessions) embedded via `include_bytes!`. No-arg `bowerbird replay` prints `using bundled fixture (12 events across 2 sessions)` preamble then `replayed 12 events from bundled-fixture`.
- **AC #4 (multi-session fan-out)**: Fixture interleaves `session-alpha` and `session-beta` events. Contract test `replay_emits_state_frames_for_each_session` verifies State frames cover both sessions; hermetic guardrail `bundled_fixture_spans_at_least_two_sessions` asserts the invariant statically.
- **AC #5 (no timing preservation)**: The endpoint pushes envelopes onto `ingest_tx` via `try_send` with no `tokio::time::sleep`; the daemon's writer reassigns `created_at` via `current_unix_millis()` at projection-write time. Contract test `replay_dropped_event_id_and_created_at_are_reassigned` POSTs `{"event_id":999999, "created_at":1, ...}` and asserts the persisted row carries a fresh small AUTOINCREMENT id and a wall-clock `created_at` within ±5s of test time.
- **AC #6 (architecture.md updates)**: §CLI framework now alphabetizes `auth token, export, install, replay, start, status, stop, uninstall` and drops the "arrive in Story 4.1 (Epic 4)" deferral. §Implementation Order trailing sentence updated to "`replay`/`export` ship in Story 4.1." §Project Structure tree drops the `# Epic 4 will add` comment and lists `export.rs` + `replay.rs` inline. §FR mapping table now reads "Story 4.1; examples/*/ (Story 4.2 deferred); docs/cookbook/ (Story 4.3 deferred)". New §Infrastructure & Deployment "Replay & Export" paragraph added under §CLI framework. Doc-drift test `architecture_md_lists_replay_and_export_as_shipped` enforces these.
- **AC #7 (protocol-changelog)**: One new `type: schema` entry appended under the v1.0 → v1.1 section with `(Resolves: 4.1)` marker; covers endpoint shape, bearer-auth requirement, per-line continue-on-error policy, `ingest_tx` relationship, sentinel rejection, no rate-limit, wall-clock-rewrite contract, and the round-trip with `bowerbird export`. Doc-drift test `protocol_changelog_documents_post_replay_endpoint` enforces presence.
- **CLI dep-tree invariant (Task 8.1)**: `cargo tree -p bowerbird --depth 8 | grep -cE '^.* (tokio|axum|reqwest|ureq) v'` → `0`. The CLI continues to use only `serde`, `serde_json`, `protocol`, `clap`, `anyhow`, `libc`, `secrecy`, `keyring`, `toml`, `adapter-claude`.
- **Departure from story spec**: The story's Task 4.3 step 3 ("404 → `session <id> not found`") could not be satisfied by `GET /sessions/{id}/events` alone — that endpoint returns 200 with empty events list for unknown session IDs (it just queries the events table by session_id). Adding a pre-check via `GET /sessions/{id}` (which DOES 404 for unknown sessions via `session_projections` lookup) was the cleanest fix. Added `http_get_session_detail` helper alongside `http_get_events`. This adds one round trip per export but only on the success path. Recorded as deferred-work entry #7 in the Story 4.1 deferred-work block suggesting future alignment of `/sessions/{id}/events` to 404 for unknown sessions.

### File List

**Created:**
- `crates/daemon/src/api/replay.rs` (daemon `POST /replay` handler)
- `src/commands/replay.rs` (CLI `bowerbird replay [<file>]`)
- `src/commands/export.rs` (CLI `bowerbird export <session-id> [-o <path>]`)
- `fixtures/replay-demo.jsonl` (bundled demo fixture; 12 events / 2 sessions)
- `tests/cli_replay.rs` (CLI E2E suite for replay)
- `tests/cli_export.rs` (CLI E2E suite for export)
- `tests/cli_replay_fixture.rs` (hermetic doc-drift guardrails)

**Modified:**
- `src/main.rs` (added `Export`, `Replay` Command variants + match arms)
- `src/commands/mod.rs` (added `pub mod export;` + `pub mod replay;`)
- `src/commands/daemon.rs` (added `ReplayResponse`, `EventsResponse`, `SessionDetailResponse` enums and `http_post_replay`, `http_get_events`, `http_get_session_detail` helpers + `encode_path_segment` + unit tests)
- `crates/daemon/src/state.rs` (added `pub ingest_tx` field to `AppState`)
- `crates/daemon/src/main.rs` (cloned `ingest_tx` into `AppState` construction)
- `crates/daemon/src/api/mod.rs` (added `pub mod replay;`, `axum::routing::post` import, `.route("/replay", post(replay::run))`)
- `crates/daemon/tests/contract_daemon.rs` (added `ingest_tx` field to two direct `AppState` construction sites + `make_test_state_with_ws`; added `story_4_1_replay` module with 6 tests)
- `README.md` (added one Quickstart line demonstrating `bowerbird replay` against bundled fixture)
- `docs/bmad/planning-artifacts/architecture.md` (§CLI framework, §Implementation Order, §Project Structure tree, §FR mapping, new §Replay & Export paragraph)
- `docs/protocol-changelog.md` (new `type: schema` entry for `POST /replay` with `Resolves: 4.1`)
- `docs/bmad/implementation-artifacts/deferred-work.md` (new "Deferred from: Story 4.1" section with 7 entries)
- `docs/bmad/implementation-artifacts/sprint-status.yaml` (story status `ready-for-dev` → `in-progress` → `review` → `done`; `last_updated` bumped)
- `docs/bmad/implementation-artifacts/4-1-bowerbird-replay-and-export-commands.md` (task checkboxes [x], Dev Agent Record populated, Status: done)
- `docs/bmad/implementation-artifacts/tests/test-summary.md` (rewritten for Story 4.1; documents the QA-pass-added tests for Gap A/B/C/D coverage; supersedes the Story 3.4 summary)

### Change Log

- 2026-05-25 — Story 4.1 implementation pass: added `POST /replay` endpoint, `bowerbird replay [<file>]` CLI, `bowerbird export <session-id>` CLI, bundled fixture, contract tests, E2E tests, doc-drift guardrails. All ACs covered; all gates pass.
- 2026-05-25 — Story 4.1 review pass (auto-fix): reconciled File List with git (added `tests/test-summary.md` to Modified, which the QA pass had rewritten for Story 4.1 without it being tracked). Refreshed Debug Log References to reflect actual test counts after the QA-pass test additions (story_4_1_replay 6 → 8, cli_export 4 → 7). Status: review → done. Sprint status synced.
