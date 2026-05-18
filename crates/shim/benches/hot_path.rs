// Criterion benchmark for the shim hot path.
//
// Spins up a synchronous-stdlib UDS mock that mirrors the daemon's
// `200\n` response, then measures end-to-end shim invocation latency.
// Per AC #1 the p99 target is ≤5ms with per-platform baselines stored
// under `benches/baselines/`.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use assert_cmd::Command;
use criterion::{criterion_group, criterion_main, Criterion};
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

        // Pre-warm: ensure inode is in the dentry cache.
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

fn bench_uds_post_ingest(c: &mut Criterion) {
    let mock = MockIngest::start();
    let sock = mock.sock_path.clone();

    // Per-bench-iteration log dir (each invocation may write to it on a
    // theoretical 503 — under the success path nothing is written).
    let log_tmp = TempDir::new().expect("log tmpdir");
    let log_path = log_tmp.path().join("shim.log");

    c.bench_function("uds_post_ingest", |b| {
        b.iter(|| {
            Command::cargo_bin("bowerbird-shim")
                .expect("cargo_bin")
                .arg("--hook-kind")
                .arg("PreToolUse")
                .env("BOWERBIRD_INGEST_SOCK", &sock)
                .env("BOWERBIRD_SHIM_LOG", &log_path)
                .write_stdin(STDIN_FIXTURE)
                .assert()
                .success();
        });
    });
}

criterion_group!(benches, bench_uds_post_ingest);
criterion_main!(benches);
