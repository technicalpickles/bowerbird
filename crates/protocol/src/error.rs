// Error surface between source adapters and the daemon. Adapters convert
// their internal errors into this enum via `From`; the daemon matches on it
// to choose the wire response.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("serde error: {0}")]
    Serde(String),
    #[error("unknown hook_kind: {0}")]
    UnknownHookKind(String),
}

pub type Result<T> = std::result::Result<T, Error>;
