use std::backtrace::Backtrace;
use std::fs::OpenOptions;
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

fn crash_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".bowerbird"))
}

fn write_crash_file(message: &str) -> Option<PathBuf> {
    let dir = crash_dir()?;
    if std::fs::create_dir_all(&dir).is_err() {
        return None;
    }
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let path = dir.join(format!("crash-{ts}.log"));
    let mut opts = OpenOptions::new();
    opts.create_new(true).write(true).mode(0o600);
    let mut f = opts.open(&path).ok()?;
    f.write_all(message.as_bytes()).ok()?;
    Some(path)
}

/// Write a crash report for a top-level unhandled error (i.e. a `Result::Err`
/// returned from the startup orchestrator). Best-effort: returns `None` if
/// `HOME` is unset, the directory can't be created, or the file can't be
/// written. AC #8 covers both panics (via the panic hook) and unhandled-error
/// exits — this function is the unhandled-error half.
pub fn write_error_report(message: &str) -> Option<PathBuf> {
    let bt = Backtrace::capture();
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let report = format!(
        "timestamp_ms: {ts}\nkind: unhandled_error\nmessage: {message}\nbacktrace:\n{bt}\n"
    );
    write_crash_file(&report)
}

pub fn install_panic_hook() {
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let payload = info.payload();
        let msg = payload
            .downcast_ref::<&'static str>()
            .map(|s| (*s).to_owned())
            .or_else(|| payload.downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "<non-string panic payload>".to_owned());
        let location = info
            .location()
            .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
            .unwrap_or_else(|| "<unknown location>".to_owned());
        let bt = Backtrace::capture();
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0);

        let report = format!(
            "timestamp_ms: {ts}\nkind: panic\nlocation: {location}\nmessage: {msg}\nbacktrace:\n{bt}\n"
        );

        // Best-effort; hook MUST NOT panic.
        let _ = std::panic::catch_unwind(|| {
            let _ = write_crash_file(&report);
        });

        prev(info);
    }));
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    // Serialize tests that mutate the process-global HOME env var.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn write_error_report_creates_file_under_home_bowerbird() {
        let _guard = ENV_LOCK.lock().unwrap();
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let prev_home = std::env::var_os("HOME");
        // SAFETY: serialized via ENV_LOCK above.
        std::env::set_var("HOME", tmp.path());

        let path =
            write_error_report("test error: missing config").expect("crash file should be written");
        assert!(path.starts_with(tmp.path().join(".bowerbird")));
        assert!(path.exists());

        let contents = std::fs::read_to_string(&path).expect("read report");
        assert!(contents.contains("kind: unhandled_error"));
        assert!(contents.contains("test error: missing config"));

        if let Some(h) = prev_home {
            std::env::set_var("HOME", h);
        } else {
            std::env::remove_var("HOME");
        }
    }

    #[test]
    fn write_error_report_returns_none_when_home_unset() {
        let _guard = ENV_LOCK.lock().unwrap();
        let prev_home = std::env::var_os("HOME");
        std::env::remove_var("HOME");

        let result = write_error_report("any");
        assert!(result.is_none());

        if let Some(h) = prev_home {
            std::env::set_var("HOME", h);
        }
    }
}
