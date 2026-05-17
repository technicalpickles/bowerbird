use std::backtrace::Backtrace;
use std::fs::OpenOptions;
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

fn crash_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".bowerbird"))
}

fn write_crash_file(message: &str) {
    let Some(dir) = crash_dir() else { return };
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let path = dir.join(format!("crash-{ts}.log"));
    let mut opts = OpenOptions::new();
    opts.create_new(true).write(true).mode(0o600);
    let Ok(mut f) = opts.open(&path) else { return };
    let _ = f.write_all(message.as_bytes());
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

        let report =
            format!("timestamp_ms: {ts}\nlocation: {location}\nmessage: {msg}\nbacktrace:\n{bt}\n");

        // Best-effort; hook MUST NOT panic.
        let _ = std::panic::catch_unwind(|| write_crash_file(&report));

        prev(info);
    }));
}
