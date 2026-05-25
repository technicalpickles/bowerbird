# Test Automation Summary — Story 4.3

Generated 2026-05-25 via `bmad-qa-generate-e2e-tests`. Supersedes the Story 4.2 summary.

## Context

Story 4.3 ships a documentation suite (`docs/quickstart.md`, `docs/presenter-authoring.md`, `docs/protocol.md`, `docs/no-list.md`, `docs/cookbook/*.md`) plus structural doc-drift guardrails in `tests/cli_docs_drift.rs` (Task 6) and a single README-coupling test added to `tests/release_pipeline_docs.rs` (Task 6.9).

This is a docs-only story for a Rust workspace — no UI, no API surface added, so the workflow's conventional "API tests / E2E tests" boxes don't apply. The analogous QA surface is **content-drift guardrails**: hermetic Rust tests that read the shipped markdown and assert load-bearing markers are present. The existing tests in `tests/cli_docs_drift.rs` cover *structural* drift (file existence, section ordering, cookbook-anchor byte-equality, link resolution). This run adds *content* drift coverage.

## Baseline coverage already in place

Before this QA pass:

- **`tests/cli_docs_drift.rs`** (6 tests) — `required_docs_exist`, `every_cookbook_entry_has_canonical_four_sections`, `cookbook_include_directives_match_example_anchors`, `every_cookbook_anchor_in_examples_has_a_cookbook_entry`, `quickstart_internal_links_resolve`, `architecture_md_docs_tree_matches_shipped_surface`. Pins structural shape: the five required docs exist, cookbook entries follow the four-section recipe, cookbook code blocks are byte-identical to the example anchors they reference (bidirectional), internal markdown links resolve to files on disk, and architecture.md's `docs/` tree matches the shipped surface (not the stale `docs/architecture/` + `docs/api/` placeholders).
- **`tests/release_pipeline_docs.rs::readme_links_to_quickstart_and_protocol_docs`** (1 test, Task 6.9) — README.md links to `docs/quickstart.md` and `docs/protocol.md` and the "in flight under Story 4.3" placeholder is gone.

Total Story-4.3 doc-drift tests pre-QA: **7**.

## Framework

- Rust `#[test]` functions in workspace-root test crates (`tests/*.rs`).
- Invocation: `cargo test --workspace -- --test-threads=1` (Epic 2 retro AI-3 / Story 3.4 AC #6).
- Dependencies: `std::fs`, `std::path`, `pretty_assertions` (already transitive via `assert_cmd`). No new deps.

## Gaps discovered and filled

Audit of the seven story ACs against existing tests revealed that file-existence + section-ordering tests do not catch silent paraphrasing of AC-mandated marker strings. Added seven content-drift tests to `tests/cli_docs_drift.rs`:

| Test | AC | What it pins |
|---|---|---|
| `quickstart_carries_load_bearing_markers` | #1 | Five-step walkthrough commands (`bowerbird start/replay/stop/auth token`, `BOWERBIRD_TOKEN`, `--experimental-strip-types`), Node 22.6+ floor, the troubleshooting grep-target sentence (`should now see` + `{event:"state"` + `scrolling on stdout`), three forward pointers (`docs/presenter-authoring.md`, `docs/protocol.md`, `docs/cookbook/`). |
| `presenter_authoring_carries_load_bearing_markers` | #2 | Six required sections in order (substrate model → WS connection → Subscribe → ServerMessage handler → dropped-frame recovery → REST snapshot), seven ServerMessage variants (`hello`/`event`/`state`/`sync`/`dropped`/`close`/`Unknown`), six topic-grammar entries, Bearer-auth + `server.json` markers. |
| `protocol_md_lists_eight_rest_routes` | #3 b | Eight REST routes declared in `crates/daemon/src/api/mod.rs`: `/healthz`, `/readyz`, `/status`, `/sessions`, `/sessions/{id}`, `/sessions/{id}/events`, `/sessions/{id}/stats`, `/replay`. |
| `protocol_md_documents_wire_surface_variants` | #3 d | Two `ClientMessage` variants + seven `ServerMessage` variants as `### `-level headings, plus per-frame type names (`HelloFrame`, `EventFrame`, `StateFrame`, `SyncFrame`, `DroppedFrame`, `CloseFrame`) sourced from `crates/protocol/src/ws.rs`. |
| `protocol_md_covers_topic_grammar_wire_format_and_ingest_contract` | #3 a + e + f | Wire-format conventions (`deny_unknown_fields`, `protocol_version`, `Bearer`), six topic-grammar entries, ingest-socket contract markers (`ingest.sock`, `hook_kind`, `0600`). |
| `no_list_enumerates_thirteen_scope_cuts_with_intentional_framing` | #5 | Opening "intentional / non-targets" framing + all thirteen scope cuts (a–m): No Windows, No distro packaging, No HITL, No tool blocking, No personas, No LAN, No daemon-side activity-rate, No crates.io, No `bowerbird gc`, No musl, No code signing, No structured JSON logging, No rate limiting. |
| `cookbook_readme_lists_three_required_entries_paired_with_examples` | #4 | Cookbook README table lists the three V1 entries (`state-session-fanout.md`, `rest-cursor-pagination.md`, `dropped-frame-recovery.md`) paired with their example tools (`multi-session-router`, `event-log-viewer`, `reconnect-recovery`). |

Pattern: `assert_contains_all(label, body, &[needle, …])` — same shape as `tests/release_pipeline_docs.rs::WALKTHROUGH_MARKERS` (Story 3.4 AC #5). The helper is local to the crate; no shared-module extraction since duplication is small and the helper is idiomatic.

## What we did NOT add

- **No "API tests"** — Story 4.3 adds no HTTP routes. Existing route-shape tests live in `tests/cli_*.rs` (untouched).
- **No E2E browser tests** — bowerbird has no UI surface.
- **No Node-spawning smoke for the quickstart walkthrough** — `tests/cli_examples.rs` already covers Node-side example invocation; the quickstart's *commands* are pinned at substring granularity in the new content test, which is the right tradeoff for V1.
- **No new test for AC #7 (architecture.md reconciliation)** — the existing `architecture_md_docs_tree_matches_shipped_surface` already pins it bidirectionally (must contain shipped paths; must NOT contain stale paths).

## Coverage

- **Story 4.3 ACs**: 7/7 ACs have at least one structural OR content guardrail. ACs #1, #2, #3, #4, #5 each got an additional content-marker test in this run; ACs #6 + #7 were already saturated by the structural tests.
- **Doc-drift test count**: `tests/cli_docs_drift.rs` 6 → **13 tests** (+7). `tests/release_pipeline_docs.rs` unchanged (already has `readme_links_to_quickstart_and_protocol_docs`).
- **All tests pass**: `cargo test --test cli_docs_drift -- --test-threads=1` → 13 passed. `cargo fmt --check` clean. `cargo clippy --test cli_docs_drift -- -D warnings` clean.

## Checklist validation

Mapped to `.claude/skills/bmad-qa-generate-e2e-tests/checklist.md`:

- [x] API tests generated (if applicable) — N/A; docs-only story.
- [x] E2E tests generated (if UI exists) — N/A; no UI.
- [x] Tests use standard test framework APIs — `#[test]`, `std::fs`, `assert_eq!`, `pretty_assertions::assert_eq!`.
- [x] Tests cover happy path — every test asserts the AC-mandated content is present.
- [x] Tests cover 1-2 critical error cases — `architecture_md_docs_tree_matches_shipped_surface` and the cookbook bidirectional integrity test both have negative assertions (stale paths must NOT be present; orphan anchors must NOT exist).
- [x] All generated tests run successfully — 13/13 passing.
- [x] Tests use proper locators — substring matches against load-bearing tokens, not brittle line numbers.
- [x] Tests have clear descriptions — each test name describes the AC it pins; failure messages explain what's missing and why it matters.
- [x] No hardcoded waits or sleeps — hermetic file reads, no async, no I/O beyond `fs::read_to_string`.
- [x] Tests are independent (no order dependency) — each test reads its own files; no shared state.

## Next steps

- The seven new tests run automatically under `cargo test --workspace -- --test-threads=1` on every PR (CI gate from Story 3.4 AC #6).
- If a future story renames a route, a frame variant, or a scope cut, the corresponding doc-drift test fails with a message naming the missing marker — making the doc-update obligation visible in the same PR that changes the wire surface.
- If a doc rewrite paraphrases an AC-mandated phrase (e.g. drops "intentional non-targets" from `no-list.md`), the test names the missing substring so the fix is mechanical.

No deferred work surfaced.
