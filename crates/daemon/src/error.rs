#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("pool error: {0}")]
    Pool(String),
    #[error("migration error: {0}")]
    Migration(String),
}

pub type Result<T> = std::result::Result<T, Error>;
