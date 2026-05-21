use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::path::PathBuf;
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct Config {
    pub db_path: PathBuf,
    pub bind_addr: SocketAddr,
    pub ingest_channel_capacity: usize,
    pub ingest_sock_path: PathBuf,
    pub tool_reactions_path: PathBuf,
    pub ws_max_connections: usize,
    pub ws_ping_interval: Duration,
    pub ws_pong_timeout: Duration,
    pub ws_broadcast_capacity: usize,
}

impl Config {
    pub fn with_bowerbird_dir(bowerbird_dir: &std::path::Path) -> Self {
        Self {
            db_path: bowerbird_dir.join("bower.db"),
            bind_addr: SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::new(127, 0, 0, 1), 0)),
            ingest_channel_capacity: 1024,
            ingest_sock_path: bowerbird_dir.join("ingest.sock"),
            tool_reactions_path: bowerbird_dir.join("adapters/claude/tool-reactions.toml"),
            ws_max_connections: 256,
            ws_ping_interval: Duration::from_secs(30),
            ws_pong_timeout: Duration::from_secs(10),
            ws_broadcast_capacity: 1024,
        }
    }
}
