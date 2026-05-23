use std::fs::{set_permissions, OpenOptions, Permissions};
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::error::{Error, Result};

/// Append one timestamped line to the shim log, ensuring the file is mode 0600
/// regardless of the caller's umask (mirrors Story 1.3's chmod-after-bind for
/// the ingest socket).
pub(crate) fn append(log_path: &Path, level: &str, message: &str) -> Result<()> {
    if let Some(parent) = log_path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent).map_err(Error::LogIo)?;
        }
    }

    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(log_path)
        .map_err(Error::LogIo)?;

    // `OpenOptionsExt::mode(0o600)` is the *target* mode passed to open(2),
    // but the kernel applies `mode & !umask`. To force 0o600 regardless of
    // umask, chmod after open — same pattern Story 1.3 used for ingest.sock.
    set_permissions(log_path, Permissions::from_mode(0o600)).map_err(Error::LogIo)?;

    let timestamp = iso8601_utc_now();
    let line = format!("{timestamp} {level} {message}\n");
    file.write_all(line.as_bytes()).map_err(Error::LogIo)?;
    Ok(())
}

fn iso8601_utc_now() -> String {
    let dur = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let total_secs = dur.as_secs();
    let millis = dur.subsec_millis();

    let days = (total_secs / 86_400) as i64;
    let secs_of_day = total_secs % 86_400;
    let (y, m, d) = civil_from_days(days);
    let hh = (secs_of_day / 3600) as u32;
    let mm = ((secs_of_day % 3600) / 60) as u32;
    let ss = (secs_of_day % 60) as u32;
    format!("{y:04}-{m:02}-{d:02}T{hh:02}:{mm:02}:{ss:02}.{millis:03}Z")
}

/// Howard Hinnant's `civil_from_days`: days-since-1970-01-01 → (year, month, day).
/// Public-domain algorithm from <http://howardhinnant.github.io/date_algorithms.html>.
fn civil_from_days(z: i64) -> (i32, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 }.div_euclid(146_097);
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = if m <= 2 { y + 1 } else { y } as i32;
    (y, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iso8601_known_dates() {
        // Epoch.
        let s = civil_from_days(0);
        assert_eq!(s, (1970, 1, 1));
        // Y2K.
        let s = civil_from_days(10_957);
        assert_eq!(s, (2000, 1, 1));
        // A known future date — 2026-05-18 (story date).
        // Days from 1970-01-01 to 2026-05-18 = 20_591.
        let s = civil_from_days(20_591);
        assert_eq!(s, (2026, 5, 18));
    }

    #[test]
    fn iso8601_format_shape() {
        let s = iso8601_utc_now();
        // YYYY-MM-DDTHH:MM:SS.sssZ — exactly 24 chars.
        assert_eq!(s.len(), 24, "got: {s}");
        assert!(s.ends_with('Z'));
        assert_eq!(s.chars().nth(4), Some('-'));
        assert_eq!(s.chars().nth(7), Some('-'));
        assert_eq!(s.chars().nth(10), Some('T'));
        assert_eq!(s.chars().nth(13), Some(':'));
        assert_eq!(s.chars().nth(16), Some(':'));
        assert_eq!(s.chars().nth(19), Some('.'));
    }
}
