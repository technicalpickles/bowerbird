---
project_name: bowerbird
user_name: pickles
date: 2026-05-11
status: complete
project_status: pre-mvp
framing: 'current-thinking-with-motivations'
sections_completed:
  - technology_stack
  - language_specific
  - framework_patterns
  - testing
  - code_quality
  - workflow
  - critical_dont_miss
source_basis:
  - docs/research/AGENTS-draft.md
  - docs/research/README-draft.md
  - docs/research/12-mvp-and-milestones.md
  - docs/decisions/0001-project-name.md
  - party-mode reviews (3 rounds) 2026-05-11
optimized_for_llm: true
update_protocol: 'every ADR declares Affects context.md sections: field'
---

# Project Context for AI Agents

_This file captures the **current thinking** behind how bowerbird should be built, with the reasons and motivations behind each choice. It is not a list of locked-in commandments._

**How to read this file.** bowerbird is pre-MVP. Most details below are proposals sourced from the design corpus in `docs/research/` and refined in a party-mode review. The point of writing them down isn't to freeze the decisions, it's to make sure the *reasoning* travels with the choice so that when reality pushes back (and it will), the next revision is made with full context instead of guessing.

**Status legend used below:**

- **Decided.** Locked by an ADR or external constraint. Don't revisit without a new ADR.
- **Proposed.** Current design intent. Likely to ship as-is, but open to revision when code meets reality.
- **Open.** Identified as a real question with no answer yet. Decide before the relevant code lands.

---

## Project axioms

These are the rules every other rule derives from. When two specific rules seem to contradict, the axiom is the tiebreaker.

> **Axiom 1: The substrate observes; it does not interpret.**
>
> Anything that turns raw data into application-level concepts (personas, voices, sprites, priorities, urgency, sentiment) is a *presenter* concern. The daemon's job is to preserve and expose; the presenter's job is to interpret. A PR that proposes adding such a concept to the daemon is a discussion, not a code review.

> **Axiom 2: Small at two scopes, not one.**
>
> "Small" applies *per-component* (each crate is tightly scoped to one job, pi-mono-style — split rather than extend) AND *overall* (the project refuses new responsibilities that don't fit Axiom 1). These compose: the overall gate filters what the substrate takes on; per-component discipline determines how it's structured. A crate that outgrows its job gets split. A new responsibility that fails Axiom 1 gets refused.

> **Axiom 3: Performance is hard at trust boundaries, soft inside.**
>
> The shim runs in someone else's process; a stall there destroys the user's coding session. That's a *hard contract* — the <5ms-ish target is non-negotiable in spirit even if the exact number is. The daemon's ingest, WAL flush, presenter query path are all in *our* process. Those budgets are negotiable in service of clarity. Don't treat every perf number as equally load-bearing — ask which side of the trust boundary it sits on.

> **Axiom 4: Mechanical facts in the protocol; semantics in the presenter.**
>
> The daemon emits cursor data, event payloads, reaction enum values. It does not emit *interpretations* of those values. `oldest_available_event_id` (mechanical fact) is fine. A `gap_detected: true` flag (presenter semantics — "you should react") is the camel's nose. Every protocol field should pass the test: is this a fact the daemon knows, or an instruction the presenter would derive?

---

**Cross-cutting preferences (derived from the axioms):**

- Performance budgets are negotiable inside trust boundaries; non-negotiable across them. Per Axiom 3.
- Prefer one code path over a branch, even when each branch handles a real case. The project's value comes from being small and legible.
- This project is optimized for **small surface area over contributor throughput** — name this honestly. Every workflow rule below follows from that trade.

---

## Maintenance model for this file

This document describes a moving target. It needs an explicit update protocol or it silently becomes the stale front door (Quinn's catch).

The rule:

- **Every merged ADR includes an `Affects context.md sections:` field** (see ADR format below). The value is either a list of sections, or the literal string "none."
- If the ADR affects sections, the same PR updates context.md in those sections. The CI gate that already enforces "protocol/src/*.rs change → doc touch" extends to "ADR landed → matching context.md sections touched."
- If the same-PR update isn't feasible, the ADR explicitly notes "context.md is now stale on section X until the next pass" — making the staleness visible rather than silent.

The trigger never reads "we'll update this when we feel like it." Drift is the most expensive kind of documentation failure: it makes both humans and AI agents wrong simultaneously.

---

## Technology Stack & Versions

### Core language: Rust — Decided

Rust everywhere for the core crates (`crates/protocol`, `crates/shim`, `crates/daemon`, `crates/adapter-claude`). Edition 2021. Stable toolchain.

**Why:** The shim's performance target (see Performance bars) requires a language that compiles to a static binary with no runtime and predictable allocation. Node and Python don't meet the bar. Mixing two languages in the same workspace adds friction for contributors and AI agents, so the daemon is Rust too. One protocol crate, two binaries, no drift.

**Open under this:**
- **MSRV.** "Stable" is a moving target. Pin a floor (e.g. `rust-version = "1.81"` in each `Cargo.toml`) and bump deliberately. Decide before workspace `Cargo.toml` is committed.
- **Edition 2024.** Available now. 2021 has broader ecosystem support today; "boring" wins for v1.

### Daemon async: Tokio, single-threaded — Decided

Single-threaded runtime (`current_thread`) by default.

**Why:** A local daemon serving a handful of presenters and one adapter is not contention-bound. Single-threaded means no `Send` bounds rippling through types, no accidental data races, lower scheduler overhead. The risk vector is N slow WebSocket clients blocking the single thread — mitigated by per-client send tasks with their own bounded `mpsc` (slow client backpressures itself, not the broadcaster). If profiling ever shows contention, switching to `rt-multi-thread` with `worker_threads = 2` is a one-line change.

**Tokio features to pin** (don't enable `features = ["full"]`):
`["rt", "macros", "net", "io-util", "sync", "time", "signal", "fs"]`. Skip `rt-multi-thread`. Skip `process` unless the daemon spawns children (it shouldn't).

**Shim gets zero Tokio.** Runtime init alone (~1-3ms) would consume the shim's budget. Sync I/O, blocking writes, `std::process::exit`.

**Recommended test discipline (from party review):** add a CI lane that runs the daemon test suite on a multi-threaded runtime even though prod is single-threaded. If tests pass there, ordering invariants are real, not accidental.

### HTTP + WebSocket: axum — Proposed

`axum` (built on `hyper` + `tower`) for the daemon's HTTP and WS surface.

**Why:** Two converging arguments:

1. **Test surface.** `axum::Router` + `tower::ServiceExt::oneshot` lets the whole HTTP/WS stack be exercised in-process without binding a port. Hyper-only would mean ephemeral-port test harnesses (flaky on CI) or hand-rolled scaffolding.
2. **Ergonomics for a daemon-sized service.** Extractors, middleware (tracing, request IDs), and `axum::extract::ws::WebSocketUpgrade` give the canonical pub/sub pattern (`tokio::sync::broadcast` per channel, fan out per-connection in the WS handler) without reinventing it.

Raw `hyper 1.x` saves ~200 LOC and one dep but loses both above. Not justified for this scope.

**Confirm with a 50-line spike** (WS handshake + one REST route) before promoting to Decided.

**Pub/sub channel sizing:** `broadcast::channel` capacity is a per-channel ring buffer. Default to 1024; size up for worst slow-presenter burst. Drop slow consumers via `RecvError::Lagged`.

### Storage: SQLite via rusqlite — Decided

WAL mode, `synchronous=NORMAL`. Single-process, single-writer.

**Why:**
- Local single-host daemon. No Postgres-style server process to install.
- WAL gives concurrent readers while the daemon writes.
- `synchronous=NORMAL` is the standard "I trust the OS not to crash mid-fsync" setting. `FULL` would cost 3-10x on write latency and threaten the hook-to-projection target. `OFF` risks corruption.
- The `sqlite3` CLI is a free debugger for the user.
- Alternatives (sled, redb) have smaller ecosystems and no out-of-band debug tooling.

**`rusqlite` over `sqlx`:** Single embedded SQLite has no network round trip to overlap; `sqlx`'s async pool and compile-time query checking buy nothing here. `rusqlite` with the `bundled` feature pins a known SQLite version (WAL guarantees depend on it).

**Required `rusqlite` features:** `["bundled", "backup", "blob"]` minimum; add `serde_json` if JSON blobs land in columns.

**Other PRAGMAs to set:** `journal_size_limit`, `wal_autocheckpoint`, `mmap_size`. Run `PRAGMA optimize` on connection close. Use a connection pool (`r2d2_sqlite` or hand-rolled) — opening a connection per request is 1-2ms wasted.

**Open under this:**
- **Gap-detection in the protocol.** `synchronous=NORMAL` accepts "last few events lost on hard crash." For an observability substrate, that's only acceptable if presenters can *detect* the loss on reconnect. Proposed mechanism: monotonic sequence numbers on events plus a "last seen" cursor on reconnect — if the daemon's sequence has jumped, the presenter knows to refetch state. **This is a protocol concern, not a storage concern.** Resolve before v1 ships.
- **Migrations.** `rusqlite` doesn't ship migrations. Hand-rolled `PRAGMA user_version` is fine for v1; document the approach so it's not reinvented badly. `refinery` and `rusqlite_migration` are the off-the-shelf options.
- **Time and IDs.** `SystemTime` vs `chrono` vs `time`? Event IDs UUIDv7, ULID, or monotonic integers? These propagate through the wire format and schema. Name the choices before the first row is written.

### Shim-when-daemon-is-down: ??? — Open

The design corpus proposed shim→spool-to-disk→daemon-picks-up-on-startup. Pickles dislikes the resulting fork: the shim has two code paths (daemon up vs down), and the daemon has to poll the spool directory.

**Alternatives on the table:**

1. **Shim writes to SQLite directly** (possibly a separate write-heavy DB the daemon also reads). One code path in the shim; daemon doesn't poll, it just reads. Cost: shim now links SQLite and writes hot — non-trivial perf and binary-size impact. Concurrent writers + WAL is supported but adds lock contention.
2. **Shim always POSTs; if daemon down, retry briefly then give up.** Drops events on failure. Acceptable if "missed events" is a tolerable failure mode for v1 (probably true for a developer tool).
3. **Spool, but daemon uses inotify/FSEvents to react** (no polling). Solves the polling concern but keeps two write paths.

**Resolve before:** the shim's first event-emit code lands.

**Why this is open and not Proposed:** the choice has cascading effects on the shim's binary, the daemon's startup behavior, and what counts as "lost data." Pick it deliberately.

### Errors: anyhow + thiserror, scoped — Proposed

Split by crate, not blanket:

- **`protocol` crate:** `thiserror` only. Library crate — `anyhow` is a smell in a library because it erases type information and breaks consumer-driven contract testing.
- **`shim` crate:** `thiserror` only. Five-ish error variants total (config read, socket connect, write, timeout, malformed args). `anyhow::Error` allocates on every `?` conversion; the shim's hot path can't afford it.
- **`daemon` crate:** `thiserror` for crate-internal errors; `anyhow` permitted at the binary edge (HTTP/WS handler return types, top-level `main`).
- **`adapter-claude` crate:** same as daemon — `thiserror` internal, `anyhow` at the edge.

**Why:** Standard idiom, but the protocol+shim restriction matters more here than it would for a typical service.

### Wire format: JSON via serde — Decided

JSON wire format. Serialize/Deserialize on all public protocol types. Versioned via `protocol@vN` namespace.

**Why:** JSON isn't the most efficient format, but it's debuggable from `curl` and parseable from any language. For a substrate consumed by lots of small presenters, that matters more than throughput. JSON also leaves the SDK target language open — any language with a JSON parser can build a presenter.

**Asymmetric `deny_unknown_fields` policy** (party-review convergence):

- **Inbound from clients (REST requests, WS subscribe messages, adapter YAML/TOML configs): strict.** `#[serde(deny_unknown_fields)]`. Catches typos and version skew loudly.
- **Outbound to presenters (event envelopes, state frames): permissive.** Presenter deserialize types do NOT use `deny_unknown_fields` — that's how v1.1 ships a new field without breaking v1.0 presenters.
- **Shim → daemon ingest: permissive deserialize on the daemon side.** Shim might be ahead during a rolling upgrade.

This is the inverse of Postel and it's the right call here because additive forward-compat is the substrate's whole value prop. Encode the asymmetry in a test that asserts an outbound envelope with an extra field round-trips through `protocol`.

**Additive v1.x outbound fields (live examples of the policy):** `cwd` (`SessionState` / `SessionListItem` / `Event`) and `started_at` (`SessionState` / `SessionListItem`, state/list-only) were added under v1.0 → v1.1 (Story 5.7, ADR 0006) as `Option<T>` fields on outbound types. A pre-5.7 presenter decodes a frame carrying them and silently drops them; a pre-5.7 projection blob lacking them deserializes to `None`. No version bump, no blob rewrite — exactly the additive forward-compat this policy exists to enable.

**Additive v1.x inbound field (the inverse direction):** `ClientMessage::Subscribe.states` (Story 5.8, ADR 0008) is an optional `Vec<String>` snapshot-scoping filter added with `#[serde(default)]`, so a v1.0 presenter omitting it still parses under the strict-inbound `deny_unknown_fields` (the field is *known*; a typo'd *other* key still 1008-closes). The known forward-compat edge: a *newer* presenter sending `states` to an *older* daemon is rejected (WS close 1008) — the acceptable "client ahead of daemon" direction.

### Adapter configs: TOML — Decided

TOML for `adapters/<source>/*.toml` (capabilities, tool-reactions, settings-merge templates). Schemas in `crates/protocol/schemas/`.

**Why** (changed from YAML during party-mode review):
- `serde_yaml` is archived (dtolnay deprecated it March 2024). The community fork `serde_yml` has shaky maintenance and dependency drama.
- YAML's significant whitespace, type coercion (`yes` → `true`, `1.10` → `1.1`), and Norway problem all bite users.
- TOML is dtolnay-maintained, has a stricter grammar, and matches the Rust ecosystem (it's what `Cargo.toml` already is).
- Adapter configs are small, flat-ish, human-edited — TOML's sweet spot.

### Example presenters: TypeScript on Node — Proposed

TypeScript, runs on Node. Lives in `examples/`. No build step beyond `tsc`.

**Why:** Most presenter authors reach for Node first; that's where the docs land. The substrate doesn't care what speaks WebSocket+JSON, so this is purely an *example-language* choice, not a protocol constraint. (Earlier draft said "Bun-compatible" — that's a deployment detail; Node is the actual target.)

**Open under this:**
- **Reference SDK question.** The README promises "80-150 line presenters" but the first 30 lines of any real presenter today would be WS connect + reconnect-with-backoff + token loading + topic subscribe + heartbeat. Options: (a) ship `@bowerbird/presenter` so presenters skip the plumbing, (b) make the raw protocol simple enough that no SDK is needed. Pickles leans (b) — see if the protocol can be tight enough that an SDK is overkill. Resolve when the first reference presenter exists and the plumbing-to-feature ratio is visible.

### Tooling: shell, minimal — Proposed

Shell kept narrow — install/uninstall and CI glue (GitHub Actions). Anything more complex goes in Rust.

**Why:** Shell scripts in a Rust workspace tend to grow into unmaintained logic. Soft budget: 200 lines of shell total. If we blow it, that's the signal to move to `just` or a Rust binary.

**Cross-platform hazards to watch:** macOS bash 3.2 vs Linux bash 5 behave differently. Run `shellcheck` in strict mode in CI. Test the install script on both runners.

### Observability: tracing — Proposed

`tracing` + `tracing-subscriber` in the daemon. Structured logging with span context.

**Why:** Standard for tokio-based services. Presenter authors debugging integration issues get better signal than `log` + `env_logger`. Tracing is also what `tower-http` and most axum middleware already emit into.

**Shim logging exception:** the shim does not log to stdout/stderr on the success path (Claude's hook environment shouldn't see daemon-internal noise), but it should log to a file (e.g. `~/.bowerbird/shim.log` with rotation). Failures must be visible somewhere; "silently exits on error" makes the shim untestable in the field.

### Release profile for the shim — Proposed

The shim's binary must be small and fast to load:

```toml
[profile.release-shim]
inherits = "release"
panic = "abort"
lto = "fat"
codegen-units = 1
opt-level = "z"   # or "s"; benchmark which is smaller and still meets the time budget
strip = true
```

Plus `#[deny(unsafe_code)]` at every crate root and a committed `Cargo.lock` (yes, even for library crates — reproducibility matters when we're claiming a perf budget).

---

## Repository layout (target)

**Proposed.** Not realized in code yet.

```
bowerbird/
├── AGENTS.md               # project rules (currently a draft in docs/research/)
├── crates/
│   ├── protocol/           # stable wire surface (public API)
│   ├── shim/               # static binary, fast exit on hot path
│   ├── daemon/             # long-running service (sqlite + ws + rest)
│   └── adapter-claude/     # reference adapter
├── adapters/
│   └── claude/             # TOML data files (capabilities, tool-reactions)
├── examples/               # tested in CI; cookbook entries reference these
└── docs/
    ├── design/             # design rationale (currently in docs/research/)
    ├── decisions/          # ADRs for load-bearing choices
    ├── cookbook/           # how-to recipes for presenter authors
    └── no-list.md          # what we deliberately don't do
```

**Why this shape:** The protocol crate is the stable surface — anything in it is part of the public API and changes need an ADR. The shim is isolated because its rules are radically different from the daemon. One adapter per crate keeps adapter-specific logic contained.

---

## Performance bars — Proposed

Targets, not contracts. Willing to ease any of these if the relaxation makes the project easier to maintain or use.

| Bar | Current target | Notes |
|---|---|---|
| Shim exit (warm cache) | <5ms p95 | Measured as marginal cost of *our* code, not wall-clock from Claude's perspective (process spawn on macOS is 3-8ms before we run). Document what the number measures. Cold-vs-warm is being treated as warm; cold-start scenarios get measured separately if they become user-visible. |
| Hook → projection (daemon) | <50ms p95 | Achievable with WAL + `synchronous=NORMAL`. `FULL` likely misses it. |
| Hook → presenter (end-to-end) | <100ms p95 | Bounded by slowest connected presenter unless per-subscriber queues drop on overflow. Drop policy is a protocol concern. |
| Daemon idle CPU | <0.5% | Define "idle" precisely (zero presenters? one connected doing nothing?). WS keepalive cadence can blow this. |
| Daemon RSS | <50MB | Easy at v1; sample in bench harness from day one so future regressions are visible. |
| Core LOC | 5K-7K | Alarm at 10K. |

**Measurement infrastructure to build before perf claims are made:**

1. `shim/benches/hot_path.rs` with Criterion, gating CI on p95 regression (not just printing numbers).
2. End-to-end bench: daemon up, fire N synthetic hooks via the shim, measure time-to-projection and time-to-WS-frame.
3. RSS + CPU sampling during the e2e bench, asserted against bars.
4. Burst-shape bench (Claude emits hooks in clumps around tool calls; uniform throughput tests miss the real load).
5. Soak test on main: 1 hour, modest event rate, RSS must not drift.

---

## Durability and chaos — Proposed

What happens when things go wrong:

- **Daemon crashes mid-write.** WAL handles in-flight writes. The state-emission-and-event-INSERT-must-be-same-txn rule (from AGENTS-draft) is what keeps the projection consistent. Validate with a `SIGKILL`-during-load test that asserts projection matches event log on restart.
- **Disk fills.** `SQLITE_FULL` is the failure mode. Daemon must crash cleanly with a clear log line, not hang. Shim's behavior depends on the still-Open "shim-when-daemon-down" choice above.
- **Settings.json corruption (bowerbird's own config).** Read → validate → atomic-swap (write `.tmp`, rename). Reject and fall back to last-good if validation fails.
- **Claude's settings.json mid-edit (during `bowerbird install`).** Must be atomic-replacement. Fuzz the partial-read case.
- **Hook delivery is not reliable** (Claude can drop hooks if the shim is slow or killed). The protocol's gap-detection signal (Open question above) is what makes this visible to presenters.
- **Daemon down after a reboot drops every event until restart** (dogfood Finding 1, 2026-06-01). On **macOS** the daemon is supervised by a launchd LaunchAgent (`com.technicalpickles.bowerbird.daemon`) installed by `bowerbird install`: `RunAtLoad=true` brings it back on login/reboot and `KeepAlive={SuccessfulExit=false}` restarts it on crash (non-zero exit) while leaving a clean `bowerbird stop` (graceful exit 0) down. Per ADR 0007. On **Linux**, supervision stays manual for V1 (systemd deferred); the shim still never blocks Claude, so a down daemon costs events, not the coding session.

### Health-check endpoint — Proposed

A dedicated `GET /healthz` (or similar) that returns daemon liveness without requiring the caller to hit a session/event endpoint.

**Why:** lets external watchdogs (launchd, systemd, a status-bar dot, a presenter) poll for "is the daemon alive" without coupling to data shape. Cheap to add; high optionality.

---

## CI — Proposed

**GitHub Actions** is the CI. Minimum matrix: macOS-latest + Linux-latest (ubuntu). x86_64 + arm64 if budget allows. No Windows runner (see Scope cuts).

CI must:
- Run `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test --workspace`.
- Compile all benches (`cargo bench --no-run`).
- Run the shim hot-path bench and fail on p95 regression.
- Run the end-to-end smoke test for every example presenter in `examples/`.
- Run `shellcheck` on every committed shell script.

**Platform divergence hazards to watch:** process spawn timing differs across macOS and Linux runners (report perf separately, don't average them); APFS is case-insensitive by default, ext4 isn't; signal/process-group behavior differs; WAL breaks on network filesystems (`~/.bowerbird` on SMB — detect and refuse, or document loudly).

---

## Scope cuts (explicit)

These are deliberate non-targets. Calling them out so they're not silently revisited:

- **No Windows support.** No way to test it locally; better to scope-cut than ship something broken. Don't gratuitously write Windows-hostile code (path separators, line endings) — someone may port it later — but don't pay for it either.
- **No distro packaging.** Homebrew (macOS) + `cargo install` is the distribution surface. Debian/Arch/nixpkgs are community-driven if they happen at all.
- **No HITL backflow, no tool blocking, no personas, no LAN/multi-host, no daemon-side activity-rate.** From the design corpus. See `docs/no-list.md` (to be created).

---

## Critical Implementation Rules

### Language-Specific: Rust + serde

#### Shim hot-path discipline (`crates/shim`) — Decided

The shim runs once per Claude Code hook event. Its goal is to never block Claude. The rules cascade from that goal — not from the 5ms number itself, which is a means, not an end.

- **No async runtime.** No Tokio in the shim. Sync I/O, blocking writes, `std::process::exit`. Tokio's runtime init alone is 1-3ms.
- **No allocation on the success path.** Use `&str` and stack buffers; serialize directly to the network buffer. `Vec::push` and `String::from` allocate.
- **No `anyhow`.** A `thiserror` enum with a small fixed set of variants (config-read fail, socket-connect fail, write fail, timeout, malformed args). `anyhow::Error` allocates on every `?` conversion.
- **No `unwrap` / `expect` on per-event code paths.** Setup-time `expect` in `main` is acceptable when the alternative is uglier; the per-event hot path returns typed errors.
- **No structured logging on the success path.** Logging adds work to the path that's supposed to be invisible.
- **Logging on failure goes to a file**, not stdout/stderr. The shim runs inside Claude's hook environment — anything on stdout/stderr risks polluting Claude's experience. Proposed path: `~/.bowerbird/shim.log` with size-based rotation.
- **No config load at runtime.** Defaults compiled in. A single small override file is read once at startup if it exists.
- **No subprocess on the hot path.** No `git`, no `tmux`, no anything. All enrichment happens daemon-side where the cost amortizes.
- **`#![deny(unsafe_code)]` at the crate root.** No exceptions.

**Why these are Decided and not Proposed:** every one of them follows from "the shim must never block Claude." Negotiating any of them individually loses sight of the constraint they enforce together.

**Release profile** (from Performance bars section above): `panic = "abort"`, `lto = "fat"`, `codegen-units = 1`, `opt-level = "z"` or `"s"`, `strip = true`. Commit `Cargo.lock`.

#### Daemon style (`crates/daemon`, `crates/adapter-claude`) — Proposed

- **No `unwrap` / `expect` in production code paths.** Tests can unwrap freely. `main` setup can `expect` with a clear message when the alternative is less debuggable.
- **Module size cap: ~800 lines.** If a module crosses that, split it. The cap is a smell detector, not a hard rule — when it triggers, the question is "why is this one module doing so much?"
- **One module per concern.** Examples from the design corpus: `projection.rs`, `pubsub.rs`, `storage.rs`, `ingest.rs`.
- **Internal types stay internal.** If a type doesn't cross the WebSocket or REST surface, it does NOT belong in `crates/protocol`. When in doubt, internal.
- **SQLite writes go through `spawn_blocking`.** Don't block the single-threaded runtime on disk I/O. Use a connection pool (`r2d2_sqlite` or hand-rolled) — opening a connection per request is 1-2ms wasted.
- **Per-client send-task pattern for WebSocket fanout.** Each WS connection gets its own task that owns the sink, fed by a bounded `mpsc`. Slow consumers backpressure their own mpsc, not the daemon's broadcast channel. Drop-oldest on overflow with a `dropped` frame so the presenter knows to refetch state.
- **No `println!` / `eprintln!` in shipped code.** Use `tracing::{info, debug, warn, error}`.
- **`anyhow` only at the binary edge.** HTTP/WS handler return types, top-level `main`, error reporting. Crate-internal errors use `thiserror`.

#### Protocol crate (`crates/protocol`) stability — Decided

This crate is the public API. Every change to it is a coordination cost across the daemon, the shim, and every presenter.

- **`thiserror` only. No `anyhow`.** Library crates that erase error types break consumer-driven contract testing.
- **All public types implement `Serialize` + `Deserialize`.** No exceptions.
- **Versioned via `protocol@vN` namespace.** `v2` is a parallel module tree, not an in-place change. Within a version, only additive changes.
- **Asymmetric `deny_unknown_fields` policy** (from Wire format section above, restated as a rule):
  - Types parsed from clients/configs: strict.
  - Types emitted to presenters: permissive.
  - Write a test that asserts an outbound envelope with an extra field round-trips through `protocol` without error. That test is the canary for the asymmetry.
- **No new dependencies that aren't already in the daemon.** Adding a dep to `protocol` adds it to every consumer. The dep budget tightens here.
- **`#![deny(unsafe_code)]` at the crate root.**

#### Crate-wide invariants — Decided

Applied at every crate root:

- `#![deny(unsafe_code)]`. The substrate runs against the user's filesystem and process tree. No unsafe blocks anywhere; no exceptions.
- `rust-version = "x.y"` in each `Cargo.toml` (MSRV pinned; bump deliberately). The actual floor is Open (see Open questions).
- `Cargo.lock` committed. Including for library crates. We claim a perf budget; reproducible builds back that claim up.
- `cargo fmt --check` and `cargo clippy --all-targets --workspace -- -D warnings` gate CI. Warnings are errors.

#### Error discipline (crate matrix) — Decided

| Crate | Internal errors | Edge errors |
|---|---|---|
| `protocol` | `thiserror` | n/a (library) |
| `shim` | `thiserror` (small enum) | n/a (exits with code) |
| `daemon` | `thiserror` | `anyhow` at HTTP/WS handler return types and `main` |
| `adapter-claude` | `thiserror` | `anyhow` at the daemon-facing boundary |

The intent: typed errors where callers might branch on the failure mode; `anyhow` only where the next thing the program does is log-and-bail.

### Framework patterns

The framework choices (Tokio + axum + rusqlite + tracing) are covered in the Stack section. This subsection captures the *patterns* on top of those choices that a fresh agent would miss.

#### WebSocket pub/sub topology — Proposed

```
                    ┌──────────────────────────┐
ingest task ──────► │ broadcast::Sender<Event> │ ── (events channel)
                    └────────────┬─────────────┘
                                 │ subscribe
                       ┌─────────┼─────────┐
                       ▼         ▼         ▼
                  ┌─────────┐ ┌─────────┐ ┌─────────┐
                  │ client  │ │ client  │ │ client  │
                  │ task A  │ │ task B  │ │ task C  │   ← each owns a BroadcastStream + filter
                  └────┬────┘ └────┬────┘ └────┬────┘
                       ▼           ▼           ▼
                  WebSocket   WebSocket   WebSocket
```

- One `tokio::sync::broadcast::Sender<EventEnvelope>` per channel (one for `events.*`, one for `state.*`). `tokio::sync::broadcast` already has overwrite-oldest semantics — no second hop needed. Capacity ~1024; tune from burst measurements, not intuition.
- Per-client task subscribes via `BroadcastStream` (`tokio-stream`), `.filter`s for the client's topic subscription, sends through the WS sink.
- `BroadcastStream` surfaces channel lag as `Err(BroadcastStreamRecvError::Lagged(n))`. Per-client task maps that to exactly one `dropped` frame (with the lag count) and continues — it does NOT close the socket on lag.
- Subscription filtering happens in the per-client task, not in the broadcaster.

**Correction from earlier draft:** an earlier version of this section had a per-client `mpsc` between the broadcaster and the WS sink with "drop-oldest on `try_send` Full." That doesn't work — `tokio::sync::mpsc` is FIFO with no drop-oldest semantic; `try_send` returns `Err(Full(value))` and you keep the rejected message. If a hand-rolled drop-oldest queue is ever needed (e.g. ordering guarantees that `broadcast` can't provide), it's a custom `VecDeque<EventEnvelope>` + `Notify` behind a `Mutex` in the per-client task — not an mpsc. For now, `broadcast` end-to-end is the simpler shape.

**WS ping/pong:** axum's WS doesn't auto-ping. Per-client task should spawn a `tokio::time::interval(Duration::from_secs(30))` arm in `select!` to send `Ping`. Pong handled in the recv arm. Without this, FIN-less TCP drops leak tasks forever.

#### Daemon → SQLite — Proposed

- Pool: **`deadpool-sqlite`**. It's async (wraps `spawn_blocking` internally) and exposes `interact(|conn| …)`. `r2d2_sqlite` is sync and blocks the runtime on `get()` — wrong shape for tokio.
- **Two pools, explicitly:** `writer` with `max_size = 1`, `readers` with `max_size = 4`. Let the pool enforce single-writer; don't rely on the SQL layer alone. WAL gives concurrent readers + one writer; the pool topology must match.
- **Connection factory is the only public path to a `Connection`.** Module-private constructor, no raw `rusqlite::Connection::open` calls outside it. CI lint (`grep`/clippy) forbids the raw call. This is how we keep the PRAGMA invariants from being silently bypassed.
- **Connection-init hook on BOTH pools** sets every PRAGMA: `journal_mode = WAL`, `synchronous = NORMAL`, `foreign_keys = ON`, `busy_timeout = 5000`. `foreign_keys` is the canary — it is NOT on by default in SQLite and forgetting it silently disables `REFERENCES` clauses. A test asserts `PRAGMA foreign_keys` returns 1 on every checkout.
- **Migrations: `rusqlite_migration` from day one.** Earlier draft proposed hand-rolling `PRAGMA user_version` with "switch later." Party review pushed back: people don't switch, and the migration story is the worst place to discover a data-integrity bug. `rusqlite_migration` is lighter than `refinery` and adopting it now is one Cargo.toml line.
- Long-running statements: prepare-once, reuse via `Connection::prepare_cached`. Note that the cache is per-connection — 4 readers means 4 caches.
- **Panic discipline in `spawn_blocking`:** every `spawn_blocking` is awaited via a helper that logs `JoinError` with `tracing::error!` and converts to a structured `DbError::TaskPanic`. Don't `.unwrap()` the join; the calling task dies and whatever was waiting on it hangs.

#### Tracing usage — Proposed

- Use `tracing::instrument` on ingest, projection, and WS-handler entry points. Get span context for free in logs.
- **`#[instrument(skip_all, fields(...))]` is mandatory on hot paths.** Without `skip_all`, every argument gets `Debug`-formatted into the span on every call — that allocation shows up in flamegraphs at thousands of events per second.
- **`#[instrument(level = "debug")]` on the hottest inner functions** so the span is compiled out at the default `RUST_LOG=info`.
- Standardized span fields: always `source`, `session_id` (when known), `event_id` (when known). Debugging is grep-driven and inconsistent names cost time.
- `tracing-subscriber` configured from `RUST_LOG` (or a `--log` flag). Default level `info`; bench/CI runs use `warn` to keep output readable.

**Level policy** (sharpened from "no `info!` in hot loops"):

| Level | What goes here |
|---|---|
| `trace` | Loop-body detail. Off in CI; on for targeted debugging. |
| `debug` | Per-event work in the daemon's hot loops. Off by default. |
| `info` | Lifecycle events: connection open/close, migration applied, shutdown initiated, daemon started, adapter loaded. |
| `warn` | Recoverable problems: dropped/lagged WS frames, slow consumer disconnected, malformed message ignored, retry happening. |
| `error` | Caller-visible failures: handler returning an error to a client, panic caught, DB unreachable. |

The default `info` level should produce a steady, readable trail of *what the daemon is doing*, not a flood of per-event noise.

#### HTTP surface — Proposed

**Health endpoints — split** (party-review refinement):

| Endpoint | Auth | Returns |
|---|---|---|
| `GET /healthz` | none | `{"status":"ok"}` — only that the process is up and can respond. |
| `GET /readyz` | none | Readiness: DB reachable, migrations applied, broadcasters initialized. |
| `GET /status` | bearer | Version, uptime, connected presenters, last event time, anything useful for an operator. |

Splitting liveness from readiness lets a supervisor restart on `/readyz` failure without flapping on transient `/healthz`. Putting version behind auth means a misconfigured non-loopback bind doesn't leak fingerprintable info.

**Bind address:** explicitly `127.0.0.1`, not `0.0.0.0`. Document it in config and in code. Non-loopback bind is a separate ADR that must reconsider the bearer-token model (a world-readable file is not auth on a shared host).

**Auth:** all non-`/healthz` and non-`/readyz` endpoints require the bearer token from `~/.bowerbird/server.json`. Token is rotated per daemon start (Pixel Agents pattern).

**Data endpoints:** `GET /sessions`, `GET /sessions/:id`, `GET /sessions/:id/events?since=<cursor>`, `GET /sessions/:id/stats`. These exist both for polling-based presenters and as the implementation backing for the WS `snapshot` frame. Each session carries `last_pid` (Story 5.3) plus `cwd` and `started_at` (Story 5.7) — `cwd`/`started_at` ride `SessionListItem` and `SessionDetail.state` (and `cwd` also rides each `Event` in `/sessions/:id/events`); all are mechanical facts, with repo/age derivations left to presenters (Axiom 4, ADR 0006). `GET /sessions` accepts optional `?state=<csv>` (read-time `current_state` tokens, filtered in Rust), `?since=<updated_at_ms>` (exclusive recency bound, SQL), and `?limit=<n>` (SQL row cap) filters — all default-unfiltered, invalid values `400` (Story 5.8, ADR 0008); the presenter expresses intent and the daemon filters by the mechanical `current_state`, never deciding relevance on its own (Axiom 1). The matching WS surface is the optional `Subscribe.states` snapshot filter.

**Cursor-based pagination on `events`:** `since=<event_id>`. The event log is append-only and monotonically growing.

**Gap-detection: mechanical fact, no semantics** (per Axiom 4). The protocol emits the *facts a presenter needs to detect a gap*, but does NOT emit interpretations.

- Mechanical fact (substrate): every `events` response includes `oldest_available_event_id`. This is unambiguous data the daemon already knows.
- Mechanical fact (substrate): every event carries a monotonic `event_id`.
- Presenter concern (NOT in the protocol): a `gap_detected: true` boolean flag. That's a derived interpretation — `client_cursor < oldest_available_event_id` is a comparison the presenter can do in one line. The daemon doing the comparison would smuggle "you should react" semantics into a substrate response.

The presenter SDK (if one ever ships) can derive `gap_detected` client-side. The daemon's contract is "I tell you what's available; you decide whether that means anything." This is the same line drawn elsewhere: native payloads verbatim, only one normalization (tool→reaction), no presenter concepts in the projection.

**Reserved (not implemented in MVP):** `GET /metrics` for Prometheus-compatible counters. Reserve the path now so we don't have to negotiate it later. ADR notes that metrics will ship via `tracing-opentelemetry` rather than a direct `prometheus` dep.

#### Required framework infrastructure — Proposed

These aren't optional polish — they're what makes the daemon shippable. Add them before declaring the framework section "done."

- **`AppState` shape.** One `AppState { db: DbPools, broadcasters: Broadcasters, auth: TokenStore, shutdown: CancellationToken }` wrapped in `Arc`, accessed via `State<Arc<AppState>>`. Don't pass individual `Arc`s through extension types — that's not the canonical axum 0.7 shape.
- **`CancellationToken`** from `tokio-util` propagated to every spawned task. Leaf tasks observe shutdown by `select!`-ing on the token.
- **Graceful shutdown.** `axum::serve(listener, app).with_graceful_shutdown(shutdown_signal())`. `shutdown_signal()` selects on `tokio::signal::ctrl_c()` and `SIGTERM` (`tokio::signal::unix::signal(SignalKind::terminate())`). On signal: stop accepting new WS connections, send `close` frame to existing clients, drain broadcast channels with a 5s timeout, flush + close DB pools, exit. Test: SIGTERM mid-ingest, assert exit code 0 and the in-flight event is either fully committed or fully rolled back.
- **`CatchPanicLayer`** from `tower-http`. Single-threaded runtime means a panic in any handler kills the whole tokio runtime if not caught. This is one `.layer()`.
- **Request-id middleware.** `tower-http::request_id::SetRequestIdLayer` + `PropagateRequestIdLayer`, tied into the tracing span via `tower-http::trace::TraceLayer`. One layer chain. Skipping it makes "correlate this client bug report to the daemon logs" guesswork.
- **Timeouts.** `tower::timeout::TimeoutLayer` on the HTTP router (30s ceiling). Not on WS — those are long-lived.
- **Body-size limits.** `tower-http::limit::RequestBodyLimitLayer`. Default-deny large bodies; the ingest endpoint sets its own higher cap explicitly.
- **WS concurrency cap.** Cap concurrent WS clients (e.g. `Semaphore` in `AppState`, default 256). Six lines; prevents fork-bomb-by-runaway-presenter.
- **Backpressure counters** (reserved for `/metrics` later, but track them internally now): `ws_broadcast_lag`, `ws_client_queue_depth`, `ws_dropped_total`, `db_write_queue_depth`, `db_spawn_blocking_pool_active`. The first time a "presenter feels laggy" report lands, these are the only way to answer it.

#### Adapter file layout (per-source) — Proposed

```
adapters/<source>/
├── capabilities.toml      # what this source supports (capabilities matrix lands in M4)
├── tool-reactions.toml    # tool name → reaction enum mapping
└── settings-merge.json    # template for the source's settings file (e.g. ~/.claude/settings.json)
```

Plus a Rust crate:

```
crates/adapter-<source>/
├── Cargo.toml
└── src/
    ├── lib.rs             # public adapter API
    ├── hooks.rs           # hook config installation/removal
    ├── ingest.rs          # event normalization (native → canonical)
    └── projection.rs      # tool-name → reaction enum lookup
```

Each adapter's README documents the source-specific oddities (Codex uses TOML for its own settings; OpenCode requires a plugin instead of a hook shim). The adapter-authoring guide (`docs/adapter-authoring.md`) is the entry point.

#### Cookbook discipline — Proposed

Examples in `examples/` are the source of truth. Cookbook entries explain them. **Do not hand-copy snippets** — they rot.

But: a bare hyperlink ("see `examples/lamp-pulse/`") is the wrong execution. Readers want code on the page, not a tab they have to open. Use one of:

- **mdBook-style include directives** with line anchors: `{{#include examples/lamp-pulse/src/main.rs:13:38}}`.
- **Marked regions**: `// cookbook-begin: signal-subscribe` … `// cookbook-end` plus a tiny build step that inlines them at doc-build time.

Either way: examples are still the canonical code, cookbook entries still get inlined snippets, drift breaks the build (CI fails if any anchor goes missing).

**Reference by function name, not line number.** `see fan_out_with_backpressure() in examples/ws-fanout.rs`. Line numbers drift; function names don't (and renames fail the build via include anchors).

**`cargo build --examples` runs in CI on every PR.** This is what keeps `examples/` from quietly desyncing. `cargo test --examples` if examples carry test-style assertions.

**Cookbook entry shape:**
1. **Problem**: one paragraph stating what the presenter wants to do.
2. **Approach**: which substrate signals and why.
3. **Code**: inlined snippets via include/anchor — anchored on functions, not line numbers.
4. **Variants**: one or two notes on adapting the pattern.

**Length target: ~80-150 lines.** The number is a smell, not a rule. The actual test is "one entry = one question the reader had." If you can't name the question in a sentence, it's two entries.

**Reader-path through the docs** (added per party review — separate from the reference triangle):

```
Quickstart (lamp turns green in 5 minutes)
   │
   ▼
docs/presenter-authoring.md  (now you understand the pieces)
   │
   ▼
docs/protocol.md             (now you need the details)
   │
   ▼
docs/cookbook/               (now you have a specific problem)

(adapter-authoring.md is a separate journey for adapter contributors)
```

The Quickstart works against a *recorded or faked* signal stream so the reader doesn't need Claude Code or a real adapter to see a lamp turn green. That's the lowest-friction entry point for the audience the README promises to serve.

### Testing rules

#### Test types and where they live — Proposed

| Type | Lives in | What it covers |
|---|---|---|
| Unit | `mod tests` alongside the code | Pure functions, type-level invariants, parsing |
| Integration | `crates/<crate>/tests/` | One crate at a time, against its public surface |
| End-to-end | `examples/` smoke tests in CI | Full daemon + shim + presenter, real WS |
| Bench | `crates/<crate>/benches/` (Criterion) | Performance regression gating |
| Doc | `///` examples | Cheap, runs as part of `cargo test`; use where the example is the explanation |

Tests use `tempfile::tempdir()` for SQLite, not `:memory:`. `:memory:` doesn't exercise WAL, and the WAL guarantees are part of what we test.

#### Required contract tests — Decided

Each row below is a test that proves a *protocol or correctness contract* the project claims. If any of these regresses silently, the project ships a lie. They're required before MVP.

| Contract | Test |
|---|---|
| WS dropped-frame behavior | Spawn broadcaster (cap 1024), push 2000 events before client `recv()`, assert next stream item is `Err(Lagged(n))`, assert client emits exactly one `dropped` frame, assert next legitimate event arrives, assert socket stays open. Use `tokio::test(start_paused = true)` if timing is involved — no `sleep()`. |
| PRAGMA invariants on every connection | Acquire from each pool (writer + each reader); assert `PRAGMA foreign_keys = 1`, `PRAGMA journal_mode = wal`, `PRAGMA synchronous = 1` (NORMAL). |
| Connection factory enforcement | CI grep/clippy lint forbids `rusqlite::Connection::open` outside the factory module. Test that the factory is the only path. |
| State-emission and event-INSERT atomicity | SIGKILL the daemon mid-load; on restart, assert projection rows and event-log rows agree. No half-state. |
| Graceful shutdown | Send SIGTERM mid-ingest; assert exit code 0; assert in-flight event was either fully committed or fully rolled back. |
| Cursor-gap detection | Insert 100 events, truncate first 50, request `?since=10`; assert response contains `oldest_available_event_id = 50` and `gap_detected: true`. |
| Atomic `~/.claude/settings.json` install | Simulate an interrupt during `bowerbird install`; assert the original settings.json is still valid JSON and not partially overwritten. |
| Hook unreliability tolerance | Fire `PreToolUse` without a matching `PostToolUse`; assert projection still reaches a sane state (not stuck in `working`). |
| Outbound envelope additive-compat | Round-trip an outbound envelope with an extra unknown field through `protocol`; assert no error. This is the canary for the asymmetric `deny_unknown_fields` policy. |
| `(source, session_id)` collision safety | Insert events with identical `session_id` but different `source`; assert they're treated as distinct sessions. |
| Backpressure escalation policy | Sustain client lag past the drop-frame threshold for 30s; assert the daemon coalesces (or disconnects) per policy, doesn't emit 50k individual `dropped` frames. Tests the *policy*, not just the *mechanism*. |
| WAL checkpoint under load | Writer doing ~1k inserts/sec while a reader holds a long transaction; assert WAL file doesn't grow unbounded and `wal_checkpoint(TRUNCATE)` actually reclaims. Catches the bug that only shows up at hour 6 of soak. |
| Projection rebuild from event log | Delete the projection table, restart, rebuild from the event log; assert byte-identical to pre-delete state. This is the "is the event log actually the source of truth" test. |
| Shim parser fuzz | `cargo-fuzz` on the shim's input boundary. A panic in the shim is a Claude Code outage. 60s budget per PR, longer nightly. |
| Cross-version protocol upgrade | Start daemon vN, write events, shut down, start daemon vN+1 against the same data dir; assert no data loss and additive-compat works. Every release without this is a coin flip. |
| Connection-factory lint self-test | A fixture file that violates the "no raw `Connection::open` outside factory" rule; assert CI fails on it. Lints rot silently — test the lint. |
| Tracing field emission | Drive the daemon through ingest → projection → WS-handler boundaries; assert key spans/fields (`source`, `session_id`, `event_id`) are emitted. Tracing regressions are invisible until you need them. |

#### Performance benches — Proposed

| Bench | Gates | Notes |
|---|---|---|
| `shim/benches/hot_path.rs` | shim p95 vs target | Criterion with regression alarm, NOT print-only. Stores baselines in CI artifacts. |
| End-to-end hook→projection | daemon p95 vs target | Spawns daemon, fires N synthetic hooks via shim, measures time-to-projection. The only bench that catches integration regressions. |
| End-to-end hook→presenter | full pipeline p95 | Same, with a connected WS client. Measures time to WS frame arrival. |
| Burst load | Resilience under realistic shape | Claude emits hooks in clumps around tool calls; uniform-throughput tests miss the real load. |
| RSS + CPU sampling during e2e | Daemon memory + idle CPU targets | Asserts the bars (50MB / 0.5%) — track baselines in CI. |
| Soak (1 hour, modest rate) | RSS drift = leak | Doesn't run per-PR; runs on `main`. Catches unbounded channels and slow leaks. |

Bench rule: **a bench that prints numbers but doesn't fail the build doesn't gate anything.** Use Criterion's regression detection wired to fail CI.

**Bench thresholds — differentiated, per-platform, with committed baselines** (per Murat):

| Bench | Tail metric | Fail threshold | Warn threshold |
|---|---|---|---|
| Shim hot-path | p99 | +15% regression | +5% |
| Hook → projection | p99 | +20% | +10% |
| Hook → presenter | p99 | +20% | +10% |
| Burst-load throughput | sustained ops/sec | -10% | -5% |
| Daemon RSS | peak | +25% | +10% |

**p99, not p95** for the shim — the tail is what users feel as "Claude feels slow today."

**Per-platform baselines.** Linux x86_64 CI is not macOS arm64 laptop. Averaging across runners hides regressions that only hit one platform.

**Baselines are committed files**, updated deliberately by PR with reviewer sign-off. Auto-rolling baselines silently absorb 1% regressions until you've lost 30%.

**Soak-smoke on PR, real soak on `main`** (per Murat). The 1-hour soak runs nightly on `main`. A *5-minute* smoke-soak runs on every PR: RSS sampled at 5min, fail if `RSS(5min) / RSS(0)` exceeds a small threshold. Catches gross leaks before they hit main. Soak failures must auto-file an issue with the commit range — otherwise nightly soak rots into "yeah it's been red for a week."

**Known untested risk paths** (document so they're not silently assumed away):
- WAL on real disk vs. tmpfs. CI `tempdir` is often tmpfs, which has different fsync semantics than a real block device. Until a CI job forces a real block device (e.g. `TMPDIR=/var/tmp` on Linux) and a chaos test injects fsync failures, "WAL guarantees on actual disk" is a known gap.
- Disk-full chaos. SQLite's behavior under `SQLITE_FULL` has sharp edges. Loopback-filesystem or `ulimit`-based test recommended; not landed yet.
- Network-fs detection. WAL on NFS/SMB is a corruption generator. Detect-and-refuse logic needs its own test before it can be relied on.

#### Deterministic test discipline — Proposed

- **No real `sleep()`** for synchronization in tests. Use `tokio::test(start_paused = true)` + `tokio::time::advance` for time-dependent assertions.
- **No sleeps in CI workarounds.** A test that "works locally but is flaky on CI" should fail loudly, not get a `sleep(100ms)` patch.
- **Property tests** via `proptest` for invariants where the input space is non-trivial — projection determinism, ring-queue invariants if any get hand-rolled, monotonic event-id ordering.
- **Snapshot tests** via `insta` for wire-format serialization. Cheap way to catch unintentional JSON shape changes.
- **`unwrap()`/`expect()` is fine in tests.** The production discipline doesn't extend to tests; test code reads better with `unwrap`.

#### Multi-threaded test lane — Proposed

Prod runs single-threaded Tokio. A multi-threaded CI lane is worth having ONLY if it includes tests that *only make sense* multi-threaded (per Murat). Just flipping the runtime flavor and re-running existing tests catches nothing.

Required tests for this lane to earn its keep:

- **Concurrent subscribers, identical ordering.** N WS clients subscribed to the same channel, broadcast K events, assert every client received them in the same order.
- **Two-writer race.** Two tasks racing on event-id generation (during shim → daemon ingest); assert no gaps and no duplicates in the assigned IDs.
- **Reader during writer SIGKILL.** A reader holds a transaction; SIGKILL the writer; assert reader either sees the consistent pre-kill snapshot or fails cleanly. Pins rusqlite + WAL behavior.

If we don't write those tests, drop the lane and save the CI minutes.

#### Cross-platform CI matrix — Proposed

| Runner | Required | Notes |
|---|---|---|
| macOS-latest | yes | Primary target |
| ubuntu-latest | yes | Secondary; catches platform divergence early |
| arm64 | if budget allows | Different fork/exec timing; surfaces shim-budget surprises |
| Windows | no | Explicit scope cut (see Scope cuts) |

Per-platform perf bars: don't average across runners. macOS M-series and Linux x86_64 are far enough apart on process-spawn that one bar would either be wrong on one or unenforceable on the other.

#### Examples-as-tests — Proposed

`cargo build --examples` runs on every PR. If an example doesn't compile, the build fails. `cargo test --examples` if examples carry test-style assertions.

Every example presenter has a smoke test in CI:
1. Spawn a fresh daemon (via the test harness).
2. Run the example as a subprocess.
3. Feed synthetic events through the shim path.
4. Assert the example produced the expected output (e.g. "lamp received state.session.X.current_state = working").

The smoke test is what keeps cookbook entries honest. If `cargo build --examples` passes but the example doesn't actually work end-to-end, the smoke test is the catch.

#### Coverage philosophy — Proposed

No coverage percentage target. Coverage is a smell detector, not a goal.

- Every contract in the "Required contract tests" table is covered.
- Every public API has at least one integration test.
- Don't chase coverage on YAGNI branches. If a branch is never expected to fire in production, the branch is the bug — delete it, don't test it.

### Substrate-not-actor invariants — Decided

These aren't language rules; they're the project's load-bearing semantics. Easy to violate by accident. Easy to spot in review once they're named.

- **`(source, session_id)` is the natural key.** Never assume `session_id` is globally unique. Claude session IDs and Codex session IDs can collide; just because they haven't yet doesn't mean they won't. Every query, every cache key, every log line that names a session uses both.
- **Native hook payloads ride verbatim.** The daemon does NOT strip or rename fields in the `payload` column. Presenters that want full fidelity get it. If a presenter wants a derived field, it computes it itself — that's why the substrate is small.
- **Carried mechanical facts on `SessionState` are observations, never interpretations.** The projection carries a small set of facts the daemon directly observes and threads forward: `last_pid` (Story 5.3, shim-injected PID, carry-forward/overwrite-on-Some), `cwd` (Story 5.7, the source's native hook working directory, stored verbatim — no path canonicalization/`~`-expansion/symlink resolution — carry-forward/overwrite-on-Some), and `started_at` (Story 5.7, epoch-ms of the session's first observed event, daemon-derived set-once). Derivations *from* these (repo/project/branch from `cwd`, session age from `started_at`) are presenter concerns, never daemon fields (Axiom 4, ADR 0006).
- **Exactly one normalization is applied: tool name → reaction enum.** Eleven values from OpenPets. Mapping table lives in `adapters/<source>/tool-reactions.toml`. No other normalization sneaks in. If you find yourself adding a field like `is_user_attention_needed` to the projection, that's a presenter concern — stop, file a discussion.

- **The reaction enum follows demand; it does not anticipate it** (Quinn's catch). New reaction values require *two independent presenters* to demonstrate the need in their cookbook examples first. Without this rule, the enum grows from 11 to 47 over two years and the daemon becomes a sentiment-analysis engine. The bar isn't "this seems reasonable"; it's "two unrelated use cases independently produced the same gap."
- **State emission and event INSERT must happen in the same SQLite transaction.** Otherwise a subscriber gets a state-change notification for an event that hasn't landed yet, and a presenter that re-snapshots via REST sees inconsistent data. Validate with a fault-injection test (SIGKILL during a load run; on restart, projection must match event log).
- **`bowerbird install` writes to `~/.claude/settings.json` atomically.** Read → parse → merge → write `.tmp` → rename. Anything else risks leaving the user's Claude config broken if interrupted.
- **Hook delivery is not reliable.** Claude can drop hooks if the shim is slow or if Claude is killed mid-tool-call. The projection must tolerate a missing `PostToolUse` after a `PreToolUse` and fall through to a sane state. The protocol's gap-detection signal (Open question) is how presenters learn about the gap; the daemon shouldn't pretend everything is fine.
- **State topic discipline: don't add a STATE topic per session field.** `state.session.<id>` (whole row), `state.session.<id>.current_state`, `state.session.<id>.attachment`, `state.session.<id>.context` are the high-frequency ones. Resist adding `state.session.<id>.branch`, `state.session.<id>.remote_url`, etc. — most fields don't change often enough to deserve their own topic.

### Development workflow

#### Before writing code — Decided

Four-step check, reordered for the common case (per-Paige):

1. **Check the wire protocol** (`docs/protocol.md`). What shape is the data? If your change would bump the protocol, that's a discussion, not a normal PR.
2. **Check `docs/cookbook/`** for an existing pattern. Has someone already solved this? If your change makes a pattern obsolete, update the cookbook in the same PR.
3. **Check the relevant ADR** in `docs/decisions/`. Why is it this way? Load-bearing decisions are documented with alternatives considered. To change a decision, write a new ADR that supersedes the old one.
4. **Check `docs/no-list.md`** (once it exists; until then, see Scope cuts above). Are you about to do something forbidden? Treat this as the metal-detector — runs at the end of the checklist, but if you're proposing a new *daemon responsibility*, read no-list first regardless.

#### Contributor decision flow — Decided

```
contributor wants to change something
        │
        ▼
   is it data shape?    ── yes ──▶ protocol.md + ADR if new pattern
        │
        no
        ▼
   is it behavior?      ── yes ──▶ protocol.md (behavior section) + changelog entry
        │
        no
        ▼
   is it a new pattern? ── yes ──▶ cookbook recipe + ADR if non-obvious
        │
        no
        ▼
   probably just code. proceed.
```

#### ADR triggers — Decided

Write a new ADR for any change that:

- Adds, removes, or modifies the wire protocol.
- Adds or removes a crate.
- Changes which language/runtime a crate uses.
- Adds a new ingest model (HookProvider / PluginProvider / TranscriptProvider / etc.).
- Makes a non-additive change to the storage schema.
- Reverses a previous ADR.
- Locks in any item currently marked `Open` in this file.

ADR format (`docs/decisions/NNN-kebab-case-title.md`):

```markdown
# NNN. Title

Date: YYYY-MM-DD
Status: Accepted | Superseded by ADR-NNN
Deciders: @handle, @handle
Related: ADR-NNN (supersedes / refines / conflicts-with)
Implementation: <path/glob, or "N/A: process-only">
Affects context.md sections: <list, or "none">

## Context
What's the situation that prompted this decision?

## Decision (one sentence)
What we chose.

## Alternatives considered
Each alternative with one paragraph on why it was rejected.

## Consequences
What changes as a result. What's now harder. What's now easier.

## Revisit when
*Observable triggers only — not vibes.*

Examples of good triggers:
- "When event volume exceeds 10k/sec."
- "When a second consumer needs STATE topics beyond `state.session.*`."

Bad trigger (don't write this): "When requirements change" or "When it stops feeling right."

If this section is empty in a PR, the reviewer rejects it.
```

**New required fields explained:**
- **Deciders.** In 18 months you won't remember whether this was a hallway agreement or a deliberated call. Names help.
- **Related.** ADRs accrete. Without backlinks you get archaeology instead of architecture.
- **Implementation.** Half the value of an ADR is finding the code it justifies.
- **Affects context.md sections.** Forces the question "is this file now stale?" Either context.md updates in the same PR, or the ADR explicitly notes which section is now stale until the next pass.

#### Documentation co-update — Decided

Update docs **in the same PR** as the code change. Doc updates as a follow-up get forgotten.

Concrete triggers:
- New event kind → update `docs/protocol.md`, the kind table in the source's `tool-reactions.toml`, and at least one cookbook entry that uses it.
- New STATE topic → update `docs/protocol.md` and the relevant cookbook entry.
- New REST endpoint → update `docs/protocol.md`.
- New capability flag → update `capabilities.toml` for adapters that support it, plus the capabilities section in `docs/protocol.md`.
- New adapter → write its README + add a section to `docs/adapter-authoring.md`.
- **Any change to a documented behavior** (retention policy, ordering or delivery guarantee, default config, perf characteristics consumers depend on), even without a code-shape change → update `docs/protocol.md` and `docs/protocol-changelog.md`. CI can't catch behavior-only changes; reviewers can, but only if the trigger is named.

**CI gate:** any change to `crates/protocol/src/*.rs` must touch at least one doc file in the same PR.

**Sharpened from party review:** "touched" is not "updated." A whitespace-only change passes the gate. The mitigation is a `docs/protocol-changelog.md` with a versioned heading required for any protocol change — that's the structured signal the gate actually checks against. (How exactly that's enforced is on the Open list.)

#### Contribution model — Decided (from README-draft)

New issues and PRs from new contributors are **auto-closed by default**. Maintainers review the auto-closed queue weekly.

This is borrowed from pi-mono. It isn't hostile — it's how the project stays small and coherent. The rules:

| Change type | Expected path |
|---|---|
| New feature or behavior change | File a GitHub Discussion first. Describe use case + alternatives considered. |
| Bug report with repro | Auto-closed, but reopened quickly once reviewed. |
| New adapter (`adapter-codex`, etc.) | PR directly. Highest-value contribution; faster review. See `docs/adapter-authoring.md`. |
| Documentation fix | Always reviewed. |
| Anything else | Discussion first. PRs without discussions are routinely declined regardless of code quality. |

**Presenter or extension authors don't contribute upstream — *except* for bug reports and protocol critique.** Publish your own *code* downstream; bring *findings* upstream. The split (per John + Quinn): presenter authors are the population closest to where the substrate hurts. Losing their bug reports loses our most useful feedback channel.

**A failing cookbook example IS a bug report** (Quinn's framing). The highest-fidelity way to report a protocol bug is: write a cookbook entry that exercises the behavior, demonstrate it fails against current `main`, file an issue with the example attached. That fast-tracks the report and gives the maintainer an executable, CI-gated regression test.

#### Auto-close reason tags — Proposed

Every auto-close needs a structured tag, not just a closure. Tags let a returning maintainer (or a 90-day catch-up) query the queue rather than re-read it:

- `auto-closed:scope` — proposal exceeded the no-list or substrate-not-actor axiom.
- `auto-closed:new-feature-no-discussion` — feature PR without a prior Discussion.
- `auto-closed:new-adapter-fast-track-missed` — *flagged for second look* (adapters are supposed to be fast-tracked).
- `auto-closed:bug-no-repro` — bug report without reproduction steps.
- `auto-closed:doc-fix` — *flagged for review* (doc fixes are always reviewed).
- `auto-closed:presenter-code` — presenter code that belongs in a downstream package.
- `auto-closed:presenter-finding` — *flagged for second look* (presenter bug reports are first-class).

The starred categories are the ones where auto-close is a triage signal, not a final answer. If pickles is offline, those queue items need a human eye when someone returns — not silent dismissal.

#### Response-time commitments — Proposed

Without named SLAs, contributors assume "never" and walk. These aren't promises; they're targets that get tracked openly.

| Surface | Target reply time | What "reply" means |
|---|---|---|
| New-adapter PR | within 7 days | First substantive review (not just acknowledgment). |
| Bug report with repro | within 7 days | Triage label + first response. |
| New-feature Discussion | within 14 days | Maintainer position: support / decline / "discuss further." |
| Doc fix PR | within 7 days | Reviewed and merged or feedback returned. |
| Security report (private channel) | within 72 hours | Acknowledgment + initial assessment. |
| Cookbook-example-as-bug-report | within 7 days | Treated as a bug report with a fast lane. |

If a target is missed by 2x, the contributor is encouraged to ping the issue. Tracking these targets honestly means accepting that maintainer absence is a real failure mode — see "Failure recovery" below.

#### Security fast-path — Proposed

Don't report vulnerabilities in public Discussions. Use the private channel (TBD before MVP — likely `SECURITY.md` with a contact + GitHub's private vulnerability reporting).

The bar: "this lets an unauthorized process on the same host read or modify another user's session data" or worse. Lower-severity issues (e.g. log information disclosure) can go through the normal issue channel.

#### Discussion-converged signaling — Proposed

A contributor needs to know when a Discussion is "ready to PR" vs. "still discussing." The maintainer labels Discussions:

- `status:exploring` — open ideation, no commitment.
- `status:converged-ready-to-pr` — direction settled; a PR matching the discussion would be reviewed against it.
- `status:declined` — won't accept upstream; here's the reasoning (still useful for fork authors).
- `status:stalled-need-input` — waiting on the contributor for more detail.

Without these labels, "discussion converged" is invisible.

#### Failure recovery: maintainer offline — Proposed

This project has one maintainer. That's deliberate (small surface area > contributor throughput) but it has a real failure mode: pickles is unavailable for 90 days and the project freezes silently while looking healthy (Quinn's catch).

The mitigation isn't "find a co-maintainer." It's:

1. **Auto-close tags above** make the queue auditable on return.
2. **A pinned issue named `MAINTAINER STATUS`** that pickles updates with current availability. "Active" / "Slow-response through DATE" / "Out through DATE." Contributors plan around it; expectations stay honest.
3. **Bug reports never get silently auto-closed for "stale" reasons during an offline window** — the staleness clock pauses while status is non-Active.
4. **A monthly review reminder** (cron job, calendar reminder, whatever): pickles scans `auto-closed:*-flagged-for-second-look` even during reduced availability.

#### CI gates — Decided

CI must pass before merge. Required jobs:

- `cargo fmt --check`
- `cargo clippy --all-targets --workspace -- -D warnings`
- `cargo test --workspace`
- `cargo build --examples` (and `cargo test --examples` if examples carry assertions)
- `cargo bench --no-run`
- `shim/benches/hot_path.rs` with regression-alarm
- `shellcheck` strict mode on every committed shell script
- Doc-update gate on `crates/protocol/src/*.rs`
- All on the platform matrix (macOS + Linux; arm64 if budget allows)

Warnings are errors. The `-D warnings` flag is non-negotiable in CI.

#### Local tooling — Proposed

- **Pre-commit:** `cargo fmt`, `cargo clippy`, ideally as a `lefthook` or `pre-commit` config so contributors hit issues locally before CI does. Don't require it — but ship a config that does the right thing if installed.
- **Dependency audit:** `cargo audit` and `cargo deny check advisories` in CI, gated as warnings (not blocking) for v1, blocking after the project ships.

#### Decision authority — Decided (from AGENTS-draft)

The maintainer has final authority over scope. Contributors build extensions, adapters, and presenters freely — those don't require maintainer approval. Core changes require discussion + merge approval.

Order of preference for resolving disputes:

1. **Existing ADR or no-list entry settles it.** Cheapest case.
2. **Discussion converges on a path.** ADRs or no-list get updated as part of the resolution.
3. **Maintainer decides** if discussion doesn't converge. Maintainer commits to writing the reasoning down — new ADR or no-list update.

No committee, no voting, no consensus requirement. The discipline that keeps the project healthy is the maintainer reading every discussion thread and updating the no-list quarterly.

### Code quality and style

Covered above in earlier sections:

- **`#![deny(unsafe_code)]`** at every crate root → Language-Specific Rules / Crate-wide invariants.
- **`cargo fmt --check` + `cargo clippy -D warnings`** gate CI → Crate-wide invariants + Workflow CI gates.
- **Module size cap (~800 lines)** → Daemon style.
- **No `unwrap`/`expect` in production paths** → Shim hot-path + Daemon style.
- **Naming and file structure** → Repository layout + Adapter file layout.
- **No `println!`/`eprintln!` in shipped code** → Daemon style.
- **MSRV pinned per-crate, `Cargo.lock` committed** → Crate-wide invariants.
- **Error discipline matrix** → Error discipline section.

No new content here. If a future contributor asks "what's the style guide?" point them at those sections.

### Critical don't-miss rules

Covered above in earlier sections:

- **Substrate-not-actor invariants** — `(source, session_id)` natural key, native payloads verbatim, one normalization (tool → reaction), state+event atomicity, atomic `settings.json` writes, hook unreliability tolerance, state-topic discipline.
- **Shim hot-path discipline** — never block Claude; rules cascade from that.
- **Asymmetric `deny_unknown_fields`** — strict on parse, permissive on emit.
- **Gap detection via `oldest_available_event_id`** — `synchronous=NORMAL` is honest because presenters can detect what they missed.
- **Connection factory enforcement** — the only public path to a `Connection`; CI lint forbids the alternative.
- **Required framework infrastructure** (graceful shutdown, panic handling, request-id, WS ping/pong, etc.) — non-optional pre-MVP.

The single "if you remember nothing else" rule is **Axiom 1: the substrate observes; it does not interpret.** Stated once at the top of this file, derived from everywhere below.

---

## Open questions to resolve before code lands

Punch list of real questions surfaced by the design corpus and the party-mode review:

- **Shim-when-daemon-down approach.** Direct SQLite write, fire-and-forget POST with drop-on-failure, or inotify-driven spool? Affects shim binary, daemon startup, and the definition of "lost data."
- **Protocol-level gap detection.** Sequence numbers + last-seen cursor on reconnect. Required to make `synchronous=NORMAL` honest.
- **Adapter contract shape.** Is `adapter-codex` a Rust crate, a subprocess speaking JSON-lines, or a config-driven entry in `adapters/`? Determines who can contribute one.
- **Reference SDK question.** Ship `@bowerbird/presenter` for TS/Node, or aim for a protocol simple enough that no SDK is needed? Lean toward the latter; revisit when the first real presenter is written and the plumbing-to-feature ratio is visible.
- **MSRV.** Pin a minimum Rust version when the workspace `Cargo.toml` is committed.
- **Time and ID types.** Timestamps (`SystemTime` / `chrono` / `time`), event IDs (UUIDv7 / ULID / monotonic int). Propagate through wire + schema.
- **Auth-token storage.** File at `~/.bowerbird/server.json` is the design baseline. Keychain via the `keyring` crate is the upgrade path; cost may not be worth it for a local-only daemon.
- **`AGENTS.md` naming.** "Agents" is overloaded: (a) project rules for AI coding agents working *on* bowerbird, (b) the coding agents bowerbird *observes*. Naming options narrowed to: `CONTRIBUTING.md` (human-readable; pulls double duty for AI agents) or split into `CONTRIBUTING.md` + `docs/agent-handoff.md`. Decide before moving the draft out of `docs/research/AGENTS-draft.md`.
- **Event-log truncation policy.** Append-only forever (disk growth problem), bounded by row count, bounded by age, manual `bowerbird gc` command? Affects the cursor-gap behavior — if the log never truncates, `gap_detected` only fires when a presenter reconnects to a daemon that restarted with a fresh DB.
- **Cookbook anchor tooling.** mdBook with `{{#include}}`, a hand-rolled `// cookbook-begin:` build step, or something else. Decide before the second cookbook entry exists.
- **Protocol changelog mechanism.** The "CI gates `protocol/src/*.rs` changes against a doc touch" rule has a hole: a whitespace touch passes the gate, and behavior changes without type changes don't trip it. Resolve via a structured `docs/protocol-changelog.md` with versioned headings, or hash-check the prose sections.
- **Doc bind for non-loopback case.** What changes if a user binds bowerbird to a LAN address? At minimum the bearer-token model needs rethinking. ADR-worthy.

---

## Using and maintaining this file

This file is the **front door** for AI agents (and humans) working on bowerbird. It's not the whole story — `docs/research/`, ADRs in `docs/decisions/`, and the eventual `AGENTS.md` carry the rest. But it's where everyone starts.

**For AI agents working in this codebase:**

- Read the **Project axioms** at the top before anything else. Every specific rule derives from them.
- Use the **Status legend** (Decided / Proposed / Open) as your tiebreaker. Decided is locked. Proposed pushes back if reality disagrees. Open is your invitation to propose an answer in an ADR.
- When a specific rule and an axiom seem to conflict, the axiom wins.
- When you need to make a load-bearing choice that's marked Open, don't silently pick — write the ADR that resolves it.
- If you find yourself adding application-level semantics to the daemon, stop. Read Axiom 1 again.

**For humans (pickles + future contributors):**

- This file is alive. When an ADR lands, its `Affects context.md sections:` field tells you which sections to update in the same PR. If you can't update them in the same PR, mark them stale explicitly.
- The **Open questions** list at the bottom is a punch list. Resolve them by writing ADRs, not by quietly picking answers in code.
- This file is *deliberately long* because the project is pre-MVP and the *reasoning* matters more than the conclusion. Once code lands and conclusions harden, sections will shorten. Don't trim early — the why is the point.
- Review this file when:
  - An ADR lands (mechanical; per-ADR).
  - A new crate appears or an existing one is split (Axiom 2 trigger).
  - A perf budget is renegotiated (Axiom 3 trigger).
  - A presenter ships using a protocol field in an unexpected way (Axiom 4 trigger).
  - Pickles returns from a non-Active maintainer-status window.

Last updated: 2026-05-11.
