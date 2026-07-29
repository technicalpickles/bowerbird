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

    /// An expired `SO_SNDTIMEO` / `SO_RCVTIMEO` on the ingest round-trip
    /// (Story 5.16). Distinct from [`Error::SocketIo`] so a dropped event is
    /// diagnosable: the operator can tell "the daemon did not answer inside
    /// the budget" from "the socket genuinely failed", which the shared
    /// `Error::SocketIo` bucket made impossible.
    ///
    /// Names the operation and the budget it blew rather than restating the
    /// errno, because the errno is the least informative part — see
    /// `socket::classify` for why the platform spelling is worthless here.
    ///
    /// Both fields are `&'static str` / `u64` — no allocation, per the shim's
    /// hot-path discipline.
    #[error("socket {op} timed out after {budget_ms}ms; event dropped")]
    Timeout { op: &'static str, budget_ms: u64 },

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
            //
            // `Timeout` belongs here and NOT with the exit-1 class: the connect
            // succeeded, so the daemon is up — it just did not answer inside the
            // budget (Story 5.16 Task 1 traced this to the daemon's runtime
            // thread being starved by the OS scheduler under heavy load, not to
            // the daemon being down or broken). Claude must still see success.
            Error::SocketIo(_)
            | Error::Timeout { .. }
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

    /// Human-readable cause for the one-line stderr hint `main` emits on the
    /// failure path. `Some(<cause>)` for every exit-1 (ERROR-level) variant,
    /// `None` for every exit-0 (WARN-level) variant.
    ///
    /// The `None` for the WARN/exit-0 class is **by contract**: those errors
    /// mean the daemon is up and answering (mid-write hiccup, backpressure, a
    /// daemon-side 400). Per NFR20 the shim is fire-and-forget there, so Claude
    /// must see success — surfacing a warning on stderr would regress that.
    ///
    /// The match arms are kept in the SAME order as `exit_code()` so the
    /// exit-1/exit-0 partition can be diffed side by side; the
    /// `stderr_hint_matches_exit_code` test below is the canary that a new
    /// variant cannot be added without a deliberate hint decision.
    ///
    /// All cause strings are `&'static str` — no allocation.
    pub fn stderr_hint(&self) -> Option<&'static str> {
        match self {
            // exit-1 (ERROR) class → name the cause
            Error::Stdin(_) => Some("could not read hook payload from stdin"),
            Error::StdinEmpty => Some("empty hook payload"),
            Error::StdinNotJsonObject => Some("hook payload was not a JSON object"),
            Error::StdinTooLarge { .. } => Some("hook payload exceeds size cap"),
            Error::StdinJson(_) => Some("hook payload was not valid JSON"),
            Error::Connect { .. } => Some("daemon not running, event dropped"),
            Error::LogIo(_) => Some("could not write shim log"),
            Error::BadArgs(_) => Some("invalid shim arguments"),
            Error::NoHome => Some("HOME not set, cannot record event"),

            // exit-0 (WARN) class → silent by contract (NFR20: daemon is up and
            // answering, fire-and-forget, Claude must see success)
            //
            // `Timeout` is silent here on purpose. Story 5.16 is a
            // diagnosability story, and the surface it improves is the shim
            // LOG LINE, not stderr — a timeout means the daemon answered the
            // connect, so putting it on stderr would regress Story 5.10's
            // exit-1/exit-0 partition and surface a hook warning to Claude for
            // an event loss Claude can do nothing about.
            Error::SocketIo(_)
            | Error::Timeout { .. }
            | Error::BadResponse(_)
            | Error::Backpressure(_)
            | Error::Backpressure503
            | Error::DaemonError400(_) => None,
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
            // Both operations, so the partition canaries cover each spelling of
            // the Story 5.16 timeout rather than just whichever came first.
            Error::Timeout {
                op: "write",
                budget_ms: 2,
            },
            Error::Timeout {
                op: "read",
                budget_ms: 3,
            },
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

    #[test]
    fn stderr_hint_matches_exit_code() {
        // The stderr-hint partition MUST track the exit-code partition exactly:
        // a hint iff the error exits 1, no hint iff it exits 0. This is the
        // canary against a future variant getting a hint (or no hint) by
        // accident — mirrors `exit_code_never_2` / `level_matches_exit_code`.
        for e in sample_variants() {
            assert_eq!(
                e.stderr_hint().is_some(),
                e.exit_code() == 1,
                "exit-1 variants must have a stderr hint: {e:?}"
            );
            assert_eq!(
                e.stderr_hint().is_none(),
                e.exit_code() == 0,
                "exit-0 variants must NOT have a stderr hint: {e:?}"
            );
        }
    }

    #[test]
    fn connect_hint_names_the_daemon_down_cause() {
        // Pins the dogfood-relevant wording (Finding 2): the daemon-down line
        // Claude surfaces instead of its causeless "No stderr output".
        let e = Error::Connect {
            path: PathBuf::from("/tmp/nope.sock"),
            source: dummy_io(),
        };
        assert_eq!(e.stderr_hint(), Some("daemon not running, event dropped"));
    }
}
