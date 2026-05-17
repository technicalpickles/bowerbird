use std::path::PathBuf;

use tokio::net::UnixListener;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::error::{Error, Result};

pub async fn run(
    sock_path: PathBuf,
    tx: mpsc::Sender<protocol::EventEnvelope>,
    shutdown: CancellationToken,
) -> Result<()> {
    let _ = std::fs::remove_file(&sock_path);

    let listener = UnixListener::bind(&sock_path)
        .map_err(|e| Error::Ingest(format!("bind {}: {e}", sock_path.display())))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&sock_path, std::fs::Permissions::from_mode(0o600))
            .map_err(|e| Error::Ingest(format!("set_permissions on ingest.sock: {e}")))?;
    }

    loop {
        tokio::select! {
            accept_result = listener.accept() => {
                match accept_result {
                    Ok((stream, _)) => {
                        tokio::spawn(super::handler::handle(stream, tx.clone()));
                    }
                    Err(e) => {
                        tracing::warn!(error = ?e, "ingest accept error");
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
