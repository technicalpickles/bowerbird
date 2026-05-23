//! Wall-clock helpers shared across the daemon.
//!
//! `current_unix_millis` is the single source of truth for "now" as a
//! `i64` millisecond timestamp. Keep this internal — wall-clock reads
//! from elsewhere should call this fn, not hand-roll the conversion.
//! Adding a `chrono` or `time` dep here is explicitly *not* the project
//! pattern (see Story 1.6 Anti-Patterns).

use std::time::{SystemTime, UNIX_EPOCH};

use crate::error::{Error, Result};

pub fn current_unix_millis() -> Result<i64> {
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| Error::Clock(format!("system time before UNIX_EPOCH: {e}")))?
        .as_millis();
    i64::try_from(now_ms).map_err(|_| Error::Clock(format!("timestamp overflows i64: {now_ms}")))
}
