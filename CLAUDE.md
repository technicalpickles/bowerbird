# bowerbird

## Running tests

Always run tests via `scripts/test.sh`, never raw `cargo test`. A second
`cargo test` process running concurrently in this worktree is the confirmed
trigger for this project's intermittent test hangs. See
`docs/research/test-isolation-bowerbird-findings.md` and
`docs/bmad/implementation-artifacts/investigations/test-serialization-investigation.md`.
`scripts/test.sh` takes an exclusive lock and runs under a timeout (so a
real hang fails loudly instead of hanging forever). Every run's output is
tee'd to `target/test-logs/<run>/run.log`; on a timeout the script first
captures `sample` backtraces of the live test and daemon processes into
that same run dir (and, for a serial `--test-threads=1` run, which test was
mid-run), then kills the tree and exits 124. The default run is parallel;
re-run serially when you need the hung test named. If a run times out, look there before rerunning — that capture is the
evidence the hang investigation needs. If another run already
holds the lock, it exits immediately rather than waiting — it does not
block/poll, so don't retry-loop on it either; re-run once the other one
finishes, or use `--unlock` (below) if it looks stuck.

```sh
scripts/test.sh                          # cargo test --workspace (parallel)
scripts/test.sh -p bowerbird-daemon --test contract_daemon -- --exact some_test
```

This applies to background/parallel agent work too: never launch a second
test run while one is already in flight in this worktree, even in a
different subagent or terminal. Ctrl-C / SIGTERM on a running
`scripts/test.sh` stops it immediately (kills the test process tree,
releases the lock) rather than leaving it running.

If a run is stuck (hung past its timeout, or the lock looks stale) and you
need to clear it, don't hand-kill processes or `rm -rf` the lock directory —
run `scripts/test.sh --unlock`. It kills the stuck run's process tree (SIGTERM,
then SIGKILL after a grace period) and removes the lock, so a fresh
`scripts/test.sh` can proceed.

Full project context (architecture, decisions, conventions) lives in
`docs/bmad/project-context.md`.
