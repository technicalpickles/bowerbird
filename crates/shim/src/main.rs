mod error;
mod log;
mod socket;

use std::ffi::OsString;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use error::{Error, Result};

const MAX_STDIN_BYTES: usize = 1 << 20; // 1 MiB cap on hook payload

fn main() {
    // Resolve log_path BEFORE attempting any work that might need to log.
    // If neither HOME nor the env override is set we have nowhere to write
    // the failure log — name the cause on stderr (no `(see ...)` pointer: no
    // log path could be resolved) and exit 1 with the daemon-unreachable exit
    // code, which is the closest signal Claude Code will pick up. Swallow the
    // stderr write error exactly as the failure-arm log append does (AC5).
    let log_path = match resolve_log_path() {
        Ok(p) => p,
        Err(_) => {
            let _ = writeln!(io::stderr(), "bowerbird: HOME not set, cannot record event");
            std::process::exit(1);
        }
    };

    match run(&log_path) {
        Ok(()) => std::process::exit(0),
        Err(e) => {
            // Swallow log failures: we're already failing, and crashing the
            // shim makes things worse than dropping the log line.
            let _ = log::append(&log_path, e.level(), &e.to_string());
            // Name the cause on stderr for the exit-1 (ERROR) class so Claude
            // surfaces "bowerbird: <cause>" instead of its causeless
            // "No stderr output" hook error. The exit-0 (WARN) class returns
            // `None` by contract (NFR20: daemon up and answering → see success).
            // Swallow write errors (AC5): a failed stderr write never panics
            // and never changes the exit code, mirroring the log append above.
            if let Some(hint) = e.stderr_hint() {
                let _ = writeln!(
                    io::stderr(),
                    "bowerbird: {hint} (see {})",
                    log_path.display()
                );
            }
            std::process::exit(e.exit_code());
        }
    }
}

fn run(_log_path: &Path) -> Result<()> {
    let hook_kind = parse_hook_kind()?;
    let sock_path = resolve_sock_path()?;

    let stdin_bytes = read_stdin_capped()?;
    if stdin_bytes.is_empty() {
        return Err(Error::StdinEmpty);
    }

    let mut value: serde_json::Value =
        serde_json::from_slice(&stdin_bytes).map_err(Error::StdinJson)?;

    let obj = value.as_object_mut().ok_or(Error::StdinNotJsonObject)?;

    // Inject hook_kind. `serde_json::Map::insert` overwrites any existing
    // entry — the CLI arg wins. The original `hook_event_name` (Claude's
    // own field) is preserved verbatim.
    obj.insert(
        "hook_kind".to_string(),
        serde_json::Value::String(hook_kind),
    );

    // Inject bowerbird_ppid (Story 5.3). Claude Code is the shim's parent, so
    // libc::getppid() returns Claude's PID — the daemon uses this as the
    // session's liveness probe target. getppid is signal-safe and cannot fail.
    #[allow(unsafe_code)]
    let ppid = unsafe { libc::getppid() };
    obj.insert(
        "bowerbird_ppid".to_string(),
        serde_json::Value::Number(serde_json::Number::from(ppid)),
    );

    let mut wire = serde_json::to_vec(&value).map_err(Error::StdinJson)?;
    wire.push(b'\n');

    match socket::send(&sock_path, &wire)? {
        socket::Response::Ok => Ok(()),
        socket::Response::Backpressure => Err(Error::Backpressure503),
        socket::Response::DaemonError(reason) => Err(Error::DaemonError400(reason)),
    }
}

fn parse_hook_kind() -> Result<String> {
    let mut args = std::env::args_os().skip(1);
    let mut hook_kind: Option<String> = None;

    while let Some(arg) = args.next() {
        if arg == "--hook-kind" {
            let Some(val) = args.next() else {
                return Err(Error::BadArgs("--hook-kind requires a value".into()));
            };
            let s = val.to_string_lossy().into_owned();
            hook_kind = Some(s);
        } else if let Some(rest) = arg.to_str().and_then(|s| s.strip_prefix("--hook-kind=")) {
            hook_kind = Some(rest.to_string());
        } else {
            return Err(Error::BadArgs(format!(
                "unknown argument: {}",
                arg.to_string_lossy()
            )));
        }
    }

    let kind = hook_kind.ok_or_else(|| Error::BadArgs("--hook-kind not provided".into()))?;
    match kind.as_str() {
        "UserPromptSubmit" | "PreToolUse" | "PostToolUse" | "Stop" | "Notification" => Ok(kind),
        other => Err(Error::BadArgs(format!("invalid hook-kind: {other}"))),
    }
}

fn resolve_log_path() -> Result<PathBuf> {
    if let Some(p) = std::env::var_os("BOWERBIRD_SHIM_LOG") {
        let p = PathBuf::from(p);
        if !p.as_os_str().is_empty() {
            return Ok(p);
        }
    }
    let home = home_env()?;
    Ok(Path::new(&home).join(".bowerbird/shim.log"))
}

fn resolve_sock_path() -> Result<PathBuf> {
    if let Some(p) = std::env::var_os("BOWERBIRD_INGEST_SOCK") {
        let p = PathBuf::from(p);
        if !p.as_os_str().is_empty() {
            return Ok(p);
        }
    }
    let home = home_env()?;
    Ok(Path::new(&home).join(".bowerbird/ingest.sock"))
}

fn home_env() -> Result<OsString> {
    match std::env::var_os("HOME") {
        Some(h) if !h.is_empty() => Ok(h),
        _ => Err(Error::NoHome),
    }
}

fn read_stdin_capped() -> Result<Vec<u8>> {
    // Read one byte past the cap so we can distinguish "payload fits" from
    // "payload was truncated to the cap." `take()` alone reports neither;
    // it just stops at the limit.
    let stdin = std::io::stdin();
    let mut handle = stdin.lock().take((MAX_STDIN_BYTES as u64) + 1);
    let mut buf = Vec::with_capacity(4096);
    handle.read_to_end(&mut buf).map_err(Error::Stdin)?;
    if buf.len() > MAX_STDIN_BYTES {
        return Err(Error::StdinTooLarge {
            cap_bytes: MAX_STDIN_BYTES,
        });
    }
    Ok(buf)
}
