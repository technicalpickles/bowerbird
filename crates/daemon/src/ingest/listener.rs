use std::path::PathBuf;
use std::sync::Arc;

use tokio::net::UnixListener;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use adapter_claude::ClaudeAdapter;

use crate::error::{Error, Result};

pub async fn run(
    sock_path: PathBuf,
    tx: mpsc::Sender<super::IngestItem>,
    shutdown: CancellationToken,
    adapter: Arc<ClaudeAdapter>,
) -> Result<()> {
    let listener = bind(&sock_path)?;
    run_bound(listener, sock_path, tx, shutdown, adapter).await
}

pub fn bind(sock_path: &std::path::Path) -> Result<UnixListener> {
    let _ = std::fs::remove_file(sock_path);

    let listener = UnixListener::bind(sock_path)
        .map_err(|e| Error::Ingest(format!("bind {}: {e}", sock_path.display())))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(sock_path, std::fs::Permissions::from_mode(0o600))
            .map_err(|e| Error::Ingest(format!("set_permissions on ingest.sock: {e}")))?;
    }

    Ok(listener)
}

pub async fn run_bound(
    listener: UnixListener,
    sock_path: PathBuf,
    tx: mpsc::Sender<super::IngestItem>,
    shutdown: CancellationToken,
    adapter: Arc<ClaudeAdapter>,
) -> Result<()> {
    loop {
        tokio::select! {
            accept_result = listener.accept() => {
                match accept_result {
                    Ok((stream, _)) => {
                        tokio::spawn(super::handler::handle(stream, tx.clone(), adapter.clone()));
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {
                        tracing::warn!(error = ?e, "transient ingest accept error");
                    }
                    Err(e) => {
                        tracing::error!(error = ?e, "fatal ingest accept error");
                        break;
                    }
                }
            }
            _ = shutdown.cancelled() => {
                break;
            }
        }
    }

    let _ = std::fs::remove_file(&sock_path);
    Ok(())
}
