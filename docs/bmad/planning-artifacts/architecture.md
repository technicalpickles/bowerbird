---
stepsCompleted:
  - step-01-init
  - step-02-context
  - step-03-starter
  - step-04-decisions
  - step-05-patterns
inputDocuments:
  - docs/bmad/planning-artifacts/prd.md
  - docs/bmad/project-context.md
workflowType: 'architecture'
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
  hook events with < 5ms p95 marginal latency; no stdout/stderr on any path;
  logs failures to `~/.bowerbird/shim.log` (mode 0600); adapter normalizes
  payloads to canonical protocol format with raw payload preserved verbatim.
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
  missing `PostToolUse`).
- **Installation & Configuration (FR27–FR30):** Prebuilt binaries +
  `cargo install`; daemon lifecycle commands; status/version CLI.
- **Developer Tools & Experience (FR31–FR35):** `bowerbird replay` and
  `bowerbird export`; three reference examples (multi-session router, event
  log viewer, reconnect recovery); bundled fixtures; full documentation path.
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
- Process supervision: launchd (macOS V1); manual on Linux V1

**Important Decisions (Shape Architecture):**
- deadpool-sqlite writer(max=1) + readers(max=4) pool split
- Asymmetric serde `deny_unknown_fields`
- SourceAdapter trait: sync + pure `normalize()`; `Reaction::Vendor(u16)` escape hatch
- CLI framework: clap 4.x with derive macro
- Keyring crate: `keyring` v3

**Deferred Decisions (Post-MVP):**
- Linux systemd service integration
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
- Token storage: `keyring` v3 (system keychain primary → `BOWERBIRD_TOKEN` env-var → `~/.bowerbird/token` file mode 0600)

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

**WebSocket:**
- Upgrade at `GET /ws`; bearer auth on upgrade
- Topic filtering: session_id or wildcard subscriptions
- Fan-out: tokio broadcast channel per topic; slow consumer receives `DroppedFrame`; channel never blocks
- Max 256 concurrent WS connections; 257th receives defined rejection

**Protocol serde:**
- Inbound: `deny_unknown_fields` — strict
- Outbound: permissive — additive forward-compat guaranteed
- `Event.payload: String` — verbatim raw JSON

**Error handling:**
- `thiserror` in `protocol` + `shim`; `anyhow` at binary edges only
- HTTP errors: `{ "error": "<message>" }` with appropriate status code

**Rate limiting:** None in V1; documented limitation.

### Frontend Architecture

Not applicable.

### Infrastructure & Deployment

**Process supervision:**
- macOS V1: `bowerbird daemon install` writes a launchd plist to `~/Library/LaunchAgents/` and
  runs `launchctl load`; `bowerbird daemon uninstall` reverses this
- Linux V1: manual invocation only; systemd integration is post-V1

**CLI framework: clap 4.x with derive macro**
- Subcommands: `daemon` (start/stop/status/install/uninstall), `replay`, `export`, `version`

**Distribution:**
- Prebuilt binaries: macOS arm64, macOS x86_64, Linux x86_64 (glibc)
- `cargo install` as alternative
- `Cargo.lock` committed

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
8. CLI binary — clap subcommands, launchd plist install/uninstall

**Cross-component dependencies:**
- `protocol` is a dep of all crates; every change has maximum blast radius — dep budget tightest here
- `shim` depends on `protocol` only; zero daemon deps — enforced by Cargo dep graph
- `daemon` depends on `protocol`; normalizes via `SourceAdapter` at the ingest boundary
- launchd integration depends on stable daemon binary path from the distribution step

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
- `0` on success
- `1` on any failure (daemon down, write error) — non-blocking warning; Claude continues
- `2` is **forbidden** — exit 2 blocks Claude tool calls, which violates the
  substrate-not-actor axiom

**Shim wire format:** shim writes raw hook JSON verbatim to the Unix socket.
No normalization in shim. Daemon calls
`adapter_claude::normalize(hook_kind, raw) -> Result<NormalizeResult>`.

**Shim hot-path rules (non-negotiable):**
- No heap allocation on the success path (best-effort; enforced via criterion
  benchmark with p95 < 5ms CI gate, not a compile-time guarantee)
- No `unwrap()` or `expect()` anywhere in shim
- No `eprintln!` / `println!` / `tracing` calls — silence on success path;
  failures write to `~/.bowerbird/shim.log` only

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
