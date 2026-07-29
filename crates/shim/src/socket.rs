use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::Duration;

use crate::error::{Error, Result};

pub(crate) enum Response {
    Ok,
    Backpressure,
    DaemonError(String),
}

/// Per-operation budgets for the ingest round-trip.
///
/// Single-sourced as `u64` milliseconds so the `Duration` handed to
/// `set_*_timeout` and the number in the [`Error::Timeout`] message can never
/// drift apart — a log line claiming "timed out after 3ms" while the socket was
/// actually set to something else would be worse than no message at all.
const WRITE_BUDGET_MS: u64 = 2;
const READ_BUDGET_MS: u64 = 3;

/// Classify a failed socket read/write: an expired `SO_SNDTIMEO` /
/// `SO_RCVTIMEO` becomes [`Error::Timeout`]; anything else stays
/// [`Error::SocketIo`].
///
/// **Why both `WouldBlock` and `TimedOut` map to a timeout.** The platform
/// spelling of "your socket timeout expired" is not portable: macOS reports it
/// as `EAGAIN` (errno 35) → [`std::io::ErrorKind::WouldBlock`], while Linux
/// reports `ETIMEDOUT` → [`std::io::ErrorKind::TimedOut`]. Story 5.16 Task 1
/// proved the macOS half directly — a peer that accepts and never replies
/// yields `ErrorKind::WouldBlock` / `raw_os_error == Some(35)`, whose `Display`
/// is `Resource temporarily unavailable (os error 35)`. That is exactly the line
/// the rc1 dogfood logged, which is how two dropped events came to look like
/// generic I/O failures. Matching on only one kind would leave the other
/// platform silently misclassified, so this is a cross-platform classification
/// rather than a macOS special case.
///
/// **Why `WouldBlock` is unambiguous here.** On a *non-blocking* socket
/// `WouldBlock` means "no data yet, try again" — but this socket is never set
/// non-blocking. With `SO_*TIMEO` set on a blocking socket, `WouldBlock` can
/// only mean the budget expired. If a future change makes this socket
/// non-blocking, this classification stops being sound and must be revisited.
fn classify(op: &'static str, budget_ms: u64, e: std::io::Error) -> Error {
    match e.kind() {
        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut => {
            Error::Timeout { op, budget_ms }
        }
        _ => Error::SocketIo(e),
    }
}

/// Synchronously send one wire payload to the daemon ingest socket and read
/// back the single-line status response.
///
/// `wire_bytes` MUST already include the trailing `\n` framing byte.
pub(crate) fn send(sock_path: &Path, wire_bytes: &[u8]) -> Result<Response> {
    let stream = UnixStream::connect(sock_path).map_err(|source| Error::Connect {
        path: sock_path.to_path_buf(),
        source,
    })?;

    // Tight per-op timeouts bound the two operations that wait on the DAEMON:
    // write + read ≤ 5ms. This is deliberately NOT a bound on the whole
    // function — the `connect` above is unbounded and sits outside the 5ms.
    //
    // That is a safe asymmetry rather than an oversight (Story 5.16 AC #4,
    // measured). A Unix-socket `connect` to a listening socket completes in the
    // kernel as soon as the connection lands in the accept backlog; it does not
    // wait for the daemon to call `accept`, so it does not depend on the daemon
    // being scheduled. The measurement bears this out: under the load that
    // pushed the reply-path tail to 5.7ms, connect's own worst case stayed at
    // 337µs (p50 39µs), because the read is the phase that needs the daemon's
    // thread to actually run and connect is not.
    //
    // Bounding it anyway was rejected on cost: `std` has no
    // `UnixStream::connect_timeout`, so it would take either a new dependency
    // in the shim or a hand-rolled non-blocking connect + poll + restore, which
    // adds syscalls to the SUCCESS path (the path that must stay invisible) to
    // guard a phase with no measured tail. It would also break `classify`'s
    // blocking-socket assumption. Correcting the claim is the honest fix; see
    // taskwarrior 719e7027 for the daemon-side starvation question.
    stream
        .set_write_timeout(Some(Duration::from_millis(WRITE_BUDGET_MS)))
        .map_err(Error::SocketIo)?;
    stream
        .set_read_timeout(Some(Duration::from_millis(READ_BUDGET_MS)))
        .map_err(Error::SocketIo)?;

    let mut write_stream = &stream;
    // `UnixStream::write_all` on a connected socket is unbuffered — flush is a
    // no-op syscall and is intentionally skipped to keep the hot path tight.
    write_stream
        .write_all(wire_bytes)
        .map_err(|e| classify("write", WRITE_BUDGET_MS, e))?;

    let mut reader = BufReader::with_capacity(64, &stream);
    let mut line = String::with_capacity(64);
    reader
        .read_line(&mut line)
        .map_err(|e| classify("read", READ_BUDGET_MS, e))?;

    // `read_line` retains the trailing `\n`; trim before matching.
    let trimmed = line.trim_end_matches('\n').trim_end_matches('\r');

    if trimmed == "200" {
        Ok(Response::Ok)
    } else if trimmed == "503" {
        Ok(Response::Backpressure)
    } else if let Some(reason) = trimmed.strip_prefix("400 ") {
        Ok(Response::DaemonError(reason.to_string()))
    } else {
        Err(Error::BadResponse(line))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::ErrorKind;

    /// AC #6: the io::Error → `Error` classification, tested directly rather
    /// than by racing a real socket. `WouldBlock` is the macOS spelling of an
    /// expired `SO_*TIMEO` and `TimedOut` is the Linux one; both must land on
    /// `Timeout` or one platform silently keeps the old undiagnosable behavior.
    #[test]
    fn would_block_and_timed_out_both_classify_as_timeout() {
        for kind in [ErrorKind::WouldBlock, ErrorKind::TimedOut] {
            let e = classify("read", READ_BUDGET_MS, std::io::Error::from(kind));
            match e {
                Error::Timeout { op, budget_ms } => {
                    assert_eq!(op, "read", "op must survive classification: {kind:?}");
                    assert_eq!(budget_ms, READ_BUDGET_MS, "budget must survive: {kind:?}");
                }
                other => panic!("{kind:?} must classify as Timeout, got {other:?}"),
            }
        }
    }

    /// The macOS case as it actually arrives from the kernel: `EAGAIN` carried
    /// as a raw OS error, not a synthesized `ErrorKind`. Story 5.16 Task 1
    /// verified this is what an expired `SO_RCVTIMEO` yields on macOS, and its
    /// `Display` is the exact string the rc1 dogfood logged.
    #[test]
    fn raw_eagain_from_macos_classifies_as_timeout() {
        let eagain = std::io::Error::from_raw_os_error(35);
        // Guard the premise: on a non-macOS host errno 35 is something else, so
        // only assert the mapping where the premise holds.
        if eagain.kind() == ErrorKind::WouldBlock {
            assert!(
                matches!(
                    classify("read", READ_BUDGET_MS, eagain),
                    Error::Timeout { .. }
                ),
                "raw EAGAIN must classify as Timeout on this platform"
            );
        }
    }

    /// The other half of the partition: a genuine socket failure must NOT be
    /// relabelled as a timeout. This is the regression guard against
    /// over-eager classification hiding real I/O errors.
    #[test]
    fn genuine_io_errors_stay_socket_io() {
        for kind in [
            ErrorKind::BrokenPipe,
            ErrorKind::ConnectionReset,
            ErrorKind::ConnectionRefused,
            ErrorKind::PermissionDenied,
            ErrorKind::UnexpectedEof,
        ] {
            let e = classify("write", WRITE_BUDGET_MS, std::io::Error::from(kind));
            assert!(
                matches!(e, Error::SocketIo(_)),
                "{kind:?} must stay SocketIo, got {e:?}"
            );
        }
    }

    /// AC #1/#2: the log line must name the expired budget instead of restating
    /// the errno. The old wording is what made the dogfood finding
    /// undiagnosable, so assert the new message does not fall back to it.
    #[test]
    fn timeout_message_names_the_operation_and_budget() {
        let write = classify(
            "write",
            WRITE_BUDGET_MS,
            std::io::Error::from(ErrorKind::WouldBlock),
        );
        let read = classify(
            "read",
            READ_BUDGET_MS,
            std::io::Error::from(ErrorKind::WouldBlock),
        );

        let w = write.to_string();
        let r = read.to_string();

        assert!(w.contains("write"), "must name the operation: {w:?}");
        assert!(w.contains("2ms"), "must name the write budget: {w:?}");
        assert!(r.contains("read"), "must name the operation: {r:?}");
        assert!(r.contains("3ms"), "must name the read budget: {r:?}");

        // The two must be distinguishable from each other AND from the generic
        // bucket they used to share.
        assert_ne!(w, r, "write and read timeouts must not read identically");
        for m in [&w, &r] {
            assert!(
                !m.contains("socket I/O failed"),
                "timeout must not reuse the generic SocketIo wording: {m:?}"
            );
            assert!(
                !m.contains("os error"),
                "timeout must name the budget, not restate the errno: {m:?}"
            );
        }
    }

    /// The budget constants are the single source of truth for both the
    /// `set_*_timeout` calls and the message. Pin them so a change is a
    /// deliberate edit here (and, per AC #3/#5, a measured one).
    #[test]
    fn budgets_match_the_documented_contract() {
        assert_eq!(WRITE_BUDGET_MS, 2);
        assert_eq!(READ_BUDGET_MS, 3);
        assert_eq!(
            WRITE_BUDGET_MS + READ_BUDGET_MS,
            5,
            "the socket.rs comment claims write + read <= 5ms"
        );
    }
}
