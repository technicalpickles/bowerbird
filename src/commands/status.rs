//! `bowerbird status` — informational view of the daemon.
//!
//! Resolution order (graceful-degradation, each step has a fallback so the
//! user always gets *something*):
//!
//! 1. Read `~/.bowerbird/bowerbird.pid` — if missing, "stopped".
//! 2. `kill(pid, 0)` — if the named process is dead, "stopped (stale pid file)".
//! 3. Connect-probe `~/.bowerbird/ingest.sock` — if it doesn't accept,
//!    "degraded (process exists but not accepting ingest)".
//! 4. Read `~/.bowerbird/server.json` — if missing/unparseable, liveness-only
//!    output (no version/uptime/ws-clients).
//! 5. `GET /status` with `$BOWERBIRD_TOKEN` — if 401, document the token
//!    limitation; Story 3.3 will resolve it. On 200, format the full block.
//!
//! Always exits 0 unless we can't tell what's going on at all (e.g. EACCES on
//! the data dir, which is reported as an error).

use std::net::SocketAddr;
use std::time::Duration;

use clap::Args;

use super::daemon::{self, StatusResponse};

#[derive(Args)]
pub struct StatusArgs {}

const STATUS_PER_ATTEMPT: Duration = Duration::from_millis(500);

pub fn run(_args: StatusArgs) -> anyhow::Result<()> {
    let bowerbird_dir = super::resolve_bowerbird_dir()?;

    let pid_path = bowerbird_dir.join("bowerbird.pid");
    let pid = match daemon::read_pid(&pid_path)? {
        None => {
            print_stopped(None);
            return Ok(());
        }
        Some(p) => p,
    };

    if !daemon::pid_alive(pid) {
        print_stopped(Some(format!("stale pid file: pid {pid} is not running")));
        return Ok(());
    }

    let ingest_sock = bowerbird_dir.join("ingest.sock");
    if !super::daemon_is_up(&ingest_sock) {
        print_degraded(pid, "is not accepting ingest connections");
        return Ok(());
    }

    let info = match daemon::read_server_info(&bowerbird_dir) {
        Some(i) => i,
        None => {
            // Liveness without details — the daemon is up but we cannot reach
            // its HTTP surface because server.json is missing or unparseable.
            print_running_basic(pid, "server.json missing or unparseable");
            return Ok(());
        }
    };

    let addr: SocketAddr = match info.bind_addr.parse() {
        Ok(a) => a,
        Err(_) => {
            print_running_basic(
                pid,
                &format!("server.json bind_addr {:?} unparseable", info.bind_addr),
            );
            return Ok(());
        }
    };

    let token = std::env::var("BOWERBIRD_TOKEN").ok().filter(|s| !s.is_empty());
    if token.is_none() {
        print_running_basic(
            pid,
            "$BOWERBIRD_TOKEN unset; cannot read /status — set it or wait for Story 3.3",
        );
        return Ok(());
    }

    match daemon::http_get_status(addr, token.as_deref(), STATUS_PER_ATTEMPT) {
        StatusResponse::Ok(body) => {
            let status: protocol::DaemonStatus = match serde_json::from_slice(&body) {
                Ok(s) => s,
                Err(e) => {
                    print_running_basic(pid, &format!("parsed /status body failed: {e}"));
                    return Ok(());
                }
            };
            print_full_status(pid, &status);
        }
        StatusResponse::Status(401) => {
            print_running_basic(
                pid,
                "$BOWERBIRD_TOKEN is stale; cannot read /status — set the current token or wait for Story 3.3 keychain integration",
            );
        }
        StatusResponse::Status(code) => {
            print_running_basic(pid, &format!("/status returned HTTP {code}"));
        }
        StatusResponse::Unreachable => {
            print_running_basic(pid, &format!("/status unreachable at http://{addr}"));
        }
    }
    Ok(())
}

fn print_stopped(detail: Option<String>) {
    println!("bowerbird daemon");
    match detail {
        Some(d) => println!("  status        : stopped ({d})"),
        None => println!("  status        : stopped"),
    }
}

fn print_degraded(pid: i32, why: &str) {
    println!("bowerbird daemon");
    println!("  status        : degraded (pid {pid} exists but {why})");
}

fn print_running_basic(pid: i32, note: &str) {
    println!("bowerbird daemon");
    println!("  status        : running (pid {pid}; {note})");
}

fn print_full_status(pid: i32, status: &protocol::DaemonStatus) {
    println!("bowerbird daemon");
    println!("  status        : running");
    println!("  pid           : {pid}");
    println!("  version       : {}", status.daemon_version);
    println!("  protocol      : {}", status.protocol_version);
    println!(
        "  uptime        : {}",
        format_uptime(Duration::from_millis(u64::try_from(status.uptime_ms.max(0)).unwrap_or(0)))
    );
    println!("  connected ws  : {}", status.connected_ws_clients);
    match (status.last_event_at_ms, status.last_event_id) {
        (Some(at_ms), Some(id)) => {
            // Best-effort "Xs ago" relative to local wall clock; if the clock
            // is skewed we still print a sane absolute fallback.
            let now = current_unix_millis();
            let ago_ms = now.saturating_sub(at_ms);
            if ago_ms >= 0 {
                let ago = Duration::from_millis(u64::try_from(ago_ms).unwrap_or(0));
                println!(
                    "  last event    : {} ago (event_id={})",
                    format_uptime(ago),
                    id.0
                );
            } else {
                println!("  last event    : at_ms={at_ms} (event_id={})", id.0);
            }
        }
        _ => {
            println!("  last event    : (never)");
        }
    }
}

/// Hand-rolled `Duration → "1h 23m 7s"` formatter to avoid adding `humantime`
/// for a single display path (per Task 5.5 recommendation).
fn format_uptime(d: Duration) -> String {
    let total = d.as_secs();
    let h = total / 3600;
    let m = (total % 3600) / 60;
    let s = total % 60;
    if h > 0 {
        format!("{h}h {m}m {s}s")
    } else if m > 0 {
        format!("{m}m {s}s")
    } else {
        format!("{s}s")
    }
}

fn current_unix_millis() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    let dur = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    i64::try_from(dur.as_millis()).unwrap_or(i64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_uptime_seconds_only() {
        assert_eq!(format_uptime(Duration::from_secs(7)), "7s");
    }

    #[test]
    fn format_uptime_minutes_and_seconds() {
        assert_eq!(format_uptime(Duration::from_secs(125)), "2m 5s");
    }

    #[test]
    fn format_uptime_hours_minutes_seconds() {
        let d = Duration::from_secs(3600 + 23 * 60 + 7);
        assert_eq!(format_uptime(d), "1h 23m 7s");
    }
}
