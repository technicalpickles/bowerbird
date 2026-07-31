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

- `absolute_budget_nanos` — per-platform p99 ceiling. Missing field
  falls back to 5 ms.
- `regression_max_ratio` — multiplier on the committed `p99_nanos`.
  `null` disables the regression gate. Missing field falls back to
  1.15.

The committed baseline files are the source of truth for both values —
this README deliberately does not restate them (restated numbers drift;
Story 5.18 closed exactly that). ADR 0003 and its dated updates record
why each platform's policy is what it is.

`scripts/check-shim-bench-p99.py` reads both files and enforces:

1. **Absolute gate:** `current p99_nanos <= absolute_budget_nanos`.
2. **Regression gate:** `current p99_nanos <= committed p99_nanos * regression_max_ratio` (skipped when ratio is `null`).
3. **Missing baseline = hard fail.** Both gates are unarmed without a
   committed baseline; the required CI job exits non-zero until the
   baseline is committed.

Both gates run on every PR via the `shim-bench-gate` job in
`.github/workflows/ci.yml`, wrapped by `scripts/run-bench-gate.py`
(Story 5.18): a policy failure earns exactly one re-measure, recorded
in the step summary; tooling failures and bench crashes never retry.

## Current per-platform policy

Read it from `baselines/linux.json` and `baselines/macos.json` — the
committed files are the policy. The rationale (macOS runner noise, the
Linux runner-image drift, and the 2026-07-30 recalibration) lives in
ADR 0003 and its dated update sections.

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
exceeding the committed regression ratio. A locally-seeded baseline fails its
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

## Threshold rationale

The regression ratio is per-platform and lives in the baseline files
(see "Current per-platform policy" above). Each ratio is calibrated
from measured multi-run spread on that runner — wide enough to absorb
runner-to-runner noise without becoming a useless gate — and the
absolute budget backstops it. ADR 0003 records each calibration.

If CI reports a regression past the committed ratio (on both best-of-2
attempts — a single-attempt failure that passes the re-measure is
counted as runner noise in the step summary):

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
