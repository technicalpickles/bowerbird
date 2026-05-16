---
title: "Product Brief Distillate: bowerbird"
type: llm-distillate
source: "product-brief-bowerbird.md"
created: "2026-05-11"
purpose: "Token-efficient context for downstream PRD creation"
---

# Product Brief Distillate: bowerbird

Dense structured context for PRD and architecture workflows. Each bullet is standalone.

---

## Core Philosophy (Load-Bearing)

- **Axiom 1 — Substrate observes; it does not interpret.** Any PR that adds application-level concepts (personas, priorities, sentiment, urgency) to the daemon is a scope violation, not a code review. This is the tiebreaker for every design decision.
- **Axiom 2 — Small at two scopes.** Each crate is tightly scoped to one job (split rather than extend). The overall project refuses new responsibilities that fail Axiom 1.
- **Axiom 3 — Performance is hard at trust boundaries, soft inside.** < 5ms is a non-negotiable *spirit* for the shim (which runs in Claude's process). Daemon budgets are negotiable.
- **Axiom 4 — Mechanical facts in the protocol; semantics in the presenter.** `oldest_available_event_id` (fact the daemon knows) is fine. `gap_detected: true` (a presenter conclusion) is not.
- **One normalization only:** tool name → 11-value reaction enum. No other normalization ever enters the daemon.

---

## Technical Stack (Decided)

- **Language:** Rust everywhere (protocol, shim, daemon, adapter-claude). Edition 2021, stable toolchain.
- **Async runtime:** Tokio `current_thread` in daemon. **Zero Tokio in the shim** — runtime init alone is 1-3ms.
- **HTTP/WS:** axum (built on hyper + tower). `axum::Router` + `tower::ServiceExt::oneshot` for in-process testing without port binding.
- **Storage:** SQLite via `rusqlite` with `bundled` feature. WAL mode, `synchronous=NORMAL`. Single-writer.
- **Connection pool:** `deadpool-sqlite` (async, wraps `spawn_blocking`). Two explicit pools: writer (`max_size=1`), readers (`max_size=4`).
- **Wire format:** JSON via serde. Versioned as `protocol@vN`.
- **Adapter configs:** TOML (`serde_yaml` is archived; TOML matches Cargo ecosystem).
- **Error handling:** `thiserror` in protocol + shim (library/hot-path). `anyhow` only at daemon binary edge (HTTP handlers, `main`).
- **Observability:** `tracing` + `tracing-subscriber`. Shim logs to file (`~/.bowerbird/shim.log`), never stdout/stderr.

---

## Architecture (Proposed)

```
crates/
  protocol/          # stable wire surface — public API, changes need ADR
  shim/              # static binary, < 5ms p95, no async, no alloc on hot path
  daemon/            # Tokio single-thread, axum, SQLite, WS pub/sub
  adapter-claude/    # reference adapter; normalizes Claude Code hooks to protocol
adapters/
  claude/            # TOML data: capabilities, tool-reactions, settings-merge
examples/            # tested in CI; cookbook references these
docs/
  design/            # design rationale
  decisions/         # ADRs (format defined in project-context.md)
  cookbook/          # how-to recipes for presenter authors
  no-list.md         # explicit non-targets
```

---

## WS Pub/Sub Design (Proposed)

- One `tokio::sync::broadcast::Sender<EventEnvelope>` per channel (`events.*`, `state.*`). Capacity ~1024.
- Per-client task subscribes via `BroadcastStream`, filters for client's topic, sends via WS sink.
- `BroadcastStream` lag → exactly one `dropped` frame (with lag count), then continues. Socket does NOT close on lag.
- **Correction:** no per-client `mpsc` between broadcaster and WS sink — `tokio::sync::mpsc` has no drop-oldest semantic. `broadcast` end-to-end is the correct shape.
- WS ping/pong: per-client task spawns `tokio::time::interval(30s)` arm in `select!`. Without this, FIN-less TCP drops leak tasks.
- WS concurrency cap: `Semaphore` in `AppState`, default 256. Prevents fork-bomb-by-runaway-presenter.

---

## Performance Bars (Proposed, CI-enforced)

| Bar | Target | Notes |
|---|---|---|
| Shim exit (warm cache) | < 5ms p95 | Marginal cost of *our* code, not wall-clock. Measured separately on macOS vs Linux — don't average. |
| Hook → projection (daemon) | < 50ms p95 | WAL + `synchronous=NORMAL` makes this achievable. |
| Hook → presenter (end-to-end) | < 100ms p95 | Bounded by slowest presenter unless per-subscriber queues drop on overflow. |
| Daemon idle CPU | < 0.5% | Define "idle" precisely; WS keepalive cadence matters. |
| Daemon RSS | < 50MB | Easy at v1; sample from day one. |
| Core LOC | 5K–7K | Alarm at 10K. |

Bench thresholds: p99 (not p95) for shim — the tail is what users feel. CI fails on +15% shim regression, warns on +5%.

---

## Required Framework Infrastructure (Pre-MVP, Non-Optional)

- `AppState { db: DbPools, broadcasters: Broadcasters, auth: TokenStore, shutdown: CancellationToken }` in `Arc` via `State<Arc<AppState>>`
- `CancellationToken` (tokio-util) propagated to every spawned task
- Graceful shutdown: stop accepting WS, send `close` frames, drain channels (5s timeout), flush DB, exit code 0
- `CatchPanicLayer` from `tower-http` (single-threaded runtime — panic kills the whole process)
- Request-ID middleware (`SetRequestIdLayer` + `PropagateRequestIdLayer`)
- Timeouts: `TimeoutLayer` on HTTP (30s); not on WS
- Body-size limits: `RequestBodyLimitLayer`

---

## Open Questions (Must Resolve Before Code Lands)

- **Shim-when-daemon-down:** Direct SQLite write, fire-and-forget POST with drop-on-failure, or inotify-driven spool? Cascading effects on shim binary, daemon startup, and "lost data" definition.
- **Protocol-level gap detection:** Sequence numbers + last-seen cursor on reconnect. Required to make `synchronous=NORMAL` an honest choice.
- **MSRV:** Pin `rust-version = "x.y"` in each `Cargo.toml` before workspace is committed.
- **Time and ID types:** `SystemTime` / `chrono` / `time` for timestamps; UUIDv7, ULID, or monotonic int for event IDs. Propagate through wire format and schema.
- **Auth-token storage:** File at `~/.bowerbird/server.json` is the baseline. Keychain via `keyring` crate is the upgrade path.
- **Event-log truncation policy:** Append-only forever (disk growth), bounded by row count, bounded by age, manual `bowerbird gc`? Affects gap-detection behavior.
- **Reference SDK for presenters:** Ship `@bowerbird/presenter` (TypeScript) or keep the protocol simple enough that no SDK is needed? Current lean: no SDK; revisit when the first real presenter reveals the plumbing-to-feature ratio.
- **Adapter contract shape:** Is a third-party adapter a Rust crate, a subprocess speaking JSON-lines, or a config-driven TOML entry? Determines who can contribute one.
- **Cookbook anchor tooling:** mdBook `{{#include}}`, hand-rolled `// cookbook-begin:` markers, or something else? Decide before the second cookbook entry.
- **AGENTS.md naming:** `CONTRIBUTING.md` (doubles for AI agents), or split into `CONTRIBUTING.md` + `docs/agent-handoff.md`?

---

## Rejected Ideas (With Rationale — Don't Re-Propose)

- **YAML for adapter configs:** `serde_yaml` is archived (March 2024). Community fork `serde_yml` has maintenance issues. TOML chosen instead.
- **`sqlx` over `rusqlite`:** No network round trips to overlap in single-embedded SQLite; `sqlx`'s async pool and compile-time query checks buy nothing. `rusqlite` with `bundled` feature pins a known SQLite version.
- **`r2d2_sqlite` connection pool:** Sync; blocks the Tokio runtime on `get()`. `deadpool-sqlite` wraps `spawn_blocking` internally — correct shape.
- **Per-client mpsc between broadcaster and WS sink:** `mpsc` has no drop-oldest semantics. Rejected in favor of `broadcast` end-to-end.
- **Global `features = ["full"]` for Tokio:** Enables unnecessary features. Explicit list required: `["rt", "macros", "net", "io-util", "sync", "time", "signal", "fs"]`.
- **Bare `magpie` as project name:** Taken on crates.io (Othello engine, 23K downloads). `earshot`, `wiretap`, `vigil`, `patchbay` all also taken.
- **`agent-state-bus` / `claude-state-bus` as project names:** Rejected — `claude-*` prefix implies single-agent; `agent-*` is too generic. Both lack brand coherence.
- **Windows support:** Explicit scope cut. No way to test locally; better to scope-cut than ship broken.
- **Distro packaging (Debian, Arch, nixpkgs):** Community-driven if it happens. Not a maintainer commitment.
- **HITL backflow / tool blocking / personas / LAN/multi-host / daemon-side activity-rate:** All explicit non-targets per the no-list. Any feature requiring the daemon to "act" rather than "observe" is a scope violation.
- **Hand-rolling `PRAGMA user_version` migrations:** Party review rejected deferred migration tooling. `rusqlite_migration` from day one — one Cargo.toml line; people don't switch later.

---

## Contribution Model (Decided)

- New issues and PRs from new contributors are **auto-closed by default**. Maintainer reviews queue weekly.
- Fast-track: bug reports with repro, new adapter PRs, doc fixes.
- Required path for new features: GitHub Discussion first, then PR.
- Presenter and extension authors publish downstream; bring *findings* upstream (not code).
- Response-time targets: 7 days for bug reports + adapter PRs + doc fixes; 14 days for feature Discussions; 72 hours for security reports.
- Maintainer: pickles (sole maintainer by design — small surface area > contributor throughput).

---

## Naming: bowerbird (Decided)

Selected over ~170 candidates across 10 naming categories. Rationale:
- Bowerbirds collect bright objects and arrange them in their bower for display — maps exactly to "preserve native payload verbatim" (collect) + "presenters render" (display).
- Namespace clean across crates.io, npm, PyPI, Homebrew, GitHub dev-tools space.
- Metaphor extends naturally: SQLite event log = bower (`~/.bowerbird/bower.db`), subscribers = visitors.
- Runners-up: `magpie-d` (daemon convention), `bystander` (philosophy-first, encodes substrate-not-actor literally).

---

## Testing Requirements (High-Priority Contract Tests)

The following are required before MVP — each validates a protocol or correctness claim the project makes publicly:

- WS dropped-frame behavior (lag → exactly one `dropped` frame, socket stays open)
- PRAGMA invariants on every connection checkout (foreign_keys, journal_mode, synchronous)
- Connection factory enforcement (CI lint: no raw `Connection::open` outside factory)
- State-emission + event-INSERT atomicity (SIGKILL during load → restart → projection matches event log)
- Graceful shutdown (SIGTERM mid-ingest → exit code 0 → in-flight event committed or rolled back cleanly)
- Cursor-gap detection (truncate log, `?since=N` → response contains `oldest_available_event_id`)
- Atomic `~/.claude/settings.json` install (interrupt simulation → original file still valid JSON)
- Hook unreliability tolerance (`PreToolUse` without matching `PostToolUse` → sane state, not stuck)
- Outbound envelope additive-compat (extra unknown field round-trips through `protocol` without error)
- Shim parser fuzz (`cargo-fuzz`, 60s per PR budget)

---

## Scope Signals (In/Out/Maybe)

| Feature | Status | Notes |
|---|---|---|
| Claude Code adapter | In (MVP) | Reference implementation |
| TypeScript/Node example presenters | In (MVP) | CI smoke-tested |
| REST snapshot API | In (MVP) | Cursor-based pagination; backing for WS `snapshot` frame |
| Health/readiness endpoints | In (MVP) | `/healthz` (liveness), `/readyz` (DB + migrations + broadcasters) |
| Second agent adapter (Codex/Gemini/Cursor) | Out (MVP) | Validates adapter model but not a v1 commitment |
| Presenter SDK (`@bowerbird/presenter`) | Maybe | Deferred; revisit after first real presenter reveals boilerplate ratio |
| `/metrics` endpoint (Prometheus) | Maybe | Path reserved; implementation deferred |
| `bowerbird gc` (event-log truncation) | Open | Truncation policy not yet decided |
| arm64 CI runner | If budget allows | Different fork/exec timing surfaces shim budget surprises |
| Windows | Out (explicit) | Hard scope cut |
| Multi-host / LAN bind | Out (explicit) | Hard scope cut; changes auth model substantially |
