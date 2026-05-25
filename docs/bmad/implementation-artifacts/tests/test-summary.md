# Test Automation Summary — Story 4.4

Generated 2026-05-25 via `bmad-qa-generate-e2e-tests`. Supersedes the Story 4.3 summary.

## Context

Story 4.4 ships the **protocol compatibility guarantee and contract test suite** — itself an entirely test/QA story. The dev workflow already landed five new CI gates (protocol-changelog gate, v1.0 wire-compat corpus, contract test inventory, hook→presenter daemon Criterion bench, cross-version upgrade), the `#[serde(other)] Unknown` sweep across `EventKind` / `SessionCurrentState` / `Reaction`, the deadlock fix on `state_plus_event_atomicity_under_sigkill_during_load`, and a per-platform daemon bench infrastructure with the same shape as the existing shim hot-path gate.

So this is **not** a conventional "generate API + E2E tests for a UI feature" workflow run — bowerbird is a Rust workspace with no UI surface. The analogous QA surface for Story 4.4 is **gap analysis against the implemented gate suites**: do all the new gates fire on the right surfaces, and is the v1.0 wire-shape corpus complete?

## Framework

- Rust `#[test]` functions in workspace-root test crates (`tests/*.rs`).
- Invocation: `cargo test --workspace -- --test-threads=1` (Epic 2 retro AI-3 / Story 3.4 AC #6).
- Story 4.4 ships compiled tests, not shell scripts — "compiled tests beat greps" per Epic 3 retro Team Agreement A7.
- No new dependencies introduced by this QA pass.

## Baseline coverage already in place (post Story 4.4 dev work)

Pre-existing gates that landed with the story's nine tasks:

- **`tests/protocol_changelog_gate.rs`** (1 test) — fails CI when `crates/protocol/src/*.rs` changes without a `type:` entry under `docs/protocol-changelog.md` (AC #1).
- **`tests/protocol_v1_compat.rs`** (18 tests pre-gap, 21 tests post-gap) — load-bearing v1.0 wire-shape corpus runner (AC #2).
- **`tests/contract_test_inventory.rs`** (2 tests) — pins the 10 required contract tests by name; fails on rename or deletion (AC #3, #3a).
- **`tests/cross_version_upgrade.rs`** (1 test, SKIPs when `BOWERBIRD_RUN_CROSS_VERSION_TEST=1` is unset) — cross-version data-dir compatibility against the prior tag (AC #5).
- **`crates/protocol/tests/contract_protocol.rs`** (19 tests, +3 from the AC #6 enum sweep) — wire-format snapshot mandate, including the three new `Unknown` variants and the load-bearing `Reaction::deserialize` `Err → Ok(Unknown)` fix.
- **`crates/daemon/tests/contract_daemon.rs`** — the 10 contract surfaces the inventory targets, including the de-flaked SIGKILL test.
- **`crates/daemon/benches/hook_to_presenter.rs`** — Criterion-replacement subprocess bench with four shapes (solo, fanout3, burst, steady) and per-platform baselines (AC #7).

Total Story-4.4 gate tests pre-QA pass: **41** new or extended `#[test]` functions; total workspace passing: **414**.

## Gaps discovered and filled

Audit revealed the v1.0 wire-compat corpus had **inconsistent coverage** of the three new `Unknown` catch-all variants added by AC #6. The protocol-crate unit tests (`event_kind_unknown_variant_round_trips_as_unknown`, `session_current_state_unknown_variant_round_trips_as_unknown`, `server_message_unknown_variant_round_trips_as_unknown`, `reaction_unknown_variant_round_trips_via_unknown`) pin the variants at the bare-string `serde_json::from_str::<EnumType>(...)` layer. The **corpus** is a different surface — it pins the FULL outbound envelope decode (`ServerMessage::Event { event: Event { kind: ... } }`, `ServerMessage::State { state: SessionState { current_state: ... } }`, `ServerMessage::Unknown` as the tagged-enum dispatch fallback).

Pre-gap corpus had only `event-with-unknown-reaction.json` exercising this surface. Three matching fixtures + three matching tests were missing:

| Added fixture | Added test | What it pins |
|---|---|---|
| `tests/fixtures/protocol-v1-corpus/event-with-unknown-kind.json` | `event_with_unknown_kind_decodes_via_unknown` | Future v1.x `Event.kind = "SubAgentSpawn"` decodes through `ServerMessage::Event` → `Event` → `EventKind::Unknown` |
| `tests/fixtures/protocol-v1-corpus/state-unknown.json` | `state_unknown_decodes_via_unknown` | Future v1.x `SessionCurrentState = "Compacting"` decodes through `ServerMessage::State` → `StateFrame` → `SessionState` → `SessionCurrentState::Unknown` |
| `tests/fixtures/protocol-v1-corpus/server-message-unknown.json` | `server_message_unknown_op_decodes_via_unknown` | Future v1.x top-level `op = "telemetry"` decodes to `ServerMessage::Unknown` via the tagged-enum `#[serde(other)]` catch-all (the Story 2.1 surface re-confirmed by AC #6) |

Why this matters: AC #6's load-bearing claim is that "v1.0 presenters continue to deserialize all five enums without modification." The protocol-crate tests verify the variant-level decode; the corpus verifies the wire-envelope-level decode. Both layers need to stay green together — a future refactor that changed the tagged-enum dispatch could silently break envelope decode while keeping variant decode intact. The new fixtures pin that interaction.

The `every_corpus_file_is_valid_json` floor was also tightened from `count >= 15` to `count >= 20` so a future deletion of any of the three new fixtures (or any of the existing 17) fails the gate.

## What we did NOT add

- **No new "API tests" or "E2E tests"** in the conventional sense — bowerbird has no UI surface, and Story 4.4 adds no HTTP routes. Coverage on the existing REST routes lives in `crates/daemon/tests/contract_daemon.rs` and `tests/cli_*.rs` (untouched).
- **No chaos-injection PRs** — Tasks 4.3 and 7.7 require opening real draft PRs to verify the bench gates fire under deliberate slowdowns on macOS + Linux runners. This is a manual user-driven action; the QA workflow cannot open draft PRs. Listed as deferred user-action items in the story's Completion Notes.
- **No daemon-bench baseline seeding** — the committed baselines at `crates/daemon/benches/baselines/{macos,linux}.json` carry zero-seeded p99 values per the same artifact-upload pattern the shim bench uses. First green CI run uploads the real values for the maintainer to commit.
- **No new contract surface beyond the 10** — the inventory test enforces `count >= 10`, so future stories can grow the surface (e.g., second adapter disambiguation) but never shrink it. Adding to the inventory is a story-level decision, not a QA gap closure.

## Coverage

- **Wire-shape compatibility (AC #2 / corpus):** 20 fixtures covering every public outbound type that has shipped under v1.x, plus the three Unknown-variant envelope fixtures added in this pass. Every fixture traces back to a `docs/protocol-changelog.md` entry.
- **Contract test inventory (AC #3):** 11 named surfaces (10 required + 1 redundant SIGKILL coverage); inventory matches actual fn/mod/landmark presence in all four target files.
- **Wire-enum sweep (AC #6):** all four `Unknown` variants (`ServerMessage`, `EventKind`, `SessionCurrentState`, `Reaction`) covered at both layers (bare-string protocol-crate tests + full-envelope corpus runner). The `ClientMessage` strict-by-design exception is documented in the enum's source comment and pinned by `inbound_type_rejects_unknown_fields`.
- **Cross-version upgrade (AC #5):** test SKIPs cleanly on this checkout (no v0.1.0 prior tag yet); release-pipeline CI lane will exercise it once v0.1.x ships.
- **Bench gates (AC #4 + AC #7):** existing shim hot-path gate verified by passing tests; new daemon bench infrastructure in place with placeholder baselines. Chaos-injection sanity check deferred to user.
- **Workspace test count:** **417 passed** across 25 suites in ~23s wall-clock (was 414 before this gap closure; +3 from the new corpus tests).

## Validation results

- `cargo fmt --check` — clean.
- `cargo clippy --workspace --all-targets -- -D warnings` — clean.
- `cargo test --workspace -- --test-threads=1` — 417 passed.
- `cargo test --test protocol_changelog_gate` — 1 passed.
- `cargo test --test protocol_v1_compat` — 21 passed (was 18 pre-gap-close).
- `cargo test --test contract_test_inventory` — 2 passed.
- `cargo test --test cross_version_upgrade` — 1 passed (SKIPs without env var; clean exit).

## Files touched in this QA pass

**NEW fixtures (3):**
- `tests/fixtures/protocol-v1-corpus/event-with-unknown-kind.json`
- `tests/fixtures/protocol-v1-corpus/state-unknown.json`
- `tests/fixtures/protocol-v1-corpus/server-message-unknown.json`

**UPDATED:**
- `tests/protocol_v1_compat.rs` — three new test fns (envelope-level `Unknown` decode); corpus-floor count bumped from `>= 15` to `>= 20`.

No protocol-crate source files changed in this QA pass, so the protocol-changelog gate is not triggered.

## Next steps

- Open the two chaos-injection PR pairs (Task 4.3 for shim, Task 7.7 for daemon) to demonstrate both bench gates fire on macOS + Linux runners. These are user-driven actions; the story's Completion Notes documents them.
- After the first green CI run on this branch, download `daemon-bench-macos-latest` and `daemon-bench-ubuntu-latest` artifacts and commit the real p99 values to the per-platform baseline files.
- Once v0.1.0 ships, verify the release-pipeline `cross-version-test` job exercises `tests/cross_version_upgrade.rs` against the prior tag instead of SKIPping.
- Close taskwarrior `a2ea3bfb` (deadlock test) post-merge with the resolving commit SHA annotation.
