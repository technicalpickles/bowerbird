//! All SQL string literals used by the daemon outside of `migrations.rs`.
//!
//! Inline SQL strings anywhere else in the daemon crate are forbidden and the
//! CI grep gate (`.github/workflows/ci.yml`) enforces this.

pub(crate) const INSERT_EVENT: &str = "\
INSERT INTO events (source, session_id, kind, reaction, payload, created_at)
VALUES (?, ?, ?, ?, ?, ?)
RETURNING event_id";

pub(crate) const UPSERT_PROJECTION: &str = "\
INSERT INTO session_projections (source, session_id, state, updated_at)
VALUES (?, ?, ?, ?)
ON CONFLICT(source, session_id)
DO UPDATE SET state = excluded.state, updated_at = excluded.updated_at";

pub(crate) const READYZ_PROBE: &str = "SELECT 1 FROM events WHERE 1=0 LIMIT 1";

#[cfg(test)]
pub(crate) const PRAGMA_FOREIGN_KEYS: &str = "PRAGMA foreign_keys";
#[cfg(test)]
pub(crate) const PRAGMA_JOURNAL_MODE: &str = "PRAGMA journal_mode";
#[cfg(test)]
pub(crate) const PRAGMA_SYNCHRONOUS: &str = "PRAGMA synchronous";
