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

    /// Inspect the tool-reactions TOML for misconfiguration at startup.
    /// Returns a list of human-readable warnings; empty when the file is clean.
    /// The adapter has no tracing of its own (it's a pure library); callers
    /// surface these to their own logging layer.
    pub fn validate_config(&self) -> Vec<String> {
        normalize::validate_config(&self.tool_reactions_path)
    }
}

impl SourceAdapter for ClaudeAdapter {
    fn meta(&self) -> AdapterMeta {
        AdapterMeta {
            source: normalize::SOURCE,
        }
    }

    fn normalize(&self, hook_kind: &str, raw: &[u8]) -> protocol::Result<NormalizeResult> {
        normalize::normalize(&self.tool_reactions_path, hook_kind, raw)
            .map_err(protocol::Error::from)
    }
}
