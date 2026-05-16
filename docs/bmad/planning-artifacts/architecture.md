---
stepsCompleted:
  - step-01-init
  - step-02-context
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
