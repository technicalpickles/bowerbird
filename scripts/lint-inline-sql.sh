#!/usr/bin/env bash
# Inline SQL ban.
#
# Fails if any `.rs` file under `crates/daemon/src/` contains inline SQL string
# literals (SELECT, INSERT INTO, UPDATE, DELETE FROM) outside of the two files
# that are allowed to hold SQL: `db/queries.rs` and `db/migrations.rs`.
#
# Usage:
#   ./scripts/lint-inline-sql.sh
#   SCAN_EXTRA="path1 path2" ./scripts/lint-inline-sql.sh
#
# Exit code 0 = clean. Non-zero = violation found.
#
# Compatibility: portable bash (no `mapfile`); works on macOS bash 3.2 and
# Ubuntu bash 5.

set -eu

ALLOWED_RE='crates/daemon/src/db/(queries|migrations)\.rs'

# Collect scan targets without mapfile.
files=()
while IFS= read -r f; do
  [ -n "$f" ] && files+=("$f")
done < <(git ls-files 'crates/daemon/src/**/*.rs' 'crates/daemon/src/*.rs')

if [ -n "${SCAN_EXTRA:-}" ]; then
  for extra in $SCAN_EXTRA; do
    files+=("$extra")
  done
fi

violations=0
for f in "${files[@]}"; do
  if echo "$f" | grep -Eq "$ALLOWED_RE"; then
    continue
  fi
  if [ ! -f "$f" ]; then
    continue
  fi
  if grep -nE '"(SELECT |INSERT INTO|UPDATE |DELETE FROM)' "$f" >/dev/null 2>&1; then
    grep -nE '"(SELECT |INSERT INTO|UPDATE |DELETE FROM)' "$f" | while IFS= read -r line; do
      echo "$f: $line"
    done
    violations=$((violations + 1))
  fi
done

if [ "$violations" -gt 0 ]; then
  echo "ERROR: inline SQL string literals found outside db/queries.rs and db/migrations.rs" >&2
  exit 1
fi

echo "ok: no inline SQL outside db/queries.rs and db/migrations.rs"
