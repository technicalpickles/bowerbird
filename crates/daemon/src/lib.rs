pub mod api;
pub mod config;
pub mod db;
pub mod error;
pub mod projection;
pub mod state;

pub use error::{Error, Result};

use std::path::{Path, PathBuf};

pub fn install_panic_hook(crash_dir: PathBuf) {
    use std::panic;

    panic::set_hook(Box::new(move |info| {
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0);
        let crash_path = crash_dir.join(format!("crash-{}.log", now_ms));
        let backtrace = std::backtrace::Backtrace::capture();
        let location = info
            .location()
            .map(|l| l.to_string())
            .unwrap_or_else(|| "<unknown>".to_string());
        let payload = info
            .payload()
            .downcast_ref::<&str>()
            .copied()
            .map(|s| s.to_string())
            .or_else(|| info.payload().downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "<non-string panic payload>".to_string());
        let body = format!(
            "PANIC at {}\n{}\nBacktrace:\n{}\n",
            location, payload, backtrace
        );
        let _ = std::fs::write(&crash_path, body);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&crash_path, std::fs::Permissions::from_mode(0o600));
        }
    }));
}

pub fn init_tracing(verbosity: u8) {
    use tracing_subscriber::{fmt, prelude::*, EnvFilter};

    let level = match verbosity {
        0 => "error",
        1 => "info",
        2 => "debug",
        _ => "trace",
    };

    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(level));

    let _ = tracing_subscriber::registry()
        .with(filter)
        .with(
            fmt::layer()
                .with_target(false)
                .with_level(true)
                .with_ansi(false)
                .with_timer(fmt::time::ChronoUtc::rfc_3339()),
        )
        .try_init();
}

pub fn ensure_bowerbird_dir(path: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}
