use std::net::SocketAddr;
use std::path::PathBuf;

use crate::error::{Error, Result};

#[derive(Debug, Clone)]
pub struct Config {
    pub data_dir: PathBuf,
    pub ingest_socket_path: PathBuf,
    pub tcp_addr: SocketAddr,
    pub writer_pool_max: usize,
    pub reader_pool_max: usize,
    pub ingest_channel_capacity: usize,
}

impl Config {
    pub fn from_env() -> Result<Self> {
        let home = std::env::var("HOME")
            .map_err(|_| Error::Config("HOME environment variable not set".into()))?;
        let data_dir = PathBuf::from(home).join(".bowerbird");
        Self::with_data_dir(data_dir)
    }

    pub fn with_data_dir(data_dir: PathBuf) -> Result<Self> {
        let ingest_socket_path = data_dir.join("ingest.sock");
        let tcp_addr: SocketAddr = "127.0.0.1:38121"
            .parse()
            .map_err(|e: std::net::AddrParseError| Error::Config(e.to_string()))?;
        Ok(Self {
            data_dir,
            ingest_socket_path,
            tcp_addr,
            writer_pool_max: 1,
            reader_pool_max: 4,
            ingest_channel_capacity: 1024,
        })
    }

    pub fn db_path(&self) -> PathBuf {
        self.data_dir.join("bowerbird.sqlite")
    }
}
