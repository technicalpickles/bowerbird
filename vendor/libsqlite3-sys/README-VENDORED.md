# Vendored libsqlite3-sys 0.36.0 with SQLite 3.51.3

This is crates.io `libsqlite3-sys 0.36.0` with one change: the bundled
SQLite amalgamation (`sqlite3/` directory) is replaced with the one from
`libsqlite3-sys 0.37.0`, upgrading SQLite 3.51.1 -> 3.51.3. The two
crates' pregenerated bindings are identical except for version-string
constants, so this is a drop-in swap. The `sqlcipher/` amalgamation was
deleted to save 10MB; it is only compiled under the `bundled-sqlcipher*`
features, which this workspace does not enable (restore it from the
registry copy if that ever changes).

## Why

SQLite 3.51.1 (the exact version bundled by libsqlite3-sys 0.36.0, and
only that release) has a lock-order-inversion regression in the unix
VFS: `unixLock` -> `unixIsSharingShmNode` acquires the global VFS mutex
while holding the per-inode lock mutex, while `unixClose` takes the same
two mutexes in the opposite (documented-correct) order. Two connections
to the same WAL database closed concurrently can deadlock inside
`sqlite3_close`, which is exactly what deadpool's fire-and-forget
connection drops produce at pool teardown. This was the root cause of
this repo's intermittent test hangs (and a latent daemon graceful-
shutdown hang). Evidence and diagnosis:
`docs/bmad/implementation-artifacts/investigations/test-serialization-investigation.md`
(2026-07-28 addendum). Upstream: reported 2025-12-05 on the SQLite
forum ("TSAN: lock-order-inversion since 3.51.1"), fixed same day,
shipped in SQLite 3.51.2.

We cannot take the fix the normal way because fixed SQLite lives in
libsqlite3-sys >= 0.37.0, which requires rusqlite >= 0.39, and
deadpool-sqlite (0.13.0, latest as of 2026-07) still pins rusqlite
`^0.38`. This vendored patch is wired in via `[patch.crates-io]` in the
workspace `Cargo.toml`.

## When to remove

As soon as deadpool-sqlite releases support for rusqlite >= 0.39
(tracking: https://github.com/deadpool-rs/deadpool/issues/490), bump
rusqlite/deadpool-sqlite, delete this directory, and drop the
`[patch.crates-io]` section. Verify with:
`grep SQLITE_VERSION target/debug/build/libsqlite3-sys-*/out/sqlite3/sqlite3.h`
or a runtime `rusqlite::version()` check — anything >= 3.51.2 is safe.
