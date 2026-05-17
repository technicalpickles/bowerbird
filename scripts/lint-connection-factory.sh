#!/usr/bin/env bash
# Connection factory enforcement lint.
#
# Fails if `rusqlite::Connection::open` appears in any `.rs` file under
# `crates/daemon/src/` other than `crates/daemon/src/db/pool.rs`. The factory
# rule is documented in `crates/daemon/src/db/pool.rs` and Story 1.2.
#
# Usage:
#   ./scripts/lint-connection-factory.sh           # scan the working tree
#   SCAN_EXTRA=path/to/fixture ./scripts/...       # also scan an extra file
#
# Exit code 0 = clean. Non-zero = violation found.

set -euo pipefail

ALLOWED='crates/daemon/src/db/pool.rs'

# Collect candidate files: every tracked .rs file under crates/daemon/src/,
# plus anything injected via SCAN_EXTRA (for the self-test fixture).
mapfile -t files < <(git ls-files 'crates/daemon/src/**/*.rs' 'crates/daemon/src/*.rs')
if [[ -n "${SCAN_EXTRA:-}" ]]; then
  files+=("$SCAN_EXTRA")
fi

violations=0
for f in "${files[@]}"; do
  if [[ "$f" == "$ALLOWED" ]]; then
    continue
  fi
  if [[ ! -f "$f" ]]; then
    continue
  fi
  if grep -nE 'rusqlite::Connection::open' "$f" >/dev/null 2>&1; then
    grep -nE 'rusqlite::Connection::open' "$f" | while IFS= read -r line; do
      echo "$f: $line"
    done
    violations=$((violations + 1))
  fi
done

if [[ $violations -gt 0 ]]; then
  echo "ERROR: rusqlite::Connection::open found outside $ALLOWED" >&2
  exit 1
fi

echo "ok: no rusqlite::Connection::open calls outside $ALLOWED"
