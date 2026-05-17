use std::fmt;
use std::time::{SystemTime, UNIX_EPOCH};

use tracing_subscriber::fmt::format::Writer;
use tracing_subscriber::fmt::time::FormatTime;
use tracing_subscriber::EnvFilter;

pub struct Iso8601Utc;

impl FormatTime for Iso8601Utc {
    fn format_time(&self, w: &mut Writer<'_>) -> fmt::Result {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default();
        let total_secs = now.as_secs();
        let millis = now.subsec_millis();
        let (y, m, d, hh, mm, ss) = unix_seconds_to_ymd_hms(total_secs);
        write!(
            w,
            "{y:04}-{m:02}-{d:02}T{hh:02}:{mm:02}:{ss:02}.{millis:03}Z"
        )
    }
}

fn unix_seconds_to_ymd_hms(secs: u64) -> (i32, u32, u32, u32, u32, u32) {
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    let hh = (rem / 3600) as u32;
    let mm = ((rem % 3600) / 60) as u32;
    let ss = (rem % 60) as u32;

    let (y, m, d) = days_to_ymd(days);
    (y, m, d, hh, mm, ss)
}

/// Convert days since 1970-01-01 to (year, month, day) using Howard Hinnant's
/// civil-from-days algorithm (public domain).
fn days_to_ymd(days: i64) -> (i32, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 {
        z / 146_097
    } else {
        (z - 146_096) / 146_097
    };
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = if m <= 2 { y + 1 } else { y };
    (y as i32, m, d)
}

pub fn init(verbose: u8) {
    let level = match verbose {
        0 => "error",
        1 => "info",
        _ => "debug",
    };
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(level));

    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_timer(Iso8601Utc)
        .with_target(false)
        .with_level(true)
        .try_init()
        .ok();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn epoch_is_1970_01_01() {
        assert_eq!(unix_seconds_to_ymd_hms(0), (1970, 1, 1, 0, 0, 0));
    }

    #[test]
    fn known_date_2026_05_17() {
        // days from 1970-01-01 to 2026-05-17 = 20590
        // (56 years * 365 + 14 leap days + 136 days into 2026)
        let secs = 20_590u64 * 86_400 + 14 * 3600 + 32 * 60 + 11;
        assert_eq!(unix_seconds_to_ymd_hms(secs), (2026, 5, 17, 14, 32, 11));
    }

    #[test]
    fn leap_day_2024_02_29() {
        // 2024-02-29: days from 1970-01-01 = 54 * 365 + 13 leap days + 59
        let secs = (54u64 * 365 + 13 + 59) * 86_400;
        assert_eq!(unix_seconds_to_ymd_hms(secs), (2024, 2, 29, 0, 0, 0));
    }
}
