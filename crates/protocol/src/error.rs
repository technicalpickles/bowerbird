#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("serde error: {0}")]
    Serde(String),
}

pub type Result<T> = std::result::Result<T, Error>;
