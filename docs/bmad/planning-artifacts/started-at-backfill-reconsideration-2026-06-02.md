# Decision brief: legacy `started_at` backfill vs. preserve-`None`

Date: 2026-06-02
Status: **Open — needs a call from @pickles**
Context: Story 5.7 (`5-7-session-cwd-on-the-wire`), ADR 0006, four code-review passes
Author: dev-story (Amelia) at @pickles' request

## TL;DR

The legacy-`started_at` backfill has thrown a code-review finding on **every one of the four passes** Story 5.7 has been through. That is not four unrelated bugs — it is one design (reconstruct `started_at` from the event log on the fly, inside the live write path) leaking a new edge each time. This brief lays out *why* it keeps leaking and asks for a deliberate choice between **keeping the backfill** and **dropping it for preserve-`None`**, with the four-pass cost as the evidence.

Pass 4's fix is correct and tested; this brief does not block it. But the fix stands or falls with the same choice — preserve-`None` would delete the backfill *and* the pass-4 predicate term together.

## The pattern (the evidence)

Every pass touched the legacy `started_at` backfill, each finding a different edge of the same design:

| Pass | Finding on `started_at` backfill | Change Log |
|---|---|---|
| 1 | **[Decision]** What semantics for a legacy row whose blob has `started_at: None`? Chose: backfill from `MIN(events.created_at)`. | 0.2 |
| 2 | **[Patch]** `MIN` is wrong — `rebuild_missing_projections` folds by `event_id ASC` and keeps the first folded event's timestamp, so under non-monotonic `created_at` they diverge. Switched to first-event-by-`event_id`. | 0.4 |
| 3 | **[Patch]** ADR 0006 + protocol/presenter/changelog docs still described `MIN` / "null forever." Corrected the wording. | 0.6 |
| 4 | **[Patch]** The backfill changes `started_at` (`None → Some`) but the post-commit `State` publish predicate only compared `current_state`, so a same-state write fixed storage/REST but never pushed a live `StateFrame`. Added a `started_at_changed` predicate term. | 0.8 |

Four passes, four findings, one feature.

## Why it keeps leaking

The backfill exists for exactly one reason: **to hold "the projection is a byte-identical function of the event log" across the upgrade boundary.**

That guarantee is load-bearing in the architecture:
- ADR 0006 (Decision, and Consequences line 31/49): `started_at` "reconstructs identically on rebuild."
- `project-context.md` §"Required contract tests" → *Projection rebuild from event log*: "Delete the projection table, restart, rebuild from the event log; assert byte-identical to pre-delete state. This is the 'is the event log actually the source of truth' test."

For a session the daemon projects fresh under v5.7+, live ingest and a full rebuild both set `started_at` on the first event — identical, no special handling. The problem is **only** the upgrade window: a pre-5.7 projection blob deserializes with `started_at: None`, `rebuild_missing_projections` skips rows that already have a projection, and `transition`'s set-once rule (`prev.started_at.or(Some(now_ms))`) would otherwise stamp the *next post-upgrade event's* clock onto a session that began before the upgrade.

To make the **live** path equal a **full rebuild** for those legacy rows, the daemon has to reconstruct `started_at` from the event log inside `write_inner`. Every pass has been another spot where "live write" and "full rebuild" don't line up:

- *which* event's timestamp (rebuild folds by `event_id`) — passes 1–2
- the docs describing it — pass 3
- the live broadcast (rebuild is silent; the live write *publishes*, and the gate didn't know `started_at` had moved) — pass 4

All of this machinery — a dedicated query (`SELECT_FIRST_EVENT_CREATED_AT_FOR_SESSION`), a write-path read inside the writer txn, a 5th closure-result tuple element, a publish-predicate term, and ~4 tests — serves a **transitional** concern: rows written *before* this story, only until they are rebuilt or naturally re-projected. `started_at` is non-`None` for every row the daemon writes from here on.

## The options

There are three behaviors for a legacy row (blob has `started_at: None`) on its next post-upgrade write. Note that **two of the three diverge from a full rebuild** — only the backfill makes live == rebuild:

### A. Keep the backfill (current state)

`write_inner` reads the first event by `event_id` and restores it before `transition`.

- **Live == rebuild** for legacy rows. Byte-identical-across-upgrade holds.
- **Cost:** the four-pass complexity above. Pass 4 was (plausibly) the last edge — but "plausibly" is doing work; this is the design that surprised us four times.
- Pass 5 should confirm clean.

### B. Preserve `None` (drop the backfill)  ← recommended

Change `transition` so a legacy row keeps `started_at: None` instead of stamping `now_ms` (i.e. only set it when there is no prior projection at all). Legacy rows read `null` until a full rebuild reconstructs them.

- **Honest:** we genuinely did not record `started_at` before 5.7. `null` = "unknown," which is true. A presenter already has to handle `null` (pre-5.7 rows, non-Claude sources, producers that omit it) — this just widens that window slightly.
- **Deletes** the backfill query, the write-path read, the pass-4 predicate term, and their tests. The recurring-finding surface goes away.
- **Does NOT break the existing byte-identical contract test** — that test exercises rows the daemon projected under v5.7+ (live-then-rebuild), which stay identical. The divergence is only for hand-seeded pre-5.7 blobs, which the test does not cover. We would narrow the guarantee's *wording* to "holds for rows projected under v5.7+; legacy rows read `null` until rebuilt," which is what the test actually verifies anyway.
- **Cost:** a legacy row shows `null` age until a rebuild (`bowerbird` restart with a deleted projection table, or natural re-projection). For a workstation tool during a one-time upgrade, this is mild — and arguably more correct than a backfilled-but-could-be-wrong value.

### C. Accept the approximation (do nothing)

Let the set-once rule stamp `now_ms` on the legacy row's next event (the pre-pass-1 behavior).

- **Rejected then, still worse than B:** a legacy session renders a false "started just now," which is actively misleading, vs. B's honest `null`. Same simplicity as B with worse UX. Not recommended.

## Recommendation

**Option B (preserve-`None`).** The backfill buys byte-identical-across-upgrade for a field that is `null`-tolerant by construction and only `null` during a one-time upgrade window. That guarantee is stricter than the contract test verifies, and paying for it has cost four review passes. Preserve-`None` is simpler, honest (`null` = unknown), and removes the surface that keeps generating findings. The byte-identical guarantee stays intact for everything the daemon writes going forward — we just stop trying to retroactively satisfy it for rows that predate the field.

This is @pickles' call: it refines an **Accepted** ADR (0006) and narrows a documented contract. Both are legitimate to change with a new/updated ADR, but it should be deliberate, not a quiet code edit.

## If we pick B — what it takes

- `crates/daemon/src/projection/state.rs::transition` — set `started_at` only when there is no prior projection (`prev.is_none()`); a prior row with `started_at: None` keeps `None`.
- `crates/daemon/src/projection/session.rs::write_inner` — delete the legacy-backfill read and the `prev_started_at` capture; revert the publish predicate to `current_state`-only (the pass-4 `started_at_changed` term is no longer needed — `started_at` no longer changes on a legacy same-state write).
- `crates/daemon/src/db/queries.rs` — remove `SELECT_FIRST_EVENT_CREATED_AT_FOR_SESSION`.
- Tests — remove `legacy_started_at_backfills_by_first_event_order_under_nonmonotonic_created_at` and `legacy_started_at_backfill_publishes_state_frame_on_same_state_event`; add one asserting a legacy row stays `None` on its next write and reconstructs on a full rebuild.
- ADR 0006 — replace the "Legacy projection rows backfill" consequence (line 49) with the preserve-`None` decision + the narrowed byte-identical wording; add a "Revisit when" or supersede note.
- Docs — `protocol.md`, `presenter-authoring.md`, `protocol-changelog.md`: pre-5.7 `started_at` reads `null` until rebuilt (drop the "backfilled on next write" wording).
- `deferred-work.md` — update the Story-5.3 item #3 resolution note to match.

## If we pick A — what it takes

- Nothing new. Run code-review pass 5 (fresh context, different LLM per the workflow tip) to confirm the live-publish edge was the last one.

## Suggested vehicle

If B: run `bmad-correct-course` (this is a mid-sprint design change to an in-flight story) or author an ADR refining 0006, then the code/doc edits above land in the same PR. If A: no process change; proceed to pass 5.
