use std::path::PathBuf;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("stdin read failed: {0}")]
    Stdin(#[source] std::io::Error),

    #[error("stdin was empty")]
    StdinEmpty,

    #[error("stdin payload was not a JSON object")]
    StdinNotJsonObject,

    #[error("stdin payload too large: exceeds {cap_bytes}-byte cap")]
    StdinTooLarge { cap_bytes: usize },

    #[error("stdin JSON parse failed: {0}")]
    StdinJson(#[source] serde_json::Error),

    #[error("connect to ingest socket {} failed: {source}", path.display())]
    Connect {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("socket I/O failed: {0}")]
    SocketIo(#[source] std::io::Error),

    #[error("log file I/O failed: {0}")]
    LogIo(#[source] std::io::Error),

    #[error("unexpected daemon response: {0}")]
    BadResponse(String),

    // Reserved for a future daemon wire variant that returns `503 <reason>\n`
    // (current daemon emits only `503\n`). Listed in the story spec so the
    // enum is forward-compatible without an enum-shape change.
    #[error("backpressure (daemon ingest queue full): {0}")]
    #[allow(dead_code)]
    Backpressure(String),

    #[error("daemon returned 503 (backpressure)")]
    Backpressure503,

    #[error("daemon returned 400: {0}")]
    DaemonError400(String),

    #[error("missing or invalid --hook-kind argument: {0}")]
    BadArgs(String),

    #[error("HOME environment variable not set and no overrides provided")]
    NoHome,
}

pub type Result<T> = std::result::Result<T, Error>;

impl Error {
    /// Process exit code to use when this error reaches `main`.
    ///
    /// Contract — must NEVER return 2 (exit 2 blocks Claude Code tool calls;
    /// the `exit_code_never_2` unit test below is the belt-and-suspenders
    /// gate against a future variant being added without thought).
    pub fn exit_code(&self) -> i32 {
        match self {
            // Daemon-unreachable or bad-input class → 1 (surfaces a real failure)
            Error::Stdin(_)
            | Error::StdinEmpty
            | Error::StdinNotJsonObject
            | Error::StdinTooLarge { .. }
            | Error::StdinJson(_)
            | Error::Connect { .. }
            | Error::LogIo(_)
            | Error::BadArgs(_)
            | Error::NoHome => 1,

            // Mid-write / daemon-responding-with-error class → 0
            // (fire-and-forget per NFR20: the daemon is up and answering)
            Error::SocketIo(_)
            | Error::BadResponse(_)
            | Error::Backpressure(_)
            | Error::Backpressure503
            | Error::DaemonError400(_) => 0,
        }
    }

    /// Log level for this error: "ERROR" for exit-1 variants, "WARN" for exit-0.
    pub fn level(&self) -> &'static str {
        if self.exit_code() == 0 {
            "WARN"
        } else {
            "ERROR"
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dummy_io() -> std::io::Error {
        std::io::Error::other("test")
    }

    fn dummy_json_err() -> serde_json::Error {
        serde_json::from_str::<serde_json::Value>("not json").unwrap_err()
    }

    fn sample_variants() -> Vec<Error> {
        vec![
            Error::Stdin(dummy_io()),
            Error::StdinEmpty,
            Error::StdinNotJsonObject,
            Error::StdinTooLarge { cap_bytes: 1 << 20 },
            Error::StdinJson(dummy_json_err()),
            Error::Connect {
                path: PathBuf::from("/tmp/nope.sock"),
                source: dummy_io(),
            },
            Error::SocketIo(dummy_io()),
            Error::LogIo(dummy_io()),
            Error::BadResponse("x".into()),
            Error::Backpressure("x".into()),
            Error::Backpressure503,
            Error::DaemonError400("x".into()),
            Error::BadArgs("x".into()),
            Error::NoHome,
        ]
    }

    #[test]
    fn exit_code_never_2() {
        for e in sample_variants() {
            assert_ne!(e.exit_code(), 2, "exit code 2 is forbidden: {e:?}");
        }
    }

    #[test]
    fn level_matches_exit_code() {
        for e in sample_variants() {
            let lvl = e.level();
            match e.exit_code() {
                0 => assert_eq!(lvl, "WARN", "exit=0 must be WARN: {e:?}"),
                1 => assert_eq!(lvl, "ERROR", "exit=1 must be ERROR: {e:?}"),
                other => panic!("unexpected exit code {other} for {e:?}"),
            }
        }
    }
}
