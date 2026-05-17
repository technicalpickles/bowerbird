#!/usr/bin/env bash
# Enforces the connection-factory policy: rusqlite::Connection::open may only
# appear in crates/daemon/src/db/pool.rs. Every other call site must check out
# a connection from the deadpool-managed DbPools so that the PRAGMA bundle
# (foreign_keys, journal_mode=WAL, synchronous=NORMAL) is applied to every
# connection.
#
# Exit codes:
#   0  - no violations
#   1  - one or more violations found
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
src_dir="${repo_root}/crates/daemon/src"
factory="${repo_root}/crates/daemon/src/db/pool.rs"

if [[ ! -d "${src_dir}" ]]; then
    echo "lint-db-access: ${src_dir} not found"
    exit 1
fi

# `grep -r` over the daemon src tree. Tests directory is intentionally excluded;
# integration tests may simulate a corrupt DB by calling Connection::open
# directly.
matches="$(grep -rln "rusqlite::Connection::open" "${src_dir}" || true)"

violations=""
while IFS= read -r path; do
    [[ -z "${path}" ]] && continue
    if [[ "${path}" != "${factory}" ]]; then
        violations+="${path}"$'\n'
    fi
done <<< "${matches}"

if [[ -n "${violations}" ]]; then
    echo "lint-db-access: rusqlite::Connection::open is permitted only in ${factory}"
    echo "violations:"
    printf '%s' "${violations}"
    exit 1
fi
