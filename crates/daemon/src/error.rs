#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("pool error: {0}")]
    Pool(String),
    #[error("migration error: {0}")]
    Migration(String),
    #[error("clock error: {0}")]
    Clock(String),
    #[error("ingest error: {0}")]
    Ingest(String),
    #[error("projection error: {0}")]
    Projection(String),
}

pub type Result<T> = std::result::Result<T, Error>;
