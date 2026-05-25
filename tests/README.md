# workspace-root tests

These tests sit at the workspace root (NOT under a per-crate `tests/`) because
they pin cross-cutting invariants that no single crate owns: the CLI binary's
user-facing surface, doc-drift guardrails between code and `docs/`, the
protocol-changelog gate, and the v1.0 wire-compat corpus.

The rest (`crates/<name>/tests/`) is per-crate contract tests — those stay
co-located with the crate they exercise.

## Gated tests (do NOT run by default)

- `cross_version_upgrade.rs` (Story 4.4 / AC #5) — SKIPs unless
  `BOWERBIRD_RUN_CROSS_VERSION_TEST=1` is set AND a prior-version daemon
  binary is resolvable via `BOWERBIRD_PRIOR_VERSION_BINARY` or
  `target/cross-version-installs/v0.1.0/bin/bowerbird-daemon`. Wired into the
  release-pipeline CI lane (`.github/workflows/release.yml::cross-version-test`),
  NOT the per-PR lane. Becomes load-bearing once v0.1.x ships and the
  release pipeline tags successive versions.

  Local invocation:
  ```sh
  cargo install --git . --tag v0.1.0 --root target/cross-version-installs/v0.1.0 --bin bowerbird-daemon
  BOWERBIRD_RUN_CROSS_VERSION_TEST=1 \
    BOWERBIRD_PRIOR_VERSION_BINARY=target/cross-version-installs/v0.1.0/bin/bowerbird-daemon \
    cargo test --test cross_version_upgrade
  ```
