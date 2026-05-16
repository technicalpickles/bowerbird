---
stepsCompleted:
  - step-01-validate-prerequisites
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

{{requirements_coverage_map}}

## Epic List

{{epics_list}}
