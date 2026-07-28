# bowerbird

## Running tests

Always run tests via `scripts/test.sh`, never raw `cargo test`. A second
`cargo test` process running concurrently in this worktree is the confirmed
trigger for this project's intermittent test hangs — see
`docs/research/test-isolation-bowerbird-findings.md` and
`docs/bmad/implementation-artifacts/investigations/test-serialization-investigation.md`.
`scripts/test.sh` takes an exclusive lock (so a second invocation queues
instead of racing) and runs under a timeout (so a real hang fails loudly
instead of hanging forever).

```sh
scripts/test.sh                          # cargo test --workspace -- --test-threads=1
scripts/test.sh -p bowerbird-daemon --test contract_daemon -- --exact some_test
```

This applies to background/parallel agent work too: never launch a second
test run while one is already in flight in this worktree, even in a
different subagent or terminal.

If a run is stuck (hung past its timeout, or the lock looks stale) and you
need to clear it, don't hand-kill processes or `rm -rf` the lock directory —
run `scripts/test.sh --unlock`. It kills the stuck run's process tree (SIGTERM,
then SIGKILL after a grace period) and removes the lock, so a fresh
`scripts/test.sh` can proceed.

Full project context (architecture, decisions, conventions) lives in
`docs/bmad/project-context.md`.
