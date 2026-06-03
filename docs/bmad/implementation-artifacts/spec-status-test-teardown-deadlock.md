---
title: 'Fix SQLite teardown deadlock in in-process REST oneshot tests'
type: 'bugfix'
created: '2026-06-03'
status: 'done'
baseline_commit: '880d94a15fac240e474cba51e107080b92420420'
context:
  - '{project-root}/docs/bmad/implementation-artifacts/investigations/test-serialization-investigation.md'
---

<frozen-after-approval reason="human-owned intent — do not modify unless human renegotiates">

## Intent

**Problem:** `contract_daemon.rs::story_1_7_rest::status_returns_none_last_event_when_only_sentinels` hangs intermittently (~1-in-5, even in isolation), which stalls the whole `--test-threads=1` CI suite. Root cause (investigation Follow-up 2026-06-03): it creates deadpool-sqlite pools and a `TempDir`, then drops them at scope exit without the explicit ordered teardown that prevents the documented `sqlite3_close → sqlite3_mutex_enter → pthread_mutex_wait` deadlock. The audit found this is a *class* defect: ~21 in-process `oneshot` tests that check out a DB connection share the identical missing guard; the status test is just the one observed losing the race.

**Approach:** Add one shared async test helper (`teardown_pools`) that drops the pools, yields so SQLite's connection-close finalizers run, then drops the `TempDir` — mirroring the proven inline fix at `state_plus_event_atomicity_under_sigkill_during_load` (contract_daemon.rs:2471-2482). Apply it to the in-process `oneshot` tests that actually exercise a pool connection. Leave the connection-less `oneshot` tests (auth-rejection, healthz, 400-validation) untouched — they never check out a connection, so they carry no deadlock risk.

## Boundaries & Constraints

**Always:** Replicate the existing proven ordering (drop pools → `tokio::task::yield_now().await` → drop tmp → yield). Keep each test's existing assertions byte-for-byte; this is teardown-only. Tests stay on their current runtime flavor. Helper lives at file top-level alongside `fresh_pools` so all submodules can call it via `super::teardown_pools`.

**Ask First:** If verification shows the helper does NOT eliminate the hang (i.e., the loop repro still stalls), HALT — the mechanism would be wrong and the fix needs rethinking, not more yields. If applying the helper requires changing any assertion or control flow in a test (beyond binding `tmp`/`pools` as locals and adding the trailing call), HALT and surface it.

**Never:** Do not touch the connection-less `oneshot` tests (no pool checkout = no risk; churning them adds noise). Do not introduce a new dependency (no cargo-nextest in this pass — that's the separate Symptom-B follow-up). Do not change production code in `crates/daemon/src/**` — this is a test-harness fix only. Do not remove or weaken the existing inline teardown at 2471-2482.

</frozen-after-approval>

## Code Map

- `crates/daemon/tests/contract_daemon.rs:23-29` -- `fresh_pools()` helper; the new `teardown_pools` helper goes adjacent at top-level.
- `crates/daemon/tests/contract_daemon.rs:2471-2482` -- canonical inline teardown + the doc-comment (2194-2209) documenting the deadlock mechanism. The pattern to replicate.
- `crates/daemon/tests/contract_daemon.rs:3378` -- `status_returns_none_last_event_when_only_sentinels`, the observed hang.
- `crates/daemon/src/db/pool.rs:19-44` -- pool config (5s wait caps confirm a >60s hang is a true deadlock, not a timeout). Read-only context.

## Tasks & Acceptance

**Execution:**
- [x] `crates/daemon/tests/contract_daemon.rs` -- Add top-level `async fn teardown_pools(pools: DbPools, tmp: TempDir)` (drop pools → yield → drop tmp → yield) with a doc-comment pointing at the deadlock mechanism. -- Single source of the ordered teardown so individual tests can't forget it.
- [x] `crates/daemon/tests/contract_daemon.rs` -- In `mod story_1_7_rest`, apply the helper to the 15 pool-exercising oneshot tests: `sessions_list_returns_known_sessions_with_read_time_state`, `sessions_list_applies_stale_working_fallback`, `sessions_detail_returns_projection_state`, `sessions_rest_surfaces_cwd_and_started_at`, `events_rest_surfaces_event_cwd`, `sessions_detail_returns_404_when_unknown`, `events_list_returns_all_in_ascending_order`, `events_list_returns_404_for_unknown_session`, `events_list_respects_since_cursor`, `events_list_oldest_available_after_purge`, `sessions_stats_returns_stats_for_known_session`, `sessions_stats_returns_404_when_unknown`, `stats_first_event_at_min_diverges_from_started_at_under_nonmonotonic_created_at`, `status_returns_uptime_and_last_event`, `status_returns_none_last_event_when_only_sentinels`. Per test: bind `tmp`/`pools` as live locals, pass `pools.clone()` into the state, append `super::teardown_pools(pools, tmp).await;`. -- Eliminate the deadlock across the whole REST module.
- [x] `crates/daemon/tests/contract_daemon.rs` -- Apply the helper to `readyz_returns_503_before_migrations_complete` (root mod), `events_404_for_unknown_session` + `events_200_for_existing_session_with_no_new_events` (`mod story_5_4_events_404`), and `sessions_unfiltered_unchanged` + `sessions_since_lower_bound` + `sessions_limit_caps_rows` (`mod story_5_8_session_filter`). -- The remaining 6 pool-exercising oneshot tests outside `story_1_7_rest`.

**Implementation notes (2026-06-03):** All 21 tests edited; helper added at file top-level. `cargo fmt --check` clean, `cargo clippy -p bowerbird-daemon --tests` clean (no warnings), `story_1_7_rest` module green. No `drop(app)` needed — every target test's final request is a bare `app.oneshot(...)` that consumes `app` before teardown.

**Honest verification status (correcting an earlier over-claim):** This change is **defense-in-depth, not independently proven.** It applies the team's own canonical teardown pattern (`contract_daemon.rs:2471`, the validated fix for this exact `sqlite3_close` deadlock) to the test that was actually *observed* hanging plus its REST siblings. It is harmless and cannot regress the passing tests. But I could **not** reproduce the deadlock to prove the fix works: the original unfixed racy drop ran **50/50 clean** (30× direct binary + 20× `cargo test`) on a quiet machine. The hang appears **load-correlated** (it surfaced only while another session was concurrently hammering this worktree), which is the same trigger profile as Symptom B — so my earlier "65/65 verified" was the bug not reproducing, not proof of the fix. See the investigation case file's 2026-06-03 follow-up for the load-correlation finding.

**Known incompleteness (deferred, see Spec Change Log):** A reliable parser-based re-audit found this fix covers only ~21 of **79 in-process `fresh_pools` tests** (every `fresh_pools` test opens a migration writer connection, so the "connection-less" exclusion in the frozen Intent is factually wrong), plus ~63 real-server `fresh_pools` tests are entirely out of scope. The systemic fix (un-missable guard) was scoped out per user decision; recorded as follow-up.

**Acceptance Criteria:**
- Given `status_returns_none_last_event_when_only_sentinels`, when run 20× in isolation (`--exact`, each under a 30s timeout), then 20/20 pass with zero stalls.
- Given the daemon contract binary, when `cargo test -p bowerbird-daemon --test contract_daemon -- --test-threads=1`, then all tests pass and no run exceeds its previous wall-clock baseline by more than noise.
- Given the connection-less oneshot tests (auth-rejection / healthz / 400-validation), when the diff is reviewed, then none of them were modified.
- Given `cargo clippy --all-targets -- -D warnings` and `cargo fmt --check`, when run, then both pass clean.

## Spec Change Log

**2026-06-03 — step-04 review (no code re-derivation; user chose to ship the 21 as defense-in-depth).** Three adversarial reviewers + a reliable parser-based re-audit surfaced three things the original spec got wrong, none of which the user chose to fix in this pass:

1. **Scope was wrong (under-counted).** The frozen Intent classified the `story_5_8_session_filter` and auth/healthz tests as "connection-less" and excluded them. That is factually incorrect: `fresh_pools()` runs `run_migrations()`, which checks out a writer connection, so **every** `fresh_pools` test holds an open connection at teardown. The true at-risk set is **79 in-process tests** (this fix covers 21) **+ 63 real-server tests** (out of scope). The "connection-less" wording in the frozen block is superseded by this entry. **Deferred** — not corrected here.
2. **The `yield_now()` ordering is not a hard barrier.** `deadpool-sync` closes connections on a detached `spawn_blocking` task that `yield_now()` does not join; the fix narrows the race rather than provably closing it (a limitation it shares with the canonical inline fix at `:2471`). **Deferred** to a systemic fix (deterministic close / un-missable guard).
3. **Could not reproduce the hang to verify any fix.** Unfixed control ran 50/50 clean on a quiet machine; the hang correlates with concurrent worktree load (same profile as Symptom B). Earlier "65/65 verified" was the bug not firing, not proof. Recorded honestly above; the load-correlation finding is added to the investigation case file.

**KEEP (survives any future re-derivation):** the `teardown_pools` helper + the per-test transformation pattern are correct and verified-consistent with the canonical fix; the determination that every edited test consumes `app` before teardown (so the local `pools` is the last ref) is verified. Rejected as noise: the blind reviewer's "`app.clone()` leaves the fix a no-op" findings (refuted — every edited test's final request consumes `app`).

## Design Notes

The helper and a before/after for the golden case:

```rust
/// Ordered teardown for in-process pool tests: drop the pools so SQLite's
/// connection-close runs, yield so those finalizers complete, THEN drop the
/// TempDir that removes `bower.db`. Prevents the intermittent
/// `sqlite3_close → sqlite3_mutex_enter → pthread_mutex_wait` deadlock — same
/// fix as the inline block in `state_plus_event_atomicity_under_sigkill...`.
async fn teardown_pools(pools: DbPools, tmp: TempDir) {
    drop(pools);
    tokio::task::yield_now().await;
    drop(tmp);
    tokio::task::yield_now().await;
}
```

```rust
// before:  let (_tmp, pools) = fresh_pools().await;
//          let app = api::router(ready_state(pools));
//          ...asserts...
// after:   let (tmp, pools) = fresh_pools().await;
//          let app = api::router(ready_state(pools.clone()));
//          ...asserts (unchanged)...
//          super::teardown_pools(pools, tmp).await;
```

`pools.clone()` is an Arc bump (deadpool pools are clone-by-design); the router's clone is dropped when `oneshot` consumes the service, so the local `pools` is the last ref and its `drop` triggers the close before the yield. Root-mod tests call `teardown_pools(...)` directly (no `super::`).

## Verification

**Commands:**
- `for i in (seq 20); RTK_DISABLED=1 timeout 30 cargo test -p bowerbird-daemon --test contract_daemon -- --test-threads=1 --exact story_1_7_rest::status_returns_none_last_event_when_only_sentinels; or echo "HANG iter $i"; end` -- expected: 20 passes, zero "HANG" lines.
- `RTK_DISABLED=1 cargo test -p bowerbird-daemon --test contract_daemon -- --test-threads=1` -- expected: all pass.
- `cargo fmt --check && cargo clippy --all-targets -- -D warnings` -- expected: clean.
