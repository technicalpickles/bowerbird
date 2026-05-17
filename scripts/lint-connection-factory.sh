#!/usr/bin/env bash
# Connection factory enforcement lint.
#
# AC #6: `rusqlite::Connection::open` in *any call form* outside the
# connection factory module fails the build. This script catches:
#   1. Fully-qualified: `rusqlite::Connection::open(...)`
#   2. Fully-qualified open_with_flags / open_in_memory / etc.
#   3. Imported `Connection::open(...)` (paired with `use rusqlite::Connection`)
#   4. Imported `Connection::open_with_flags(...)`
#
# The allowed factory module is `crates/daemon/src/db/pool.rs`. Test code in
# `crates/daemon/tests/**` is also exempt — the lint scope is production
# sources only.
#
# Usage:
#   ./scripts/lint-connection-factory.sh
#   SCAN_EXTRA="path1 path2" ./scripts/lint-connection-factory.sh
#
# Exit code 0 = clean. Non-zero = violation found.
#
# Compatibility: portable bash (no `mapfile`); works on macOS bash 3.2 and
# Ubuntu bash 5.

set -eu

ALLOWED='crates/daemon/src/db/pool.rs'

# Pattern catches:
#   rusqlite::Connection::open(_with_flags|_in_memory|...)
#   Connection::open(_with_flags|_in_memory|...)
# but only when preceded by a non-identifier character (so we don't match
# `MyConnection::open` or `connection_open`).
PATTERN='(^|[^A-Za-z0-9_:])(rusqlite::)?Connection::open(_[A-Za-z_]+)?[[:space:]]*\('

# Collect scan targets without mapfile (macOS bash 3.2 compatible).
files=()
while IFS= read -r f; do
  [ -n "$f" ] && files+=("$f")
done < <(git ls-files 'crates/daemon/src/**/*.rs' 'crates/daemon/src/*.rs')

if [ -n "${SCAN_EXTRA:-}" ]; then
  for extra in $SCAN_EXTRA; do
    files+=("$extra")
  done
fi

# Additionally require that the imported `Connection::open` form is only a
# violation when there's an actual `use rusqlite::Connection` or
# `use rusqlite::{...Connection...}` in the same file — otherwise
# `Connection::open` could be referring to any other Connection type. The
# fully-qualified `rusqlite::Connection::open` form is always a violation.

is_violation_line() {
  local file="$1"
  local line="$2"

  case "$line" in
    *rusqlite::Connection::open*)
      return 0
      ;;
  esac

  if echo "$line" | grep -Eq '(^|[^A-Za-z0-9_:])Connection::open(_[A-Za-z_]+)?[[:space:]]*\('; then
    if grep -Eq '^[[:space:]]*use[[:space:]]+rusqlite::(\{[^}]*Connection[^}]*\}|Connection)' "$file"; then
      return 0
    fi
  fi

  return 1
}

violations=0
for f in "${files[@]}"; do
  if [ "$f" = "$ALLOWED" ]; then
    continue
  fi
  if [ ! -f "$f" ]; then
    continue
  fi

  while IFS= read -r match; do
    [ -z "$match" ] && continue
    line_content="${match#*:}"
    if is_violation_line "$f" "$line_content"; then
      echo "$f: $match"
      violations=$((violations + 1))
    fi
  done < <(grep -nE "$PATTERN" "$f" 2>/dev/null || true)
done

if [ "$violations" -gt 0 ]; then
  echo "ERROR: Connection::open call(s) found outside $ALLOWED" >&2
  exit 1
fi

echo "ok: no rusqlite::Connection::open calls outside $ALLOWED"
