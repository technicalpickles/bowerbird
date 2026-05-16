---
stepsCompleted:
  - step-01-validate-prerequisites
  - step-02-design-epics
inputDocuments:
  - docs/bmad/planning-artifacts/prd.md
  - docs/bmad/project-context.md
---

# bowerbird - Epic Breakdown

## Overview

This document provides the complete epic and story breakdown for bowerbird, decomposing the requirements from the PRD and project-context.md (which serves as the architecture reference) into implementable stories.

## Requirements Inventory

### Functional Requirements

FR1: The shim can capture Claude Code hook events and deliver them to the daemon without adding perceptible latency to Claude Code's operation
FR2: The shim can operate without network timeouts or blocking calls that could delay Claude Code's hook execution
FR3: Tool builders can install and remove the bowerbird hook from Claude Code's configuration without manually editing configuration files
FR4: The Claude Code adapter can normalize Claude Code hook payloads into the canonical protocol event format
FR5: The shim can log failure information to a dedicated log file without writing to stdout or stderr
FR6: The daemon can persist incoming events to a local event log atomically with their associated session state projection
FR7: The daemon can survive unexpected termination without leaving the event log in a corrupt or inconsistent state
FR8: Tool builders can query the event log with a cursor to retrieve events from a specific point forward
FR9: The daemon exposes the oldest available event identifier so tools can detect whether they have missed events
FR10: Tool builders can subscribe to a stream of agent activity events over a persistent connection
FR11: Tool builders can filter their subscription to specific topics at session, source, or global scope
FR12: Tool builders can subscribe to activity across all sessions simultaneously using a wildcard subscription
FR13: The daemon can notify subscribed tools when new sessions appear without requiring reconnection
FR14: The daemon can notify a tool when it has missed events due to slow consumption, including how many events were missed
FR15: The daemon can deliver a current-state snapshot to a connecting tool without requiring a separate query
FR16: Multiple tools can connect to and receive the same event stream simultaneously without affecting each other
FR17: The daemon can send a shutdown notification to connected tools before terminating
FR18: Tool builders can retrieve a list of known agent sessions
FR19: Tool builders can retrieve the current projected state of a specific session
FR20: Tool builders can retrieve paginated event history for a session from a given cursor position
FR21: Tool builders can retrieve per-session event statistics
FR22: Tool builders can check daemon liveness without authenticating
FR23: Tool builders can check daemon readiness — including storage and broadcaster state — without authenticating
FR24: The daemon can track multiple concurrent agent sessions, distinguishing them by both source and session identifier
FR25: The daemon can maintain a current-state projection per session, updated in the same operation as event storage
FR26: The daemon can tolerate missing hook events without entering an inconsistent or stuck state
FR27: Tool builders can install bowerbird without a Rust development environment using prebuilt binaries from GitHub Releases
FR28: Tool builders can install bowerbird from source using the Rust toolchain
FR29: Tool builders can start and stop the daemon independently of the Claude Code hook configuration
FR30: Tool builders can check the daemon's current status and version from the command line
FR31: Tool builders can replay a recorded event sequence through the daemon's full pub/sub path without a live Claude Code session
FR32: Tool builders can export a real session's events to a file for replay or debugging
FR33: Tool builders can access reference implementations demonstrating event subscription, multi-session fan-out, and dropped-frame recovery
FR34: Tool builders can run all reference implementations against bundled fixture data without a live agent session
FR35: Tool builders can access documentation covering: quickstart (no live agent required), tool-building guide, protocol reference, and recipe cookbook
FR36: The protocol guarantees that tools built against v1 continue to work on any v1.x daemon release without modification
FR37: The daemon accepts inbound events via a socket accessible only to the current OS user
FR38: Tool builders can authenticate REST and WebSocket connections using a bearer token
FR39: Tool builders can access structured changelog information identifying the type and nature of any protocol changes between releases

### NonFunctional Requirements

NFR1: The shim must add no more than 5ms at the p95 percentile to Claude Code's hook execution time (hard constraint; benchmarked from day one via shim/benches/hot_path.rs)
NFR2: The daemon must introduce no perceptible lag under normal single-developer load on a modern laptop; performance is tuned when evidence warrants, not speculatively
NFR3: The daemon must be ready to accept connections within 2 seconds of cold start on reference hardware; verified via the health endpoint
NFR4: The event log is unbounded for V1; the documented V1 escape hatch is deleting or truncating ~/.bowerbird/bower.db directly; a dedicated bowerbird gc command for managed truncation is post-V1
NFR5: When the host filesystem is full (ENOSPC), the daemon logs the drop at error level and closes the ingest connection; the shim treats any write error as fire-and-forget and exits 0 without blocking Claude Code
NFR6: The event log survives unexpected daemon termination; any event acknowledged to the shim is durable on restart (guaranteed by WAL-mode atomic writes)
NFR7: The daemon accepts unbounded event ingest rate in V1 for single-developer workloads; no rate limiting or burst protection; this is a documented design limitation
NFR8: Prebuilt binaries target currently-supported macOS versions on both x86_64 and arm64
NFR9: Linux prebuilts target glibc-based distributions; musl deferred post-V1
NFR10: The cargo install path requires only the Rust stable toolchain; no nightly features
NFR11: The daemon bearer token is a UUID4 value, stored in the system keychain (macOS Keychain / Linux Secret Service) and retrieved via bowerbird auth token
NFR12: Fallback order when keychain unavailable: (1) environment variable, (2) on-disk config file in ~/.bowerbird/; fallback mechanism is documented
NFR13: If no token is resolvable via any fallback path, the daemon exits non-zero with a human-readable error to stderr
NFR14: Token rotation requires a daemon restart; the daemon reads the token once at startup and does not hot-reload it
NFR15: The shim failure log is created with mode 0600 regardless of the process umask
NFR16: The daemon logs at error level by default; -v and -vv flags expose progressively more detail; each log line follows the format <ISO8601 timestamp> <LEVEL> <message>; structured JSON logging deferred to V2
NFR17: On unexpected crash, the daemon writes crash information to ~/.bowerbird/; no external crash reporting
NFR18: A daemon metrics endpoint is deferred until usage patterns justify it; health and readiness endpoints are sufficient for V1
NFR19: No breaking changes to the REST or WebSocket protocol within any v1.x release series; tools built against v1.0 continue to work on any v1.x daemon without modification
NFR20: The daemon's ingest socket listen backlog is at minimum 128; the shim exits non-zero on ECONNREFUSED or socket-not-found (daemon unreachable), and exits 0 on mid-write errors
NFR21: The daemon auto-migrates the SQLite schema on startup; migration failures are fatal with a human-readable error to stderr
NFR22: The V1 event log schema includes a timestamp column on all event rows to support future event-log management without schema changes

### Additional Requirements

Architecture requirements from project-context.md that affect story implementation:

**Workspace structure:**
- Multi-crate Rust workspace: crates/protocol, crates/shim, crates/daemon, crates/adapter-claude
- adapters/claude/ for TOML data files (capabilities, tool-reactions, settings-merge)
- examples/ for reference tools (TypeScript/Node), CI smoke-tested
- docs/ for all documentation deliverables
- #![deny(unsafe_code)] at every crate root; Cargo.lock committed; MSRV pinned per-crate

**Shim implementation constraints:**
- No async runtime (no Tokio); sync I/O only; std::process::exit
- No heap allocation on the hot path (use &str and stack buffers)
- No anyhow; thiserror only with small fixed error enum
- Logging on failure to file (~/.bowerbird/shim.log with rotation) only; never stdout/stderr
- release-shim profile: panic=abort, lto=fat, codegen-units=1, opt-level=z, strip=true

**Daemon implementation constraints:**
- Single-threaded Tokio runtime (current_thread); axum for HTTP+WebSocket
- deadpool-sqlite for async DB access; two pools: writer(max=1) + readers(max=4)
- Connection factory is the only path to rusqlite::Connection; CI lint enforces this
- PRAGMA invariants set on every connection: journal_mode=WAL, synchronous=NORMAL, foreign_keys=ON, busy_timeout=5000
- rusqlite_migration for schema migrations from day one
- AppState: { db: DbPools, broadcasters: Broadcasters, auth: TokenStore, shutdown: CancellationToken }
- Required middleware: CatchPanicLayer, request-id, TimeoutLayer (30s HTTP), RequestBodyLimitLayer
- WS ping/pong every 30s per-client; WS concurrency cap via Semaphore (default 256)
- tokio::sync::broadcast for pub/sub; per-client task owns the WS sink
- Graceful shutdown: drain broadcast channels (5s timeout), flush DB pools, send close frames, exit 0

**Protocol crate constraints:**
- thiserror only; no anyhow
- All public types implement Serialize + Deserialize
- Asymmetric deny_unknown_fields: strict on inbound parse, permissive on outbound emit
- Versioned via protocol@vN namespace; additive-only within v1.x
- CI gate: any change to crates/protocol/src/*.rs requires a protocol-changelog.md entry

**Wire format and auth:**
- JSON wire format; TOML for adapter configs
- Two socket surfaces: Unix domain socket (~/.bowerbird/ingest.sock, 0600) for shim→daemon ingest; TCP (127.0.0.1) for tools
- Bearer token: UUID4, stored in macOS Keychain / Linux Secret Service; fallback to env var then file
- PATH-relative binary name in ~/.claude/settings.json hook entry (not absolute path)
- Atomic ~/.claude/settings.json install: read → parse → merge → write .tmp → rename

**CI requirements:**
- cargo fmt --check, cargo clippy --all-targets --workspace -- -D warnings, cargo test --workspace
- cargo build --examples and cargo test --examples
- cargo bench --no-run; shim hot-path bench with p99 regression alarm
- shellcheck strict mode on all shell scripts
- macOS-latest + ubuntu-latest minimum; per-platform perf baselines committed as files

**Required contract tests (10 total, must pass before MVP):**
1. WS dropped-frame behavior (lag → one dropped frame → socket stays open)
2. PRAGMA invariants on every connection checkout
3. Connection factory enforcement (CI lint forbids raw Connection::open outside factory)
4. State-emission and event-INSERT atomicity (SIGKILL test)
5. Graceful shutdown (SIGTERM mid-ingest, exit 0, in-flight event fully committed or rolled back)
6. Cursor-gap detection (oldest_available_event_id in response)
7. Atomic ~/.claude/settings.json install (interrupt simulation)
8. Hook unreliability tolerance (PreToolUse without PostToolUse → sane projection state)
9. Outbound envelope additive-compat (extra field round-trips without error)
10. (source, session_id) collision safety (identical session_id, different source → distinct sessions)

### UX Design Requirements

N/A — bowerbird is a headless substrate with CLI and API surfaces only. No UI design document is required or in scope.

### FR Coverage Map

FR1: Epic 1 — Shim captures hooks without perceptible latency
FR2: Epic 1 — Shim operates without blocking calls
FR3: Epic 3 — Install/remove hook via CLI
FR4: Epic 1 — Adapter normalizes Claude Code payloads to canonical format
FR5: Epic 1 — Shim logs failures to file only (never stdout/stderr)
FR6: Epic 1 — Atomic event + projection persistence
FR7: Epic 1 — Daemon survives unexpected termination without corruption
FR8: Epic 1 — Cursor-based event log query
FR9: Epic 1 — Oldest available event ID exposed for gap detection
FR10: Epic 2 — Subscribe to event stream over persistent WS connection
FR11: Epic 2 — Filter subscription by topic
FR12: Epic 2 — Wildcard subscription across all sessions
FR13: Epic 2 — Notify tools of new sessions without reconnect
FR14: Epic 2 — Dropped frame notification with lag count
FR15: Epic 2 — State snapshot delivered on connect
FR16: Epic 2 — Multiple simultaneous tool connections
FR17: Epic 2 — Shutdown notification to connected tools
FR18: Epic 1 — List known sessions via REST
FR19: Epic 1 — Retrieve projected state of a session
FR20: Epic 1 — Cursor-paginated event history via REST
FR21: Epic 1 — Per-session event statistics via REST
FR22: Epic 1 — /healthz liveness check (no auth required)
FR23: Epic 1 — /readyz readiness check (no auth required)
FR24: Epic 1 — Multi-session tracking by (source, session_id)
FR25: Epic 1 — Current-state projection per session
FR26: Epic 1 — Tolerate missing hook events without inconsistent state
FR27: Epic 3 — Install via prebuilt binaries (no Rust toolchain needed)
FR28: Epic 3 — Install via cargo from source
FR29: Epic 3 — Start/stop daemon via CLI
FR30: Epic 3 — Status and version check via CLI
FR31: Epic 4 — Replay recorded event sequence through daemon pub/sub
FR32: Epic 4 — Export session events to JSONL file
FR33: Epic 4 — Reference implementations (subscription, fan-out, dropped-frame recovery)
FR34: Epic 4 — Run reference examples against bundled fixtures
FR35: Epic 4 — Full documentation path (quickstart, presenter-authoring, protocol ref, cookbook)
FR36: Epic 4 — v1.x protocol backward compatibility guarantee
FR37: Epic 1 — Ingest socket accessible only to current OS user (Unix socket 0600)
FR38: Epic 3 — Bearer token authentication for REST and WebSocket
FR39: Epic 4 — Structured protocol changelog (CI-enforced)

## Epic List

### Epic 1: Agent activity captured and queryable via REST
A tool builder can run bowerbird alongside Claude Code and query captured events via REST. This epic produces the full capture pipeline — protocol crate, shim binary, daemon with SQLite persistence and REST API — everything needed to have real data to work with. Hook installation is manual in this epic; the install CLI arrives in Epic 3.
**FRs covered:** FR1, FR2, FR4, FR5, FR6, FR7, FR8, FR9, FR18, FR19, FR20, FR21, FR22, FR23, FR24, FR25, FR26, FR37

### Epic 2: Live event streaming to multiple simultaneous tools
Tool builders can subscribe to Claude Code activity via WebSocket, with multiple tools running simultaneously, automatic lag recovery via dropped frames, and state snapshot delivery on connect. Builds on Epic 1's daemon foundation.
**FRs covered:** FR10, FR11, FR12, FR13, FR14, FR15, FR16, FR17

### Epic 3: Easy installation, lifecycle management, and secure access
Tool builders can install bowerbird with a single command, manage the daemon lifecycle via CLI, and authenticate tool connections using a secure bearer token. Prebuilt binaries make bowerbird available without a Rust toolchain.
**FRs covered:** FR3, FR27, FR28, FR29, FR30, FR38

### Epic 4: Developer experience, replay, and protocol stability
Tool builders can learn bowerbird via comprehensive docs and reference examples, experiment with a fake event stream without a live Claude Code session, and build against a stable protocol with a documented compatibility guarantee and CI-enforced changelog.
**FRs covered:** FR31, FR32, FR33, FR34, FR35, FR36, FR39

---

## Epic 1: Agent activity captured and queryable via REST

A tool builder can run bowerbird alongside Claude Code and query captured events via REST. This epic produces the full capture pipeline — protocol crate, shim binary, daemon with SQLite persistence, and REST API — everything needed to have real data to work with. Hook installation in this epic is manual (direct settings.json edit); the install CLI arrives in Epic 3.

### Story 1.1: Workspace and protocol crate foundation

As a tool builder,
I want a stable, well-typed protocol library that defines all bowerbird wire types,
So that I can write deserializers and client code against a documented, versioned schema before the daemon is complete.

**Acceptance Criteria:**

**Given** the bowerbird Rust workspace (Cargo.toml, crates/protocol, crates/shim, crates/daemon, crates/adapter-claude)
**When** I run `cargo build --workspace`
**Then** all crates compile cleanly with zero warnings, `cargo fmt --check` passes, and `cargo clippy --all-targets --workspace -- -D warnings` passes

**Given** the protocol crate's outbound types (EventEnvelope, state frames, event frames)
**When** I deserialize a wire payload that contains an extra unknown field not present in the Rust struct
**Then** deserialization succeeds without error, confirming the permissive outbound policy

**Given** the protocol crate's inbound parse types (subscribe messages, REST request bodies, adapter configs)
**When** I submit a payload containing an unknown field
**Then** deserialization fails with a clear `deny_unknown_fields` error, confirming the strict inbound policy

**Given** any crate root in the workspace
**When** I add an `unsafe` block anywhere in the crate
**Then** the build fails due to `#![deny(unsafe_code)]`

**Given** the workspace Cargo.toml and each crate's Cargo.toml
**When** I inspect them
**Then** every crate declares a pinned `rust-version` (MSRV), `Cargo.lock` is committed to the repository, and the edition is 2021

**Given** a GitHub Actions PR workflow
**When** a pull request is opened
**Then** the CI matrix runs `cargo fmt --check`, `cargo clippy --all-targets --workspace -- -D warnings`, and `cargo test --workspace` on both macOS-latest and ubuntu-latest runners

### Story 1.2: Daemon foundation with SQLite persistence

As a tool builder,
I want bowerbird's daemon to persist events durably to a local WAL-mode SQLite database that survives crashes,
So that I can trust no acknowledged event is ever lost due to unexpected daemon termination.

**Acceptance Criteria:**

**Given** a running daemon that has acknowledged a `POST /ingest` event with a 200 response
**When** SIGKILL is sent to the daemon and the daemon is restarted
**Then** the acknowledged event is present in the event log (NFR6: WAL durability guarantee)

**Given** a connection is checked out from either the writer pool (max\_size=1) or any reader pool (max\_size=4)
**When** `PRAGMA foreign_keys`, `PRAGMA journal_mode`, and `PRAGMA synchronous` are queried on that connection
**Then** the results are 1 (ON), 'wal', and 1 (NORMAL) respectively, on every checkout without exception

**Given** the daemon starts for the first time against a fresh data directory
**When** the daemon process becomes ready
**Then** schema migrations have run automatically via rusqlite\_migration and `GET /readyz` returns 200 (NFR21)

**Given** a migration failure (e.g., manually corrupted user\_version)
**When** the daemon attempts to start
**Then** it exits non-zero with a human-readable error message to stderr before accepting any connections

**Given** the writer pool is actively inserting rows
**When** a reader pool connection executes a SELECT query concurrently
**Then** the reader completes without blocking on the writer (WAL concurrent read/write validation)

**Given** any file in the codebase
**When** a CI lint (grep or clippy) scans for `rusqlite::Connection::open`
**Then** any call outside the designated connection factory module fails the build, confirming the factory-only access policy

### Story 1.3: Unix socket ingest endpoint

As a tool builder,
I want the daemon to accept events from the shim via a local Unix domain socket with filesystem-level access control,
So that only processes running as my OS user can inject events into bowerbird, with no bearer token overhead on the hot path.

**Acceptance Criteria:**

**Given** a running daemon
**When** the ingest socket is created at `~/.bowerbird/ingest.sock`
**Then** its file mode is 0600 (accessible only to the owning user)

**Given** a well-formed `POST /ingest` request over the Unix socket
**When** the daemon processes it
**Then** it returns 200 synchronously after accepting the event into the write queue — not after the SQLite commit — and the shim receives the ACK within the 5ms budget

**Given** the write queue is at maximum capacity (backpressure condition)
**When** the shim sends `POST /ingest`
**Then** the daemon returns 503, the shim logs a warning to `~/.bowerbird/shim.log`, and the shim exits 0 (fire-and-forget per NFR5)

**Given** the daemon is not running (socket does not exist or ECONNREFUSED)
**When** the shim attempts to connect to the ingest socket
**Then** the shim logs to `~/.bowerbird/shim.log` and exits non-zero, surfacing the hook failure to Claude Code

**Given** the ingest socket
**When** its listen backlog is checked
**Then** it is at minimum 128 (per NFR20)

**Given** a malformed or structurally invalid event payload on `POST /ingest`
**When** the daemon attempts to parse it
**Then** it returns 400 with a descriptive error and does not insert a partial row into the event log

### Story 1.4: Claude Code adapter and event normalization

As a tool builder,
I want Claude Code hook payloads normalized to the canonical bowerbird event format before storage,
So that my tools receive consistent, predictable data regardless of changes to Claude Code's internal hook schema.

**Acceptance Criteria:**

**Given** a PreToolUse hook payload from Claude Code containing tool\_name, session\_id, and native payload fields
**When** the adapter-claude crate processes it
**Then** the resulting EventEnvelope contains: `source="claude"`, `session_id` from the hook, `event_kind=PreToolUse`, the correct `reaction` from tool-reactions.toml lookup, and the complete native payload verbatim in the `payload` column with no fields stripped or renamed

**Given** a tool name that is not present in `adapters/claude/tool-reactions.toml`
**When** the adapter processes a hook event for that tool
**Then** the `reaction` field is set to the `Unknown` enum variant, the event is still persisted without error, and no panic occurs

**Given** `adapters/claude/tool-reactions.toml` is updated with a new tool→reaction mapping at runtime
**When** the adapter processes the next hook event for that tool
**Then** it uses the updated mapping (TOML file is the source of truth, not a hardcoded enum)

**Given** two hook events that share an identical `session_id` value but have different `source` values ("claude" vs. a hypothetical second source)
**When** both are ingested
**Then** they are stored as distinct sessions and appear as separate records in `GET /sessions`

**Given** a Claude Code hook payload that contains extra unknown fields beyond the defined schema
**When** the adapter normalizes it
**Then** those fields are preserved verbatim in the `payload` column (the substrate observes, it does not filter)

### Story 1.5: Shim binary with hot-path event delivery

As a Claude Code user,
I want the bowerbird shim to capture and forward hook events to the daemon in under 5ms at p99,
So that bowerbird is invisible during normal coding sessions and never causes Claude Code to feel slow.

**Acceptance Criteria:**

**Given** the shim compiled with the release-shim profile (`panic=abort`, `lto=fat`, `codegen-units=1`, `opt-level=z`, `strip=true`)
**When** Criterion runs `shim/benches/hot_path.rs` on a warm-cache daemon connection on both macOS-latest and ubuntu-latest CI runners
**Then** p99 latency is ≤5ms per platform (baselines committed as files, per-platform — not averaged); a p99 regression >15% from the committed baseline fails CI

**Given** a successful shim event delivery
**When** the shim exits
**Then** it exits with code 0 and has written nothing to stdout or stderr

**Given** the daemon is unreachable (ECONNREFUSED or socket path does not exist)
**When** the shim attempts delivery
**Then** it logs a timestamped error line to `~/.bowerbird/shim.log` and exits non-zero

**Given** a 503 backpressure response from the daemon
**When** the shim receives it
**Then** it exits 0 and logs a warning to `~/.bowerbird/shim.log` (fire-and-forget)

**Given** `~/.bowerbird/shim.log` is created for the first time
**When** its file mode is inspected
**Then** it is 0600, regardless of the process umask (NFR15)

**Given** the shim source code in `crates/shim`
**When** it is searched for `tokio`, `async fn`, or `.await`
**Then** none are found — the shim contains no async runtime, only synchronous I/O

### Story 1.6: Session projection and hook unreliability tolerance

As a tool builder,
I want the daemon to maintain a consistent current-state projection per Claude Code session that stays sane even when hook events are dropped,
So that my tools always show meaningful session state rather than getting stuck due to missing hook delivery.

**Acceptance Criteria:**

**Given** a `PreToolUse` event is ingested for a session but the corresponding `PostToolUse` event never arrives (hook dropped)
**When** the projection is queried after a defined timeout window
**Then** the session's `current_state` is not permanently stuck in `working` — it falls through to a sane fallback state

**Given** an event INSERT and its corresponding projection row update are committed
**When** SIGKILL is sent to the daemon mid-transaction
**Then** on daemon restart, every projection row has a matching event-log row and vice versa — no half-state exists (state+event atomicity contract test)

**Given** two sessions with identical `session_id` values but different `source` values
**When** events are ingested for both simultaneously
**Then** their projections are stored and queried independently with no cross-contamination — `(source, session_id)` is the natural key throughout

**Given** a sequence of mixed PreToolUse and PostToolUse events for a session
**When** the projection is queried
**Then** `current_state` reflects the deterministic state derivable from the complete event sequence

**Given** the projection rebuild test: projection table is deleted, daemon restarts, event log is intact
**When** the daemon finishes startup
**Then** the rebuilt projection is byte-identical to the pre-deletion state (event log is the source of truth)

### Story 1.7: REST query API

As a tool builder,
I want to query bowerbird's REST API for session list, projected state, and cursor-paginated event history with gap-detection support,
So that I can build tools that show current Claude Code session state and recover correctly when they have missed events.

**Acceptance Criteria:**

**Given** a running daemon
**When** I call `GET /healthz` with no Authorization header
**Then** I receive 200 `{"status":"ok"}` — process is up and responding

**Given** a running daemon with DB reachable and migrations applied
**When** I call `GET /readyz` with no Authorization header
**Then** I receive 200; if the DB is unreachable or migrations have not applied, I receive 503

**Given** events have been ingested for two sessions
**When** I call `GET /sessions` with a valid bearer token
**Then** both sessions appear in the JSON response

**Given** 100 events ingested for a session
**When** I call `GET /sessions/:id/events?since=0` with a valid bearer token
**Then** all 100 events are returned in ascending `event_id` order, each row contains a `timestamp` field (NFR22), and the response body includes `oldest_available_event_id`

**Given** the first 50 of 100 events have been purged from the log
**When** I call `GET /sessions/:id/events?since=10` with a valid bearer token
**Then** the response contains `oldest_available_event_id=50`, enabling my client to detect that events 10–49 are no longer available (gap-detection mechanical fact, per Axiom 4)

**Given** a request to any authenticated endpoint (`/sessions`, `/sessions/:id`, `/sessions/:id/events`, `/sessions/:id/stats`, `/status`)
**When** no Authorization header or an incorrect bearer token is provided
**Then** I receive 401

**Given** `GET /sessions/:id/stats` with a valid bearer token
**When** the response body contains an extra unknown field that was added in a future daemon release
**Then** a v1.0 client that does not know about that field still deserializes the response without error (additive-compat validation)
