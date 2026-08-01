---
stepsCompleted:
  - step-01-init
  - step-02-context
  - step-03-starter
  - step-04-decisions
  - step-05-patterns
  - step-06-structure
  - step-07-validation
  - step-08-complete
inputDocuments:
  - docs/bmad/planning-artifacts/prd.md
  - docs/bmad/project-context.md
workflowType: 'architecture'
lastStep: 8
status: 'complete'
completedAt: '2026-05-16'
project_name: 'bowerbird'
user_name: 'pickles'
date: '2026-05-16'
---

# Architecture Decision Document

_This document builds collaboratively through step-by-step discovery. Sections are appended as we work through each architectural decision together._

## Project Context Analysis

### Requirements Overview

**Functional Requirements (39 total):**

- **Hook Integration & Event Capture (FR1–FR5):** Shim captures Claude Code
  hook events with < 5ms p95 marginal latency; stdout/stderr-silent on the
  success and exit-0 (daemon-answered) paths, with one `bowerbird: <cause>`
  stderr line on exit-1 failures (Story 5.10); logs failures to
  `~/.bowerbird/shim.log` (mode 0600); adapter normalizes payloads to canonical
  protocol format with raw payload preserved verbatim.
- **Event Storage & Persistence (FR6–FR9):** Events persisted atomically with
  session state projection in the same SQLite transaction; WAL-mode durability;
  cursor-based retrieval via `EventId(i64)`; `oldest_available_event_id`
  exposed for gap detection.
- **Real-Time Event Streaming (FR10–FR17):** WebSocket pub/sub with topic
  filtering, wildcard subscriptions, multi-session fan-out; `DroppedFrame`
  on lag with `first_dropped_event_id` + `last_dropped_event_id` + count;
  snapshot-on-subscribe via sync frame; graceful `close` on shutdown.
- **Event Query & History (FR18–FR23):** REST snapshot API
  (`/sessions/:id/events?since=<cursor>`); session stats; unauthenticated
  `/healthz` + `/readyz`.
- **Session Tracking (FR24–FR26):** `(source, session_id)` composite key;
  per-session projection; hook-unreliability tolerance (no stuck state on
  missing `PostToolUse` or `Stop`; 5-minute read-time stale-`Working`
  fallback backstops both).
- **Installation & Configuration (FR27–FR30):** Prebuilt binaries +
  `cargo install`; daemon lifecycle commands; status/version CLI.
- **Developer Tools & Experience (FR31–FR35):** `bowerbird replay` and
  `bowerbird export`; three self-contained cookbook entries (state-session
  fan-out, REST cursor-pagination, dropped-frame recovery); bundled
  fixtures; full documentation path.
- **Protocol & Compatibility (FR36–FR39):** Additive-only guarantee within
  v1.x; Unix socket with filesystem auth; UUID4 bearer token for TCP surface;
  structured changelog CI-enforced.

**Non-Functional Requirements (22 total):**

- **Performance (NFR1–NFR3):** Shim p95 < 5ms (hard, measured as marginal cost
  on the warm success path); daemon 2s cold-start readiness; no speculative
  optimization.
- **Reliability (NFR4–NFR7):** WAL-mode durability; ENOSPC handled with error
  log + clean ingest close; acknowledged events survive daemon crash;
  no V1 rate limiting (documented limitation).
- **Compatibility (NFR8–NFR10):** macOS arm64 + x86_64; Linux x86_64 (glibc);
  stable toolchain only.
- **Security (NFR11–NFR15):** UUID4 bearer token; system keychain primary with
  env-var + file fallback; shim failure log mode 0600.
- **Operability (NFR16–NFR18):** Error-level default logging; crash info to
  `~/.bowerbird/`; metrics deferred.
- **Protocol stability (NFR19):** No breaking changes within v1.x.
- **Implementation constraints (NFR20–NFR22):** Socket listen backlog ≥ 128;
  auto-migration on startup (fatal on failure); timestamp column on all event
  rows.

**Scale & Complexity:**

- Primary domain: Local developer tool / system binary + API substrate
- Complexity level: Medium — technically nuanced, single-developer workload
- Estimated architectural components: 5 crates (protocol, shim, daemon,
  adapter-claude, CLI binary) + 2 socket surfaces + SQLite + pub/sub layer

### Technical Constraints & Dependencies

- Rust stable toolchain — no nightly features; `rust-version` pinned per-crate
- `Cargo.lock` committed — reproducible builds required for perf budget claims
- No async runtime in shim — Tokio runtime init alone exceeds the 5ms budget
- Single-writer SQLite with WAL — `deadpool-sqlite` with explicit
  `writer(max=1)` + `readers(max=4)` pools
- `127.0.0.1` bind only — non-loopback bind is a separate ADR
- macOS + Linux CI matrix — per-platform perf baselines; no averaging
- Protocol crate stability — every change is a coordination cost; dep budget
  tighter than any other crate

### Cross-Cutting Concerns Identified

- **Performance isolation:** Shim hot-path rules (no async, no allocation on
  success path, no logging on success path) must be enforced at crate
  boundary — not by convention
- **Atomicity:** State projection + event INSERT in the same SQLite transaction
  is a load-bearing correctness invariant
- **Protocol stability:** Asymmetric `deny_unknown_fields` (strict inbound,
  permissive outbound) is the key invariant for additive forward-compat
- **Observability:** `tracing` span fields (`source`, `session_id`, `event_id`)
  must be consistent across ingest → projection → WS-handler boundary
- **Security:** Two auth models (filesystem 0600 for ingest socket; UUID4
  bearer token for TCP) — must not leak token via ingest path or vice versa
- **Error propagation:** `thiserror`-only in `protocol` and `shim`; `anyhow`
  permitted at daemon/adapter-claude binary edges only
- **Test infrastructure:** Contract tests + bench regression gating are
  pre-MVP gates, not post-MVP polish

---

### Open Questions — Resolved

All five architectural open questions were resolved through collaborative
analysis before architecture decisions were made.

**OQ#1 — Shim-when-daemon-down: Fire-and-forget**

The shim attempts `connect()` + `write()` on `~/.bowerbird/ingest.sock`. On
`ECONNREFUSED` or `ENOENT`, it logs to `~/.bowerbird/shim.log` (mode 0600)
and exits. Events during daemon-down are **lost by design** — this is a
documented property of the transport contract, consistent with the
developer-tool, local-service nature of the project.

Rationale: direct SQLite write couples the shim to a specific schema version
(deployment hazard); inotify spool masquerades as a durability guarantee it
cannot provide. Zero new shim crates.

Corollary: the daemon writes `EventKind::RecordingStarted` on startup and
`EventKind::RecordingEnded` on clean shutdown. Gaps between sentinels are
*known unknowns*, not unknown unknowns. A `RecordingStarted` not preceded by
a `RecordingEnded` is the mechanical fingerprint of a crash gap.

**OQ#2 — Protocol-level gap detection: Full surface defined**

*REST `EventListResponse`:* `events: Vec<Event>` + `cursor: Option<EventId>`
+ `oldest_available_event_id: EventId` (never Option; `i64::MAX` when empty).
Presenter inference: `if since < oldest_available_event_id { /* gap */ }`.
No `gap_detected: bool` — Axiom 4.

*`HelloFrame` (on WS connect):* `protocol_version` + `daemon_version` +
`oldest_available_event_id` + `daemon_started_at: i64` +
`history_begins_cleanly: bool`. The bool tells cold-start presenters whether
`oldest_available_event_id` falls inside a known-clean recording window —
derived from the `recording_sessions` shadow table.

*WS `sync` frame (on subscribe):* `oldest_available_event_id` +
`latest_event_id`. No inline events — presenter REST-catches-up, then
re-subscribes. Keeps WS path live-only; REST is the single source of history.

*WS `DroppedFrame`:* `count` + `first_dropped_event_id: EventId` +
`last_dropped_event_id: EventId`. Precision cursors; presenter passes
`first_dropped_event_id` as `since` to REST re-fetch.

*Sentinel events:* `EventKind::RecordingStarted` / `EventKind::RecordingEnded`
with `EventSource::Daemon`. Normal events in the stream, detectable by kind.

*`recording_sessions` shadow table* (schema from day one, never truncated):
`(id, started_event_id, ended_event_id nullable)`. Truncation cannot destroy
sentinel semantics. `history_begins_cleanly` is derived from this table.

*Serde rule:* inbound `deny_unknown_fields`; outbound permissive — no
exceptions on the public surface. `Event.payload: String` — verbatim raw
JSON, no schema imposed, no information loss.

*Reconnect flow (8 steps):* connect → receive HelloFrame (check
`daemon_started_at` for restart detection, check `history_begins_cleanly`) →
gap check (compare `local_cursor` to `oldest_available_event_id`) → REST
snapshot fetch → WS subscribe with cross-check → live consumption with
dedup on `event_id` → DroppedFrame handling (REST re-fetch from
`first_dropped_event_id`) → sentinel detection (`RecordingStarted` =
daemon-down gap, no EventId hole).

**OQ#3 — Adapter contract shape: In-process Rust trait + Vendor escape hatch**

V1: `SourceAdapter` trait in `crates/protocol/src/adapter.rs`. `normalize()`
is sync and pure — testable with raw byte slice, no daemon, no Tokio.
`event_id` is zero on entry; daemon assigns at INSERT. Raw payload preserved
verbatim — normalization is labeling (attach reaction), not replacement.

`Reaction::Vendor(u16)` added to the reaction enum. Future adapters claim a
numbered value without touching `crates/protocol`. Two-presenters rule
governs graduation from `Vendor(n)` to a named variant. Adapters ship without
being blocked on protocol consensus.

V2 adapter contract shape (subprocess vs. in-process): **deferred**. Will be
designed against the first real external adapter.

**OQ#4 — Time/ID types: `EventId(i64)` AUTOINCREMENT**

`EventId(i64)` newtype in `crates/protocol`. SQLite `INTEGER PRIMARY KEY
AUTOINCREMENT`. Wire format: plain JSON number (`since=42`).
`WHERE event_id > $cursor` is integer comparison — no collation concerns, no
generator state, no sub-millisecond ordering hazard.

Sentinel events get normal AUTOINCREMENT IDs — zero special handling. ADR
documents V2 break conditions: multi-daemon merge, backup restore to prior
state.

**OQ#5 — Event-log truncation policy: Deferred post-V1**

No truncation in V1. Documented escape hatch: delete/truncate
`~/.bowerbird/bower.db` directly. `bowerbird gc` command is post-V1.
`recording_sessions` shadow table (from OQ#2 resolution) is added to the
schema now so truncation can be implemented correctly when OQ#5 is resolved.

---

### Additional Contract Tests Identified (beyond original 10)

The following tests were surfaced during architectural analysis and should
join the pre-MVP gate list:

- **Pool starvation behavioral contract:** All 4 reader slots exhausted under
  concurrent REST requests — assert defined error or timeout behavior,
  not silent hang.
- **Unix socket backpressure:** Shim connects to a live daemon whose kernel
  socket buffer is full — assert defined behavior (log + exit), not block.
- **Single-threaded chaos under slow writes:** Inject artificial write latency
  (WAL checkpoint); assert WS delivery latency stays within the 100ms
  presenter bar.
- **Settings.json atomic install (tightened):** After simulated interrupt,
  assert file is in exactly one of two valid states (pre-install or
  post-install content hash), not merely "valid JSON."
- **`recording_sessions` row lifecycle:** Row written on startup;
  `ended_event_id` populated on clean shutdown; null on crash.
- **`history_begins_cleanly` accuracy:** Cold-start presenter with cursor
  below a crash gap sees `history_begins_cleanly: false`.
- **WS concurrency cap:** 257th connection attempt receives defined rejection
  (not hang).
- **Adapter `event_id` invariant:** Daemon rejects any normalized
  `EventEnvelope` where `event_id != 0`.

---

## Starter Template Evaluation

### Primary Technology Domain

**System binary + API substrate** — this project has no web framework frontend.
The "starter" is a Rust workspace scaffold, hand-constructed rather than
generated by a CLI tool.

### Starter Options Considered

For a Rust workspace with these characteristics (shim binary requiring no async
runtime, daemon with Tokio + axum + SQLite, shared protocol crate), no single
CLI generator (`cargo new`, `cargo-generate` templates) produces the correct
multi-crate layout with the precise dependency constraints required.

Options evaluated:

1. **`cargo new` + manual workspace conversion** — Standard path; requires
   adding `[workspace]` to root `Cargo.toml` and creating member crates.
2. **`cargo-generate` with a Rust workspace template** — No maintained template
   matched the shim/daemon isolation requirement.
3. **Hand-scaffold the workspace manifest** — Full control over member list,
   dep pinning, lint config, and custom release profile.

### Selected Starter: Rust Workspace Hand-Scaffold

**Rationale for Selection:**

The shim's 5ms p95 hard budget prohibits a Tokio runtime, making the project
architecturally bimodal: one crate class (shim) is sync/static-binary, the
other class (daemon, adapter-claude) is async/Tokio. No existing generator
template encodes this split. The workspace manifest is the effective "starter" —
it sets dependency budgets, lint rules, and build profiles that govern all five
crates.

**Initialization Commands:**

```bash
# Create workspace root (existing git repo)
cargo init --name bowerbird-ws --vcs none .
# Create crate members
cargo new --lib crates/protocol
cargo new --lib crates/shim
cargo new crates/daemon
cargo new crates/adapter-claude
```

Then replace root `Cargo.toml` with the workspace manifest below.

**Architectural Decisions Provided by Scaffold:**

**Language & Runtime:**
- Rust stable toolchain; `rust-version` pinned per-crate in each `[package]`
- `Cargo.lock` committed — reproducible builds required for perf budget CI assertions
- No nightly features anywhere in the workspace

**Build Profiles:**
- `[profile.release-shim]`: `panic=abort`, `lto="fat"`, `codegen-units=1`,
  `opt-level="z"`, `strip=true` — maximizes binary compactness and eliminates
  unwinding overhead on the hot path
- Default `[profile.release]` for daemon and adapter-claude

**Workspace Lint Configuration:**
- `[workspace.lints.rust] unsafe_code = "forbid"` — propagates to all crates;
  violations are compile errors, not warnings

**Dependency Version Pins (verified at scaffold time):**

| Crate | Version | Notes |
|---|---|---|
| tokio | 1.52.1 | `rt,macros,net,io-util,sync,time,signal,fs` |
| axum | 0.8.9 | HTTP + WebSocket server |
| rusqlite | 0.39.0 | `bundled,backup,blob` |
| deadpool-sqlite | 0.13.0 | writer(max=1) + readers(max=4) |
| tower-http | 0.6.10 | axum middleware |
| tracing | 0.1.44 | Structured logging/spans |
| tracing-subscriber | 0.3.20 | Log formatting + filtering |
| serde | 1.0.228 | `derive` feature |
| serde_json | 1.0.149 | JSON ser/de |
| thiserror | 2.0.18 | Error types in `protocol` and `shim` |
| anyhow | 1.0.102 | Error handling at binary edges only |
| uuid | 1.23.1 | `v4` feature for bearer token generation |
| rusqlite_migration | 2.5.0 | Schema migrations from day one |
| tokio-util | 0.7.18 | Codec, compat utilities |
| tokio-stream | 0.1.17 | Stream adapters for WS fan-out |
| clap | 4.5.37 | `derive` feature; CLI argument parsing |
| secrecy | 0.10.3 | `SecretString` for bearer token; zeroizes on drop |
| keyring | 3.6.1 | System keychain access (macOS Keychain / Linux Secret Service) |
| tempfile | 3.20.0 | Dev-only; `TempDir` for integration test socket paths |

**Code Organization:**

```
bowerbird/
├── Cargo.toml              # workspace manifest
├── Cargo.lock              # committed
├── crates/
│   ├── protocol/           # stable wire types, SourceAdapter trait, EventId
│   ├── shim/               # static binary, sync-only, no Tokio
│   ├── daemon/             # Tokio + axum + SQLite, pub/sub, REST + WS
│   └── adapter-claude/     # Claude Code hook normalization, installs shim
└── docs/
```

**Development Experience:**
- `cargo build -p shim --profile release-shim` — shim-specific build
- `cargo test --workspace` — full workspace test run
- `cargo clippy --workspace --all-targets` — lint all crates
- `RUST_LOG=error` default; `RUST_LOG=debug` for development

**Note:** The first implementation story is: initialize this workspace scaffold,
verify all crate version pins compile, and get `cargo check --workspace` green.

---

## Core Architectural Decisions

### Decision Priority Analysis

**Critical Decisions (Block Implementation):**
- SQLite schema (events, session_projections, recording_sessions)
- Auth model split: Unix socket 0600 ingest / UUID4 bearer TCP
- EventId(i64) AUTOINCREMENT cursor semantics
- Process supervision: on macOS, a launchd LaunchAgent (`RunAtLoad` + `KeepAlive={SuccessfulExit=false}`) starts the daemon on login and restarts it on crash (V1, Story 5.9 / ADR 0007); on Linux, a backgrounded process via `setsid` (V1), with systemd integration still deferred post-V1. CLI surfaces lifecycle: `bowerbird install` registers the LaunchAgent (macOS) or spawns the daemon detached (Linux), `bowerbird stop` (macOS) boots out a loaded LaunchAgent (`launchctl bootout`, so `KeepAlive` cannot bounce a forced stop back), falling back to PID-file SIGTERM with SIGKILL escalation after 10s for a manual / pre-5.9 daemon, on Linux, and when launchd state is unverifiable.

**Important Decisions (Shape Architecture):**
- deadpool-sqlite writer(max=1) + readers(max=4) pool split
- Asymmetric serde `deny_unknown_fields`
- SourceAdapter trait: sync + pure `normalize()`; `Reaction::Vendor(u16)` escape hatch
- CLI framework: clap 4.x with derive macro
- Keyring crate: `keyring` v3

**Deferred Decisions (Post-MVP):**
- Linux systemd service integration (macOS launchd start-on-login is now V1 — Story 5.9 / ADR 0007; Linux supervision stays deferred)
- Event-log truncation (`bowerbird gc`)
- V2 adapter contract (subprocess vs. in-process)
- Rate limiting
- Non-loopback TCP bind
- Metrics collection

### Data Architecture

**Database: SQLite (WAL mode)**
- Bundled via rusqlite 0.39.0 (`bundled,backup,blob` features)
- WAL mode enabled on startup; read/write concurrency without locking
- deadpool-sqlite 0.13.0: writer pool `max_size=1`, reader pool `max_size=4`
- Pool starvation behavior: defined error returned (not silent hang) — contract test required
- ENOSPC: error logged; ingest socket closed cleanly

**Schema Migrations: rusqlite_migration 2.5.0**
- Auto-migration on daemon startup; fatal on failure (daemon refuses to start with unknown schema)
- Append-only; no destructive migrations in V1

**SQLite Schema:**

```sql
CREATE TABLE events (
    event_id   INTEGER PRIMARY KEY AUTOINCREMENT,
    source     TEXT    NOT NULL,         -- adapter source, e.g. "claude-code"
    session_id TEXT    NOT NULL,
    kind       TEXT    NOT NULL,         -- EventKind as string
    reaction   TEXT,                     -- Reaction variant; NULL for daemon sentinels
    payload    TEXT    NOT NULL,         -- verbatim raw JSON; no information loss
    created_at INTEGER NOT NULL          -- Unix timestamp ms
);

CREATE TABLE session_projections (
    source     TEXT    NOT NULL,
    session_id TEXT    NOT NULL,
    state      TEXT    NOT NULL,         -- JSON blob of projected session state
    updated_at INTEGER NOT NULL,
    PRIMARY KEY (source, session_id)
);

-- Shadow table; never truncated; enables history_begins_cleanly post-truncation
CREATE TABLE recording_sessions (
    id               INTEGER PRIMARY KEY AUTOINCREMENT,
    started_event_id INTEGER NOT NULL,
    ended_event_id   INTEGER             -- NULL until clean shutdown
);
```

**Cursors & IDs:**
- `EventId(i64)` newtype; AUTOINCREMENT; wire format: plain JSON number
- `WHERE event_id > $cursor` — integer comparison, no collation concerns
- `oldest_available_event_id` is `i64::MAX` when the events table is empty

**Caching:** None. Local SQLite with WAL readers is sufficient for the developer-tool load profile.

### Authentication & Security

**Ingest socket (shim → daemon):**
- Unix domain socket `~/.bowerbird/ingest.sock`; filesystem 0600; no wire auth
- Listen backlog ≥ 128
- Shim failure log: `~/.bowerbird/shim.log` mode 0600

**TCP surface (tools → REST + WS):**
- Bind: `127.0.0.1:<port>` only
- Auth: UUID4 bearer token, `Authorization: Bearer <token>` header
- `/healthz` and `/readyz` unauthenticated
- Token storage resolver: `BOWERBIRD_TOKEN` env-var → `keyring` v3 system keychain (macOS Keychain / Linux Secret Service, service=`bowerbird-daemon` user=`bearer-token`; UUID4 generated and stored on first run) → `~/.bowerbird/config.toml` `token` field (mode 0600 expected; warn but still load on wider mode). Env-first ordering is the Story 3.3 reconciliation of NFR12: test infrastructure reuse + escape-hatch ergonomics; documented inline in `crates/daemon/src/api/token.rs` module doc comment.

**Invariants:**
- Ingest path never reads or logs the bearer token
- `unsafe_code = "forbid"` workspace-wide

### API & Communication Patterns

**HTTP Server: axum 0.8.9**
- Tokio `current_thread` runtime — no work-stealing overhead; sufficient for local tool load
- tower-http 0.6.10 for auth + tracing middleware
- `AppState { db: DbPools, broadcasters: Broadcasters, auth: TokenStore, shutdown: CancellationToken }`

**REST:**
- `GET /sessions` — list sessions with stats
- `GET /sessions/:id/events?since=<cursor>` — returns `EventListResponse { events, cursor, oldest_available_event_id }`
- `GET /healthz` — liveness (unauthenticated)
- `GET /readyz` — readiness; 503 until migrations complete (unauthenticated)

### WebSocket subsystem

**Wire surface:** Upgrade at `GET /ws`; bearer auth on upgrade (header or `?token=` query fallback per protocol-changelog v1.0 → v1.1). Topic filtering: `events.*`, `events.<source>.*`, `events.<source>.<session_id>`, `state.session.*`, `state.session.<id>`, `state.session.<id>.current_state`. Fan-out via `tokio::sync::broadcast`; slow consumers receive a coalesced `DroppedFrame` rather than blocking the publisher.

**Runtime config knobs** (defaults in `crates/daemon/src/config.rs::Config::with_bowerbird_dir`; all overridable via the daemon's `Config` builder):

| Field | Default | Role |
|---|---|---|
| `ws_max_connections` | `256` | Semaphore cap on concurrent WS connections; the 257th upgrade returns HTTP 503. |
| `ws_ping_interval` | `30s` | Per-client liveness probe cadence (axum WS Ping frame). |
| `ws_pong_timeout` | `10s` | If no Pong arrives within this deadline of a Ping, the connection is closed; dead-connection cleanup is deadline-granularity, not next-tick-granularity. |
| `ws_broadcast_capacity` | `1024` | Per-channel ring buffer size; a subscriber more than this many envelopes behind the publisher triggers a `DroppedFrame`. |
| `shutdown_drain_timeout` | `5s` | After SIGTERM/SIGINT, the daemon waits up to this long for WS tasks to drain protocol `close` frames before forcing the WebSocket control close. |
| `ws_broadcast_coalesce_window` | `1s` | Sliding window for coalescing `DroppedFrame` emissions on a sustained-lagging connection; 30s of continuous lag emits ≤31 frames, not 1024+. |

Defaults are committed at `crates/daemon/src/config.rs::Config::with_bowerbird_dir`; the table above MUST be updated in the same commit as any field-default change. There is no machine-checked binding between source and doc — the discipline lives in commit hygiene and code review.

**Hook→presenter latency gate (Story 4.4, AC #7).** End-to-end p99 from ingest-socket write to WS frame receive is gated by `crates/daemon/benches/hook_to_presenter.rs` against the NFR2 absolute ceiling of 100ms AND a per-platform regression ratio committed in the baseline files. Four shapes: solo presenter, 3-presenter fanout (max-of-three), burst (8 events within 50ms, Claude Code's tool-call clump), and steady-state. Per-platform policy (absolute budget + regression ratio) at `crates/daemon/benches/baselines/{macos,linux}.json`; the CI job `daemon-bench-gate` invokes `scripts/check-daemon-bench-p99.py` via the best-of-2 wrapper `scripts/run-bench-gate.py` (Story 5.18). The discipline mirrors the shim hot-path gate (`crates/shim/benches/hot_path.rs`); the daemon ratio is looser than the shim's per Axiom 3, daemon-internal perf is soft inside, with the committed baseline files as the source of truth for the actual numbers (restating them here is how they drifted; Story 5.18).

**Contract-test serialization (operational note — retired 2026-07-29).** The suite ran under `--test-threads=1` from Epic 2 retro AI-3 until 2026-07-29. The original diagnosis blamed shared process-wide state (subprocesses, signal handlers, `BOWERBIRD_DATA_DIR`, keychain backends) for hangs observed in Stories 1.6, 2.5, 3.1, 3.2, 3.3; later investigation (see `docs/bmad/implementation-artifacts/investigations/test-serialization-investigation.md`) narrowed the real hang trigger to a *second concurrent `cargo test` process* in the same worktree — which `scripts/test.sh`'s exclusive lock now prevents — plus a SQLite 3.51.1 close deadlock fixed by the vendored libsqlite3-sys patch. The last genuinely parallel-unsafe tests (`story_3_3_auth`, which mutated process env via `set_var`) were converted to inject a `TokenEnv` snapshot; everything else was already isolated (per-test TempDir, ephemeral daemon ports, per-child env). CI and `scripts/test.sh` now run `cargo test --workspace` in parallel; `tests/release_pipeline_docs.rs` gates against re-pinning `--test-threads=1`. If a future test needs serialization, isolate its state instead — and if a parallel run hangs, `scripts/test.sh`'s timeout captures the run log plus `sample` backtraces into `target/test-logs/`.

**Protocol serde:**
- Inbound: `deny_unknown_fields` — strict
- Outbound: permissive — additive forward-compat guaranteed
- `Event.payload: String` — verbatim raw JSON

**Error handling:**
- `thiserror` in `protocol` + `shim`; `anyhow` at binary edges only
- HTTP errors: `{ "error": "<message>" }` with appropriate status code

**Rate limiting:** None in V1; documented limitation.

### Frontend Architecture

Not applicable in this repository. bowerbird has no UI; presenters are external consumers of the WebSocket and REST surfaces.

**Companion projects (out of scope for `crates/`).** A first-party presenter shipped alongside V1 lives in a sibling repository, not in this crate workspace. Per Axiom 1 (the substrate observes; it does not interpret), interpretation belongs in a presenter, and a presenter is structurally a *consumer* of bowerbird — not a component of it. Sibling-repo conventions (naming, license, install path) are documented in the presenter repo itself, not here. See [bowerbird-deck](https://github.com/technicalpickles/bowerbird-deck) — the V1 first-party presenter (Story 5.1).

### Infrastructure & Deployment

**Process supervision (V1):**
- `bowerbird install` writes the hook entries into `~/.claude/settings.json` (atomic: read → parse → merge → write `.tmp` → fsync → rename). On **macOS** it then registers a launchd LaunchAgent at `~/Library/LaunchAgents/com.technicalpickles.bowerbird.daemon.plist` (`RunAtLoad=true` for start-on-login, `KeepAlive={SuccessfulExit=false}` for crash-restart) and bootstraps it (`launchctl bootstrap gui/<uid>`) — launchd owns the lifecycle, so install does NOT also `setsid`-spawn. On **Linux** it spawns the daemon detached via `setsid`; the daemon survives the install process's exit and is owned by the user's session. `--no-start` writes the plist (macOS) but skips the bootstrap/spawn.
- `bowerbird uninstall` reverses the settings.json merge. On **macOS** it boots the LaunchAgent out (`launchctl bootout`, which terminates the daemon) and removes the plist; `--no-stop` removes the plist but skips the bootout. On **Linux** it sends SIGTERM to the daemon (10s drain budget, then SIGKILL escalation). The data directory at `~/.bowerbird/` is intentionally NOT removed — your event history is your data.
- macOS launchd start-on-login + crash-restart is **V1** (Story 5.9 / ADR 0007, reversing the earlier "deferred post-V1"). Linux systemd integration stays deferred post-V1. `KeepAlive={SuccessfulExit=false}` (not `true`) gives crash-restart on a non-zero exit without auto-restarting on a clean shutdown; macOS `bowerbird stop` keeps the daemon down by booting the loaded LaunchAgent out of the domain (`launchctl bootout`) rather than relying on the daemon's exit code, so even a SIGKILL-escalated forced stop is not bounced back (Story 5.9 review pass-6). PID-file SIGTERM/SIGKILL is the manual / Linux / unverifiable-launchctl fallback.

**CLI framework: clap 4.x with derive macro**
- Subcommands (top-level, alphabetical): `auth token`, `export`, `install`, `replay`, `start`, `status`, `stop`, `uninstall`. `version` is provided by clap's built-in `--version` flag.
- CLI binary is intentionally lightweight: no `tokio`, no `axum`, no `reqwest`. HTTP probes to `/healthz`, `/status`, `/sessions/{id}`, `/sessions/{id}/events`, and `POST /replay` are hand-rolled over `std::net::TcpStream`. Daemon spawn uses `libc::setsid` directly. Verified per-story via `cargo tree -p bowerbird --depth 8 | grep -cE '^.* (tokio|axum) v' == 0`.

**Replay & Export (Story 4.1):**
- `bowerbird replay [<file>]` reads JSONL of `protocol::Event` records, POSTs them to the daemon's new `POST /replay` endpoint (bearer-auth). The daemon strips `event_id` + `created_at`, constructs `EventEnvelope`s, and pushes them onto the existing `ingest_tx` channel — so replayed events flow through the same `ingest::writer::run` → `projection::session::write` → broadcast path as live ingest. The CLI's no-arg form uses a bundled fixture embedded via `include_bytes!("../../fixtures/replay-demo.jsonl")`. Replay does NOT preserve original inter-event timing; events are forwarded as fast as the channel accepts them. (`crates/daemon/src/api/replay.rs`, `src/commands/replay.rs`, `fixtures/replay-demo.jsonl`)
- `bowerbird export <session-id>` reads `/sessions/<session-id>/events?since=<cursor>` in a cursor-paginated loop and writes JSONL of `protocol::Event` records to stdout (or `-o <path>`). The output shape is the input shape for `bowerbird replay`, so `bowerbird export <id> | bowerbird replay /dev/stdin` round-trips an entire session through the pub/sub path on the same daemon (or, after `bowerbird export <id> > session.jsonl`, on a different machine after `bowerbird install`). (`src/commands/export.rs`)

**Distribution:**
- Prebuilt tarballs (per `.github/workflows/release.yml`): macOS arm64 (`aarch64-apple-darwin`, native build on macos-latest), macOS x86_64 (`x86_64-apple-darwin`, cross-compiled from arm64 runner), Linux x86_64 (`x86_64-unknown-linux-gnu`, built on `ubuntu-22.04` for glibc 2.35+ baseline). Each tarball contains `bin/{bowerbird, bowerbird-shim, bowerbird-daemon}` plus `adapters/claude/tool-reactions.toml`, `LICENSE*`, `README.md`, `INSTALL.md`, `CHANGELOG.md`.
- The shim binary in shipped tarballs is built under the `release-shim` profile (panic=abort, lto=fat, codegen-units=1, opt-level=z, strip=true) to preserve the p99 ≤5ms hot-path budget; the CLI and daemon use the default `release` profile.
- `cargo install --git https://github.com/<owner>/bowerbird --tag vX.Y.Z` as alternative from-source path (NFR10: stable Rust 1.82+; no nightly required).
- musl Linux is deferred post-V1 (NFR9). Windows is an explicit V1 scope cut.
- Crates.io publishing deferred post-V1; verify namespace availability before tagging the first crates.io-targeted release.
- `Cargo.lock` committed; `--locked` enforced on every release build.

**Logging:**
- `tracing` + `tracing-subscriber`; default level `error`; `RUST_LOG` override
- Span fields consistent across ingest → projection → WS handler: `source`, `session_id`, `event_id`
- Crash info to `~/.bowerbird/`; metrics deferred

**CI matrix:** macOS arm64, macOS x86_64, Linux x86_64 — per-platform perf baselines, no averaging.

### Decision Impact Analysis

**Implementation sequence:**
1. Workspace scaffold (`cargo check --workspace` green)
2. `crates/protocol` — EventId, wire types, SourceAdapter trait, Reaction enum
3. `crates/shim` — sync hot path, Unix socket write, shim.log failure handler
4. `crates/daemon` — SQLite schema + migrations, ingest socket, event INSERT + projection (same transaction)
5. `crates/daemon` — axum REST endpoints + auth middleware
6. `crates/daemon` — WebSocket pub/sub, DroppedFrame, HelloFrame
7. `crates/adapter-claude` — hook normalization, shim installation
8. CLI binary — clap subcommands (`install`/`uninstall`/`start`/`stop`/`status`/`auth token`), settings.json merge via `adapter-claude::install`, daemon spawn via `setsid`, PID-file + flock singleton, system-keychain token resolver. `replay`/`export` ship in Story 4.1.

**Cross-component dependencies:**
- `protocol` is a dep of all crates; every change has maximum blast radius — dep budget tightest here
- `shim` depends on `protocol` only; zero daemon deps — enforced by Cargo dep graph
- `daemon` depends on `protocol`; normalizes via `SourceAdapter` at the ingest boundary
- CLI `bowerbird install` depends on the prebuilt-binary distribution from the release step (or `cargo install --git` for from-source installs); the hook entry written into `~/.claude/settings.json` uses `protocol::SHIM_BINARY_NAME` (= `"bowerbird-shim"`) as a PATH-relative name so re-downloads to a different `$PATH` location are picked up automatically without re-running `bowerbird install`.

---

## Implementation Patterns & Consistency Rules

### Pattern Categories

**Conflict points addressed:** 18 explicit decisions covering naming, structure,
wire format, and process conventions — all areas where independent AI agents
could make incompatible choices while individually following reasonable defaults.

### Naming Conventions

| Context | Convention | Example |
|---|---|---|
| Rust types, traits, enums, variants | PascalCase | `EventKind`, `ToolUse`, `SourceAdapter` |
| Rust functions, variables, modules, fields | snake_case | `event_id`, `normalize`, `session_id` |
| SQLite tables | snake_case, plural | `events`, `session_projections` |
| SQLite columns | snake_case | `event_id`, `created_at`, `session_id` |
| JSON wire fields | snake_case (serde default; no `rename_all`) | `"event_id"`, `"session_id"` |
| REST path segments | snake_case, plural resources | `/sessions`, `/sessions/:id/events` |
| REST query parameters | snake_case | `?since=42` |
| WS subscription topic | `session:<session_id>` or literal `*` | `session:abc-123` |
| EventKind wire string | PascalCase matching variant name | `"ToolUse"`, `"RecordingStarted"` |
| Reaction wire string | PascalCase; Vendor(n) as string | `"Pause"`, `"Vendor(42)"` |
| WS frame op value | snake_case via `rename_all` on outer enum | `"hello"`, `"dropped"`, `"sync"` |

**Critical:** `EventKind` uses PascalCase-as-written (no `rename_all`). The WS
frame outer enum uses `rename_all = "snake_case"`. These are different policies
on different types. Applying `rename_all` to `EventKind` would silently break
wire compatibility.

### Structural Conventions

**Test placement:**
- Unit tests: `#[cfg(test)]` module at the bottom of the file under test
- Integration tests: `crates/<name>/tests/<name>.rs`
- Contract tests (pre-MVP gates): `crates/<name>/tests/contract_<name>.rs`

**Error module contract** — every crate's `src/error.rs` must contain exactly:
```rust
pub enum Error { ... }
pub type Result<T> = std::result::Result<T, Error>;
```
And `lib.rs` re-exports: `pub use error::{Error, Result};`. Callers always
import from the crate root (`protocol::Error`), never from the submodule path
(`protocol::error::Error`).

**Protocol re-exports:** `crates/protocol/src/lib.rs` re-exports all public
types. Callers never import internal submodule paths.

### Wire Format Conventions

**WS frame enum:**
```rust
#[derive(Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum ServerMessage {
    Hello(HelloFrame),
    Sync(SyncFrame),
    Event(EventFrame),
    Dropped(DroppedFrame),
}

#[derive(Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum ClientMessage {
    Subscribe { topic: String },
    Unsubscribe { topic: String },
}
```

**Timestamps:** Unix milliseconds as `i64` everywhere on the wire. No RFC3339
strings. No seconds. No microseconds.

**EventId on wire:** plain JSON number. Never a string, never an object.

**HTTP error body:** exactly `{ "error": "<human-readable message>" }`. No
`code` field, no nested structure, no additional keys.

**`Reaction::Vendor(n)` serialization:** custom `impl Serialize` /
`impl Deserialize` on `Reaction` in `crates/protocol/src/reaction.rs`. Wire
string is `"Vendor(42)"`. No serde derive on this type — hand-written only.
This is the single exception to the derive-based serde pattern.

**Serde policy:**
- Inbound (client→daemon): `#[serde(deny_unknown_fields)]` on every type — no exceptions
- Outbound (daemon→client): no `deny_unknown_fields` — additive forward-compat guaranteed

### Process Conventions

**Shim exit codes:**
- `0` on success — and on the daemon-answered / mid-write WARN class (`SocketIo`,
  `BadResponse`, `Backpressure`, daemon `503`, daemon `400`): the daemon is up and
  answering, so fire-and-forget per NFR20 means Claude must see success
- `1` on the daemon-unreachable / bad-input ERROR class (`Connect`, the stdin
  errors, `BadArgs`, `NoHome`, `LogIo`) — non-blocking warning; Claude continues,
  and `main` additionally names the cause on stderr (Story 5.10)
- `2` is **forbidden** — exit 2 blocks Claude tool calls, which violates the
  substrate-not-actor axiom

**Shim wire format:** newline-delimited JSON over the Unix socket (one
`{object}\n` line in, one status line out: `200\n` / `503\n` / `400 <reason>\n`).
The shim writes the hook JSON with one transport-routing field injected
(`hook_kind`, from the `--hook-kind` CLI flag); Claude Code's original
`hook_event_name` is preserved verbatim. No interpretive normalization in shim;
that remains adapter-claude's job. Daemon calls
`adapter_claude::normalize(hook_kind, raw) -> Result<NormalizeResult>`.
See [ADR-0002](../../decisions/0002-ingest-wire-framing-and-hook-kind.md).

**Shim hot-path rules (non-negotiable):**
- No heap allocation on the success path (best-effort; enforced via criterion
  benchmark with p95 < 5ms CI gate, not a compile-time guarantee)
- No `unwrap()` or `expect()` anywhere in shim
- No `eprintln!` / `println!` / `tracing` calls — silence on the success and
  exit-0 (daemon-answered) paths; failures write to `~/.bowerbird/shim.log`,
  and exit-1 failures additionally emit exactly one `bowerbird: <cause>` line to
  stderr in `main`'s error arm (with a fixed `(see the shim log)` pointer — never
  the env-controlled path — only when the log append succeeded) so a daemon
  outage is not causeless to Claude (Story 5.10)

**Transaction invariant (load-bearing correctness rule):**
```rust
// Exactly these two operations; nothing else joins this transaction
conn.execute("INSERT INTO session_projections ... ON CONFLICT DO UPDATE ...", ...)?;
conn.execute("INSERT INTO events ...", ...)?;
```
The projection UPSERT and event INSERT are the only operations in the
transaction. A broader wrapping transaction is a prohibited pattern.

**Projection UPSERT pattern** (handles both first-event and subsequent events):
```sql
INSERT INTO session_projections (source, session_id, state, updated_at)
VALUES (?, ?, ?, ?)
ON CONFLICT(source, session_id)
DO UPDATE SET state = excluded.state, updated_at = excluded.updated_at;
```

**`event_id` INSERT:** always omit the `event_id` column. Never pass `0` or any
explicit value. AUTOINCREMENT assigns. The schema has no `DEFAULT` on this
column to prevent accidental explicit-zero inserts.

**Error propagation:**
- `thiserror` error types in `protocol` and `shim` crates, and in all internal
  modules of binary crates (`db.rs`, `server.rs`, `ingest.rs`, etc.)
- `anyhow::Context` permitted only in `main.rs` files (the binary entry points)
- `?` throughout all call chains; no `.unwrap()` outside `#[cfg(test)]` code

**Tracing instrumentation:**
```rust
#[tracing::instrument(skip_all, fields(session_id = %session_id))]
async fn handle_event(session_id: &str, ...) { ... }
```
- `skip_all` is the default — prevents payloads, DB handles, and sensitive data
  from appearing in traces
- Specific fields opted in via `fields(...)` syntax only
- Zero tracing on shim (not even `tracing::error!`)
- `#[tracing::instrument]` applied to every async fn crossing a crate boundary

**Bearer token storage:**
```rust
// crates/daemon/src/auth.rs
use secrecy::SecretString;
pub struct BearerToken(SecretString);
```
`secrecy::SecretString` wraps `Zeroizing<String>` and prevents accidental
logging via `Debug`/`Display`. This is a mandatory security control, not a
style preference.

**Unix socket 0600 mechanism:** `umask(0o177)` set before `bind()`, not
`chmod` after bind. This closes the TOCTOU window between file creation and
permission setting.

**WAL checkpoint:** PASSIVE checkpoint on clean daemon shutdown only. No
periodic checkpointing in V1. SQLite's automatic WAL threshold handles routine
operation.

**WS `DroppedFrame` trigger:** emitted when a `tokio::sync::broadcast`
receiver's lag exceeds channel capacity (default 256). Sent on the next
successful delivery to that subscriber after the lag is detected. The channel
never blocks on slow consumers.

**WS wildcard `*` delivery:** delivers all events across all sessions, including
events for sessions that started before the subscribe. Subscribers filter by
session_id in their own logic if needed.

**Integration test fixture:**
- SQLite: `:memory:` per test (not a shared file)
- Unix socket: unique temp path via `tempfile::TempDir` per test, dropped on
  test teardown. Never a fixed path — parallel tests would collide.
- Contract tests testing WAL behavior specifically may use a file-backed SQLite
  in a `TempDir`.

### Enforcement Guidelines

**All AI agents MUST:**
- Run `cargo test --workspace` before marking any story complete
- Run `cargo clippy --workspace --all-targets -- -D warnings` and fix all warnings
- Snapshot-test every wire-format type in `crates/protocol/tests/contract_protocol.rs`
  with `assert_eq!(serde_json::to_string(&EventKind::ToolUse).unwrap(), "\"ToolUse\"")`
  — this is the canonical guard against `rename_all` drift
- Never add `deny_unknown_fields` to any outbound (daemon→client) type
- Never emit exit code 2 from the shim

**Anti-patterns (explicitly forbidden):**
- `unwrap()` / `expect()` outside `#[cfg(test)]` code
- `eprintln!` / `println!` anywhere in shim or daemon
- Splitting the projection UPSERT and event INSERT across separate transactions
- Importing internal submodule paths from `crates/protocol`
- `deny_unknown_fields` on any daemon→client type
- `anyhow::Context` in any module other than `main.rs` files
- Exit code 2 from the shim
- Any explicit `event_id` value in an INSERT statement
- Fixed Unix socket paths in tests

---

## Project Structure & Boundaries

### Complete Project Directory Structure

```
bowerbird/
├── Cargo.toml                          # workspace manifest; members = ["crates/*"] only; docs/cookbook/*/ is a Node project zone, not a Cargo zone (see project-context.md §Example presenters)
├── Cargo.lock                          # committed; reproducible builds
├── rust-toolchain.toml                 # stable channel pin
├── .github/
│   └── workflows/
│       ├── ci.yml                      # build+test+clippy; macOS arm64/x86_64; Linux x86_64
│       └── release.yml                 # prebuilt binary distribution
├── fixtures/                           # versioned test/demo data; single authoritative location
│   ├── hook_pre_tool_use.json          # raw Claude Code hook payloads for shared use
│   ├── hook_post_tool_use.json
│   ├── hook_stop.json
│   └── event_log_sample.db             # SQLite fixture for replay/export demos
├── docs/
│   ├── decisions/                      # ADRs (0001, 0002, 0003, ...)
│   ├── cookbook/                       # self-contained pattern directories; prose README colocated with runnable code
│   │   ├── README.md                   # index of cookbook entries
│   │   ├── state-session-fanout/       # representative entry; rest-cursor-pagination/ and dropped-frame-recovery/ follow the same shape
│   │   │   ├── README.md               # what this is / how to run it / how it works / how to apply it
│   │   │   ├── package.json            # engines.node >= 22.6.0; type: module
│   │   │   ├── package-lock.json       # committed; npm ci in the CI typecheck job depends on it
│   │   │   ├── tsconfig.json           # strict, noEmit (Node strips types at runtime)
│   │   │   └── src/
│   │   │       └── index.ts            # canonical runnable pattern code, smoke-tested in CI
│   │   ├── rest-cursor-pagination/      # same shape as state-session-fanout/
│   │   └── dropped-frame-recovery/      # same shape, plus tests/recover.test.ts
│   ├── quickstart.md                   # 5-minute walkthrough
│   ├── presenter-authoring.md          # conceptual tool-building guide
│   ├── protocol.md                     # wire-surface reference (REST + WS + ingest)
│   ├── no-list.md                      # explicit V1 scope cuts
│   ├── protocol-changelog.md           # protocol change history (CI-enforced)
│   └── bmad/                           # planning artifacts + implementation artifacts
└── crates/
    ├── protocol/                       # stable wire surface; dep of all crates
    │   ├── Cargo.toml
    │   └── src/
    │       ├── lib.rs                  # pub use re-exports of ALL public types
    │       ├── error.rs                # pub enum Error; pub type Result<T>
    │       ├── event.rs                # Event, EventEnvelope, EventId(i64), EventKind
    │       ├── reaction.rs             # Reaction enum; custom Serialize/Deserialize
    │       ├── adapter.rs              # SourceAdapter trait, NormalizeResult, AdapterMeta
    │       ├── constants.rs            # SHIM_BINARY_NAME and other cross-crate string constants
    │       ├── rest.rs                 # EventListResponse, SessionStats; no framework types
    │       └── ws.rs                  # ServerMessage, ClientMessage, all frame types
    │   └── tests/
    │       └── contract_protocol.rs   # wire-format snapshot assertions (pre-MVP gate)
    │
    ├── shim/                           # sync-only static binary; no Tokio; <5ms hot path
    │   ├── Cargo.toml
    │   └── src/
    │       ├── main.rs                 # arg parse → socket::send() → exit 0/1
    │       ├── error.rs
    │       └── socket.rs              # Unix socket write path; timeout; failure logging
    │   └── tests/
    │       └── contract_shim.rs        # exit-code contract; silence-on-success contract
    │
    ├── daemon/                         # Tokio current_thread + axum + SQLite
    │   ├── Cargo.toml
    │   └── src/
    │       ├── main.rs                 # startup; migration; socket bind; axum serve; shutdown
    │       ├── error.rs
    │       ├── config.rs              # socket paths; port; pool sizes; token source;
    │       │                          #   ingest_channel_capacity: usize (default 1024)
    │       ├── state.rs               # AppState { db: DbPools, hub: BroadcastHub,
    │       │                          #             auth: BearerToken, shutdown: CancellationToken }
    │       ├── db/
    │       │   ├── mod.rs
    │       │   ├── migrations.rs       # rusqlite_migration definitions
    │       │   ├── pool.rs             # deadpool-sqlite writer(1) + readers(4)
    │       │   └── queries.rs          # ALL SQL strings live here; no inline SQL elsewhere
    │       ├── ingest/
    │       │   ├── mod.rs
    │       │   ├── listener.rs         # Unix socket accept loop; umask(0o177) before bind
    │       │   └── handler.rs          # receive raw bytes → normalize → projection::write()
    │       ├── projection/
    │       │   ├── mod.rs
    │       │   └── session.rs          # OWNS the transaction: projection UPSERT + event INSERT
    │       ├── api/
    │       │   ├── mod.rs
    │       │   ├── auth.rs             # tower auth layer; bearer token validation (timing-safe)
    │       │   ├── token.rs            # UUID4 token issuance; SecretString wrapping
    │       │   ├── sessions.rs         # GET /sessions
    │       │   ├── events.rs           # GET /sessions/:id/events?since=<cursor>
    │       │   ├── health.rs           # GET /healthz, GET /readyz
    │       │   └── ws.rs              # WS upgrade; subscription router; fan-out; DroppedFrame
    │       └── broadcast/
    │           ├── mod.rs
    │           ├── event.rs            # BroadcastEvent wrapper; channel capacity constants
    │           └── hub.rs              # BroadcastHub; tokio broadcast channels per topic
    │   └── tests/
    │       ├── contract_config.rs      # config round-trip; missing env var; bad pool size
    │       ├── contract_daemon.rs      # ingest → DB → WS fan-out contract tests
    │       └── contract_ws.rs          # WS frame protocol contract tests
    │
    └── adapter-claude/
        ├── Cargo.toml
        └── src/
            ├── lib.rs
            ├── error.rs
            ├── normalize.rs            # normalize(hook_kind, raw) → NormalizeResult
            ├── install.rs              # write settings.json atomically;
            │                          #   uses protocol::SHIM_BINARY_NAME
            └── hooks/
                ├── mod.rs
                ├── pre_tool_use.rs
                ├── post_tool_use.rs
                ├── stop.rs
                └── notification.rs
        └── tests/
            ├── contract_adapter.rs
            └── fixtures/               # adapter-specific payloads ONLY (not shared with root)
                                        # naming: <hook_type>/<scenario>.json
                                        # loaded via include_str!

bowerbird/                              # CLI binary (workspace-root package, not under crates/)
├── Cargo.toml
├── src/
│   ├── main.rs                         # clap entrypoint; anyhow::Context permitted here only
│   └── commands/
│       ├── mod.rs                      # shared helpers (path resolution, daemon-binary discovery, ingest-socket probe)
│       ├── auth.rs                     # `bowerbird auth token` + CLI-side token resolver (mirrors daemon chain)
│       ├── daemon.rs                   # shared helpers: start_daemon_detached, stop_daemon_via_pid_file, hand-rolled HTTP probes
│       ├── export.rs                   # `bowerbird export <session-id>` — fetch /sessions/{id}/events, write JSONL
│       ├── install.rs                  # `bowerbird install` — settings.json merge + daemon spawn
│       ├── replay.rs                   # `bowerbird replay [<file>]` — POST /replay with JSONL body (bundled fixture default)
│       ├── start.rs                    # `bowerbird start`
│       ├── status.rs                   # `bowerbird status` — resolution chain + /status probe + formatted block
│       ├── stop.rs                     # `bowerbird stop`
│       └── uninstall.rs                # `bowerbird uninstall` — settings.json removal + daemon stop
└── (tests at workspace root in tests/cli_*.rs)
```

Workspace-root tests for the CLI:
- `tests/cli_install.rs` — install / uninstall round-trip via real `bowerbird` subprocess
- `tests/cli_lifecycle.rs` — start / stop / status round-trip via real `bowerbird` + `bowerbird-daemon` subprocesses
- `tests/cli_auth.rs` — `bowerbird auth token` via real subprocess + `BOWERBIRD_KEYRING_BACKEND=mock|disable`
- `tests/release_pipeline_docs.rs` — doc-drift guardrails (architecture.md ↔ source defaults, AC walkthrough markers, license metadata)

### Fixture Ownership

| Location | Contains | Used by |
|---|---|---|
| `fixtures/` (workspace root) | Shared hook payloads + demo SQLite | `docs/cookbook/*/` (runtime read by Node via fs.readFile when needed; primary path is `bowerbird replay` which embeds the fixture compile-time), `bowerbird/tests/integration/` |
| `crates/adapter-claude/tests/fixtures/` | Adapter-specific raw payloads | `contract_adapter.rs` only; loaded via `include_str!` |

No overlap. No symlinks. Workspace root fixtures are the single authoritative source for anything shared across crates.

### Architectural Boundaries

**Ingest boundary (shim → daemon):**
- `crates/shim/src/socket.rs` — write path, timeout, failure log
- `crates/daemon/src/ingest/listener.rs` — accept loop (renamed from `socket.rs` to avoid naming collision)
- Newline-delimited JSON wire framing (one `{object}\n` in, one status line out); shim injects `hook_kind` as transport routing but adds no interpretive normalization. See [ADR-0002](../../decisions/0002-ingest-wire-framing-and-hook-kind.md).

**Normalization boundary:**
- `crates/daemon/src/ingest/handler.rs` calls `adapter_claude::normalize()`
- Result passed immediately to `projection::session::write()`

**Transaction boundary (load-bearing):**
- `crates/daemon/src/projection/session.rs` is the SOLE owner of the SQLite transaction
- Projection UPSERT + event INSERT; nothing else joins the transaction
- All SQL strings in `crates/daemon/src/db/queries.rs`

**API boundary:**
- `crates/daemon/src/api/auth.rs` — validation (`subtle::ConstantTimeEq` timing-safe compare against stored token)
- `crates/daemon/src/api/token.rs` — four-step token resolver (env → keychain → config.toml → `Err(TokenError)`); UUID4 generated and stored in keychain on first run; `secrecy::SecretString` wrapping
- `src/commands/auth.rs` — CLI-side token resolver mirroring the daemon chain (CLI does not depend on `crates/daemon` to stay tokio-free)
- `src/commands/daemon.rs` — hand-rolled HTTP/1.1 GET probes against `/healthz` and `/status` over `std::net::TcpStream` (no `reqwest`, no `ureq`)

**Protocol crate boundary:**
- `crates/protocol/src/constants.rs` owns `SHIM_BINARY_NAME` — single authoritative string used by `adapter-claude/src/install.rs`. No duplication across crates.

**Examples boundary:**
- `docs/cookbook/*/` are TypeScript projects on Node 22.6+; the workspace root's `[workspace] members = ["crates/*"]` deliberately excludes them: `docs/cookbook/*/` is a Node project zone, not a Cargo zone
- Hand-write the ~30 lines of TypeScript interface declarations they need per entry (no shared SDK, per project-context.md §Example presenters)
- Consume the WS + REST surfaces via Node's built-in `WebSocket` and `fetch`; no runtime npm dependencies
- Smoke-tested in CI via `tests/cli_examples.rs` (Rust orchestrates daemon + Node subprocess); break loudly on protocol-shape changes via the smoke's stdout-shape assertions

### Requirements to Structure Mapping

| FR Group | Location |
|---|---|
| FR1–FR5: Hook capture + shim | `crates/shim/src/socket.rs`, `crates/adapter-claude/src/hooks/` |
| FR6–FR9: Event persistence | `crates/daemon/src/db/`, `crates/daemon/src/projection/session.rs` |
| FR10–FR17: WS pub/sub | `crates/daemon/src/api/ws.rs`, `crates/daemon/src/broadcast/` |
| FR18–FR23: REST + history | `crates/daemon/src/api/sessions.rs`, `events.rs`, `health.rs` |
| FR24–FR26: Session tracking | `crates/daemon/src/projection/session.rs` (UPSERT) |
| FR27–FR30: Install + lifecycle | `src/commands/{install,uninstall,start,stop,status,daemon}.rs`, `crates/adapter-claude/src/install.rs`, `crates/daemon/src/{singleton,server_file,config_file}.rs` |
| FR31–FR35: Developer tools + examples | `src/commands/{replay,export}.rs` (Story 4.1); `docs/cookbook/*/` (Story 4.2, TypeScript on Node 22.6+; consolidated into `docs/cookbook/` by Story 5.13); `docs/{quickstart,presenter-authoring,protocol,no-list}.md` (Story 4.3) |
| FR36–FR39: Protocol compat | `crates/protocol/` (wire types + constants) |

### Data Flow

```
Claude Code process
  → crates/shim/src/socket.rs (raw JSON) → Unix socket
  → crates/daemon/src/ingest/listener.rs (accept)
  → ingest/handler.rs → adapter_claude::normalize()
  → projection/session.rs (UPSERT + INSERT, same tx)
  → broadcast/hub.rs (fan-out per topic)
  → api/ws.rs (DroppedFrame on lag) → tool WS client

tool REST client
  → api/events.rs → db/queries.rs (reader pool) → EventListResponse

bowerbird CLI (`bowerbird status`)
  → src/commands/status.rs
  → src/commands/auth.rs::resolve_token_for_cli (env → keychain → ~/.bowerbird/config.toml)
  → src/commands/daemon.rs::http_get_status (TcpStream + bearer)
  → crates/daemon/src/api/status.rs (semaphore permit count → DaemonStatus)
```

CLI ↔ daemon discovery uses `~/.bowerbird/{bowerbird.pid, server.json, ingest.sock}`:
- `bowerbird.pid` (Story 3.1 singleton): authoritative liveness — `kill(pid, 0)` probe.
- `server.json` (Story 3.2): publishes the daemon's ephemeral bind-addr; written atomically with mode 0600; best-effort removed on clean shutdown. Hint, not liveness proof.
- `ingest.sock` (Story 1.3): Unix domain socket for hot-path shim writes; CLI uses it as a second-layer liveness probe (`UnixStream::connect` succeeds → daemon is up and accepting).

---

## Architecture Validation Results

### Coherence Validation ✅

**Decision compatibility:** All technology choices are compatible. Tokio
`current_thread` + axum 0.8 + deadpool-sqlite is a well-established combination
with no known conflicts. rusqlite_migration 2.5.0 is compatible with rusqlite
0.39.0. The secrecy/zeroize chain is compatible with keyring v3. clap 4.5
derive is stable.

**Pattern consistency:** The asymmetric serde rule is enforced at every inbound
surface. EventKind uses PascalCase-as-written consistently. `Reaction::Vendor(n)`
serialization has a designated custom impl in `reaction.rs`. The
`thiserror`/`anyhow` boundary is precise: `thiserror` in all library code and
internal modules; `anyhow` only in `main.rs` files.

**Structure alignment:** Every architectural decision has a structural address.
The transaction invariant has a sole owner (`projection/session.rs`). All SQL
is centralized. The shim/daemon naming collision is resolved (`socket.rs` for
shim write path; `listener.rs` for daemon accept loop). Bearer token issuance
and validation are in separate files.

**Runtime constraint:** The `current_thread` runtime means all daemon work runs
on a single OS thread. All SQLite access goes through the deadpool-sqlite pool
— agents must never introduce raw `thread::spawn` for SQLite work.

### Requirements Coverage Validation ✅

| FR Group | Coverage |
|---|---|
| FR1–FR5: Hook capture | shim/socket.rs + adapter-claude/hooks/ + normalize.rs ✅ |
| FR6–FR9: Event persistence | db/ + projection/session.rs atomic transaction ✅ |
| FR10–FR17: WS streaming | broadcast/ + api/ws.rs; DroppedFrame; SyncFrame; HelloFrame ✅ |
| FR18–FR23: REST history | api/sessions.rs + events.rs + health.rs; EventListResponse ✅ |
| FR24–FR26: Session tracking | projection/session.rs UPSERT; no stuck state on missing PostToolUse or Stop (5-min stale-Working fallback); **Story 5.3**: daemon-observed liveness via 5s `kill(pid, 0)` probe → `SessionEnded`; `Notification → WaitingInput` is typed-`notification_type`-driven; `PostToolUse → Working` unconditionally ✅ |
| FR27–FR30: Install/lifecycle | commands/daemon.rs + adapter-claude/install.rs + config.rs ✅ |
| FR31–FR35: Developer tools | replay.rs + export.rs + docs/cookbook/*/ TypeScript projects + fixtures/ ✅ |
| FR36–FR39: Protocol compat | protocol/ wire types + additive serde + CHANGELOG CI gate ✅ |

**NFR coverage:** Shim p95 <5ms → criterion benchmark gate. Daemon 2s readiness
→ readyz. WAL durability → rusqlite WAL mode on startup. ENOSPC → log + close.
Keychain + env-var + file fallback chain defined. `unsafe_code = "deny"`
workspace-wide (downgraded from `forbid` in Story 5.3 for shim `libc::getppid()`;
the single inline `#[allow(unsafe_code)]` is the only opt-in). ✅

### Implementation Readiness Validation ✅

**Decision completeness:** 19 dependencies pinned with exact patch versions. All
5 crates have defined internal module layouts to file level. `AppState` fields
explicitly named. Transaction owner explicitly designated. Error type contract
explicit per-crate. `ingest_channel_capacity` backpressure constant in `config.rs`.

**Structure completeness:** Complete directory tree to individual file level.
Fixture ownership table. Data flow diagram. FR-to-structure mapping table.
Boundary descriptions for every inter-crate surface.

**Pattern completeness:** 18 explicit process decisions. Wire format snapshot
testing mandated. `skip_all` tracing policy. Exit code semantics (0/1/never-2).
Shim binary name constant in protocol. Named integration test files.

**Resolved during Story 1.3 implementation:** Ingest socket wire framing is
newline-delimited JSON (one `{object}\n` request, one status-line response).
Ratified by [ADR-0002](../../decisions/0002-ingest-wire-framing-and-hook-kind.md).

### Architecture Completeness Checklist

**Requirements Analysis**
- [x] Project context thoroughly analyzed
- [x] Scale and complexity assessed (Medium; single-developer; local tool)
- [x] Technical constraints identified (stable toolchain; no async in shim; single-writer SQLite; 127.0.0.1 bind)
- [x] Cross-cutting concerns mapped (performance isolation; atomicity; protocol stability; observability; security; error propagation)

**Architectural Decisions**
- [x] Critical decisions documented with versions (19 pinned deps; all 5 crates specified)
- [x] Technology stack fully specified (Rust stable; Tokio current_thread; axum 0.8; SQLite WAL; deadpool-sqlite; clap 4.x)
- [x] Integration patterns defined (SourceAdapter trait; WS pub/sub; REST cursor-based)
- [x] Performance considerations addressed (shim hot-path rules; benchmark gate; no speculative optimization)

**Implementation Patterns**
- [x] Naming conventions established (18 explicit decisions; table by context)
- [x] Structure patterns defined (test placement; error.rs contract; protocol re-exports)
- [x] Communication patterns specified (WS frame enum; ClientMessage; serde asymmetry; DroppedFrame trigger)
- [x] Process patterns documented (transaction invariant; exit codes; tracing skip_all; bearer token type)

**Project Structure**
- [x] Complete directory structure defined (file-level for all 5 crates + docs/cookbook/ entries + fixtures)
- [x] Component boundaries established (ingest; normalization; persistence; API; broadcast)
- [x] Integration points mapped (data flow diagram; boundary descriptions)
- [x] Requirements to structure mapping complete (FR group → file table)

### Architecture Readiness Assessment

**Overall Status: READY FOR IMPLEMENTATION**

All 16 checklist items confirmed. No critical gaps remain. Two party mode
rounds surfaced 14 distinct gaps across steps 5 and 6; all resolved. The
document now specifies architecture to a level where independent agents will
make the same structural choices on any given decision point.

**Confidence level: High**

**Key strengths:**
- Transaction invariant has a sole designated owner — the most common
  correctness failure point is structurally addressed
- Shim exit code semantics verified against Claude Code hook documentation
- `Reaction::Vendor(u16)` escape hatch allows external adapters without
  protocol crate changes
- `recording_sessions` shadow table + sentinel events give gap detection
  without client-side inference
- 19 dependency versions pinned; `Cargo.lock` committed

**Post-V1 enhancements (deferred by design):**
- Linux systemd service integration
- `bowerbird gc` event-log truncation
- V2 adapter contract (subprocess model)
- Rate limiting on TCP surface
- Non-loopback TCP bind option
- `Reaction::Vendor(n)` → named variant graduation (two-presenters rule)

### Implementation Handoff

**AI agent guidelines:**
- Follow all architectural decisions exactly as documented
- The anti-pattern list is a code review gate — treat violations as build failures
- All SQL strings go in `db/queries.rs`; all wire types come from `crates/protocol`
- The transaction in `projection/session.rs` is the load-bearing correctness
  invariant — never split it

**First implementation story:** Initialize the workspace scaffold, verify all
19 crate version pins compile, get `cargo check --workspace` green.
