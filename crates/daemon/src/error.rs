use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("sqlite error: {0}")]
    Db(#[from] rusqlite::Error),

    #[error("pool error: {0}")]
    Pool(#[from] deadpool_sqlite::PoolError),

    #[error("pool build error: {0}")]
    PoolBuild(#[from] deadpool_sqlite::BuildError),

    #[error("pool create error: {0}")]
    PoolCreate(#[from] deadpool_sqlite::CreatePoolError),

    #[error("pool interact error: {0}")]
    Interact(String),

    #[error("migration error: {0}")]
    Migration(#[from] rusqlite_migration::Error),

    #[error("bind error: {0}")]
    Bind(std::io::Error),

    #[error("config error: {0}")]
    Config(String),

    #[error("task panic: {0}")]
    TaskPanic(String),
}

impl From<deadpool_sqlite::InteractError> for Error {
    fn from(value: deadpool_sqlite::InteractError) -> Self {
        Error::Interact(value.to_string())
    }
}

pub type Result<T> = std::result::Result<T, Error>;
