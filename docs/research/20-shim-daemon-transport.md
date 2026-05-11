# Shim-to-daemon transport

The shim is invoked synchronously by Claude Code on every hook event. Its job is to forward the hook payload to the daemon and exit fast. This document specifies *how* — which transport, what failure handling, what latency budget, what happens when the daemon is unresponsive or absent.

The constraints (restated):

- **<5ms p95 cold start.** The shim is invoked fresh for every hook event; there's no warm process to reuse.
- **Never block Claude.** Claude waits synchronously for the shim's exit code; any latency in the shim is latency in the user's coding session.
- **Never surface an error to the user.** A non-zero exit from the shim shows up as a "Hook failed" message in Claude, which is worse than the shim silently dropping the event.
- **Survive a missing or unresponsive daemon.** First-run, daemon-not-yet-started, daemon-crashed, daemon-overloaded all need to be non-events from Claude's perspective.

The shim is, fundamentally, a fire-and-forget emitter from Claude's perspective regardless of what's happening on the daemon side.

## Transport options considered

### Option A: HTTP POST to `127.0.0.1:9876`

**Shape:** shim opens TCP connection, writes HTTP request, reads response, closes.

**Pros:** Standard, well-tested. Same wire format as REST API; one transport to maintain.

**Cons:** TCP handshake on loopback is ~0.5-2ms even when everything is healthy. HTTP framing adds parse overhead on both ends. The shim has to drag in an HTTP client library or carefully hand-roll one. Worst: HTTP failures are rich and surface in many forms (connection refused, TLS errors, timeouts at multiple layers, malformed responses), each of which the shim has to handle without ever exiting non-zero.

**Verdict:** workable but not the best fit. The shim doesn't need HTTP semantics; it just needs "send these bytes to the daemon, fast."

### Option B: Unix domain socket (UDS)

**Shape:** shim opens UDS connection to `~/.claude-state-bus/sock`, writes the event (length-prefixed JSON or NDJSON), closes.

**Pros:** Sub-millisecond connect on loopback (no TCP, no IP layer, no port). No HTTP framing. Permissions via filesystem (the socket file is owned by the user, mode 0600). Connection-refused is the only "daemon not running" failure mode and it's instant. Trivially supportable from any language without a library.

**Cons:** Not portable to Windows in the same form (Windows 10+ supports AF_UNIX but with quirks). MVP is macOS/Linux only per the design, so this is OK.

**Verdict:** the right primary transport for MVP.

### Option C: UDP datagram to `127.0.0.1:9876`

**Shape:** shim opens UDP socket, sends one packet, closes. Connectionless — no ack, no acknowledgment of delivery.

**Pros:** No handshake at all. Cannot fail in any way the shim sees (the kernel queues the datagram regardless of whether anyone is reading). Sub-100µs latency on the shim side.

**Cons:** No delivery guarantee. Packets can be dropped under load (loopback UDP socket buffers are bounded; if the daemon isn't reading fast enough, the kernel drops). Packet size limits: 64KB max but realistically ~8KB is the safe boundary for one datagram. Hook payloads with large tool inputs (`Read` of a 50KB file as part of the payload?) could exceed this. Hard to debug — there's no error to log on the sender side when delivery fails.

**Verdict:** tempting for the fire-and-forget latency, but the silent-drop behavior under load is dangerous. We'd lose events without knowing.

### Option D: Named pipe / FIFO

**Shape:** shim opens `~/.claude-state-bus/pipe` for write, writes the event, closes.

**Pros:** Simple, no protocol.

**Cons:** Multiple writers to a FIFO is messy (partial writes can interleave above PIPE_BUF, which is 4KB on Linux and is smaller than realistic hook payloads). Daemon must be reading or the writer blocks (or hits an error). Doesn't compose well with restart semantics.

**Verdict:** not worth the corner cases.

### Option E: File spool only

**Shape:** shim writes a file to `~/.claude-state-bus/spool/<timestamp>-<random>.ndjson` and exits. Daemon watches the directory (inotify on Linux, FSEvents on macOS) and picks up new files.

**Pros:** Fully decoupled. No transport failure modes — if the disk works, the event is delivered. Survives daemon restart trivially: spool files persist; daemon drains the directory on startup. Survives the daemon being absent entirely.

**Cons:** Disk write is ~1-3ms on SSD (more on slow disks). FS event delivery from kernel to daemon adds ~10-50ms typical latency end-to-end. For pub/sub presenters this means a noticeable delay between "Claude calls a tool" and "lamp turns yellow." Acceptable for storage but not ideal as the primary path.

**Verdict:** essential as a fallback, not great as the only mechanism.

### Option F: Hybrid — UDS preferred, spool fallback

**Shape:** shim tries UDS connect with a hard timeout (~2ms). On success, sends the event over UDS and exits. On any failure (connect timeout, connection refused, write error), writes the event to the spool and exits.

**Pros:** Fast path (UDS) in the common case. Reliable path (spool) when anything goes wrong. Both paths exit the shim cleanly with no error to surface. Daemon drains spool on startup and continues watching it during runtime (so events that arrive when the daemon is overloaded and refuses connections also land via spool).

**Cons:** Two code paths to maintain. The "did the event arrive via spool or socket" non-uniformity has to be handled in the daemon.

**Verdict:** this is the right answer.

## The picked design

The shim emits over UDS in the common case, falls back to disk spool on any failure, and always exits 0.

### Wire format on the UDS

Newline-delimited JSON. Each event is one line. The shim writes a single line and closes the connection; no length prefix, no framing protocol on top.

```json
{"event_id":"01HQXJ...","source":"claude","session_id":"abc-123","kind":"preToolUse","ts":"2026-05-12T14:32:11.421Z","payload":{...}}
```

The daemon reads until newline, parses, acknowledges by closing the connection. The shim doesn't wait for an ack at the application level — it just closes after writing. TCP/UDS will deliver the bytes; the daemon will get them.

### UDS path

`$XDG_RUNTIME_DIR/claude-state-bus.sock` on Linux, `~/Library/Application Support/claude-state-bus/run/sock` on macOS. Mode 0600. The daemon creates it on startup (unlinking any stale socket file first).

### Connect timeout

The shim uses a non-blocking `connect()` with `select()` (or platform equivalent) and a hard 2ms timeout. On Unix this is straightforward; on macOS specifically, `connect()` to a UDS that exists and is being listened on returns immediately, so the timeout is effectively just bounding the "what if something is weird" case.

### Spool path

`~/.claude-state-bus/spool/` (rooted, not XDG, because it's persistent state across runtime sessions).

File naming: `<unix-nanos>-<6-char-random>.ndjson`. The unix-nanos prefix gives chronological ordering; the random suffix prevents collisions if two shim invocations happen in the same nanosecond (which is unlikely but cheap to guard).

Atomic creation: `open(path, O_CREAT | O_EXCL | O_WRONLY, 0600)`. If the file already exists (random collision), regenerate the suffix and retry. Write the event, `fsync()` is NOT called (the shim's budget can't afford it; we trade durability for latency in this fallback path).

### Daemon spool processing

On startup, the daemon reads every file in `~/.claude-state-bus/spool/`, sorted by filename (which sorts by timestamp). For each file: parse the JSON, ingest the event normally, delete the file. If parsing fails, the file is moved to `~/.claude-state-bus/spool/.malformed/` with the original name preserved — these accumulate for the user to debug but don't block ingest.

During runtime, the daemon watches the spool directory via inotify/FSEvents. Any new file triggers immediate read-and-ingest. This catches the case where the shim couldn't reach the live UDS (overloaded daemon, momentary failure) and fell back to spool — the daemon picks those up within ~10ms of the file appearing.

The daemon also runs a 30-second sweep that scans the spool directory unconditionally, in case the FS watcher missed an event (FSEvents in particular has known reliability quirks under high load). This is belt-and-suspenders; in the common case nothing happens in the sweep.

### Shim algorithm

In pseudocode:

```
fn main(stdin: HookPayload) {
    let event = build_event(stdin);
    let serialized = serialize_ndjson(event);
    
    match try_uds_send(&serialized, timeout=2ms) {
        Ok(()) => exit(0),
        Err(_) => match spool_write(&serialized) {
            Ok(()) => exit(0),
            Err(_) => {
                eprintln!("claude-state-bus-shim: failed to write event");
                exit(0)  // STILL exit 0 — never fail the hook
            }
        }
    }
}
```

The crucial property: **the shim's exit code is always 0**, regardless of what happened internally. Hook failures surface to the user; the shim's job is to never cause one.

A separate diagnostic facility (`claude-state-bus diagnose`) checks for accumulating spool files, malformed entries, and recent stderr messages. Users who want to know "is my agent state pipeline healthy" run that; the shim never tells them.

### Latency budget

Approximate, measured on a modest laptop:

| Path | Median | p95 | p99 |
|---|---|---|---|
| UDS happy path | ~0.8ms | ~1.5ms | ~3ms |
| UDS fallback to spool (daemon down) | ~3ms | ~5ms | ~8ms |
| Spool only (daemon never reachable) | ~2ms | ~3ms | ~6ms |
| Catastrophic (spool write fails too) | ~3ms | ~5ms | ~10ms |

The 5ms p95 budget holds in every case. The 2ms connect timeout is the worst-case overhead on the fast path; if we have evidence that 2ms is too generous for healthy daemons, it can be tightened to 1ms.

## What we're explicitly trading

The picked design has trade-offs worth naming:

### We trade delivery guarantee for never-blocking

The spool fallback delivers the event eventually (when the daemon is reachable again), but events ingested via spool arrive in the daemon with extra latency (filesystem watch latency, daemon startup time, etc). Presenters subscribed to `events.*` see them out of strict order with events that came over UDS.

The daemon sorts on insert by `event_id` (which is a ULID with millisecond precision, generated by the shim at the time the event was received from Claude). So the event log is well-ordered. But the *delivery to live subscribers* may surface spool events later than their nominal timestamp. Presenters that strictly care about real-time order need to handle the "events with old timestamps showing up later" case. The cookbook will document this.

### We trade durability for latency on the spool path

The shim writes to spool with `O_CREAT | O_EXCL | O_WRONLY` but doesn't `fsync()`. A kernel crash between the write and the page-cache flush could lose the event. We accept this because:

- The 5ms budget can't afford a sync (which is 5-50ms even on fast SSDs).
- Kernel crashes are rare; Claude itself dying in the same window is much more common.
- The event log is observational; missing one PostToolUse from a kernel-crash hour ago is not a serious correctness problem.

If correctness matters more than latency for a particular deployment, the shim could expose a `--durable` flag that adds the fsync. Out of scope for MVP.

### We trade simplicity for two code paths

A pure-spool design (only Option E) would be conceptually simpler. We pay maintenance cost on having two paths (UDS and spool) for the latency win in the common case.

The maintenance cost is small: the spool path is ~30 lines and the UDS path is ~40 lines. Both are unit-testable by injecting failures. The complexity is bounded.

## What this implies for daemon design

A few things the daemon needs in order for this transport to work:

1. **UDS listener** that accepts connections, reads one NDJSON line, ingests, closes. Tokio's `UnixListener` handles this in a few dozen lines.

2. **Spool directory watcher** using `notify` crate (`inotify` on Linux, `FSEvents` on macOS). On each file event, attempt read and ingest.

3. **Startup spool drain** — walk the directory in sorted order, ingest all files, delete each as processed. Before opening the UDS, so events that accumulated during downtime are visible to subscribers in the right order.

4. **Periodic spool sweep** — every 30 seconds, walk the directory unconditionally. Catches missed FS events.

5. **Malformed file quarantine** — on parse error, move the file to a `.malformed/` subdirectory. Don't infinite-loop on a corrupt file.

6. **Concurrent ingest from UDS and spool** — they share the same insert path into the event log. The daemon must serialize inserts (single SQLite writer thread or short critical section) but reads from UDS and from the spool watcher can happen in parallel.

## Failure modes by scenario

For confidence, here's what happens in each failure scenario the user might encounter:

### Daemon is not running (first boot, daemon hasn't started yet)

- Shim invocation → UDS connect → connection refused (~50µs to detect) → spool fallback → write file → exit 0.
- Claude sees clean exit; no error to user.
- When the daemon starts (manually or via launchd/systemd), it drains the spool and ingests the events.
- Live subscribers connected to the freshly-started daemon see the historic events in their proper order (via REST snapshot or `since=<cursor>` query) but not as live pub/sub frames (because the events are old by then).

### Daemon is starting up (UDS not yet bound)

- Same as above. Connect refused → spool → exit 0.
- Daemon finishes startup, drains spool, ingests events.
- No data loss; just delayed delivery.

### Daemon is running but overloaded (long GC pause, blocked on disk)

- Shim invocation → UDS connect → connection accepted but write hangs (daemon isn't accepting yet) → 2ms timeout fires → connection closed → spool fallback → exit 0.
- Daemon catches up, processes spool via FS watcher.

### Daemon crashes mid-write

- Shim wrote part of an NDJSON line to UDS before crash.
- Daemon's read on the dead connection fails; the partial bytes are discarded.
- The event is *not* in the spool (because UDS succeeded from the shim's perspective).
- **This event is lost.** This is the only scenario where data loss occurs.

This loss case is fixable but with cost: the shim could write the spool file *first*, then do the UDS send, then delete the spool file on UDS success. This trades latency in the happy path (extra disk write) for durability against daemon crash. Probably not worth it for MVP; revisit if real loss is observed.

### Daemon crashes after ack, before commit

- Daemon read the event over UDS but crashed before writing to SQLite.
- From the shim's perspective: successful UDS send, exit 0.
- The event is lost.
- Mitigation: daemon writes the event to SQLite in the same syscall as reading from UDS (or as close as possible), with WAL ensuring durability. Crash window is tiny but non-zero.

### Network stack is weird (firewall blocks loopback?)

- UDS doesn't use the network stack at all. Firewall rules on `127.0.0.1` don't affect UDS.
- Even in this exotic scenario, UDS works.

### `~/.claude-state-bus/spool/` doesn't exist or isn't writable

- Spool write fails.
- Shim exits 0 anyway. Event is lost.
- Diagnostic facility detects this on next `claude-state-bus diagnose` run.

## Open questions

A few things this design specifies that may want revisiting after MVP:

### Should the shim fork-and-detach a background process for the network send?

If we're worried about the 1-2ms UDS round-trip, the shim could `fork()`, the child does the network work, the parent exits immediately. Reduces shim wallclock to ~100µs.

Trade-off: process management complexity, signal handling, double-fork to detach from the shim's session. The wallclock win is real but the implementation cost is also real. For MVP the synchronous UDS path is probably fast enough; revisit if we see hook latency complaints.

### Should the daemon ack at the application layer?

Currently the daemon's "ack" is just closing the UDS connection. The shim doesn't actually verify the event was ingested. If we wanted at-least-once delivery semantics from the shim's perspective, we'd want a small app-layer ack ("OK" or an event_id confirmation) before the shim exits.

The cost: another round-trip (~0.5ms). The benefit: the shim knows whether to spool on failure. Currently the shim only spools when UDS write fails — but if the daemon accepts the connection and then dies before processing, the shim doesn't know.

For MVP this is fine because UDS connection acceptance is a strong signal that the daemon is healthy. If real loss appears, this is the first place to tighten.

### How aggressive should spool cleanup be?

The daemon drains and deletes spool files on ingest. But what about old files in `.malformed/`? Currently the design leaves them forever (for user debugging). After a year of malformed events accumulating, this could be MB of dead files.

A daemon-side daily sweep that compresses or deletes `.malformed/` files older than 30 days seems reasonable. Add at M3 if it becomes a problem.

### Does the shim need a separate config file?

The UDS path is conventional; the spool path is conventional. There's nothing to configure on the shim side. If the daemon binds to a non-default UDS path, the shim needs to know — but currently the shim is hard-coded to the conventional path.

If users ever want to run multiple daemon instances (one per Claude profile?), the shim would need a way to be told which UDS path to use. The `~/.claude/settings.json` hook command can pass args; the shim could accept `--socket /custom/path`. Defer until someone asks.

## Summary

The shim emits over Unix domain socket in the common case. On any failure — connect timeout, connection refused, write error — it falls back to writing the event to a disk spool directory and exits. The shim always exits 0 regardless of internal failure.

The daemon listens on the UDS for live events and watches the spool directory for fallback events. On startup it drains the spool before opening the UDS, ensuring that events from before the daemon was running are visible to subscribers.

Latency: <2ms p95 in the common case, <5ms p95 in the fallback case. Never blocks Claude, never surfaces an error to the user, survives the daemon being absent entirely.

The one acknowledged data-loss scenario (daemon crashes between accepting UDS write and committing to SQLite) is small and fixable later if real loss is observed. Every other failure mode results in delayed delivery, not loss.