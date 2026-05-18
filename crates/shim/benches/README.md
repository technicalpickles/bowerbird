# Shim hot-path bench: baselines and regression gate

The `hot_path.rs` Criterion bench measures end-to-end `bowerbird-shim`
invocation latency against a synchronous stdlib UDS mock. CI gates the
`uds_post_ingest` p99 against per-platform committed baselines in
`baselines/`.

## Files

- `baselines/macos.json` — committed baseline for `macos-latest` runners.
- `baselines/linux.json` — committed baseline for `ubuntu-latest` runners.

Each file is a copy of Criterion's `target/criterion/uds_post_ingest/<id>/estimates.json`.

## Initial seeding (both baselines come from CI)

**Important:** baselines must be seeded from CI runner artifacts, NOT
from local dev runs. Local hardware (especially Docker / VM dev
environments) can be 20–40% off from the GitHub Actions runner on this
workload, easily exceeding the +15% gate threshold. Seeding from a
local run produces a baseline that fails its own first CI verification.

For each platform (`linux.json` and `macos.json`):

1. Wait for any CI run to complete on a branch that builds the shim
   (this PR or `main`). The `shim-bench-gate` job emits a
   `::warning::` annotation noting the missing baseline and uploads
   `target/criterion/**` as a workflow artifact named
   `criterion-<runner-os>` (e.g. `criterion-ubuntu-latest`,
   `criterion-macos-latest`).
2. Download the artifact from the GitHub Actions run UI.
3. Copy `target/criterion/uds_post_ingest/new/estimates.json` into
   `crates/shim/benches/baselines/<platform>.json`
   (`linux.json` from `criterion-ubuntu-latest`, `macos.json` from
   `criterion-macos-latest`).
4. Commit the baseline file. The next CI run will use it as the gate
   baseline.

Once both baselines exist, every subsequent CI run loads them and
gates at +15% regression on the mean change estimate.

**Why soft-fail on missing baseline?** A brand-new platform shouldn't
red-light an otherwise-green PR. The committed baseline IS the gate;
missing baseline means the gate is unarmed for that platform, not that
the code is broken. The warning annotation is loud enough to nag, and
adding the baseline file is a trivial follow-up commit.

## Threshold rationale (+15%)

Per `docs/bmad/project-context.md` "Bench thresholds" table: 15% is the
hard fail threshold for shim p99. Smaller regressions (5%–15%) are
visible in CI logs but do not fail the build. The 15% number is
deliberately wide to absorb runner-to-runner noise without becoming a
useless gate; the AC target is p99 ≤ 5ms, and committed baselines
encode the actual runner numbers.

If CI reports a regression at or above 15%, the right responses (in
order of preference):

1. **Identify the offending PR and revert / fix it.** This is the
   common case.
2. **If the regression reflects a deliberate architectural change**
   (e.g. adding required serde validation that genuinely costs time),
   refresh the baseline in the same PR with a commit-body
   justification: "regression from feat X is intentional, new
   baseline reflects post-feat p99."
3. **If the committed baseline shows p99 > 5ms** on either platform
   for a clean codebase, do NOT silently raise the threshold. Per
   PRD line 181, file `docs/decisions/0002-shim-p99-budget.md` (or
   a successor) with the measured number, root-cause analysis, and
   either a tightened implementation plan or a justified budget
   revision.

## Refresh procedure (deliberate baseline bump)

1. Open a PR.
2. Pull the `criterion-<runner-os>` artifact from a green CI run on
   the PR (do NOT run the bench locally — CI hardware is the source
   of truth; see "Initial seeding" above for why).
3. Copy the new `estimates.json` over the committed baseline.
4. In the PR description, include:
   - the previous and new p99 numbers,
   - the reason for the change (architectural shift, dep update,
     toolchain bump, etc.),
   - confirmation that the new number still satisfies the AC
     (`p99 ≤ 5ms`) — or a link to the ADR raising the bar.

A reviewer signs off on the baseline change explicitly; auto-rolling
baselines defeat the gate (silently absorbed 1% regressions become 30%
over a year).
