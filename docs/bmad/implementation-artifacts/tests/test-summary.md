# Test Automation Summary — Story 3.3

Generated 2026-05-25 via `bmad-qa-generate-e2e-tests`. Supersedes the Story 3.2 summary and its 3.3 addendum.

## Gap Analysis

Story 3.3 (bearer-token auth with keychain storage) landed with substantial test coverage from the dev-story run:

- `crates/daemon/tests/contract_daemon.rs::story_3_3_auth` (10 tests) — resolver-chain unit/contract coverage for every branch (env wins, keychain disabled→env, keychain disabled→config.toml, all paths exhausted, wrong-mode warns, unknown-field rejected, empty-token === missing, mock-keychain generate, mock+env wins, daemon-subprocess exit-code on chain exhaustion).
- `tests/cli_auth.rs` (7 tests) — CLI E2E for `bowerbird auth token` covering env / config.toml / failure / piped-stderr / clap help / wide-mode warning / `bowerbird status` full-block.
- `crates/daemon/src/api/token.rs::tests` (6 unit tests in-module) — `BearerToken::verify` semantics + `TokenError::Display` shape.
- `crates/daemon/src/config_file.rs::tests` (6 unit tests in-module) — `read()` happy/missing/parse-error/unknown-field, `check_mode()` 0600/wider.
- `src/commands/auth.rs::tests` (2 unit tests in-module) — `TokenSource::Display` user-facing shape + `TokenError::Display`.

Three gaps remained after that baseline:

**Gap A — NFR14 "token never reloaded at runtime without a restart" had no E2E proof.** The dev-story dropped Task 6.9 (`keychain_value_preserved_across_two_load_or_generate_calls`) because the keyring v3 mock builder produces a fresh `MockCredential` per `Entry::new(service, user)` call — it has no service+user interning, so a write-then-read round-trip cannot be exercised against the mock. The Completion Notes claim NFR14 is "exercised by `status_shows_full_block_without_user_supplied_token`," but that test only proves the daemon and CLI resolve to the *same* config.toml value at startup — it does NOT prove the daemon ignores a *later* mutation to config.toml. Without an E2E rotation test, a future change that adds e.g. `tokio::time::interval(...).then(reread_config_toml)` would silently regress NFR14: every existing 3.3 test still passes because none of them touch config.toml after the daemon starts.

**Gap B — Task 7.4 (daemon→CLI direct token round-trip) was never landed.** The story explicitly scoped a test asserting "the daemon's authoritative token and the CLI's emitted token agree." The existing `status_shows_full_block_without_user_supplied_token` test proves the daemon's `/status` handler accepts the CLI's resolved bearer — but it doesn't prove that `bowerbird auth token` (the user-visible CLI surface) prints the same value the daemon caches. A rename of `SERVICE` / `USER` in one binary but not the other would silently break the keychain-path round-trip; renaming the file-format expectations would silently break the config.toml-path round-trip. The CLI's emitted token needs its own equality check against the daemon's authoritative copy.

**Gap C — `bowerbird auth token` failure mode lacked an exact-exit-code assertion.** Task 3.4 of the story spec is explicit ("Exit code MUST be 1 on failure"); the existing `auth_token_returns_nonzero_when_all_paths_exhausted` test used `.failure()`, which accepts any non-zero. A future change that exits 2 (clap's reserved code) instead of 1 — e.g., a refactor that bubbles `anyhow` errors through clap's parse error path — would silently regress the contract.

## Generated Tests

### CLI E2E Tests (new — `tests/cli_auth.rs`)

- **`auth_token_matches_daemon_token_via_shared_config_file`** — Task 7.4 (AC #4). Stages a `config.toml` with a known token, spawns the daemon (no env, keychain disabled — forcing the resolver to land on the file), runs `bowerbird auth token` (same env discipline), asserts the CLI's stdout is exactly the token plus `\n`. The mock-keyring backend cannot bridge subprocess boundaries (see the resolver-test limitation comment), so config.toml is the shared persistent backing — the recommended option-1 path from the story's sub-decision section.
- **`config_toml_rotation_does_not_affect_running_daemon`** — NFR14 (AC #1, AC #5). Stages a `config.toml` with token A, starts the daemon (which caches A in `AppState.bearer`), rewrites `config.toml` to token B, then runs `bowerbird status` twice — once with `BOWERBIRD_TOKEN=A` (env wins in the resolver chain, so this is the in-flight Bearer the CLI sends to `/status`) and once with `BOWERBIRD_TOKEN=B`. The A invocation must render the full `running` block (proves the daemon kept A); the B invocation must produce a 401-degraded message (proves the daemon did NOT reload the rotated file). Fills the dropped Task 6.9 at the E2E layer that the resolver-level mock could not reach.

### CLI E2E Tests (tightened — `tests/cli_auth.rs`)

- **`auth_token_returns_nonzero_when_all_paths_exhausted`** — Task 7.6 contract sharpened from `.failure()` to `.failure().code(1)` to match Task 3.4's exact-exit-1 spec. NFR13 end-to-end stays unchanged otherwise.

### Supporting changes

Two private helpers added at the bottom of `tests/cli_auth.rs`:

- `spawn_daemon_with_config_toml_backing(&tmp, &daemon_bin)` — `bowerbird start` with `BOWERBIRD_TOKEN` removed and the keychain disabled, then polls the ingest socket up to 5s. Used by both new tests so the spawn pattern stays consistent and the ingest-socket-up wait is not duplicated.
- `stop_daemon(&tmp, &daemon_bin)` — `bowerbird stop` plus a 5s pid-death poll. Called before any panic in the new tests so a content mismatch or 401-shape mismatch never leaks a daemon into the runner's process tree.

No new crates, no new workspace dependencies, no test framework changes. Both helpers use only the existing `bowerbird_auth_command` env discipline and `assert_cmd::cargo::cargo_bin`.

## Coverage

| AC / NFR | Resolver-level (daemon contract) | CLI E2E |
|---|---|---|
| AC #1 first-run UUID4 keychain + reused next run | ✅ `mock_keychain_first_run_generates_and_tags_source` (generate branch; mock limitation on read-back documented inline) | ✅ existing env/config-file paths + **new** `auth_token_matches_daemon_token_via_shared_config_file` (round-trip via config.toml backing) |
| AC #2 keychain unavailable → env → config.toml | ✅ `env_var_wins_*`, `disable_keychain_unavailable_falls_back_to_env`, `disable_keychain_no_env_falls_back_to_config_file` | ✅ existing env + config.toml stdout-shape tests |
| AC #3 no token → exit non-zero + enumerated paths (NFR13) | ✅ `disable_no_path_resolves_token_returns_error_naming_each_attempted_path`, `daemon_exits_nonzero_when_token_chain_exhausted` (subprocess) | ✅ **tightened** `auth_token_returns_nonzero_when_all_paths_exhausted` (now asserts exit code is exactly 1) |
| AC #4 `bowerbird auth token` pipe-safe stdout | n/a | ✅ existing `auth_token_prints_env_var_when_set`, `auth_token_reads_from_config_toml_when_no_env_and_no_keychain`, `auth_token_stderr_quiet_when_piped`, `auth_appears_in_top_level_help_and_token_appears_in_auth_help`, `auth_token_warns_on_wide_mode_but_still_loads`, **new** `auth_token_matches_daemon_token_via_shared_config_file` |
| AC #5 rotation requires restart (NFR14) | ⚠️ resolver-level test (Task 6.9) dropped — mock backend cannot model service+user persistence | ✅ **new** `config_toml_rotation_does_not_affect_running_daemon` (E2E proof that daemon caches at startup and ignores mid-run config changes) |
| AC #6 `bowerbird status` auto-resolves token (no `$BOWERBIRD_TOKEN` required) | n/a | ✅ existing `status_shows_full_block_without_user_supplied_token` |
| AC #6 placeholders removed, docs updated | n/a (static review) | ✅ verified via `grep -rn "wait for Story 3.3" src/ crates/` → 0 hits |

## Validation

- [x] `cargo test --workspace -- --test-threads=1 --skip state_plus_event_atomicity_under_sigkill_during_load` — **316 passed (17 suites, ~23s)**. Baseline 314 + 2 new tests. 1 filtered out (the known pre-existing teardown deadlock in `state_plus_event_atomicity_under_sigkill_during_load`, skipped per orchestration custom instructions).
- [x] `cargo clippy --workspace --all-targets` — **0 issues**.
- [x] Both new tests inherit the file-level isolation discipline (per-test `TempDir`, `BOWERBIRD_DATA_DIR` set via `bowerbird_auth_command`, `BOWERBIRD_KEYRING_BACKEND=disable` default, `BOWERBIRD_TOKEN` removed via the helper, `BOWERBIRD_CLAUDE_SETTINGS` / `BOWERBIRD_DAEMON_BIN` / `BOWERBIRD_INGEST_SOCK` cleared). They cannot touch the developer's real `~/.bowerbird/` or real platform keychain.

## Notes

- Scope kept tight to Story 3.3 per orchestration custom instructions; no unrelated code refactored, no new dependencies, no test framework changes. The two new tests use only existing helpers and the established `assert_cmd` + `TempDir` pattern.
- `--test-threads=1` retained per Epic 2 retro AI-3 + Story 3.1/3.2 + 3.3 lessons — the new tests spawn real daemon subprocesses and share PID-file / ingest-socket state with `cli_lifecycle.rs`.
- The keychain mock backend's "no service+user interning" limitation (documented inline on `mock_keychain_first_run_generates_and_tags_source`) is what made the rotation test land at the E2E layer instead of the resolver layer. The persistent backing chosen (`config.toml`) mirrors the story's option-1 recommendation from the cross-process keychain mock sub-decision.
- The rotation test's 401-arm relies on the CLI's `print_running_basic(... "keychain-resolved token rejected by /status (401); ...")` wording from `src/commands/status.rs:101-107` — a future copy-edit on that string would need to keep the substring `running` + `401` to stay green.
