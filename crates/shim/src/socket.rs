use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::{Duration, Instant};

use crate::error::{Error, Result, TimeoutOp};

#[derive(Debug)]
pub(crate) enum Response {
    Ok,
    Backpressure,
    DaemonError(String),
}

/// Aggregate time budgets for the two halves of the ingest round-trip, in
/// milliseconds. Single-sourced as `u64` so the `Duration`s handed to the
/// socket and the deadlines cannot drift from the numbers in the
/// [`Error::Timeout`] message.
///
/// **As of Story 5.17 these bound each half in aggregate**, not merely per
/// wait. Each value is used twice: armed as the socket's `SO_SNDTIMEO` /
/// `SO_RCVTIMEO` (bounding any single kernel wait), and enforced as a
/// deadline checked between syscalls in [`write_bounded`] / [`read_bounded`]
/// (bounding the loop). The residual overshoot is one trailing syscall past
/// the deadline check, and the two halves differ in how big that can get:
///
/// * **Read: one wait**, capped by the armed `SO_RCVTIMEO`, because a
///   `read(2)` returns on any available data. Worst case 3ms + 3ms.
/// * **Write: one trailing CHUNK**, whose duration is that chunk's drain
///   time under whatever refill quantum the peer frees. A peer freeing less
///   than a chunk per wakeup makes one capped `write(2)` span several
///   `SO_SNDTIMEO` waits (each refill restarts the wait), so the residual is
///   roughly `SEND_CHUNK_BYTES / quantum` waits. Measured in-tree
///   (`manual_repro_bounded_write_measurement_table`, macOS arm64,
///   2026-07-31): 1 MiB against an 8 KiB-quantum peer errors at
///   2.21/2.28/3.03ms for 200/500/1000µs intervals, but a 256-byte quantum
///   at 500µs stretched the trailing chunk to a measured **23.37ms** total.
///   The theoretical adversarial ceiling is a peer freeing one byte just
///   inside every 2ms wait (~8192 waits); no realistic daemon drains that
///   shape, but the bound must be stated as "budget + one chunk's drain
///   time", never as a constant.
///
/// **Multi-chunk payloads (over 8 KiB) are best-effort under this budget**,
/// and that is a deliberate Axiom 3 trade signed off in the Story 5.17
/// review: delivery needs the peer to drain nearly everything within 2ms.
/// Measured on the same run: an eagerly-drained (healthy, idle) daemon takes
/// 16 KiB / 100 KiB / 1 MiB in 30µs / 117µs / 909µs, all delivered; a peer
/// paced at one 8 KiB read per 200µs (the Story 5.16 p50 daemon-wakeup
/// figure, i.e. a busy machine) drops 100 KiB at 2.21ms. Note the deadline
/// is wall-clock: it also fires when the SHIM (not the daemon) is
/// descheduled between chunks, indistinguishable by design. Payloads at or
/// under one chunk (every normal hook event) keep the old single-syscall
/// delivery profile and are unaffected.
///
/// Why the deadline alone is not enough, and why each `write(2)` is also
/// capped at [`SEND_CHUNK_BYTES`]: `SO_SNDTIMEO` bounds how long the kernel
/// will *wait* for buffer space, not a syscall that keeps making progress.
/// macOS's `sosend` re-waits per buffer refill, so a single `write(2)` of a
/// large buffer keeps going as long as the peer drains *anything*. Measured
/// (Story 5.17, 1 MiB payload, peer draining 8 KiB per interval, socket armed
/// at 2ms): **40ms** at 200µs intervals, **97ms** at 500µs, **189ms** at
/// 1000µs, each returning `Ok` from ONE `write(2)`. A deadline checked
/// between syscalls never runs when the first syscall never returns, which is
/// exactly how Story 5.16's deadline-loop attempt failed. Linux's
/// `unix_stream_sendmsg` re-waits the same way and pushed 255,360 bytes
/// through one 2ms-armed syscall, so the chunk cap is load-bearing on both
/// platforms. Capping each syscall at the smallest send buffer in play keeps
/// every syscall short, which is what makes the between-syscalls deadline
/// real. The `#[ignore]`d `manual_repro_*` test below regenerates these
/// numbers from std's `write_all` on demand.
///
/// `connect` sits outside both budgets; see the comment in [`send`] for the
/// measured justification.
const WRITE_BUDGET_MS: u64 = 2;
const READ_BUDGET_MS: u64 = 3;

/// Cap on the number of bytes handed to any single `write(2)`.
///
/// Equal to macOS's `net.local.stream.sendspace` (8192), the smallest send
/// buffer in play. The cap bounds how much the kernel's internal re-wait
/// loop (see [`WRITE_BUDGET_MS`]) can transfer inside ONE syscall to one
/// chunk's worth, which is what makes the between-syscalls deadline
/// reachable at all; it does NOT limit the syscall to a single buffer-refill
/// wait. A peer freeing less than a chunk per wakeup makes one chunk span
/// several waits, so the trailing chunk past the deadline costs the chunk's
/// drain time (measured 23.37ms at a 256-byte/500µs drain; see
/// WRITE_BUDGET_MS for the honest residual statement). Linux's default
/// AF_UNIX buffer is larger, which only makes chunks cheaper there (more of
/// them fit without waiting); the drain-rate sweep test below runs on both
/// CI platforms, so the bound is measured rather than assumed on each (Story
/// 5.17 Task 1).
///
/// A normal hook payload (a few hundred bytes) is far below this cap, so the
/// success path is a single `write(2)` exactly as before; the
/// `success_path_is_one_syscall` test counts it.
const SEND_CHUNK_BYTES: usize = 8192;

/// Stack buffer for the daemon's single-line reply.
///
/// The longest line the daemon emits is `"400 "` plus 512 sanitized `char`s
/// (up to 4 UTF-8 bytes each; see
/// `daemon/src/ingest/handler.rs::sanitize_for_wire`) plus the trailing
/// newline, ≈ 2053 bytes. 4096 leaves headroom without touching the heap. A
/// reply that fills the buffer with no newline is a broken daemon and reports
/// as [`Error::BadResponse`].
const REPLY_BUF_BYTES: usize = 4096;

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
/// misleading. Both supported platforms were measured and both yield
/// `WouldBlock`: macOS `raw_os_error == Some(35)`, Linux (glibc)
/// `Some(11)`, i.e. `EAGAIN` either way, exactly as POSIX specifies for
/// `SO_RCVTIMEO`/`SO_SNDTIMEO` expiry. So `TimedOut` is not the Linux
/// counterpart to a macOS quirk; nothing we ship produces it. It is kept for a better reason: `std` reserves
/// the right to produce either kind, and being wrong in that direction would
/// silently restore the undiagnosable behavior this story exists to remove. One
/// extra pattern, one whole class of silent regression avoided.
///
/// **Why `WouldBlock` is unambiguous here.** On a *non-blocking* socket
/// `WouldBlock` means "no data yet, try again", but this socket is never set
/// non-blocking. With `SO_*TIMEO` set on a blocking socket, `WouldBlock` can
/// only mean the budget expired. If a future change makes this socket
/// non-blocking, this classification stops being sound and must be revisited.
fn classify(op: TimeoutOp, budget_ms: u64, elapsed: Duration, e: std::io::Error) -> Error {
    match e.kind() {
        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut => Error::Timeout {
            op,
            budget_ms,
            elapsed_ms: elapsed.as_millis() as u64,
        },
        _ => Error::SocketIo(e),
    }
}

/// Write `buf` in chunks of at most [`SEND_CHUNK_BYTES`], checking `budget`
/// against elapsed wall time before every syscall.
///
/// This is the Story 5.17 bound: the chunk cap keeps any single `write(2)`
/// from looping in the kernel past one chunk's worth (see
/// [`WRITE_BUDGET_MS`] for why an uncapped syscall makes any deadline
/// unreachable), and the deadline check between chunks keeps the loop from
/// outliving the budget. Worst case is therefore `budget` plus one trailing
/// chunk's DRAIN TIME, which depends on the peer's refill quantum (measured
/// up to 23.37ms at a pathological 256-byte/500µs drain; see
/// WRITE_BUDGET_MS); each in-chunk wait is separately capped by the
/// already-armed `SO_SNDTIMEO`.
///
/// A payload at or under [`SEND_CHUNK_BYTES`] is one chunk, so the success
/// path for a normal hook payload stays exactly one `write(2)` and its only
/// added cost is two `Instant` reads.
///
/// Generic over `W` so tests can wrap a real `UnixStream` in a counting
/// adapter and count syscalls instead of reasoning about them (AC #6);
/// production passes `&UnixStream` and monomorphizes to the direct calls.
fn write_bounded<W: Write>(w: &mut W, buf: &[u8], budget: Duration) -> Result<()> {
    let budget_ms = budget.as_millis() as u64;
    let start = Instant::now();
    let mut written = 0usize;
    while written < buf.len() {
        let elapsed = start.elapsed();
        if elapsed >= budget {
            return Err(Error::Timeout {
                op: TimeoutOp::Write,
                budget_ms,
                elapsed_ms: elapsed.as_millis() as u64,
            });
        }
        let end = (written + SEND_CHUNK_BYTES).min(buf.len());
        match w.write(&buf[written..end]) {
            Ok(0) => {
                return Err(Error::SocketIo(std::io::Error::new(
                    std::io::ErrorKind::WriteZero,
                    "write returned 0 bytes",
                )))
            }
            Ok(n) => written += n,
            // Interrupted is retried like `write_all` does, but through the
            // loop head, so even a stream of EINTRs cannot outlive the
            // deadline.
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => return Err(classify(TimeoutOp::Write, budget_ms, start.elapsed(), e)),
        }
    }
    Ok(())
}

/// Read the daemon's single-line reply into `buf`, checking `budget` against
/// elapsed wall time before every syscall. Returns the total number of bytes
/// consumed, which can include bytes past the first `\n` when they arrived in
/// the same `read(2)`; [`parse_reply`] does the line framing and ignores the
/// tail. (This differs from `read_line`, which stopped consuming at the
/// delimiter; the parity with the retired path is in what `send` ultimately
/// parses, not in what is read off the socket.)
///
/// The read half needs no chunk cap: a `read(2)` returns as soon as *any*
/// data is available, so a single syscall is already bounded by the armed
/// `SO_RCVTIMEO`; the unbounded shape lived in `read_line`'s refill loop
/// (measured 12ms and 24ms against the 3ms budget with a dribbling peer,
/// Story 5.17). The deadline between syscalls bounds that loop, so the worst
/// case is `budget` plus one wait.
///
/// EOF before a newline returns what arrived, matching `read_line`; the
/// caller parses it and a partial reply fails there as a bad response. A full
/// buffer with no newline is a broken daemon and reports as
/// [`Error::BadResponse`] rather than looping.
fn read_bounded<R: Read>(r: &mut R, buf: &mut [u8], budget: Duration) -> Result<usize> {
    let budget_ms = budget.as_millis() as u64;
    let start = Instant::now();
    let mut filled = 0usize;
    loop {
        let elapsed = start.elapsed();
        if elapsed >= budget {
            return Err(Error::Timeout {
                op: TimeoutOp::Read,
                budget_ms,
                elapsed_ms: elapsed.as_millis() as u64,
            });
        }
        if filled >= buf.len() {
            // A buffer's worth of reply with no newline is a broken daemon.
            // The buffer is newline-free by construction here (a newline
            // returns from the Ok(n) arm below), but it is up to 4 KiB of
            // arbitrary bytes; truncate to a short prefix so one broken reply
            // cannot bloat a log line by kilobytes.
            let prefix = String::from_utf8_lossy(&buf[..64.min(buf.len())]);
            return Err(Error::BadResponse(format!(
                "{prefix}... ({} bytes, no newline)",
                buf.len()
            )));
        }
        match r.read(&mut buf[filled..]) {
            Ok(0) => return Ok(filled),
            Ok(n) => {
                // Defensive clamp: a conforming Read cannot report more than
                // the slice it was given, but this function is generic and a
                // non-conforming impl must hit the overflow arm above, not an
                // out-of-range slice panic.
                let n = n.min(buf.len() - filled);
                let had_newline = buf[filled..filled + n].contains(&b'\n');
                filled += n;
                if had_newline {
                    return Ok(filled);
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => return Err(classify(TimeoutOp::Read, budget_ms, start.elapsed(), e)),
        }
    }
}

/// Parse the daemon's status line out of the raw reply bytes.
///
/// Only the bytes up to the first `\n` participate, mirroring the
/// `read_line` framing this replaced; a trailing `\r` is trimmed. A reply
/// that is not valid UTF-8 is a bad response (lossily rendered for the log),
/// not a socket I/O failure: the bytes arrived fine, the daemon spoke
/// garbage.
fn parse_reply(raw: &[u8]) -> Result<Response> {
    let line = match raw.iter().position(|&b| b == b'\n') {
        Some(pos) => &raw[..pos],
        None => raw,
    };
    let Ok(line) = std::str::from_utf8(line) else {
        return Err(Error::BadResponse(
            String::from_utf8_lossy(line).into_owned(),
        ));
    };
    let trimmed = line.trim_end_matches('\r');

    if trimmed.is_empty() {
        // The daemon accepted the connection and closed (or sent a bare
        // newline) without a status. Name that instead of logging
        // "unexpected daemon response: " with nothing after the colon.
        // Matches the daemon's EOF-before-newline and read-error paths, which
        // return without writing a reply (daemon/src/ingest/handler.rs).
        return Err(Error::BadResponse(
            "empty reply (daemon closed the connection without a status)".to_string(),
        ));
    }

    if trimmed == "200" {
        Ok(Response::Ok)
    } else if trimmed == "503" {
        Ok(Response::Backpressure)
    } else if let Some(reason) = trimmed.strip_prefix("400 ") {
        Ok(Response::DaemonError(reason.to_string()))
    } else {
        // `trimmed`, not the raw line: the raw bytes may carry the `\n`, which
        // would make `log::append` write two newlines for one event and break
        // the one-line-per-event framing the contract tests assert. Unreachable
        // with the current daemon (it emits only `200\n` / `503\n` / a
        // newline-sanitized `400 …`), but this is the one path in this function
        // that could violate that invariant.
        Err(Error::BadResponse(trimmed.to_string()))
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

    // What these two values bound (Story 5.17; the two previous shapes of
    // this comment each hid a real stall, so it stays precise):
    //
    //   * each WAIT for send-buffer space  ≤ 2ms  (SO_SNDTIMEO)
    //   * each WAIT for reply data         ≤ 3ms  (SO_RCVTIMEO)
    //   * the write as a whole             ≤ 2ms budget + one trailing chunk's
    //                                        DRAIN TIME (quantum-dependent;
    //                                        measured up to 23.37ms, see
    //                                        WRITE_BUDGET_MS)
    //   * the read as a whole              ≤ 3ms budget + one trailing wait
    //   * connect                          NOT bounded, and outside both
    //
    // The aggregate bounds are enforced by `write_bounded` / `read_bounded`
    // (deadline between syscalls, plus the SEND_CHUNK_BYTES cap that bounds
    // what one write(2) can transfer before the deadline is consulted). See
    // WRITE_BUDGET_MS for the measurement tables behind the design. The
    // deadlines are deliberately NOT re-armed into the socket options as the
    // remaining time shrinks: that would add setsockopt syscalls to the
    // success path, and it cannot tighten the write's residual anyway, since
    // SO_SNDTIMEO bounds a wait, not a progressing syscall, which is this
    // story's founding lesson.
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
    write_bounded(
        &mut write_stream,
        wire_bytes,
        Duration::from_millis(WRITE_BUDGET_MS),
    )?;

    // Stack buffer, no heap: replaces the BufReader + String pair that were
    // the success path's only allocations (shim hot-path discipline).
    let mut reply = [0u8; REPLY_BUF_BYTES];
    let mut read_stream = &stream;
    let filled = read_bounded(
        &mut read_stream,
        &mut reply,
        Duration::from_millis(READ_BUDGET_MS),
    )?;

    parse_reply(&reply[..filled])
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::ErrorKind;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use std::thread;

    /// Wide wall-clock ceiling for bounded-path assertions. This is a hang
    /// guard in miniature, NOT a latency assertion (project rule:
    /// deterministic test discipline): the in-tree measured values are 2.17
    /// to 3.03ms at full-chunk quanta (23.37ms at the pathological 256-byte
    /// quantum), and the real regression signal in these tests is
    /// Err-versus-Ok plus the syscall count, so a starved CI runner inflating
    /// wall time must not fail them. 5s is ~2000x the budget while still
    /// failing loudly if the bound is gone entirely.
    const GENEROUS_CEILING: Duration = Duration::from_secs(5);

    /// Counts calls to the inner `Write`/`Read` so tests count syscalls
    /// instead of reasoning about them (AC #6): each `write`/`read` call on a
    /// raw `&UnixStream` is exactly one syscall. The counter lives outside
    /// `write_bounded`/`read_bounded`, so it survives an `Err` return.
    struct Counting<T> {
        inner: T,
        calls: u64,
    }

    impl<T> Counting<T> {
        fn new(inner: T) -> Self {
            Self { inner, calls: 0 }
        }
    }

    impl<T: Write> Write for Counting<T> {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.calls += 1;
            self.inner.write(buf)
        }
        fn flush(&mut self) -> std::io::Result<()> {
            self.inner.flush()
        }
    }

    impl<T: Read> Read for Counting<T> {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            self.calls += 1;
            self.inner.read(buf)
        }
    }

    /// Peer that drains `chunk` bytes then parks for `interval`, the exact
    /// harness shape behind the Story 5.17 measurement table. Returns total
    /// bytes drained. Exits on EOF, error, `stop`, or its own 30s hang guard.
    ///
    /// The read comes BEFORE the stop check so that data already written by
    /// the test is always drained at least once even if `stop` was set while
    /// this thread waited to be scheduled; that is what lets callers assert
    /// `drained > 0` and thereby detect a drainer that never ran (a dead
    /// drainer would silently turn the sweep into the peer-slower-than-budget
    /// shape that made Story 5.16's test useless). `Interrupted` is retried,
    /// not treated as EOF, so a stray signal cannot kill the pacing.
    fn spawn_draining_peer(
        peer: UnixStream,
        chunk: usize,
        interval: Duration,
        stop: Arc<AtomicBool>,
    ) -> thread::JoinHandle<usize> {
        thread::spawn(move || {
            let hang_guard = Instant::now();
            let mut peer = &peer;
            let mut buf = vec![0u8; chunk];
            let mut drained = 0usize;
            while hang_guard.elapsed() < Duration::from_secs(30) {
                match peer.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => drained += n,
                    Err(e) if e.kind() == ErrorKind::Interrupted => continue,
                    Err(_) => break,
                }
                if stop.load(Ordering::Relaxed) {
                    break;
                }
                // Semantic timing, not synchronization: the paced drain IS the
                // mechanism under test (a peer that keeps the write alive).
                thread::sleep(interval);
            }
            drained
        })
    }

    fn timeout_pair() -> (UnixStream, UnixStream) {
        let (ours, peer) = UnixStream::pair().expect("socketpair");
        ours.set_write_timeout(Some(Duration::from_millis(WRITE_BUDGET_MS)))
            .expect("set_write_timeout");
        ours.set_read_timeout(Some(Duration::from_millis(READ_BUDGET_MS)))
            .expect("set_read_timeout");
        (ours, peer)
    }

    // ─── Story 5.17 AC #1 / #6: the write bound, driven at the drain rates ──
    // ─── that defeated both previous attempts ────────────────────────────────

    /// The core Story 5.17 test: a peer that drains *faster than the budget
    /// but still slowly* (the shape both retired tests missed; at these rates
    /// the unbounded code returned `Ok` after 40/97/189ms) must produce
    /// `Err(Timeout)` in bounded time, having given up mid-payload.
    ///
    /// The sweep varies BOTH pacing axes: interval (how often the peer wakes)
    /// and quantum (how many bytes it frees per wakeup). The small-quantum
    /// row exists because a peer freeing less than a full chunk makes one
    /// capped `write(2)` span several `SO_SNDTIMEO` waits, so the trailing
    /// chunk's overshoot is largest there; a full-chunk-only sweep could not
    /// falsify the chunk cap's premise (review finding, 2026-07-31).
    ///
    /// Environmental assumption, stated: the sender's socket buffer must be
    /// smaller than the 1 MiB payload, or every chunk queues instantly and
    /// the write legitimately succeeds. macOS pins it at 8 KiB
    /// (`net.local.stream.sendspace`); Linux defaults to ~208 KiB
    /// (`net.core.wmem_default`) and a host tuned to >= 1 MiB would fail this
    /// test's fixture, not the bound.
    ///
    /// Verified to fail against the unbounded code (Task 4): with the body of
    /// `write_bounded` swapped for `(&stream).write_all(buf)`, every rate
    /// returns `Ok` and the `is_err` assertion trips. Running on both CI
    /// platforms is also Task 1's Linux drain-rate sweep.
    #[test]
    fn bounded_write_gives_up_within_budget_across_drain_rates() {
        let payload = vec![0u8; 1 << 20]; // MAX_STDIN_BYTES, the real cap
        let n_chunks = payload.len().div_ceil(SEND_CHUNK_BYTES) as u64;

        for (quantum, interval_us) in [
            (SEND_CHUNK_BYTES, 200u64),
            (SEND_CHUNK_BYTES, 500),
            (SEND_CHUNK_BYTES, 1000),
            (1024, 200), // sub-chunk quantum: one write(2) spans several waits
        ] {
            let (ours, peer) = timeout_pair();
            let stop = Arc::new(AtomicBool::new(false));
            let drainer = spawn_draining_peer(
                peer,
                quantum,
                Duration::from_micros(interval_us),
                stop.clone(),
            );

            let mut counting = Counting::new(&ours);
            let start = Instant::now();
            let res = write_bounded(
                &mut counting,
                &payload,
                Duration::from_millis(WRITE_BUDGET_MS),
            );
            let wall = start.elapsed();
            let calls = counting.calls;

            stop.store(true, Ordering::Relaxed);
            drop(ours); // EOF for the peer in case it is parked in read
            let drained = drainer.join().expect("drainer thread must not panic");

            // At these rates the unbounded code returns Ok (the worst cases
            // return success); Err IS the mechanism assertion.
            let err = res.expect_err(&format!(
                "a peer draining {quantum}B every {interval_us}µs must trip \
                 the bound, not complete 1 MiB"
            ));
            match err {
                Error::Timeout {
                    op: TimeoutOp::Write,
                    budget_ms,
                    elapsed_ms,
                } => {
                    assert_eq!(budget_ms, WRITE_BUDGET_MS);
                    // AC #3: the reported figure tracks time actually spent.
                    // The 1ms slack covers the classify path: a kernel
                    // SO_SNDTIMEO can expire a hair before the monotonic
                    // clock crosses the budget, and as_millis() floors, so
                    // demanding elapsed_ms >= budget exactly would flake on
                    // an artifact rather than catch a regression.
                    assert!(
                        elapsed_ms + 1 >= WRITE_BUDGET_MS,
                        "a timeout cannot report materially less than the \
                         budget, reported {elapsed_ms}ms"
                    );
                    assert!(
                        u128::from(elapsed_ms) <= wall.as_millis(),
                        "reported {elapsed_ms}ms cannot exceed measured wall \
                         time {wall:?}"
                    );
                }
                other => panic!("expected a write Timeout, got {other:?}"),
            }

            // AC #6: syscalls counted, not reasoned about. Giving up
            // mid-payload means strictly fewer chunk writes than the payload
            // needs; the unbounded shape is 1 call returning Ok (caught above
            // by is_err) and a chunked-but-deadline-free regression pushes all
            // of them.
            assert!(
                calls >= 1 && calls < n_chunks,
                "expected an abandoned chunk loop (1..{n_chunks} syscalls), \
                 made {calls} at {quantum}B/{interval_us}µs"
            );

            // The drainer must actually have run, or this sweep silently
            // degenerates into the peer-slower-than-budget shape that made
            // Story 5.16's test useless. The drainer reads before honoring
            // stop, so bytes we wrote are always drained at least once.
            assert!(
                drained > 0,
                "the paced drainer never drained a byte at \
                 {quantum}B/{interval_us}µs; the sweep did not test the \
                 dribbling-peer shape"
            );

            // Hang guard in miniature, not a latency assertion (see
            // GENEROUS_CEILING).
            assert!(
                wall < GENEROUS_CEILING,
                "bounded write took {wall:?} at {quantum}B/{interval_us}µs"
            );
        }
    }

    /// AC #2 / Task 2: the success path must still be ONE `write(2)`, counted.
    /// A normal hook payload is a few hundred bytes against an empty 8 KiB
    /// send buffer, so the single chunk is accepted by one syscall with no
    /// waiting. (The end-to-end p99 claim on top of this is the hot-path
    /// bench gate in CI, per-platform policy in benches/baselines/.)
    #[test]
    fn success_path_is_one_syscall() {
        let (ours, peer) = timeout_pair();
        let payload = vec![b'x'; 400];

        let mut counting = Counting::new(&ours);
        write_bounded(
            &mut counting,
            &payload,
            Duration::from_millis(WRITE_BUDGET_MS),
        )
        .expect("400 bytes into an empty 8 KiB buffer cannot block");
        assert_eq!(
            counting.calls, 1,
            "a normal payload must stay a single write(2)"
        );

        // Drain and confirm nothing was truncated.
        drop(ours);
        let mut got = Vec::new();
        let mut peer = &peer;
        peer.read_to_end(&mut got).expect("drain");
        assert_eq!(got, payload, "the whole payload must have been written");
    }

    /// A payload exactly at the chunk cap is still one syscall; one byte over
    /// needs two. Pins the chunk arithmetic at its boundary.
    #[test]
    fn chunk_boundary_arithmetic() {
        for (len, want_calls) in [(SEND_CHUNK_BYTES, 1u64), (SEND_CHUNK_BYTES + 1, 2u64)] {
            // NOT timeout_pair(): its production 2ms SO_SNDTIMEO would let the
            // second chunk's wait flake on a starved runner. This test pins
            // syscall counts, not timing, so both the arm and the budget are
            // generous.
            let (ours, peer) = UnixStream::pair().expect("socketpair");
            ours.set_write_timeout(Some(Duration::from_millis(1000)))
                .expect("set_write_timeout");
            let stop = Arc::new(AtomicBool::new(false));
            // Eager drainer so the second chunk never waits long.
            let drainer = spawn_draining_peer(peer, SEND_CHUNK_BYTES, Duration::ZERO, stop.clone());

            let payload = vec![b'y'; len];
            let mut counting = Counting::new(&ours);
            write_bounded(
                &mut counting,
                &payload,
                // Generous budget: this test pins syscall counts, not timing.
                Duration::from_millis(1000),
            )
            .expect("an eagerly drained payload must complete");
            assert_eq!(
                counting.calls, want_calls,
                "{len} bytes must take exactly {want_calls} write(2) calls"
            );

            stop.store(true, Ordering::Relaxed);
            drop(ours);
            let _ = drainer.join();
        }
    }

    // ─── Story 5.17 AC #5: the read half is bounded too ─────────────────────

    /// A peer that dribbles reply bytes faster than the read budget but never
    /// sends the newline used to keep `read_line` alive indefinitely
    /// (measured 12ms and 24ms against the 3ms budget). The bounded read must
    /// give up at its deadline.
    ///
    /// Verified to fail against the unbounded shape (Task 4): with
    /// `read_bounded` swapped for the old `BufReader::read_line`, the dribble
    /// keeps the loop alive until the peer stops and the error surfaces as a
    /// late `BadResponse`/`Timeout` long past the ceiling asserted here, and
    /// the syscall-count assertion has no equivalent at all.
    #[test]
    fn bounded_read_gives_up_on_a_dribbling_peer() {
        let (ours, peer) = timeout_pair();
        let stop = Arc::new(AtomicBool::new(false));
        let stop_peer = stop.clone();
        let dribbler = thread::spawn(move || {
            let hang_guard = Instant::now();
            let mut peer = &peer;
            // One digit at a time, never a newline: each byte re-arms
            // `read_line`'s appetite while the bounded read's deadline keeps
            // counting. Semantic pacing, not synchronization.
            while !stop_peer.load(Ordering::Relaxed)
                && hang_guard.elapsed() < Duration::from_secs(30)
            {
                if peer.write_all(b"2").is_err() {
                    break;
                }
                thread::sleep(Duration::from_micros(800));
            }
        });

        let mut reply = [0u8; REPLY_BUF_BYTES];
        let mut counting = Counting::new(&ours);
        let start = Instant::now();
        let res = read_bounded(
            &mut counting,
            &mut reply,
            Duration::from_millis(READ_BUDGET_MS),
        );
        let wall = start.elapsed();
        let calls = counting.calls;

        stop.store(true, Ordering::Relaxed);
        drop(ours);
        let _ = dribbler.join();

        match res {
            Err(Error::Timeout {
                op: TimeoutOp::Read,
                budget_ms,
                elapsed_ms,
            }) => {
                assert_eq!(budget_ms, READ_BUDGET_MS);
                // 1ms slack for the classify path: the kernel SO_RCVTIMEO can
                // expire a hair before the monotonic clock crosses the budget
                // and as_millis() floors (same rationale as the write sweep).
                assert!(
                    elapsed_ms + 1 >= READ_BUDGET_MS,
                    "a timeout cannot report materially less than the budget, \
                     reported {elapsed_ms}ms"
                );
                assert!(
                    u128::from(elapsed_ms) <= wall.as_millis(),
                    "reported {elapsed_ms}ms cannot exceed wall {wall:?}"
                );
            }
            other => panic!("a dribbling peer must produce a read Timeout, got {other:?}"),
        }
        // A tight syscall-count assertion is deliberately impossible here: on
        // an unstarved run the dribble yields ~4 reads before the 3ms
        // deadline, but a starved runner legitimately produces exactly 1 (a
        // single read(2) parked until its SO_RCVTIMEO expires, surfacing
        // through classify). So >= 1 is a ran-at-all tripwire, and the
        // mechanism assertion is the Timeout match above: against the old
        // unbounded read_line this harness produces a late BadResponse
        // instead (measured 4.92s, Task 4 swap run).
        assert!(
            calls >= 1,
            "the deadline must be reached through real syscalls"
        );
        assert!(
            wall < GENEROUS_CEILING,
            "bounded read took {wall:?} (hang-guard ceiling, not latency)"
        );
    }

    /// Success path: a healthy daemon's whole `200\n` arrives in one
    /// `read(2)`, counted.
    #[test]
    fn read_success_is_one_syscall() {
        let (ours, peer) = timeout_pair();
        (&peer).write_all(b"200\n").expect("peer write");

        let mut reply = [0u8; REPLY_BUF_BYTES];
        let mut counting = Counting::new(&ours);
        let filled = read_bounded(
            &mut counting,
            &mut reply,
            Duration::from_millis(READ_BUDGET_MS),
        )
        .expect("a buffered reply cannot time out");
        assert_eq!(counting.calls, 1, "a ready reply must be a single read(2)");
        assert!(matches!(parse_reply(&reply[..filled]), Ok(Response::Ok)));
    }

    /// EOF before a newline returns whatever arrived, byte-for-byte what
    /// `read_line` did, so `send`'s parse behavior is unchanged: a daemon
    /// that closes after writing `200` (no newline) still counts as an ack.
    #[test]
    fn read_eof_before_newline_matches_read_line_behavior() {
        // NOT timeout_pair(): this needs TWO read(2) calls ("200", then EOF)
        // with a deadline check between them, so the production 3ms budget
        // would flake on a starved runner descheduled in that window. This
        // test pins EOF semantics, not timing; arm and budget are generous.
        let (ours, peer) = UnixStream::pair().expect("socketpair");
        ours.set_read_timeout(Some(Duration::from_millis(1000)))
            .expect("set_read_timeout");
        (&peer).write_all(b"200").expect("peer write");
        drop(peer); // EOF, no newline ever

        let mut reply = [0u8; REPLY_BUF_BYTES];
        let mut counting = Counting::new(&ours);
        let filled = read_bounded(&mut counting, &mut reply, Duration::from_millis(1000))
            .expect("EOF is not an error, matching read_line");
        assert_eq!(&reply[..filled], b"200");
        assert!(matches!(parse_reply(&reply[..filled]), Ok(Response::Ok)));
    }

    /// A reply that fills the whole buffer without a newline is a broken
    /// daemon: report BadResponse rather than reading forever. The buffer has
    /// 2x headroom over the longest line the daemon can emit, so this is
    /// unreachable against the real daemon.
    #[test]
    fn read_overlong_reply_is_bad_response() {
        // NOT timeout_pair(): its production 3ms SO_RCVTIMEO could fire before
        // the writer thread is scheduled on a starved runner. This test pins
        // the overflow arm, not timing, so both the arm and budget are
        // generous.
        let (ours, peer) = UnixStream::pair().expect("socketpair");
        ours.set_read_timeout(Some(Duration::from_millis(1000)))
            .expect("set_read_timeout");
        let blob = vec![b'a'; REPLY_BUF_BYTES + 64];
        let writer = thread::spawn(move || {
            let mut peer = &peer;
            let _ = peer.write_all(&blob);
        });

        let mut reply = [0u8; REPLY_BUF_BYTES];
        let mut ours_ref = &ours;
        let res = read_bounded(
            &mut ours_ref,
            &mut reply,
            // Generous budget: this test pins the overflow arm, not timing.
            Duration::from_millis(1000),
        );
        drop(ours);
        let _ = writer.join();

        assert!(
            matches!(res, Err(Error::BadResponse(_))),
            "a newline-free flood must be a BadResponse, got {res:?}"
        );
    }

    // ─── parse_reply: framing parity with the retired read_line path ────────

    #[test]
    fn parse_reply_covers_the_wire_grammar() {
        assert!(matches!(parse_reply(b"200\n"), Ok(Response::Ok)));
        assert!(matches!(parse_reply(b"503\n"), Ok(Response::Backpressure)));
        match parse_reply(b"400 invalid JSON: boom\n") {
            Ok(Response::DaemonError(reason)) => assert_eq!(reason, "invalid JSON: boom"),
            other => panic!("expected DaemonError, got {other:?}"),
        }
        // CRLF tolerated, as the old trim chain did.
        assert!(matches!(parse_reply(b"200\r\n"), Ok(Response::Ok)));
        // Only the first line participates; trailing bytes are ignored, as
        // read_line's framing did.
        assert!(matches!(parse_reply(b"200\ngarbage"), Ok(Response::Ok)));
        // Unknown status: BadResponse carrying the trimmed line.
        match parse_reply(b"999\n") {
            Err(Error::BadResponse(s)) => assert_eq!(s, "999"),
            other => panic!("expected BadResponse, got {other:?}"),
        }
        // Non-UTF-8: BadResponse (the daemon spoke garbage), not SocketIo.
        assert!(matches!(
            parse_reply(&[0xff, 0xfe, b'\n']),
            Err(Error::BadResponse(_))
        ));
    }

    // ─── Story 5.16 classification tests, carried forward ───────────────────

    /// AC #6 (Story 5.16): the io::Error → `Error` classification, tested
    /// directly rather than by racing a real socket.
    ///
    /// Both kinds are matched because `std` does not pin which one an expired
    /// socket timeout produces (see `classify`'s doc); missing either would let
    /// a real timeout keep the old undiagnosable `SocketIo` wording.
    #[test]
    fn would_block_and_timed_out_both_classify_as_timeout() {
        for kind in [ErrorKind::WouldBlock, ErrorKind::TimedOut] {
            let e = classify(
                TimeoutOp::Read,
                READ_BUDGET_MS,
                Duration::from_millis(7),
                std::io::Error::from(kind),
            );
            match e {
                Error::Timeout {
                    op,
                    budget_ms,
                    elapsed_ms,
                } => {
                    assert_eq!(
                        op,
                        TimeoutOp::Read,
                        "op must survive classification: {kind:?}"
                    );
                    assert_eq!(budget_ms, READ_BUDGET_MS, "budget must survive: {kind:?}");
                    assert_eq!(elapsed_ms, 7, "elapsed must survive: {kind:?}");
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
            classify(
                TimeoutOp::Read,
                READ_BUDGET_MS,
                Duration::from_millis(3),
                eagain
            ),
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
                Duration::from_millis(1),
                std::io::Error::from(kind),
            );
            assert!(
                matches!(e, Error::SocketIo(_)),
                "{kind:?} must stay SocketIo, got {e:?}"
            );
        }
    }

    /// AC #1/#2 (Story 5.16): the log line must name the expired budget instead
    /// of restating the errno, and (Story 5.17 AC #3) carry the measured
    /// elapsed time next to it.
    #[test]
    fn timeout_message_names_the_operation_and_budget() {
        let write = classify(
            TimeoutOp::Write,
            WRITE_BUDGET_MS,
            Duration::from_millis(48),
            std::io::Error::from(ErrorKind::WouldBlock),
        );
        let read = classify(
            TimeoutOp::Read,
            READ_BUDGET_MS,
            Duration::from_millis(12),
            std::io::Error::from(ErrorKind::WouldBlock),
        );

        let w = write.to_string();
        let r = read.to_string();

        assert!(w.contains("write"), "must name the operation: {w:?}");
        assert!(
            w.contains("budget 2ms"),
            "must name the write budget: {w:?}"
        );
        assert!(
            w.contains("after 48ms"),
            "must report measured elapsed, not the configured value: {w:?}"
        );
        assert!(r.contains("read"), "must name the operation: {r:?}");
        assert!(r.contains("budget 3ms"), "must name the read budget: {r:?}");
        assert!(
            r.contains("after 12ms"),
            "must report measured elapsed, not the configured value: {r:?}"
        );

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

    /// The timeout constants are the single source of truth for the socket
    /// options, the loop deadlines, and the message. Pin them so a change is a
    /// deliberate edit here (and, per Story 5.17 AC #1, a measured one).
    ///
    /// As of Story 5.17 these ARE aggregate budgets, enforced by
    /// `write_bounded`/`read_bounded`; the drain-rate tests above are the
    /// mechanism tests two earlier constant-sum assertions failed to be.
    #[test]
    fn budgets_match_the_documented_contract() {
        assert_eq!(WRITE_BUDGET_MS, 2);
        assert_eq!(READ_BUDGET_MS, 3);
        assert_eq!(SEND_CHUNK_BYTES, 8192, "matches net.local.stream.sendspace");
    }

    // ─── Task 1: the measurement table, reproducible from the repo ───────────

    /// Regenerates the Story 5.17 measurement table from std's `write_all`,
    /// the exact unbounded shape that shipped before this story. Ignored in
    /// CI on purpose: it is wall-clock-heavy, machine-sensitive by design
    /// (it MEASURES a stall), and asserts nothing tight; the reproducible
    /// numbers are the point. Run on an idle machine:
    ///
    ///   scripts/test.sh -p bowerbird-shim -- --ignored manual_repro
    ///
    /// Expected shape (macOS arm64, idle): dribbling peers at 200-1000µs keep
    /// a SINGLE 2ms-armed write(2) alive for tens to hundreds of ms and it
    /// returns Ok; only a peer slower than the budget errors. Linux pushes
    /// ~31x more per syscall before consulting the timeout.
    #[test]
    #[ignore = "manual measurement of the unbounded write_all stall; see doc"]
    fn manual_repro_unbounded_write_all_measurement_table() {
        let payload = vec![0u8; 1 << 20];
        eprintln!("interval | result | elapsed | write(2) calls");
        for interval_us in [0u64, 200, 500, 1000, 1400, 2000] {
            let (ours, peer) = timeout_pair();
            let stop = Arc::new(AtomicBool::new(false));
            let drainer = spawn_draining_peer(
                peer,
                SEND_CHUNK_BYTES,
                Duration::from_micros(interval_us),
                stop.clone(),
            );

            let mut counting = Counting::new(&ours);
            let start = Instant::now();
            let res = counting.write_all(&payload);
            let wall = start.elapsed();
            let calls = counting.calls;

            stop.store(true, Ordering::Relaxed);
            drop(ours);
            let _ = drainer.join();

            eprintln!(
                "{interval_us:>7}µs | {} | {wall:>10.2?} | {calls}",
                if res.is_ok() { "Ok " } else { "Err" }
            );
        }
    }

    /// The SHIPPED bound, measured in-tree (review finding, 2026-07-31: the
    /// bound's magnitude previously existed only as scratchpad-harness
    /// numbers). Three row groups:
    ///
    /// 1. `write_bounded`, 1 MiB, the classic drain rates: the aggregate
    ///    bound at the shapes that previously stalled 40-189ms.
    /// 2. `write_bounded`, 1 MiB, sub-chunk quanta: the trailing-chunk
    ///    residual at its worst (one capped write(2) spanning several waits).
    /// 3. `write_bounded`, mid-size payloads against an eager drainer plus a
    ///    paced one: the payload-size delivery profile (the "cliff") under
    ///    the production 2ms budget.
    ///
    /// Ignored in CI on purpose, same rationale as the table above; run:
    ///
    ///   scripts/test.sh -p bowerbird-shim -- --ignored --nocapture manual_repro
    #[test]
    #[ignore = "manual measurement of the shipped bound; see doc"]
    fn manual_repro_bounded_write_measurement_table() {
        eprintln!("payload | quantum | interval | result | elapsed | write(2) calls");
        let rows: &[(usize, usize, u64)] = &[
            // group 1: classic rates
            (1 << 20, SEND_CHUNK_BYTES, 200),
            (1 << 20, SEND_CHUNK_BYTES, 500),
            (1 << 20, SEND_CHUNK_BYTES, 1000),
            // group 2: sub-chunk quanta (trailing-chunk residual)
            (1 << 20, 1024, 200),
            (1 << 20, 256, 500),
            // group 3: payload-size profile, eager drain (healthy idle daemon)
            (16 << 10, 64 << 10, 0),
            (100 << 10, 64 << 10, 0),
            (1 << 20, 64 << 10, 0),
            // group 3: payload-size profile, paced drain (busy-daemon proxy,
            // ~one 8 KiB wakeup per 200µs, the 5.16 p50 wakeup figure)
            (100 << 10, SEND_CHUNK_BYTES, 200),
        ];
        for &(payload_len, quantum, interval_us) in rows {
            let payload = vec![0u8; payload_len];
            let (ours, peer) = timeout_pair();
            let stop = Arc::new(AtomicBool::new(false));
            let drainer = spawn_draining_peer(
                peer,
                quantum,
                Duration::from_micros(interval_us),
                stop.clone(),
            );

            let mut counting = Counting::new(&ours);
            let start = Instant::now();
            let res = write_bounded(
                &mut counting,
                &payload,
                Duration::from_millis(WRITE_BUDGET_MS),
            );
            let wall = start.elapsed();
            let calls = counting.calls;

            stop.store(true, Ordering::Relaxed);
            drop(ours);
            let _ = drainer.join();

            eprintln!(
                "{:>7}K | {quantum:>7}B | {interval_us:>7}µs | {} | {wall:>10.2?} | {calls}",
                payload_len >> 10,
                if res.is_ok() { "Ok " } else { "Err" }
            );
        }
    }
}
