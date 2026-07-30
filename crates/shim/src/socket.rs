use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::Duration;

use crate::error::{Error, Result, TimeoutOp};

pub(crate) enum Response {
    Ok,
    Backpressure,
    DaemonError(String),
}

/// Socket timeout values for the ingest round-trip.
///
/// Single-sourced as `u64` milliseconds so the `Duration` handed to the socket
/// and the number in the [`Error::Timeout`] message cannot drift apart.
///
/// **Read these as per-wait values, NOT as a bound on the round-trip.** Getting
/// this wrong has now cost two review passes, so the true shape, measured:
///
/// `SO_SNDTIMEO` / `SO_RCVTIMEO` bound how long the kernel will *wait* for
/// socket-buffer space or data. They do not bound a syscall that keeps making
/// progress, and they do not bound a loop of syscalls:
///
/// * **A single `write(2)` can far exceed its timeout.** macOS's `sosend` loop
///   re-waits per buffer refill, so as long as the peer drains *anything* the
///   call keeps going. Measured with a 1 MiB payload against a peer draining
///   8 KiB at a time and this socket armed at 2ms: **40ms** (peer at 200µs
///   intervals), **97ms** (500µs), **189ms** (1000µs), each returning `Ok` from
///   ONE `write(2)`.
/// * **`write_all` compounds it**, since each partial write starts a fresh wait.
/// * **`read_line` has the same shape**, looping `fill_buf` until it sees `\n`;
///   measured at 12ms and 24ms against this 3ms value with a dribbling peer.
///
/// So the honest statement is: each *wait* is bounded at 2ms/3ms, the aggregate
/// is **not** bounded, and `connect` sits outside both. The worst case is
/// therefore proportional to payload size and to how slowly the peer drains, up
/// to the 1 MiB `main.rs::MAX_STDIN_BYTES` cap against an 8 KiB
/// `net.local.stream.sendspace`.
///
/// That is a real Axiom 3 trust-boundary exposure and it is **not fixed here**.
/// An attempt to bound it inside Story 5.16 was backed out after measurement
/// showed it did not work (the loop never iterated, so its deadline check was
/// unreachable), and it is tracked as its own story with the measurements
/// attached, rather than being retried as a rider on a diagnosability hotfix:
/// see `docs/bmad/implementation-artifacts/5-17-shim-write-budget-is-not-a-bound.md`.
/// Do not add a bound here without reading that story's measurements first.
const WRITE_BUDGET_MS: u64 = 2;
const READ_BUDGET_MS: u64 = 3;

/// Classify a failed socket read/write: an expired `SO_SNDTIMEO` /
/// `SO_RCVTIMEO` becomes [`Error::Timeout`]; anything else stays
/// [`Error::SocketIo`].
///
/// **Why both `WouldBlock` and `TimedOut` map to a timeout.** `std` does not
/// pin which kind an expired socket timeout produces, it documents *either*
/// for a timed-out read/write, so matching both is coding to the documented
/// contract rather than to one platform's observed errno.
///
/// What is actually verified here (Story 5.16 Task 1): on **macOS**, an expired
/// `SO_RCVTIMEO` yields `ErrorKind::WouldBlock` with `raw_os_error == Some(35)`
/// (`EAGAIN`), whose `Display` is `Resource temporarily unavailable (os error
/// 35)`, exactly the line the rc1 dogfood logged, which is how two dropped
/// events came to look like generic I/O failures. The same holds for an expired
/// `SO_SNDTIMEO`.
///
/// Note the story spec described `TimedOut` as "the Linux spelling", which is
/// misleading: POSIX specifies `EAGAIN` for `SO_RCVTIMEO`/`SO_SNDTIMEO` expiry
/// on both supported platforms, so Unix normally lands on `WouldBlock` (macOS
/// errno 35, Linux errno 11). The `TimedOut` arm is therefore not the Linux
/// counterpart to a macOS quirk. It is kept for a better reason: `std` reserves
/// the right to produce either kind, and being wrong in that direction would
/// silently restore the undiagnosable behavior this story exists to remove. One
/// extra pattern, one whole class of silent regression avoided.
///
/// **Why `WouldBlock` is unambiguous here.** On a *non-blocking* socket
/// `WouldBlock` means "no data yet, try again", but this socket is never set
/// non-blocking. With `SO_*TIMEO` set on a blocking socket, `WouldBlock` can
/// only mean the budget expired. If a future change makes this socket
/// non-blocking, this classification stops being sound and must be revisited.
fn classify(op: TimeoutOp, budget_ms: u64, e: std::io::Error) -> Error {
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

    // What these two values actually bound, stated precisely, because two
    // successive versions of this comment got it wrong and each wrong version
    // hid a real stall (Story 5.16 AC #4, and its pass-2 review):
    //
    //   * each WAIT for send-buffer space  ≤ 2ms
    //   * each WAIT for reply data         ≤ 3ms
    //   * the write as a whole             NOT bounded
    //   * the read as a whole              NOT bounded
    //   * connect                          NOT bounded, and outside both
    //
    // A single `write(2)` keeps going while the peer drains anything, so it can
    // exceed 2ms by orders of magnitude (measured: 189ms for 1 MiB against a
    // peer draining 8 KiB per millisecond, returning Ok from one syscall).
    // `write_all` and `read_line` both loop, compounding it. See
    // WRITE_BUDGET_MS for the full measurement table and for the story that
    // owns fixing it; do not restate a total here without measuring one.
    //
    // The connect exclusion is a measured judgement, not an oversight, but it
    // has a precondition worth stating: a Unix-socket connect completes in the
    // kernel as soon as the connection lands in the accept backlog, so *while
    // the backlog has room* it does not wait for the daemon to call `accept` and
    // does not depend on the daemon being scheduled. Measured: under the load
    // that pushed the ack tail to 5.7ms, connect's worst case stayed at 337µs
    // (p50 39µs).
    //
    // Where that precondition fails, a backlog filled to `somaxconn`, i.e. a
    // wedged daemon, the guarantee is gone: on Linux a blocking connect waits
    // on the socket's send timeout, which is still 0 (infinite) because we arm
    // it only after connect returns, so that case is an unbounded wait; macOS
    // returns ECONNREFUSED instead, bounded but reported as `Connect` ("daemon
    // not running") while the daemon is in fact running. Reachability is low and
    // the measurement above cannot speak to it, since it was taken against a
    // responsive daemon. Tracked with the daemon-starvation work, taskwarrior
    // 719e7027.
    //
    // Bounding connect here was rejected on cost: `std` has no
    // `UnixStream::connect_timeout`, so it needs either a new shim dependency or
    // a hand-rolled non-blocking connect + poll + restore, which adds syscalls
    // to the SUCCESS path (the path that must stay invisible) and would break
    // `classify`'s blocking-socket premise.
    stream
        .set_write_timeout(Some(Duration::from_millis(WRITE_BUDGET_MS)))
        .map_err(Error::SocketIo)?;
    stream
        .set_read_timeout(Some(Duration::from_millis(READ_BUDGET_MS)))
        .map_err(Error::SocketIo)?;

    let mut write_stream = &stream;
    // A connected `UnixStream` is unbuffered, flush is a no-op syscall and is
    // intentionally skipped to keep the hot path tight.
    write_stream
        .write_all(wire_bytes)
        .map_err(|e| classify(TimeoutOp::Write, WRITE_BUDGET_MS, e))?;

    let mut reader = BufReader::with_capacity(64, &stream);
    let mut line = String::with_capacity(64);
    reader
        .read_line(&mut line)
        .map_err(|e| classify(TimeoutOp::Read, READ_BUDGET_MS, e))?;

    // `read_line` retains the trailing `\n`; trim before matching.
    let trimmed = line.trim_end_matches('\n').trim_end_matches('\r');

    if trimmed == "200" {
        Ok(Response::Ok)
    } else if trimmed == "503" {
        Ok(Response::Backpressure)
    } else if let Some(reason) = trimmed.strip_prefix("400 ") {
        Ok(Response::DaemonError(reason.to_string()))
    } else {
        // `trimmed`, not `line`: the raw line still carries its `\n`, which would
        // make `log::append` write two newlines for one event and break the
        // one-line-per-event framing the contract tests assert. Unreachable with
        // the current daemon (it emits only `200\n` / `503\n` / a
        // newline-sanitized `400 …`), but this is the one path in this function
        // that could violate that invariant.
        Err(Error::BadResponse(trimmed.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::ErrorKind;

    /// AC #6: the io::Error → `Error` classification, tested directly rather
    /// than by racing a real socket.
    ///
    /// Both kinds are matched because `std` does not pin which one an expired
    /// socket timeout produces (see `classify`'s doc); missing either would let
    /// a real timeout keep the old undiagnosable `SocketIo` wording.
    #[test]
    fn would_block_and_timed_out_both_classify_as_timeout() {
        for kind in [ErrorKind::WouldBlock, ErrorKind::TimedOut] {
            let e = classify(TimeoutOp::Read, READ_BUDGET_MS, std::io::Error::from(kind));
            match e {
                Error::Timeout { op, budget_ms } => {
                    assert_eq!(
                        op,
                        TimeoutOp::Read,
                        "op must survive classification: {kind:?}"
                    );
                    assert_eq!(budget_ms, READ_BUDGET_MS, "budget must survive: {kind:?}");
                }
                other => panic!("{kind:?} must classify as Timeout, got {other:?}"),
            }
        }
    }

    /// The macOS case as it actually arrives from the kernel: `EAGAIN` carried
    /// as a raw OS error, not a synthesized `ErrorKind`.
    ///
    /// macOS-gated and asserted unconditionally. Previously this was guarded by
    /// `if eagain.kind() == WouldBlock`, which meant it silently asserted
    /// nothing on Linux (errno 35 is `EDEADLK` there) and, worse, would have
    /// gone green if macOS ever stopped mapping errno 35 to `WouldBlock`, which
    /// is the premise this whole story rests on. Now that change is a failure.
    #[cfg(target_os = "macos")]
    #[test]
    fn raw_eagain_from_macos_classifies_as_timeout() {
        let eagain = std::io::Error::from_raw_os_error(35);
        assert_eq!(
            eagain.kind(),
            ErrorKind::WouldBlock,
            "macOS must map EAGAIN(35) to WouldBlock; Story 5.16's premise"
        );
        assert_eq!(
            eagain.to_string(),
            "Resource temporarily unavailable (os error 35)",
            "this Display is the exact rc1 dogfood log line"
        );
        assert!(matches!(
            classify(TimeoutOp::Read, READ_BUDGET_MS, eagain),
            Error::Timeout { .. }
        ));
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
            let e = classify(
                TimeoutOp::Write,
                WRITE_BUDGET_MS,
                std::io::Error::from(kind),
            );
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
            TimeoutOp::Write,
            WRITE_BUDGET_MS,
            std::io::Error::from(ErrorKind::WouldBlock),
        );
        let read = classify(
            TimeoutOp::Read,
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

    /// The timeout constants are the single source of truth for both the socket
    /// options and the message. Pin them so a change is a deliberate edit here
    /// (and, per AC #3/#5, a measured one).
    ///
    /// Deliberately does NOT assert `WRITE + READ == 5`, and there is
    /// deliberately no test here claiming an aggregate bound. Two earlier
    /// versions of this module asserted one: first a sum of constants (which
    /// says nothing about what the code enforces), then a wall-clock test whose
    /// peer was slower than the budget, so the write always failed on its first
    /// wait and the mechanism under test never ran. Both went green while the
    /// stall they were supposed to guard was live. A meaningful test here has to
    /// drive a peer that drains *faster* than the budget but still slowly, which
    /// is what Story 5.17 owns along with the fix.
    #[test]
    fn budgets_match_the_documented_contract() {
        assert_eq!(WRITE_BUDGET_MS, 2);
        assert_eq!(READ_BUDGET_MS, 3);
    }
}
