# Story 5.5: Bench gates converted to load-bearing

Status: ready-for-dev

<!-- Note: Validation is optional. Run validate-create-story for quality check before dev-story. -->

## Story

As a release manager,
I want every committed CI bench gate to fail loudly when a real regression lands,
so that the bench infrastructure is producing signal — not just running.

**Paperwork-flavored, human-in-the-loop, no net production code.** This story converts two already-built CI gates (`daemon-bench-gate`, `shim-bench-gate`) from "structurally complete but unarmed" to "load-bearing." The only files that change on merge are the two daemon baseline JSONs (zeros → real p99) and the strike-through of the tracking entries. Everything else (chaos-injection PRs) is opened, observed in CI, and reverted before merge — it never lands on `main`.

**Closes Epic 4 retro AI-1, AI-2, AI-3** (per `epic-4-retro-2026-05-25.md` §"Action items for V1 release readiness" table). Resequenced from 5.2 → 5.5 by `sprint-change-proposal-2026-05-27-epic-5-resequencing.md` (dogfooding-first ordering: bench-gate work doesn't unblock daily dogfooding).

**Why this story exists (the gap it closes):** A CI gate that has never been observed in failure mode is structurally aspirational, not load-bearing (epic-4-retro Discovery #5). Two specific holes today:
1. The daemon-bench baselines (`crates/daemon/benches/baselines/{macos,linux}.json`) ship with placeholder `*_p99_nanos: 0`. `scripts/check-daemon-bench-p99.py` **auto-skips the regression gate per-shape when baseline p99 is 0** (see the `baseline_p99 <= 0` branch). So a 29% — or 200% — regression in any shape passes CI silently. Only the absolute 100ms NFR2 ceiling fires.
2. Neither the daemon-bench gate (brand new in Story 4.4) nor the shim hot-path gate (Story 1.5 origin, but Task 4.3 deferred) has ever been deliberately tripped. "We know it compiles and runs; we don't know it fails when it should."

## Acceptance Criteria

1. **Given** `crates/daemon/benches/baselines/macos.json` and `linux.json` currently contain placeholder zero values **When** Story 5.5 lands **Then** both files contain non-zero p99 values per shape (solo, fanout3, burst, steady) sourced from the most recent green CI run on `main` (or the Story 5.5 PR's CI run if it's green); the bench gate `daemon-bench-gate` exercises the regression check without auto-skipping any shape (i.e. every shape's `*_p99_nanos` is `> 0`, so the `baseline_p99 <= 0` skip branch in `scripts/check-daemon-bench-p99.py` is no longer reachable for any shape).

2. **Given** the daemon-bench gate has never been exercised in failure mode **When** Story 5.5 lands **Then** the Dev Agent Record documents two chaos-injection sanity PRs (one macOS, one Linux) that injected `tokio::time::sleep(Duration::from_millis(50)).await` between `tx.commit()` and `broadcaster.publish` in `crates/daemon/src/projection/session.rs::write` (concretely: in the async body after `interact_res?` resolves at `session.rs:262` and before the `broadcaster.publish(BroadcastEnvelope::Event(...))` call at `session.rs:289`, since `tx.commit()` itself runs inside the synchronous `interact` closure where `.await` is illegal), verified CI's `daemon-bench-gate` failed on the burst-shape p99 regression, and were reverted before merge. Each PR's CI run URL + the failing gate's `::error::` line are captured in the Dev Agent Record.

3. **Given** the shim hot-path bench gate has never been exercised in failure mode (Story 4.4 Task 4.3 deferred) **When** Story 5.5 lands **Then** the Dev Agent Record documents two chaos-injection sanity PRs (one per platform) that injected a blocking sleep into the shim's hot path in `crates/shim/src/main.rs::run` (between the `socket::send` at `main.rs:69` and the prior work, or just before it), verified CI's `shim-bench-gate` failed on each platform, and were reverted before merge. **CRITICAL per-platform asymmetry (ADR 0003):** the Linux shim baseline has `regression_max_ratio: 1.35` against a ~1.19ms p99, so a `std::thread::sleep(Duration::from_millis(2))` injection trips the regression gate (~3.2ms > 1.61ms threshold). The **macOS** shim baseline has `regression_max_ratio: null` (regression gate disabled per ADR 0003 — `macos-latest` runner noise is 4.3×) and a **15ms absolute** ceiling, so a 2ms injection is a **no-op for the macOS gate** (~4.7ms < 15ms). To trip the macOS gate the injection must exceed the 15ms absolute budget (use `std::thread::sleep(Duration::from_millis(14))` or larger on the macOS PR). The Dev Agent Record states the per-platform injection magnitudes used and notes that the macOS shim gate is absolute-only by design.

4. **Given** the work is paperwork-flavored (no production code changes after the chaos PRs are reverted) **When** Story 5.5 closes **Then** Epic 4 retro action items AI-1, AI-2, AI-3 in `docs/bmad/implementation-artifacts/epic-4-retro-2026-05-25.md` §"Action items for V1 release readiness" are struck through (or their Status-criteria cell marked ✅ Resolved) with a backlink to this story's merge commit. **Note:** `deferred-work.md` does NOT currently carry standalone entries named AI-1/AI-2/AI-3 (verified by grep); the canonical home for these three is the epic-4-retro Action Items table. If a `docs/release-checklist.md` (epic-4-retro AI-9) has been authored by merge time, also tick the corresponding pre-flight items there. Do not invent new deferred-work entries to strike through.

## Tasks / Subtasks

- [ ] **Task 0: Confirm a green CI run on `main` with daemon-bench artifacts exists** (AC: 1)
  - [ ] `gh run list --branch main --workflow ci.yml --limit 10` to find the most recent green run that includes the `daemon-bench-gate` matrix jobs (macOS + Linux). Story 5.4 just merged (`de88c45` + CI-green fix `7f1e402`), so a recent green `main` run should exist.
  - [ ] If no green `main` run has the daemon-bench artifacts (e.g. the gate was added but artifacts expired), fall back to this story's own PR CI run once it's green — the AC explicitly permits sourcing from the PR run.

- [ ] **Task 1: Seed the daemon-bench baselines (AC #1)** (AC: 1)
  - [ ] `gh run download <run-id> -n daemon-bench-macos-latest` and `-n daemon-bench-ubuntu-latest` to pull each run's `target/daemon-bench-summary.json`. (Artifact names per `ci.yml:103,145`: `daemon-bench-${{ matrix.os }}` → `daemon-bench-macos-latest`, `daemon-bench-ubuntu-latest`.)
  - [ ] For `macos.json`: copy the four `*_p99_nanos` values from the macOS summary into `crates/daemon/benches/baselines/macos.json`. Keep `samples`, `absolute_budget_nanos: 100000000`, `regression_max_ratio: 1.30`. **Delete the `_seeding_note` field** (its presence is the signal the baseline is unseeded).
  - [ ] Same for `linux.json` from the ubuntu summary.
  - [ ] Sanity: all four `*_p99_nanos` are non-zero and well under 100ms (Story 4.4 smoke saw solo 1.713ms / fanout3 1.608ms / burst 1.928ms / steady 1.242ms — same order of magnitude is expected). If any shape's p99 is implausibly high (>10ms), the source run was noisy — pick a cleaner run.
  - [ ] Verify locally that the gate no longer auto-skips: `cargo bench -p bowerbird-daemon --bench hook_to_presenter` then `python3 scripts/check-daemon-bench-p99.py target/daemon-bench-summary.json crates/daemon/benches/baselines/<your-platform>.json` — confirm the output shows `regression gate OK` (not `regression gate skipped — baseline p99 is zero`).

- [ ] **Task 2: Daemon-bench chaos-injection sanity PRs (AC #2)** [HUMAN-IN-THE-LOOP — requires real draft PRs + CI]
  - [ ] Prepare the chaos patch: in `crates/daemon/src/projection/session.rs::write_inner`, after `interact_res?` resolves (~`session.rs:262`) and before `broadcaster.publish(BroadcastEnvelope::Event(event))` (~`session.rs:289`), insert `tokio::time::sleep(std::time::Duration::from_millis(50)).await;` with a `// CHAOS: revert before merge` comment.
  - [ ] Open one draft PR targeting the chaos against a branch CI runs (the gate runs on PRs per `ci.yml`). Because the matrix runs both OSes, a single draft PR exercises both — but the AC asks the record to attribute the burst-shape failure per platform, so capture both matrix legs' logs.
  - [ ] Observe `daemon-bench-gate` fail with `::error::burst: p99 regression gate FAILED: ...` on each platform. 50ms vs a ~1.9ms baseline × 1.30 ≈ 2.5ms threshold → fails by ~20×, comfortably. (50ms is also under the 100ms absolute ceiling, so it's the *regression* gate that fires, which is exactly the gate Task 1 just armed.)
  - [ ] Revert the chaos commit; confirm the draft PR's gate goes green again, then close/abandon the PR. The chaos NEVER merges to `main`.
  - [ ] Record both CI run URLs + the failing `::error::` lines in the Dev Agent Record.

- [ ] **Task 3: Shim hot-path chaos-injection sanity PRs (AC #3)** [HUMAN-IN-THE-LOOP — requires real draft PRs + CI]
  - [ ] **Linux PR:** inject `std::thread::sleep(std::time::Duration::from_millis(2));` into `crates/shim/src/main.rs::run` (e.g. just before `socket::send` at `main.rs:69`). Expect `shim-bench-gate` (ubuntu-latest leg) to fail the regression gate (~3.2ms > 1.19ms × 1.35 = 1.61ms). The macOS leg of THIS PR will NOT fail (2ms < 15ms absolute, regression disabled) — that's expected; document it.
  - [ ] **macOS PR:** inject `std::thread::sleep(std::time::Duration::from_millis(14));` (or larger) to exceed the 15ms absolute budget on `macos-latest`. Expect the macOS leg to fail with `absolute gate FAILED`. The Linux leg will also fail here (14ms ≫ regression threshold) — fine.
  - [ ] Revert each chaos commit; confirm green; close the draft PRs. Nothing merges.
  - [ ] Record both CI run URLs, the per-platform injection magnitudes (2ms Linux / 14ms macOS), the failing `::error::` lines, and a one-line note that the macOS shim gate is absolute-only by design (ADR 0003) so a small regression-style injection cannot exercise it.

- [ ] **Task 4: Strike through the tracking entries (AC #4)** (AC: 4)
  - [ ] In `docs/bmad/implementation-artifacts/epic-4-retro-2026-05-25.md` Action Items table, mark AI-1, AI-2, AI-3 as ✅ Resolved with a backlink to this story's merge commit (use the same strike-through/✅ convention the table already uses for cross-epic closures, e.g. the AI-style markers elsewhere in the retro).
  - [ ] If `docs/release-checklist.md` exists (epic-4-retro AI-9), tick the daemon-bench-seeding + chaos-injection pre-flight items there too. If it doesn't exist, do NOT create it — that's AI-9's job, out of scope here.
  - [ ] Do NOT add fabricated AI-1/2/3 entries to `deferred-work.md` — confirmed they don't exist there; the retro table is the canonical home.

- [ ] **Task 5: Verification + File List** (AC: all)
  - [ ] `cargo fmt --check` and `cargo clippy --all-targets --workspace -- -D warnings` — must be clean (the merged tree has no code changes, so this is a baseline-JSON + docs diff; still run it).
  - [ ] `cargo test --workspace -- --test-threads=1` green.
  - [ ] `cargo bench -p bowerbird-daemon --bench hook_to_presenter` + run the gate script against the newly-seeded baseline locally; confirm no shape reports `regression gate skipped`.
  - [ ] `cargo build -p bowerbird-shim --profile release-shim --locked` (shim profile preserved).
  - [ ] `git status --porcelain` and reconcile against the File List before declaring review (epic-4-retro Discovery #6 / AI-6: File-List-vs-git drift has bitten four prior stories — do not repeat it). Expected File List: the two daemon baseline JSONs, `epic-4-retro-2026-05-25.md`, this story file, `sprint-status.yaml`.

## Dev Notes

### What "load-bearing" means here (the core insight)
- The gates already **run** in CI (`ci.yml:72` shim, `ci.yml:107` daemon). They just don't **bite**: the daemon regression gate is unarmed (zero baselines auto-skip), and neither gate has been observed failing. This story arms the daemon regression gate (Task 1) and proves both gates fire (Tasks 2–3). The proof lives in the Dev Agent Record, not in committed code.
- Per Axiom 3 (`project-context.md:52`): the shim is across a hard trust boundary (lives in Claude's process), the daemon is inside our own. That's why the shim gate uses a tight 15% regression ratio (Linux) and the daemon uses a loose 30% — both are "real signal worth gating on" at their respective altitudes.

### The auto-skip branch you're closing (read this)
`scripts/check-daemon-bench-p99.py` lines ~156–159:
```python
if baseline_p99 <= 0:
    gh_notice(f"{shape}: regression gate skipped — baseline p99 is zero (uninitialized?).")
    continue
```
Today every daemon shape hits this branch (all baselines are 0). Task 1 makes all four `*_p99_nanos > 0`, so this branch becomes unreachable and the regression gate arms for solo/fanout3/burst/steady. The absolute 100ms gate (NFR2) was already armed regardless.

### Files that actually change on merge (vs. files touched-then-reverted)
- **Change on merge:** `crates/daemon/benches/baselines/macos.json`, `crates/daemon/benches/baselines/linux.json` (zeros → real p99, drop `_seeding_note`), `docs/bmad/implementation-artifacts/epic-4-retro-2026-05-25.md` (strike-through), this story file, `sprint-status.yaml`.
- **Touched then reverted (NEVER on main):** `crates/daemon/src/projection/session.rs` (chaos sleep), `crates/shim/src/main.rs` (chaos sleep). These are the chaos-injection PRs — they exist only to prove the gates fire.

### The macOS shim gate trap (the one thing most likely to make a dev "lie about completion")
ADR 0003 (`docs/decisions/0003-shim-p99-budget-on-macos-latest.md`) disabled the macOS shim **regression** gate (`regression_max_ratio: null`) and set a **15ms absolute** ceiling because `macos-latest` showed a 4.3× p99 spread (2.66 → 6.19 → 11.35ms) on *unchanged* code. Consequence: the epic's AC3 verbatim injection (`std::thread::sleep(2ms)`) trips the **Linux** gate but is a **no-op on macOS** (~4.7ms < 15ms). A dev who injects 2ms, sees Linux fail, and reports "shim gate verified on both platforms" has lied — the macOS gate didn't fire because nothing exceeded 15ms. The fix is a larger macOS injection (≥14ms) to trip the absolute ceiling. AC3 has been written to require per-platform injection magnitudes precisely to prevent this.

### The daemon chaos site is async, the commit is sync (don't put `.await` in the wrong place)
`session.rs::write_inner` does `tx.commit()?` **inside** the deadpool `interact(|conn| {...})` closure (`session.rs:250`), which is a synchronous `spawn_blocking` context — `.await` is illegal there. The publish happens **after** the closure returns, in the async fn body (`session.rs:289`). So "between commit and publish" = "after `interact_res?` at line 262, before `broadcaster.publish` at line 289." That's an `.await` point. Put the `tokio::time::sleep(...).await` there. (If you tried to sleep inside the closure you'd need `std::thread::sleep`, which would also work to inflate the timing but isn't where the AC says to put it.)

### Human-in-the-loop reality
Tasks 2 and 3 require opening real draft PRs and reading their CI results — the dev workflow historically "cannot open real draft PRs" (epic-4-retro Discovery #5, which is exactly why AI-2/AI-3 were deferred to "whoever cuts v0.1.0-rc1"). If you (the dev agent) have `gh` PR-creation authority in this session, drive it end-to-end and capture the run URLs. If not, prepare the exact chaos patches as ready-to-apply diffs in the Dev Agent Record, and surface to the human that Tasks 2–3 need a human to push the draft PRs and paste back the CI run URLs + `::error::` lines. Either way, AC2/AC3 are satisfied by *documented, CI-observed* failures — not by code reading.

### Anti-patterns to avoid
- **Do NOT raise a budget to make a number fit.** If a seeded baseline shows a real p99 that's surprisingly high, that's a finding (pick a cleaner run or investigate), not a reason to bump `absolute_budget_nanos`. PRD line 181 / ADR 0003: "if the number can't be met cleanly, the right response is an ADR."
- **Do NOT leave any chaos sleep on `main`.** The whole point is the gates are proven by transient PRs. A leftover `sleep` is a self-inflicted perf regression.
- **Do NOT delete the in-memory daemon-bench shapes or change the harness.** This story is data + docs + transient chaos. No bench-harness edits (`hook_to_presenter.rs`, `hot_path.rs` stay byte-identical).
- **Do NOT fabricate deferred-work strike-throughs.** AC4's target is the epic-4-retro table, confirmed by grep.

### Testing standards summary
- Bench harnesses are `harness = false` per-invocation timers (NOT Criterion) — Story 1.5 review finding 2: Criterion's flat sampling batches iterations and hides high-tail regressions (`crates/shim/benches/README.md`). Don't "improve" them to Criterion.
- Gate scripts are Python, mirror each other (`check-shim-bench-p99.py` ↔ `check-daemon-bench-p99.py`) — same CLI shape, same JSON schema floor. Don't diverge them.
- Per-platform baselines are committed files, updated deliberately by PR with reviewer sign-off (`project-context.md:631`). Auto-rolling baselines silently absorb regressions — never automate the seeding.

### Project Structure Notes
- Baselines live at `crates/daemon/benches/baselines/{macos,linux}.json` and `crates/shim/benches/baselines/{macos,linux}.json`. Schema documented in `crates/shim/benches/README.md` and the docstring of each gate script.
- CI jobs: `shim-bench-gate` (`ci.yml:72`) and `daemon-bench-gate` (`ci.yml:107`), both `fail-fast: false` matrix over `macos-latest` + `ubuntu-latest`, both upload the current-run summary as an artifact for seeding.
- No new files, no new crates, no protocol change → no ADR trigger (this story *operationalizes* ADR 0003's gate policy, it doesn't change it).

### References
- [Source: docs/bmad/planning-artifacts/epics.md#Story 5.5: Bench gates converted to load-bearing] (lines 1094–1118) — the four ACs.
- [Source: docs/bmad/implementation-artifacts/epic-4-retro-2026-05-25.md#Action items for V1 release readiness] — AI-1 (seed daemon baselines), AI-2 (daemon chaos PRs), AI-3 (shim chaos PRs); Discovery #3 (zero-seeded baselines), Discovery #5 (chaos PRs unverified), Discovery #6 (File-List drift); Team agreement A12 (manual user-action items → release pre-flight).
- [Source: docs/decisions/0003-shim-p99-budget-on-macos-latest.md] — macOS shim regression gate disabled, 15ms absolute budget; the per-platform asymmetry behind AC3.
- [Source: scripts/check-daemon-bench-p99.py] — the `baseline_p99 <= 0` auto-skip branch (lines ~156–159); absolute + regression gate logic.
- [Source: scripts/check-shim-bench-p99.py + crates/shim/benches/README.md] — shim gate idiom + seeding flow.
- [Source: crates/daemon/src/projection/session.rs:250,262,289] — `tx.commit()` (sync, in closure), `interact_res?`, `broadcaster.publish` (async body) — the daemon chaos site.
- [Source: crates/shim/src/main.rs:34-74] — `run()` hot path; `socket::send` at line 69 — the shim chaos site.
- [Source: crates/daemon/benches/baselines/{macos,linux}.json] — current zero placeholders + `_seeding_note`.
- [Source: crates/shim/benches/baselines/{macos,linux}.json] — macos `regression_max_ratio: null` / 15ms; linux `1.35` / 5ms.
- [Source: .github/workflows/ci.yml:72-146] — both bench-gate jobs + artifact upload.
- [Source: docs/bmad/project-context.md] — Axiom 3 (perf hard at boundaries, soft inside, line 52); bench thresholds (lines 617–633); "baselines are committed files, updated deliberately" (line 631).
- [Source: docs/bmad/implementation-artifacts/deferred-work.md] — confirmed: no standalone AI-1/AI-2/AI-3 entries; the hook→presenter bench entry (line 70) and `bowerbird install` entry (line 84) are already resolved by Story 4.4 / 5.4 respectively.

## Dev Agent Record

### Agent Model Used

claude-opus-4-8[1m] (Opus 4.8, 1M context) — dev-story session 2026-05-29.

### Debug Log References

**BLOCKED at Task 0 (2026-05-29): GitHub Actions billing failure — CI cannot run.**

Task 0 requires a green CI run on `main` with `daemon-bench-gate` artifacts to seed the baselines (AC1), and Tasks 2–3 require CI to observe the chaos-injection gate failures (AC2, AC3). Investigation found CI is non-functional:

- `gh run list --workflow ci.yml`: the **last successful run was 2026-05-20** (story-1.8 / epic-1-retro era, run `26178675374`). Every run since `26189080365` (2026-05-20T20:48) is `failure`.
- Run `26580304829` (latest on `main`, 2026-05-28) annotation on every job: *"The job was not started because recent account payments have failed or your spending limit needs to be increased. Please check the 'Billing & plans' section in your settings."* Jobs fail in 2–3s without starting.
- The `daemon-bench-gate` was introduced in Story 4.4 (2026-05-25), **after** billing broke, so it has never successfully executed — explaining the all-zero, never-seeded baselines this story was meant to arm.
- Latest run carries **0 artifacts**; nothing to download for seeding.

**Consequence for the ACs:**
- **AC1** — cannot source per-platform p99 from a green CI run or this PR's CI run; neither can be produced. Seeding locally on a darwin box would (a) violate the story's "baselines are CI-sourced, never auto-rolled / local-hardware" discipline (Dev Notes "Testing standards summary"; project-context.md:631) and (b) cannot produce a Linux baseline at all.
- **AC2 / AC3** — chaos-injection PRs cannot be observed failing in CI because CI jobs never start.
- **AC4** — striking through epic-4-retro AI-1/2/3 as "✅ Resolved" would be a lie while AC1–3 are unmet.

**Resolution required (user action):** restore GitHub Actions billing on the `technicalpickles` GitHub account (Settings → Billing & plans → raise spending limit / fix payment). Once a green `main` run produces `daemon-bench-{macos-latest,ubuntu-latest}` artifacts, this story can resume from Task 0. No code workaround exists — this is account-level infrastructure. Tracked by bean **gt-9205**.

**Decision (2026-05-29):** parked as blocked per pickles. Story stays `ready-for-dev`; resume from Task 0 when CI is green.

### Completion Notes List

- Story is **blocked**, not failed. No production code, baseline, or doc changes have been made — the all-zero baselines and unstruck retro items remain accurate until the gates are genuinely armed and proven in CI.

### File List

_(none yet — story blocked at Task 0 pending CI billing restoration)_
