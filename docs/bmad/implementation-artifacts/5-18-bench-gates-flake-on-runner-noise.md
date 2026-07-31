# Story 5.18: The bench gates fire on runner noise, so a red build carries no information

Status: ready-for-dev

<!-- Note: Validation is optional. Run validate-create-story for quality check before dev-story. -->

## Story

As the bowerbird maintainer,
I want a red bench gate to mean a real regression landed,
so that accepting perf work through CI is verification rather than ceremony.

**Origin.** Two gates fired spuriously within one session, on diffs that could not have caused them, and both passed on immediate re-run. Filed as taskwarrior `b6e4eceb-d62d-4c5b-a423-d8b0326c5eed` with the structural diagnosis. Sequenced ahead of Story 5.17 by maintainer call (2026-07-30): 5.17's AC #2 is verified by the shim hot-path bench, and that gate produced a 3.8x-budget false positive the day before. Landing perf work through a gate with that failure amplitude proves nothing.

**The secondary cost is cultural.** Two red benches that were both noise teaches reflex-rerunning, which is exactly how a real regression eventually gets waved through.

**Relationship to Story 5.5.** Story 5.5 (`in-progress`) owns making the gates *load-bearing*: arming the daemon regression gate and proving both gates fire via chaos injection. This story owns making them *trustworthy*. They are complementary and 5.5 is not modified. One deliberate conflict: 5.5's Dev Notes freeze the bench harnesses (`hook_to_presenter.rs`, `hot_path.rs` stay byte-identical) as a constraint on 5.5's own diff. This story edits `hook_to_presenter.rs`, which is fine because 5.5's freeze scopes 5.5, but a dev picking up 5.5 afterward should re-read its Task 1 numbers against the new sample counts.

## Evidence (already gathered, do not re-derive)

### Incident 1: shim hot-path, macOS, 2026-07-29 (PR #29)

Diff touched only `release.yml` and a docs markdown file, so it cannot affect the shim binary.

```
results: mean=8.232ms p50=5.757ms p99=57.262ms max=160.631ms (n=200)
```

Failed the 15ms absolute budget by 3.8x. Passed on immediate re-run with identical code; the same shim code passed on PR #28 and the rc2 release run minutes earlier.

### Incident 2: daemon hook-to-presenter, macOS, 2026-07-30 (PR #32)

Diff was docs plus one *shim* comment, so it cannot touch the daemon.

```
solo   p99 11.401ms vs baseline 1.569ms x1.30 = 2.040ms allowed  (+626.6%)
steady p99  6.955ms vs                3.689ms allowed  (+145.1%)
burst  fine at -39.4%
```

All shapes were far inside the 100ms NFR2 absolute budget; it was the *regression* gate that fired. Passed on immediate re-run.

### The two incidents do NOT share a mechanism

The taskwarrior annotation calls them the same root cause. They share a *cause* (hosted macOS runner contention) but not the same *gate defect*, and the fixes differ.

**Daemon: p99 is literally the max, in all four shapes.** `percentile()` in `hook_to_presenter.rs:203` computes `ceil(n * p) - 1`. With the CI defaults:

| shape | n in CI | source of n | p99 index | samples strictly above p99 |
| --- | --- | --- | --- | --- |
| solo | 50 | `DEFAULT_SAMPLES` | 49 | **0** |
| fanout3 | 25 | `samples / 2` (`main()`) | 24 | **0** |
| burst | 20 | `DAEMON_BENCH_BURST_COUNT` | 19 | **0** |
| steady | ~25 | 5s at 5/sec | 24 | **0** |

The filed annotation identified solo at n=50. It is worse than that: `fanout3` silently runs at half the sample count, and `burst` samples are already worst-of-8, so its gate is effectively a max-of-160. Every daemon shape is one scheduler hiccup from red. The failing log makes it visible: `solo: p50=0.289ms p99=11.401ms max=11.401ms`, a single sample at 39x the median defining the gate.

Reseeding from worst-observed-max cannot fix this, and the record proves it: `crates/daemon/benches/baselines/macos.json`'s `_seeding_note` documents a reseed on 2026-07-29 from the per-shape MAX across 7 CI runs, and the gate flaked again the next day. The failure mode is one *new* outlier, not a shifted distribution.

**Shim: the statistic is fine; the budget is stale.** The shim runs n=200, so p99 sits at index 197 with 2 samples above it. Raising n is not available as a fix and would not have helped: for p99 to read 57.262ms at least three samples were at or above 57ms, and p50 was 5.757ms against a committed baseline *mean* of 2.537ms. That whole run was globally starved, roughly a 2.3x median shift, not a spike.

The real problem is calibration drift. ADR 0003 (2026-05-18) set the 15ms macOS budget as "≈1.32x headroom over worst observed", where worst observed was 11.345ms. The **green** run on 2026-07-30 (run `30546733823`) measured:

```
macos: mean=5.881ms p50=5.613ms p99=11.554ms max=12.643ms (n=200)
linux: mean=0.836ms p50=0.833ms p99=0.990ms max=1.022ms (n=200)
```

macOS p99 of 11.554ms is *past* the worst observation the budget was calibrated against, sitting at 77% of ceiling on a passing run. The committed baseline (p99 2.663ms, mean 2.537ms) is 4.3x below current reality, and because `regression_max_ratio` is `null` on macOS nothing has been watching that drift. We cannot tell from CI whether macOS shim spawn got 2.3x slower because of runner images or because of bowerbird. That is the gate Story 5.17's AC #2 leans on.

ADR 0003's own "Revisit when" section lists "the runner topology changes" as a trigger. It has fired.

### Cost basis for sample-count changes

Measured from run `30546733823`'s Linux daemon step timestamps:

| shape | n | wall clock |
| --- | --- | --- |
| solo | 50 | 0.39s |
| fanout3 | 25 | 0.10s |
| burst | 20 | 0.12s |
| steady | 5s | 5.08s |

Job wall clock is dominated by an uncached `cargo build`: the daemon macOS job is 2m03s total and the bench itself is ~5.7s. The shim macOS job is 54s total with a ~1s bench. **There is no cargo cache step in `ci.yml` at all.** That is why a re-measure on the failure path costs seconds rather than another job, and it is filed separately as a followup.

## Acceptance Criteria

1. **Given** a bench gate fails on a run whose diff cannot have caused it **When** the gate re-measures **Then** the job fails only if the gate fails on **both** attempts, and both attempts' full output plus both summary JSONs are available in the job log and artifacts. A tooling failure (exit 2) or a bench process crash must **not** be retried; only a policy failure (exit 1) triggers the re-measure.

2. **Given** a re-measure was needed but the second attempt passed **When** the job goes green **Then** the retry is recorded in `$GITHUB_STEP_SUMMARY` with both attempts' numbers. A silent retry is the reflex-rerunning failure mode wearing a costume; the whole point is that the noise stays countable.

3. **Given** the daemon bench **When** it reports `*_p99_nanos` for any of the four shapes **Then** that value is a real 99th percentile with at least two samples strictly above it, so no single outlier can define the gate. The `samples` field in the summary JSON must not claim a count that three of four shapes do not use.

4. **Given** the retry mechanism is the load-bearing piece **When** this story lands **Then** it has tests that exercise the branch matrix directly (pass first try, fail-then-pass, fail twice, exit 2 no retry, bench crash no retry), not merely a green CI run. Story 5.16's pass-2 review is the precedent: two prior tests went green while the bug they claimed to guard was live.

5. **Given** the shim macOS budget was calibrated in ADR 0003 against a worst-observed 11.345ms that a green run has now exceeded **When** new numbers are committed **Then** they are sourced from fresh multi-run CI data on the story branch, ADR 0003 carries a dated update section recording the new evidence and policy, and the macOS regression gate is either restored with a documented ratio or left disabled with the measured reason why. Do not bump a budget to make a number fit; that is 5.5's anti-pattern list and PRD line 181.

6. **Given** `docs/bmad/project-context.md:629` and Story 5.16 AC #5 both state the shim gates at +15% while `macos.json` has `regression_max_ratio: null` and `linux.json` has `1.35` **When** this story lands **Then** the documented thresholds match the committed config, and `deferred-work.md` item 6 (code review of 5.16) is struck through.

7. **Given** the Linux daemon baseline was deliberately left at placeholder zero **When** this story lands **Then** `crates/daemon/benches/baselines/linux.json` is **unchanged**. The 2026-07-28 maintainer call and the unexplained ~40x macOS/Linux gap behind it are recorded in `deferred-work.md` § "Deferred from: Story 5.5" and remain Story 5.5's to resolve.

## Tasks / Subtasks

- [ ] **Task 1: Build `scripts/run-bench-gate.py` (AC: 1, 2)**
  - [ ] CLI: `--bench <command>` `--gate <command>` `--summary <path>`. Commands are split with `shlex.split` and run without a shell (no shell metacharacters are needed by either call site).
  - [ ] Attempt 1: run bench. Non-zero exit propagates immediately and the gate never runs. A bench crash is breakage, not noise.
  - [ ] Run gate. Exit 0 ends the run at 0. Exit 2 propagates immediately with no retry. Any other non-zero is treated as a policy failure.
  - [ ] On policy failure: copy `--summary` to a sibling `*.attempt1.json`, emit a `::warning::` naming the retry, then re-run bench and gate once.
  - [ ] Attempt 2 exit 0: exit 0, and append a re-measure note to `$GITHUB_STEP_SUMMARY` carrying both attempts' summary contents. Attempt 2 exit 1: emit `::error::` stating the gate failed on both attempts, exit 1. Attempt 2 exit 2: exit 2.
  - [ ] Fixed at two attempts. Do not add a configurable retry count; a knob here is a knob for making red builds go away.
  - [ ] Mirror the two existing gate scripts' idiom: module docstring stating contract and exit codes, `gh_error`/`gh_notice` helpers, `argparse`. Do not diverge the three scripts' shape.

- [ ] **Task 2: Test the wrapper (AC: 4)**
  - [ ] `scripts/tests/test_run_bench_gate.py`, driving the wrapper with stub bench/gate commands whose exit sequence is scripted through a counter file.
  - [ ] Cases: (a) gate passes first try, bench ran once; (b) gate fails then passes, bench ran twice, attempt1 JSON preserved, step-summary note written; (c) gate fails twice, exit 1, bench ran twice; (d) gate exits 2, exit 2, bench ran **once**; (e) bench exits non-zero, that code propagates and the gate never ran; (f) gate exits 2 on attempt 2, exit 2.
  - [ ] Assert on **both** the exit code and the number of bench invocations. Case (d) is the one that silently degrades into "retry everything" if the exit-code branch is wrong, and only the invocation count catches it.
  - [ ] Wire into the existing `ci` job. There is no Python test runner in CI today; `python3 -m unittest` over `scripts/tests/` keeps the dependency at zero.

- [ ] **Task 3: Wire both gates through the wrapper (AC: 1, 2)**
  - [ ] `.github/workflows/ci.yml`: replace the separate bench and gate steps in `shim-bench-gate` and `daemon-bench-gate` with one `run-bench-gate.py` invocation each.
  - [ ] Widen both artifact upload paths to `target/*bench-summary*.json` so the attempt-1 file ships alongside the final one.
  - [ ] Rename both job names to stop stating numbers that do not match the committed config (they currently read "p99 ≤ 5ms, +15% regression fails" and "p99 ≤ 100ms, +30% regression fails"). Per-platform policy lives in the baseline files; the job name should say so rather than restate a number that drifts.

- [ ] **Task 4: Make the daemon p99 a real percentile (AC: 3)**
  - [ ] `DEFAULT_SAMPLES` 50 → 200, matching the shim harness.
  - [ ] `bench_fanout3` call site: pass `samples`, not `samples / 2`. Document the change; the halving was a CI-budget concession that silently made fanout3 the weakest shape.
  - [ ] `DAEMON_BENCH_BURST_COUNT` default 20 → 200.
  - [ ] `bench_steady`: 5s → 40s at the existing 5/sec pacing, yielding ~200 samples. **Do not reach 200 by raising the rate.** The shape exists to catch slow leaks and accumulating contention, which duration buys and rate does not; 5/sec is already a reduction from the original AC's 1/sec-for-30s intent. Maintainer decision (2026-07-30), weighed against a 20s-at-10/sec alternative.
  - [ ] Expected new bench wall clock ≈ 43.5s (was ≈5.7s), putting the daemon job at roughly 2m41 from 2m03. Confirm against the real run rather than trusting this estimate.
  - [ ] The summary JSON's `samples` field becomes accurate for solo, fanout3, and burst; steady stays time-derived. Note that in the harness docstring rather than changing the schema.
  - [ ] Verify the arithmetic after the change: `ceil(n * 0.99) - 1` must be ≤ n - 3 for every shape.

- [ ] **Task 5: Calibration runs (AC: 5)** [HUMAN-IN-THE-LOOP: needs real CI runs]
  - [ ] With Tasks 1-4 landed on the story branch, trigger CI ~5 times with **no code change between runs**. This is the same protocol ADR 0003 used with 3 runs; a single run is informative about the day's runner state, not about the distribution.
  - [ ] Download every `shim-bench-{macos,ubuntu}-latest` and `daemon-bench-{macos,ubuntu}-latest` artifact. Record all of them in the Dev Agent Record, including the ones that look boring; the spread is the finding, not the best number.
  - [ ] Compute the best-of-2 spread, not the raw spread, since that is what the gate now sees. Replaying ADR 0003's own three macOS numbers (2.66 / 6.19 / 11.35ms) as best-of-2 pairs collapses 4.3x to 2.3x; confirm that against real data rather than assuming it.

- [ ] **Task 6: Reseed baselines and amend ADR 0003 (AC: 5, 7)**
  - [ ] `crates/shim/benches/baselines/macos.json`: reseed `p99_nanos` / `mean_nanos`; restore `regression_max_ratio` at whatever the measured best-of-2 spread supports (~2.0 is the target, not a commitment); set `absolute_budget_nanos` with real headroom over worst observed. If the data says a ratio still cannot work, leave it `null` and record the measured reason, per AC #5's escape hatch.
  - [ ] `crates/daemon/benches/baselines/macos.json`: reseed all four shapes. The current values were seeded from n=50 maxes and are stale by construction once n=200.
  - [ ] `crates/shim/benches/baselines/linux.json`: verify against the fresh data. It looks healthy today (green-run p99 0.990ms against a 1.194 x 1.35 = 1.613ms threshold); reseed only if the data says to.
  - [ ] `crates/daemon/benches/baselines/linux.json`: **do not touch.** See AC #7.
  - [ ] Commit the baselines as their own commit, separate from the mechanism commits, so the calibration is reviewable on its own.
  - [ ] `docs/decisions/0003-shim-p99-budget-on-macos-latest.md`: add a dated update section following the existing 2026-05-20 Linux-update pattern. Do **not** supersede the ADR. Record the 2026-07-30 evidence, that the 15ms calibration point was exceeded on a green run, that best-of-2 narrows the effective single-run spread, and the new per-platform policy table.
  - [ ] State plainly in the ADR update that this raises a budget, which 5.5's anti-pattern list forbids "to make a number fit", and why this is the sanctioned path instead: the number is reset from measured evidence via the ADR amendment PRD line 181 requires, and the regression gate is restored in the same change, so macOS ends with more signal than it has today rather than less.

- [ ] **Task 7: Reconcile the documented thresholds (AC: 6)**
  - [ ] `docs/bmad/project-context.md:629` and Story 5.16 AC #5 both claim +15%. Correct both to point at the per-platform baseline files and ADR 0003 rather than restating a number.
  - [ ] Both gate scripts' module docstrings hardcode the pre-ADR-0003 defaults in prose (`check-shim-bench-p99.py` says "Absolute: ≤ 5,000,000 ns" and "Regression: ≤ committed_p99 * 1.15"). The code already reads per-platform policy from the baseline; make the prose say that.
  - [ ] Strike through `deferred-work.md` item 6 under "Deferred from: code review of 5-16-hotfix-shim-timeout-drops-events" with a backlink to this story.

- [ ] **Task 8: Verification + File List (AC: all)**
  - [ ] `cargo fmt --check` and `cargo clippy --all-targets --workspace -- -D warnings` clean.
  - [ ] `scripts/test.sh` green. Never raw `cargo test`; see the project CLAUDE.md.
  - [ ] `python3 -m unittest discover scripts/tests` green.
  - [ ] Run the daemon bench locally at the new sample counts and confirm no shape reports a p99 equal to its max.
  - [ ] `git status --porcelain` reconciled against the File List before declaring review (epic-4-retro Discovery #6 / AI-6: File-List-vs-git drift has bitten four prior stories).

## Dev Notes

### What this story is not

- **Not seeding the Linux daemon baseline.** All four `*_p99_nanos` are 0 and the regression gate auto-skips on Linux. That is a recorded maintainer decision from 2026-07-28, not an oversight: the observed Linux numbers are ~40ms against macOS's ~1ms for the same shapes, reproducible across two independent runs, and committing them would lock in "40ms is normal" for every future Linux run. See `deferred-work.md` § "Deferred from: Story 5.5". Story 5.5 owns it.
- **Not investigating the ~40ms Linux gap.** Already filed with the discriminator identified: `steady` is fine on both platforms because it is paced, and only the shapes that fire back-to-back with no inter-event pacing show it. Leading hypothesis on file is SQLite WAL fsync / disk I/O on `ubuntu-latest`, unconfirmed. Do not relitigate from a fresh guess.
- **Not doing Story 5.5's chaos injection.** Tasks 2-3 of 5.5 remain unstarted and remain 5.5's. Note for whoever picks them up: best-of-2 does not change the required injection magnitudes, because an injected sleep is deterministic and therefore fails both attempts.
- **Not adding a cargo cache.** The bench jobs rebuild from scratch every run, which dominates their wall clock and would more than pay back Task 4's added bench time. Filed as a separate taskwarrior item rather than folded in here.

### Why best-of-2 is not just automated reflex-rerunning

The objection is real and was raised before the design was accepted. Three things separate them:

1. It is bounded at two attempts and cannot be escalated by a frustrated human.
2. It is recorded. AC #2 requires the retry to land in the step summary with both attempts' numbers, so the rate of noise stays countable instead of living in people's memory of how many times they hit re-run.
3. It cannot mask a deterministic regression, which is what a real one is. A code change that makes the shim slower fails attempt 2 for the same reason it failed attempt 1.

What it genuinely gives up: a *probabilistic* regression, one that only manifests on some runs, is now half as likely to be caught per CI run. Nothing in the current bench suite targets that class, and the alternative (single-shot gating on a max) has been demonstrated twice in one session to produce false positives at 3.8x and 5.6x over the respective thresholds. That trade was accepted deliberately.

### Why raising n does nothing for the shim

Stated explicitly because it is the obvious first instinct and it is wrong here. The shim already runs n=200; its p99 is at index 197 with two samples above. The macOS failure was a 2.3x shift of the entire distribution, not a tail spike. Sample count cannot fix a runner that is uniformly slow for a run. That is what the re-measure is for, and what the budget recalibration is for.

### The percentile arithmetic, in one place

Both harnesses compute the index as `ceil(n * p) - 1`, clamped to `[0, n-1]`. Computed, not reasoned about:

| n | samples strictly above p99 |
| --- | --- |
| 50 | 0 |
| 99 | 0 |
| **100** | **1** (first n that is not a max-gate) |
| 150 | 1 |
| 199 | 1 |
| **200** | **2** |

So n=100 is the smallest n where p99 stops being the max, and n=200 is the smallest where two samples sit above it. Task 4 targets 200 for every shape, matching the shim harness. Re-run this check after changing the counts rather than trusting the table.

### Testing standards

- Bench harnesses are `harness = false` per-invocation timers, not Criterion. Story 1.5 review finding 2: Criterion's flat sampling batches iterations and hides high-tail regressions. Do not "improve" them to Criterion.
- The three `scripts/` gate-adjacent Python files mirror each other by design. Same CLI shape, same helpers, same exit-code contract. Do not diverge them.
- Per-platform baselines are committed files updated deliberately by PR with reviewer sign-off (`project-context.md:631`). Auto-rolling baselines silently absorb regressions. The re-measure in Task 1 changes which *run* the committed number comes from on a retry, and nothing else; it must never write a baseline.

### Project Structure Notes

- Gate scripts: `scripts/check-shim-bench-p99.py`, `scripts/check-daemon-bench-p99.py`, plus the new `scripts/run-bench-gate.py`.
- Harnesses: `crates/shim/benches/hot_path.rs`, `crates/daemon/benches/hook_to_presenter.rs`.
- Baselines: `crates/{shim,daemon}/benches/baselines/{macos,linux}.json`.
- CI jobs: `shim-bench-gate` (`ci.yml:72`), `daemon-bench-gate` (`ci.yml:107`), both `fail-fast: false` matrices over `macos-latest` + `ubuntu-latest`.
- No protocol change, no new crate. The ADR trigger is met by the ADR 0003 amendment, which is Task 6 rather than a new ADR.

### References

- [Source: taskwarrior `b6e4eceb-d62d-4c5b-a423-d8b0326c5eed`]: both incidents with full numbers and the structural diagnosis.
- [Source: docs/decisions/0003-shim-p99-budget-on-macos-latest.md]: the 15ms macOS budget, its 11.345ms calibration point, `regression_max_ratio: null`, the 2026-05-20 Linux update pattern to follow, and the "Revisit when" triggers.
- [Source: docs/bmad/implementation-artifacts/5-5-bench-gates-load-bearing.md]: Task 1's partial completion, the Linux punt and its reasoning, the harness freeze, and the "do not raise a budget to make a number fit" anti-pattern.
- [Source: docs/bmad/implementation-artifacts/deferred-work.md]: § "Deferred from: Story 5.5" (the ~40x gap); § "code review of 5-16..." item 6 (the +15% doc drift this story closes).
- [Source: crates/daemon/benches/hook_to_presenter.rs:49,203,371-374]: `DEFAULT_SAMPLES`, `percentile()`, and the `samples / 2` fanout3 call site.
- [Source: crates/shim/benches/hot_path.rs:113,150-155]: `SHIM_BENCH_SAMPLES` and the same percentile arithmetic.
- [Source: .github/workflows/ci.yml:72-147]: both bench-gate jobs, their step structure, and the artifact upload paths.
- [Source: GitHub Actions run 30546733823]: the 2026-07-30 green run on `main` carrying the macOS shim p99 of 11.554ms and the Linux daemon ~40.9ms figures.
- [Source: docs/bmad/implementation-artifacts/5-17-shim-write-budget-is-not-a-bound.md]: AC #2, the shim-bench-verified acceptance this story unblocks.

## Dev Agent Record

### Agent Model Used

### Debug Log References

### Completion Notes List

### File List

## Change Log

| Date | Author | Summary |
|------|--------|---------|
| 2026-07-30 | claude-opus-5 | Story created from taskwarrior `b6e4eceb` after a scoping pass that found the filed diagnosis covered one of the two incidents. Corrections folded in: all four daemon shapes are max-gates (not just solo at n=50); the shim incident is a distribution shift that no sample-count change addresses, and its real cause is that ADR 0003's 15ms calibration point was exceeded on a green run. Two candidate scope items were dropped after finding them already decided: seeding the Linux daemon baseline (2026-07-28 maintainer punt) and the ~40ms Linux gap (already filed with better evidence). Design decisions this session: best-of-2 re-measure with both attempts logged; Python wrapper rather than shell, since the repo has no shell test harness and the two gate scripts are already Python; steady grows to 40s at 5/sec rather than 20s at 10/sec, keeping duration as the thing the shape buys; macOS regression gate restored rather than left absolute-only. |
