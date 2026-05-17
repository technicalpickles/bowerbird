use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct Config {
    pub db_path: PathBuf,
    pub bind_addr: SocketAddr,
    pub ingest_channel_capacity: usize,
    pub ingest_sock_path: PathBuf,
}

impl Config {
    pub fn with_bowerbird_dir(bowerbird_dir: &std::path::Path) -> Self {
        Self {
            db_path: bowerbird_dir.join("bower.db"),
            bind_addr: SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::new(127, 0, 0, 1), 0)),
            ingest_channel_capacity: 1024,
            ingest_sock_path: bowerbird_dir.join("ingest.sock"),
        }
    }
}
