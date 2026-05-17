#[derive(Debug, thiserror::Error)]
pub(crate) enum Error {
    #[error("invalid UTF-8 in payload: {0}")]
    InvalidUtf8(String),
    #[error("json parse error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("missing required field: {0}")]
    MissingField(&'static str),
    #[error("unrecognized hook kind: {0}")]
    InvalidHookKind(String),
}

impl From<Error> for protocol::Error {
    fn from(e: Error) -> Self {
        protocol::Error::Serde(e.to_string())
    }
}
