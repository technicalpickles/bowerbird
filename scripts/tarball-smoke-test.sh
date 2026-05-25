#!/usr/bin/env bash
# Local-only smoke test for the Story 3.4 release tarball layout.
#
# Mirrors the staging+tar logic in `.github/workflows/release.yml` against
# already-built workspace binaries, then asserts the extracted tarball
# matches the eight-entry layout documented in Task 3.5 of story
# `docs/bmad/implementation-artifacts/3-4-prebuilt-binary-distribution-and-release-pipeline.md`.
#
# This script is NOT wired into CI — by design. The release workflow only
# runs on tag push, and the GH-hosted runner is the production validator.
# This local smoke is for the cycle BEFORE tagging: run it on macOS arm64
# (the dev box) after `cargo build --release --workspace`, before pushing
# a `vX.Y.Z` tag. It catches the regression modes a missing-step grep
# can't (a `cp` source path that exists in the repo but doesn't end up in
# the tarball, a tar invocation that flattens the staging dir, etc.).
#
# Usage:
#   ./scripts/tarball-smoke-test.sh                  # uses host triple, vX.Y.Z-smoke tag
#   ./scripts/tarball-smoke-test.sh v0.1.0           # custom tag
#   ./scripts/tarball-smoke-test.sh v0.1.0 ARM64     # custom tag + custom triple
#
# Exits 0 on a clean tarball that matches the documented layout; non-zero
# with a diagnostic on any mismatch. The script is read-only against the
# repo — it only writes under a TMPDIR-managed staging area.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$REPO_ROOT"

TAG="${1:-v0.0.0-smoke}"

# Detect the host triple if not provided. The release workflow ships three
# targets; this smoke runs against whichever the dev's local cargo builds
# for by default.
if [ "${2:-}" != "" ]; then
  TARGET="$2"
else
  TARGET="$(rustc -vV | awk '/^host:/ {print $2}')"
fi

if [ -z "$TARGET" ]; then
  echo "ERROR: could not detect host target triple via \`rustc -vV\`" >&2
  exit 2
fi

echo "==> tarball-smoke-test: tag=$TAG, target=$TARGET"

# The three binaries the release workflow expects. The shim normally ships
# under `target/<triple>/release-shim/` but local builds without an
# explicit `--target <triple>` land under `target/release/` and
# `target/release-shim/` instead. We check both layouts and fall back so
# this script is usable from a plain `cargo build --release --workspace`.
RELEASE_DIR="target/$TARGET/release"
SHIM_DIR="target/$TARGET/release-shim"
if [ ! -d "$RELEASE_DIR" ]; then
  RELEASE_DIR="target/release"
fi
if [ ! -d "$SHIM_DIR" ]; then
  SHIM_DIR="target/release-shim"
fi

for needed in \
  "$RELEASE_DIR/bowerbird" \
  "$RELEASE_DIR/bowerbird-daemon"; do
  if [ ! -x "$needed" ]; then
    echo "ERROR: required binary missing: $needed" >&2
    echo "  Run: cargo build --release --workspace --exclude bowerbird-shim" >&2
    exit 3
  fi
done

if [ ! -x "$SHIM_DIR/bowerbird-shim" ]; then
  echo "ERROR: required binary missing: $SHIM_DIR/bowerbird-shim" >&2
  echo "  Run: cargo build --profile release-shim -p bowerbird-shim" >&2
  exit 3
fi

# Eight documented tarball entries (Task 3.5). The release.yml `cp`
# sources for each — kept in sync with the test
# `release_workflow_stages_all_documented_tarball_entries` in
# `tests/release_pipeline_docs.rs`.
REQUIRED_REPO_FILES=(
  "adapters/claude/tool-reactions.toml"
  "LICENSE"
  "LICENSE-MIT"
  "LICENSE-APACHE"
  "README.md"
  "INSTALL.md"
  "docs/protocol-changelog.md"
)
for f in "${REQUIRED_REPO_FILES[@]}"; do
  if [ ! -f "$f" ]; then
    echo "ERROR: required source file missing: $f" >&2
    echo "  The release workflow's staging step will fail." >&2
    exit 4
  fi
done

# Staging area under a tmpdir so re-runs are clean.
WORK="$(mktemp -d -t bowerbird-tarball-smoke.XXXXXX)"
trap 'rm -rf "$WORK"' EXIT

NAME="bowerbird-${TAG}-${TARGET}"
STAGING="$WORK/staging/$NAME"

echo "==> staging into $STAGING"

mkdir -p "$STAGING/bin"
mkdir -p "$STAGING/adapters/claude"

cp "$RELEASE_DIR/bowerbird"          "$STAGING/bin/"
cp "$RELEASE_DIR/bowerbird-daemon"   "$STAGING/bin/"
cp "$SHIM_DIR/bowerbird-shim"        "$STAGING/bin/"
cp "adapters/claude/tool-reactions.toml" "$STAGING/adapters/claude/"

cp LICENSE        "$STAGING/"
cp LICENSE-MIT    "$STAGING/"
cp LICENSE-APACHE "$STAGING/"
cp README.md      "$STAGING/"
cp INSTALL.md     "$STAGING/"
cp docs/protocol-changelog.md "$STAGING/CHANGELOG.md"

echo "==> creating tarball"
TARBALL="$WORK/${NAME}.tar.gz"
tar -czf "$TARBALL" -C "$WORK/staging" "$NAME"

# Verify the tarball extracts cleanly into a sibling dir and that the
# extracted layout matches the documented eight entries.
EXTRACT="$WORK/extract"
mkdir -p "$EXTRACT"
tar -xzf "$TARBALL" -C "$EXTRACT"

if [ ! -d "$EXTRACT/$NAME" ]; then
  echo "ERROR: tarball did not extract into expected directory: $EXTRACT/$NAME" >&2
  echo "  Listing:" >&2
  ls -laR "$EXTRACT" >&2
  exit 5
fi

# The eight entries (3 binaries + 1 data file + 3 license files + README +
# INSTALL + CHANGELOG = 9 files in 3 directories. We list the must-exist
# paths exhaustively.)
EXPECTED_PATHS=(
  "bin/bowerbird"
  "bin/bowerbird-shim"
  "bin/bowerbird-daemon"
  "adapters/claude/tool-reactions.toml"
  "LICENSE"
  "LICENSE-MIT"
  "LICENSE-APACHE"
  "README.md"
  "INSTALL.md"
  "CHANGELOG.md"
)

FAIL=0
for p in "${EXPECTED_PATHS[@]}"; do
  if [ ! -e "$EXTRACT/$NAME/$p" ]; then
    echo "FAIL: extracted tarball missing $p" >&2
    FAIL=1
  fi
done

# Sanity check: the three binaries should be executable after extraction.
for b in bowerbird bowerbird-shim bowerbird-daemon; do
  if [ ! -x "$EXTRACT/$NAME/bin/$b" ]; then
    echo "FAIL: extracted bin/$b is not executable" >&2
    FAIL=1
  fi
done

# Sanity check: the extracted bowerbird actually runs and prints a version.
if ! "$EXTRACT/$NAME/bin/bowerbird" --version >/dev/null 2>&1; then
  echo "WARN: extracted bowerbird --version did not exit 0 (acceptable for cross-target tarballs; investigate if smoke-testing the host target)"
fi

if [ "$FAIL" -ne 0 ]; then
  echo "==> tarball-smoke-test FAILED" >&2
  exit 6
fi

echo "==> tarball-smoke-test OK"
echo "    tarball: $TARBALL"
echo "    entries:"
( cd "$EXTRACT/$NAME" && find . -mindepth 1 -maxdepth 3 -print | sort | sed 's/^/      /' )
