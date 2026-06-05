//! Shared lifecycle helpers used by `bowerbird install`, `bowerbird start`,
//! `bowerbird uninstall`, and `bowerbird stop`.
//!
//! The helpers here are the single copy of: detached daemon spawn (`setsid` +
//! redirected stdio), PID-file-driven graceful stop (SIGTERM → drain →
//! SIGKILL fallback with kernel-reap drain), and a minimal hand-rolled HTTP
//! GET for the `/healthz` readiness probe. The CLI intentionally does NOT
//! link `tokio` or an async HTTP client — see project-context.md
//! "CLI binary should NOT pull `tokio`" and Story 3.1's lightness invariant.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

use anyhow::Context;

/// Result of [`start_daemon_detached`].
#[derive(Debug, Clone, Copy)]
pub enum StartOutcome {
    /// The daemon was already up (ingest socket connectable). No new process
    /// was spawned; the caller can read the existing PID file if it wants the
    /// running daemon's pid.
    AlreadyRunning,
    /// A fresh daemon process was spawned. The returned PID is the immediate
    /// child PID — after `setsid` detach it becomes its own session leader.
    Spawned { pid: u32 },
}

/// Result of [`stop_daemon_via_pid_file`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopOutcome {
    /// No PID file present or it pointed at a dead PID. The caller can treat
    /// both "fresh install" and "stale pid file" as the same no-op.
    NotRunning,
    /// SIGTERM landed and the daemon exited within the graceful drain budget.
    Stopped,
    /// SIGTERM was sent but the daemon did not exit within 10s; SIGKILL was
    /// issued and the kernel reaped it. The CLI exits 0 even in this branch —
    /// the user wanted the daemon stopped; it is stopped.
    Escalated,
}

/// Spawn `bowerbird-daemon` detached from this process group if it is not
/// already running. Idempotent: if the ingest socket is connectable, returns
/// [`StartOutcome::AlreadyRunning`] without spawning.
pub fn start_daemon_detached(bowerbird_dir: &Path) -> anyhow::Result<StartOutcome> {
    let ingest_sock = bowerbird_dir.join("ingest.sock");
    if super::daemon_is_up(&ingest_sock) {
        return Ok(StartOutcome::AlreadyRunning);
    }

    let bin = super::resolve_daemon_bin();
    let pid = spawn_detached(&bin).with_context(|| {
        format!(
            "spawn daemon binary `{}` (set BOWERBIRD_DAEMON_BIN or ensure bowerbird-daemon is on PATH)",
            bin.display()
        )
    })?;
    Ok(StartOutcome::Spawned { pid })
}

/// Stop the running daemon by reading the singleton PID file (Story 3.1's
/// `~/.bowerbird/bowerbird.pid`), sending SIGTERM, waiting up to 10s for the
/// graceful shutdown sequence (Story 2.5), then escalating to SIGKILL with a
/// 1s post-signal drain to absorb kernel reap latency.
pub fn stop_daemon_via_pid_file(bowerbird_dir: &Path) -> anyhow::Result<StopOutcome> {
    let pid_path = bowerbird_dir.join("bowerbird.pid");

    let pid = match read_pid(&pid_path)? {
        Some(p) => p,
        None => return Ok(StopOutcome::NotRunning),
    };

    if !pid_alive(pid) {
        return Ok(StopOutcome::NotRunning);
    }

    println!("sending SIGTERM to bowerbird-daemon (pid {pid})");
    send_signal(pid, libc::SIGTERM).context("send SIGTERM")?;

    // Wait up to twice the daemon's default drain timeout (5s). The CLI does
    // not link the daemon crate so it cannot read `Config::shutdown_drain_timeout`
    // directly; doubling the documented default gives the graceful path room
    // without making the CLI feel hung.
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut escalated = false;
    while pid_alive(pid) {
        if Instant::now() >= deadline {
            eprintln!("daemon did not exit within 10s; escalating to SIGKILL");
            let _ = send_signal(pid, libc::SIGKILL);
            // SIGKILL delivery is asynchronous: the kernel reaps within
            // microseconds but `kill(pid, 0)` may briefly still report the
            // process as alive between signal delivery and reaping. Drain
            // for up to 1s so the post-loop liveness check does not bail
            // with a scary "still alive after SIGKILL" message that is
            // really just the kernel catching up.
            let kill_deadline = Instant::now() + Duration::from_secs(1);
            while pid_alive(pid) && Instant::now() < kill_deadline {
                std::thread::sleep(Duration::from_millis(20));
            }
            escalated = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }

    if pid_alive(pid) {
        anyhow::bail!("daemon pid {pid} still alive after SIGKILL");
    }
    if escalated {
        Ok(StopOutcome::Escalated)
    } else {
        Ok(StopOutcome::Stopped)
    }
}

/// Read the PID file. Returns `Ok(None)` for both "file missing" and "file
/// present but content is not a positive integer" — both cases mean "no
/// recoverable PID."
pub fn read_pid(pid_path: &Path) -> anyhow::Result<Option<i32>> {
    match std::fs::read_to_string(pid_path) {
        Ok(s) => match s.trim().parse::<i32>() {
            Ok(p) if p > 0 => Ok(Some(p)),
            _ => Ok(None),
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(anyhow::Error::new(e).context(format!("read {}", pid_path.display()))),
    }
}

/// `kill(pid, 0)`: returns Ok if the process exists, ESRCH if not. EPERM means
/// "exists, but we lack permission to signal it" — still alive for our
/// purposes.
pub fn pid_alive(pid: i32) -> bool {
    let r = unsafe { libc::kill(pid, 0) };
    if r == 0 {
        return true;
    }
    let errno = std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
    errno == libc::EPERM
}

/// Is a live process recorded in the data dir's singleton PID file? This is
/// distinct from [`super::daemon_is_up`] (a socket probe): the daemon acquires
/// its singleton (flock + `bowerbird.pid`) BEFORE it binds the ingest socket
/// (`crates/daemon/src/singleton.rs`, taken in `main` before any socket work),
/// so a holder can be alive while the socket is still down — wedged before bind,
/// or still bound to a previous socket path after the registered socket changed.
/// The macOS launchd handoff uses this so it does not bootstrap/kickstart over a
/// singleton holder the socket probe cannot see, which would fail the launchd
/// daemon's singleton acquisition and crash-loop it under
/// `KeepAlive={SuccessfulExit=false}` (Story 5.9 review pass-5 F2).
pub fn pid_holder_alive(bowerbird_dir: &Path) -> bool {
    match read_pid(&bowerbird_dir.join("bowerbird.pid")) {
        Ok(Some(pid)) => pid_alive(pid),
        _ => false,
    }
}

fn send_signal(pid: i32, sig: libc::c_int) -> std::io::Result<()> {
    let r = unsafe { libc::kill(pid, sig) };
    if r == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(unix)]
fn spawn_detached(bin: &Path) -> anyhow::Result<u32> {
    use std::os::unix::process::CommandExt;

    // Detach the child so it survives the CLI process exiting:
    //  - `setsid` makes it a new session leader (no controlling terminal).
    //  - stdio is rerouted to /dev/null so it does not write to our terminal.
    // The daemon's own panic hook + tracing already handles its log output to
    // ~/.bowerbird and stderr.
    let child = unsafe {
        std::process::Command::new(bin)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .pre_exec(|| {
                let _ = nix_setsid();
                Ok(())
            })
            .spawn()?
    };
    Ok(child.id())
}

#[cfg(unix)]
fn nix_setsid() -> std::io::Result<()> {
    // Avoid pulling `nix` into the CLI's dep tree (the daemon already uses it
    // but the CLI is intentionally lightweight). libc's setsid is a single
    // syscall with a stable signature.
    extern "C" {
        fn setsid() -> libc::pid_t;
    }
    let r = unsafe { setsid() };
    if r == -1 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(not(unix))]
fn spawn_detached(bin: &Path) -> anyhow::Result<u32> {
    let child = std::process::Command::new(bin)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    Ok(child.id())
}

/// Path to the daemon-published `server.json` that carries the bound HTTP
/// address. See `crates/daemon/src/server_file.rs` for the wire shape.
pub fn server_json_path(bowerbird_dir: &Path) -> PathBuf {
    bowerbird_dir.join("server.json")
}

/// Read and parse `server.json`. Returns `Ok(None)` if the file is missing or
/// unparseable — the caller decides how to degrade.
pub fn read_server_info(bowerbird_dir: &Path) -> Option<protocol::ServerInfo> {
    let path = server_json_path(bowerbird_dir);
    let bytes = std::fs::read(&path).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// Wait for `server.json` to appear, polling at `interval` up to `deadline`.
/// Returns the parsed `ServerInfo` if the file becomes available; `None` on
/// timeout. Used by `bowerbird start` to discover the spawned daemon's port
/// before probing `/healthz`.
pub fn wait_for_server_json(
    bowerbird_dir: &Path,
    deadline: Instant,
    interval: Duration,
) -> Option<protocol::ServerInfo> {
    while Instant::now() < deadline {
        if let Some(info) = read_server_info(bowerbird_dir) {
            return Some(info);
        }
        std::thread::sleep(interval);
    }
    None
}

/// Outcome of [`http_get_healthz`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HealthzOutcome {
    /// `HTTP/1.1 200 ...` came back.
    Ok,
    /// Connected and got a response but the status line was not 200.
    Unhealthy,
    /// Could not connect within the per-attempt budget.
    Unreachable,
}

/// Probe `GET /healthz` against the daemon's bound address. Hand-rolled over
/// `TcpStream` to avoid pulling `ureq` or `reqwest` into the CLI for two
/// fixed-shape requests. The endpoint is unauthenticated (project-context.md
/// "API surface" health-endpoint split), so no Authorization header.
pub fn http_get_healthz(addr: SocketAddr, per_attempt: Duration) -> HealthzOutcome {
    let mut stream = match TcpStream::connect_timeout(&addr, per_attempt) {
        Ok(s) => s,
        Err(_) => return HealthzOutcome::Unreachable,
    };
    // Read deadline mirrors the connect timeout. /healthz responds in <1ms on
    // loopback; per_attempt is the safe ceiling.
    let _ = stream.set_read_timeout(Some(per_attempt));
    let _ = stream.set_write_timeout(Some(per_attempt));

    let req = format!(
        "GET /healthz HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n\r\n",
        addr
    );
    if stream.write_all(req.as_bytes()).is_err() {
        return HealthzOutcome::Unreachable;
    }
    // We only need the status line; the body is tiny but we let
    // `Connection: close` end the read for us.
    let mut buf = [0u8; 256];
    let n = match stream.read(&mut buf) {
        Ok(n) => n,
        Err(_) => return HealthzOutcome::Unreachable,
    };
    let prefix = &buf[..n];
    if prefix.starts_with(b"HTTP/1.1 200") || prefix.starts_with(b"HTTP/1.0 200") {
        HealthzOutcome::Ok
    } else {
        HealthzOutcome::Unhealthy
    }
}

/// Same hand-rolled HTTP as [`http_get_healthz`], but for the bearer-gated
/// `/status` endpoint. Returns the raw response body bytes on 200; status code
/// on non-200; transport-level errors map to `Ok(None)`.
#[derive(Debug)]
pub enum StatusResponse {
    Ok(Vec<u8>),
    Status(u16),
    Unreachable,
}

pub fn http_get_status(
    addr: SocketAddr,
    bearer: Option<&str>,
    per_attempt: Duration,
) -> StatusResponse {
    let mut stream = match TcpStream::connect_timeout(&addr, per_attempt) {
        Ok(s) => s,
        Err(_) => return StatusResponse::Unreachable,
    };
    let _ = stream.set_read_timeout(Some(per_attempt));
    let _ = stream.set_write_timeout(Some(per_attempt));

    let mut req = String::new();
    req.push_str("GET /status HTTP/1.1\r\n");
    req.push_str(&format!("Host: {addr}\r\n"));
    req.push_str("Connection: close\r\n");
    if let Some(t) = bearer {
        req.push_str(&format!("Authorization: Bearer {t}\r\n"));
    }
    req.push_str("Accept: application/json\r\n\r\n");
    if stream.write_all(req.as_bytes()).is_err() {
        return StatusResponse::Unreachable;
    }
    let mut response = Vec::with_capacity(2048);
    if stream.read_to_end(&mut response).is_err() {
        // Partial reads still might contain enough to parse the status; fall
        // through.
    }
    let status_code = parse_status_code(&response).unwrap_or(0);
    if status_code == 200 {
        let body = body_after_headers(&response).to_vec();
        StatusResponse::Ok(body)
    } else if status_code == 0 {
        StatusResponse::Unreachable
    } else {
        StatusResponse::Status(status_code)
    }
}

/// Outcome of [`http_post_replay`]. Same shape as [`StatusResponse`]; kept
/// distinct so the caller can match without confusing the two endpoints.
#[derive(Debug)]
pub enum ReplayResponse {
    Ok(Vec<u8>),
    Status(u16),
    Unreachable,
}

/// Hand-rolled `POST /replay` against the bearer-gated endpoint added in
/// Story 4.1. Mirrors [`http_get_status`]'s shape with three differences:
/// (a) verb is `POST`, (b) `Content-Type: application/x-ndjson` +
/// `Content-Length: <body.len()>`, (c) body bytes follow the headers.
///
/// Returns the raw response body bytes on 200; status code on non-200;
/// transport-level errors map to `ReplayResponse::Unreachable`. The caller
/// parses the 200 body as the daemon's `{replayed_count, parse_errors}`
/// shape (see `crates/daemon/src/api/replay.rs::ReplayResponse`).
pub fn http_post_replay(
    addr: SocketAddr,
    bearer: &str,
    body: &[u8],
    per_attempt: Duration,
) -> ReplayResponse {
    let mut stream = match TcpStream::connect_timeout(&addr, per_attempt) {
        Ok(s) => s,
        Err(_) => return ReplayResponse::Unreachable,
    };
    let _ = stream.set_read_timeout(Some(per_attempt));
    let _ = stream.set_write_timeout(Some(per_attempt));

    // Headers, then a blank line, then the body bytes. `Content-Length` must
    // be the byte length (not char count) for axum's body extractor to read
    // exactly that many bytes.
    let mut head = String::new();
    head.push_str("POST /replay HTTP/1.1\r\n");
    head.push_str(&format!("Host: {addr}\r\n"));
    head.push_str("Connection: close\r\n");
    head.push_str(&format!("Authorization: Bearer {bearer}\r\n"));
    head.push_str("Content-Type: application/x-ndjson\r\n");
    head.push_str(&format!("Content-Length: {}\r\n", body.len()));
    head.push_str("Accept: application/json\r\n\r\n");

    if stream.write_all(head.as_bytes()).is_err() {
        return ReplayResponse::Unreachable;
    }
    if stream.write_all(body).is_err() {
        return ReplayResponse::Unreachable;
    }

    let mut response = Vec::with_capacity(4096);
    let _ = stream.read_to_end(&mut response);
    let status_code = parse_status_code(&response).unwrap_or(0);
    if status_code == 200 {
        let body = body_after_headers(&response).to_vec();
        ReplayResponse::Ok(body)
    } else if status_code == 0 {
        ReplayResponse::Unreachable
    } else {
        ReplayResponse::Status(status_code)
    }
}

/// Outcome of [`http_get_events`]. Mirrors [`StatusResponse`]; we
/// distinguish 404 explicitly because the export command needs to render a
/// distinct "session not found" message.
#[derive(Debug)]
pub enum EventsResponse {
    Ok(Vec<u8>),
    NotFound,
    Status(u16),
    Unreachable,
}

/// Hand-rolled `GET /sessions/{id}/events?since=<cursor>` against the
/// bearer-gated endpoint. Mirrors [`http_get_status`]'s shape; the response
/// body is the JSON-serialized `protocol::EventListResponse` that
/// `bowerbird export` iterates via the `cursor` field.
pub fn http_get_events(
    addr: SocketAddr,
    bearer: &str,
    session_id: &str,
    since: i64,
    per_attempt: Duration,
) -> EventsResponse {
    let mut stream = match TcpStream::connect_timeout(&addr, per_attempt) {
        Ok(s) => s,
        Err(_) => return EventsResponse::Unreachable,
    };
    let _ = stream.set_read_timeout(Some(per_attempt));
    let _ = stream.set_write_timeout(Some(per_attempt));

    // `session_id` is user-supplied; escape it for URL-path inclusion.
    // We only encode the unreserved characters strict enough to keep the
    // path well-formed — letters, digits, `-`, `_`, `.`, `~` pass through;
    // anything else becomes `%XX`. Avoids pulling `percent-encoding` for
    // a one-shot path.
    let encoded_id = encode_path_segment(session_id);

    let mut req = String::new();
    req.push_str(&format!(
        "GET /sessions/{encoded_id}/events?since={since} HTTP/1.1\r\n"
    ));
    req.push_str(&format!("Host: {addr}\r\n"));
    req.push_str("Connection: close\r\n");
    req.push_str(&format!("Authorization: Bearer {bearer}\r\n"));
    req.push_str("Accept: application/json\r\n\r\n");

    if stream.write_all(req.as_bytes()).is_err() {
        return EventsResponse::Unreachable;
    }

    let mut response = Vec::with_capacity(8192);
    let _ = stream.read_to_end(&mut response);
    let status_code = parse_status_code(&response).unwrap_or(0);
    match status_code {
        200 => EventsResponse::Ok(body_after_headers(&response).to_vec()),
        404 => EventsResponse::NotFound,
        0 => EventsResponse::Unreachable,
        other => EventsResponse::Status(other),
    }
}

fn encode_path_segment(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for &b in s.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            _ => {
                out.push_str(&format!("%{b:02X}"));
            }
        }
    }
    out
}

fn parse_status_code(response: &[u8]) -> Option<u16> {
    let line_end = response.iter().position(|&b| b == b'\n')?;
    let line = &response[..line_end];
    let line_str = std::str::from_utf8(line).ok()?;
    let parts: Vec<&str> = line_str.split_whitespace().collect();
    if parts.len() < 2 {
        return None;
    }
    parts[1].parse::<u16>().ok()
}

fn body_after_headers(response: &[u8]) -> &[u8] {
    // Locate the `\r\n\r\n` sentinel that separates headers from body. Some
    // pathological responses use `\n\n`; tolerate that too.
    if let Some(p) = find_subslice(response, b"\r\n\r\n") {
        return &response[p + 4..];
    }
    if let Some(p) = find_subslice(response, b"\n\n") {
        return &response[p + 2..];
    }
    &[]
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    (0..=(haystack.len() - needle.len())).find(|&i| &haystack[i..i + needle.len()] == needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_status_code_handles_standard_line() {
        let resp = b"HTTP/1.1 200 OK\r\n\r\n";
        assert_eq!(parse_status_code(resp), Some(200));
    }

    #[test]
    fn parse_status_code_handles_non_200() {
        let resp = b"HTTP/1.1 401 Unauthorized\r\n\r\n";
        assert_eq!(parse_status_code(resp), Some(401));
    }

    #[test]
    fn body_after_headers_extracts_body() {
        let resp = b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nhello";
        assert_eq!(body_after_headers(resp), b"hello");
    }

    #[test]
    fn body_after_headers_returns_empty_when_no_separator() {
        let resp = b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n";
        assert_eq!(body_after_headers(resp), b"");
    }

    #[test]
    fn find_subslice_finds_needle() {
        assert_eq!(find_subslice(b"abcdef", b"cd"), Some(2));
        assert_eq!(find_subslice(b"abcdef", b"xyz"), None);
    }

    #[test]
    fn encode_path_segment_passes_unreserved() {
        assert_eq!(encode_path_segment("session-alpha"), "session-alpha");
        assert_eq!(encode_path_segment("foo_bar.baz"), "foo_bar.baz");
        assert_eq!(encode_path_segment("abc123~"), "abc123~");
    }

    #[test]
    fn encode_path_segment_escapes_slashes_and_spaces() {
        assert_eq!(encode_path_segment("a/b"), "a%2Fb");
        assert_eq!(encode_path_segment("a b"), "a%20b");
        assert_eq!(encode_path_segment("foo?bar"), "foo%3Fbar");
    }
}
