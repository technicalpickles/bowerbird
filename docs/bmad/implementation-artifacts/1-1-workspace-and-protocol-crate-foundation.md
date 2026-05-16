# Story 1.1: Workspace and Protocol Crate Foundation

Status: review

## Story

As a tool builder,
I want a stable, well-typed protocol library that defines all bowerbird wire types,
So that I can write deserializers and client code against a documented, versioned schema before the daemon is complete.

## Acceptance Criteria

1. **Given** the bowerbird Rust workspace (Cargo.toml, crates/protocol, crates/shim, crates/daemon, crates/adapter-claude) **When** I run `cargo build --workspace` **Then** all crates compile cleanly with zero warnings, `cargo fmt --check` passes, and `cargo clippy --all-targets --workspace -- -D warnings` passes.

2. **Given** the protocol crate's outbound types (EventEnvelope, state frames, event frames) **When** I deserialize a wire payload that contains an extra unknown field not present in the Rust struct **Then** deserialization succeeds without error (permissive outbound policy — no `deny_unknown_fields` on daemon→client types).

3. **Given** the protocol crate's inbound parse types (ClientMessage, subscribe messages) **When** I submit a payload containing an unknown field **Then** deserialization fails with a clear `deny_unknown_fields` error (strict inbound policy).

4. **Given** any crate root in the workspace **When** I add an `unsafe` block anywhere in the crate **Then** the build fails due to `#![deny(unsafe_code)]` (enforced via workspace lint `unsafe_code = "forbid"`).

5. **Given** the workspace Cargo.toml and each crate's Cargo.toml **When** I inspect them **Then** every crate declares a pinned `rust-version` (MSRV), `Cargo.lock` is committed to the repository, and the edition is 2021.

6. **Given** a GitHub Actions PR workflow **When** a pull request is opened **Then** the CI matrix runs `cargo fmt --check`, `cargo clippy --all-targets --workspace -- -D warnings`, and `cargo test --workspace` on both macOS-latest and ubuntu-latest runners.

## Tasks / Subtasks

- [x] Task 1: Scaffold workspace root (AC: #1, #4, #5)
  - [x] Create root `Cargo.toml` with `[workspace]` + `[package]` sections (root is also the CLI binary crate stub)
  - [x] Add workspace members: `crates/protocol`, `crates/shim`, `crates/daemon`, `crates/adapter-claude`
  - [x] Add `[workspace.lints.rust] unsafe_code = "forbid"`
  - [x] Add `[profile.release-shim]` with `panic="abort"`, `lto="fat"`, `codegen-units=1`, `opt-level="z"`, `strip=true`
  - [x] Add `[workspace.dependencies]` section with pinned versions for all shared deps
  - [x] Create `rust-toolchain.toml` pinning `channel = "stable"`
  - [x] Create stub `src/main.rs` for the root CLI binary (just `fn main() {}` with clap dep declared but not wired)
- [x] Task 2: Create crate stubs for shim, daemon, adapter-claude (AC: #1)
  - [x] `crates/shim/Cargo.toml`: bin crate; edition 2021; rust-version pinned; deps: protocol (workspace), thiserror (workspace)
  - [x] `crates/shim/src/main.rs`: empty `fn main() {}`; `#![deny(unsafe_code)]`
  - [x] `crates/daemon/Cargo.toml`: bin crate; edition 2021; rust-version pinned; deps: protocol (workspace), tokio (workspace), axum (workspace), etc.
  - [x] `crates/daemon/src/main.rs`: `#[tokio::main] async fn main() {}` stub
  - [x] `crates/adapter-claude/Cargo.toml`: lib crate; edition 2021; rust-version pinned
  - [x] `crates/adapter-claude/src/lib.rs`: empty lib with `#![deny(unsafe_code)]`
- [x] Task 3: Implement `crates/protocol` fully (AC: #1, #2, #3, #4, #5)
  - [x] `Cargo.toml`: lib crate; rust-version; deps: serde 1.0.228 (derive), serde_json 1.0.149, thiserror 2.0.18
  - [x] `src/error.rs`: `pub enum Error { ... }` + `pub type Result<T> = std::result::Result<T, Error>;`
  - [x] `src/constants.rs`: `pub const SHIM_BINARY_NAME: &str = "bowerbird";`
  - [x] `src/event.rs`: `EventId(i64)`, `EventKind` (no `rename_all`), `Event`, `EventEnvelope`
  - [x] `src/reaction.rs`: `Reaction` enum with custom `Serialize`/`Deserialize` (hand-written, no derive)
  - [x] `src/adapter.rs`: `SourceAdapter` trait, `NormalizeResult`, `AdapterMeta`
  - [x] `src/rest.rs`: `EventListResponse`, `SessionStats` (outbound — no `deny_unknown_fields`)
  - [x] `src/ws.rs`: `ServerMessage`, `ClientMessage`, all frame types (`HelloFrame`, `SyncFrame`, `EventFrame`, `DroppedFrame`, `CloseFrame`)
  - [x] `src/lib.rs`: `pub use` re-exports of ALL public types; `#![deny(unsafe_code)]`
- [x] Task 4: Write protocol contract tests (AC: #2, #3)
  - [x] `crates/protocol/tests/contract_protocol.rs`: wire-format snapshot assertions for `EventKind` (verify PascalCase-as-written, e.g., `"ToolUse"`)
  - [x] Test: `EventId` serializes as plain JSON number (not string, not object)
  - [x] Test: outbound type (`HelloFrame`) accepts extra unknown fields without error
  - [x] Test: inbound type (`ClientMessage`) rejects extra unknown fields with error
  - [x] Test: `Reaction::Vendor(42)` serializes to string `"Vendor(42)"`
  - [x] Test: `Reaction::Unknown` round-trips correctly
- [x] Task 5: Set up GitHub Actions CI (AC: #6)
  - [x] `.github/workflows/ci.yml`: matrix on `[macos-latest, ubuntu-latest]`
  - [x] Steps: `cargo fmt --check`, `cargo clippy --all-targets --workspace -- -D warnings`, `cargo test --workspace`
- [x] Task 6: Verify all checks pass
  - [x] `cargo check --workspace` — green
  - [x] `cargo fmt --check` — green
  - [x] `cargo clippy --all-targets --workspace -- -D warnings` — green
  - [x] `cargo test --workspace` — all contract tests pass
  - [x] Commit `Cargo.lock`

## Dev Notes

### Workspace Cargo.toml Structure

The workspace root `Cargo.toml` is both a `[workspace]` manifest AND a `[package]` (the CLI binary crate). This is standard Rust for co-locating the binary at root.

```toml
[workspace]
members = ["crates/*"]
resolver = "2"

[workspace.lints.rust]
unsafe_code = "forbid"

[workspace.dependencies]
# Protocol deps
serde       = { version = "1.0.228", features = ["derive"] }
serde_json  = "1.0.149"
thiserror   = "2.0.18"
# Daemon deps
tokio            = { version = "1.52.1", features = ["rt", "macros", "net", "io-util", "sync", "time", "signal", "fs"] }
axum             = "0.8.9"
rusqlite         = { version = "0.39.0", features = ["bundled", "backup", "blob"] }
deadpool-sqlite  = "0.13.0"
tower-http       = "0.6.10"
tracing          = "0.1.44"
tracing-subscriber = "0.3.20"
anyhow           = "1.0.102"
uuid             = { version = "1.23.1", features = ["v4"] }
rusqlite_migration = "2.5.0"
tokio-util       = "0.7.18"
tokio-stream     = "0.1.17"
clap             = { version = "4.5.37", features = ["derive"] }
secrecy          = "0.10.3"
keyring          = "3.6.1"
tempfile         = "3.20.0"

[profile.release-shim]
inherits      = "release"
panic         = "abort"
lto           = "fat"
codegen-units = 1
opt-level     = "z"
strip         = true
```

Protocol crate `Cargo.toml` only needs `serde`, `serde_json`, `thiserror` as deps (no tokio, no axum).

### Protocol Crate: Critical Type Definitions

#### EventId

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct EventId(pub i64);
```

Wire: plain JSON number. `EventId(42)` → `42`. Never a string, never an object.

#### EventKind — CRITICAL: no `rename_all`

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum EventKind {
    PreToolUse,
    PostToolUse,
    Stop,
    Notification,
    RecordingStarted,
    RecordingEnded,
}
```

**DO NOT** add `#[serde(rename_all = ...)]` to `EventKind`. Wire strings are PascalCase-as-written: `"PreToolUse"`, `"RecordingStarted"`. The WS frame outer enum uses `rename_all = "snake_case"` — these are different policies on different types. Mixing them silently breaks wire compatibility.

#### EventEnvelope (pre-storage) and Event (stored)

```rust
/// Pre-storage; event_id is always 0; daemon sets at INSERT. Never pass to wire.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventEnvelope {
    pub source: String,
    pub session_id: String,
    pub kind: EventKind,
    pub reaction: Option<Reaction>,
    pub payload: String,   // verbatim raw JSON, no parsing
}

/// Stored event — includes assigned event_id and created_at timestamp.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Event {
    pub event_id: EventId,
    pub source: String,
    pub session_id: String,
    pub kind: EventKind,
    pub reaction: Option<Reaction>,
    pub payload: String,   // verbatim raw JSON
    pub created_at: i64,   // Unix milliseconds
}
```

**Invariant:** `EventEnvelope.event_id` does not exist — it's pre-assignment. The daemon never passes a non-zero `event_id` to SQLite INSERT (AUTOINCREMENT assigns it).

#### Reaction — custom serde (hand-written, no derive)

```rust
/// Wire format: "Pause", "Continue", "Vendor(42)", "Unknown"
pub enum Reaction {
    Pause,
    Continue,
    // add other named variants here as the Claude adapter discovers them
    Vendor(u16),
    Unknown,
}
```

Write `impl Serialize for Reaction` and `impl Deserialize for Reaction` in `src/reaction.rs` by hand. No `#[derive(Serialize, Deserialize)]` on this type — this is the **single exception** to the derive-based serde pattern. The `Vendor(n)` variant serializes as the string `"Vendor(42)"`.

#### SourceAdapter Trait

```rust
/// sync + pure: testable with raw byte slice, no Tokio, no daemon.
pub trait SourceAdapter {
    fn meta(&self) -> AdapterMeta;
    fn normalize(&self, hook_kind: &str, raw: &[u8]) -> Result<NormalizeResult>;
}

pub struct AdapterMeta {
    pub source: &'static str,
}

pub struct NormalizeResult {
    pub envelope: EventEnvelope,
}
```

#### REST Types (outbound — NO `deny_unknown_fields`)

```rust
#[derive(Debug, Serialize, Deserialize)]   // outbound: permissive
pub struct EventListResponse {
    pub events: Vec<Event>,
    pub cursor: Option<EventId>,
    pub oldest_available_event_id: EventId,  // i64::MAX when events table empty
}

#[derive(Debug, Serialize, Deserialize)]   // outbound: permissive
pub struct SessionStats {
    pub source: String,
    pub session_id: String,
    pub event_count: i64,
    pub first_event_at: Option<i64>,   // Unix ms
    pub last_event_at: Option<i64>,    // Unix ms
}
```

#### WebSocket Types

```rust
/// Outbound (daemon → tool). Permissive: no deny_unknown_fields.
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum ServerMessage {
    Hello(HelloFrame),
    Sync(SyncFrame),
    Event(EventFrame),
    Dropped(DroppedFrame),
    Close(CloseFrame),
}

/// Inbound (tool → daemon). STRICT: deny_unknown_fields.
#[derive(Debug, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum ClientMessage {
    Subscribe { topic: String },
    Unsubscribe { topic: String },
}

#[derive(Debug, Serialize, Deserialize)]
pub struct HelloFrame {
    pub protocol_version: String,
    pub daemon_version: String,
    pub oldest_available_event_id: EventId,
    pub daemon_started_at: i64,           // Unix ms
    pub history_begins_cleanly: bool,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SyncFrame {
    pub oldest_available_event_id: EventId,
    pub latest_event_id: EventId,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct EventFrame {
    pub event: Event,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct DroppedFrame {
    pub count: u64,
    pub first_dropped_event_id: EventId,
    pub last_dropped_event_id: EventId,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CloseFrame {
    pub reason: Option<String>,
}
```

### lib.rs Re-export Pattern

`crates/protocol/src/lib.rs` must re-export ALL public types. Callers always import from the crate root (`protocol::EventId`), never from internal submodule paths (`protocol::event::EventId`). This is a hard rule — downstream crates depend on it.

```rust
#![deny(unsafe_code)]

mod error;
mod event;
mod reaction;
mod adapter;
mod constants;
mod rest;
mod ws;

pub use error::{Error, Result};
pub use event::{Event, EventEnvelope, EventId, EventKind};
pub use reaction::Reaction;
pub use adapter::{AdapterMeta, NormalizeResult, SourceAdapter};
pub use constants::SHIM_BINARY_NAME;
pub use rest::{EventListResponse, SessionStats};
pub use ws::{
    ClientMessage, CloseFrame, DroppedFrame, EventFrame,
    HelloFrame, ServerMessage, SyncFrame,
};
```

### Error Module Contract (every crate)

Every crate's `src/error.rs` must contain exactly:
```rust
#[derive(Debug, thiserror::Error)]
pub enum Error {
    // variants here
}
pub type Result<T> = std::result::Result<T, Error>;
```

For Story 1.1, `protocol/src/error.rs` can start with a minimal variant (e.g., `#[error("serde error: {0}")] Serde(String)`). The error module is extended by later stories as needed.

### Serde Policy Summary

| Direction | `deny_unknown_fields` | Types |
|---|---|---|
| Inbound (tool→daemon) | YES (strict) | `ClientMessage`, any request body type |
| Outbound (daemon→tool) | NO (permissive) | `ServerMessage`, all frame types, `EventListResponse`, `SessionStats`, `Event` |

**Never** add `deny_unknown_fields` to outbound types. This is the additive forward-compat guarantee.

### Contract Tests Required

In `crates/protocol/tests/contract_protocol.rs`, these assertions are **pre-MVP gates** and must all pass:

```rust
// EventKind wire format — PascalCase, no rename
assert_eq!(serde_json::to_string(&EventKind::PreToolUse).unwrap(), "\"PreToolUse\"");
assert_eq!(serde_json::to_string(&EventKind::RecordingStarted).unwrap(), "\"RecordingStarted\"");

// EventId wire format — plain number
assert_eq!(serde_json::to_string(&EventId(42)).unwrap(), "42");

// Reaction::Vendor wire format
assert_eq!(serde_json::to_string(&Reaction::Vendor(42)).unwrap(), "\"Vendor(42)\"");
assert_eq!(serde_json::from_str::<Reaction>("\"Vendor(99)\"").unwrap(), Reaction::Vendor(99));

// Outbound type accepts unknown fields (permissive)
let extra_field = r#"{"protocol_version":"1.0","daemon_version":"0.1.0","oldest_available_event_id":0,"daemon_started_at":0,"history_begins_cleanly":true,"unknown_future_field":"ok"}"#;
assert!(serde_json::from_str::<HelloFrame>(extra_field).is_ok());

// Inbound type rejects unknown fields (strict)
let with_unknown = r#"{"op":"subscribe","topic":"events.*","unknown_field":"bad"}"#;
assert!(serde_json::from_str::<ClientMessage>(with_unknown).is_err());
```

### CI Configuration

`.github/workflows/ci.yml` must run on both `macos-latest` and `ubuntu-latest`:

```yaml
on: [push, pull_request]
jobs:
  ci:
    strategy:
      matrix:
        os: [macos-latest, ubuntu-latest]
    runs-on: ${{ matrix.os }}
    steps:
      - uses: actions/checkout@v4
      - run: cargo fmt --check
      - run: cargo clippy --all-targets --workspace -- -D warnings
      - run: cargo test --workspace
```

### Project Structure Notes

- Workspace root = CLI binary crate (stub `src/main.rs` only; CLI implementation in Epic 3)
- No `examples/` members yet — these arrive in Epic 4
- `Cargo.lock` **must be committed** — architecture requirement for reproducible builds
- The shim and daemon stub crates must compile but have no meaningful logic yet
- Protocol crate has no tokio/axum deps — only serde, serde_json, thiserror
- Daemon stub `src/main.rs` may reference `tokio::main` to verify tokio dep resolves

### Key Constraints for Stub Crates

**shim stub** (`crates/shim/src/main.rs`):
- NO `async`, NO `tokio`, NO `.await` — shim is sync-only, always
- `fn main() {}` is sufficient for this story

**daemon stub** (`crates/daemon/src/main.rs`):
- May use `#[tokio::main]` on the stub to verify tokio wires in

**adapter-claude stub** (`crates/adapter-claude/src/lib.rs`):
- Empty lib with `#![deny(unsafe_code)]` is sufficient

### Anti-Patterns to Avoid

- `rename_all` on `EventKind` — breaks wire format
- `deny_unknown_fields` on any outbound daemon→client type
- `unwrap()` / `expect()` outside `#[cfg(test)]` code
- Importing `protocol::event::EventId` — always import from `protocol::EventId`
- Putting any SQL, axum routes, or tokio runtime in the protocol crate
- Custom `impl Serialize`/`impl Deserialize` on any type other than `Reaction`

### References

- Architecture decisions: [Source: docs/bmad/planning-artifacts/architecture.md#Starter Template Evaluation]
- Dependency version table: [Source: docs/bmad/planning-artifacts/architecture.md#Dependency Version Pins]
- Protocol wire conventions: [Source: docs/bmad/planning-artifacts/architecture.md#Wire Format Conventions]
- Naming conventions: [Source: docs/bmad/planning-artifacts/architecture.md#Naming Conventions]
- Contract tests list: [Source: docs/bmad/planning-artifacts/architecture.md#Enforcement Guidelines]
- Serde policy: [Source: docs/bmad/planning-artifacts/architecture.md#Protocol serde]
- Project directory structure: [Source: docs/bmad/planning-artifacts/architecture.md#Complete Project Directory Structure]
- Story AC: [Source: docs/bmad/planning-artifacts/epics.md#Story 1.1]

## Dev Agent Record

### Agent Model Used

claude-sonnet-4-6

### Debug Log References

- Dependency conflict: architecture doc pinned `rusqlite 0.39.0` + `rusqlite_migration 2.5.0` (requires rusqlite ^0.39) + `deadpool-sqlite 0.13.0` (requires rusqlite ^0.38) — mutually incompatible. Resolved by using the consistent 0.38.x set: rusqlite 0.38.0, deadpool-sqlite 0.13.0, rusqlite_migration 2.4.1.
- tokio workspace feature set was missing `rt-multi-thread`; added to support `#[tokio::main]` in daemon stub.

### Completion Notes List

- Scaffolded the full Rust workspace: root CLI binary crate + 4 member crates (protocol, shim, daemon, adapter-claude).
- Implemented all protocol types exactly per spec: EventId (plain number), EventKind (PascalCase no rename_all), EventEnvelope/Event, Reaction (hand-written serde), SourceAdapter trait, REST outbound types (permissive), WebSocket types (ServerMessage permissive, ClientMessage strict deny_unknown_fields).
- All 6 contract tests pass: EventKind PascalCase wire format, EventId as plain number, Reaction::Vendor serialization, outbound type permissiveness, inbound type strictness.
- `cargo fmt --check`, `cargo clippy --all-targets --workspace -- -D warnings`, `cargo test --workspace` all pass with zero warnings.
- Cargo.lock committed (1350 lines, 139 packages locked).
- Adjusted pinned dependency versions to resolve native library link conflict (see Debug Log).

### File List

- Cargo.toml
- Cargo.lock
- rust-toolchain.toml
- src/main.rs
- crates/protocol/Cargo.toml
- crates/protocol/src/lib.rs
- crates/protocol/src/error.rs
- crates/protocol/src/constants.rs
- crates/protocol/src/event.rs
- crates/protocol/src/reaction.rs
- crates/protocol/src/adapter.rs
- crates/protocol/src/rest.rs
- crates/protocol/src/ws.rs
- crates/protocol/tests/contract_protocol.rs
- crates/shim/Cargo.toml
- crates/shim/src/main.rs
- crates/daemon/Cargo.toml
- crates/daemon/src/main.rs
- crates/adapter-claude/Cargo.toml
- crates/adapter-claude/src/lib.rs
- .github/workflows/ci.yml
- docs/bmad/implementation-artifacts/sprint-status.yaml

## Change Log

- 2026-05-16: Initial implementation of Story 1.1 — Rust workspace scaffolded, protocol crate implemented with all wire types, 6 contract tests added and passing, CI workflow configured.
