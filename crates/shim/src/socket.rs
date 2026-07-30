use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::{Duration, Instant};

use crate::error::{Error, Result, TimeoutOp};

pub(crate) enum Response {
    Ok,
    Backpressure,
    DaemonError(String),
}

/// Aggregate budgets for the ingest round-trip: at most `WRITE_BUDGET_MS` in
/// total to hand the payload over, then at most `READ_BUDGET_MS` waiting for the
/// one-line ack.
///
/// Single-sourced as `u64` milliseconds so the `Duration` handed to the socket
/// and the number in the [`Error::Timeout`] message can never drift apart, a
/// log line claiming "timed out after 2ms" after an operation that actually
/// consumed tens of ms is worse than no message at all.
///
/// **These are aggregates, and that takes work to be true.** `SO_SNDTIMEO`
/// bounds a single `write(2)`, NOT a loop over several of them, so
/// `io::Write::write_all` is not bounded by it: every partial write that makes
/// progress gets a *fresh* budget. With the shim's 1 MiB stdin cap
/// (`main.rs::MAX_STDIN_BYTES`) against an 8 KiB Unix-stream send buffer
/// (`net.local.stream.sendspace`), a large `PostToolUse` payload could take
/// ~128 syscalls and stall Claude for hundreds of milliseconds while still
/// returning `Ok`. Measured before the fix: a 256 KiB payload to a slow-draining
/// peer took **82ms** through `write_all` and reported success, 16x the entire
/// nominal round-trip budget, inside Claude's hook, which is exactly the
/// trust-boundary stall Axiom 3 forbids. [`write_all_within`] is what makes the
/// aggregate claim real; do not swap it back for `write_all`.
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

/// Write the whole payload under a bound on **total** elapsed time.
///
/// This replaces `io::Write::write_all`, which cannot honor an aggregate budget:
/// `SO_SNDTIMEO` applies per `write(2)`, and `write_all` loops on every partial
/// success, so each syscall gets a fresh budget and the total is unbounded (see
/// [`WRITE_BUDGET_MS`] for the measured 82ms case). Here the socket is re-armed
/// with only the time *remaining* before each subsequent syscall, so the loop
/// cannot outlive the deadline.
///
/// **Costs nothing on the success path.** The common case is a payload smaller
/// than the socket send buffer: one `write` returns everything and the function
/// returns without re-arming. The extra `setsockopt` happens only when a write
/// came back partial, which is already the slow path.
///
/// The caller must have armed the socket with the full budget before calling.
fn write_all_within(stream: &UnixStream, mut buf: &[u8], deadline: Instant) -> Result<()> {
    loop {
        match (&*stream).write(buf) {
            // Wrote everything: the overwhelmingly common path, one syscall.
            Ok(n) if n >= buf.len() => return Ok(()),
            Ok(0) => {
                // Peer will not accept more and did not error. Not a timeout ,
                // keep it in the genuine-I/O bucket.
                return Err(Error::SocketIo(std::io::Error::from(
                    std::io::ErrorKind::WriteZero,
                )));
            }
            Ok(n) => {
                buf = &buf[n..];
                // Partial write. Re-arm with what is LEFT of the budget rather
                // than letting the next syscall start a fresh one.
                let left = deadline.saturating_duration_since(Instant::now());
                if left.is_zero() {
                    return Err(Error::Timeout {
                        op: TimeoutOp::Write,
                        budget_ms: WRITE_BUDGET_MS,
                    });
                }
                // A sub-microsecond `left` is safe: `std` rejects a zero
                // `Duration` (guarded above) and floors the resulting `timeval`
                // at 1µs, so this can never degrade into "no timeout".
                stream
                    .set_write_timeout(Some(left))
                    .map_err(Error::SocketIo)?;
            }
            // `write_all` retries `Interrupted`; match that so a signal does not
            // surface as a spurious failure.
            Err(ref e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(classify(TimeoutOp::Write, WRITE_BUDGET_MS, e)),
        }
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

    // What the two budgets actually bound, stated precisely, because the
    // previous version of this comment was wrong in a way that hid a real stall
    // for two stories (Story 5.16 AC #4):
    //
    //   * hand-off  ≤ 2ms  AGGREGATE, enforced by `write_all_within` re-arming
    //                      the socket with the remaining time after any partial
    //                      write. `SO_SNDTIMEO` alone would only bound each
    //                      `write(2)`; see WRITE_BUDGET_MS for the 82ms case
    //                      that proves the difference.
    //   * ack wait  ≤ 3ms  a single `recv`, so `SO_RCVTIMEO` bounds it directly.
    //   * connect          NOT bounded, and outside both numbers.
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

    // Deadline is taken AFTER the socket options are armed so the budget covers
    // the hand-off itself rather than setup syscalls.
    let write_deadline = Instant::now() + Duration::from_millis(WRITE_BUDGET_MS);
    // A connected `UnixStream` is unbuffered, flush is a no-op syscall and is
    // intentionally skipped to keep the hot path tight.
    write_all_within(&stream, wire_bytes, write_deadline)?;

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

    /// The budget constants are the single source of truth for both the socket
    /// options and the message. Pin them so a change is a deliberate edit here
    /// (and, per AC #3/#5, a measured one).
    ///
    /// Deliberately does NOT assert `WRITE + READ == 5`. The earlier version of
    /// this test did, which pinned the very claim that turned out to be false:
    /// a sum of constants says nothing about the aggregate the code enforces.
    /// `write_all_within_bounds_the_aggregate_not_each_syscall` is the test that
    /// actually has something to say about it.
    #[test]
    fn budgets_match_the_documented_contract() {
        assert_eq!(WRITE_BUDGET_MS, 2);
        assert_eq!(READ_BUDGET_MS, 3);
    }

    /// The regression guard for the review's HIGH finding: `write_all` gives each
    /// partial write a *fresh* `SO_SNDTIMEO`, so a payload larger than the socket
    /// send buffer can stall far past the budget and still return `Ok`. Measured
    /// at 82ms for 256 KiB against a slow-draining peer, inside Claude's hook,
    /// which Axiom 3 forbids.
    ///
    /// Drives a real slow-draining peer (deterministic: the payload cannot fit
    /// the 8 KiB send buffer, so partial writes are guaranteed) and asserts the
    /// aggregate is honored.
    ///
    /// Sized for a wide margin rather than a tight latency assertion, per the
    /// project's rule that timing assumptions which hold on a fast laptop are
    /// bugs. A 1 MiB payload (the actual `MAX_STDIN_BYTES` cap) against a peer
    /// draining 8 KiB every 2ms needs ~256ms to push through under the old
    /// `write_all`; the bounded version bails in single-digit ms. The 100ms
    /// ceiling sits an order of magnitude above the expected value and well
    /// below the regression, so a starved 4-vCPU runner cannot flip it.
    #[test]
    fn write_all_within_bounds_the_aggregate_not_each_syscall() {
        use std::io::Read;
        use std::os::unix::net::UnixListener;

        let dir = std::env::temp_dir().join(format!("bb516-agg-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("tmpdir");
        let sock = dir.join("s.sock");
        let _ = std::fs::remove_file(&sock);
        let listener = UnixListener::bind(&sock).expect("bind");

        let drain = std::thread::spawn(move || {
            let (mut s, _) = listener.accept().expect("accept");
            let mut buf = vec![0u8; 8192];
            // Drain slowly so the writer keeps making partial progress. This is
            // the starved-daemon shape, not a synchronization sleep.
            while let Ok(n) = s.read(&mut buf) {
                if n == 0 {
                    break;
                }
                std::thread::sleep(Duration::from_millis(2));
            }
        });

        let stream = UnixStream::connect(&sock).expect("connect");
        stream
            .set_write_timeout(Some(Duration::from_millis(WRITE_BUDGET_MS)))
            .expect("arm");

        // The real worst case: the shim's full 1 MiB stdin cap, ~128x the 8 KiB
        // send buffer.
        let payload = vec![b'x'; 1024 * 1024];
        let deadline = Instant::now() + Duration::from_millis(WRITE_BUDGET_MS);

        let started = Instant::now();
        let result = write_all_within(&stream, &payload, deadline);
        let elapsed = started.elapsed();

        // It must give up rather than grind through the whole payload.
        assert!(
            matches!(
                result,
                Err(Error::Timeout {
                    op: TimeoutOp::Write,
                    ..
                })
            ),
            "a payload that cannot be handed over inside the budget must report a \
             write timeout, got {result:?}"
        );

        let ceiling = Duration::from_millis(100);
        assert!(
            elapsed < ceiling,
            "write must stay near its {WRITE_BUDGET_MS}ms aggregate budget; took \
             {elapsed:?} (ceiling {ceiling:?}). If this fails, the aggregate bound \
             regressed, most likely write_all_within was replaced by write_all, \
             which gives every partial write a fresh SO_SNDTIMEO."
        );

        drop(stream);
        let _ = drain.join();
        let _ = std::fs::remove_dir_all(&dir);
    }
}
