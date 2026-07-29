# Investigation: Why does CI serialize the entire test suite (`--test-threads=1`)?

## Hand-off Brief

1. **What happened.** CI serializes the *entire* workspace test suite with `--test-threads=1` (`.github/workflows/ci.yml:41`), but the real reason is a single daemon test module (`contract_daemon.rs::story_3_3_auth`, 9 in-process tests) that mutates **process-global `std::env`** and installs an **irreversible process-global keyring mock backend** — both of which race under parallelism. The four culprits named in the CI comment (subprocesses, signal handlers, file fixtures, keychains) are folklore: none was ever diagnosed, and the only deadlock anyone root-caused was an unrelated SQLite-teardown drop-order bug that happened *under* the flag and was fixed in Epic 4.
2. **Where the case stands.** **Concluded, High confidence.** Real cause confirmed in code (the module's own doc-comment states the `--test-threads=1` requirement and why). H1 (signal handlers) and H2 (flock) refuted with `file:line` evidence. H3 (cargo-cult) partially confirmed: the *stated* rationale is cargo-cult, but a narrow real need exists, so deleting the flag outright would flake. The actual cause is H4.
3. **What's needed next.** De-globalize the 9 `story_3_3_auth` tests (convert to the per-child subprocess pattern the rest of the workspace already uses, or adopt nextest's process-per-test for that module), then delete the workspace-wide flag. Saves ~60-90s/CI run and replaces folklore with a named cause.

## Case Info

| Field            | Value                                                                      |
| ---------------- | -------------------------------------------------------------------------- |
| Ticket           | N/A (worktree: `isolation-audit`)                                          |
| Date opened      | 2026-06-01                                                                 |
| Status           | Concluded (High confidence; root cause confirmed)                          |
| System           | Rust workspace (4 crates), Tokio current-thread, GitHub Actions macOS+Linux |
| Evidence sources | CI workflow, architecture.md, story files, story retros, test source       |

## Problem Statement

CI serializes the entire workspace test suite with `--test-threads=1`. The stated rationale (CI comment + architecture.md "Contract-test serialization (operational note)") attributes hangs under parallel execution to four shared-state sources, observed across Stories 1.6, 2.5, 3.1, 3.2, 3.3.

**The premise is suspect.** A prior pass found the tests already well-isolated: each uses its own `tempfile::TempDir`, passes env vars PER-SUBPROCESS via `Command::env(...)` (not process-global `std::env::set_var`), and binds ephemeral ports via `TcpListener::bind("127.0.0.1:0")`. Per-subprocess env doesn't leak; ephemeral ports don't collide. So the documented culprits shouldn't cause parallel hangs — yet the hangs were real. Something else is the actual cause.

### Hypotheses (registered, to be graded)

- **H1.** In-process `#[tokio::test]` daemon instances fight over process-global signal handler registration (SIGTERM/SIGINT installed at daemon startup). Last-writer-wins across tests in the same test binary.
- **H2.** The daemon's PID-file + flock singleton collides on a FIXED (non-tempdir) path, so two concurrent subprocess tests deadlock/hang on the lock.
- **H3.** Cargo-cult — isolation is already sufficient and `--test-threads=1` can be deleted outright.
- **H4 (open slot).** A fourth cause: shared tokio runtime, global tracing subscriber init, fd/subprocess-count exhaustion under parallelism, or another shared OS-level resource.

## Evidence Inventory

| Source   | Status      | Notes     |
| -------- | ----------- | --------- |
| `.github/workflows/ci.yml:31-41` | Available | Stronghold. CI comment names 4 culprits, attributes hangs to Stories 1.6/2.5/3.1/3.2/3.3, cites "Epic 2 retro AI-3 / Discovery #3". |
| `docs/bmad/planning-artifacts/architecture.md` "Contract-test serialization (operational note)" | Available | Mirrors the CI comment. Same 4 culprits. |
| `crates/daemon/tests/contract_daemon.rs` | Available (9034 lines / 352K) | The daemon contract suite. Over 10K-token threshold → must delegate analysis to subagents returning JSON. Mix of in-process `#[tokio::test]` and subprocess `assert_cmd` tests. |
| `tests/cli_*.rs` (14 files, ~5K lines) | Available | Workspace CLI E2E suites, assert_cmd subprocess tests. |
| `crates/daemon/src/` | Available | Where signal handlers + PID-file/flock singleton live (H1, H2). Not yet read. |
| `crates/shim/src/` | Available | Shim singleton/lock behavior (H2). Not yet read. |
| Story retros (`epic-2-retro-2026-05-24.md` etc.) + story files 1-6, 2-5, 3-1, 3-2, 3-3 | Available, not yet read | Primary source for the *original* hang observations. The CI comment is a secondary summary of these. |
| `docs/research/test-isolation-parallelism-research-results.md` | Available | Analysis framework (§1 diagnostic checklist, §1d false alarms, §3 partition rubric). nextest process-per-test neutralizes H1 class for free. |

## Investigation Backlog

| # | Path to Explore | Priority | Status | Notes |
| - | --------------- | -------- | ------ | ----- |
| 1 | Partition `contract_daemon.rs` + `tests/cli_*.rs` into in-process vs subprocess tests | High | Done | See Test→Resource map. 9 risky tests isolated to `story_3_3_auth`. |
| 2 | Find signal-handler registration; process-global? | High | Done | `main.rs:419-441`, binary-only. H1 refuted. |
| 3 | Find PID-file + flock singleton; fixed or per-tempdir? | High | Done | `singleton.rs:76-77`, per-TempDir. H2 refuted. |
| 4 | Build test → shared-resource map | High | Done | Delivered. |
| 5 | Read story retros for original hang symptoms | High | Done | No diagnosis ever performed; folklore. See Finding 1/3. |
| 6 | Check for global tracing subscriber init | Medium | Done | `try_init`, not called by tests. Not a cause. |
| 7 | Apply research §3 partition rubric | Medium | Done | Group A / Group B partition in Recommended Next Steps. |
| 8 | **(follow-up)** De-globalize the 9 `story_3_3_auth` tests, then remove the flag | High | Open | The fix. Not implemented this pass per scope. |

## Timeline of Events

| Time | Event | Source | Confidence |
| ---- | ----- | ------ | ---------- |
| Stories 1.6 → 3.3 | Parallel test hangs observed incrementally; flag presumably added/reinforced | CI comment, architecture.md | Hypothesized (need retros to confirm when/how flag was introduced) |
| 2026-06-01 | Investigation opened | this case | Confirmed |

## Test → Shared-Resource Map

Scope: the two suites the flag targets. The daemon contract suite was partitioned by a delegated subagent (file is 9034 lines); the workspace `tests/*.rs` suites by direct grep. "Risky" = unsafe under parallel execution.

### `crates/daemon/tests/contract_daemon.rs` (~165 tests: ~153 in-process, 8 subprocess, 4 mixed)

| Test / group | Type | Process-global / OS resource touched | Parallel-safe? |
| --- | --- | --- | --- |
| `story_3_3_auth` — 9 in-process `#[test]` fns (lines 7416-7602: `env_var_wins_*`, `disable_*` x6, `mock_keychain_first_run_*`, `mock_env_var_wins_*`) | in_process | **`std::env::set_var`/`remove_var`** on `BOWERBIRD_TOKEN`/`BOWERBIRD_KEYRING_BACKEND`/`BOWERBIRD_DATA_DIR`, then `token::load_or_generate()` in-process (7420 ff). The `mock_*` tests install `keyring::set_default_credential_builder` which **has no inverse** (module doc 7345-7351). | **RISKY** — the only genuinely unsafe tests in the workspace |
| `sigterm_/sigint_uses_graceful_shutdown_*` (6869, 6874) | subprocess | `nix::kill(child_pid, SIGTERM/SIGINT)` to the spawned real daemon binary | Safe — signal + handler live in the child process |
| `second_daemon_exits_nonzero_when_first_holds_lock`, `singleton_releases_lock_on_{clean,sigkill}_exit` (7133, 7168, 7195) | subprocess | flock(2) on `<TempDir>/.bowerbird/bowerbird.pid` | Safe — lock path is per-test TempDir; contention is intentional *within* the test |
| `state_plus_event_atomicity_under_sigkill_during_load` (2091) | subprocess | SIGKILL to child; (historical: SQLite-teardown deadlock, fixed via drop ordering 2359-2363) | Safe (the historical deadlock was intra-test, fixed in Epic 4) |
| `shim_*_round_trip` (1064, 1113) | mixed | in-process ingest socket on `<TempDir>/ingest.sock`; real shim child via `Command::env` | Safe — TempDir path, per-child env |
| ~153 remaining in-process tests | in_process | DB at `<TempDir>/bower.db`; HTTP/WS on `TcpListener::bind("127.0.0.1:0")` (ephemeral, spawn_test_daemon @ 3137) | Safe — TempDir + ephemeral ports |

No `set_global_default`/`try_init` anywhere in the file → tracing is not a cause. `static TOKEN_SEQ: AtomicU64` (4081) is a race-free monotonic counter, safe.

### Workspace `tests/cli_*.rs` (14 files) + adapter/shim/protocol tests

| Resource class | Finding | Parallel-safe? |
| --- | --- | --- |
| Env vars (`HOME`, `BOWERBIRD_DATA_DIR`, `BOWERBIRD_TOKEN`, `BOWERBIRD_KEYRING_BACKEND`, `BOWERBIRD_DAEMON_BIN`) | **100% via `Command::env(...)` / `.env(...)` per-child** (e.g. `cli_install.rs:53`, `cli_auth.rs:39-40`). **Zero `std::env::set_var`/`remove_var` in the entire workspace test tree.** | Safe |
| Keyring | Always `cmd.env("BOWERBIRD_KEYRING_BACKEND", "disable")` per-child (`cli_auth.rs:40` etc.). No `set_default_credential_builder` outside `story_3_3_auth`. | Safe |
| Data dirs / sockets / ports | Per-test `TempDir`; ephemeral ports | Safe |

**Conclusion of the map: the workspace-wide flag is serializing hundreds of already-isolated tests to protect 9 tests in one module.**

## Confirmed Findings

### Finding 1: The stated rationale is one folklore narrative quoted three times, never diagnosed

**Evidence:** `.github/workflows/ci.yml:31-41`; `architecture.md` "Contract-test serialization (operational note)"; both downstream of `epic-2-retro-2026-05-24.md:176-180` (Discovery #3) and `:200` (AI-3).

**Detail:** AI-3's charter is explicitly *"Document the [...] requirement"* — not diagnose it. The retro calls the hang *"a pre-existing known mode"* that *"lives in the heads of the authors"* (`epic-2-retro:110-112`). Stories 3.1/3.2/3.3 each *cite AI-3* rather than observe anything new. Only Story 2.5 has a contemporaneous symptom, and it's a **stop**, not a diagnosed hang: the author *"stopped after daemon contract tests hung"* (`2-5:176,180`). Story 1.6, credited in the comment, says the opposite: *"implementation landed without unexpected failures"* (`1-6:519`). The flag is now frozen by a string-matching drift test (`tests/release_pipeline_docs.rs::ci_workflow_runs_workspace_tests_single_threaded`) that asserts the *text* is present — institutionalizing folklore without ever validating it.

### Finding 2: The real, code-confirmed cause is process-global `std::env` mutation + an irreversible keyring mock backend in one module

**Evidence:** `crates/daemon/tests/contract_daemon.rs:7340-7602`. Module doc-comment (7342-7351), verbatim:

> *"**Run requirement:** `--test-threads=1`. Tests mutate process-global state (env vars + the keyring credential-builder). Parallel execution would race and flake. **Mock state caveat:** `keyring::set_default_credential_builder` has no inverse — once a mock is installed in the process, it stays installed."*

`EnvGuard` (7364-7390) restores env on Drop but does **not** serialize concurrent access — it prevents leakage between *sequential* tests, not races between *parallel* ones. This produces **flakes/wrong-value races, not hangs.**

### Finding 3: The only deadlock ever root-caused was unrelated to parallelism and was fixed

**Evidence:** `epic-3-retro-2026-05-25.md:121,189` — *"the deadlock is in test teardown, not production code; the symptom is `tokio::runtime` shutdown ordering against the `deadpool-sqlite` pool's `close()` call."* It occurred **under serial execution** (`3-2:421` had to *both* `--test-threads=1` *and* `--skip` it). Fixed in `epic-4-retro-2026-05-25.md:202` via *"explicit drop ordering"*, applied as *"defense-in-depth"* because it *"was already passing cleanly... 5 consecutive runs."* So the flag the CI comment credits with curing hangs did not cure this one, and by Epic 4 it could not be reproduced.

## Deduced Conclusions

### Deduction 1: The flag's scope is ~18x too broad

**Based on:** the Test→Resource map + Finding 2.

**Reasoning:** `--test-threads=1` serializes every test in every crate in the workspace. The map shows exactly one module (9 tests in `contract_daemon.rs::story_3_3_auth`) is unsafe under parallelism. Everything else — the other ~156 daemon tests, all 14 `cli_*.rs` suites, adapter/shim/protocol tests — is already isolated (per-child env, TempDir, ephemeral ports).

**Conclusion:** The correct serialization boundary is one module, not the workspace. The flag pays ~60-90s/run (architecture.md) to protect 9 tests.

## Hypothesized Paths

### Hypothesis 1 (H1): In-process daemons race on process-global signal-handler registration

**Status:** **Refuted.**

**Would confirm:** signal registration reachable from in-process `#[tokio::test]` paths, last-writer-wins.

**Resolution:** Signal registration (`next_signal()`) lives in `crates/daemon/src/main.rs:419-441` — the **binary** entry path. In-process tests call `bowerbird_daemon::` lib functions + `AppState` directly and never invoke `main()`, so they never register handlers. The signal tests (`story_2_5`, `story_3_1`) spawn the real daemon **binary as a child** and `nix::kill` the **child PID**; the handler lives in the child process, not the test process. No cross-test handler race exists. (Secondary: tokio's `signal()` uses a refcounted broadcast registry, not last-writer-wins, so even concurrent in-process registration would not hang — but it's moot.)

### Hypothesis 2 (H2): The daemon's PID-file + flock singleton collides on a fixed path

**Status:** **Refuted.**

**Would confirm:** `singleton::acquire` locking a fixed/constant path shared across tests.

**Resolution:** `crates/daemon/src/singleton.rs:76-77` — `acquire(data_dir)` locks `data_dir.join("bowerbird.pid")`. `data_dir` is `BOWERBIRD_DATA_DIR`, set per-child to a per-test `TempDir` everywhere (`cli_install.rs:268`, contract `story_3_1` @ 7059 ff). Two parallel tests use different directories → no contention. The lock contention inside `second_daemon_exits_nonzero_when_first_holds_lock` is intentional and internal (two children of the *same* test sharing that test's TempDir).

### Hypothesis 3 (H3): Cargo-cult — isolation is already sufficient, delete the flag outright

**Status:** **Partially confirmed (and the "delete outright" action is Refuted).**

**Resolution:** The *stated justification* is cargo-cult (Finding 1) and ~99% of the suite is genuinely isolated (Deduction 1). But H3's prescription — delete `--test-threads=1` with no other change — would let `story_3_3_auth`'s 9 env-mutating tests race and flake (Finding 2). So isolation is *almost* sufficient: the flag is removable only *after* those 9 tests are de-globalized.

### Hypothesis 4 (H4): A fourth cause — process-global env + irreversible keyring backend

**Status:** **Confirmed.** This is the actual cause. See Finding 2. Class: shared process-global state (env table + `keyring` credential-builder singleton). Symptom: races/flakes under parallelism, not deadlock-hangs. Note the research doc's observation that **nextest's process-per-test would neutralize this entire class for free** (each test in its own process → `set_var` cannot leak, and the irreversible mock backend cannot poison a sibling).

## Missing Evidence

| Gap | Impact | How to Obtain |
| --- | ------ | ------------- |
| A reproduction of the *parallel* hang with a stack trace | Would upgrade "flake-not-hang" from Deduced to Confirmed | Run the daemon contract suite without the flag on a multi-core CI runner; the prediction is *flaky env-test failures*, not a hang |
| Confirmation that `git log` on `ci.yml` shows add-and-forget (no removal experiment) | Closes the cargo-cult provenance | `git log -p .github/workflows/ci.yml` |

## Source Code Trace

| Element | Detail |
| --- | --- |
| Cause origin | `crates/daemon/tests/contract_daemon.rs:7416-7602` (`mod story_3_3_auth`, 9 in-process tests) |
| Mechanism | `std::env::set_var`/`remove_var` (process-global) + `keyring::set_default_credential_builder` (irreversible process-global) |
| Trigger | Running those tests concurrently with any other test that reads `BOWERBIRD_*` env or the keyring |
| Why it looks isolated | `EnvGuard` (7364-7390) restores env on Drop — hides leakage *between sequential tests*, not *between parallel ones* |
| Red-herring #1 | Signal handlers — binary-only (`main.rs:419-441`), never in-process |
| Red-herring #2 | flock singleton — per-TempDir path (`singleton.rs:76-77`) |
| Red-herring #3 | The Epic 3 SQLite-teardown deadlock — intra-test drop-order bug, fixed Epic 4, orthogonal to parallelism |

## Conclusion

**Confidence: High.** Confirmed root cause with a deterministic, code-documented mechanism; the test module's own doc-comment names the requirement and the reason.

`--test-threads=1` is **partially justified but massively over-scoped, and its documented rationale is wrong.** Exactly one module — `contract_daemon.rs::story_3_3_auth` (9 in-process tests) — is unsafe under parallelism, because it mutates process-global `std::env` and installs an irreversible process-global keyring mock backend. The CI comment's four named culprits (subprocesses, signal handlers, `BOWERBIRD_DATA_DIR` fixtures, keychain backends) are folklore propagated across four epics without diagnosis: signal handlers are binary-only (H1 refuted), the flock path is per-TempDir (H2 refuted), and the only real deadlock ever found was an unrelated SQLite-teardown drop-order bug fixed in Epic 4. The flag is not pure cargo-cult (H3) — a narrow real need exists — so it cannot simply be deleted; the 9 tests must be de-globalized first.

## Recommended Next Steps

### Fix direction — partition per research §3

- **Group A (parallel-safe, everything else):** all ~156 other daemon tests + every workspace `tests/*.rs` + adapter/shim/protocol. Already isolated. Should run fully parallel.
- **Group B (the only serialization-requiring set):** `contract_daemon.rs::story_3_3_auth`'s 9 in-process env/keyring tests.

Two ways to neutralize Group B, then drop the workspace flag:

1. **No-new-dep refactor (recommended smallest safe first step).** Convert the 9 in-process `#[test]`s to the **subprocess pattern the rest of `contract_daemon.rs` and all of `cli_*.rs` already use**: spawn `bowerbird`/`bowerbird-daemon` via `assert_cmd` with per-child `.env(...)`, asserting on exit code / output instead of calling `token::load_or_generate()` in-process. This removes the *only* `std::env::set_var` in the workspace and the in-process keyring-mock install. Dev-deps stay `tempfile` + `assert_cmd` (the team's stated constraint). Then delete `-- --test-threads=1` and update the CI comment + `architecture.md` + the `release_pipeline_docs.rs` drift test. Net: ~60-90s/run back, folklore replaced with a true (now-empty) cause.

2. **Strategic alternative: adopt cargo-nextest.** Its process-per-test default neutralizes the entire H1/H4 shared-process-global class for free (each test is its own process), and a named serial group can pin the `mock_*` ordering if kept in-process. Costs a toolchain dep and CI wiring; buys isolation-by-construction going forward. The research doc explicitly flags this as the "for free" option. Worth a follow-up ADR, not the first step.

**Do NOT** apply a `.cargo/config.toml [alias]` to serialize — the team correctly rejected it because it also serializes `cargo build` (architecture.md).

### Smallest safe first step

Refactor option 1 on the 9 `story_3_3_auth` tests (de-globalize via the existing subprocess pattern). It is self-contained, needs no new dependency, makes the whole suite parallel-safe, and is the precondition for removing the flag. Removing the flag *before* this step would reintroduce real flakes.

### Diagnostic (to upgrade the one Deduced point to Confirmed)

Run `cargo test -p bowerbird-daemon` (no `--test-threads=1`) on a multi-core runner. Prediction: intermittent *wrong-value assertion failures* in `story_3_3_auth` (e.g. a `disable_*` test seeing a `mock` backend, or an env value from a sibling), **not** a hang. Observing that confirms "flake, not hang" and retires the last folklore claim.

## Reproduction Plan

1. Checkout current `isolation-audit`.
2. `cargo test -p bowerbird-daemon -- --test-threads=8` (force parallelism).
3. Expect: `story_3_3_auth` tests flake intermittently (env/keyring cross-talk). The other ~156 daemon tests and all `cli_*.rs` suites pass reliably in parallel.
4. Re-run 3-5x to surface the race (it is timing-dependent on which test's `set_var` lands first).

## Side Findings

- The team deliberately avoided a `.cargo/config.toml [alias]` because it would also serialize `cargo build` (architecture.md). A real constraint on any "make it the default" fix.
- The `release_pipeline_docs.rs::ci_workflow_runs_workspace_tests_single_threaded` test asserts the *string* `--test-threads=1` is present in `ci.yml`. It locks in the flag (and its folklore comment) — it must be updated in the same PR that removes the flag, or it will fail the build. (Confirmed: `tests/release_pipeline_docs.rs`.)
- `init_tracing` (`crates/daemon/src/lib.rs:160-198`) uses non-panicking `try_init()` and is not called by any test — so even repeated subscriber init is a no-op, not a serialization hazard.
- The historical SQLite-teardown deadlock fix (explicit `drop(reader) → drop(pools) → drop(tmp)` with `yield_now()`) lives at `contract_daemon.rs:2359-2363` as defense-in-depth; worth keeping regardless of the flag decision.

---

## Follow-up: 2026-06-03

New scope, two distinct symptoms surfaced during Story 5.8 pass-4 (the broadcast-lag snapshot-coverage fix in `crates/daemon/src/api/ws.rs`). Case prep already existed at `docs/research/test-isolation-bowerbird-findings.md` (captured 2026-06-03); this block root-causes both. The first concluded section above answered a *different* question (why the workspace-wide `--test-threads=1` flag exists); these two symptoms are new and are NOT the `story_3_3_auth` env/keyring cause.

**Naming reconciliation:** the intake brief called Symptom B "the F1 e2e," but `F1` is the Story 5.8 *pass-2* finding (`widening_filter_resends_only_uncovered_rows`, `contract_daemon.rs:5717`). The test that actually flakes under `--workspace` is the Story 5.8 *pass-4* test `story_2_4_dropped::lag_invalidates_snapshot_coverage_resubscribe_resnapshots` (matches the brief's "3/3, 9s→14s, daemon emits frame, client misses deadline" exactly). Subject of Symptom B below = the pass-4 lag test.

### Hand-off Brief (follow-up)

1. **Symptom A — the intermittent hang is a SQLite connection-close teardown deadlock, not signal handlers or a daemon spawn.** `story_1_7_rest::status_returns_none_last_event_when_only_sentinels` (`contract_daemon.rs:3377-3391`) is a pure in-process `app.oneshot("/status")` test — no WS, no signal handlers, no child daemon. It writes via the writer pool and reads via the reader pool, then drops `app`/`pools`/`_tmp` at scope exit **without** the explicit `drop → yield_now` ordering that the codebase already uses elsewhere to avoid the documented `sqlite3_close → pthread_mutex_wait` teardown deadlock (`contract_daemon.rs:2194-2209`). Race-y (~1-in-5), process-local, deterministic mechanism. Confidence: Medium-High (documentary; one live backtrace would seal it).
2. **Symptom B — the e2e flake is wall-clock fragility under host load, not a logic bug.** `lag_invalidates_snapshot_coverage_resubscribe_resnapshots` asserts on observing a single WS snapshot frame over a real localhost TCP socket within a 5s deadline, in a connection deliberately stressed into broadcast-lag. The daemon provably emits the frame (file-trace confirmed in the research doc); `cargo test --workspace` runs other crates' test binaries concurrently with `contract_daemon` (cargo parallelizes *across* test binaries — `--test-threads=1` only serializes *within* one), raising the daemon binary's wall time ~9s→~14s and pushing frame delivery past the deadline. Confidence: High that it's load/timing.
3. **What's needed next.** A: apply the existing `drop(reader)/drop(writer) → drop(pools) → drop(tmp)` + `yield_now()` teardown to the status test (and audit sibling in-process pool tests lacking it); optionally capture one hung backtrace to upgrade to High. B: add a testability seam around `connection_task`/`snapshotted_keys` so the re-snapshot is observed deterministically instead of racing a real-socket deadline; nextest serialized test-group is the cheaper interim that reduces (not eliminates) the contention.

### Evidence Inventory (follow-up)

| Source | Status | Notes |
| ------ | ------ | ----- |
| `docs/research/test-isolation-bowerbird-findings.md` | Available | Stronghold/case-prep. Repro commands for A and B; established B's daemon-emits-frame fact and that flavor/read-strategy are not the cause. |
| `crates/daemon/tests/contract_daemon.rs:3377-3391` | Available | Symptom A test. In-process `oneshot("/status")`; writer-pool write + reader-pool read; **no teardown drop/yield ordering**. |
| `crates/daemon/tests/contract_daemon.rs:2194-2209` | Available | Documents the exact deadlock: `sqlite3_close → sqlite3_mutex_enter → pthread_mutex_wait` in TempDir teardown; fix = explicit `drop(reader)→drop(pools)→drop(tmp)` + `yield_now()` (Story 4.4 AC#3a, Epic 3 retro AI-2). |
| `crates/daemon/tests/contract_daemon.rs:216, 2478-2481, 8261, 8313` | Available | The teardown-ordering fix is applied here — pattern exists in-tree, just not on the status test. |
| `crates/daemon/src/db/pool.rs:21-71` | Available | `init_pools`: writer `max_size=1`, reader `max_size=4`, `Runtime::Tokio1`, 5s pool-wait, post_create/post_recycle PRAGMA hooks via `interact`. Confirms 5s-bounded waits → a >60s hang is a true deadlock, not a timeout. |
| `crates/daemon/src/api/status.rs:16-49` | Available | `/status`: `reader.get().await` then `interact()` (deadpool-sync spawn-blocking). The DB work that creates the reader-pool connection whose close races teardown. |
| `crates/daemon/src/api/events.rs` (deny_unknown_fields 400 path) | Partial | Why `events_endpoint_rejects_unknown_query_param` (sibling at 3393) does NOT hang: the 400 is raised at the extractor before any pool checkout, so no deadpool connection is created → no close-mutex teardown race. (Deduced; worth a 1-line confirm.) |
| deadpool-sqlite 0.13.0 / deadpool-sync 0.2.0 / deadpool 0.13.0 (`Cargo.lock`) | Available | Pinned versions whose `interact`/close-on-background-thread behavior is the deadlock substrate. |

### Confirmed Findings (follow-up)

#### Finding A1: The status hang's documented siblings prove the mechanism; the status test simply lacks the guard

**Evidence:** `contract_daemon.rs:2194-2209` names the deadlock (`sqlite3_close → pthread_mutex_wait` in TempDir teardown) and its fix; the fix is present at lines 216 / 2478-2481 / 8261 / 8313 but absent at the status test (3377-3391). `db/pool.rs:19,40-44` caps every pool wait at 5s, so the observed >60s hang (research doc repro: `timeout 60 … → 1 of 5 times out`) cannot be a pool/busy timeout — it is an unbounded wait, i.e. a mutex deadlock, consistent with the documented `pthread_mutex_wait`.

#### Finding A2: Symptom A is independent of WS / Story 5.8 and of the first investigation's cause

**Evidence:** the status test imports no WS/broadcast types, spawns no child (`oneshot` against an in-process `api::router`), and registers no signal handler (those are binary-only, `main.rs`, per H1 refutation in the concluded section). It also touches no `std::env`/keyring (the `story_3_3_auth` cause). So A is orthogonal to both Story 5.8 and the `--test-threads=1` rationale — it is purely a per-test teardown defect.

#### Finding B1: cargo parallelizes across test binaries; `--test-threads=1` does not prevent it

**Evidence:** research doc §"Symptom B" measures contract_daemon at ~9s alone vs ~14s under `cargo test --workspace -- --test-threads=1`. `--test-threads=1` is a libtest (within-binary) flag; cargo still schedules each crate's test binary concurrently. The wall-time delta is the other crates' binaries (protocol/shim/adapter/cli) competing for CPU. This is the "extra system load" the research doc hypothesized — confirmed by the flag's semantics.

#### Finding B2: The B failure is in the assertion's timing model, not the daemon

**Evidence:** research doc: file-trace of `api/ws.rs` shows the daemon clears `snapshotted_keys`, re-snapshots `sess-A`, and calls `socket.send` on the post-lag re-subscribe; "the failure is purely that the client side never observes the frame within the deadline." Flavor-invariant and read-strategy-invariant (both tried). So the regression test encodes a wall-clock race against real-socket delivery; under added load the 5s deadline loses.

### Hypotheses (follow-up)

#### HA (Symptom A): in-process SQLite connection-close teardown deadlock

**Status: Confirmed (Medium-High).** Mechanism documented in-tree (2194-2209), guard demonstrably present on siblings and absent here, timeout math rules out a bounded wait. **To refute/seal:** capture one hung backtrace via `sample`/`lldb` during the research-doc repro and confirm a `sqlite3_close`/`pthread_mutex` frame on a deadpool background thread → upgrades to High. **Refutation attempt:** considered "pool-wait timeout" and "busy_timeout" — both refuted by the 5s caps in `pool.rs` vs the >60s observed hang.

#### HA-alt (refuted): signal-handler registration / daemon-spawn-that-doesn't-come-up

**Status: Refuted.** The research doc lead #4 guessed these. The status test neither spawns the daemon binary nor calls `main()` (signal handlers are binary-only), and uses `oneshot` (no TCP listener to fail to bind). Neither guess can apply.

#### HB (Symptom B): wall-clock-deadline assertion over a real socket, perturbed by cross-binary CPU contention

**Status: Confirmed (High).** Findings B1+B2. **Note:** nextest serialized groups / `--jobs`-capped runs *reduce* contention but do not remove the wall-clock fragility — only decoupling the assertion from real-socket delivery does (the durable fix).

### Source Code Trace (follow-up)

| Element | Symptom A | Symptom B |
| --- | --- | --- |
| Defect site | `contract_daemon.rs:3377-3391` (missing teardown ordering) | `lag_invalidates_snapshot_coverage_resubscribe_resnapshots` (5s real-socket deadline after a forced lag burst) |
| Mechanism | `sqlite3_close → sqlite3_mutex_enter → pthread_mutex_wait` race between deadpool connection close and TempDir/runtime teardown | host-load-perturbed WS frame delivery missing a wall-clock deadline |
| Trigger | scope-exit drop with both pools' connections live, no `yield_now` tick | `cargo test --workspace` running sibling test binaries concurrently (~9s→~14s) |
| Why intermittent | timing race on the close mutex (~1/5) | depends on how much concurrent load the box carries during the deadline window |
| Not the cause | signal handlers (binary-only), daemon spawn (none), env/keyring (`story_3_3_auth` only) | runtime flavor, read strategy (both eliminated by experiment); the daemon logic (frame is emitted) |

### Fix Direction (follow-up)

**Symptom A (small, deterministic).** Give the status test the same teardown the codebase already uses: bind the pools to a local, then at end-of-body `drop(reader_conn if any); drop(state/app); drop(pools); yield_now().await; drop(tmp); yield_now().await;` (mirror lines 2478-2481). Then audit the other in-process `fresh_pools`+`oneshot` tests for the same missing guard. Higher-value than B because it intermittently hangs the whole serialized CI run. A systemic option worth a follow-up: a teardown helper or an `init_pools`/Drop-side fix so individual tests can't forget the ordering.

**Symptom B (testability seam).** Add a deterministic observation point around `connection_task` so the post-lag re-snapshot can be asserted on the per-connection coverage set (`snapshotted_keys`) via a test hook, rather than racing a real-socket 5s deadline. Interim, load-reducing-only step: move the heavyweight WS/daemon contract tests into a cargo-nextest serialized test-group (`max-threads = 1`) — this is the same nextest direction the concluded section already recommended for the `story_3_3_auth` class, so both threads converge on adopting nextest.

### Diagnostic / Reproduction (follow-up)

- **A (seal to High):** loop the research-doc repro (`contract_daemon.rs` built; in-process test spawns no children, so no zombie-daemon risk), and on a hang, `sample <pid>` / `lldb -p <pid> -o 'thread backtrace all'` to capture the `sqlite3_close`/`pthread_mutex` frame. Expected: a deadpool background thread parked in `pthread_mutex_wait` inside SQLite close, and the runtime drop blocked on it.
- **B (confirm load hypothesis cheaply):** run `cargo nextest run -p bowerbird-daemon` alone vs a `--workspace` nextest run with the WS contract tests in a `max-threads=1` group; expect the flake to shrink as concurrency drops, while only the seam fix makes it disappear.

### Status (follow-up): Active — A diagnosed (Medium-High, backtrace pending), B diagnosed (High); both with concrete fix directions. No code changed this pass (investigation scope).

## Follow-up: 2026-06-03 #2 — Symptom A reproduction attempt (during the quick-dev fix)

Attempting to *fix* Symptom A (the teardown guard, spec `spec-status-test-teardown-deadlock.md`) surfaced evidence that **revises the Symptom A diagnosis**:

- **Could not reproduce the hang on a quiet machine.** The original *unfixed* racy drop (replicated exactly: `fresh_pools` → write sentinel → `ready_state(pools)` → `oneshot("/status")`, no teardown) ran **50/50 clean** — 30× direct test-binary + 20× `cargo test --exact`. Zero hangs.
- **The hang correlates with concurrent worktree load, not pure in-isolation timing.** Every observation of the hang (the original "~1-in-5", and the one cargo-level stall seen during this session) coincided with *another* session concurrently running full `contract_daemon`/`--workspace` suites in the same worktree (confirmed: a second `CLAUDE_SESSION_ID` + a background full-suite run + a spawned daemon, contending for CPU and the cargo lock). When that load was absent, neither the fixed nor the unfixed test hung.
- **Implication:** Symptom A is likely the **same trigger profile as Symptom B** (load-/scheduler-sensitive), not a distinct in-isolation deadlock. The research-doc "~1/5 in isolation" was captured *during* Story 5.8 pass-4 work, which plausibly carried its own background load. This does **not** refute the documented `sqlite3_close → pthread_mutex_wait` mechanism (it is real, and the canonical fix at `contract_daemon.rs:2471` exists) — it reclassifies it as a **rare, load-amplified** race rather than a ~20%-in-isolation one.

**Correction to the prior follow-up's confidence:** the "65/65 clean after the fix" figure recorded informally during the fix is **not** evidence the fix works — the unfixed control also passes on a quiet machine. Symptom A's fix is therefore *unproven* (bug not reproducible on demand), shipped only as defense-in-depth consistent with the canonical pattern.

**Corrected scope of the at-risk set (reliable parser-based audit):** the deadlock class is **not** "in-process oneshot tests" — it is **every test that calls `fresh_pools()`** (each opens a migration writer connection). That is **79 in-process** `fresh_pools` tests (the quick-dev guarded 21) **+ 63 real-server** `fresh_pools` tests. Per-test enumeration proved unreliable (helper indirection: `seed()`, `list_ids()`); the only robust identification is "calls `fresh_pools`".

**Open diagnostic to settle it:** reproduce Symptom A *under controlled load* (status test in a loop while a `--workspace` build/test or CPU stressor runs). If the unfixed control hangs under load and the leak/teardown variants don't, that both confirms the mechanism and validates a fix. Until then, A and B should likely be treated as one load-sensitivity problem with a shared fix (nextest binary-concurrency control / testability seam), per the §"Leads" in `docs/research/test-isolation-bowerbird-findings.md`.

## Follow-up: 2026-07-28 — Root cause found and fixed (SQLite 3.51.1 close deadlock)

**The hang is diagnosed, confirmed from source and a live specimen, and
fixed.** Symptom A's mechanism (sqlite3_close racing teardown) was right;
its framing (a per-test missing-teardown problem) was not.

**Root cause:** SQLite 3.51.1 — the exact version bundled by
libsqlite3-sys 0.36.0, and the only affected release — has a lock-order
inversion in the unix VFS, introduced in 3.51.1 and fixed upstream in
3.51.2 (SQLite forum: "TSAN: lock-order-inversion since 3.51.1", reported
and fixed 2025-12-05). When two connections to the same WAL database are
closed concurrently:

- the close that can delete the WAL runs `sqlite3WalClose ->
  sqlite3OsLock(EXCLUSIVE) -> unixLock`, which takes the per-inode
  `pLockMutex` (sqlite3.c:41142) and then, still holding it, calls
  `unixIsSharingShmNode` -> `unixEnterMutex` (global VFS mutex) — the
  order sqlite3.c's own comment (lines 40088-40098) marks `ERROR`;
- the other close runs `unixClose`, which correctly takes the global VFS
  mutex first (41545) and then wants `pLockMutex` (41551).

Textbook ABBA deadlock. Both blocking threads park in
`__psynch_mutexwait` forever; the test (or daemon) thread then hangs in
`Runtime` drop -> `BlockingPool::shutdown` waiting for them.

**The live specimen:** an orphaned `contract_daemon` binary (pid 64520,
parented to launchd, hung ~90+ minutes in
`story_5_8_session_filter::sessions_state_filter_multi_drops_ended`) was
found and sampled before killing. The `sample` backtrace shows exactly
the two threads above, both inside `sqlite3_close` on the same
`bower.db` inode (lsof-confirmed), plus the test thread parked in
blocking-pool shutdown. Evidence preserved in
`scratch/hang-hunt-evidence/` (orphan-sample-64520.txt,
orphan-lsof-64520.txt).

**Why the drop-ordering guards never fully worked:** deadpool's
connection drops are fire-and-forget — `deadpool_sync::SyncWrapper::drop`
spawns the real `sqlite3_close` onto a background blocking thread and
returns immediately (deadpool-sync 0.2.0 src/lib.rs:162-176), and
`Pool::close()`/`drop(pools)` pops idle connections in a tight loop.
`DbPools` holds writer(max 1) + reader(max 4) pools on one file, so any
teardown with >=2 idle connections fires concurrent closes no matter how
drops are ordered; `drop(pools)` is itself the trigger. That made the
exposure 130 of 151 `fresh_pools()` tests, and also the daemon's
graceful shutdown (`main.rs` `reader.close(); writer.close();`) — a
latent prod hang, not just a test flake.

**Why it only showed under concurrent-worktree load:** the window is a
few instructions wide inside `unixClose`. 44 instrumented runs under
saturating synthetic CPU load (10x `yes` + a scratch cargo-build loop)
all passed; a concurrent cargo test's scheduler churn is simply a much
better source of the fine-grained jitter needed. Absence under synthetic
load was never evidence of absence.

**Fix:** vendored libsqlite3-sys 0.36.0 with the 3.51.3 amalgamation
swapped in (bindings identical except version constants), wired via
`[patch.crates-io]` — see `vendor/libsqlite3-sys/README-VENDORED.md`. A
plain dependency bump is blocked: fixed SQLite needs libsqlite3-sys
>=0.37 -> rusqlite >=0.39, and deadpool-sqlite (0.13.0, latest) pins
rusqlite ^0.38 (tracking:
https://github.com/deadpool-rs/deadpool/issues/490). Remove the vendor
patch when that lands. Full workspace suite (630 tests) passes on
3.51.3.

**Status of the symptoms:** Symptom A is resolved (root cause was never
specific to the status-sentinel test). Symptom B (the WS delivery
deadline flake in `lag_invalidates_snapshot_coverage_resubscribe_resnapshots`)
is unrelated to this deadlock and remains open. The `scripts/test.sh`
timeout+`sample` diagnostics added the same day (commit 4f7ca57) remain
as the safety net for any future hang class.

## Follow-up: 2026-07-28 #2: Symptom B re-examined (B1 refuted; flake not reproducible on current hardware)

Re-opened the Symptom B thread after the SQLite fix landed. Two findings,
one refuting a prior one, one closing the symptom.

**Finding B1 is wrong: cargo does NOT run test binaries concurrently.**
`cargo test` builds in parallel but executes test targets one at a time;
`--test-threads=1` then serializes within each binary, so a
`cargo test --workspace -- --test-threads=1` run is fully serial end to
end. Two lines of evidence from this worktree's own logs
(`target/test-logs/`): every run's output is strictly sequential per
binary (test output is unbuffered by cargo, so concurrent binaries would
interleave), and `contract_daemon` completes in 9.00s inside full
workspace runs, identical to its "binary alone" timing. The June
measurement of ~14s under `--workspace` was not cross-binary contention;
there is no cross-binary execution to contend with.

**What actually made B fail 3/3 in June:** the same confounder the
2026-06-03 #2 follow-up documented for Symptom A. The June B evidence was
captured on older, slower hardware, during Story 5.8 pass-4 work that
carried a second concurrent session running builds/suites in the same
worktree. "Workspace vs alone" was a proxy for "loaded box vs quiet box":
the longer workspace run simply spent more wall time overlapping the
external load window.

**Reproduction attempts on current hardware (2026-07-28), all negative:**

- 3/3 full `scripts/test.sh` workspace runs green, plus the prior
  evening's run: 4/4, lag test passing in each.
- 20/20 green running the lag test in a loop via the built
  `contract_daemon` binary while 20 `yes` processes saturated all 10
  cores (2x oversubscription). Under that load the test still completes
  in **0.05s** against its 5s observation deadline, roughly 100x
  headroom. The helper deadlines (`wait_subscribe_live`, 2s) were never
  approached either.

**Implications:**

- The interim "nextest serialized test-group" recommendation for B is
  moot: it presumed cross-binary concurrency that does not exist. (nextest
  remains interesting for other reasons, e.g. per-test process isolation,
  but it would *add* parallelism, not remove it.)
- The testability seam (assert the re-snapshot on `snapshotted_keys` via
  a test hook instead of racing a real-socket deadline) remains the
  durable fix *if the flake ever resurfaces*, e.g. on a slow CI runner.
  Not built now: the project is pre-MVP, the test has ~100x deadline
  headroom on current hardware, and the seam would add daemon code that
  exists only for one test's observability. Sketch stays in
  `docs/research/test-isolation-bowerbird-findings.md` §Leads #3.
- The test's "KNOWN FLAKE under `cargo test --workspace`" doc-comment was
  corrected the same day to stop attributing the flake to workspace runs.

**Status: Symptom B closed** as a misattributed-trigger, hardware-bound
flake: the assertion's wall-clock timing model is real but has two orders
of magnitude of headroom on current hardware, and the historical failures
tracked concurrent-worktree load that `scripts/test.sh` now locks out.
Re-open via the seam sketch if it fires again (the failure signature is
the `re-subscribe after lag MUST re-snapshot sess-A` assertion at a 5s
deadline).

## Follow-up 2026-07-29 #3: parallel-by-default flip

With both symptoms closed, the workspace suite went parallel-by-default
(`scripts/test.sh` and CI now run `cargo test --workspace` with libtest's
default thread count; commit `2be689a`). Enabling work:

- `story_3_3_auth` stopped mutating process env: `token::load_or_generate`
  gained a `TokenEnv` snapshot seam (`33313a5`), removing the last
  `--test-threads=1` dependency.
- Parallel soaks at 16 threads on 10 cores surfaced exactly one failure in
  ~12 pre-fix full-workspace runs — not a deadlock but a startup race:
  SIGTERM landing between the "daemon listening" readiness marker and the
  lazy signal-handler registration killed the daemon via default
  disposition (`unix_wait_status(15)`). Fixed by registering the signal
  streams synchronously before any readiness marker (`a61b0a6`).
- Post-fix evidence: 18 consecutive 16-thread contract-suite runs plus 10
  mixed default/16-thread full-workspace runs, all green (see final-soak
  numbers in the session that landed the flip).

The `release_pipeline_docs.rs` AC #6 gate now points the other way: it
fails if `--test-threads=1` reappears in an effective (non-comment) line
of `ci.yml`. The serialized-invocation lock in `scripts/test.sh` is
unchanged — two concurrent `cargo test` *processes* in one worktree remain
the confirmed hang trigger.
