use std::path::PathBuf;

use protocol::{AdapterMeta, NormalizeResult, SourceAdapter};

pub(crate) mod error;
pub(crate) mod normalize;

pub struct ClaudeAdapter {
    tool_reactions_path: PathBuf,
}

impl ClaudeAdapter {
    pub fn new(tool_reactions_path: PathBuf) -> Self {
        Self {
            tool_reactions_path,
        }
    }
}

impl SourceAdapter for ClaudeAdapter {
    fn meta(&self) -> AdapterMeta {
        AdapterMeta { source: "claude" }
    }

    fn normalize(&self, hook_kind: &str, raw: &[u8]) -> protocol::Result<NormalizeResult> {
        normalize::normalize(&self.tool_reactions_path, hook_kind, raw)
            .map_err(protocol::Error::from)
    }
}
