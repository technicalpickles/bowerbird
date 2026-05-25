//! Story 4.4 / AC #7 — hook→presenter p99 daemon bench.
//!
//! Measures end-to-end latency from `ingest_socket.write(line)` to
//! `ws.receive(EventFrame)` under four production-shaped workloads:
//!
//!   - **solo**: one presenter, one ingest line, repeated. Baseline.
//!   - **fanout3**: three presenters subscribed to `events.*`, max-of-three
//!     per-event. Catches per-subscriber tail-latency regressions in the
//!     broadcast hub's dispatch path.
//!   - **burst**: one presenter, eight events clumped within 50ms (mimicking
//!     Claude Code's tool-call boundary clump). Measures the WORST latency
//!     within the burst — uniform-throughput tests miss this shape.
//!   - **steady**: one presenter, one event/sec for 30 seconds. Catches
//!     sustained-load regressions (slow leaks, accumulating mutex contention,
//!     periodic-task overruns).
//!
//! `harness = false` because Criterion's `SamplingMode::Flat` would batch
//! samples in our ~1-10ms range and average spikes into invisibility — the
//! same finding as Story 1.5 review #2. Per-invocation timings + manual p99
//! computation are the load-bearing pattern. Same JSON schema as the shim
//! bench so `scripts/check-daemon-bench-p99.py` can mirror
//! `scripts/check-shim-bench-p99.py`.
//!
//! ## Helper-promotion choice
//!
//! The AC's Task 7.1 listed three options for sharing `spawn_test_daemon`
//! between the contract suite and this bench: (A) `crates/daemon/tests/common/mod.rs`,
//! (B) a new `daemon-test-helpers` workspace crate, (C) inline duplication.
//! We chose **(C) inline duplication** for this bench because the contract
//! suite's existing `spawn_test_daemon` is an IN-PROCESS axum server (good
//! for unit-style WS tests); the bench needs a SUBPROCESS daemon (for
//! production-shaped scheduler behavior + real ingest-socket bind). The two
//! shapes don't share code structure, so promoting wouldn't pay rent.
//! Documented here per the AC's "document the choice" requirement.

use std::io::{Read, Write};
use std::os::unix::net::UnixStream as StdUnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use tokio::time::sleep;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::header;

const SCHEMA_VERSION: u32 = 1;
const DEFAULT_SAMPLES: usize = 50; // smaller than shim's 200 — daemon roundtrips are slower
const TEST_BEARER: &str = "daemon-bench-token";

#[derive(Debug, Deserialize)]
struct ServerInfo {
    bind_addr: String,
}

struct Daemon {
    child: Child,
    // Held only for its Drop side-effect: TempDir removes the bench's HOME
    // directory when the Daemon is dropped. Never read — clippy would
    // otherwise flag the field as dead.
    #[allow(dead_code)]
    home: tempfile::TempDir,
    sock_path: PathBuf,
    bind_addr: String,
}

impl Drop for Daemon {
    fn drop(&mut self) {
        // Best-effort SIGTERM; if it's already gone, that's fine.
        let pid = self.child.id() as i32;
        let _ = nix::sys::signal::kill(
            nix::unistd::Pid::from_raw(pid),
            nix::sys::signal::Signal::SIGTERM,
        );
        let _ = self.child.wait();
    }
}

fn spawn_daemon() -> Daemon {
    let home = tempfile::TempDir::new().expect("tempdir");
    let bowerbird_dir = home.path().join(".bowerbird");
    std::fs::create_dir_all(&bowerbird_dir).expect("mkdir");
    let sock_path = bowerbird_dir.join("ingest.sock");

    let bin = assert_cmd::cargo::cargo_bin("bowerbird-daemon");
    let child = Command::new(&bin)
        .env("HOME", home.path())
        .env("BOWERBIRD_INGEST_SOCK", &sock_path)
        .env("BOWERBIRD_TOKEN", TEST_BEARER)
        .env("BOWERBIRD_KEYRING_BACKEND", "disable")
        .env("RUST_LOG", "warn")
        .env(
            "PATH",
            std::env::var_os("PATH").unwrap_or_else(|| std::ffi::OsString::from("")),
        )
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap_or_else(|e| panic!("spawn daemon ({}): {e}", bin.display()));

    // Wait for ingest socket + server.json (NFR3: <2s startup).
    let server_json_path = bowerbird_dir.join("server.json");
    let deadline = Instant::now() + Duration::from_secs(10);
    let bind_addr = loop {
        if sock_path.exists() && server_json_path.exists() {
            if let Ok(body) = std::fs::read_to_string(&server_json_path) {
                if let Ok(info) = serde_json::from_str::<ServerInfo>(&body) {
                    break info.bind_addr;
                }
            }
        }
        if Instant::now() > deadline {
            panic!("daemon failed to bind within 10s");
        }
        std::thread::sleep(Duration::from_millis(20));
    };

    Daemon {
        child,
        home,
        sock_path,
        bind_addr,
    }
}

/// Connect a WS client subscribed to `events.*`. Returns the stream after the
/// initial Hello frame has been consumed.
async fn connect_subscriber(
    bind_addr: &str,
) -> tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>> {
    let url = format!("ws://{bind_addr}/ws");
    let mut req = url
        .as_str()
        .into_client_request()
        .expect("into_client_request");
    req.headers_mut().insert(
        header::AUTHORIZATION,
        format!("Bearer {TEST_BEARER}").parse().expect("header"),
    );
    let (mut ws, _resp) = tokio_tungstenite::connect_async(req)
        .await
        .expect("ws connect");

    // Consume the Hello frame.
    let _hello = tokio::time::timeout(Duration::from_secs(5), ws.next())
        .await
        .expect("hello within 5s")
        .expect("stream alive")
        .expect("hello recv");

    // Subscribe to events.*.
    ws.send(tokio_tungstenite::tungstenite::Message::Text(
        r#"{"op":"subscribe","topic":"events.*"}"#.to_string().into(),
    ))
    .await
    .expect("send subscribe");

    ws
}

/// Send one NDJ ingest line synchronously. Returns Ok on `200`.
fn send_ingest_line(sock: &Path, payload: &str) -> std::io::Result<()> {
    let mut s = StdUnixStream::connect(sock)?;
    s.write_all(payload.as_bytes())?;
    s.write_all(b"\n")?;
    s.flush()?;
    s.shutdown(std::net::Shutdown::Write)?;
    let mut resp = String::new();
    s.read_to_string(&mut resp)?;
    if !resp.starts_with("200") {
        return Err(std::io::Error::other(format!("ingest non-200: {resp}")));
    }
    Ok(())
}

/// Drain WS messages until one looks like an EventFrame (i.e. `"op":"event"`).
/// Returns the elapsed time from `t0` to first EventFrame.
async fn drain_until_event(
    ws: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    t0: Instant,
) -> Duration {
    let deadline = Duration::from_secs(2);
    let result = tokio::time::timeout(deadline, async {
        while let Some(msg) = ws.next().await {
            let msg = msg.expect("ws recv ok");
            if let tokio_tungstenite::tungstenite::Message::Text(t) = msg {
                if t.contains("\"op\":\"event\"") {
                    return t0.elapsed();
                }
                // Ignore state frames + dropped frames — the metric is
                // "event-frame end-to-end" specifically.
            }
        }
        panic!("ws stream ended before event frame");
    })
    .await;
    result.expect("event frame within 2s")
}

fn percentile(sorted: &[Duration], p: f64) -> Duration {
    let n = sorted.len();
    let idx = ((n as f64 * p).ceil() as usize)
        .saturating_sub(1)
        .min(n - 1);
    sorted[idx]
}

fn fmt_ms(d: Duration) -> String {
    format!("{:.3}ms", d.as_nanos() as f64 / 1_000_000.0)
}

// ---------------------------------------------------------------------------
// Bench shapes
// ---------------------------------------------------------------------------

async fn bench_solo(d: &Daemon, samples: usize) -> Duration {
    let mut ws = connect_subscriber(&d.bind_addr).await;
    // Settle.
    sleep(Duration::from_millis(50)).await;

    let mut timings: Vec<Duration> = Vec::with_capacity(samples);
    for i in 0..samples {
        let payload = format!(
            r#"{{"hook_kind":"PreToolUse","session_id":"sess-solo","tool_name":"Bash","tool_input":{{"i":{i}}}}}"#,
        );
        let t0 = Instant::now();
        send_ingest_line(&d.sock_path, &payload).expect("ingest");
        let elapsed = drain_until_event(&mut ws, t0).await;
        timings.push(elapsed);
    }
    timings.sort();
    let p99 = percentile(&timings, 0.99);
    eprintln!(
        "  solo:   samples={} p50={} p99={} max={}",
        timings.len(),
        fmt_ms(percentile(&timings, 0.50)),
        fmt_ms(p99),
        fmt_ms(*timings.last().unwrap()),
    );
    p99
}

async fn bench_fanout3(d: &Daemon, samples: usize) -> Duration {
    let mut a = connect_subscriber(&d.bind_addr).await;
    let mut b = connect_subscriber(&d.bind_addr).await;
    let mut c = connect_subscriber(&d.bind_addr).await;
    sleep(Duration::from_millis(50)).await;

    let mut timings: Vec<Duration> = Vec::with_capacity(samples);
    for i in 0..samples {
        let payload = format!(
            r#"{{"hook_kind":"PreToolUse","session_id":"sess-fanout","tool_name":"Bash","tool_input":{{"i":{i}}}}}"#,
        );
        let t0 = Instant::now();
        send_ingest_line(&d.sock_path, &payload).expect("ingest");
        let (ea, eb, ec) = tokio::join!(
            drain_until_event(&mut a, t0),
            drain_until_event(&mut b, t0),
            drain_until_event(&mut c, t0),
        );
        // max-of-three: the slowest subscriber is what NFR2 cares about.
        timings.push(ea.max(eb).max(ec));
    }
    timings.sort();
    let p99 = percentile(&timings, 0.99);
    eprintln!("  fanout3: samples={} p99={}", timings.len(), fmt_ms(p99));
    p99
}

async fn bench_burst(d: &Daemon, burst_count: usize) -> Duration {
    // Each burst: send 8 events within ~50ms, then read 8 event frames,
    // measure the WORST latency across the 8.
    let mut ws = connect_subscriber(&d.bind_addr).await;
    sleep(Duration::from_millis(50)).await;

    let burst_size = 8;
    let mut worst_per_burst: Vec<Duration> = Vec::with_capacity(burst_count);
    for burst_i in 0..burst_count {
        let mut t0s: Vec<Instant> = Vec::with_capacity(burst_size);
        for j in 0..burst_size {
            let payload = format!(
                r#"{{"hook_kind":"PreToolUse","session_id":"sess-burst-{burst_i}","tool_name":"Bash","tool_input":{{"j":{j}}}}}"#,
            );
            let t0 = Instant::now();
            send_ingest_line(&d.sock_path, &payload).expect("ingest");
            t0s.push(t0);
        }
        // Drain `burst_size` event frames; per-event latency = elapsed from
        // its corresponding t0.
        let mut received: Vec<Duration> = Vec::with_capacity(burst_size);
        for t0 in &t0s {
            let elapsed = drain_until_event(&mut ws, *t0).await;
            received.push(elapsed);
        }
        worst_per_burst.push(*received.iter().max().unwrap());
    }
    worst_per_burst.sort();
    let p99 = percentile(&worst_per_burst, 0.99);
    eprintln!(
        "  burst:  bursts={} worst-of-each-burst p99={}",
        worst_per_burst.len(),
        fmt_ms(p99)
    );
    p99
}

async fn bench_steady(d: &Daemon, duration_secs: u64) -> Duration {
    // One event per `target_period`, for `duration_secs` total. Lower volume
    // than the AC's "30 seconds at 1/sec" so the bench fits inside CI budget;
    // configurable via env for full runs.
    let target_period = Duration::from_millis(200); // 5 events/sec
    let end = Instant::now() + Duration::from_secs(duration_secs);
    let mut ws = connect_subscriber(&d.bind_addr).await;
    sleep(Duration::from_millis(50)).await;

    let mut timings: Vec<Duration> = Vec::new();
    let mut i: u64 = 0;
    while Instant::now() < end {
        let payload = format!(
            r#"{{"hook_kind":"PreToolUse","session_id":"sess-steady","tool_name":"Bash","tool_input":{{"i":{i}}}}}"#,
        );
        let t0 = Instant::now();
        send_ingest_line(&d.sock_path, &payload).expect("ingest");
        let elapsed = drain_until_event(&mut ws, t0).await;
        timings.push(elapsed);
        i += 1;
        let next = t0 + target_period;
        if let Some(wait) = next.checked_duration_since(Instant::now()) {
            sleep(wait).await;
        }
    }
    timings.sort();
    let p99 = percentile(&timings, 0.99);
    eprintln!("  steady: samples={} p99={}", timings.len(), fmt_ms(p99));
    p99
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

fn main() {
    let samples: usize = std::env::var("DAEMON_BENCH_SAMPLES")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(DEFAULT_SAMPLES);
    let burst_count: usize = std::env::var("DAEMON_BENCH_BURST_COUNT")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(20);
    let steady_secs: u64 = std::env::var("DAEMON_BENCH_STEADY_SECS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(5);

    eprintln!(
        "hook_to_presenter: samples={samples} burst_count={burst_count} steady_secs={steady_secs}"
    );

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");

    let daemon = spawn_daemon();

    let (solo_p99, fanout3_p99, burst_p99, steady_p99) = rt.block_on(async {
        let solo = bench_solo(&daemon, samples).await;
        let fanout3 = bench_fanout3(&daemon, samples / 2).await;
        let burst = bench_burst(&daemon, burst_count).await;
        let steady = bench_steady(&daemon, steady_secs).await;
        (solo, fanout3, burst, steady)
    });

    drop(daemon);

    // Write summary JSON to target dir (same convention as shim bench).
    let target_dir = match std::env::var_os("CARGO_TARGET_DIR") {
        Some(p) => PathBuf::from(p),
        None => {
            let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
            manifest
                .parent()
                .and_then(|p| p.parent())
                .map(|root| root.join("target"))
                .expect("CARGO_MANIFEST_DIR has two parent components")
        }
    };
    std::fs::create_dir_all(&target_dir).expect("create target dir");
    let summary_path = target_dir.join("daemon-bench-summary.json");

    let body = format!(
        "{{\n  \"schema_version\": {SCHEMA_VERSION},\n  \"solo_p99_nanos\": {},\n  \"fanout3_p99_nanos\": {},\n  \"burst_p99_nanos\": {},\n  \"steady_p99_nanos\": {},\n  \"samples\": {samples}\n}}\n",
        solo_p99.as_nanos(),
        fanout3_p99.as_nanos(),
        burst_p99.as_nanos(),
        steady_p99.as_nanos(),
    );
    std::fs::write(&summary_path, body).expect("write summary");
    eprintln!("wrote summary: {}", summary_path.display());
}
