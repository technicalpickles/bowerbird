---
stepsCompleted:
  - step-01-validate-prerequisites
  - step-02-design-epics
  - step-03-create-stories
  - step-04-final-validation
inputDocuments:
  - docs/bmad/planning-artifacts/prd.md
  - docs/bmad/project-context.md
revisions:
  - 2026-05-24: Folded Epic 2 retrospective action items AI-1..AI-6 into Story 3.1 (singleton enforcement), Story 3.2 (connected_ws_clients wiring), Story 3.4 (CI --test-threads=1, architecture.md WebSocket subsystem section), Story 4.4 (wire-enum serde(other) sweep, hook-to-presenter p99 Criterion bench, NDJ ingest framing narrative). Source: docs/bmad/implementation-artifacts/epic-2-retro-2026-05-24.md
  - 2026-05-26: Added Epic 5 (V1 Release Readiness) with 6 stories — first-party presenter (sibling repo), bench gates load-bearing, release pipeline E2E, install UX + middleware closure, first-time-reader docs pass, crates.io + v0.1.0 tag. Source: docs/bmad/planning-artifacts/sprint-change-proposal-2026-05-26.md (folds Epic 3 retro AI-3/AI-4, Epic 4 retro AI-1..AI-5, plus 5 deferred-work entries).
  - 2026-05-26: Inserted new Story 5.5 (Cookbook consolidation) into Epic 5; old 5.5 (first-time-reader docs pass) → 5.6; old 5.6 (crates.io + v0.1.0 tag) → 5.7. Closes Story 4.2/4.3 cookbook-coupling AC and deferred-work.md:104. Source: docs/bmad/planning-artifacts/sprint-change-proposal-2026-05-26-cookbook-consolidation.md.
  - 2026-05-27: Inserted new Story 5.7 (Session state projection correctness) into Epic 5; old 5.7 (crates.io + v0.1.0 tag) → 5.8. Tightens state-broadcast to transitions-only, removes the PostToolUse→Idle flip, adds UserPromptSubmit hook subscription. Surfaced during dogfooding via pickletown /sessions livestream page. Source: docs/bmad/planning-artifacts/sprint-change-proposal-2026-05-27.md.
  - 2026-05-27: Inserted new Story 5.8 (Session-process liveness via PID capture) into Epic 5; old 5.8 (crates.io + v0.1.0 tag) → 5.9. Adds `SessionState.last_pid` (mechanical fact), shim captures `getppid()`, presenters compute liveness via `kill(pid, 0)` per Axiom 1/4. Surfaced during Story 5.1 bowerbird-deck dogfooding (30+ stale session rows visible, no signal to mark dead-process tombstones). Source: docs/bmad/planning-artifacts/sprint-change-proposal-2026-05-27-pid-liveness.md.
  - 2026-05-27: Resequenced Epic 5 for dogfooding-first ordering. The two dogfood-surfaced correctness stories move forward (old 5.7 projection correctness → new 5.2; old 5.8 PID liveness → new 5.3) so they sit adjacent to Story 5.1's presenter and make daily dogfooding actually useful before the CI/release/docs polish work. Reader-facing and CI/release work shifts back (old 5.2 bench gates → new 5.5; old 5.3 release E2E → new 5.6; old 5.5 cookbook → new 5.7; old 5.6 first-time-reader docs → new 5.8). Story 5.1, 5.4, 5.9 unchanged. No story content modified; pure resequencing. Source: docs/bmad/planning-artifacts/sprint-change-proposal-2026-05-27-epic-5-resequencing.md.
  - 2026-05-29: Inserted new Story 5.6 (`idle_prompt` reclassified as transient) into Epic 5 after bench-gates (5.5); old 5.6 (release pipeline E2E) → 5.7, old 5.7 (cookbook consolidation) → 5.8, old 5.8 (first-time-reader docs) → 5.9, old 5.9 (crates.io + v0.1.0 tag) → 5.10. Moves `idle_prompt` from the input-required bucket to the transient (preserve-prior) bucket in `transition()`, narrowing `WaitingInput` to genuine hard blocks (`permission_prompt`/`elicitation_dialog`). Surfaced during 2026-05-29 bowerbird-deck dogfooding (13 of 15 live sessions falsely `WaitingInput`). Amends ADR 0004 §3; recorded as ADR 0005. Source: docs/bmad/planning-artifacts/sprint-change-proposal-2026-05-29-idle-prompt-reclassification.md.
  - 2026-06-01: Inserted four dogfood-triage stories into Epic 5 after Story 5.6 — 5.7 (session cwd + started_at on the wire; +ADR 0006), 5.8 (server-side session filter), 5.9 (daemon start-on-login supervision; +ADR 0007), 5.10 (shim names the cause on daemon-down). Renumbered the release-readiness tail: old 5.7 (release pipeline E2E) → 5.11, old 5.8 (cookbook) → 5.12, old 5.9 (first-time-reader docs) → 5.13, old 5.10 (crates.io + v0.1.0 tag) → 5.14. All four new stories gate the v0.1.0 tag (now Story 5.14). Surfaced during 2026-06-01 deck+web triage-radar dogfooding (presenters can only triage on what the wire carries: no cwd to group by, full Ended graveyard dumped on connect, no daemon supervision across reboot, causeless hook-error wall). Story 5.7 fully specified here and landing first; 5.8–5.10 are stubs to be fleshed out at their own create-story time. Source: docs/bmad/planning-artifacts/sprint-change-proposal-2026-06-01-dogfood-triage.md.
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

### Epic 5: V1 Release Readiness
The maintainer dogfoods bowerbird daily via a first-party presenter (sibling repo), and the planned stories then harden the substrate for public release: bench gates converted to load-bearing, release pipeline driven end-to-end, install UX polished, README + quickstart rewritten for first-time readers, crates.io namespace decision, v0.1.0 tag. Closes Epic 4 retro AI-1..AI-5 and 5 deferred-work entries.
**FRs covered:** hardening only — no new FRs.

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

**Given** a running daemon that has acknowledged an ingest write with a `200` status line
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

**Given** the daemon is running with default log level
**When** it emits log output
**Then** each line follows the format `<ISO8601 timestamp> <LEVEL> <message>` and the default level is error; running with `-v` exposes info-level output and `-vv` exposes debug-level output (NFR16)

**Given** the daemon crashes unexpectedly (panic or unhandled error)
**When** the process exits
**Then** crash information (panic message, backtrace if available) is written to a file under `~/.bowerbird/` and nothing is sent to an external crash reporting service (NFR17)

### Story 1.3: Unix socket ingest endpoint

As a tool builder,
I want the daemon to accept events from the shim via a local Unix domain socket with filesystem-level access control,
So that only processes running as my OS user can inject events into bowerbird, with no bearer token overhead on the hot path.

**Acceptance Criteria:**

**Given** a running daemon
**When** the ingest socket is created at `~/.bowerbird/ingest.sock`
**Then** its file mode is 0600 (accessible only to the owning user)

**Given** a well-formed event line on the ingest socket (one `{object}\n` over the Unix socket per [ADR-0002](../../decisions/0002-ingest-wire-framing-and-hook-kind.md))
**When** the daemon processes it
**Then** it returns `200\n` synchronously after accepting the event into the write queue — not after the SQLite commit — and the shim receives the ACK within the 5ms budget

**Given** the write queue is at maximum capacity (backpressure condition)
**When** the shim sends an ingest line on the Unix socket
**Then** the daemon returns 503, the shim logs a warning to `~/.bowerbird/shim.log`, and the shim exits 0 (fire-and-forget per NFR5)

**Given** the daemon is not running (socket does not exist or ECONNREFUSED)
**When** the shim attempts to connect to the ingest socket
**Then** the shim logs to `~/.bowerbird/shim.log` and exits non-zero, surfacing the hook failure to Claude Code

**Given** the ingest socket
**When** its listen backlog is checked
**Then** it is at minimum 128 (per NFR20)

**Given** a malformed or structurally invalid event line on the ingest socket
**When** the daemon attempts to parse it
**Then** it returns `400 <reason>\n` and does not insert a partial row into the event log

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

### Story 1.8: Tighten daemon `hook_kind` to a required transport field

As a daemon maintainer,
I want the ingest handler to require `hook_kind` on every payload (no default fallback) now that the shim from Story 1.5 is the only first-party ingest client,
So that malformed or non-shim writers fail loudly with a `400` instead of silently being interpreted as `PreToolUse`.

Follow-up to [ADR-0002 §Consequences](../../decisions/0002-ingest-wire-framing-and-hook-kind.md#consequences) and the deferred-work entry from the Story 1.4 review (`docs/bmad/implementation-artifacts/deferred-work.md` line 37).

**Acceptance Criteria:**

**Given** an ingest line whose JSON object has no `hook_kind` field
**When** the daemon parses it
**Then** the daemon returns `400 missing hook_kind\n` and inserts no row, and the `"PreToolUse"` default at `crates/daemon/src/ingest/handler.rs:53-57` is removed

**Given** an ingest line with `hook_kind` set to a value the adapter does not recognize
**When** the daemon parses it
**Then** the daemon returns `400 unknown hook_kind: <value>\n` and inserts no row

**Given** the existing contract test suite (daemon + shim)
**When** Story 1.8 lands
**Then** all tests previously relying on the default still pass, either by injecting an explicit `hook_kind` or by asserting the new `400` response

**Given** Story 1.8 is merged
**When** `docs/bmad/implementation-artifacts/deferred-work.md` is reviewed
**Then** the line-37 entry ("hook_kind defaults to PreToolUse when absent ...") has been struck with a backlink to the merging PR or commit

---

## Epic 2: Live event streaming to multiple simultaneous tools

Tool builders can subscribe to Claude Code activity via WebSocket, with multiple tools running simultaneously, automatic lag recovery via dropped frames, and state snapshot delivery on connect. Builds on Epic 1's daemon foundation.

### Story 2.1: WebSocket connection and topic subscription

As a tool builder,
I want to establish an authenticated WebSocket connection to bowerbird and declare which event topics I want to receive,
So that I only receive the agent activity relevant to my tool without filtering it myself.

**Acceptance Criteria:**

**Given** a tool connects to `ws://127.0.0.1:<port>/ws` with a valid bearer token in the Authorization header or `?token=` query parameter
**When** the connection is established
**Then** the daemon sends a `hello` frame immediately containing `protocol_version` and the daemon version string

**Given** a tool sends a subscribe message `{"op":"subscribe","topic":"state.session.*"}`, then later `{"op":"subscribe","topic":"events.*"}`
**When** the daemon processes each one
**Then** the per-connection subscription set is the union of the declared topics; subsequent server frames are filtered to deliver only matches.

> Wire shape clarified per Story 2.1 creation, 2026-05-20 — single topic per Subscribe message; multi-topic via repeated sends (per `crates/protocol/src/ws.rs::ClientMessage::Subscribe { topic: String }`).

**Given** a tool connects with an invalid or missing bearer token
**When** the WebSocket upgrade is attempted
**Then** the connection is rejected with a 401 response before the upgrade completes

**Given** 257 tools attempt concurrent WebSocket connections (exceeding the default cap of 256)
**When** the 257th connection arrives
**Then** it is rejected gracefully (semaphore acquire fails) and the 256 existing connections are unaffected

**Given** a connected tool has been idle with no frames exchanged for 30 seconds
**When** the daemon's per-client ping timer fires
**Then** the daemon sends a WebSocket Ping frame; when the client responds with Pong, the connection remains open

**Given** a connected tool whose underlying TCP connection has been dropped without a FIN (e.g., abrupt network loss)
**When** the 30-second ping fires and no Pong is received within a timeout
**Then** the per-client task detects the dead connection and cleans up without leaking the task

### Story 2.2: Real-time event and state broadcast to multiple tools

As a tool builder,
I want to receive live Claude Code event and state frames over my WebSocket connection simultaneously with other connected tools,
So that multiple tools can observe the same agent activity independently without affecting each other.

**Acceptance Criteria:**

**Given** three tools connected and subscribed to `events.*`
**When** a new event is ingested by the daemon
**Then** all three tools receive the `event` frame with identical content, in the same order, within the end-to-end latency budget (hook→presenter p99 ≤100ms)

**Given** a tool subscribed to `state.session.<id>.current_state`
**When** an event causes the projection for session `<id>` to change
**Then** the tool receives a `state` frame containing the updated `current_state` value for that session only

**Given** a tool subscribed to `events.claude.*`
**When** an event from source `claude` is ingested and an event from a hypothetical second source is ingested
**Then** the tool receives only the `claude` event, confirming source-scoped topic filtering

**Given** a tool subscribed to `state.session.*` (wildcard)
**When** events arrive for three different concurrent sessions
**Then** the tool receives state frames for all three sessions routed correctly by session identity

**Given** two tools with identical topic subscriptions
**When** one tool's connection is closed
**Then** the other tool continues receiving frames uninterrupted — tools are fully independent consumers

### Story 2.3: New session discovery and state snapshot on connect

As a tool builder,
I want to receive the current state of all matching sessions immediately when I connect, and to be notified automatically when new sessions appear while I am subscribed,
So that my tool is always up to date without polling and without missing sessions that started before or during my connection.

**Acceptance Criteria:**

**Given** three active sessions exist when a tool connects and subscribes to `state.session.*`
**When** the subscription message is processed
**Then** the daemon sends one `state` frame per active session before sending any subsequent `event` frames, giving the tool a complete snapshot

**Given** a tool is connected and subscribed to `state.session.*`
**When** a new Claude Code session starts and its first event is ingested
**Then** the daemon emits a `state` frame for the new session to the subscribed tool without requiring the tool to reconnect or re-subscribe

**Given** a tool subscribed to `state.session.<specific-id>` (single session)
**When** a new session with a different ID starts
**Then** the tool does not receive a `state` frame for the new session — wildcard and specific-session subscriptions are correctly distinguished

**Given** a tool connects to a daemon with no active sessions
**When** the subscription message is sent
**Then** the daemon sends no initial `state` frames and transitions immediately to streaming new events as they arrive

### Story 2.4: Lagged consumer recovery with dropped frame

As a tool builder,
I want bowerbird to notify me with a single `dropped` frame when my tool falls behind the event stream, rather than silently losing events or closing my connection,
So that my tool can detect the gap and re-fetch state via REST to recover gracefully.

**Acceptance Criteria:**

**Given** a broadcast channel with capacity 1024 and a tool whose WebSocket read loop is blocked
**When** 1025 events are published before the tool reads any
**Then** the tool receives exactly one `dropped` frame containing the lag count in events (not bytes), and the next frame is the next legitimate event — the socket remains open

**Given** a tool receives a `dropped` frame
**When** the tool calls `GET /sessions/:id/events?since=<last_cursor>` via REST
**Then** it can re-fetch the missed events using `oldest_available_event_id` in the response to detect whether the gap is recoverable

**Given** a tool is lagging continuously for 30 seconds past the drop threshold
**When** the backpressure policy is applied
**Then** the daemon does not emit 50,000 individual `dropped` frames — it coalesces them into a bounded number of `dropped` frames per policy period (backpressure escalation contract test)

**Given** a tool that has received a `dropped` frame
**When** it resumes consuming events normally
**Then** subsequent events arrive in order with no further interruption — the channel is not permanently degraded

### Story 2.5: Graceful shutdown notification to connected tools

As a tool builder,
I want to receive a `close` frame from bowerbird before it shuts down,
So that my tool knows the disconnection is intentional and can show an appropriate status to the user rather than treating it as an error.

**Acceptance Criteria:**

**Given** three tools are connected via WebSocket and an event is mid-ingest
**When** SIGTERM is sent to the daemon
**Then** the daemon stops accepting new WebSocket connections, sends a `close` frame to all three connected tools, drains the broadcast channels (5-second timeout), flushes the DB connection pools, and exits with code 0

**Given** SIGTERM is sent to the daemon while a SQLite write transaction is in progress
**When** the daemon shuts down
**Then** the in-flight event is either fully committed or fully rolled back — no partial rows exist after restart (graceful shutdown contract test)

**Given** Ctrl-C (SIGINT) is received by the daemon
**When** the shutdown sequence runs
**Then** it follows the same path as SIGTERM — `close` frames to all clients, clean DB flush, exit 0

**Given** the broadcast channel drain takes longer than 5 seconds during shutdown
**When** the timeout expires
**Then** the daemon proceeds with shutdown rather than hanging indefinitely, logs a warning about the drain timeout, and still exits 0

---

## Epic 3: Easy installation, lifecycle management, and secure access

Tool builders can install bowerbird with a single command, manage the daemon lifecycle via CLI, and authenticate tool connections using a secure bearer token backed by the system keychain. Prebuilt binaries make bowerbird available without a Rust toolchain.

### Story 3.1: bowerbird install and uninstall

As a tool builder,
I want to add and remove the bowerbird hook from my Claude Code configuration with a single CLI command,
So that I never have to manually edit `~/.claude/settings.json` or worry about leaving my config in a broken state if the operation is interrupted.

**Acceptance Criteria:**

**Given** `~/.claude/settings.json` exists and is valid JSON
**When** I run `bowerbird install`
**Then** the hook entry is merged into settings.json using the atomic sequence (read → parse → merge → write `.tmp` → rename), the hook binary reference is a PATH-relative name (`bowerbird`), and the daemon is started if not already running

**Given** a concurrent write to `~/.claude/settings.json` occurs during `bowerbird install` (e.g., Claude Code updating settings simultaneously)
**When** the rename step detects the conflict
**Then** the operation retries with exponential backoff and either succeeds or exits non-zero with a descriptive error — never leaves settings.json partially overwritten

**Given** `bowerbird install` is interrupted mid-write (process killed between write `.tmp` and rename)
**When** Claude Code next reads `~/.claude/settings.json`
**Then** the original settings.json is still valid JSON and not partially overwritten (atomic install contract test)

**Given** `bowerbird install` has been run successfully
**When** I run `bowerbird uninstall`
**Then** the hook entry is removed from settings.json atomically, the daemon is stopped, and settings.json remains valid JSON

**Given** `~/.claude/settings.json` does not exist
**When** I run `bowerbird install`
**Then** a valid settings.json is created with the hook entry and the operation succeeds

**Given** a `bowerbird` daemon is already running and holding `~/.bowerbird/bower.db`
**When** I start a second `bowerbird` process targeting the same data directory
**Then** the second process exits non-zero with a human-readable error to stderr identifying the conflict (PID file or file lock), so no concurrent migration race is possible and `bower.db` is never opened by two daemons simultaneously (folded from `deferred-work.md` 1-2 entry "Singleton enforcement", Epic 2 retro Next Steps #3)

### Story 3.2: Daemon lifecycle CLI

As a tool builder,
I want CLI commands to start, stop, and inspect the bowerbird daemon independently of my Claude Code hook configuration,
So that I can restart the daemon after a crash or manually test it without reinstalling the hook.

**Acceptance Criteria:**

**Given** the daemon is not running
**When** I run `bowerbird start`
**Then** the daemon starts in the background, `~/.bowerbird/ingest.sock` appears, and `GET /healthz` returns 200 within 2 seconds (NFR3)

**Given** the daemon is running
**When** I run `bowerbird stop`
**Then** the daemon receives SIGTERM, executes its graceful shutdown sequence (close frames, DB flush), and exits 0

**Given** the daemon is running
**When** I run `bowerbird status`
**Then** the output includes the daemon version, process uptime, and a liveness indicator; if the daemon is not running, the output clearly states it is stopped

**Given** the daemon crashes unexpectedly
**When** I run `bowerbird start` after freeing the cause (e.g., disk space)
**Then** the daemon starts cleanly, applies any pending WAL checkpoint, and `GET /readyz` returns 200

**Given** `bowerbird install` is run
**When** the installation completes
**Then** the daemon starts automatically as part of the install flow (daemon auto-start on install)

**Given** the daemon is running with N active WebSocket subscribers
**When** I run `bowerbird status` or query `GET /status`
**Then** the output includes `connected_ws_clients: N` reflecting current WS subscriber count, sourced from the existing `AppState::ws_semaphore` permit accounting (Epic 2 retro action item AI-1)

**Given** Story 3.2 ships
**When** the code lands
**Then** `protocol::rest::DaemonStatus` gains a `connected_ws_clients: u32` field, `daemon::api::status::get` populates it, the `"reserved for Epic 2 and intentionally omitted"` comment in `crates/daemon/src/api/status.rs` is removed, and the corresponding entry in `docs/bmad/implementation-artifacts/deferred-work.md` (Story 1.7 section, `/status.connected_ws_clients` line) is struck through with a backlink to this story

### Story 3.3: Bearer token auth with keychain storage

As a tool builder,
I want bowerbird's API to be protected by a secure bearer token that is stored in my system keychain and retrievable via CLI,
So that tools I build can authenticate without storing credentials in plaintext, and unauthorized processes on the same host cannot access my agent activity data.

**Acceptance Criteria:**

**Given** the daemon starts for the first time
**When** no existing token is found in the keychain
**Then** a UUID4 bearer token is generated, stored in the system keychain (macOS Keychain / Linux Secret Service), and the daemon uses it for all authenticated requests in this and future runs

**Given** the keychain is unavailable (e.g., headless CI environment)
**When** the daemon starts and resolves the token
**Then** it falls back in order: (1) `BOWERBIRD_TOKEN` environment variable, (2) `~/.bowerbird/config.toml` token field; the active fallback path is logged at info level

**Given** no token is resolvable via any fallback path (keychain unavailable, no env var, no config file)
**When** the daemon attempts to start
**Then** it exits non-zero with a human-readable error to stderr (NFR13)

**Given** the daemon is running with a valid token
**When** I run `bowerbird auth token`
**Then** the current bearer token is printed to stdout so I can copy it into tool configuration or HTTP client headers

**Given** a new token is needed (e.g., token rotation)
**When** I update the token in the keychain and restart the daemon
**Then** the daemon reads the new token at startup and uses it from that point forward; the token is never reloaded at runtime without a restart (NFR14)

### Story 3.4: Prebuilt binary distribution and release pipeline

As a tool builder,
I want to install bowerbird from a prebuilt binary without needing a Rust development environment,
So that I can start using bowerbird in under a minute regardless of my local toolchain setup.

**Acceptance Criteria:**

**Given** a tagged release on GitHub
**When** the release CI pipeline runs
**Then** prebuilt binaries are produced and attached to the GitHub Release for: macOS arm64, macOS x86\_64, and Linux x86\_64 (glibc)

**Given** a Linux user on a musl-based distribution
**When** they check the release notes
**Then** musl support is documented as deferred post-V1, with `cargo install` as the recommended alternative (NFR9)

**Given** a user with only the Rust stable toolchain installed
**When** they run `cargo install bowerbird`
**Then** the build succeeds using only stable Rust features — no nightly required (NFR10); `Cargo.lock` is committed and the build is reproducible

**Given** a user who downloads a prebuilt binary and runs `bowerbird install`
**When** the install completes
**Then** the hook entry in settings.json uses the PATH-relative binary name `bowerbird` (not an absolute install path), so the binary survives being updated via a new download to the same PATH location

**Given** the project documentation
**When** a new user reads about `bowerbird install`
**Then** they find a clear description of exactly what the command does to their system (files created, settings modified, daemon started) before they run it

**Given** the CI workflow at `.github/workflows/ci.yml`
**When** the daemon contract-test job runs
**Then** it invokes the test binary with `-- --test-threads=1`, because the contract suite shares process-wide state (real subprocesses, signal handlers, file system) and concurrent execution causes hangs (Epic 2 retro action item AI-3, observed in Stories 1.6 and 2.5)

**Given** `architecture.md` is the canonical "what does this system look like" reference
**When** a tool builder or new contributor reads it
**Then** it contains a "WebSocket subsystem" section listing the six runtime config knobs with their default values and roles: `ws_max_connections` (256, Semaphore cap), `ws_ping_interval` (30s), `ws_pong_timeout` (10s), `ws_broadcast_capacity` (1024, per-channel ring buffer), `shutdown_drain_timeout` (5s, graceful drain budget), `ws_broadcast_coalesce_window` (1s, Dropped-frame coalescing), so the protocol-changelog is no longer the only consolidated reference for these values (Epic 2 retro action item AI-2)

---

## Epic 4: Developer experience, replay, and protocol stability

Tool builders can learn bowerbird via comprehensive docs and reference examples, experiment with a fake event stream without a live Claude Code session, and build against a stable protocol with a documented compatibility guarantee and CI-enforced changelog.

### Story 4.1: bowerbird replay and export commands

As a tool builder,
I want to replay a recorded event sequence through bowerbird's full pub/sub path and export real sessions to replay files,
So that I can develop and debug my tools against realistic event streams without needing a live Claude Code session.

**Acceptance Criteria:**

**Given** a JSONL file containing wire-format EventEnvelope records
**When** I run `bowerbird replay <file>`
**Then** each event is routed through the daemon's broadcast channels exactly as if it arrived via the ingest socket — subscribed WebSocket clients receive the frames in order

**Given** a live session in the daemon's SQLite event log
**When** I run `bowerbird export <session-id>`
**Then** a JSONL file is produced containing all events for that session in wire-format EventEnvelope format, suitable for use with `bowerbird replay`

**Given** the bowerbird binary distribution
**When** I run `bowerbird replay` with no file argument
**Then** the command uses bundled demo fixture data (embedded in the binary) so new users can run the Quickstart without capturing a real session first

**Given** a replay file with events spanning two sessions
**When** `bowerbird replay` runs
**Then** tools subscribed to `state.session.*` receive state frames for both sessions, demonstrating multi-session fan-out without a live Claude Code instance

**Given** a replay file event whose timestamp is in the past
**When** `bowerbird replay` processes it
**Then** the daemon does not attempt to preserve original inter-event timing — events are replayed as fast as the pub/sub path allows (replay is for development, not performance reproduction)

### Story 4.2: Three reference example tools

As a tool builder,
I want complete, working TypeScript reference examples that demonstrate the core bowerbird patterns,
So that I can understand how to build my own tools by reading and running real code, not just documentation.

**Acceptance Criteria:**

**Given** `examples/multi-session-router/`
**When** I run it against `bowerbird replay` with the bundled fixture
**Then** it subscribes to `state.session.*`, correctly routes state frames to per-session state objects, and handles a new session appearing mid-subscription — demonstrating the core fan-out pattern

**Given** `examples/event-log-viewer/`
**When** I run it against `bowerbird replay` with the bundled fixture
**Then** it reads `events.*`, fetches initial history via `GET /sessions/:id/events?since=0`, and renders a tool-call history list — demonstrating cursor-based pagination via REST

**Given** `examples/reconnect-recovery/`
**When** I run it against a daemon and the WebSocket is intentionally disrupted
**Then** it fetches a snapshot on connect, detects a `dropped` frame and re-fetches via REST, and resumes correctly — demonstrating the resilience pattern every long-running tool needs

**Given** all three examples in `examples/`
**When** CI runs `cargo build --examples` (or equivalent for Node examples) on every PR
**Then** all three examples compile and pass their smoke tests against a live daemon with bundled fixture data

**Given** a cookbook entry referencing code from a reference example
**When** I update a function in the example
**Then** the cookbook entry automatically reflects the change via include anchors — no manual copy-paste required (cookbook-example coupling invariant)

### Story 4.3: Documentation suite

As a tool builder,
I want comprehensive documentation that takes me from zero to a working tool without needing to contact the maintainer,
So that bowerbird is genuinely self-serve for the developer audience it targets.

**Acceptance Criteria:**

**Given** the Quickstart document
**When** I follow it step by step on a machine with bowerbird installed
**Then** I can run a reference example against `bowerbird replay` with bundled fixture data and see live output — no Claude Code session or live agent required

**Given** `docs/presenter-authoring.md`
**When** I read it
**Then** it covers: establishing a WebSocket connection, sending a subscribe message, handling `state`/`event`/`dropped`/`close` frames, and fetching a REST snapshot on reconnect — with TypeScript code examples

**Given** `docs/protocol.md`
**When** a tool builder reads it
**Then** it contains: all REST endpoints with auth requirements and response shapes, all WebSocket frame types with their JSON schemas, topic syntax (wildcards, source-scoped, session-scoped), and the ingest socket contract

**Given** `docs/cookbook/` at launch
**When** I browse it
**Then** it contains at least three entries, each paired with a reference example, each following the format: Problem → Approach → Code (inlined via anchor, not copy-pasted) → Variants

**Given** `docs/no-list.md`
**When** an epic author or contributor reads it
**Then** it explicitly names the scope cuts: no Windows support, no distro packaging, no HITL backflow, no tool blocking, no personas, no LAN/multi-host — so contributors don't propose features that are deliberate non-targets

### Story 4.4: Protocol compatibility guarantee and contract test suite

As a tool builder,
I want a documented and CI-enforced guarantee that my tools will continue working on future bowerbird releases, backed by a complete contract test suite,
So that I can build on bowerbird with confidence rather than checking every daemon update for breaking changes.

**Acceptance Criteria:**

**Given** `docs/protocol-changelog.md`
**When** any file under `crates/protocol/src/` is changed in a PR
**Then** CI enforces that a corresponding entry exists in protocol-changelog.md with a structured header (`type: schema | behavioral | security`); the PR fails without it (FR39 CI gate)

**Given** the v1.x compatibility guarantee
**When** a tool built against v1.0 is run against any v1.x daemon
**Then** it continues to function — no REST endpoint removed, no WebSocket frame type removed, no required field added to outbound types (FR36 additive-only contract)

**Given** all 10 required contract tests
**When** `cargo test --workspace` runs on CI
**Then** all 10 pass: (1) WS dropped-frame behavior, (2) PRAGMA invariants on every connection, (3) connection factory lint enforcement, (4) state+event INSERT atomicity, (5) graceful shutdown, (6) cursor-gap detection, (7) atomic settings.json install, (8) hook unreliability tolerance, (9) outbound envelope additive-compat, (10) (source, session\_id) collision safety

**Given** the shim hot-path bench (`shim/benches/hot_path.rs`)
**When** CI runs it
**Then** it compares p99 against the committed per-platform baseline file and fails if regression exceeds 15%; the baseline file is updated only via a deliberate PR with reviewer sign-off — not auto-rolled

**Given** a future daemon version vN+1 is started against a data directory written by daemon vN
**When** the daemon completes startup
**Then** no data is lost, existing projection rows are intact, and additive-compat holds for all API responses (cross-version protocol upgrade contract test)

**Given** the wire-surface enums in `crates/protocol/src/` (`ServerMessage`, `ClientMessage`, `EventKind`, `Reaction`, and any `Error` variants serialized on the wire)
**When** a v1.x daemon emits a variant a v1.0 tool does not know about
**Then** the tool's `Deserialize` either decodes via `#[serde(other)] Unknown` (or equivalent catch-all) or the protocol crate carries a written justification why that enum cannot accept the catch-all; `ServerMessage::Unknown` (added in Epic 2 Story 2.1) is the existing template (Epic 2 retro action item AI-4)

**Given** the hook-to-presenter p99 ≤100ms budget (NFR2)
**When** a Criterion benchmark runs in CI
**Then** it exercises at least four shapes (solo presenter baseline, 3-presenter fanout, burst-shape with events clumped at tool-call boundaries, and steady-state at modest event rate), comparing p99 against a committed per-platform baseline file and failing the build on regression past the threshold described in `project-context.md` (Epic 2 retro action item AI-5; closes the Story 2.2 deferred-work entry)

**Given** the protocol documentation (`docs/protocol.md` and the `docs/protocol-changelog.md` rationale entries)
**When** a tool builder reads about the ingest socket
**Then** the NDJ framing on the shim-to-daemon path is documented as a deliberate choice for shim-dependency minimalism (the shim is `std`-only with no async runtime), not as a latency optimization; this narration replaces any retconned perf-driven framing (Epic 1 retro Agreement A3 carryover, Epic 2 retro action item AI-6)

---

## Epic 5: V1 Release Readiness

The maintainer installs bowerbird on their main machine, builds a first-party presenter in a sibling repository, runs it daily against live Claude Code sessions, and harvests the friction. The planned stories below convert the CI gates from aspirational to load-bearing, exercise the release pipeline end-to-end against a real tag, polish the install UX, consolidate the cookbook into single-directory entries that colocate prose with runnable code (pocketflow pattern), and rewrite the README + quickstart for a first-time reader. Closing event: v0.1.0 tagged on GitHub Releases.

**FRs covered:** primarily hardening of FRs already covered by Epics 1–4. No new FRs introduced.
**NFRs covered:** strengthens NFR1, NFR2 (bench gates load-bearing); NFR19 (protocol stability, cross-version upgrade test load-bearing).

### Story 5.1: First-party presenter tool (sibling repository)

As the bowerbird maintainer,
I want a real presenter tool I can use daily against live Claude Code sessions,
So that dogfooding has a useful surface to observe — not just JSON in a terminal — and the friction I find informs the rest of Epic 5.

**Acceptance Criteria:**

**Given** a sibling repository (naming decision finalized during story creation; candidate names include `bowerbird-statusbar`, `bowerbird-deck`)
**When** I run the presenter against a locally running `bowerbird` daemon connected to a live Claude Code session
**Then** the presenter surfaces session state (idle / working / waiting-on-input) and recent tool-use activity in a form the maintainer finds useful for daily work — exact UI form (terminal TUI, menu bar, web UI, etc.) decided during story creation

**Given** the presenter is installed on the maintainer's main machine
**When** the maintainer codes with Claude Code for at least 5 working days
**Then** the presenter is the maintainer's actual signal source for "is Claude doing something" — used in preference to alt-tabbing to the terminal

**Given** the presenter is in a sibling repository, not in `crates/` or `examples/`
**When** a reader of the bowerbird repository looks at architecture.md §Frontend Architecture
**Then** they find a backlink to the presenter's repository, with a one-sentence justification that interpretation does not belong in the substrate

**Given** the presenter consumes the WebSocket and REST API
**When** any aspect of consumption is awkward (auth flow, snapshot-on-connect, dropped-frame handling, reconnect behavior)
**Then** the awkwardness is captured as a `5.X-hotfix-<topic>` story or as a deferred-work entry against bowerbird, *not* worked around silently in the presenter

**Given** the presenter codebase
**When** the maintainer reaches a "this is the V1 presenter" milestone (subjective)
**Then** a README in the sibling repo names: required bowerbird version, how to install, how to run, and the one cookbook pattern from `docs/cookbook/` the presenter most directly demonstrates

### Story 5.2: Session state projection correctness

As a presenter author,
I want session-state broadcasts to fire only on actual `current_state` transitions, and Working signals to cover the agent's full active period (user prompt submission through tool completion — not just PreToolUse moments),
So that ribbon UIs render only on meaningful state changes — no flap between back-to-back tool calls, no false Idle gap during the agent's between-tool thinking, no false Idle gap while the agent composes its first tool call after a user prompt.

Closes the dogfooding finding in `sprint-change-proposal-2026-05-27.md`. Resequenced from 5.7 → 5.2 by `sprint-change-proposal-2026-05-27-epic-5-resequencing.md` (dogfooding-first ordering).

**Acceptance Criteria:**

**Given** a session in `Working` and an incoming `PostToolUse` event
**When** the projection writes the new state
**Then** `last_event_kind` and `last_event_at_ms` are updated AND `current_state` remains `Working` (not `Idle`); subscribers to `state.session.*` and `state.session.<id>` receive NO `state` envelope for this event; subscribers to `events.*` still receive the `event` envelope

**Given** N back-to-back `PreToolUse`/`PostToolUse` pairs for one session
**When** the events are ingested
**Then** subscribers to `state.session.*` receive exactly one `state` envelope (the first `PreToolUse`'s `Idle`→`Working`); subscribers to `events.*` receive 2N event envelopes; `last_event_at_ms` still updates on every `PostToolUse`

**Given** Claude Code running with bowerbird installed
**When** the user submits a prompt
**Then** the `UserPromptSubmit` hook fires; the daemon ingests it; the `EventEnvelope` has `kind=UserPromptSubmit`; `current_state` transitions to `Working` (or remains `Working`); `last_event_at_ms` updates

**Given** a fresh `bowerbird install` against a Claude Code settings file with no prior hooks
**When** installation completes
**Then** `~/.claude/settings.json` registers five hooks (`PreToolUse`, `PostToolUse`, `Stop`, `Notification`, `UserPromptSubmit`); `bowerbird uninstall` removes all five; an existing install that pre-dates Story 5.2 surfaces "re-run `bowerbird install` to subscribe UserPromptSubmit" when old-style hooks are detected

**Given** a v1.0 presenter compiled against the pre-Story-5.2 protocol enum
**When** it receives an event with `kind: "UserPromptSubmit"` from a Story-5.2+ daemon
**Then** serde decodes it as `EventKind::Unknown` (Story 4.4 catch-all); no crash, no panic, no protocol-violation close frame

**Given** `crates/daemon/src/projection/state.rs` after Story 5.2
**When** `transition()` is called with each `EventKind` variant
**Then** `PostToolUse` preserves `prev.current_state`; `UserPromptSubmit` returns `Working`; `PreToolUse` returns `Working`; `Stop` returns `Idle`; `Notification` returns `WaitingInput`; `RecordingStarted`/`RecordingEnded`/`Unknown` preserve prev (unchanged); the 5-minute `STALE_WORKING_MS` fallback is unchanged and now backstops both missing-`Stop` and missing-`PostToolUse`

**Given** the protocol surface
**When** Story 5.2 lands
**Then** `crates/protocol/src/event.rs` `EventKind` gains `UserPromptSubmit`; `crates/adapter-claude/src/normalize.rs` maps the string; `HOOK_KINDS` in `crates/adapter-claude/src/install.rs` adds it

**Given** the doc and contract-test surface
**When** Story 5.2 lands
**Then** `docs/protocol.md:280` rewrites the broadcast emission rule to transitions-only; `docs/protocol.md:334` and `:338` add `UserPromptSubmit`; `docs/protocol-changelog.md` gains two entries (behavioral: tighten state broadcast to transitions-only; schema: `UserPromptSubmit` `EventKind`); `crates/protocol/tests/contract_protocol.rs` and `crates/daemon/tests/contract_daemon.rs` are updated for both rules

**Given** the planning artifacts
**When** Story 5.2 lands
**Then** `prd.md:206` tightens "goes green when Claude finishes" to "goes green when Claude finishes the turn"; `architecture.md:50` and `:1026` amend "no stuck state on missing PostToolUse" to "no stuck state on missing PostToolUse or Stop"

### Story 5.3: Daemon-observed session liveness + typed-notification WaitingInput

As a presenter author,
I want the substrate to observe process death and emit a mechanical `SessionEnded` event, and I want `WaitingInput` to reflect Claude's typed `notification_type` field rather than collapse every `Notification` into one bucket,
So that my ribbon UI can render an accurate per-session state without doing its own liveness syscalls, without doing its own payload regex on `notification_type`, and without breaking when the presenter and daemon are on different machines.

**Closes two Story 5.1 dogfooding findings** against `bowerbird-deck`: (1) ~48 sessions stuck at `WaitingInput`, none actually waiting — terminals closed without firing `Stop`, frozen on the last `Notification`; (2) no mechanical signal for "session process is gone" — every presenter would have to call `kill(pid, 0)` itself.

**Operationalizes ADR 0004** (`docs/decisions/0004-daemon-observed-session-liveness.md`). **Refines Story 5.2's** `PostToolUse → preserve prior` rule to `PostToolUse → Working unconditionally`. Resequenced 5.8 → 5.3 by `sprint-change-proposal-2026-05-27-epic-5-resequencing.md`; design amended in `sprint-change-proposal-2026-05-28-daemon-observed-liveness.md`.

**Acceptance Criteria:**

**Given** a Claude Code hook fires and the shim runs
**When** the shim sends the payload to the daemon's ingest socket
**Then** the payload JSON includes a `bowerbird_ppid` field whose value is the integer returned by `libc::getppid()` at shim-invocation time; the field is injected by the shim, not present in the upstream Claude Code hook payload; the shim hot-path p99 ≤5ms budget (Story 1.5) is preserved under the shim-bench-gate.

**Given** the `adapter-claude` normalize path receives a payload with `bowerbird_ppid` set
**When** normalize constructs the `EventEnvelope`
**Then** `EventEnvelope.pid` is `Some(<that value>)`; a payload missing `bowerbird_ppid` or carrying a non-integer value yields `EventEnvelope.pid = None` and is normalized successfully (not a failure mode).

**Given** the `adapter-claude` normalize path receives a payload with `hook_kind = Notification` and a `notification_type` field
**When** normalize constructs the `EventEnvelope`
**Then** `EventEnvelope.notification_type` is `Some(NotificationType::X)` for known values (`permission_prompt`, `idle_prompt`, `auth_success`, `elicitation_dialog`, `elicitation_response`, `elicitation_complete`); an unrecognized value yields `Some(NotificationType::Unknown)`; a missing field yields `None`; the event is normalized successfully in all three cases.

**Given** an `EventEnvelope` with `pid: Some(N)` reaches `projection::session::write`
**When** the projection writes inside its single transaction
**Then** the `events` row stores `pid = N`; the upserted `session_projections` row's deserialized `SessionState` carries `last_pid: Some(N)`; the `BroadcastEnvelope::State` published after commit (if gated through per Story 5.2) carries the same `last_pid`; the `BroadcastEnvelope::Event` likewise carries `pid: Some(N)`.

**Given** a follow-up `EventEnvelope` for the same `(source, session_id)` with `pid: None`
**When** the projection writes
**Then** `SessionState.last_pid` retains the prior `Some(N)` (carry-forward semantics); the `events` row stores `pid = NULL` for that specific event.

**Given** a follow-up `EventEnvelope` for the same `(source, session_id)` with `pid: Some(M)` where `M != N`
**When** the projection writes
**Then** `SessionState.last_pid` becomes `Some(M)` (overwrite-on-Some semantics).

**Given** an `EventEnvelope` for `hook_kind = Notification` with `notification_type` in `{PermissionPrompt, IdlePrompt, ElicitationDialog}`
**When** the projection's `transition` function runs
**Then** the resulting `current_state` is `WaitingInput`; the prior state is irrelevant.
> **Superseded for `IdlePrompt` by Story 5.6 / ADR 0005 (2026-05-29):** `IdlePrompt` got its own rule (code-review D3): → `Idle`, except a prior `WaitingInput` is preserved — it does NOT join the generic preserve-prior bucket below. As of 5.6, `PermissionPrompt` and `ElicitationDialog` are the only types that transition a session into `WaitingInput`. This Story 5.3 AC is preserved as-shipped history; see the Story 5.6 section.

**Given** an `EventEnvelope` for `hook_kind = Notification` with `notification_type` in `{AuthSuccess, ElicitationResponse, ElicitationComplete}` OR `notification_type = Unknown` OR `notification_type = None`
**When** the projection's `transition` function runs
**Then** the resulting `current_state` preserves the prior state (no transition).
> **Refined by Story 5.6 / ADR 0005:** `IdlePrompt` is NOT in this preserve-prior set — it has its own rule (→ `Idle`, except a prior `WaitingInput` is preserved; code-review D3). And for the types listed here, a prior `Ended` now resurrects to `Idle` (a notification hook proves the process is alive; code-review D1) rather than being preserved.

**Given** an `EventEnvelope` for `hook_kind = PostToolUse`
**When** the projection's `transition` function runs
**Then** the resulting `current_state` is `Working` unconditionally (refines Story 5.2's "preserve prior" rule — flagged as a `type: behavioral` changelog entry); `last_event_kind` and `last_event_at_ms` update normally.

**Given** the daemon completes `run_migrations` and `rebuild_missing_projections` at startup
**When** the daemon proceeds to accept connections
**Then** one synchronous iteration of the liveness probe has run before the WS server binds — for each `session_projections` row where `last_pid IS NULL` OR `libc::kill(last_pid as i32, 0) != 0` (errno = ESRCH), a `SessionEnded` event is written via the normal `projection::session::write` path; the projection row transitions to `current_state = Ended`; the events row carries `source = <row's source>`, `session_id = <row's session_id>`, `kind = SessionEnded`, `payload = {"reason": "no_pid_at_upgrade"|"pid_dead", "pid": <last_pid or null>, "observed_at_ms": <epoch_ms>}`.

**Given** the daemon is running steady-state with the WS server up
**When** the periodic liveness probe task wakes (5-second cadence via `tokio::time::interval` with `MissedTickBehavior::Skip`)
**Then** the same per-row logic from the startup iteration runs; `SessionEnded` events are written and broadcast on `events.*`; resulting state transitions are broadcast on `state.session.*`; an in-flight probe iteration that takes longer than the tick interval does NOT queue (next tick skipped).

**Given** a `session_projections` row in `current_state = Ended`
**When** a subsequent hook `EventEnvelope` arrives for the same `(source, session_id)` (e.g. from `claude --resume`)
**Then** `transition` runs normally: `UserPromptSubmit`/`PreToolUse`/`PostToolUse → Working`; `Stop → Idle`; `Notification` with input-required `notification_type` → `WaitingInput`; `last_pid` updates from the new envelope's PID via overwrite-on-Some semantics; the row exits `Ended`.

**Given** a daemon restart with a non-empty `events` table that includes `SessionEnded` events
**When** `rebuild_missing_projections` runs
**Then** for each rebuilt session the reconstructed `SessionState.last_pid` AND `current_state` match what live ingest would have produced from the same event sequence (Story 1.6 AC #5 "storage layer is a pure function of the event sequence" is preserved); `SessionEnded` events in the log drive transitions to `Ended` during rebuild exactly as they did during live ingest.

**Given** `GET /sessions` and `GET /sessions/{id}`
**When** the daemon serializes the response
**Then** `SessionListItem` and `SessionDetail.state` each carry `last_pid` as a number-or-null field; `SessionCurrentState` includes the new `Ended` variant in `current_state` for rows where the liveness probe observed death; the read-time stale-`Working` → `Idle` fallback (Story 1.6 `current_state_for_read`) does NOT alter `last_pid` and does NOT interfere with `Ended` (which passes through unchanged); the sentinel session row (`source = "__daemon__"`) continues to be filtered out.

**Given** a WS subscriber to `state.session.*` receives a `StateFrame`
**When** the frame is decoded
**Then** `frame.state.last_pid` carries the same value as the REST `SessionDetail.state.last_pid` would for the same session at the same moment; snapshot-on-subscribe frames (Story 2.3) likewise carry `last_pid`; transitions to `Ended` (driven by the liveness probe) broadcast a `StateFrame` per the Story 5.2 transitions-only policy.

**Given** a WS subscriber to `events.*` receives an `EventFrame`
**When** the frame is decoded for a `SessionEnded` event
**Then** the frame carries `kind = "SessionEnded"`, the real `source` and `session_id` of the session that ended, and a payload object with `reason`, `pid` (number or null), and `observed_at_ms`.

**Given** a v1.0 presenter compiled against the pre-Story-5.3 protocol type
**When** it deserializes a `SessionState` frame, a `StateFrame`, or an `EventFrame` from a Story-5.3+ daemon
**Then** serde silently ignores the `last_pid` field; the `Ended` `SessionCurrentState` variant decodes to `Unknown` via the Story 4.4 `#[serde(other)]` catch-all; the `SessionEnded` `EventKind` decodes to `Unknown` via the same catch-all; no decode error, no crash, no protocol-violation close frame; additive-compat contract tests in `contract_protocol.rs` exercise each path.

**Given** the SQLite `events` schema before Story 5.3 (v1)
**When** the daemon starts against an existing v1 database
**Then** migration v2 runs `ALTER TABLE events ADD COLUMN pid INTEGER`; existing rows have `pid = NULL`; the migration is idempotent (re-running `to_latest` is a no-op per Story 5.4's migration-idempotency contract test).

**Given** the protocol surface
**When** Story 5.3 lands
**Then** `crates/protocol/src/state.rs` `SessionState` gains `last_pid: Option<u32>` AND `SessionCurrentState` gains the `Ended` variant; `crates/protocol/src/event.rs` `EventEnvelope` gains `pid: Option<u32>` (internal) AND `notification_type: Option<NotificationType>` (internal), `EventKind` gains the `SessionEnded` variant, a new `NotificationType` enum is added with six known variants + `Unknown`, and stored `Event` gains `pid: Option<u32>`; `crates/shim/Cargo.toml` adds the workspace `libc` dep; `crates/shim/src/main.rs` injects `bowerbird_ppid`; `crates/adapter-claude/src/normalize.rs` extracts both `bowerbird_ppid` and `notification_type`; a new module `crates/daemon/src/projection/liveness.rs` houses the probe loop.

### Story 5.4: Install UX polish and middleware closure

As a first-time user,
I want `bowerbird install` to leave my system in a fully working state without manual file shuffling,
And as a release manager, I want the missing-on-purpose middleware (`CatchPanicLayer`) wired before V1 exposes the daemon to a wider audience.

Folds in five deferred-work entries; no new design surface.

**Acceptance Criteria:**

**Given** a user runs `bowerbird install` from a freshly extracted prebuilt tarball
**When** the install completes
**Then** `~/.bowerbird/adapters/claude/tool-reactions.toml` is present, seeded from the bundled file (Epic 3 retro AI-4 / Story 3.4 deferred-work entry "bowerbird install auto-copies tool-reactions.toml"); if the file already exists with user modifications, it is left untouched and a warning is logged

**Given** an HTTP handler panics inside the daemon
**When** the panic happens
**Then** `CatchPanicLayer` (Story 2.1 deferred-work entry) returns a structured `500` JSON response and the daemon continues serving other requests, rather than the panic bubbling to axum's default close-the-connection path

**Given** the TypeScript reference examples under `examples/`
**When** CI runs against a PR
**Then** a new `Typecheck examples` job runs `tsc --noEmit` against each example (Story 4.2 deferred-work entry "Typecheck CI lane for examples"); type errors fail the build

**Given** a populated SQLite database with prior-version schema
**When** the daemon starts and `run_migrations` runs against it
**Then** a migration-idempotency contract test verifies a second `run_migrations` call against the now-migrated DB is a no-op (Story 1.2 deferred-work entry "Migration idempotency on a populated DB is untested")

**Given** a request to `GET /sessions/{id}/events` for a session_id that has never existed
**When** the daemon processes it
**Then** the response is `404 Not Found` rather than `200 {events: [], cursor: None, ...}` (Story 4.1 deferred-work entry "/sessions/{id}/events 404 for unknown sessions"); a `type: behavioral` entry lands in `docs/protocol-changelog.md` documenting the alignment; `bowerbird export` drops its pre-check round trip

### Story 5.5: Bench gates converted to load-bearing

As a release manager,
I want every committed CI bench gate to fail loudly when a real regression lands,
So that the bench infrastructure is producing signal — not just running.

Closes Epic 4 retro AI-1, AI-2, AI-3 (per `epic-4-retro-2026-05-25.md` Action Items table). Resequenced from 5.2 → 5.5 by `sprint-change-proposal-2026-05-27-epic-5-resequencing.md` (dogfooding-first ordering: bench-gate work doesn't unblock daily dogfooding).

**Acceptance Criteria:**

**Given** `crates/daemon/benches/baselines/macos.json` and `linux.json` currently contain placeholder zero values
**When** Story 5.5 lands
**Then** both files contain non-zero p99 values per shape (solo, fanout3, burst, steady) sourced from the most recent green CI run on `main` (or the Story 5.5 PR's CI run if it's green); the bench gate `daemon-bench-gate` exercises the regression check without auto-skipping any shape

**Given** the daemon-bench gate has never been exercised in failure mode
**When** Story 5.5 lands
**Then** the Dev Agent Record documents two chaos-injection sanity PRs (one macOS, one Linux) that injected `tokio::time::sleep(50ms)` between `tx.commit()` and `broadcaster.publish` in `crates/daemon/src/projection/session.rs::write`, verified CI's daemon-bench-gate failed on the burst-shape p99 regression, and were reverted before merge

**Given** the shim hot-path bench gate has never been exercised in failure mode (Story 4.4 Task 4.3 deferred)
**When** Story 5.5 lands
**Then** the Dev Agent Record documents two chaos-injection sanity PRs (one per platform) that injected `std::thread::sleep(Duration::from_millis(2))` into the shim's hot path, verified CI's shim-bench-gate failed, and were reverted before merge

**Given** the work is paperwork-flavored (no production code changes after the chaos PRs are reverted)
**When** Story 5.5 closes
**Then** the deferred-work entries naming AI-1/AI-2/AI-3 are struck through with a backlink to this story's merge commit

### Story 5.6: `idle_prompt` reclassified as transient (not input-required)

As the bowerbird maintainer dogfooding `bowerbird-deck`,
I want `idle_prompt` notifications to stop forcing a session into `WaitingInput`,
So that the deck's `WaitingInput` column contains only sessions genuinely blocked on me (permission / elicitation), and finished-but-idle sessions read `Idle`.

Inserted by `sprint-change-proposal-2026-05-29-idle-prompt-reclassification.md` (Accepted 2026-05-29) after bench-gates (5.5). Operationalizes ADR 0005 (`docs/decisions/0005-idle-prompt-transient-not-input-required.md`), which amends ADR 0004 §3. Surfaced during 2026-05-29 live dogfooding: 13 of 15 *live* sessions rendered `WaitingInput` (aged 5m–1h51m), none blocked — `idle_prompt` (fired ~60s after `Stop → Idle`) was classified as input-required, making the idle nudge a one-way ratchet. Refines Story 5.3's notification-type buckets: one type moves from input-required to transient. No wire-format change, no migration, no new field.

**Acceptance Criteria:**

**Given** the `EventKind::Notification` arm of `crates/daemon/src/projection/state.rs::transition` (Story 5.3)
**When** Story 5.6 lands
**Then** `Some(NotificationType::IdlePrompt)` gets its own rule (code-review D3): `idle_prompt → Idle`, EXCEPT a prior `WaitingInput` is preserved; `PermissionPrompt` and `ElicitationDialog` remain the only notification types that *transition a session into* `WaitingInput`; `IdlePrompt` resolves prior `Working`/`Idle`/`Ended` and no-prior to `Idle` (covering a dropped `Stop`) and a prior `WaitingInput` to `WaitingInput`; the truly-transient types (`AuthSuccess`/`ElicitationResponse`/`ElicitationComplete`/`Unknown`/`None`) preserve prior except a prior `Ended` → `Idle` (code-review D1); `last_event_kind`/`last_event_at_ms` still update

**Given** the `state.rs` test module
**When** Story 5.6 lands
**Then** `transition_notification_input_required_yields_waiting_input` no longer iterates `IdlePrompt`; `transition_notification_transient_preserves_prior` does NOT include `IdlePrompt`; tests cover `IdlePrompt` + prior `Idle`/`Working`/no-prior → `Idle` and `IdlePrompt` + prior `WaitingInput` → `WaitingInput` (a pending block is not clobbered); `transition_from_ended_preserve_prior_notification_yields_idle` covers prior `Ended` → `Idle`

**Given** ADR 0004 §3's notification-type table
**When** Story 5.6 lands
**Then** the `idle_prompt` row changes from `→ WaitingInput` to `→ Idle, except a prior WaitingInput is preserved` (code-review D3), and a top-of-file Status note records the ADR 0005 amendment dated 2026-05-29; 0004's liveness probe, `Ended` state, `SessionEnded` event, and `PostToolUse → Working` refinement are unchanged

**Given** `docs/protocol.md` (`SessionCurrentState`, the `notification_type` extraction prose, and the `Notification` hook-kind table row)
**When** Story 5.6 lands
**Then** `idle_prompt` is documented as resolving to `Idle` (except a prior `WaitingInput` is preserved), the `WaitingInput` definition is narrowed to "blocked on user input with work queued (`permission_prompt`/`elicitation_dialog`, incl. `AskUserQuestion`)", and it is noted that `idle_prompt` does not *transition a session into* `WaitingInput` (a session already in `WaitingInput` stays there — the nudge neither creates nor clears a block)

**Given** `docs/protocol-changelog.md` (the changelog gate fires only on `crates/protocol/src/*.rs` changes, which this story does NOT touch)
**When** Story 5.6 lands
**Then** exactly one `type: behavioral` entry is added deliberately under `v1.0 → v1.1` stating `idle_prompt` no longer *transitions a session into* `WaitingInput` (it preserves prior state), explicitly superseding the Story 5.3 `Notification → WaitingInput` entry's `idle_prompt` classification, `(Resolves: 5.6)`

**Given** the change is strictly a narrowing of `WaitingInput`
**When** Story 5.6 lands
**Then** `crates/protocol/src/*.rs` is unmodified, `NotificationType` keeps all seven variants, no SQLite migration is added, and old presenters decoding with `#[serde(other)]` are unaffected (state set unchanged; only `WaitingInput` frequency drops); `cargo test --workspace -- --test-threads=1`, `cargo fmt --check`, and `cargo clippy --all-targets --workspace -- -D warnings` pass

### Story 5.7: Session working directory and start time on the wire

As a presenter author,
I want each session's working directory (`cwd`) and start time (`started_at`) carried as mechanical facts on the wire,
So that I can group, filter, and label sessions by repo/directory and render session age from the snapshot, without reading transcript content or inventing a persona model.

Inserted by `sprint-change-proposal-2026-06-01-dogfood-triage.md` (§4.1, Finding 3/4) after Story 5.6. First of four dogfood-triage stories (5.7–5.10); all four gate the v0.1.0 tag (Story 5.14). Operationalizes **ADR 0006** (`docs/decisions/0006-session-cwd-on-the-wire.md`). `cwd` mirrors the Story 5.3 `last_pid` additive-field pattern exactly — `Option<T>`, overwrite-on-Some carry-forward, schema migration v3, `#[serde(other)]`-safe — with one divergence: `cwd` rides the **native Claude Code hook payload** (no shim change; the adapter reads it in `normalize`). `started_at` (the deferred Story 5.3 item) is bundled in per proposal §6: daemon-derived, **set-once / keep-earliest** (the inverse of cwd), session-level only (no `Event` field, no migration).

**Acceptance Criteria:**

**Given** the `adapter-claude` normalize path receives a hook payload with a top-level `cwd` string
**When** normalize constructs the `EventEnvelope`
**Then** `EventEnvelope.cwd = Some(<that string>)`; a missing or non-string `cwd` yields `None` without failing normalization; `cwd` is extracted for ALL hook kinds (not kind-gated like `notification_type`)

**Given** an `EventEnvelope` with `cwd` reaches `projection::session::write`
**When** the projection writes in its single transaction
**Then** the `events.cwd` column, the upserted `SessionState.cwd`, the stored `Event.cwd`, and the post-commit `BroadcastEnvelope::Event`/`State` all carry the value; carry-forward / overwrite-on-Some semantics match `last_pid` (a follow-up `cwd: None` preserves prior; `cwd: Some(q)` overwrites)

**Given** the projection write for a `(source, session_id)` with no prior row, then later events
**When** the projection writes
**Then** `SessionState.started_at` is set once to the first event's timestamp and never overwritten (`prev.started_at.or(Some(now_ms))`); it is daemon-derived (no payload field, no adapter code, no `events` column, no migration), and reconstructs identically during `rebuild_missing_projections` (Story 1.6 AC #5 pure-function-of-event-log preserved)

**Given** the SQLite `events` schema (v2)
**When** the daemon starts against an existing DB
**Then** migration v3 runs `ALTER TABLE events ADD COLUMN cwd TEXT` (idempotent; pre-v3 rows `cwd = NULL`; `PRAGMA user_version` → 3); rebuild reads `cwd` from the typed column (not re-parsed from payload), carrying forward the last non-NULL value

**Given** `GET /sessions`, `GET /sessions/{id}`, WS `StateFrame` (incl. snapshot-on-subscribe), and `EventFrame`
**When** serialized
**Then** `SessionListItem` and `SessionDetail.state` carry `cwd` (string-or-null) AND `started_at` (number-or-null epoch ms); `Event`/`EventFrame.event` carry `cwd` (string-or-null, NOT `started_at`); the read-time stale-`Working` → `Idle` fallback does not alter either field

**Given** a v1.0 / pre-5.7 presenter compiled against the older protocol
**When** it deserializes a Story-5.7+ `SessionState` / `StateFrame` / `Event` / `EventFrame` / `SessionListItem`
**Then** serde silently ignores `cwd` / `started_at` (asymmetric permissive-outbound policy); no decode error; pre-5.7 projection blobs lacking the fields deserialize to `None`; additive-compat tests in `contract_protocol.rs` cover both fields as the Story 5.3 tests do for `last_pid` / `pid`

**Given** the protocol + docs surface
**When** Story 5.7 lands
**Then** `crates/protocol/src/{state,event,rest}.rs` gain the fields (`cwd` on `SessionState`/`EventEnvelope`/`Event`/`SessionListItem`; `started_at` on `SessionState`/`SessionListItem` only); NO shim change; NO state-machine change; `docs/protocol.md` + `docs/protocol-changelog.md` (one `type: schema` entry, `Resolves: 5.7`) + `docs/presenter-authoring.md` (group/label-by-cwd note, `cwd != repo` caveat, session-age-from-started_at) + `project-context.md` (ADR 0006 `Affects context.md sections`) updated; `cargo test --workspace -- --test-threads=1`, `cargo fmt --check`, `cargo clippy --all-targets --workspace -- -D warnings` pass

### Story 5.8: Server-side session filter (+ ADR 0008)

As a presenter author,
I want `GET /sessions` to accept `?state=`/`?since=`/`?limit=` filters AND the WS snapshot-on-subscribe burst to be scopeable by an optional `states` filter,
So that a new presenter isn't blasted with the full `Ended` graveyard and can fetch (and be handed) only the sessions it cares about.

Inserted by `sprint-change-proposal-2026-06-01-dogfood-triage.md` (§4.2, Finding 5-filter); operationalized by **ADR 0008** (presenter-controlled filtering on both surfaces — the maintainer's choice to scope the snapshot by a presenter-supplied predicate modified the wire protocol, which §4.2's "no ADR" assumption did not anticipate). Gates v0.1.0 (Story 5.14). ACs (see `implementation-artifacts/5-8-server-side-session-filter.md` for the full set):

- **REST `GET /sessions`** gains optional `?state=<csv>` (case-insensitive `SessionCurrentState` tokens, filtered in Rust on the read-time `current_state`), `?since=<updated_at_ms>` (exclusive recency lower bound, SQL), and `?limit=<n>` (SQL row cap) — all default-unfiltered so pre-5.8 behavior is byte-identical; invalid values `400`. `limit` caps the pre-state-filter set, so `?state=`+`?limit=` may return fewer than `n` (documented). `?since=`/`?limit=` are a recency filter and a row cap, not true cursor pagination (no stable exact-N page — a real cursor stays deferred).
- **WS `ClientMessage::Subscribe` gains an optional `states: Vec<String>`** (`#[serde(default)]`) that scopes the snapshot-on-subscribe burst by the same read-time `current_state` predicate; empty/absent = unfiltered. An invalid token closes the connection (`bad message`/1008). Scopes ONLY the snapshot — the live stream is unchanged.
- **No schema migration** (`events`/`session_projections` unchanged; query/transport-shape only). `docs/protocol.md` + a `type: schema` `docs/protocol-changelog.md` entry + the ADR-0008 `project-context.md` `Affects context.md sections: Wire format, HTTP surface` same-PR touch. `?state=` filters in Rust (not SQL `json_extract`) so the filter matches the rendered `current_state`, never a divergent stored value.

Partially addresses the deferred-work "no pagination on `GET /sessions`" item — the filters + row cap kill the unbounded response, but true cursor pagination stays deferred (the sibling `/sessions/{id}/events` page-size limit also stays deferred — different endpoint). **Partially resolves `gt-3cnt`** (filter half; the retention sweep stays open on the bean, a no-list "no `gc`" deferral).

### Story 5.9: Daemon start-on-login supervision (stub — refine at create-story) + ADR 0007

As the bowerbird maintainer,
I want `bowerbird install` to register the daemon to start on login with crash-restart,
So that a reboot doesn't silently drop every event until I manually restart the daemon.

Inserted by `sprint-change-proposal-2026-06-01-dogfood-triage.md` (§4.3, Finding 1). Gates v0.1.0 (Story 5.14). **Stub** — ACs to be derived from proposal §4.3 at `bmad-create-story` time; **ADR 0007** (`docs/decisions/0007-daemon-start-on-login.md`) pending authoring (launchd-vs-lazy-spawn decision + shim-stays-thin rationale; `Affects context.md sections: Durability and chaos`). Scope: `bowerbird install` writes a `~/Library/LaunchAgents/<label>.plist` (start-on-login + `KeepAlive` crash-restart); `bowerbird uninstall` removes it (symmetry tested); macOS-only (matches the no-list Windows/Linux-packaging posture). The shim is explicitly **NOT** changed (no lazy-spawn — that would put a subprocess fork on the hot path, violating the shim discipline).

### Story 5.10: Shim names the cause on daemon-down (stub — refine at create-story)

As a Claude Code user,
I want the shim to print one human-readable line naming the cause when the daemon is down,
So that a daemon outage doesn't render as Claude Code's generic causeless no-stderr hook error on every call.

Inserted by `sprint-change-proposal-2026-06-01-dogfood-triage.md` (§4.4, Finding 2 — minor). Gates v0.1.0 (Story 5.14). **Stub** — ACs to be derived from proposal §4.4 at `bmad-create-story` time. Scope: `crates/shim/src/main.rs` writes one stderr line on the exit-1 path, e.g. `bowerbird: daemon not running, event dropped (see ~/.bowerbird/shim.log)`; keep `Error::Connect → exit 1` (NFR20 contract intact); the success path stays stderr-silent (no hot-path cost). Per-call coalescing / exit-0-vs-exit-1 reconsideration is **deferred** (proposal §6 — the shim is stateless per-invocation, so cross-call rate-limiting needs shared state).

### Story 5.11: Release pipeline end-to-end verification

As a release manager,
I want the GitHub Releases pipeline driven to a real (non-prerelease) tag, producing artifacts that install and run on a fresh machine,
So that v0.1.0 is the second release we cut — not the first.

Resequenced from 5.3 → 5.6 by `sprint-change-proposal-2026-05-27-epic-5-resequencing.md`, then → 5.7 by `sprint-change-proposal-2026-05-29-idle-prompt-reclassification.md` (idle-prompt story inserted at 5.6) (dogfooding-first ordering: release-pipeline verification doesn't unblock daily dogfooding).

**Acceptance Criteria:**

**Given** the release workflow at `.github/workflows/release.yml`
**When** a `v0.1.0-rc1` tag is pushed
**Then** the workflow produces tarballs for `aarch64-apple-darwin`, `x86_64-apple-darwin`, and `x86_64-unknown-linux-gnu`, attached to the GitHub Release as draft assets

**Given** a fresh macOS arm64 machine (or VM, or wiped `~/.bowerbird/` and `~/.claude/settings.json` backup-and-restore)
**When** the maintainer downloads the `v0.1.0-rc1` tarball, runs `tar -xz`, then `bowerbird install`, and starts a Claude Code session
**Then** events appear in `~/.bowerbird/bower.db`, the daemon is running, and the first-party presenter from Story 5.1 receives state frames

**Given** the cross-version upgrade contract test `cross_version_upgrade.rs`
**When** Story 5.11 lands
**Then** its SKIP guard (currently load-bearing on the absence of a real prior tag) is removed or asserts against `v0.1.0-rc1`'s data directory, depending on which boundary is tested

**Given** Gatekeeper warnings on first run of unsigned macOS tarball binaries
**When** the maintainer follows `INSTALL.md`'s `xattr -d com.apple.quarantine ...` step
**Then** the binary runs successfully; this is documented as the V1-acceptable path and the deferred-work entry for code-signing/notarization remains open (cost decision: post-V1)

**Given** the rc1 release surfaces a behavioral, install, or release-pipeline issue
**When** the maintainer escalates it
**Then** a `5.X-hotfix-<topic>` story is created inline before moving to Story 5.12

### Story 5.12: Cookbook consolidation into self-contained directory entries

As the bowerbird maintainer,
I want each cookbook entry to be one self-contained directory under `docs/cookbook/<name>/` containing prose (`README.md`) and runnable code (`src/`, `package.json`, `tsconfig.json`) colocated,
So that the cookbook is the canonical home of the working examples — no duplication, no drift-check, no separate `examples/` surface to navigate.

Closes Story 4.2 AC at `epics.md:817-819`, Story 4.3 AC at `epics.md:843`, and `project-context.md` §Cookbook discipline directive (L526) "do not hand-copy snippets — they rot." Closes `deferred-work.md:104` ("Cookbook inlining mechanism"). See `sprint-change-proposal-2026-05-26-cookbook-consolidation.md` for the full rationale. Resequenced from 5.5 → 5.7 by `sprint-change-proposal-2026-05-27-epic-5-resequencing.md`, then → 5.8 by `sprint-change-proposal-2026-05-29-idle-prompt-reclassification.md` (idle-prompt story inserted at 5.6), then → 5.12 by `sprint-change-proposal-2026-06-01-dogfood-triage.md` (four dogfood-triage stories 5.7–5.10 inserted after Story 5.6, renumbering the release-readiness tail; this also reassigned ADR 0006 to Story 5.7's session-cwd decision, so cookbook consolidation's ADR moves to the next free number — see the ADR criterion below) (dogfooding-first ordering: cookbook consolidation is reader-facing, not load-bearing for the maintainer's daily use).

**Acceptance Criteria:**

**Given** the existing `examples/multi-session-router/`, `examples/event-log-viewer/`, and `examples/reconnect-recovery/` directories
**When** Story 5.12 lands
**Then** they have been `git mv`'d to `docs/cookbook/state-session-fanout/`, `docs/cookbook/rest-cursor-pagination/`, and `docs/cookbook/dropped-frame-recovery/` respectively; the `examples/` directory no longer exists at the repo root; `cargo build --workspace`, `cargo test --workspace`, and the TypeScript smoke tests all pass against the new paths

**Given** the three standalone cookbook prose files (`docs/cookbook/state-session-fanout.md`, `docs/cookbook/rest-cursor-pagination.md`, `docs/cookbook/dropped-frame-recovery.md`)
**When** Story 5.12 lands
**Then** they have been deleted; their Problem / Approach / Variants content has been folded into the per-entry `docs/cookbook/<name>/README.md` files alongside the existing per-example README content

**Given** each new `docs/cookbook/<name>/README.md`
**When** a reader opens it
**Then** the README contains no embedded TypeScript code blocks — only prose sections (*What this is*, *Run it*, *How it works*, *How to apply it*, *Files* with relative links to `src/index.ts` and any sidecar code files); code is read by opening `src/index.ts` directly, matching the pocketflow cookbook pattern

**Given** the `// cookbook-begin:<name>` / `// cookbook-end:<name>` comment markers in each `src/index.ts`
**When** Story 5.12 lands
**Then** the markers have been deleted; the smoke test `tests/cli_examples.rs::each_example_source_carries_cookbook_anchors` (or its current equivalent) has been deleted; the drift-check test `tests/cli_docs_drift.rs::cookbook_include_directives_match_example_anchors` has been deleted

**Given** the smoke-test crate `tests/cli_examples.rs` and CI workflow at `.github/workflows/ci.yml`
**When** Story 5.12 lands
**Then** all `examples/*/src/index.ts` path references have been retargeted to `docs/cookbook/*/src/index.ts`; shell loops over `examples/*/` similarly retarget

**Given** the planning and project-context artifacts
**When** Story 5.12 lands
**Then** `prd.md:327, 445, 448-450`, `architecture.md:760-829, 915, 946`, and `project-context.md:242-258, 524-545` have been updated to reflect the single-directory shape; `deferred-work.md:104` is struck through with a backlink to this story's merge commit; path-retarget edits applied to `deferred-work.md:101, 102, 105, 106, 107`

**Given** the project's update protocol (`project-context.md` L77: "Every merged ADR includes Affects context.md sections: field")
**When** Story 5.12 lands
**Then** ADR 0008 has been authored at `docs/decisions/0008-cookbook-consolidation.md` documenting the decision, considered alternatives (mdBook `{{#include}}`, hand-rolled preprocessor, pocketflow pattern), the chosen path, and `Affects context.md sections: Repository layout, Cookbook discipline` (ADR 0005 is the idle_prompt reclassification per Story 5.6; ADR 0006 is session cwd + started_at on the wire per Story 5.7; ADR 0007 is reserved for daemon start-on-login per Story 5.9 — 0008 is the next free number. If the number should not be allocated until this story lands, treat the path as TBD and claim the next free ADR number then.)

**Given** reader-facing docs
**When** Story 5.12 lands
**Then** `README.md` (entries at L7-8 and L162-166), `docs/quickstart.md:19`, and `docs/presenter-authoring.md` (grep pass) have all `examples/` path references retargeted to `docs/cookbook/<name>/`

### Story 5.13: First-time-reader docs pass

As a developer who has never seen bowerbird before,
I want the README and quickstart to answer "what is this, why would I care, how do I try it in five minutes" before I bounce,
So that the V1 audience (other developers reachable via the Claude Code community) can decide bowerbird is worth their attention.

Resequenced from 5.6 → 5.8 by `sprint-change-proposal-2026-05-27-epic-5-resequencing.md`, then → 5.9 by `sprint-change-proposal-2026-05-29-idle-prompt-reclassification.md` (idle-prompt story inserted at 5.6) (dogfooding-first ordering: first-time-reader docs are reader-facing, not load-bearing for the maintainer's daily use).

**Acceptance Criteria:**

**Given** the current `README.md`
**When** a first-time reader (defined as someone who has not read `docs/`, `project-context.md`, or any planning artifact) opens it
**Then** within the first screen they learn: what bowerbird is (one sentence), why it exists (one sentence), and what they can do in five minutes (call to action linking to `docs/quickstart.md`)

**Given** the current `docs/quickstart.md`
**When** the first-time reader follows it on a fresh machine with neither Claude Code nor bowerbird installed
**Then** they complete the quickstart (install bowerbird, run `bowerbird replay`, run one reference example, see live state output) in under five minutes wall-clock

**Given** the docs path Quickstart → presenter-authoring → protocol → cookbook (PRD §Documentation Requirements line 436)
**When** the first-time reader graduates from Quickstart and reaches `docs/presenter-authoring.md`
**Then** the first paragraph names the audience switch ("you've seen it work; now you're going to build something") rather than starting directly in technical detail; cross-references to the cookbook target the per-entry directory shape introduced by Story 5.12 (e.g. `docs/cookbook/state-session-fanout/`), not pre-5.8 standalone .md files

**Given** the README in its current state mentions install before motivation
**When** Story 5.13 lands
**Then** motivation precedes install; the "Status: V1 in development" framing is removed in favor of "Status: v0.1.0 — first stable release" once Story 5.14 tags it

**Given** the Story 5.13 PR
**When** review runs
**Then** the review explicitly invokes the `bmad-editorial-review-prose` and `bmad-editorial-review-structure` skills against `README.md` and `docs/quickstart.md`, and the priority-1 findings are addressed in the same PR

### Story 5.14: Crates.io namespace decision and v0.1.0 tag

As the project owner,
I want a deliberate decision on crates.io publishing,
And the v0.1.0 tag pushed, so V1 is shipped.

Closes Epic 3 retro AI-3 / Epic 4 retro AI-5.

**Acceptance Criteria:**

**Given** `cargo search bowerbird`
**When** Story 5.14 is started
**Then** the namespace availability is documented (available / squatted / taken-by-related-project); if available, the four workspace crates are published with `description`, `repository`, `keywords`, `categories`, and `[package.metadata.docs.rs]` blocks added to each `Cargo.toml`; if not available, an ADR documents the renaming decision or the decision to publish under a different namespace

**Given** all Epic 5 stories 5.1 through 5.9 are complete and any hotfix stories are merged
**When** the maintainer tags `v0.1.0`
**Then** the release workflow runs end-to-end producing artifacts; the GitHub Release is published (not draft); release notes name the V1 scope, the dogfooding signal that motivated the tag, and the contract-test summary

**Given** the v0.1.0 tag exists
**When** the maintainer reads `docs/bmad/implementation-artifacts/deferred-work.md`
**Then** every entry referenced in this Epic 5 (Story 5.5 AI-1/AI-2/AI-3, Story 5.4's five entries, Story 5.12's deferred-work-104 closure, Story 5.14's AI-3/AI-5) is struck through with a backlink to its closing story's merge commit

**Given** the v0.1.0 release notes
**When** a first-time reader (Story 5.13's audience) finds them
**Then** they include the install one-liner, a link to Quickstart, and an honest statement of "what works today and what doesn't" (the deferred-work entries that remain — code-signing, second-adapter, etc.)
