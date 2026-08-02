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

## Writing tests

The suite runs in parallel (libtest default threads). Every test must be
parallel-safe, and CI's 4-vCPU runners can starve a test's thread for
seconds, so timing assumptions that hold on a fast laptop are bugs:

- Isolate state per test: `TempDir` data dirs, ephemeral daemon ports
  (`127.0.0.1:0`), env passed per-child via `Command::env`.
- Never mutate process env (`std::env::set_var`) — it races concurrent env
  reads and subprocess spawns. `clippy.toml` bans it. Inject a snapshot
  instead: see `token::TokenEnv` for the seam shape.
- Never sleep to synchronize with the daemon. Use the probe fences in
  `contract_daemon.rs::story_2_2_publish` (`wait_subscribe_live`,
  `wait_unsubscribe_processed`).
- Timeouts around recvs/polls/child-exits are hang detectors, not latency
  assertions: use `contract_daemon.rs`'s `HANG_GUARD` (30s), never a tight
  value. Semantic timings (ping intervals, coalesce windows, paused-clock
  tests) are the exception.
- Read WS frames through the shared helpers (`read_text_frame_or_close`
  etc.) — they skip keepalive Ping/Pong. Raw `ws.next()` is only for tests
  asserting on pings themselves.

The full rationale (with the CI failure history that produced each rule)
is in `docs/bmad/project-context.md` §Deterministic test discipline.

## Story File List audits

A BMAD story's `### File List` is verified against git, never self-attested.
Before a story goes to `review`, and again when reviewing one:

```sh
python3 scripts/check-file-list.py <story-file> --base main
```

Exit 1 is drift, not a script failure. Team agreement A9 makes it HIGH
severity in both directions: changed in git but undeclared, and declared but
unchanged (an edit was claimed that never shipped). Fix the record, not the
audit: `--ignore` is for paths genuinely outside the story's authorship, and
using it belongs in the Completion Notes.

This runs automatically under `bmad-dev-story` and `bmad-code-review` (wired
via `_bmad/custom/*.toml`); run it by hand on any other path. The drift it
catches is side-effect files (sprint-status bumps, planning-doc edits,
formatter reflows) that the dev's recollection of intentional edits drops.
That pattern bit eight stories across Epics 3 through 5, which is why the
check now lives at the point where the record is written.

Full project context (architecture, decisions, conventions) lives in
`docs/bmad/project-context.md`.
