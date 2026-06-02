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
