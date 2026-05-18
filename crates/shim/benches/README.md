# Shim hot-path bench: baselines and regression gate

The `hot_path.rs` Criterion bench measures end-to-end `bowerbird-shim`
invocation latency against a synchronous stdlib UDS mock. CI gates the
`uds_post_ingest` p99 against per-platform committed baselines in
`baselines/`.

## Files

- `baselines/macos.json` — committed baseline for `macos-latest` runners.
- `baselines/linux.json` — committed baseline for `ubuntu-latest` runners.

Each file is a copy of Criterion's `target/criterion/uds_post_ingest/<id>/estimates.json`.

## Initial seeding (first green run on each platform)

1. Locally on the dev's machine, after Tasks 1–5 of Story 1.5 are
   implemented, run:
   ```sh
   cargo bench -p bowerbird-shim --profile release-shim -- \
     --save-baseline initial uds_post_ingest
   ```
2. Copy `target/criterion/uds_post_ingest/initial/estimates.json` into
   `crates/shim/benches/baselines/<host-platform>.json`
   (`macos.json` on macOS, `linux.json` on Linux). Commit.
3. CI's first run on the *other* platform will emit a `::warning::`
   annotation (not a failure) noting the missing baseline. The gate is
   unarmed for that platform until the baseline lands. Download the
   `criterion-<runner-os>` artifact from the workflow run, copy
   `target/criterion/uds_post_ingest/new/estimates.json` into the
   missing baseline file, and commit it (separate PR or part of the
   next one, your choice).
4. Once both `macos.json` and `linux.json` exist, all subsequent CI
   runs use `--load-baseline` against them and gate at +15% regression
   on the mean change estimate.

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
2. Run the bench locally on the matching platform (or pull the
   `criterion-<runner-os>` artifact from CI after a green run on a
   different baseline file).
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
