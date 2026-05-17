#!/usr/bin/env bash
# Inline SQL ban.
#
# Fails if any `.rs` file under `crates/daemon/src/` contains inline SQL string
# literals (SELECT, INSERT INTO, UPDATE, DELETE FROM) outside of the two files
# that are allowed to hold SQL: `db/queries.rs` and `db/migrations.rs`.
#
# Exit code 0 = clean. Non-zero = violation found.

set -euo pipefail

ALLOWED_RE='crates/daemon/src/db/(queries|migrations)\.rs'

mapfile -t files < <(git ls-files 'crates/daemon/src/**/*.rs' 'crates/daemon/src/*.rs')
if [[ -n "${SCAN_EXTRA:-}" ]]; then
  files+=("$SCAN_EXTRA")
fi

violations=0
for f in "${files[@]}"; do
  if [[ "$f" =~ $ALLOWED_RE ]]; then
    continue
  fi
  if [[ ! -f "$f" ]]; then
    continue
  fi
  if grep -nE '"(SELECT |INSERT INTO|UPDATE |DELETE FROM)' "$f" >/dev/null 2>&1; then
    grep -nE '"(SELECT |INSERT INTO|UPDATE |DELETE FROM)' "$f" | while IFS= read -r line; do
      echo "$f: $line"
    done
    violations=$((violations + 1))
  fi
done

if [[ $violations -gt 0 ]]; then
  echo "ERROR: inline SQL string literals found outside db/queries.rs and db/migrations.rs" >&2
  exit 1
fi

echo "ok: no inline SQL outside db/queries.rs and db/migrations.rs"
