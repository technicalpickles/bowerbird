use std::collections::HashMap;
use std::path::Path;
use std::str::FromStr;

use protocol::{EventEnvelope, EventKind, NormalizeResult, Reaction};
use serde::Deserialize;

use crate::error::Error;

#[derive(Deserialize)]
struct ToolReactionsFile {
    tool_reactions: HashMap<String, String>,
}

fn parse_reaction(s: &str) -> Reaction {
    match s {
        "Pause" => Reaction::Pause,
        "Continue" => Reaction::Continue,
        "Unknown" => Reaction::Unknown,
        other => {
            if let Some(inner) = other
                .strip_prefix("Vendor(")
                .and_then(|t| t.strip_suffix(')'))
            {
                if let Ok(n) = u16::from_str(inner) {
                    return Reaction::Vendor(n);
                }
            }
            Reaction::Unknown
        }
    }
}

fn load_reaction(toml_path: &Path, tool_name: &str) -> Reaction {
    let contents = match std::fs::read_to_string(toml_path) {
        Ok(c) => c,
        Err(_) => return Reaction::Unknown,
    };
    let config: ToolReactionsFile = match toml::from_str(&contents) {
        Ok(c) => c,
        Err(_) => return Reaction::Unknown,
    };
    config
        .tool_reactions
        .get(tool_name)
        .map(|s| parse_reaction(s))
        .unwrap_or(Reaction::Unknown)
}

pub(crate) fn normalize(
    toml_path: &Path,
    hook_kind: &str,
    raw: &[u8],
) -> Result<NormalizeResult, Error> {
    let payload = String::from_utf8(raw.to_vec()).map_err(|e| Error::InvalidUtf8(e.to_string()))?;

    let value: serde_json::Value = serde_json::from_str(&payload)?;

    let session_id = value
        .get("session_id")
        .and_then(|v| v.as_str())
        .ok_or(Error::MissingField("session_id"))?
        .to_string();

    let kind = match hook_kind {
        "PreToolUse" => EventKind::PreToolUse,
        "PostToolUse" => EventKind::PostToolUse,
        "Stop" => EventKind::Stop,
        "Notification" => EventKind::Notification,
        other => return Err(Error::InvalidHookKind(other.to_string())),
    };

    let reaction = match kind {
        EventKind::PreToolUse => {
            let tool_name = value
                .get("tool_name")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            Some(load_reaction(toml_path, tool_name))
        }
        _ => None,
    };

    Ok(NormalizeResult {
        envelope: EventEnvelope {
            source: "claude".to_string(),
            session_id,
            kind,
            reaction,
            payload,
        },
    })
}
