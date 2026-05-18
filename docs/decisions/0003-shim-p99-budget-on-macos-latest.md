# 0003. Shim p99 budget on `macos-latest` is unstable

Date: 2026-05-18
Status: Proposed (decision deferred — see "Open question")
Deciders: @pickles
Related: PRD line 181 ("If the number can't be met cleanly, the right response is an ADR"); Story 1.5 Task 9 acknowledgment; Story 1.5 Review Findings 1 + 2
Implementation: `.github/workflows/ci.yml` (`shim-bench-gate` job), `crates/shim/benches/hot_path.rs`, `crates/shim/benches/baselines/macos.json`
Affects context.md sections: "Shim hot-path discipline" (line 334)

## Context

Story 1.5 AC #1 requires shim per-invocation p99 ≤ 5 ms on **both** `macos-latest` and `ubuntu-latest`, with the gate failing CI on a regression > 15 % from per-platform committed baselines. The story's Task 9 acknowledgment was explicit:

> Acknowledge: if the first green CI run shows p99 > 5 ms on either platform, do NOT silently raise the threshold. Per PRD line 181, the right response is an ADR documenting the real number.

After Story 1.5 review finding 2 replaced the Criterion-mean gate with a true per-invocation p99 harness (every invocation timed individually with `Instant::now`, sorted, p99 picked from the sorted samples), two back-to-back CI runs on the same code with no caching changes produced the following:

| Run | commit | linux p99 | macos p99 |
|---|---|---|---|
| 1 | `51e4a2f` | 1.103 ms | 2.664 ms |
| 2 | `6741e8e` (baseline-seed only) | 1.203 ms (+9 %) | **6.188 ms (+132 %)** |

`ubuntu-latest` is stable: the two runs differ by ≈ 9 %, well inside the +15 % regression gate.

`macos-latest` is **not stable**: the same code measured 2.66 ms then 6.19 ms across two consecutive runs. Both numbers are real measurements from the official `macos-latest` GitHub-hosted runner, n=200 invocations each, warmup=20, with the bench harness from `crates/shim/benches/hot_path.rs`. The 6.19 ms run trips **both** halves of the gate: the absolute 5 ms AC #1 budget AND the +15 % regression threshold against the freshly-seeded 2.66 ms baseline.

This is not a real shim regression — no shim code changed between the two runs. It is `macos-latest`'s noise floor for fork-exec-heavy workloads. Plausible root causes (none confirmed; each is a hypothesis worth testing if the team decides to invest in fixing this):

1. **`macos-latest` is a shared/virtualized macOS VM** (GitHub Actions hosted runners). CPU steal, scheduling, and codesign-verification latency are all variable across runs. Sibling projects (`tokio`, `rustls`, `pnpm`) have documented bench instability on this runner.
2. **Each shim invocation costs one fork + exec + dyld + codesign-trust-eval cycle.** macOS's `posix_spawn` and library-validation enforcement add hundreds of microseconds to milliseconds of variable overhead per process spawn, especially under contention.
3. **The UDS connect + write + read round-trip itself.** Less likely to be the variance source — the daemon side is a stdlib mock, and Linux measures the same workload at 1 ms with ±9 % noise.
4. **First-run cache effects.** Run 1 hit a cold runner image; run 2 hit a different cold image. macOS's kernel dentry/inode caches and the dynamic linker cache may be in different states. n=200 with warmup=20 likely doesn't fully amortize these on macOS.

Hypothesis: a single `macos-latest` run is informative about the day's runner state, not about the shim's p99 in a clean environment. The "true" per-invocation p99 of `bowerbird-shim` on macOS is somewhere in the 2.5–4 ms range under good conditions and 5–7 ms under noisy ones, with no causal coupling to shim code changes.

## Decision

**Status: Proposed.** This ADR records the problem and the option space; it does **not** pick a path. The team picks one (or stacks them) in a follow-up.

The implementation as of `6741e8e` is the strict-on-everything gate from Story 1.5 review finding 1: missing baseline = fail, absolute budget violation = fail, +15 % regression = fail. That gate is now correct AND will block PRs on `macos-latest` noise unrelated to shim changes. Something must change before this branch can merge to `main` without leaving a chronically-red required check on every subsequent PR.

## Option space

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

## Open question

**Pick one (or stack: e.g. E first, A if E doesn't help; B + C; etc.).** The PR cannot merge without one of these landing. Recommend E as a 5-minute experiment first, then either A or B as the durable answer if E doesn't tighten the noise floor.

## Consequences

Until this is resolved:

- The Story 1.5 PR has `shim-bench-gate (macos-latest)` failing red on every push.
- Linux gate is honest and operational.
- The strict-on-missing-baseline design from finding 1 is correct — do NOT roll that back to soft-pass as a workaround. The macOS issue is a budget question, not a gate question.
- AC #1 is satisfied on `ubuntu-latest` (p99 ≈ 1.1 ms) and **arguably** satisfied on `macos-latest` in clean conditions (p99 ≈ 2.6 ms in run 1). The noise floor is the new datum that wasn't visible under the old mean-only gate.

## Revisit when

- A decision is made on the option-space above (move Status from Proposed → Accepted with the chosen path, or supersede this ADR with one that picks a specific direction).
- The runner topology changes (GitHub deprecates `macos-latest`, your team adds self-hosted macOS hardware, etc.).
- Story 3.1 (`bowerbird install`) lands and the shim's deployment model shifts in a way that affects per-invocation cost.
