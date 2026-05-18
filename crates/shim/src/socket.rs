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

/// Synchronously send one wire payload to the daemon ingest socket and read
/// back the single-line status response.
///
/// `wire_bytes` MUST already include the trailing `\n` framing byte.
pub(crate) fn send(sock_path: &Path, wire_bytes: &[u8]) -> Result<Response> {
    let stream = UnixStream::connect(sock_path).map_err(|source| Error::Connect {
        path: sock_path.to_path_buf(),
        source,
    })?;

    // Tight per-op timeouts keep the total budget under 5ms even when the
    // daemon is slow to respond. Total = write + read ≤ 5ms in the worst case.
    stream
        .set_write_timeout(Some(Duration::from_millis(2)))
        .map_err(Error::SocketIo)?;
    stream
        .set_read_timeout(Some(Duration::from_millis(3)))
        .map_err(Error::SocketIo)?;

    let mut write_stream = &stream;
    // `UnixStream::write_all` on a connected socket is unbuffered — flush is a
    // no-op syscall and is intentionally skipped to keep the hot path tight.
    write_stream
        .write_all(wire_bytes)
        .map_err(Error::SocketIo)?;

    let mut reader = BufReader::with_capacity(64, &stream);
    let mut line = String::with_capacity(64);
    reader.read_line(&mut line).map_err(Error::SocketIo)?;

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
