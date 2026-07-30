use std::path::PathBuf;

use thiserror::Error;

/// Which half of the ingest round-trip blew its budget.
///
/// This exists to carry the **consequence for the event**, which differs
/// between the two halves and is the whole point of Story 5.16's log line. The
/// first cut of that line said "event dropped" for both, which was false for
/// the read case and therefore worse than the vague message it replaced: an
/// operator would go hunting for an event that is actually present.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimeoutOp {
    /// The payload did not fully reach the daemon, so the event is lost, with
    /// one boundary case noted below.
    ///
    /// The daemon frames on a trailing `\n`. A truncated payload leaves it
    /// parsing an incomplete JSON object, which fails and is logged as
    /// `ingest: invalid JSON` while the daemon replies `400`. (It is *not*
    /// `ingest: EOF before newline`, which only fires when zero bytes arrived.)
    ///
    /// **The one case where "not sent" is too strong:** if the write stopped with
    /// only the final `\n` unsent, the daemon reads a complete JSON object,
    /// `trim_end_matches('\n')` is a no-op, validation passes, and the event IS
    /// recorded. That is one byte out of N, and no other prefix of a real payload
    /// parses as valid JSON, so the `Display` string keeps the simpler wording.
    Write,
    /// The payload landed and only the acknowledgement was lost. The daemon
    /// enqueues via `try_send` *before* it writes `200\n`
    /// (`daemon/src/ingest/handler.rs`), and it reads the payload out of the
    /// kernel buffer whether or not the shim is still waiting, so by the time
    /// this fires the event has almost certainly been recorded.
    Read,
}

impl TimeoutOp {
    fn name(self) -> &'static str {
        match self {
            TimeoutOp::Write => "write",
            TimeoutOp::Read => "read",
        }
    }

    /// What the timeout means for the event. Distinguishing "lost" from
    /// "unacknowledged" is the diagnosability contract; do not collapse these
    /// back into one phrase.
    fn consequence(self) -> &'static str {
        match self {
            TimeoutOp::Write => "event not sent",
            TimeoutOp::Read => "no reply from daemon, event may already be recorded",
        }
    }
}

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

    /// The ingest round-trip blew its time budget (Story 5.16). Distinct from
    /// [`Error::SocketIo`] so a lost event is diagnosable: the operator can
    /// tell "the daemon did not answer inside the budget" from "the socket
    /// genuinely failed", which the shared `Error::SocketIo` bucket made
    /// impossible.
    ///
    /// Names the operation, the budget it blew, and **what that means for the
    /// event**, see [`TimeoutOp`], and note the two halves have opposite
    /// consequences. Deliberately does not restate the errno, which is the
    /// least informative part; see `socket::classify`.
    ///
    /// **`budget_ms` is the per-wait socket timeout, not a bound on the
    /// operation.** The elapsed time when this fires can be far larger, because
    /// neither `write_all`/`read_line` nor even a single `write(2)` is bounded by
    /// `SO_*TIMEO` once the peer is draining slowly. See
    /// `socket::WRITE_BUDGET_MS` for measurements and Story 5.17 for the fix. So
    /// this number tells the operator which budget was configured, not how long
    /// the shim actually blocked.
    ///
    /// Both fields are `Copy`, no allocation, per the shim's hot-path
    /// discipline.
    #[error("socket {} timed out after {budget_ms}ms; {}", .op.name(), .op.consequence())]
    Timeout { op: TimeoutOp, budget_ms: u64 },

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
            // `Timeout` belongs here and NOT with the exit-1 class, and the
            // reason is NFR20 alone: the shim is fire-and-forget, so a hook that
            // could not record its event must still let Claude proceed.
            //
            // Two things are deliberately NOT claimed, because both are false.
            // A successful connect does not prove the daemon is alive or
            // scheduled, only that a listener FD exists (see the connect
            // discussion in `socket::send`) - Story 5.16 traced these timeouts to
            // the daemon's thread being starved, which is exactly a healthy
            // listener with nothing running behind it. And the payload is not
            // necessarily "handed over": on a WRITE timeout it is truncated and
            // the daemon rejects it (see `TimeoutOp::Write`). Exit-0 is right
            // regardless, but it rests on NFR20, not on either of those.
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
            // LOG LINE, not stderr. Putting it on stderr would regress Story
            // 5.10's exit-1/exit-0 partition and surface a hook warning to
            // Claude for an event loss Claude can do nothing about.
            //
            // Known consequence, accepted: a wedged daemon that still holds its
            // listener FD swallows every event at exit-0 with empty stderr, and
            // only the shim log shows it. That is the NFR20 trade, not an
            // oversight, see taskwarrior 719e7027.
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
            // Both ops are listed for the *message* tests below, not for the
            // partition canaries: `exit_code()`/`stderr_hint()` match
            // `Error::Timeout { .. }` and discard `op`, so one entry would give
            // identical partition coverage. Do not read this as `op` being
            // partition-relevant.
            Error::Timeout {
                op: TimeoutOp::Write,
                budget_ms: 2,
            },
            Error::Timeout {
                op: TimeoutOp::Read,
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

    /// Story 5.16 review finding (HIGH): the two timeout halves have OPPOSITE
    /// consequences for the event, and the log line must not blur them.
    ///
    /// A read timeout fires *after* the daemon has already `try_send`-ed the
    /// event, so claiming "event dropped" there sends the operator hunting for
    /// an event that is actually present, which is worse than the vague
    /// `socket I/O failed` message this story replaced. `Error::Connect` owns
    /// the "event dropped" phrasing, where the drop is real.
    #[test]
    fn timeout_consequence_distinguishes_lost_from_unacknowledged() {
        let write = Error::Timeout {
            op: TimeoutOp::Write,
            budget_ms: 2,
        }
        .to_string();
        let read = Error::Timeout {
            op: TimeoutOp::Read,
            budget_ms: 3,
        }
        .to_string();

        // The write half is a genuine loss: the daemon frames on a trailing
        // newline and discards a line it never finished reading.
        assert!(
            write.contains("event not sent"),
            "write timeout must say the event never went out: {write:?}"
        );

        // The read half is NOT a loss, and must not imply one.
        assert!(
            read.contains("may already be recorded"),
            "read timeout must say delivery is unconfirmed, not lost: {read:?}"
        );
        assert!(
            !read.contains("event dropped"),
            "read timeout must NOT claim the event was dropped, the daemon \
             enqueues before it replies, so the event has almost certainly \
             landed: {read:?}"
        );
        assert!(
            !read.contains("not sent"),
            "read timeout must not claim the event was never sent: {read:?}"
        );

        // And the two must not read identically, or the distinction is lost.
        assert_ne!(write, read);
    }

    /// The phrase `event dropped` stays reserved for the one case where the
    /// event really is gone, so it remains a reliable signal.
    ///
    /// It lives in exactly one place, `Connect`'s stderr hint, where the daemon
    /// is genuinely unreachable. No *log line* may use it, because `Display` is
    /// what the shim log carries and a read timeout is not a drop.
    #[test]
    fn event_dropped_phrasing_is_reserved_for_real_drops() {
        let connect = Error::Connect {
            path: PathBuf::from("/tmp/nope.sock"),
            source: dummy_io(),
        };
        assert_eq!(
            connect.stderr_hint(),
            Some("daemon not running, event dropped"),
            "the reserved phrase should still be here, on the one real drop"
        );

        for e in sample_variants() {
            assert!(
                !e.to_string().contains("event dropped"),
                "no log line may use the reserved 'event dropped' phrasing, \
                 each variant must describe its own outcome: {e:?}"
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
