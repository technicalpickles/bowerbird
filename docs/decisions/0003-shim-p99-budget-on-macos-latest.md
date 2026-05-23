# 0003. Shim p99 budget on `macos-latest` is unstable

Date: 2026-05-18 (original); 2026-05-20 (Linux update — see bottom)
Status: Accepted
Deciders: @pickles
Related: PRD line 181 ("If the number can't be met cleanly, the right response is an ADR"); Story 1.5 Task 9 acknowledgment; Story 1.5 Review Findings 1 + 2; Story 1.6 PR #19 bench-gate failure
Implementation: `.github/workflows/ci.yml` (`shim-bench-gate` job), `crates/shim/benches/hot_path.rs`, `crates/shim/benches/baselines/macos.json`, `crates/shim/benches/baselines/linux.json`
Affects context.md sections: "Shim hot-path discipline" (line 334)

## Context

Story 1.5 AC #1 requires shim per-invocation p99 ≤ 5 ms on **both** `macos-latest` and `ubuntu-latest`, with the gate failing CI on a regression > 15 % from per-platform committed baselines. The story's Task 9 acknowledgment was explicit:

> Acknowledge: if the first green CI run shows p99 > 5 ms on either platform, do NOT silently raise the threshold. Per PRD line 181, the right response is an ADR documenting the real number.

After Story 1.5 review finding 2 replaced the Criterion-mean gate with a true per-invocation p99 harness (every invocation timed individually with `Instant::now`, sorted, p99 picked from the sorted samples), three back-to-back CI runs on the same code (or no-op text-only changes) with no caching changes produced the following:

| Run | commit | linux p99 | macos p99 |
|---|---|---|---|
| 1 | `51e4a2f` | 1.103 ms | 2.664 ms |
| 2 | `6741e8e` (baseline-seed only) | 1.203 ms (+9 %) | 6.188 ms (+132 %) |
| 3 | `451dd7a` (docs-only) | 1.197 ms (+8.5 %) | **11.345 ms (+326 %)** |

`ubuntu-latest` is stable: three runs all within ±10 % of the seeded baseline, well inside the +15 % regression gate.

`macos-latest` is **not stable**: 2.66 → 6.19 → 11.35 ms on identical-behavior code, a 4.3× spread, with the mean climbing 2.54 → 4.0 → 5.2 ms across the same three runs. Every number is a real measurement from the official `macos-latest` GitHub-hosted runner, n=200 invocations each, warmup=20, with the bench harness from `crates/shim/benches/hot_path.rs`. Run 3 trips **both** halves of the original gate: the absolute 5 ms AC #1 budget AND the +15 % regression threshold against the freshly-seeded 2.66 ms baseline.

This is not a real shim regression — no shim code changed between the two runs. It is `macos-latest`'s noise floor for fork-exec-heavy workloads. Plausible root causes (none confirmed; each is a hypothesis worth testing if the team decides to invest in fixing this):

1. **`macos-latest` is a shared/virtualized macOS VM** (GitHub Actions hosted runners). CPU steal, scheduling, and codesign-verification latency are all variable across runs. Sibling projects (`tokio`, `rustls`, `pnpm`) have documented bench instability on this runner.
2. **Each shim invocation costs one fork + exec + dyld + codesign-trust-eval cycle.** macOS's `posix_spawn` and library-validation enforcement add hundreds of microseconds to milliseconds of variable overhead per process spawn, especially under contention.
3. **The UDS connect + write + read round-trip itself.** Less likely to be the variance source — the daemon side is a stdlib mock, and Linux measures the same workload at 1 ms with ±9 % noise.
4. **First-run cache effects.** Run 1 hit a cold runner image; run 2 hit a different cold image. macOS's kernel dentry/inode caches and the dynamic linker cache may be in different states. n=200 with warmup=20 likely doesn't fully amortize these on macOS.

Hypothesis: a single `macos-latest` run is informative about the day's runner state, not about the shim's p99 in a clean environment. The "true" per-invocation p99 of `bowerbird-shim` on macOS is somewhere in the 2.5–4 ms range under good conditions and 5–7 ms under noisy ones, with no causal coupling to shim code changes.

## Decision

**Chosen: Option B (per-platform absolute budget, regression gate disabled on macOS), with Option D kept as a future-work follow-up.**

The Story 1.5 review finding 1 + 2 work made the gate truthful: it now measures and enforces a real per-invocation p99. With that gate in place, three CI runs revealed `macos-latest` has a 4.3× p99 spread on unchanged code. The mean climbing across consecutive runs suggests the noise is not a transient first-run effect; it is the shared runner's baseline behavior for fork-exec-heavy workloads.

Per-platform thresholds now live in the committed baseline file itself (`crates/shim/benches/baselines/<platform>.json`). The gate script (`scripts/check-shim-bench-p99.py`) reads two optional fields:

- `absolute_budget_nanos` — per-platform absolute p99 ceiling. Missing field falls back to the original AC #1 default (5_000_000 ns).
- `regression_max_ratio` — multiplier on the committed `p99_nanos`. Missing or null disables the regression gate. Missing falls back to 1.15.

Committed policy:

| Platform | absolute_budget_nanos | regression_max_ratio | Rationale |
|---|---|---|---|
| `linux.json` | `5_000_000` (5 ms) | `1.15` | AC #1 as originally written. `ubuntu-latest` is stable enough that the regression gate is meaningful (three runs within ±10 %). |
| `macos.json` | `15_000_000` (15 ms) | `null` (disabled) | Worst observed p99 across three runs is 11.345 ms. 15 ms = ≈ 1.32× headroom over worst observed, absorbing the documented runner noise floor while still catching order-of-magnitude regressions. Regression gate disabled because no percentage threshold is meaningful against a baseline whose observed spread is 4.3×. |

Option D (rearchitect the shim away from per-invocation fork-exec) is the path to actually fix this — not absorb it. Recording it as future work, not as a Story 1.5 follow-up:

- Today, every Claude Code hook invokes `bowerbird-shim` as a fresh process.
- Story 3.1 (`bowerbird install`) is the obvious place to consider an alternative deployment shape — for example, the shim becoming a small UDS client that hands off to a long-running per-session process. That changes the cost model from "fork-exec on every hook" to "fork-exec once per session." Worth designing for if `macos-latest`'s noise remains the bottleneck.
- Without Option D, the macOS gate is honest about the runner but not informative about shim perf changes on macOS. A real macOS shim regression would need to be enormous to trip the 15 ms ceiling.

## Option space considered

Each option below resolves the immediate blocker for this PR. Trade-offs are spelled out so the team can weigh them.

### Option A — Drop `macos-latest` from the required bench gate (informational only)

Add `continue-on-error: true` to the `macos-latest` entry of the `shim-bench-gate` matrix. The job still runs, still uploads the summary artifact, still surfaces annotations in the PR UI — but a red `macos-latest` doesn't block merge. `ubuntu-latest` remains a required strict gate.

- **Pro:** unblocks merge today. Linux stays honest. macOS data is still collected for trend tracking.
- **Con:** AC #1 says "p99 ≤ 5 ms per platform" — formally weakened. Easy to lose discipline on macOS perf over time.
- **Cost:** ~3 lines of YAML.

### Option B — Raise the macOS-only absolute budget to a documented number

Keep both platforms required but split the budget: linux stays at `≤ 5 ms`, macOS rises to e.g. `≤ 10 ms`. The script reads per-platform thresholds from the baseline file (`{"schema_version": 2, "p99_nanos": …, "absolute_budget_nanos": 10_000_000}`) or a small thresholds table.

- **Pro:** keeps a required gate on macOS. Honest about the runner's actual capability.
- **Con:** explicit AC #1 revision. Hides the underlying noise problem rather than addressing it. The 10 ms number is approximate — if a real regression on macOS adds 2 ms, it stays under the new ceiling.
- **Cost:** small schema bump + script change; AC #1 amendment in the story file + PRD note.

### Option C — Switch macOS to a self-hosted or larger GitHub-hosted runner

Replace `macos-latest` with `macos-14-large` (paid M-class hosted runner with dedicated cores), or set up a self-hosted macOS runner (your own M-series hardware). Both reduce CPU-steal noise.

- **Pro:** keeps the strict gate, possibly without revising AC #1. macOS bench becomes a trustworthy signal.
- **Con:** `macos-14-large` adds GitHub Actions cost (~10× per-minute vs free `macos-latest`). Self-hosted runners are infra work. Either is plausible-but-not-free, and the bench job is a few minutes per PR.
- **Cost:** workflow YAML change OR self-hosted-runner setup + key management.

### Option D — Invest in shim macOS perf work

Find what specifically about the macOS code path is slow + variable, and either remove it (avoid fork-exec per invocation by switching the shim to a long-running daemon-client model) or compensate for it (warm the process-spawn cache aggressively). Story 1.5 took the fork-exec approach deliberately — every Claude Code hook fires a fresh process — so this would be a Story-1.5-bigger architectural change.

- **Pro:** addresses the root cause, not the symptom. macOS perf parity with Linux.
- **Con:** dramatically larger scope than Story 1.5 anticipated. The original AC was written assuming fork-exec is fine; reopening that assumption is a separate epic.
- **Cost:** weeks of work + a new ADR for the architectural shift.

### Option E — Take more samples / longer warmup, accept current shape

Crank `SHIM_BENCH_SAMPLES` from 200 to 1000+ and `SHIM_BENCH_WARMUP` from 20 to 100+. If macOS variance is dominated by cold-cache effects in the first ~50 invocations, a longer warmup may tighten p99 substantially. Each run now takes ~3 minutes on macOS instead of ~30 seconds; that may or may not be acceptable.

- **Pro:** if it works, no policy change needed. Cheapest to try.
- **Con:** speculative — there is no evidence yet that warmup-tightening is sufficient. If macOS noise is steady-state (CPU steal mid-run, not cold cache), longer runs don't help.
- **Cost:** one env-var change, one CI re-push, evaluate.

## Consequences

- `crates/shim/benches/baselines/macos.json` documents the chosen macOS budget (15 ms) and the disabled regression policy. The baseline `p99_nanos` value (2.66 ms — best observed in run 1) stays as the canonical "good day" reference and is informational only without the regression gate.
- `crates/shim/benches/baselines/linux.json` keeps the AC #1 5 ms budget and the +15 % regression threshold, explicitly written into the baseline rather than relying on script defaults — so per-platform policy is auditable in version control.
- AC #1 in the Story 1.5 file ("p99 latency is ≤ 5 ms per platform") is technically amended for `macos-latest` by this ADR; the story file links here as the canonical record rather than rewriting the original AC text.
- The Story 1.5 PR can land. Both bench-gate jobs should now pass: linux on the strict gate, macOS on the wider absolute budget with regression detection deferred.
- Option D is the path to fix this if Story 3.1 (or anything else) needs macOS shim perf to be trustworthy. Worth a separate ADR if it gets picked up.

## Revisit when

- A real macOS shim regression slips past the 15 ms budget — at which point Option D moves from "follow-up" to "required" and this ADR is superseded.
- The runner topology changes (GitHub deprecates `macos-latest`, your team adds self-hosted macOS hardware, etc.) — re-evaluate whether the budget can be tightened.
- Story 3.1 (`bowerbird install`) lands and the shim's deployment model shifts in a way that affects per-invocation cost — re-examine the macOS budget and whether the regression gate can be re-enabled with a meaningful threshold.

## Update 2026-05-20: Linux runner-image drift

Story 1.6 (PR #19) tripped the Linux gate twice on a branch whose shim diff is empty (`crates/shim/` is unchanged from main; only `nix` + `cfg_aliases` were added to Cargo.lock, neither in the shim's dep tree). The same-day main run on `b5b798d` (docs-only, no code change) measured p99 1.195 ms, +8.31 % over the original seeded baseline of 1.103 ms.

| Run | commit | linux p99 | linux mean |
|---|---|---|---|
| Original seed | `51e4a2f` | 1.103 ms | 0.819 ms |
| Main today | `b5b798d` (docs only) | 1.195 ms (+8.3 %) | 1.027 ms (+25 %) |
| Branch run 1 | `4f0abaa` | 1.566 ms (+42 %) | 1.072 ms (+31 %) |
| Branch run 2 | `4f0abaa` (rerun) | 1.353 ms (+23 %) | 1.085 ms (+32 %) |

Mean spread across the three same-day runs is 5.6 % (1.027 → 1.085 ms) — well inside any reasonable regression band. p99 spread is 31 % (1.195 → 1.566 ms). The original ADR concluded `ubuntu-latest` was stable based on a 3-run sample with ±10 % variance; today's same-day 3-run sample shows the runner-image generation has a wider noise floor than that. The shim binary itself is unchanged, so this is infrastructure drift, not a real regression.

The same option-B pattern applies here as it did for macOS: reseed the baseline from the freshest stable observation and loosen `regression_max_ratio` enough to absorb the observed noise floor while still catching order-of-magnitude regressions.

Updated Linux policy (committed in `crates/shim/benches/baselines/linux.json`):

| Field | Old value | New value | Rationale |
|---|---|---|---|
| `p99_nanos` | 1,102,908 (1.103 ms) | 1,194,533 (1.195 ms) | Reseeded from main `b5b798d` CI artifact — same pattern as the original seed commit `6741e8e`. Freshest "good day" reference. |
| `mean_nanos` | 819,448 | 1,027,350 | Same source. |
| `absolute_budget_nanos` | 5,000,000 (5 ms) | unchanged | AC #1 ceiling holds. Plenty of headroom (current p99 is ~24 % of budget). |
| `regression_max_ratio` | 1.15 (+15 %) | 1.35 (+35 %) | Absorbs the demonstrated 31 % same-day p99 spread with modest headroom. Anything that ~doubles p99 or trips the 5 ms absolute still fails the gate, so it remains a meaningful detector of real regressions. |

New gate ceiling: 1.195 × 1.35 ≈ 1.61 ms (vs old ceiling of 1.268 ms). Both observed branch runs (1.353, 1.566) would pass under the new policy.

### Why not Option D (rearchitect) for Linux

Option D in the original ADR proposed rearchitecting the shim away from per-invocation fork-exec. For Linux that's not yet warranted — the absolute p99 is ~1.5 ms even on noisy runs, well under the 5 ms AC #1 budget. Linux fork-exec is fast and the shim's runtime cost on real user machines is fine. The fix is the gate, not the shim.

### Revisit when (Linux-specific)

- The reseeded baseline goes stale again — Linux runner image rolls forward and the same-day mean climbs another 25 %. Then reseed (cheap) or invest in trend-tracking (more involved).
- Story 3.1 changes the shim's deployment model — re-examine both platforms at once.
- A real shim regression of >35 % on Linux gets shipped without being caught — at which point the gate's effectiveness needs a more careful look (rolling average baseline, multi-run sampling per CI invocation, etc.).
