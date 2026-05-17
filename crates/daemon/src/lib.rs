pub mod api;
pub mod config;
pub mod db;
pub mod error;
pub mod ingest;
pub mod projection;
pub mod state;

pub use error::{Error, Result};

use std::path::{Path, PathBuf};
use std::sync::{OnceLock, RwLock};

/// Where the panic hook writes crash logs. Updated via [`set_crash_dir`].
static CRASH_DIR: OnceLock<RwLock<PathBuf>> = OnceLock::new();

/// Install the panic hook. May be called once at process startup; subsequent
/// installs are ignored, but the destination directory can be swapped later via
/// [`set_crash_dir`].
///
/// The hook chains the previous (default) hook so the standard panic message is
/// still printed to stderr regardless of file-write success — this matters when
/// `~/.bowerbird/` is missing, full, or unwritable. The hook itself runs inside
/// `catch_unwind` to prevent a panic-in-panic from aborting silently.
pub fn install_panic_hook(initial_crash_dir: PathBuf) {
    use std::panic;

    if CRASH_DIR.set(RwLock::new(initial_crash_dir)).is_err() {
        return;
    }

    let prev = panic::take_hook();
    panic::set_hook(Box::new(move |info| {
        let _ = panic::catch_unwind(panic::AssertUnwindSafe(|| {
            write_crash_log(info);
        }));
        prev(info);
    }));
}

/// Swap the crash-log destination directory. No-op if the panic hook has not
/// been installed yet.
pub fn set_crash_dir(new_dir: PathBuf) {
    if let Some(lock) = CRASH_DIR.get() {
        if let Ok(mut guard) = lock.write() {
            *guard = new_dir;
        }
    }
}

/// Write a crash log for a top-level unhandled error (i.e. a `Result::Err`
/// returned from the startup orchestrator). AC #8 covers both panics — handled
/// by the panic hook — and unhandled-error exits, which this function covers.
/// Best-effort: returns `None` if the crash directory is unset or the file
/// cannot be written.
pub fn write_error_report(message: &str) -> Option<PathBuf> {
    let dir = CRASH_DIR
        .get()
        .and_then(|l| l.read().ok().map(|g| g.clone()))?;
    write_error_to_dir(&dir, message)
}

fn write_error_to_dir(dir: &Path, message: &str) -> Option<PathBuf> {
    let path = dir.join(crash_filename());
    let backtrace = std::backtrace::Backtrace::capture();
    let body = format!("ERROR\n{message}\nBacktrace:\n{backtrace}\n");
    write_crash_file(&path, body.as_bytes()).ok()?;
    Some(path)
}

fn write_crash_log(info: &std::panic::PanicHookInfo<'_>) {
    let dir = match CRASH_DIR
        .get()
        .and_then(|l| l.read().ok().map(|g| g.clone()))
    {
        Some(d) => d,
        None => return,
    };
    let filename = crash_filename();
    let crash_path = dir.join(&filename);

    let backtrace = std::backtrace::Backtrace::capture();
    let location = info
        .location()
        .map(|l| l.to_string())
        .unwrap_or_else(|| "<unknown>".to_string());
    let payload = panic_payload_to_string(info);
    let body = format!(
        "PANIC at {}\n{}\nBacktrace:\n{}\n",
        location, payload, backtrace
    );

    if let Err(e) = write_crash_file(&crash_path, body.as_bytes()) {
        eprintln!(
            "warning: failed to write crash log to {}: {e}",
            crash_path.display()
        );
    }
}

fn crash_filename() -> String {
    use std::process;
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let pid = process::id();
    // Thread id is opaque; format it via Debug so the same panic from different
    // threads in the same nanosecond gets distinct filenames.
    let tid = format!("{:?}", std::thread::current().id());
    let tid = tid.replace(|c: char| !c.is_ascii_alphanumeric(), "");
    format!("crash-{nanos}-{pid}-{tid}.log")
}

fn panic_payload_to_string(info: &std::panic::PanicHookInfo<'_>) -> String {
    let payload = info.payload();
    if let Some(s) = payload.downcast_ref::<&str>() {
        return (*s).to_string();
    }
    if let Some(s) = payload.downcast_ref::<String>() {
        return s.clone();
    }
    format!(
        "<non-string panic payload, type_id {:?}>",
        payload.type_id()
    )
}

#[cfg(unix)]
fn write_crash_file(path: &Path, body: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut f = std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(path)?;
    f.write_all(body)?;
    Ok(())
}

#[cfg(not(unix))]
fn write_crash_file(path: &Path, body: &[u8]) -> std::io::Result<()> {
    std::fs::write(path, body)
}

/// Initialize the global tracing subscriber.
///
/// Verbosity mapping (matches AC#7): default = `error`, `-v` = `info`,
/// `-vv` = `debug`, `-vvv+` = `trace`. `RUST_LOG`, when set and non-empty,
/// takes precedence — but a malformed value is reported via stderr (one of
/// the sanctioned tracing-bootstrap exceptions to the no-eprintln rule) and
/// the verbosity-derived default is used instead.
pub fn init_tracing(verbosity: u8) {
    use tracing_subscriber::{fmt, prelude::*, EnvFilter};

    let level = match verbosity {
        0 => "error",
        1 => "info",
        2 => "debug",
        _ => "trace",
    };

    let filter = match std::env::var("RUST_LOG") {
        Ok(val) if !val.is_empty() => match EnvFilter::try_new(&val) {
            Ok(f) => f,
            Err(e) => {
                eprintln!(
                    "warning: RUST_LOG={val:?} could not be parsed ({e}); \
                     falling back to verbosity-derived default ({level})"
                );
                EnvFilter::new(level)
            }
        },
        _ => EnvFilter::new(level),
    };

    if let Err(e) = tracing_subscriber::registry()
        .with(filter)
        .with(
            fmt::layer()
                .with_target(false)
                .with_level(true)
                .with_ansi(false)
                .with_writer(std::io::stderr)
                .with_timer(fmt::time::ChronoUtc::rfc_3339()),
        )
        .try_init()
    {
        eprintln!("warning: failed to initialize tracing subscriber: {e}");
    }
}

/// Create `~/.bowerbird/` with mode 0700 if it does not already exist.
///
/// If the path already exists, leaves its permissions untouched. Refuses to
/// operate on a symlink (which `set_permissions` would follow) — chmod-via-
/// symlink is a privilege-escalation primitive we do not want to provide.
pub fn ensure_bowerbird_dir(path: &Path) -> std::io::Result<()> {
    use std::io::{Error, ErrorKind};

    match std::fs::symlink_metadata(path) {
        Ok(meta) => {
            if meta.file_type().is_symlink() {
                return Err(Error::new(
                    ErrorKind::InvalidInput,
                    format!("{} is a symlink; refusing to use it", path.display()),
                ));
            }
            if !meta.is_dir() {
                return Err(Error::new(
                    ErrorKind::InvalidInput,
                    format!("{} exists and is not a directory", path.display()),
                ));
            }
            // Already a real directory — leave perms as the operator set them.
            Ok(())
        }
        Err(e) if e.kind() == ErrorKind::NotFound => {
            std::fs::create_dir_all(path)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
            }
            Ok(())
        }
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_error_to_dir_creates_file_with_message() {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let path = write_error_to_dir(tmp.path(), "test error: missing config")
            .expect("crash file should be written");
        assert!(path.starts_with(tmp.path()));
        assert!(path.exists());
        let contents = std::fs::read_to_string(&path).expect("read report");
        assert!(contents.starts_with("ERROR\n"));
        assert!(contents.contains("test error: missing config"));
        assert!(contents.contains("Backtrace:"));
    }

    #[test]
    fn write_error_to_dir_returns_none_when_dir_missing() {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let missing = tmp.path().join("does-not-exist");
        assert!(write_error_to_dir(&missing, "anything").is_none());
    }
}
