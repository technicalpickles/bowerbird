# Release checklist

Ordered pre-tag runbook for cutting a bowerbird release. Authored by Story
5.12 ([release-pipeline-end-to-end-verification](bmad/implementation-artifacts/5-12-release-pipeline-end-to-end-verification.md))
per [Epic 4 retro AI-9](bmad/implementation-artifacts/epic-4-retro-2026-05-25.md),
consolidating AI-1 through AI-5 + AI-8 from that retro. Context on *why* each
step exists lives in that retro and in the
[Epic 3 retro](bmad/implementation-artifacts/epic-3-retro-2026-05-25.md) — this file is
deliberately just the ordered steps, not the reasoning.

Run every step in order. Don't skip ahead because a later step "should" pass —
the whole point of this checklist is that the pipeline has surprised us before.

## 1. Confirm daemon-bench baselines are seeded (Epic 4 retro AI-1)

```sh
cat crates/daemon/benches/baselines/macos.json
cat crates/daemon/benches/baselines/linux.json
```

Both files must carry non-zero `*_p99_nanos` values, OR the gap must be an
explicit, maintainer-approved deferral recorded in
[`deferred-work.md`](bmad/implementation-artifacts/deferred-work.md) (as of
Story 5.5, `linux.json` is deliberately still zeroed — a real, unconfirmed
~40x macOS/Linux p99 gap on rapid-fire ingestion shapes was found and punted
post-launch rather than silently baselined; `macos.json` is seeded and
armed). If a baseline is missing with no recorded deferral, stop and seed it
before continuing — do not tag with a silently-aspirational bench gate.

## 2. Chaos-injection sanity (Epic 4 retro AI-2, AI-3 — optional pre-rc1, recommended before final v0.1.0)

One draft PR per platform per gate, reverted before merge:

- **Daemon bench gate**: inject `tokio::time::sleep(50ms)` between commit and
  `broadcaster.publish` in `projection::session::write`; verify CI's
  `daemon-bench-gate` job fails on the burst-shape p99 regression.
- **Shim hot-path gate**: inject `std::thread::sleep(Duration::from_millis(2))`
  into `crates/shim/src/main.rs`'s hot path; verify CI's `shim-bench-gate`
  job fails on the p95 regression.

Document the verification (or its deferral) in the release notes.

## 3. Local workspace verification

```sh
cargo fmt --check
cargo clippy --all-targets --workspace -- -D warnings
scripts/test.sh
```

All three must be green. The workspace test suite **must** run serialized
(`--test-threads=1`, which `scripts/test.sh` passes by default): the daemon
contract + CLI E2E suites share process-wide state and hang/flake under
parallel execution (see
[Epic 2 retro AI-3](bmad/implementation-artifacts/epic-2-retro-2026-05-24.md); full root-cause writeup in
[the investigation doc](bmad/implementation-artifacts/investigations/test-serialization-investigation.md)).
Always run tests via `scripts/test.sh` rather than raw `cargo test`: a
*second* concurrent `cargo test` invocation in this worktree is the
confirmed trigger for this project's intermittent hangs (see
[the test-isolation findings](research/test-isolation-bowerbird-findings.md)).
The script takes a lock and enforces a timeout so a hang fails loudly
instead of running forever; if another run already holds the lock it exits
immediately rather than waiting, and `scripts/test.sh --unlock` force-clears
a stuck one.

## 4. Local tarball smoke test

```sh
cargo build --release --workspace --exclude bowerbird-shim
cargo build --profile release-shim -p bowerbird-shim
./scripts/tarball-smoke-test.sh <tag>
```

Confirms the 10 expected paths (three binaries, `adapters/claude/tool-reactions.toml`,
three license files, `README.md`, `INSTALL.md`, `CHANGELOG.md`) extract with
the right layout and executable bits, before paying the cost of a real CI
run. This script is intentionally NOT wired into CI (see the script's header
comment) — it's the local pre-flight gate.

## 5. crates.io namespace check (v0.1.0 only, not rc tags — Epic 4 retro AI-5)

```sh
cargo search bowerbird
```

Skip for `-rcN` tags. Run once, before the final non-prerelease `v0.1.0` tag
(Story 5.15's scope, not this checklist's). Document the result (available,
or a renaming decision) in the v0.1.0 release notes.

## 6. Push the tag

```sh
git tag v0.1.0-rc1
git push origin v0.1.0-rc1
```

Or, without pushing a ref directly, trigger the workflow manually against an
existing tag:

```sh
gh workflow run release.yml -f tag=v0.1.0-rc1
```

## 7. Verify the pipeline run

Watch the Actions run for the pushed tag (`gh run watch` or the Actions UI)
and confirm:

- **`build`** — all three matrix rows (`aarch64-apple-darwin`,
  `x86_64-apple-darwin`, `x86_64-unknown-linux-gnu`) green, artifacts
  uploaded.
- **`cross-version-test`** — SKIPs cleanly (no prior tag exists yet for
  `-rc1`; this becomes load-bearing at `-rc2`, see Epic 4 retro AI-8).
- **`release`** — GitHub Release created for the tag, all three tarballs
  plus their `.sha256` sidecars attached, `prerelease` and `draft` flags set
  (both are true for any tag containing `-`, per Story 5.12's draft-vs-
  prerelease decision — see that story's Dev Agent Record).

Capture the run URL and the observed artifact list in whatever record this
release's Dev Agent Record / notes live in.

## 8. Fresh-machine install + presenter smoke

On a clean macOS arm64 target (or a machine with `~/.bowerbird/` and
`~/.claude/settings.json` backed up and removed):

```sh
# From the release page (still a draft until you publish it — see step 7)
tar -xzf bowerbird-v0.1.0-rc1-aarch64-apple-darwin.tar.gz
cd bowerbird-v0.1.0-rc1-aarch64-apple-darwin
xattr -d com.apple.quarantine bin/* 2>/dev/null || true
sudo install -m 0755 bin/bowerbird bin/bowerbird-shim bin/bowerbird-daemon /usr/local/bin/
bowerbird install
```

Start a Claude Code session and confirm:

- Events land in `~/.bowerbird/bower.db`.
- `bowerbird status` shows the daemon running.
- The Story 5.1 first-party presenter receives `state.session.*` frames.

If this step is clean, publish the draft release (uncheck "draft" on the
GitHub Release page, or `gh release edit <tag> --draft=false`). If it's not
clean, leave the release as a draft (or delete it) and go to step 9.

## 9. Triage findings

If anything surfaced in steps 7 or 8, create a `5.X-hotfix-<topic>` story via
`bmad-create-story` and resolve it before moving on. If rc1 is clean, record
"no hotfix needed."

## After rc1: what changes for rc2+

Once `v0.1.0-rc1` has shipped, a prior tag exists. From `v0.1.0-rc2` onward:

- The `cross-version-test` job in step 7 stops SKIPping and actually runs the
  upgrade test body — verify it passes rather than just "SKIPs cleanly."
- `tests/cross_version_upgrade.rs`'s conventional (non-CI) fallback path
  still hardcodes `v0.1.0` for the manual/local install case; if you're
  running the cross-version test locally against an rc lineage, either set
  `BOWERBIRD_PRIOR_VERSION_BINARY` explicitly or update that hardcoded
  segment (see the file's module doc comment).
