# Story 5.18: The bench gates fire on runner noise, so a red build carries no information

Status: done

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

- [x] **Task 1: Build `scripts/run-bench-gate.py` (AC: 1, 2)**
  - [x] CLI: `--bench <command>` `--gate <command>` `--summary <path>`. Commands are split with `shlex.split` and run without a shell (no shell metacharacters are needed by either call site).
  - [x] Attempt 1: run bench. Non-zero exit propagates immediately and the gate never runs. A bench crash is breakage, not noise.
  - [x] Run gate. Exit 0 ends the run at 0. Exit 2 propagates immediately with no retry. Any other non-zero is treated as a policy failure.
  - [x] On policy failure: copy `--summary` to a sibling `*.attempt1.json`, emit a `::warning::` naming the retry, then re-run bench and gate once.
  - [x] Attempt 2 exit 0: exit 0, and append a re-measure note to `$GITHUB_STEP_SUMMARY` carrying both attempts' summary contents. Attempt 2 exit 1: emit `::error::` stating the gate failed on both attempts, exit 1. Attempt 2 exit 2: exit 2.
  - [x] Fixed at two attempts. Do not add a configurable retry count; a knob here is a knob for making red builds go away.
  - [x] Mirror the two existing gate scripts' idiom: module docstring stating contract and exit codes, `gh_error`/`gh_notice` helpers, `argparse`. Do not diverge the three scripts' shape.

- [x] **Task 2: Test the wrapper (AC: 4)**
  - [x] `scripts/tests/test_run_bench_gate.py`, driving the wrapper with stub bench/gate commands whose exit sequence is scripted through a counter file.
  - [x] Cases: (a) gate passes first try, bench ran once; (b) gate fails then passes, bench ran twice, attempt1 JSON preserved, step-summary note written; (c) gate fails twice, exit 1, bench ran twice; (d) gate exits 2, exit 2, bench ran **once**; (e) bench exits non-zero, that code propagates and the gate never ran; (f) gate exits 2 on attempt 2, exit 2. (Plus a seventh case beyond the letter of the story: bench crash on attempt 2 propagates its code, gate not re-run.)
  - [x] Assert on **both** the exit code and the number of bench invocations. Case (d) is the one that silently degrades into "retry everything" if the exit-code branch is wrong, and only the invocation count catches it.
  - [x] Wire into the existing `ci` job. There is no Python test runner in CI today; `python3 -m unittest` over `scripts/tests/` keeps the dependency at zero.

- [x] **Task 3: Wire both gates through the wrapper (AC: 1, 2)**
  - [x] `.github/workflows/ci.yml`: replace the separate bench and gate steps in `shim-bench-gate` and `daemon-bench-gate` with one `run-bench-gate.py` invocation each.
  - [x] Widen both artifact upload paths to `target/*bench-summary*.json` so the attempt-1 file ships alongside the final one.
  - [x] Rename both job names to stop stating numbers that do not match the committed config (they currently read "p99 ≤ 5ms, +15% regression fails" and "p99 ≤ 100ms, +30% regression fails"). Per-platform policy lives in the baseline files; the job name should say so rather than restate a number that drifts. **Operational side effect found while doing this:** branch protection's required status checks name the old shim job string; they must be updated to the new contexts when this story merges (see Completion Notes).

- [x] **Task 4: Make the daemon p99 a real percentile (AC: 3)**
  - [x] `DEFAULT_SAMPLES` 50 → 200, matching the shim harness.
  - [x] `bench_fanout3` call site: pass `samples`, not `samples / 2`. Document the change; the halving was a CI-budget concession that silently made fanout3 the weakest shape.
  - [x] `DAEMON_BENCH_BURST_COUNT` default 20 → 200.
  - [x] `bench_steady`: 5s → 40s at the existing 5/sec pacing, yielding ~200 samples. **Do not reach 200 by raising the rate.** The shape exists to catch slow leaks and accumulating contention, which duration buys and rate does not; 5/sec is already a reduction from the original AC's 1/sec-for-30s intent. Maintainer decision (2026-07-30), weighed against a 20s-at-10/sec alternative. (Implemented count-paced: exactly `steady_secs x 5` sends at the same 200ms period, a wall-clock cutoff measured n=198 locally because connect/settle ate into the window, which breaks AC #3's two-above-p99 requirement. Rate and duration unchanged; only the loop bound moved from clock to count.)
  - [x] Expected new bench wall clock ≈ 43.5s (was ≈5.7s), putting the daemon job at roughly 2m41 from 2m03. Confirm against the real run rather than trusting this estimate. (Confirmed: local bench wall clock ≈44s; across the 5 calibration runs the daemon-bench-gate jobs ran 2m56–3m34 total, slightly over the 2m41 estimate, dominated as before by the uncached cargo build.)
  - [x] The summary JSON's `samples` field becomes accurate for solo, fanout3, and burst; steady stays duration×rate-derived. Noted in the harness docstring; schema unchanged.
  - [x] Verify the arithmetic after the change: `ceil(n * 0.99) - 1` must be ≤ n - 3 for every shape. (Computed: n=200 → index 197, 2 samples strictly above, for all four shapes; local run confirmed n=200 on every shape and solo p99 0.242ms vs max 0.602ms.)

- [x] **Task 5: Calibration runs (AC: 5)** [HUMAN-IN-THE-LOOP: needs real CI runs]
  - [x] With Tasks 1-4 landed on the story branch, trigger CI ~5 times with **no code change between runs**. This is the same protocol ADR 0003 used with 3 runs; a single run is informative about the day's runner state, not about the distribution. (Runs 30599853185, 30600064861, 30600097130, 30600129141, 30600163266 on PR #33, run 1 on the mechanism push, runs 2-5 via empty commits so each gets a fresh VM and artifact set. All five green.)
  - [x] Download every `shim-bench-{macos,ubuntu}-latest` and `daemon-bench-{macos,ubuntu}-latest` artifact. Record all of them in the Dev Agent Record, including the ones that look boring; the spread is the finding, not the best number. (All 20 artifacts recorded below, plus run 5's shim-macOS attempt-1 file, the wrapper fired in the wild during its own calibration.)
  - [x] Compute the best-of-2 spread, not the raw spread, since that is what the gate now sees. Replaying ADR 0003's own three macOS numbers (2.66 / 6.19 / 11.35ms) as best-of-2 pairs collapses 4.3x to 2.3x; confirm that against real data rather than assuming it. (Confirmed: shim macOS single-run spread 2.10x across finals, 4.10x counting the 15.130ms attempt-1; best-of-2 across unordered pairs 1.92x.)

- [x] **Task 6: Reseed baselines and amend ADR 0003 (AC: 5, 7)**
  - [x] `crates/shim/benches/baselines/macos.json`: reseed `p99_nanos` / `mean_nanos`; restore `regression_max_ratio` at whatever the measured best-of-2 spread supports (~2.0 is the target, not a commitment); set `absolute_budget_nanos` with real headroom over worst observed. If the data says a ratio still cannot work, leave it `null` and record the measured reason, per AC #5's escape hatch. (Reseeded p99 7.725ms / mean 4.770ms from run 5's final attempt, the worst best-of-2 observation; ratio restored at 2.0 (ceiling 15.45ms, observed best-of-2 band tops out 7.7ms); absolute 15ms → 20ms since the worst observed single attempt is now 15.130ms on a green run, keeping ADR 0003's ~1.32x headroom formula.)
  - [x] `crates/daemon/benches/baselines/macos.json`: reseed all four shapes. The current values were seeded from n=50 maxes and are stale by construction once n=200. (Per-shape worst across the 5 runs: solo 0.819 / fanout3 0.847 / burst 3.793 / steady 1.821ms; ratio stays 1.30; `samples` 50 → 200.)
  - [x] `crates/shim/benches/baselines/linux.json`: verify against the fresh data. It looks healthy today (green-run p99 0.990ms against a 1.194 x 1.35 = 1.613ms threshold); reseed only if the data says to. (Verified: 1.155–1.208ms across five runs, 1.05x spread, all inside the 1.613ms ceiling. Not reseeded.)
  - [x] `crates/daemon/benches/baselines/linux.json`: **do not touch.** See AC #7. (Untouched; `git diff` confirms. Fresh observation recorded in Completion Notes for Story 5.5: the ~40ms gap did not reproduce at n=200.)
  - [x] Commit the baselines as their own commit, separate from the mechanism commits, so the calibration is reviewable on its own.
  - [x] `docs/decisions/0003-shim-p99-budget-on-macos-latest.md`: add a dated update section following the existing 2026-05-20 Linux-update pattern. Do **not** supersede the ADR. Record the 2026-07-30 evidence, that the 15ms calibration point was exceeded on a green run, that best-of-2 narrows the effective single-run spread, and the new per-platform policy table.
  - [x] State plainly in the ADR update that this raises a budget, which 5.5's anti-pattern list forbids "to make a number fit", and why this is the sanctioned path instead: the number is reset from measured evidence via the ADR amendment PRD line 181 requires, and the regression gate is restored in the same change, so macOS ends with more signal than it has today rather than less.

- [x] **Task 7: Reconcile the documented thresholds (AC: 6)**
  - [x] `docs/bmad/project-context.md:629` and Story 5.16 AC #5 both claim +15%. Correct both to point at the per-platform baseline files and ADR 0003 rather than restating a number. (Also corrected in the same sweep because they restate the same drifted numbers: 5.16's References entry for project-context, `crates/shim/benches/README.md`'s policy table + threshold-rationale section, which still said Linux gates at +15% when it has been 1.35 since ADR 0003's 2026-05-20 update, and `architecture.md:481`'s "30% vs the shim's 15%" parenthetical. Deliberately left alone: `product-brief-bowerbird-distillate.md`, a frozen distillate of a planning doc, and historical prose in done stories' Dev Agent Records.)
  - [x] Both gate scripts' module docstrings hardcode the pre-ADR-0003 defaults in prose (`check-shim-bench-p99.py` says "Absolute: ≤ 5,000,000 ns" and "Regression: ≤ committed_p99 * 1.15"). The code already reads per-platform policy from the baseline; make the prose say that.
  - [x] Strike through `deferred-work.md` item 6 under "Deferred from: code review of 5-16-hotfix-shim-timeout-drops-events" with a backlink to this story.

- [x] **Task 8: Verification + File List (AC: all)**
  - [x] `cargo fmt --check` and `cargo clippy --all-targets --workspace -- -D warnings` clean.
  - [x] `scripts/test.sh` green (638 passed / 0 failed, log `target/test-logs/20260730-224233-80525`). Never raw `cargo test`; see the project CLAUDE.md.
  - [x] `python3 -m unittest discover scripts/tests` green (7 tests).
  - [x] Run the daemon bench locally at the new sample counts and confirm no shape reports a p99 equal to its max. (solo n=200 p99 0.242ms max 0.602ms; fanout3 n=200; burst n=200; steady n=200 p99 0.907ms.)
  - [x] `git status --porcelain` reconciled against the File List before declaring review (epic-4-retro Discovery #6 / AI-6: File-List-vs-git drift has bitten four prior stories). (Clean tree; `git diff main...HEAD --name-only` is exactly the 16 files in the File List. Final post-calibration regression re-run: 638 passed / 0 failed, log `target/test-logs/20260731-074152-90222`. CI run 6 `30627715867` green against the reseeded baselines, the armed macOS shim regression gate passing is the reseed's live verification.)

### Review Findings (2026-07-31, three-layer subagent review: Blind Hunter / Edge Case Hunter / Acceptance Auditor)

- [x] [Review][Decision] The daemon macOS regression ratio (1.30) was inherited, not calibrated, this diff reseeds `crates/daemon/benches/baselines/macos.json` from the new n=200 data but keeps `regression_max_ratio: 1.30` untouched, while the measured daemon macOS best-of-2 spreads (1.62-2.32x across shapes) are wider than the shim's 1.92x, for which this same diff judged narrow ratios unusable and installed 2.0. Mitigations differ (worst-observed seeding + real percentiles + best-of-2, and no daemon distribution-shift incident is on record), so 1.30-over-worst may hold, but nothing in the record derives it. Options: (a) keep 1.30 and document why in the seeding note, (b) widen to ~1.75-2.0 to cover a shim-style global-shift day, (c) leave until a flake provides data.
- [x] [Review][Patch] Wrapper retries every gate exit except 0/2, violating AC #1's "only a policy failure (exit 1) triggers the re-measure" [scripts/run-bench-gate.py:117-174], a gate killed by a signal (negative returncode) or exiting 3/127 is classified as policy noise, re-measured, and a double-crash exits 1 ("policy failure held"). Task 1's sub-bullet "Any other non-zero is treated as a policy failure" contradicts AC #1; the AC governs. Fix: retry only on gate exit 1; any other unrecognized gate exit gets `::error::` + exit 2 (breakage, not noise), both attempts. Add tests for gate exit 3 and a signal-killed gate. (Found independently by all three layers.)
- [x] [Review][Patch] Both gate scripts exit 1 on a missing committed baseline, so seeding a new platform burns a full spurious re-measure and ends with "treat it as a real policy failure" [scripts/check-shim-bench-p99.py:105, scripts/check-daemon-bench-p99.py:118], a missing baseline is an unarmed-gate config state, not a judged policy failure. Fix: return 2 there, update both docstrings' exit-code contracts (the job still fails during seeding, as the README flow requires, just without the retry and the misleading verdict).
- [x] [Review][Patch] Wrapper robustness holes around its own tooling failures [scripts/run-bench-gate.py], (a) a typo'd command raises uncaught FileNotFoundError → traceback + exit 1, colliding with the "failed both attempts" verdict; (b) an empty `--bench`/`--gate` string yields an empty argv and raises; (c) `append_step_summary` is unguarded, so an unwritable `GITHUB_STEP_SUMMARY` flips a PASSING attempt-2 run to exit 1; (d) nothing verifies the bench actually wrote `--summary`; (e) a stale `*.attempt1.json` in a cached `target/` would ship in the widened artifact glob on a clean run. Fix: catch OSError around spawn → `::error::` + exit 2; validate commands non-empty; try/except the step-summary append (warn, don't fail); after each bench, exit 2 if the summary file is missing; unlink any pre-existing attempt1 file at startup. Add tests where cheap.
- [x] [Review][Patch] Burst's n does not track the summary's `samples` field under env override, and the new docstring claims it does [crates/daemon/benches/hook_to_presenter.rs], `burst_count` defaults to the const, not the runtime `samples`; `DAEMON_BENCH_SAMPLES=50` writes `samples: 50` while burst runs 200 (AC #3's "must not claim a count" clause, env-override case). Fix: default `burst_count` to the runtime `samples`, correct the docstring (also: soften "two samples strictly above" for the tie case, and note count-paced steady's duration bound degrades to ~2s/event under a sick daemon), and assert non-zero env overrides with a clear message instead of an index panic.
- [x] [Review][Patch] Calibration record understates its evidence limits [docs/decisions/0003-shim-p99-budget-on-macos-latest.md, story Dev Agent Record], the 5 "no-change runs" span one ~7-minute window (02:46-02:53 UTC, largely concurrent), sampling one runner-weather moment while the two incidents happened on different days; undisclosed. The ADR also never addresses why the 1.32x headroom formula survives unchanged on top of best-of-2 (two slackenings stacked), nor that the 15.45ms regression ceiling sits 2% above the observed 15.130ms single attempt (single-attempt breaches will burn re-measures at some rate). Fix: add the disclosure and a stacking-rationale paragraph (the budget's post-wrapper job is bounding the retry rate, not sole-line-of-defense correctness).
- [x] [Review][Patch] Emdashes throughout the added prose violate the standing no-emdash rule this repo's own sprint history records as enforced one story earlier [ci.yml comments, hook_to_presenter.rs comments, ADR update, README, baselines' seeding notes, story prose], sweep every added line.
- [x] [Review][Patch] README overclaims where retries are recorded [crates/shim/benches/README.md], "a policy failure earns exactly one re-measure, recorded in the step summary" is true only for the fail-then-pass path; double-fails leave only log annotations. Reword (minimal), or extend the wrapper to record double-fails in the step summary too.
- [x] [Review][Patch] Newly commented-out sprint-status history line drops its opening quote while keeping the trailing one, breaking the sibling entries' pattern [docs/bmad/implementation-artifacts/sprint-status.yaml:129-area].

**Resolution (2026-07-31, same session):** the Decision item resolved as option (a) by maintainer choice: ratio stays 1.30 with the reasoning now recorded in the daemon `macos.json` `_seeding_note` (explicit decision, not inheritance; widening-from-measured-spread is the pre-agreed next step if it flakes on an impossible diff). **Superseded within hours by that exact pre-agreed trigger firing:** CI run `30632168195` on the review-fixes push (a daemon-neutral diff: harness comments, asserts, and a defaults-identical `burst_count` change) failed BOTH best-of-2 attempts, attempt 1 on burst (5.730ms, +51% over worst x 1.30) and attempt 2 on fanout3 (1.356ms, +60%). This simultaneously validated the review's clustering finding (the 7-minute calibration window understated cross-window spread; two hours later, values 60% higher) and demonstrated best-of-2 behaving correctly (retry taken, recorded, and a genuinely-out-of-policy day still failed). Per the pre-agreed step, the ratio is now 2.0 over the unchanged worst-of-calibration seeds, derived from the measured cross-window spread (fanout3 3.9x, burst 3.1x); the seeding note carries the full history and an explicit do-not-chase-seeds-upward warning. All 8 patches applied: (1) wrapper retries ONLY on gate exit 1; any other unrecognized gate exit (incl. signal deaths) is `::error::` + exit 2, with the story's Task 1 sub-bullet text now superseded by AC #1's stricter contract; (2) both gate scripts return 2 (not 1) on a missing baseline, docstrings updated; (3) spawn failures exit 2 via a SpawnFailure handler, empty commands rejected, step-summary append guarded (warn, never flip a green run), bench-without-summary exits 2, stale attempt-1 unlinked at startup; (4) `burst_count` defaults to the runtime `samples`, docstring corrected (burst claim, rank-vs-value phrasing, degraded-daemon duration caveat), zero env overrides fail with named asserts; (5) ADR update discloses the 7-minute single-window clustering and adds the stacking rationale (the budget's post-wrapper job is bounding the retry rate; the 15.45ms-vs-15.130ms proximity is designed re-measure behavior with the once-a-week revisit trigger as tripwire); (6) emdash sweep: 45 added lines cleaned, zero remain on any branch-added line; (7) README retry-recording claim narrowed to the fail-then-pass path; (8) sprint-status comment quote restored. Test suite grew 7 → 13 cases (gate exit 3, gate SIGKILL, unknown-exit on attempt 2, unspawnable bench, empty gate command, bench-without-summary, stale-attempt1 cleanup folded into case a). Verification after fixes: 13/13 unittest, fmt + clippy clean, local bench all shapes n=200 (one transient local `NotConnected` ingest panic in burst on a single run, clean on re-run and never seen in CI; if it recurs there, the wrapper's bench-crash path correctly refuses to retry it), `scripts/test.sh` 638 passed / 0 failed (log `target/test-logs/20260731-084933-15675`).

Dismissed as noise (3): attempt-1-copy failure only warning rather than failing the run (deliberate graceful degradation; failing a green run over bookkeeping would invert priorities, the warning is the record); bench exit-code namespace collision for N∈{1,2} (inherent to raw propagation, disambiguated by the wrapper's `::error::` annotations naming which process failed); "each ratio is calibrated" README claim read as covering the daemon baseline (that sentence describes the shim gate's ratios, which are calibrated, the real daemon-ratio question is the Decision item above).

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

claude-fable-5 (dev-story session 2026-07-30)

### Debug Log References

- `target/test-logs/20260730-224233-80525/run.log`, full-workspace `scripts/test.sh` run, 638 passed / 0 failed.
- Local daemon bench at new counts (first run, clock-bounded steady): `solo n=200 p99=0.191ms max=0.394ms; fanout3 n=200; burst n=200; steady n=198`, the n=198 is what motivated count-pacing steady.
- Local daemon bench after count-pacing: `solo n=200 p50=0.123 p99=0.242 max=0.602ms; fanout3 n=200 p99=0.177ms; burst n=200 p99=3.369ms; steady n=200 p99=0.907ms`.

### Calibration record (Task 5, AC #5): 5 no-change CI runs, 2026-07-30/31, PR #33

Runs: 1 = `30599853185` (mechanism push, commit 22c328f), 2-5 = `30600064861` / `30600097130` / `30600129141` / `30600163266` (empty commits da9c483 / 545751c / 2adfed3 / 3c85076, empty commits rather than re-runs because `upload-artifact@v4` can conflict on re-run attempts). All five green. Every artifact recorded, boring ones included:

**shim macOS** (ms, n=200 each):

| run | p99 | mean |
|---|---|---|
| 1 | 7.032 | 4.326 |
| 2 | 3.686 | 2.494 |
| 3 | 5.676 | 3.303 |
| 4 | 7.090 | 3.186 |
| 5 attempt 1 | **15.130** | 4.546 |
| 5 attempt 2 | 7.725 | 4.770 |

Run 5 is the wrapper firing in the wild during its own calibration: attempt 1 breached the then-current 15ms absolute budget on a no-change run, the re-measure passed at 7.725ms, both attempts shipped in artifacts + step summary. Spreads: single-run 2.10x across finals (4.10x with the attempt-1); best-of-2 across unordered pairs 1.92x.

**shim Linux** (ms): p99 1.178 / 1.183 / 1.155 / 1.208 / 1.155, mean 0.997 / 1.009 / 1.005 / 1.016 / 0.980. Spread 1.05x. Committed policy (1.194533 × 1.35 = 1.613ms ceiling) holds with margin; not reseeded.

**daemon macOS** (p99 ms per shape, n=200):

| run | solo | fanout3 | burst | steady |
|---|---|---|---|---|
| 1 | 0.741 | 0.507 | 3.136 | 1.821 |
| 2 | 0.819 | 0.663 | 2.803 | 1.363 |
| 3 | 0.662 | 0.847 | 3.793 | 1.559 |
| 4 | 0.410 | 0.373 | 1.942 | 0.961 |
| 5 | 0.319 | 0.351 | 1.821 | 1.543 |

Single-run spreads 1.90-2.57x; best-of-2 1.62-2.32x. Note every value is far below the old n=50-max-seeded baseline (solo 1.569), real percentiles are tamer than maxes, as predicted.

**daemon Linux** (p99 ms per shape, n=200):

| run | solo | fanout3 | burst | steady |
|---|---|---|---|---|
| 1 | 0.490 | 0.538 | 4.110 | 0.740 |
| 2 | 0.403 | 0.433 | 5.098 | 0.803 |
| 3 | 0.446 | 0.552 | 4.092 | 0.691 |
| 4 | 0.394 | 0.416 | **12.595** | 0.659 |
| 5 | 0.575 | 0.619 | 4.834 | 0.795 |

Recorded for Story 5.5, not acted on here (AC #7): the deferred ~40ms solo/fanout3/burst gap did **not** reproduce at n=200, solo/fanout3 sit at 0.4-0.6ms, in line with macOS. Either the n=50 max-gate was reporting the extreme tail (burst's 12.6ms outlier shows the tail still exists on ubuntu) or the runner image changed. `linux.json` stays all-zero per the maintainer punt; this observation just updates the evidence base 5.5 will start from.

### Completion Notes List

1. **All eight tasks complete.** Task 5's HUMAN-IN-THE-LOOP CI runs were executed by the dev agent via PR #33 (draft): the story's own protocol (~5 no-change runs, download artifacts, compute best-of-2 spread) was fully scriptable with `gh`. The PR body and the empty calibration commits are on the branch for maintainer review.
2. **Implementation plan as executed:** test-first for the wrapper (7 unittest cases written red against a missing script, then the wrapper written to green); CI wiring with one wrapper invocation per bench job; harness sample counts to 200/shape; docs reconciled to point at baseline files instead of restating numbers.
3. **Deviation, documented: `bench_steady` is count-paced, not clock-bounded.** The story said "40s at the existing 5/sec pacing, yielding ~200 samples" with steady staying time-derived. A clock-bounded 40s loop measured n=198 locally (connect + 50ms settle eat into the window), and n<200 breaks AC #3's "at least two samples strictly above p99". The loop now sends exactly `steady_secs x 5` events at the same 200ms period: rate unchanged, elapsed duration still ~40s (pacing enforces it), sample count deterministic at 200. This is the smallest change that satisfies AC #3 without raising the rate.
4. **Operational finding: branch protection names the old shim job.** Required status checks on `main` are literally "Shim hot-path bench (p99 ≤ 5ms, +15% regression fails) (macos-latest, macos.json)" / "(ubuntu-latest, linux.json)". Task 3's rename means those contexts will never report again once this merges, so the required checks must be repointed at the new job name ("Shim hot-path bench gate (per-platform policy in baselines + ADR 0003) (…)"), ideally at merge time, since flipping early blocks other in-flight PRs (e.g. #28) that still report the old names. Needs a maintainer (repo-settings) action; not doable from the working tree.
5. **Wrapper never writes baselines** (Dev Notes constraint): it only copies the attempt-1 summary to `*.attempt1.json` and appends to the step summary. The seventh test case (bench crash on attempt 2 propagates) pins the "breakage, not noise" rule on the retry path too.
6. **Doc sweep scope:** beyond the two named sites, the same drifted numbers were restated in `crates/shim/benches/README.md` (still claimed Linux +15%) and `architecture.md:481`; both now point at the committed baseline files. `product-brief-bowerbird-distillate.md` was left untouched (frozen distillate, not living documentation).

### File List

- `scripts/run-bench-gate.py` (new)
- `scripts/tests/test_run_bench_gate.py` (new)
- `scripts/check-shim-bench-p99.py` (docstring only)
- `scripts/check-daemon-bench-p99.py` (docstring + one comment)
- `.github/workflows/ci.yml`
- `crates/daemon/benches/hook_to_presenter.rs`
- `crates/shim/benches/README.md`
- `docs/bmad/project-context.md`
- `docs/bmad/planning-artifacts/architecture.md`
- `docs/bmad/implementation-artifacts/5-16-hotfix-shim-timeout-drops-events.md` (AC #5 + one References entry, threshold wording only)
- `docs/bmad/implementation-artifacts/deferred-work.md` (item 6 struck through)
- `docs/bmad/implementation-artifacts/5-18-bench-gates-flake-on-runner-noise.md` (this file: permitted sections)
- `docs/bmad/implementation-artifacts/sprint-status.yaml`
- `crates/shim/benches/baselines/macos.json` (reseeded, ratio restored at 2.0, absolute 20ms)
- `crates/daemon/benches/baselines/macos.json` (reseeded at n=200, ratio unchanged)
- `docs/decisions/0003-shim-p99-budget-on-macos-latest.md` (dated 2026-07-30 update section)
- Verified-not-changed: `crates/shim/benches/baselines/linux.json`, `crates/daemon/benches/baselines/linux.json` (AC #7)

## Change Log

| Date | Author | Summary |
|------|--------|---------|
| 2026-07-31 | claude-fable-5 | Three-layer subagent code review (Blind Hunter / Edge Case Hunter / Acceptance Auditor) returned 1 decision + 8 patch findings + 3 dismissed; all resolved same-session (see Review Findings Resolution). Headline fix: the wrapper now retries ONLY on gate exit 1 per AC #1 (it had followed Task 1's looser "any other non-zero" wording, so signal-killed or crashed gates would have been retried and a double-crash laundered into a policy verdict); gate scripts return 2 on missing baselines; robustness guards + 6 new wrapper tests (13 total); burst tracks runtime samples; ADR discloses the calibration window's clustering and the budget-stacking rationale; daemon macOS ratio kept at 1.30 by explicit maintainer decision, now documented in the seeding note; emdash sweep per the standing style rule. 638 workspace tests + 13 unittests + fmt + clippy + local bench green. |
| 2026-07-31 | claude-fable-5 | Calibration half (Tasks 5-6): 5 no-change CI runs on PR #33 (run 1 mechanism push + 4 empty commits), all 20 bench artifacts recorded in the Dev Agent Record. The wrapper fired in the wild during calibration: run 5 shim-macOS attempt 1 hit 15.130ms (over the then-current 15ms absolute), re-measure passed at 7.725ms. Reseeded shim macos.json (p99 7.725ms / mean 4.770ms, regression gate RESTORED at 2.0, absolute 15→20ms per the ~1.32x headroom-over-worst-observed formula) and daemon macos.json (per-shape worst at n=200, ratio unchanged 1.30); shim linux.json verified healthy and untouched; daemon linux.json untouched per AC #7. ADR 0003 amended with a dated 2026-07-30 update (not superseded) carrying the evidence tables, the budget-raise justification, and new revisit triggers. Fresh 5.5-relevant observation recorded: the ~40ms Linux daemon gap did not reproduce at n=200. |
| 2026-07-30 | claude-fable-5 | Mechanism half implemented (Tasks 1-4, 7, local Task 8): best-of-2 wrapper `scripts/run-bench-gate.py` + 7-case unittest suite wired into the `ci` job; both bench-gate CI jobs routed through the wrapper with widened `target/*bench-summary*.json` artifact globs and number-free job names; daemon harness at 200 samples/shape (fanout3 un-halved, burst 200, steady count-paced at 5/sec for 40s, see Completion Note 3 for the clock→count deviation); threshold docs de-numbered to point at baseline files + ADR 0003; deferred-work item 6 struck. fmt/clippy/638-test workspace/7 unittest green; local bench confirms no shape's p99 is its max. Tasks 5-6 (CI calibration runs, baseline reseed, ADR 0003 amendment) pending the HUMAN-IN-THE-LOOP CI-run step. Branch-protection rename side effect recorded in Completion Note 4. |
| 2026-07-30 | claude-opus-5 | Story created from taskwarrior `b6e4eceb` after a scoping pass that found the filed diagnosis covered one of the two incidents. Corrections folded in: all four daemon shapes are max-gates (not just solo at n=50); the shim incident is a distribution shift that no sample-count change addresses, and its real cause is that ADR 0003's 15ms calibration point was exceeded on a green run. Two candidate scope items were dropped after finding them already decided: seeding the Linux daemon baseline (2026-07-28 maintainer punt) and the ~40ms Linux gap (already filed with better evidence). Design decisions this session: best-of-2 re-measure with both attempts logged; Python wrapper rather than shell, since the repo has no shell test harness and the two gate scripts are already Python; steady grows to 40s at 5/sec rather than 20s at 10/sec, keeping duration as the thing the shape buys; macOS regression gate restored rather than left absolute-only. |
