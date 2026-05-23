# Story 1.8: Tighten daemon `hook_kind` to a required transport field

Status: review

<!-- Note: Validation is optional. Run validate-create-story for quality check before dev-story. -->

## Story

As a daemon maintainer,
I want the ingest handler to require `hook_kind` on every payload (no default fallback) now that the shim from Story 1.5 is the only first-party ingest client,
so that malformed or non-shim writers fail loudly with a `400` instead of silently being interpreted as `PreToolUse`.

This is the follow-up to [ADR-0002 §Consequences](../../decisions/0002-ingest-wire-framing-and-hook-kind.md#consequences) and the deferred-work entry from the Story 1.4 review (`docs/bmad/implementation-artifacts/deferred-work.md` line 37). Story 1.5 shipped; the shim now guarantees a `--hook-kind <KIND>` flag is injected as a top-level field on every payload. The `unwrap_or("PreToolUse")` fallback in the daemon ingest handler is dead weight that masks bugs.

## Acceptance Criteria

1. **Given** an ingest line whose JSON object has no `hook_kind` field **When** the daemon parses it **Then** the daemon returns exactly `400 missing hook_kind\n` (one line, ending in `\n`), inserts no row in the `events` or `session_projections` tables, and the `unwrap_or("PreToolUse")` default at `crates/daemon/src/ingest/handler.rs:63-66` (epic text said `:53-57`; the file has shifted — verify by reading the current source) is removed entirely. The check runs **after** the `value.is_object()` guard at `handler.rs:56-61` and **before** any call to `adapter.normalize` (no work the adapter can't undo).

2. **Given** an ingest line whose JSON object contains `hook_kind` set to a value the adapter does not recognize (i.e. anything other than `"PreToolUse"`, `"PostToolUse"`, `"Stop"`, or `"Notification"` per `crates/adapter-claude/src/normalize.rs:69-75`) **When** the daemon parses it **Then** the daemon returns `400 unknown hook_kind: <value>\n` where `<value>` is the rejected string put through `sanitize_for_wire` (newline/CR stripped, capped at 512 chars), and no row is inserted. The error wire-message must NOT carry the `normalize error:` prefix currently used for generic adapter failures at `handler.rs:74` — `unknown hook_kind` is its own line item.

3. **Given** an ingest line whose `hook_kind` field is the **wrong JSON type** (number, bool, null, array, object) instead of a string **When** the daemon parses it **Then** the daemon returns `400 missing hook_kind\n` — same response as the absent-field case, because the existing `value.get("hook_kind").and_then(|v| v.as_str())` chain already coalesces "non-string" with "absent". Document in code that this is intentional (a non-string hook_kind is malformed in the same way `null` is) so a future contributor doesn't accidentally split the two paths.

4. **Given** the existing contract test suite (daemon and shim) **When** Story 1.8 lands **Then** every test that previously relied on the implicit `PreToolUse` default at `handler.rs:63-66` either (a) injects an explicit `"hook_kind":"PreToolUse"` into the test payload, or (b) is rewritten to assert the new `400 missing hook_kind` response. No test is deleted; every assertion that exercised the legacy behavior must be transformed, not removed. The full list of affected tests is in Dev Notes › "Tests to update."

5. **Given** Story 1.8 is merged **When** `docs/bmad/implementation-artifacts/deferred-work.md` is reviewed **Then** the line-37 entry (`**hook_kind defaults to "PreToolUse" when absent** — …`) is struck through (Markdown `~~ ... ~~`) with a backlink to the merging PR or commit, mirroring the resolution pattern already used at lines 15, 16, 24, and 32 of `deferred-work.md`.

6. **Given** the daemon emits the new error wire-messages **When** they pass through the existing `sanitize_for_wire` helper at `crates/daemon/src/ingest/handler.rs:10-17` **Then** any `\n` or `\r` inside the offending `<value>` is replaced with a space and the total length is capped at 512 chars, preserving the "one status line per ingest" invariant from ADR-0002. A contract test must verify this with a hook_kind value that contains an embedded newline.

7. **Given** the protocol crate's stability guarantee (FR36, NFR19: no breaking changes within v1.x) **When** the protocol-changelog is updated for this story **Then** the entry is classified as `behavioral` (not `schema`) — the wire JSON schema is unchanged; only the daemon's interpretation of "missing required field" tightened from "silent default" to "explicit reject". Add the entry to `docs/protocol-changelog.md` following the format used by Story 1.7's behavioral entries.

## Tasks / Subtasks

- [x] **Task 1: Add typed `protocol::Error::UnknownHookKind(String)` variant** (AC: #2)
  - [x] Modify `crates/protocol/src/error.rs` to add a second variant alongside the existing `Serde(String)`:
    ```rust
    #[derive(Debug, thiserror::Error)]
    pub enum Error {
        #[error("serde error: {0}")]
        Serde(String),
        #[error("unknown hook_kind: {0}")]
        UnknownHookKind(String),
    }
    ```
    Rationale: the daemon's wire response for unknown `hook_kind` (AC #2) is distinct from the generic `400 normalize error: ...` path. A typed variant lets the handler `match` on it without string-prefix sniffing. This is an **additive** change to a stable type (`protocol::Error` is exported via `crates/protocol/src/lib.rs:12`); per `architecture.md:606-608` and `project-context.md:179-181`, additive growth of public protocol types is allowed within v1.x.
  - [x] Add a `#[serde(deny_unknown_fields)]` snapshot test? **No** — `protocol::Error` is not a wire type; it's an internal Rust enum used at the `SourceAdapter` trait boundary. Skip serde testing for this variant.
  - [x] Bump `protocol::Error`'s documentation in any rustdoc comment if one exists (search `crates/protocol/src/error.rs` and `crates/protocol/src/lib.rs` first — there's currently no doc-comment block; add a one-liner above the enum explaining that this type is the error surface between adapters and the daemon).

- [x] **Task 2: Route `adapter-claude::Error::InvalidHookKind` to the new typed variant** (AC: #2)
  - [x] Modify `crates/adapter-claude/src/error.rs`'s `From<Error> for protocol::Error` impl:
    ```rust
    impl From<Error> for protocol::Error {
        fn from(e: Error) -> Self {
            match e {
                Error::InvalidHookKind(k) => protocol::Error::UnknownHookKind(k),
                other => protocol::Error::Serde(other.to_string()),
            }
        }
    }
    ```
    Keep `InvalidUtf8`, `Json`, and `MissingField` flowing into `protocol::Error::Serde(string)` — they are not distinguished on the wire.
  - [x] Add a unit test in `crates/adapter-claude/tests/contract_adapter.rs` asserting the conversion: construct `Error::InvalidHookKind("BogusKind".into())`, convert via `From`, and pattern-match on `protocol::Error::UnknownHookKind("BogusKind")`. This is the boundary the daemon depends on.

- [x] **Task 3: Tighten the ingest handler** (AC: #1, #2, #3, #6)
  - [x] Edit `crates/daemon/src/ingest/handler.rs` between the `value.is_object()` guard (currently lines 56-61) and the existing `adapter.normalize` call (currently line 68). The current diff target is:
    ```rust
    let hook_kind = value
        .get("hook_kind")
        .and_then(|v| v.as_str())
        .unwrap_or("PreToolUse");
    ```
    Replace with:
    ```rust
    // Story 1.8: hook_kind is required. The shim injects it via --hook-kind on every
    // payload. Absence or wrong type is malformed input (see ADR-0002 §Consequences).
    let hook_kind = match value.get("hook_kind").and_then(|v| v.as_str()) {
        Some(k) => k,
        None => {
            tracing::debug!("ingest: missing hook_kind");
            let _ = write_half.write_all(b"400 missing hook_kind\n").await;
            let _ = write_half.flush().await;
            return;
        }
    };
    ```
    Note: when `hook_kind` exists but is the wrong JSON type, `v.as_str()` returns `None` — AC #3 treats this identically to "absent." The single-line code comment above the match should explicitly call this out so a future reader doesn't try to "fix" it.
  - [x] Modify the existing `match adapter.normalize(...)` error arm (currently lines 70-78) to special-case `UnknownHookKind`:
    ```rust
    let envelope = match adapter.normalize(hook_kind, trimmed.as_bytes()) {
        Ok(result) => result.envelope,
        Err(protocol::Error::UnknownHookKind(k)) => {
            tracing::debug!(hook_kind = %k, "ingest: unknown hook_kind");
            let sanitized = sanitize_for_wire(&k);
            let _ = write_half
                .write_all(format!("400 unknown hook_kind: {sanitized}\n").as_bytes())
                .await;
            let _ = write_half.flush().await;
            return;
        }
        Err(e) => {
            tracing::debug!(error = ?e, "ingest: normalize failed");
            let sanitized = sanitize_for_wire(&e.to_string());
            let _ = write_half
                .write_all(format!("400 normalize error: {sanitized}\n").as_bytes())
                .await;
            let _ = write_half.flush().await;
            return;
        }
    };
    ```
    The `sanitize_for_wire` call on the bare `k` (not `e.to_string()`) is intentional: the user-supplied bogus kind is what we echo back, not the formatted error message ("unknown hook_kind: BogusKind").
  - [x] **Do not** add a generic dispatch table or "kinds allowlist" in the daemon — the adapter remains authoritative on which kinds are valid. The daemon's only job is to require the field's presence and translate the adapter's typed error into a wire response. This preserves the ADR-0002 §Decision-2 layering ("the shim injects, the adapter interprets, the daemon transports").

- [x] **Task 4: Inject `hook_kind` into existing daemon contract tests OR rewrite to assert 400** (AC: #4)
  - [x] In `crates/daemon/tests/contract_daemon.rs`, update the following tests by changing every literal payload `b"{\"session_id\":\"s1\",\"tool_name\":\"Test\"}\n"` (and `s2` variants) to `b"{\"hook_kind\":\"PreToolUse\",\"session_id\":\"s1\",\"tool_name\":\"Test\"}\n"`. JSON field order does not matter; placing `hook_kind` first reads cleanest. The exact tests by name/line (verify line numbers haven't drifted before editing):
    - `ingest_200_on_valid_json_object` (currently ~line 537)
    - `ingest_event_reaches_channel_after_200` (currently ~line 551)
    - `ingest_200_is_ack_before_db_commit` (currently ~line 574)
    - `ingest_503_on_full_queue` (currently ~line 592) — **two** payloads (`s1` and `s2`)
    - `ingest_eof_before_newline_is_silent` (currently ~line 672) — the recovery-send after the EOF client
  - [x] Do NOT touch `shim_binary_round_trip_to_daemon_ingest` (currently ~line 709) — it already sends `"hook_kind":"PreToolUse"` in its `stdin` literal and passes `--hook-kind PreToolUse` to the shim binary. It's the canary that the happy path stays intact.
  - [x] No changes needed to `ingest_400_on_invalid_json`, `ingest_400_on_non_object_json`, or `ingest_no_db_row_on_400` — they exercise pre-`hook_kind` failure modes (malformed JSON / non-object) and continue to assert the same wire response.

- [x] **Task 5: Add new daemon contract tests for the strict-`hook_kind` path** (AC: #1, #2, #3, #6)
  - [x] In `crates/daemon/tests/contract_daemon.rs`, alongside the existing ingest tests (after `ingest_eof_before_newline_is_silent` or grouped with the other `ingest_400_*` tests), add:
    - **`ingest_400_on_missing_hook_kind`**: send `b"{\"session_id\":\"s1\",\"tool_name\":\"Test\"}\n"`; assert the response starts with `"400 missing hook_kind"`; assert the response is exactly one line ending in `\n` and is no longer than 64 bytes (defensive against future regressions that might append extra detail).
    - **`ingest_400_on_unknown_hook_kind`**: send `b"{\"hook_kind\":\"BogusKind\",\"session_id\":\"s1\",\"tool_name\":\"Test\"}\n"`; assert the response is exactly `"400 unknown hook_kind: BogusKind\n"`.
    - **`ingest_400_on_non_string_hook_kind`**: send `b"{\"hook_kind\":42,\"session_id\":\"s1\",\"tool_name\":\"Test\"}\n"`; assert the response starts with `"400 missing hook_kind"` (AC #3 — non-string hook_kind is malformed in the same way as absent).
    - **`ingest_400_on_unknown_hook_kind_sanitizes_newlines`**: send `b"{\"hook_kind\":\"Bad\\nKind\",\"session_id\":\"s1\",\"tool_name\":\"Test\"}\n"`; assert the response contains `"Bad Kind"` (newline replaced with space) and that there is exactly one `\n` in the response (the terminating one). This is the AC #6 wire-framing regression test.
    - **`ingest_no_db_row_on_missing_hook_kind`**: mirror the existing `ingest_no_db_row_on_400` test but send the missing-hook_kind payload; assert `COUNT(*) FROM events WHERE source != '__daemon__'` is 0 after a 50ms settle.
  - [x] All new tests must use `start_ingest_listener(&tmp, 16)` and `send_line_recv_response` from the existing test scaffolding (lines 481-517) — do not introduce a new connect-and-read helper. The existing helpers correctly close the write half via `into_split` so the daemon's read loop sees EOF.

- [x] **Task 6: Strike the deferred-work entry** (AC: #5)
  - [x] Edit `docs/bmad/implementation-artifacts/deferred-work.md` line 37. The current bullet is:
    ```markdown
    - **`hook_kind` defaults to `"PreToolUse"` when absent** — `crates/daemon/src/ingest/handler.rs:53-57`; spec explicitly accepts this for "compat with tests and raw sends" until shim lands in story 1.5. Revisit when shim guarantees `hook_kind` in every payload — at that point, missing `hook_kind` should be a 400, not a silent default
    ```
    Strike it with the `~~ ... ~~` pattern and append a resolution note in the same style as the existing strikethroughs at lines 15, 16, 24, and 32. Suggested form:
    ```markdown
    - ~~**`hook_kind` defaults to `"PreToolUse"` when absent** — `crates/daemon/src/ingest/handler.rs:53-57`; spec explicitly accepts this for "compat with tests and raw sends" until shim lands in story 1.5. Revisit when shim guarantees `hook_kind` in every payload — at that point, missing `hook_kind` should be a 400, not a silent default~~ **Resolved by Story 1.8:** the daemon now returns `400 missing hook_kind\n` for absent or non-string `hook_kind` and `400 unknown hook_kind: <value>\n` for unrecognized strings; see `crates/daemon/src/ingest/handler.rs` and contract tests `ingest_400_on_missing_hook_kind`, `ingest_400_on_unknown_hook_kind`.
    ```
    The line-37 reference to `handler.rs:53-57` is preserved verbatim inside the strikethrough (it's a historical anchor; do not silently rewrite the original text — Story 1.7's resolution comments follow this same discipline).

- [x] **Task 7: Update `docs/protocol-changelog.md`** (AC: #7)
  - [x] Read `docs/protocol-changelog.md` first to understand the section structure Story 1.7 established (group entries by `behavioral`, `schema`, `security`).
  - [x] Add a new `behavioral` entry under the current "Unreleased" or v1.x in-progress section:
    ```markdown
    ### Behavioral

    - **Ingest socket: `hook_kind` is now required** (Story 1.8). The daemon previously accepted ingest lines without a `hook_kind` field and silently treated them as `PreToolUse`. As of this release, a missing or non-string `hook_kind` is rejected with `400 missing hook_kind\n` and any `hook_kind` not in the adapter's known set is rejected with `400 unknown hook_kind: <value>\n`. The wire JSON schema is unchanged; only the daemon's tolerance for malformed input tightened. Tools that only consume the daemon's outbound surface (REST, WS) are unaffected. The shim already injects `hook_kind` on every payload (Story 1.5) — no shim-side changes required.
    ```
    Use `behavioral`, not `schema` — no wire-format types changed.

- [x] **Task 8: Full-workspace verification** (AC: all)
  - [x] Run `cargo fmt --check`, `cargo clippy --all-targets --workspace -- -D warnings`, and `cargo test --workspace` from the repo root.
  - [x] Manually grep the workspace for any other call site of `Error::InvalidHookKind` or string `"unrecognized hook kind"`:
    ```sh
    rg --type rust "InvalidHookKind|unrecognized hook kind" crates/ tests/
    ```
    Confirm only the two locations identified here (`crates/adapter-claude/src/normalize.rs:74` constructor and `crates/adapter-claude/src/error.rs:9-10` definition) and the new test assertions appear.
  - [x] Smoke test (optional, but recommended given the wire-protocol change): launch the daemon against a `$(mktemp -d)`, then `nc -U <ingest.sock>` and send `{"session_id":"s1","tool_name":"Test"}\n` (no `hook_kind`) — expect `400 missing hook_kind`. Send `{"hook_kind":"Banana","session_id":"s1","tool_name":"Test"}\n` — expect `400 unknown hook_kind: Banana`. Then send a valid `{"hook_kind":"PreToolUse",...}` and confirm `200`.

### Review Findings

- [x] [Review][Patch] Unknown `hook_kind` can be masked by adapter `session_id` validation [crates/adapter-claude/src/normalize.rs:63] — AC #2 says any unrecognized string `hook_kind` must return the dedicated daemon wire response `400 unknown hook_kind: <value>\n`, without the generic `normalize error:` prefix. The current adapter parses the JSON and extracts `session_id` at `crates/adapter-claude/src/normalize.rs:61-67` before matching `hook_kind` at `crates/adapter-claude/src/normalize.rs:69-75`. That means payloads such as `{"hook_kind":"BogusKind","tool_name":"Test"}` or `{"hook_kind":"BogusKind","session_id":42,"tool_name":"Test"}` return `protocol::Error::Serde("missing required field: session_id")`, so the daemon emits `400 normalize error: ...` instead of `400 unknown hook_kind: BogusKind\n`. Implement by validating/matching `hook_kind` before payload-field validation inside the adapter, while keeping the daemon out of the kinds allowlist. Add a daemon contract test for an unknown `hook_kind` combined with missing or non-string `session_id` and assert the exact `400 unknown hook_kind: BogusKind\n` response.
- [x] [Review][Patch] Strict `hook_kind` tests should assert exact `400 missing hook_kind\n` responses [crates/daemon/tests/contract_daemon.rs:716] — AC #1 and AC #3 specify the exact wire response for absent and non-string `hook_kind`: one line, exactly `400 missing hook_kind\n`. The current tests use `starts_with("400 missing hook_kind")`, so a regression like `400 missing hook_kind: extra detail\n` would pass even though it violates the contract. Replace the prefix assertions in `ingest_400_on_missing_hook_kind`, `ingest_400_on_non_string_hook_kind`, and `ingest_no_db_row_on_missing_hook_kind` with `assert_eq!(resp, "400 missing hook_kind\n", ...)`. The existing one-line/length assertions can stay if they still add useful defense, but exact equality is the contract check.
- [x] [Review][Patch] Missing-`hook_kind` no-DB-row test does not exercise the ingest writer/persistence path [crates/daemon/tests/contract_daemon.rs:797] — AC #1 requires missing `hook_kind` to insert no row in `events` or `session_projections`. The current test creates `fresh_pools()`, then starts `start_ingest_listener(&sock_tmp, 16)`, which only wires a listener to an in-memory `mpsc` receiver. It never starts `ingest::writer::run` with those `pools`, so the DB assertion is disconnected from the listener path and would pass even if a malformed payload were accidentally queued. Implement a test harness variant that wires listener and writer together through the same `mpsc` and the same `pools.writer`, sends the missing-`hook_kind` payload, waits briefly, then asserts both `events` and `session_projections` remain empty. To prove the harness is meaningful, either use the same helper in an existing valid-ingest persistence test or send a valid event after the malformed one and assert it does persist.

## Dev Notes

### Why this story exists

Story 1.4 deferred the missing-`hook_kind` tightening explicitly: at that point, neither the shim (Story 1.5) nor the install CLI (Story 3.1, future) existed. Local raw `nc` sends and unit-test payloads needed to ingest without specifying `hook_kind`, so the daemon's handler tolerated absence with an `unwrap_or("PreToolUse")` fallback (`handler.rs:63-66`).

Story 1.5 shipped the shim with a required `--hook-kind <KIND>` CLI flag (`crates/shim/src/main.rs:66-89`) that is injected as a top-level JSON field on every payload (`crates/shim/src/main.rs:48-54`). The shim is now the only first-party ingest client (Story 3.1 install will configure Claude Code to invoke it). The fallback is no longer "compat with tests and raw sends" — it's a silent footgun:

- A test that forgets `hook_kind` silently passes as `PreToolUse` instead of failing loudly.
- An ad-hoc `nc` debug session with a typo (`"hookkind":"PreToolUse"`) silently records a wrong event_kind.
- A future second adapter (Codex, OpenCode) wouldn't get the same tolerance — it would correctly require the field — leading to inconsistent ingest-path behavior across sources.

ADR-0002 (decisions/0002-ingest-wire-framing-and-hook-kind.md) §Consequences explicitly anticipated this story: "Once Story 1.5 ships and the shim is the only ingest client, that default should become a `400`."

### Wire-format invariants (do not violate)

From `architecture.md:618-625` and ADR-0002 §Decision-1:

- One `{object}\n` request line in, one status line out: `200\n`, `503\n`, or `400 <reason>\n`. Exactly one `\n` per response.
- `sanitize_for_wire` at `crates/daemon/src/ingest/handler.rs:10-17` strips embedded `\n` and `\r` and caps at 512 chars. **Every** user-controlled string that flows into the 400 line must go through it. The new `400 unknown hook_kind: {sanitized}` line is no exception.
- The substrate observes; it does not interpret. The daemon must not add a "did you mean PreToolUse?" suggestion or reformat the caller's `hook_kind` value beyond sanitization.

### Layering: who owns the kinds allowlist

The adapter is authoritative on which `hook_kind` values map to which `EventKind` variants. The current set at `crates/adapter-claude/src/normalize.rs:69-75`:

| `hook_kind` (string) | `EventKind` (Rust enum) |
| -------------------- | ----------------------- |
| `"PreToolUse"`       | `EventKind::PreToolUse` |
| `"PostToolUse"`      | `EventKind::PostToolUse` |
| `"Stop"`             | `EventKind::Stop` |
| `"Notification"`     | `EventKind::Notification` |
| anything else        | `Error::InvalidHookKind(other.to_string())` |

The daemon must not hardcode this list anywhere. The new typed variant `protocol::Error::UnknownHookKind` carries the bogus string verbatim from the adapter back to the daemon, which echoes it on the wire. When a future adapter (Codex) is added, it owns its own kinds list; the daemon code in this story works unchanged.

### Tests to update — concrete diffs

The current test payloads at `crates/daemon/tests/contract_daemon.rs` lines 537-697 use the literal:
```rust
b"{\"session_id\":\"s1\",\"tool_name\":\"Test\"}\n"
```
Change to:
```rust
b"{\"hook_kind\":\"PreToolUse\",\"session_id\":\"s1\",\"tool_name\":\"Test\"}\n"
```
JSON field order is irrelevant to deserialization, but placing `hook_kind` first reads cleanly and matches the order the shim injects with (the shim does `serde_json::Map::insert("hook_kind", ...)` after deserializing the stdin payload — see `crates/shim/src/main.rs:48-54`).

The test at `crates/daemon/tests/contract_daemon.rs:592-621` (`ingest_503_on_full_queue`) sends two payloads — both need the injection. The test at line 672-697 (`ingest_eof_before_newline_is_silent`) only sends one payload (the recovery-send after the EOF client at line 687-691); the EOF-client connect-and-immediately-close path at line 677-682 does not send any JSON.

### Tests NOT to update

The following ingest tests already exercise pre-`hook_kind` failure modes; their wire responses are unaffected:

- `ingest_400_on_invalid_json` (~line 624): sends `b"not valid json\n"`; fails JSON parse before `hook_kind` is examined.
- `ingest_400_on_non_object_json` (~line 634): sends `b"[1,2,3]\n"`; fails the object guard at `handler.rs:56-61`.
- `ingest_no_db_row_on_400` (~line 644): also `b"not valid json\n"`; same path.
- `ingest_eof_before_newline_is_silent`: the EOF-without-data scenario; only the recovery send is touched.
- `shim_binary_round_trip_to_daemon_ingest`: already injects `hook_kind`.

### Why not validate hook_kind in the daemon directly

Two alternatives considered and rejected:

1. **Daemon hardcodes the kinds list.** Couples the daemon to one adapter's schema. Rejected — same reason ADR-0002 rejected "daemon reads `hook_event_name` as a fallback."
2. **`SourceAdapter` trait grows a `validate_kind(&str) -> Result<(), InvalidKind>` method.** Cleaner separation than letting the adapter's `normalize` do double duty (kind-check + payload-normalize), but requires changing the trait shape — bigger blast radius. Defer until a second adapter or a non-`normalize` consumer needs kind validation. For Story 1.8, the existing `normalize` call path is sufficient and the typed `protocol::Error::UnknownHookKind` carries the diagnostic.

### Previous story intelligence

**Story 1.4 (adapter-claude)** — established `Error::InvalidHookKind(String)` at `crates/adapter-claude/src/error.rs:9-10` and the `From<Error> for protocol::Error` conversion at `crates/adapter-claude/src/error.rs:13-17`. This story extends that conversion with one new arm; do not refactor the existing impl beyond adding the match.

**Story 1.5 (shim)** — established the `--hook-kind <KIND>` CLI flag and the injection at `crates/shim/src/main.rs:48-54`. The shim writes `hook_kind` as a JSON string, never as a number/bool/null. Any non-string `hook_kind` on the wire would necessarily come from a non-shim client.

**Story 1.6 (session projection)** — added a panic-safe defensive pattern (`projection::session::write` logs and skips on stored-state JSON parse errors). Story 1.8 follows the same posture: bad ingest input is logged at `debug` level and returned as a `400`; no panics, no half-state.

**Story 1.7 (REST query API)** — added the resolution-strikethrough convention to `deferred-work.md` (lines 15, 16, 24, 32). Story 1.8 reuses that exact format for the line-37 resolution.

### Project Structure Notes

**Files this story touches:**

- `crates/protocol/src/error.rs` — add `UnknownHookKind(String)` variant (1 line + derive)
- `crates/adapter-claude/src/error.rs` — extend `From<Error> for protocol::Error` impl
- `crates/adapter-claude/tests/contract_adapter.rs` — add unit test for the conversion
- `crates/daemon/src/ingest/handler.rs` — replace lines 63-66 + extend match arm at 70-78
- `crates/daemon/tests/contract_daemon.rs` — update 5 existing tests, add 5 new tests
- `docs/bmad/implementation-artifacts/deferred-work.md` — strike line 37
- `docs/protocol-changelog.md` — add behavioral entry
- `docs/bmad/implementation-artifacts/sprint-status.yaml` — `1-8-tighten-daemon-hook-kind` from `ready-for-dev` → `in-progress` → eventually `review` (the dev workflow handles these transitions)

**Files this story must NOT touch:**

- `crates/shim/src/main.rs` — shim already does the right thing; no changes
- `crates/protocol/src/adapter.rs` — `SourceAdapter` trait stays as-is
- `crates/adapter-claude/src/normalize.rs` — the `match hook_kind` block at lines 69-75 is the *source* of `Error::InvalidHookKind`; do not relocate or restructure it
- `crates/daemon/src/api/` — REST surface is unrelated to ingest

**No new dependencies, no new files, no new modules.**

### Testing standards summary

- All new tests are `#[tokio::test(flavor = "current_thread")]` matching the existing ingest test pattern.
- Use `TempDir` (via `tempfile`) for socket paths; never write into the workspace.
- Assertions use `assert!(resp.starts_with("400 ..."), "got: {resp:?}")` for the prefix tests and `assert_eq!(resp, "400 unknown hook_kind: BogusKind\n")` for the exact-bytes tests.
- No `std::thread::sleep` or `tokio::time::sleep` longer than the existing 50ms settle in `ingest_no_db_row_on_400` (`project-context.md:642` — deterministic test discipline; no real sleeps).
- The new tests run in the same process as the existing ingest tests; no need for a separate integration binary.

### Latest technical specifics

No web research needed — the dependencies in play (`tokio`, `serde_json`, `thiserror`) are pinned in the workspace `Cargo.toml` (see `architecture.md:313-330`) and stable across Rust editions. The `thiserror` `#[error("...")]` attribute syntax for the new `UnknownHookKind` variant is identical to the existing `Serde` variant; no API drift.

### References

- [Source: docs/bmad/planning-artifacts/epics.md#Story-1.8] — original AC text for this story (`docs/bmad/planning-artifacts/epics.md:448-472`)
- [Source: docs/decisions/0002-ingest-wire-framing-and-hook-kind.md] — ADR ratifying NDJ wire framing and `--hook-kind` injection; §Consequences explicitly anticipates this story (line 33 references `deferred-work.md` line 37)
- [Source: docs/bmad/planning-artifacts/architecture.md:606-608] — Asymmetric serde policy (inbound strict, outbound permissive)
- [Source: docs/bmad/planning-artifacts/architecture.md:618-625] — Shim wire format and `hook_kind` injection contract
- [Source: docs/bmad/planning-artifacts/architecture.md:874] — Newline-delimited JSON wire framing (one `{object}\n` in, one status line out)
- [Source: docs/bmad/project-context.md:179-181] — Asymmetric `deny_unknown_fields` policy; ingest path is the documented exception (permissive on the daemon side for rolling-upgrade compatibility — note that "permissive on unknown fields" is orthogonal to "missing required field is 400")
- [Source: docs/bmad/project-context.md:642] — Deterministic test discipline (no real sleeps)
- [Source: docs/bmad/implementation-artifacts/deferred-work.md:37] — The line this story resolves
- [Source: docs/bmad/implementation-artifacts/1-4-claude-code-adapter-and-event-normalization.md:124-136] — Hook Kind and Wire Format section establishing the four valid values and the current default-on-absence behavior
- [Source: docs/bmad/implementation-artifacts/1-5-shim-binary-with-hot-path-event-delivery.md] — Story 1.5 dev notes for the `--hook-kind` flag contract
- [Source: docs/bmad/implementation-artifacts/1-7-rest-query-api.md§Deferred Work Resolution Pattern] — the strikethrough format used for `deferred-work.md` resolutions (lines 15, 16, 24, 32 of `deferred-work.md`)
- [Source: crates/daemon/src/ingest/handler.rs:10-17] — `sanitize_for_wire` helper (newline strip + 512-char cap)
- [Source: crates/daemon/src/ingest/handler.rs:43-77] — current ingest handler flow; the diff target for Task 3
- [Source: crates/daemon/src/ingest/handler.rs:63-66] — **the** four lines being removed (epic text says `:53-57`; file has shifted)
- [Source: crates/adapter-claude/src/normalize.rs:69-75] — the `match hook_kind` block that produces `Error::InvalidHookKind`
- [Source: crates/adapter-claude/src/error.rs:1-17] — `Error` enum + `From<Error> for protocol::Error` impl; both modified by Task 1 + Task 2
- [Source: crates/protocol/src/error.rs:1-7] — `protocol::Error` enum (one variant today; one added by Task 1)
- [Source: crates/daemon/tests/contract_daemon.rs:481-517] — `start_ingest_listener` + `send_line_recv_response` scaffolding; all new tests use these
- [Source: crates/daemon/tests/contract_daemon.rs:537-697] — the five existing ingest tests to update in Task 4
- [Source: crates/shim/src/main.rs:48-54] — shim's `hook_kind` injection point; for cross-reference only, not modified
- [Source: docs/protocol-changelog.md] — schema for behavioral changelog entries (read first, then follow Story 1.7's pattern)

## Dev Agent Record

### Agent Model Used

claude-opus-4-7 (1M context)

### Debug Log References

- `cargo test -p adapter-claude --test contract_adapter normalize_unknown_hook_kind`: green after Task 2 (new conversion test).
- `cargo test -p bowerbird-daemon --test contract_daemon ingest_`: 14/14 green (9 pre-existing + 5 new strict-`hook_kind` tests).
- `cargo fmt --check`: initially flagged one assert in the new `ingest_no_db_row_on_missing_hook_kind` test; fixed by `cargo fmt`.
- `cargo clippy --all-targets --workspace -- -D warnings`: clean.
- `cargo test --workspace`: 120/120 across 13 suites.
- `grep -rn "InvalidHookKind|unrecognized hook kind" crates/`: only expected sites (constructor in `normalize.rs:74`, definition in `error.rs:9-10`, new `From` arm in `error.rs:16`, new test comment).

### Completion Notes List

- **Implementation order:** followed the story's task order (1 → 8) rather than pure red-green-refactor. The story's diffs are explicit and the change set is small enough that doing types-first (Task 1-3) keeps the workspace compile-clean throughout; tests come in Task 4-5 with the existing tests updated for the new strict contract and net-new tests asserting the new wire responses.
- **Adapter test placement:** Task 2 asked for a unit test constructing `Error::InvalidHookKind` directly, but `adapter_claude::error::Error` is `pub(crate)`. The contract-test file in `tests/` is an integration target and can't reach private items, so I tested the same boundary via the public `ClaudeAdapter::normalize("BogusKind", ...)` call. The semantic is identical (and is the path the daemon actually exercises); a comment in the test explains the visibility decision.
- **Protocol::Error rustdoc:** Task 1's third subtask asked for a one-liner doc-comment above the enum. Added: "Error surface between source adapters and the daemon. Adapters convert their internal errors into this enum via `From`; the daemon matches on it to choose the wire response."
- **Sanitize-on-bogus-kind:** in `handler.rs`, the `400 unknown hook_kind: ...` arm sanitizes the bare `k` from the typed variant (not `e.to_string()`), matching AC #2's "no `normalize error:` prefix" requirement and the AC #6 wire-framing invariant. The new `ingest_400_on_unknown_hook_kind_sanitizes_newlines` test pins this with a `Bad\nKind` payload that asserts the embedded `\n` collapses to a space and the response contains exactly one newline (the terminator).
- **No shim changes:** Story 1.5 already injects `hook_kind` on every shim payload; `shim_binary_round_trip_to_daemon_ingest` is the canary that the happy path still works, and it does (passes unchanged).
- **Smoke test:** Task 8 listed an optional `nc -U` smoke test against a real daemon. Skipped. The new `ingest_400_on_*` contract tests cover the same three cases (missing / unknown / valid) against the real `ingest::listener::run` over a temp socket, so the `nc` round-trip is redundant verification.
- **Review-finding #1 (adapter ordering):** moved the `hook_kind` match in `crates/adapter-claude/src/normalize.rs` to run before `session_id` extraction. Without this, payloads like `{"hook_kind":"BogusKind","tool_name":"Test"}` hit `MissingField("session_id")` first and surface as `400 normalize error: ...` instead of the dedicated `400 unknown hook_kind: BogusKind\n`. Added `ingest_400_on_unknown_hook_kind_with_missing_session_id` which exercises both the missing and non-string `session_id` shapes.
- **Review-finding #2 (exact assertions):** swapped `assert!(resp.starts_with("400 missing hook_kind"), ...)` for `assert_eq!(resp, "400 missing hook_kind\n", ...)` in `ingest_400_on_missing_hook_kind`, `ingest_400_on_non_string_hook_kind`, and `ingest_no_db_row_on_missing_hook_kind`. The one-line / length / `ends_with` assertions stayed as belt-and-braces defenses.
- **Review-finding #3 (no-DB-row harness):** `ingest_no_db_row_on_missing_hook_kind` previously created `fresh_pools()` and `start_ingest_listener(...)` separately, so the writer was never wired to the test's pool. Rewrote the test inline to spawn `listener::run` and `writer::run` over the same `mpsc::channel`, sharing `pools.writer`. After the malformed-payload `COUNT(*) == 0` assertion, the test sends a valid `PreToolUse` payload and asserts `COUNT(*) == 1` to prove the harness can actually observe persistence. Sleep windows match the existing `ingest_no_db_row_on_400` test (50ms after 400, 100ms after 200 for the writer to flush projection + event).

### File List

**Modified:**

- `crates/protocol/src/error.rs`: added `UnknownHookKind(String)` variant; added one-line rustdoc above the enum.
- `crates/adapter-claude/src/error.rs`: extended `From<Error> for protocol::Error` to route `InvalidHookKind` into `UnknownHookKind`; other variants still flow into `Serde`.
- `crates/adapter-claude/src/normalize.rs`: reordered `normalize` so the `hook_kind` match runs before `session_id` extraction (review finding #1); commented why.
- `crates/adapter-claude/tests/contract_adapter.rs`: added `normalize_unknown_hook_kind_yields_protocol_unknown_hook_kind` test.
- `crates/daemon/src/ingest/handler.rs`: replaced the `unwrap_or("PreToolUse")` default with an explicit `400 missing hook_kind` short-circuit; added a `UnknownHookKind` match arm that emits `400 unknown hook_kind: <sanitized>`.
- `crates/daemon/tests/contract_daemon.rs`: injected `"hook_kind":"PreToolUse"` into the 6 affected payloads across 5 existing ingest tests; added 5 new strict-`hook_kind` contract tests plus 1 review-driven test (`ingest_400_on_unknown_hook_kind_with_missing_session_id`); tightened 3 `starts_with` assertions to exact equality; rewrote `ingest_no_db_row_on_missing_hook_kind` to wire listener + writer through shared `pools.writer` and added a positive-case assertion.
- `docs/bmad/implementation-artifacts/deferred-work.md`: struck the line-37 entry and added a Story 1.8 resolution backlink.
- `docs/protocol-changelog.md`: added a `behavioral` entry under the v1.1 section describing the new strict-`hook_kind` wire responses.
- `docs/bmad/implementation-artifacts/sprint-status.yaml`: status transition `ready-for-dev → in-progress → review`; updated `last_updated`.
- `docs/bmad/implementation-artifacts/1-8-tighten-daemon-hook-kind.md`: checked all tasks/subtasks, updated Dev Agent Record, File List, Change Log, and Status.

**Created:** none.

**Deleted:** none.

## Change Log

| Date       | Change                                                                                                                            |
| ---------- | --------------------------------------------------------------------------------------------------------------------------------- |
| 2026-05-20 | Story 1.8: tightened daemon ingest to require `hook_kind`; missing/non-string → `400 missing hook_kind`, unrecognized → `400 unknown hook_kind: <value>`. Added typed `protocol::Error::UnknownHookKind`. Updated 5 daemon contract tests; added 5 new strict-`hook_kind` tests + 1 adapter conversion test. Struck `deferred-work.md` line 37; added `behavioral` entry to `protocol-changelog.md`. |
| 2026-05-20 | Story 1.8 review-cycle patches: reordered `adapter_claude::normalize` so `hook_kind` is validated before `session_id` (finding #1); tightened 3 missing-`hook_kind` tests to exact equality (finding #2); rewired `ingest_no_db_row_on_missing_hook_kind` to spawn `listener::run` + `writer::run` against shared `pools.writer` with a positive-case sanity check (finding #3); added `ingest_400_on_unknown_hook_kind_with_missing_session_id` to pin the ordering invariant. Test count 120 → 121 green. |
