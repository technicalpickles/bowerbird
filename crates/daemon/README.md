# bowerbird-daemon

Local-host substrate daemon. Owns SQLite, the HTTP/WS surface, and the projection
write path.

## Database access policy

`rusqlite::Connection::open` is permitted in **exactly one file**:
[`src/db/pool.rs`](src/db/pool.rs).

Every other call site in `crates/daemon/src/` must check out a connection from
`DbPools.writer` or `DbPools.reader`. The pool factory installs a per-connection
`post_create` hook that sets `foreign_keys = ON`, `journal_mode = WAL`, and
`synchronous = NORMAL`. Bypassing the factory means bypassing those pragmas, which
silently breaks the daemon's durability and integrity guarantees.

The policy is enforced by [`scripts/lint-db-access.sh`](../../scripts/lint-db-access.sh)
and by the `connection_factory_policy_lint_passes` contract test.
