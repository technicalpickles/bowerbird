# Shim hot-path bench: baselines and p99 gate

`hot_path.rs` is a `harness = false` Cargo bench that measures the
end-to-end `bowerbird-shim` invocation latency against a synchronous
stdlib UDS mock and writes a small JSON summary the CI gate consumes
directly. It does NOT use Criterion: Criterion's `SamplingMode::Flat`
still batches multiple iterations per sample on ~1–5ms workloads, so a
high-tail regression can sneak past a mean-based gate (Story 1.5 review
finding 2). The custom harness records every invocation with
`std::time::Instant::now()` and sorts the raw timings, so the p99 the
gate enforces is a real per-invocation p99.

## Files

- `hot_path.rs` — the bench binary. Writes `target/shim-bench-summary.json`.
- `baselines/macos.json` — committed baseline for `macos-latest` runners.
- `baselines/linux.json` — committed baseline for `ubuntu-latest` runners.

## Summary schema

The bench writes a minimal current-run summary:

```json
{
  "schema_version": 1,
  "p99_nanos": 1700000,
  "mean_nanos": 1450000,
  "samples": 200
}
```

The committed baseline adds **per-platform policy fields** (introduced
by ADR 0003 — see `docs/decisions/0003-shim-p99-budget-on-macos-latest.md`):

```json
{
  "schema_version": 1,
  "p99_nanos": 1102908,
  "mean_nanos": 819448,
  "samples": 200,
  "absolute_budget_nanos": 5000000,
  "regression_max_ratio": 1.15
}
```

- `absolute_budget_nanos` — per-platform p99 ceiling (5 ms for
  `linux.json`, 15 ms for `macos.json`). Missing field falls back to
  5 ms.
- `regression_max_ratio` — multiplier on the committed `p99_nanos`.
  `null` disables the regression gate (currently set on `macos.json`
  per ADR 0003; the runner's documented 4.3× noise floor makes no
  percentage threshold meaningful). Missing field falls back to 1.15.

`scripts/check-shim-bench-p99.py` reads both files and enforces:

1. **Absolute gate:** `current p99_nanos <= absolute_budget_nanos`.
2. **Regression gate:** `current p99_nanos <= committed p99_nanos * regression_max_ratio` (skipped when ratio is `null`).
3. **Missing baseline = hard fail.** Both gates are unarmed without a
   committed baseline; the required CI job exits non-zero until the
   baseline is committed.

Both gates run on every PR via the `shim-bench-gate` job in
`.github/workflows/ci.yml`.

## Current per-platform policy

| Platform | Absolute budget | Regression gate | Source |
|---|---|---|---|
| `linux.json` | 5 ms | +15 % | AC #1 |
| `macos.json` | 15 ms | disabled | ADR 0003 |

`macos-latest` noise is documented in ADR 0003. The 15 ms ceiling
absorbs the runner's measured spread (2.66 → 11.35 ms across three
no-op runs). Regression detection on macOS is deferred to Option D
follow-up work (rearchitect the shim away from per-invocation
fork-exec).

## Bench configuration

The bench takes two environment variables:

- `SHIM_BENCH_SAMPLES` (default `200`) — number of measured invocations.
- `SHIM_BENCH_WARMUP` (default `20`) — invocations discarded before
  measurement begins (lets the OS warm UDS / dentry cache).

Running locally:

```sh
cargo bench --profile release-shim -p bowerbird-shim --bench hot_path
```

The bench prints results to stderr and writes the canonical summary to
`target/shim-bench-summary.json`.

## Seeding the baselines (first run on each platform)

Baselines MUST come from CI runner hardware, not local machines. Local
hardware (especially macOS dev laptops and Docker-on-Mac Linux VMs) can
be 20–40% off from the GitHub Actions runner on this workload, easily
exceeding the +15% gate threshold. A locally-seeded baseline fails its
own first CI verification.

For each platform (`linux.json` and `macos.json`):

1. Push a PR. The `shim-bench-gate` job fails with
   `::error::No committed baseline at …` and uploads
   `target/shim-bench-summary.json` as an artifact named
   `shim-bench-<runner-os>` (e.g. `shim-bench-ubuntu-latest`,
   `shim-bench-macos-latest`).
2. Download the artifact from the GitHub Actions run UI.
3. Copy its `shim-bench-summary.json` into
   `crates/shim/benches/baselines/<platform>.json`
   (`linux.json` from `shim-bench-ubuntu-latest`,
   `macos.json` from `shim-bench-macos-latest`).
4. Commit the baseline file. The next CI run arms the regression gate.

The intentional design: a brand-new platform red-lights the PR until
the baseline is committed. The fail makes the seed step impossible to
forget — soft-fail-on-missing would let an unarmed gate ship to main
unnoticed (Story 1.5 review finding 1).

## Threshold rationale (+15%)

Per `docs/bmad/project-context.md` "Bench thresholds": 15% is the
hard-fail threshold for shim p99. Smaller regressions (5%–15%) are
visible in CI logs but do not fail the build. The 15% number is wide
enough to absorb runner-to-runner noise without becoming a useless
gate; the AC target is `p99 ≤ 5ms`, and committed baselines encode the
actual runner numbers.

If CI reports a regression at or above 15%:

1. **Identify the offending PR and revert or fix.** Common case.
2. **If the regression is from a deliberate architectural change**
   (e.g. adding required serde validation that genuinely costs time),
   refresh the baseline in the same PR with a commit-body
   justification: "regression from feat X is intentional, new baseline
   reflects post-feat p99."
3. **If the committed baseline shows p99 > 5ms** on either platform
   for a clean codebase, do NOT silently raise the threshold. Per
   PRD line 181, file an ADR with the measured number, root cause,
   and either a tightened implementation plan or a justified budget
   revision.

## Baseline refresh (deliberate bump)

1. Open a PR.
2. Pull the `shim-bench-<runner-os>` artifact from a green CI run on
   the PR (do NOT run the bench locally — CI hardware is the source of
   truth).
3. Copy the new `shim-bench-summary.json` over the committed baseline.
4. In the PR description, include:
   - the previous and new p99 numbers,
   - the reason for the change (architectural shift, dep update,
     toolchain bump, etc.),
   - confirmation that the new number still satisfies AC #1
     (`p99 ≤ 5ms`) — or a link to the ADR raising the bar.

A reviewer signs off on the baseline change explicitly. Auto-rolling
baselines defeat the gate (silently absorbed 1% regressions become
30% over a year).
