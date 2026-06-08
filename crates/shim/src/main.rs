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
    // log path could be resolved) and exit with the error's own exit code.
    // Route through the same `Error::stderr_hint()` / `exit_code()` machinery
    // the failure arm below uses so the cause and exit code are single-sourced
    // and guarded by the `stderr_hint_matches_exit_code` canary — never
    // hardcode the wording here, where it would silently drift from
    // `Error::NoHome`. Swallow the stderr write error exactly as the
    // failure-arm log append does (AC5).
    let log_path = match resolve_log_path() {
        Ok(p) => p,
        Err(e) => {
            if let Some(hint) = e.stderr_hint() {
                let _ = writeln!(io::stderr(), "bowerbird: {hint}");
            }
            std::process::exit(e.exit_code());
        }
    };

    match run(&log_path) {
        Ok(()) => std::process::exit(0),
        Err(e) => {
            // Swallow log failures: we're already failing, and crashing the
            // shim makes things worse than dropping the log line. Capture
            // whether the append actually landed: a failed append means the
            // log path is unusable (a directory, unwritable, ...), so the
            // `(see <log>)` pointer would send Claude to a file that was never
            // written. Drop the pointer in that case.
            let log_written = log::append(&log_path, e.level(), &e.to_string()).is_ok();
            // Name the cause on stderr for the exit-1 (ERROR) class so Claude
            // surfaces "bowerbird: <cause>" instead of its causeless
            // "No stderr output" hook error. The exit-0 (WARN) class returns
            // `None` by contract (NFR20: daemon up and answering → see success).
            // Swallow write errors (AC5): a failed stderr write never panics
            // and never changes the exit code, mirroring the log append above.
            if let Some(hint) = e.stderr_hint() {
                if log_written {
                    // The log path is `BOWERBIRD_SHIM_LOG` verbatim, which a
                    // hostile or odd environment could load with newlines or
                    // other control bytes. Escape them so the pointer cannot
                    // break the one-line hook message or inject terminal escape
                    // sequences into Claude's transcript.
                    let _ = writeln!(
                        io::stderr(),
                        "bowerbird: {hint} (see {})",
                        one_line_path(&log_path)
                    );
                } else {
                    let _ = writeln!(io::stderr(), "bowerbird: {hint}");
                }
            }
            std::process::exit(e.exit_code());
        }
    }
}

/// Render a unix path on a single line for the stderr hint. A unix path is a
/// bag of bytes that need not be valid UTF-8, and `BOWERBIRD_SHIM_LOG` is taken
/// verbatim, so a hostile or malformed value could carry bytes that break the
/// one-line hook message or spoof Claude's transcript. This sanitizes the raw
/// path bytes:
///   - C0/C1 controls and DEL (newline, CR, ANSI ESC, ...) → escaped
///   - Unicode line/paragraph separators (U+2028, U+2029) → escaped (they split
///     a line in Unicode-aware renderers even though `is_control()` is false)
///   - bidi format/override controls (LRM/RLM/ALM, the U+202A–U+202E
///     embeddings/overrides, the U+2066–U+2069 isolates) → escaped (they can
///     reorder/spoof the rendered text)
///   - bytes that are not valid UTF-8 → rendered as `\xNN`, so the pointer
///     reflects the real (if unprintable) path instead of a lossy U+FFFD
///
/// Printable text (including ordinary non-ASCII) is kept verbatim so legitimate
/// paths read naturally.
fn one_line_path(path: &Path) -> String {
    use std::os::unix::ffi::OsStrExt;
    let mut out = String::new();
    for chunk in path.as_os_str().as_bytes().utf8_chunks() {
        for c in chunk.valid().chars() {
            if needs_escape(c) {
                out.extend(c.escape_default());
            } else {
                out.push(c);
            }
        }
        for &b in chunk.invalid() {
            out.push_str(&format!("\\x{b:02x}"));
        }
    }
    out
}

/// True for chars that must not pass into the one-line stderr hint verbatim:
/// every control char plus the Unicode separators and bidi controls that
/// `char::is_control()` does not catch but that can still break or spoof the
/// single-line rendering.
fn needs_escape(c: char) -> bool {
    c.is_control()
        || matches!(
            c,
            '\u{2028}'              // LINE SEPARATOR
            | '\u{2029}'            // PARAGRAPH SEPARATOR
            | '\u{200E}'            // LEFT-TO-RIGHT MARK
            | '\u{200F}'            // RIGHT-TO-LEFT MARK
            | '\u{061C}'            // ARABIC LETTER MARK
            | '\u{202A}'..='\u{202E}'  // LRE RLE PDF LRO RLO
            | '\u{2066}'..='\u{2069}'  // LRI RLI FSI PDI
        )
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_line_path_passes_normal_paths_through() {
        // A normal path has no control chars, so the hint pointer is unchanged
        // (this is what keeps the connect-refused contract assertion exact).
        let p = Path::new("/var/folders/xy/shim.log");
        assert_eq!(one_line_path(p), "/var/folders/xy/shim.log");
    }

    #[test]
    fn one_line_path_escapes_control_chars() {
        // A newline (or other control byte) in BOWERBIRD_SHIM_LOG must not be
        // able to turn the one-line stderr hint into multiple lines.
        let p = Path::new("/tmp/foo\nbar\r\x1b[31m.log");
        let rendered = one_line_path(p);
        assert!(
            !rendered.contains('\n') && !rendered.contains('\r') && !rendered.contains('\x1b'),
            "control chars must be escaped, got: {rendered:?}"
        );
        assert_eq!(rendered, "/tmp/foo\\nbar\\r\\u{1b}[31m.log");
    }

    #[test]
    fn one_line_path_escapes_unicode_separators_and_bidi() {
        // `char::is_control()` is false for the Unicode line/paragraph
        // separators (U+2028, U+2029) and the bidi format/override controls
        // (e.g. U+202E RIGHT-TO-LEFT OVERRIDE). They are still legal UTF-8 bytes
        // in a unix filename, and they can split the hook line across renderers
        // or spoof Claude's transcript — so they must be escaped, not passed
        // through verbatim.
        let p = Path::new("/tmp/a\u{2028}b\u{2029}c\u{202e}d.log");
        let rendered = one_line_path(p);
        assert!(
            !rendered.contains('\u{2028}')
                && !rendered.contains('\u{2029}')
                && !rendered.contains('\u{202e}'),
            "unicode separators/bidi controls must be escaped, got: {rendered:?}"
        );
        assert_eq!(rendered, "/tmp/a\\u{2028}b\\u{2029}c\\u{202e}d.log");
    }

    #[test]
    fn one_line_path_renders_invalid_utf8_bytes() {
        // A unix path is a bag of bytes that need not be valid UTF-8.
        // `to_string_lossy` would replace the stray byte with U+FFFD, pointing
        // the user at a path they cannot open. Render the raw byte as `\xNN` so
        // the pointer reflects the real (if unprintable) path.
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt;
        let raw = b"/tmp/foo\xff\xfebar.log";
        let p = Path::new(OsStr::from_bytes(raw));
        assert_eq!(one_line_path(p), "/tmp/foo\\xff\\xfebar.log");
    }

    #[test]
    fn one_line_path_keeps_non_ascii_verbatim() {
        // Legitimate non-ASCII paths (e.g. a unicode username) read naturally.
        let p = Path::new("/Users/jürgen/.bowerbird/shim.log");
        assert_eq!(one_line_path(p), "/Users/jürgen/.bowerbird/shim.log");
    }
}
