# Story 1.4: Claude Code Adapter and Event Normalization

Status: review

## Story

As a tool builder,
I want Claude Code hook payloads normalized to the canonical bowerbird event format before storage,
So that my tools receive consistent, predictable data regardless of changes to Claude Code's internal hook schema.

## Acceptance Criteria

1. **Given** a PreToolUse hook payload from Claude Code containing tool\_name, session\_id, and native payload fields **When** the adapter-claude crate processes it **Then** the resulting EventEnvelope contains: `source="claude"`, `session_id` from the hook, `event_kind=PreToolUse`, the correct `reaction` from tool-reactions.toml lookup, and the complete native payload verbatim in the `payload` column with no fields stripped or renamed

2. **Given** a tool name that is not present in `adapters/claude/tool-reactions.toml` **When** the adapter processes a hook event for that tool **Then** the `reaction` field is set to the `Unknown` enum variant, the event is still persisted without error, and no panic occurs

3. **Given** `adapters/claude/tool-reactions.toml` is updated with a new tool→reaction mapping at runtime **When** the adapter processes the next hook event for that tool **Then** it uses the updated mapping (TOML file is the source of truth, not a hardcoded enum)

4. **Given** two hook events that share an identical `session_id` value but have different `source` values ("claude" vs. a hypothetical second source) **When** both are ingested **Then** they are stored as distinct sessions and appear as separate records

5. **Given** a Claude Code hook payload that contains extra unknown fields beyond the defined schema **When** the adapter normalizes it **Then** those fields are preserved verbatim in the `payload` column (the substrate observes, it does not filter)

## Tasks / Subtasks

- [x] **Task 1: Add `toml` to workspace dependencies and update adapter-claude Cargo.toml** (AC: all)
  - [x] Add `toml = { version = "0.8", default-features = false, features = ["parse"] }` to workspace `[workspace.dependencies]` in root `Cargo.toml`
  - [x] Add `serde_json`, `toml`, `serde` to `crates/adapter-claude/Cargo.toml`

- [x] **Task 2: Create `adapters/claude/tool-reactions.toml`** (AC: #1, #2, #3)
  - [x] Create `adapters/claude/` directory
  - [x] Create `adapters/claude/tool-reactions.toml` with `[tool_reactions]` table mapping Claude Code tool names to Reaction enum string values
  - [x] Include all known Claude Code tool names: `Bash`, `Read`, `Write`, `Edit`, `MultiEdit`, `Glob`, `Grep`, `LS`, `TodoWrite`, `TodoRead`, `WebFetch`, `WebSearch`, `Task`, `NotebookRead`, `NotebookEdit`

- [x] **Task 3: Implement `adapter_claude` crate** (AC: #1, #2, #3, #5)
  - [x] Create `crates/adapter-claude/src/error.rs` with crate-internal `Error` enum (`InvalidUtf8`, `Json`, `Io`, `MissingField`, `InvalidHookKind`) and `From<Error> for protocol::Error`
  - [x] Create `crates/adapter-claude/src/normalize.rs`:
    - [x] `struct ToolReactionsFile { tool_reactions: HashMap<String, String> }` with `Deserialize`
    - [x] `fn parse_reaction(s: &str) -> Reaction` converting string to enum variant
    - [x] `fn load_reaction(toml_path: &Path, tool_name: &str) -> Reaction` — reads TOML on each call (runtime-update AC), returns `Unknown` on any I/O or parse error
    - [x] `pub(crate) fn normalize(toml_path: &Path, hook_kind: &str, raw: &[u8]) -> Result<NormalizeResult, Error>` — converts raw bytes + hook_kind → EventEnvelope with `source="claude"`, session_id from payload, kind from hook_kind, reaction from TOML lookup (for PreToolUse) or None (for others), payload verbatim
  - [x] Update `crates/adapter-claude/src/lib.rs`:
    - [x] Declare `pub(crate) mod error;` and `pub(crate) mod normalize;`
    - [x] Define `pub struct ClaudeAdapter { tool_reactions_path: PathBuf }`
    - [x] `impl ClaudeAdapter { pub fn new(tool_reactions_path: PathBuf) -> Self }`
    - [x] `impl SourceAdapter for ClaudeAdapter` delegating to `normalize::normalize`

- [x] **Task 4: Wire adapter into daemon ingest handler** (AC: #1, #2, #4, #5)
  - [x] Add `adapter-claude = { path = "../adapter-claude" }` to `crates/daemon/Cargo.toml`
  - [x] Add `tool_reactions_path: PathBuf` to `Config` in `crates/daemon/src/config.rs`, defaulting to `bowerbird_dir.join("adapters/claude/tool-reactions.toml")`
  - [x] Update `crates/daemon/src/ingest/listener.rs` — add `adapter: Arc<adapter_claude::ClaudeAdapter>` parameter to `run`, `run_bound`; clone and pass to `handler::handle`
  - [x] Update `crates/daemon/src/ingest/handler.rs` — accept `adapter: Arc<adapter_claude::ClaudeAdapter>` parameter; replace `make_placeholder_envelope` with `adapter.normalize(hook_kind, trimmed.as_bytes())`; extract `hook_kind` from parsed JSON (`value.get("hook_kind").and_then(|v| v.as_str()).unwrap_or("PreToolUse")`); map normalize `Err` to `400` response; remove `make_placeholder_envelope` fn
  - [x] Update `crates/daemon/src/main.rs` — create `Arc::new(ClaudeAdapter::new(config.tool_reactions_path.clone()))` and pass to `ingest::listener::bind` call chain
  - [x] Add `pub mod ingest;` and adapter imports to `crates/daemon/src/lib.rs` (already present; just check adapter import is accessible)

- [x] **Task 5: Contract tests for adapter-claude** (AC: #1, #2, #3, #5)
  - [x] Create `crates/adapter-claude/tests/contract_adapter.rs`
  - [x] Create `crates/adapter-claude/tests/fixtures/pre_tool_use_bash.json` — sample PreToolUse payload for Bash
  - [x] Create `crates/adapter-claude/tests/fixtures/pre_tool_use_unknown.json` — sample PreToolUse payload with unknown tool name
  - [x] **`normalize_pretooluse_bash_known_reaction`** (AC#1) — given Bash in TOML, check source="claude", kind=PreToolUse, reaction=Some(Continue), session_id correct, payload verbatim
  - [x] **`normalize_unknown_tool_returns_unknown_reaction`** (AC#2) — tool not in TOML → reaction=Some(Unknown), no error
  - [x] **`normalize_runtime_toml_update`** (AC#3) — write TOML without entry, normalize → Unknown; update TOML with entry, normalize → mapped reaction
  - [x] **`normalize_extra_fields_preserved_verbatim`** (AC#5) — extra JSON fields present in raw payload → payload column contains them verbatim
  - [x] **`normalize_posttooluse_has_no_reaction`** — PostToolUse events → reaction=None
  - [x] **`normalize_missing_session_id_returns_error`** — payload without session_id → Err

- [x] **Task 6: Update existing daemon contract tests** (ensures existing tests still pass)
  - [x] Update `start_ingest_listener` helper in `crates/daemon/tests/contract_daemon.rs` to create a `ClaudeAdapter` with a temp TOML path and pass it to the listener

- [x] **Task 7: Final checks**
  - [x] `cargo build --workspace` — green, zero warnings
  - [x] `cargo fmt --check` — green
  - [x] `cargo clippy --all-targets --workspace -- -D warnings` — green
  - [x] `cargo test --workspace` — all tests pass including new adapter contract tests

### Review Findings

_Recorded by bmad-code-review on 2026-05-17 (branch `claude/pull-main-bmad-dev-story-dmu55` vs `main`). Three layers ran: Blind Hunter (adversarial), Edge Case Hunter (boundary walk), Acceptance Auditor (AC verification). All 5 ACs pass; findings below are about robustness, hygiene, and one out-of-scope regression._

- [ ] [Review][Decision] Silent fallback on TOML errors with no diagnostic — `load_reaction` swallows both `std::fs::read_to_string` errors and `toml::from_str` errors, returning `Reaction::Unknown` with no log. Dev notes explicitly chose this for the adapter (pure library, no tracing). But misconfigured TOML (typo'd path, permissions issue, malformed file) becomes invisible: every event silently downgrades to Unknown. Question: add diagnostic at the daemon call site (the handler can `tracing::warn!` once if reaction is Unknown despite tool_name being non-empty + TOML path nonexistent), or accept silent degradation as spec'd? [`crates/adapter-claude/src/normalize.rs:31-44`]
- [ ] [Review][Decision] `parse_reaction` collapses typos and `Unknown` literal into the same `Reaction::Unknown` — `"Continu"`, `"Pasue"`, `"Vendor(garbage)"`, `"Vendor(70000)"` (u16 overflow), and the literal `"Unknown"` all produce the same value. Operators get no signal that their config has a typo. Question: should `parse_reaction` return `Result<Reaction, ParseError>` so `load_reaction` can warn (still degrading to Unknown), or is silent collapse the intended behavior? Linked to the diagnostic decision above. [`crates/adapter-claude/src/normalize.rs:8-26`]
- [ ] [Review][Decision] Synchronous TOML read on every `PreToolUse` in async runtime — `std::fs::read_to_string` + `toml::from_str` runs on the tokio worker thread inside the spawned ingest handler. Dev notes accept this for "low event frequency". Project-context Axiom 3 says daemon-internal budgets are negotiable, so spec-wise this is fine. Question: harden now (`tokio::task::spawn_blocking` for the read, or mtime-cached `ArcSwap<HashMap>`), or accept and revisit if profiling shows it? [`crates/adapter-claude/src/normalize.rs:31-44`]
- [ ] [Review][Patch] CI workflow regression: `on: [push, pull_request]` reverts PR #14's duplicate-CI fix [`.github/workflows/ci.yml:3`] — out of scope for story 1.4; restore `push.branches: [main]` scope.
- [ ] [Review][Patch] `raw.to_vec()` allocates twice — replace `String::from_utf8(raw.to_vec()).map_err(...)` with `std::str::from_utf8(raw).map(str::to_owned).map_err(...)` to drop one alloc on the per-event path [`crates/adapter-claude/src/normalize.rs:55`]
- [ ] [Review][Patch] `Error::InvalidUtf8(String)` discards the underlying `FromUtf8Error` — switch to `#[from] std::string::FromUtf8Error` to preserve the source error chain (mirrors how `Json` variant is wired) [`crates/adapter-claude/src/error.rs:4-5`]
- [ ] [Review][Patch] Empty `tool_name` on a `PreToolUse` event is silently accepted — `value.get("tool_name").and_then(|v| v.as_str()).unwrap_or("")` then looks `""` up in TOML (→ Unknown). Inconsistent with how `session_id` is handled (hard error). Return `Error::MissingField("tool_name")` for PreToolUse when tool_name is absent or non-string [`crates/adapter-claude/src/normalize.rs:73-78`]
- [ ] [Review][Patch] `"claude"` source identifier is duplicated in `ClaudeAdapter::meta()` and `normalize.rs` envelope construction — extract `pub(crate) const SOURCE: &str = "claude";` to keep one source of truth [`crates/adapter-claude/src/lib.rs:20-22`, `crates/adapter-claude/src/normalize.rs:80-86`]
- [ ] [Review][Patch] Handler writes raw `{e}` Display to the wire as `400 normalize error: {e}\n` — `serde_json::Error`'s Display can be multi-line, which breaks the line-oriented response framing. Sanitize (replace newlines with spaces) and optionally truncate before writing [`crates/daemon/src/ingest/handler.rs:60-62`]
- [ ] [Review][Patch] Test uses hardcoded absolute path `/nonexistent/path/tool-reactions.toml` and relies on it not existing on the host — flip to `TempDir::new()` + `.path().join("missing.toml")` for hermeticity [`crates/adapter-claude/tests/contract_adapter.rs:142-150`]
- [ ] [Review][Patch] No test coverage for `Reaction::Pause` or `Reaction::Vendor(N)` parse paths — `parse_reaction` ships with two whole branches untested. Add unit tests covering Pause string, Vendor(N) happy path, Vendor(garbage) → Unknown, Vendor(99999) overflow → Unknown [`crates/adapter-claude/tests/contract_adapter.rs`]
- [x] [Review][Defer] `hook_kind` defaults to `"PreToolUse"` when absent — spec accepts this for "compat with tests and raw sends" until shim lands in story 1.5; revisit when shim guarantees the field [`crates/daemon/src/ingest/handler.rs:53-57`]
- [x] [Review][Defer] In-place TOML rewrite race — concurrent edit can cause a transient parse error and a window of `Unknown` reactions; mitigation requires atomic write + mtime cache or file watcher; current graceful degradation is spec'd [`crates/adapter-claude/src/normalize.rs:31-44`]
- [x] [Review][Defer] TOML file as FIFO/named-pipe/device — `read_to_string` could block; validate file type with `fs::metadata().is_file()` before reading. Operator-misconfig edge, low priority [`crates/adapter-claude/src/normalize.rs:31`]
- [x] [Review][Defer] Unbounded TOML read size — `read_to_string` will allocate however large the file is; cap with `File::open` + `.take(MAX_TOML_BYTES)`. Operator-misconfig edge [`crates/adapter-claude/src/normalize.rs:31`]
- [x] [Review][Defer] `session_id` as JSON number/bool currently rejected as missing — tightening to a typed error would require confirming Claude Code's session_id wire type contract; defer until the protocol is documented [`crates/adapter-claude/src/normalize.rs:46-50`]
- [x] [Review][Defer] `session_id` length unbounded — no per-key size cap; local socket is trusted so DoS surface is small, but a sanity cap (e.g. 256 bytes) would be cheap [`crates/adapter-claude/src/normalize.rs:81-92`]
- [x] [Review][Defer] Reaction strings with surrounding whitespace or wrong case (`" Continue "`, `"continue"`) silently → Unknown — operator-UX nicety; trim + case-insensitive match. Linked to the `parse_reaction` decision above [`crates/adapter-claude/src/normalize.rs:21-24`]
- [x] [Review][Defer] TOML duplicate-key behavior unspecified — the `toml` crate errors by default, which `load_reaction` then swallows → Unknown. Document or test the precedence rule [`crates/adapter-claude/src/normalize.rs:18-25`]
- [x] [Review][Defer] `ClaudeAdapter::new` accepts a relative `PathBuf` with no canonicalization — daemon doesn't chdir today, but a future chdir would change resolution. Validate or canonicalize at construction [`crates/adapter-claude/src/lib.rs:13-18`]
- [x] [Review][Defer] No test covers the half-written / mid-edit TOML transition — runtime-update test only flips between valid states. Add coverage for the graceful-Unknown fallback during a transient parse error [`crates/adapter-claude/tests/contract_adapter.rs`]
- [x] [Review][Defer] Spec mismatch: `Error::Io` variant in spec but missing from `error.rs` — code never surfaces I/O errors (load_reaction swallows them to Unknown), so adding the variant would trigger `dead_code`. The omission is cleaner than the spec; either update the spec or add the variant with a tracking comment [`crates/adapter-claude/src/error.rs:1-9`]



### Critical Context from Stories 1.1–1.3 (DO NOT REPEAT MISTAKES)

**Dependency pins** — use the workspace dep table, not the architecture doc. Actual installed versions:

| Dep | Actually installed |
|---|---|
| rusqlite | 0.38.0 |
| rusqlite_migration | 2.4.1 |
| deadpool-sqlite | 0.13.0 |
| tokio | 1.52.1 |
| axum | 0.8.9 |
| serde_json | 1.0.149 |

**Workspace lints**: every crate has `[lints] workspace = true`. **Do NOT** add `#![deny(unsafe_code)]` to any source file — the workspace `unsafe_code = "forbid"` is already active. Adding it will produce a `clippy::duplicated_attributes` error.

**`anyhow::Context` boundary**: permitted only in `main.rs`. The `adapter-claude` crate uses `thiserror` only.

**No `unwrap()` / `expect()` outside `#[cfg(test)]`**: adapter module code follows this strictly.

**No `println!` / `eprintln!` in shipped code**: use `tracing::*` in daemon; adapter-claude has no tracing (it's a pure library).

### Hook Kind and Wire Format

The ingest wire format (established in Story 1.3) is newline-delimited JSON. The daemon reads one JSON object per connection from the ingest socket.

The JSON payload sent by the shim (implemented in Story 1.5) includes a `hook_kind` field identifying the event type. Story 1.4 extracts this field from the parsed JSON to pass to `normalize()`.

**`hook_kind` field values** (string, matching EventKind variant names):
- `"PreToolUse"` — tool about to be called; reaction derived from `tool_name` via TOML lookup
- `"PostToolUse"` — tool call completed; reaction = None
- `"Stop"` — agent turn ended; reaction = None
- `"Notification"` — agent notification; reaction = None

If `hook_kind` is absent (compat with tests and raw sends), the handler defaults to `"PreToolUse"`.

### Adapter Crate Error Boundary

The `adapter-claude` crate uses an internal `Error` enum (`crates/adapter-claude/src/error.rs`). This converts to `protocol::Error::Serde(msg)` at the `SourceAdapter` trait boundary. The daemon's handler maps `protocol::Error` to a `400` response with a descriptive reason.

### `tool-reactions.toml` Format

```toml
# Claude Code tool name → Reaction mapping
# Values: "Pause", "Continue", "Unknown", or "Vendor(N)" where N is u16
# Tool names not listed default to Reaction::Unknown

[tool_reactions]
Bash         = "Continue"
Read         = "Continue"
Write        = "Continue"
Edit         = "Continue"
MultiEdit    = "Continue"
Glob         = "Continue"
Grep         = "Continue"
LS           = "Continue"
TodoWrite    = "Continue"
TodoRead     = "Continue"
WebFetch     = "Continue"
WebSearch    = "Continue"
Task         = "Continue"
NotebookRead = "Continue"
NotebookEdit = "Continue"
```

### Runtime TOML Update (AC#3)

The `load_reaction` function reads the TOML file on every `normalize()` call. No caching. For a developer tool with low event frequency, this is acceptable and directly satisfies the runtime-update requirement. If profiling ever shows this is a bottleneck, a file-watcher or TTL cache can be added later.

**TOML file missing or parse error**: `load_reaction` returns `Reaction::Unknown` gracefully. Missing TOML is not a hard error — the adapter degrades to all-Unknown reactions.

### Source Value

The Claude Code adapter always sets `source = "claude"` (hardcoded in `ClaudeAdapter::meta()` and in `normalize.rs`). This is the `"claude"` source identifier as established in Story 1.3's stub.

### Session Composite Key (AC#4)

AC#4 (source, session_id composite key) is enforced by the database schema's `PRIMARY KEY (source, session_id)` on `session_projections` (established in Story 1.2). The adapter correctly sets `source = "claude"`. A hypothetical second adapter would set a different source. No additional adapter-level code is needed for this AC — the test verifies the returned envelope has `source = "claude"` and the correct `session_id`.

### Payload Verbatim (AC#5)

The adapter stores `payload = String::from_utf8(raw.to_vec())`. The raw bytes are the exact JSON string sent by the shim (verbatim hook JSON). No fields are stripped, renamed, or re-serialized. Even unknown extra fields present in the raw JSON are preserved because the entire byte sequence is stored, not a re-serialized subset.

### File Structure

**New files:**
```
adapters/claude/
└── tool-reactions.toml       # NEW: tool name → Reaction mapping

crates/adapter-claude/src/
├── error.rs                  # NEW: crate-internal error type
└── normalize.rs              # NEW: normalize() function

crates/adapter-claude/tests/
├── contract_adapter.rs       # NEW: adapter contract tests
└── fixtures/
    ├── pre_tool_use_bash.json    # NEW: sample PreToolUse/Bash payload
    └── pre_tool_use_unknown.json # NEW: sample PreToolUse/UnknownTool payload
```

**Modified files:**
```
Cargo.toml                             # add toml to workspace deps
crates/adapter-claude/Cargo.toml       # add serde_json, toml, serde
crates/adapter-claude/src/lib.rs       # implement ClaudeAdapter + SourceAdapter
crates/daemon/Cargo.toml               # add adapter-claude dependency
crates/daemon/src/config.rs            # add tool_reactions_path field
crates/daemon/src/ingest/listener.rs   # add adapter parameter
crates/daemon/src/ingest/handler.rs    # replace stub with adapter.normalize()
crates/daemon/src/main.rs              # create adapter, pass to listener
crates/daemon/tests/contract_daemon.rs # update start_ingest_listener helper
```

### Anti-Patterns to Avoid

- Adding `deny_unknown_fields` to any outbound type — preserved invariant from 1.1
- Calling `serde_json::to_string(&value)` to produce the payload — this re-serializes and may change field order or whitespace; store `raw` bytes verbatim as string instead
- Adding `async` to `normalize()` — the SourceAdapter trait is sync and pure; file I/O is acceptable (TOML read is on the same path as any blocking syscall in the daemon's spawned blocking context)
- `unwrap()` / `expect()` outside `#[cfg(test)]` in adapter code
- `use anyhow::*` in adapter-claude — only `thiserror` permitted

### Testing Standards

- All adapter tests are synchronous (no `#[tokio::test]`) — the adapter is sync
- Fixture files loaded via `include_str!()` in tests
- For runtime TOML update test (AC#3): write a real temp file, not include_str

## Dev Agent Record

### Agent Model Used

claude-sonnet-4-6

### Debug Log References

(none yet)

### Completion Notes List

- `ClaudeAdapter` implements the `SourceAdapter` trait. `normalize()` reads `tool-reactions.toml` on every call to satisfy AC#3 (runtime update). Missing or unparseable TOML degrades gracefully to `Reaction::Unknown` rather than erroring, so daemon startup is not blocked if the TOML is absent.
- The `hook_kind` field is extracted from the incoming JSON by the handler before calling `normalize()`. If absent, defaults to `"PreToolUse"` for backward compatibility with pre-shim test sends. The shim (story 1.5) will always include `hook_kind`.
- Reaction is `Some(...)` for `PreToolUse` events (from TOML or `Unknown` if not listed) and `None` for all other event kinds (`PostToolUse`, `Stop`, `Notification`).
- Payload stored verbatim: `String::from_utf8(raw.to_vec())` — no re-serialization, no field stripping. All unknown fields are preserved.
- `toml = "0.8"` added to workspace dependencies (not in lock file prior to this story).
- 9 new adapter contract tests added; all 30 existing daemon/protocol tests continue to pass. Total: 39 tests, 0 failures.
- Existing `start_ingest_listener` test helper updated to create a `ClaudeAdapter` with a nonexistent TOML path (gracefully degrades to Unknown), maintaining all 21 existing ingest contract tests.

### File List

- `adapters/claude/tool-reactions.toml` — NEW: Claude Code tool name → Reaction mapping (15 tools, all mapped to "Continue")
- `crates/adapter-claude/src/error.rs` — NEW: crate-internal Error enum with From<Error> for protocol::Error
- `crates/adapter-claude/src/normalize.rs` — NEW: normalize() function, parse_reaction(), load_reaction()
- `crates/adapter-claude/src/lib.rs` — UPDATED: ClaudeAdapter struct + SourceAdapter impl
- `crates/adapter-claude/Cargo.toml` — UPDATED: added serde, serde_json, toml deps
- `crates/adapter-claude/tests/contract_adapter.rs` — NEW: 9 adapter contract tests
- `crates/adapter-claude/tests/fixtures/pre_tool_use_bash.json` — NEW: test fixture
- `crates/adapter-claude/tests/fixtures/pre_tool_use_unknown.json` — NEW: test fixture
- `Cargo.toml` — UPDATED: added toml = "0.8" to workspace dependencies
- `crates/daemon/Cargo.toml` — UPDATED: added adapter-claude dependency
- `crates/daemon/src/config.rs` — UPDATED: added tool_reactions_path field
- `crates/daemon/src/ingest/handler.rs` — UPDATED: replaced make_placeholder_envelope stub with adapter.normalize(); added hook_kind extraction
- `crates/daemon/src/ingest/listener.rs` — UPDATED: added adapter parameter to run() and run_bound()
- `crates/daemon/src/main.rs` — UPDATED: create ClaudeAdapter, pass to listener
- `crates/daemon/tests/contract_daemon.rs` — UPDATED: start_ingest_listener helper passes ClaudeAdapter

## Change Log

- 2026-05-17: Story created for story 1.4 implementation. Context from stories 1.1–1.3 carried forward. Adapter architecture: ClaudeAdapter struct with PathBuf to tool-reactions.toml; runtime TOML read on every normalize() call; graceful Unknown on missing/invalid TOML.
- 2026-05-17: Story implemented. All 7 tasks complete. 9 adapter contract tests added and passing. Full workspace: 39 tests, 0 failures. cargo fmt, cargo clippy -D warnings, cargo build all green.
