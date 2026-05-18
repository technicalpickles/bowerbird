// Custom hot-path bench for the shim. Replaces Criterion because we need
// TRUE per-invocation timings to compute p99 honestly — Criterion's
// `SamplingMode::Flat` still batches multiple iterations per sample on
// workloads in our ~1.5–5ms range, which averages spikes into invisibility
// and lets a high-tail regression pass a mean-based gate (story 1.5
// review finding 2). This file is a `harness = false` bench, so `cargo bench`
// runs it as a plain binary.
//
// Output: writes `target/shim-bench-summary.json` with the canonical baseline
// schema consumed by `scripts/check-shim-bench-p99.py` and committed as
// `crates/shim/benches/baselines/<platform>.json`:
//
//   {"schema_version": 1, "p99_nanos": N, "mean_nanos": M, "samples": K}

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use assert_cmd::cargo::cargo_bin;
use tempfile::TempDir;

struct MockIngest {
    sock_path: PathBuf,
    stop: Arc<AtomicBool>,
    _tmp: TempDir,
}

impl MockIngest {
    fn start() -> Self {
        let tmp = TempDir::new().expect("tempdir");
        let sock_path = tmp.path().join("ingest.sock");
        let listener = UnixListener::bind(&sock_path).expect("bind");
        listener.set_nonblocking(true).expect("set_nonblocking");

        let stop = Arc::new(AtomicBool::new(false));
        let stop_clone = stop.clone();

        thread::spawn(move || {
            while !stop_clone.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        let _ = stream.set_nonblocking(false);
                        let _ = handle_one(stream);
                    }
                    Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_micros(100));
                    }
                    Err(_) => break,
                }
            }
        });

        // Pre-warm: ensure the inode is in the dentry cache.
        thread::sleep(Duration::from_millis(10));

        Self {
            sock_path,
            stop,
            _tmp: tmp,
        }
    }
}

impl Drop for MockIngest {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

fn handle_one(stream: UnixStream) -> std::io::Result<()> {
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut line = String::new();
    reader.read_line(&mut line)?;
    let mut writer = stream;
    writer.write_all(b"200\n")?;
    writer.flush()?;
    Ok(())
}

const STDIN_FIXTURE: &[u8] =
    br#"{"session_id":"bench","tool_name":"Bash","tool_input":{"command":"echo hi"}}"#;

fn measure_one(shim: &std::path::Path, sock: &std::path::Path, log: &std::path::Path) -> Duration {
    let start = Instant::now();
    let mut child = std::process::Command::new(shim)
        .arg("--hook-kind")
        .arg("PreToolUse")
        .env("BOWERBIRD_INGEST_SOCK", sock)
        .env("BOWERBIRD_SHIM_LOG", log)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn shim");
    {
        let stdin = child.stdin.as_mut().expect("stdin");
        stdin.write_all(STDIN_FIXTURE).expect("write stdin");
    }
    let status = child.wait().expect("wait");
    assert!(status.success(), "shim exited non-zero: {status:?}");
    start.elapsed()
}

fn main() {
    // Honor `cargo bench`'s standard `--bench` argv it passes — accept all
    // positional args and ignore them. The bench is parameter-less; samples
    // are configurable via env to keep CI tunable without code edits.
    let samples: usize = std::env::var("SHIM_BENCH_SAMPLES")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(200);
    let warmup: usize = std::env::var("SHIM_BENCH_WARMUP")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(20);

    let shim = cargo_bin("bowerbird-shim");
    let mock = MockIngest::start();
    let log_tmp = TempDir::new().expect("log tmpdir");
    let log_path = log_tmp.path().join("shim.log");

    eprintln!(
        "shim_hot_path: warmup={warmup} samples={samples} shim={} sock={}",
        shim.display(),
        mock.sock_path.display()
    );

    for _ in 0..warmup {
        let _ = measure_one(&shim, &mock.sock_path, &log_path);
    }

    let mut timings_nanos: Vec<u128> = Vec::with_capacity(samples);
    for i in 0..samples {
        let d = measure_one(&shim, &mock.sock_path, &log_path);
        timings_nanos.push(d.as_nanos());
        if (i + 1) % 50 == 0 {
            eprintln!("  measured {}/{}", i + 1, samples);
        }
    }

    timings_nanos.sort_unstable();
    let n = timings_nanos.len();
    let mean_nanos = timings_nanos.iter().sum::<u128>() / (n as u128);
    let p50_idx = n / 2;
    let p99_idx = ((n as f64 * 0.99).ceil() as usize)
        .saturating_sub(1)
        .min(n - 1);
    let p50_nanos = timings_nanos[p50_idx];
    let p99_nanos = timings_nanos[p99_idx];
    let max_nanos = *timings_nanos.last().expect("samples > 0");

    eprintln!(
        "results: mean={:.3}ms p50={:.3}ms p99={:.3}ms max={:.3}ms (n={n})",
        mean_nanos as f64 / 1_000_000.0,
        p50_nanos as f64 / 1_000_000.0,
        p99_nanos as f64 / 1_000_000.0,
        max_nanos as f64 / 1_000_000.0,
    );

    // Write the canonical summary JSON. The bench binary's CWD is the bench
    // crate (`crates/shim/`), but the workspace target dir lives at the repo
    // root (or `$CARGO_TARGET_DIR` if overridden). Resolve through
    // CARGO_MANIFEST_DIR so the path is consistent across local and CI.
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
    let summary_path = target_dir.join("shim-bench-summary.json");
    let body = format!(
        "{{\"schema_version\": 1, \"p99_nanos\": {p99_nanos}, \"mean_nanos\": {mean_nanos}, \"samples\": {n}}}\n"
    );
    std::fs::write(&summary_path, body).expect("write summary");
    eprintln!("wrote summary: {}", summary_path.display());
}
