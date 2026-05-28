use std::collections::HashMap;
use std::path::Path;
use std::str::FromStr;

use protocol::{EventEnvelope, EventKind, NormalizeResult, NotificationType, Reaction};
use serde::Deserialize;

use crate::error::NormalizeError as Error;

pub(crate) const SOURCE: &str = "claude";

#[derive(Deserialize)]
struct ToolReactionsFile {
    tool_reactions: HashMap<String, String>,
}

pub(crate) fn parse_reaction(s: &str) -> Reaction {
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
    let payload = std::str::from_utf8(raw)
        .map_err(Error::InvalidUtf8)?
        .to_owned();

    let value: serde_json::Value = serde_json::from_str(&payload)?;

    // Match hook_kind BEFORE extracting other payload fields. Story 1.8 review
    // finding: an unknown hook_kind must surface as InvalidHookKind regardless
    // of whether session_id / tool_name are also missing or wrong-type, so the
    // daemon emits `400 unknown hook_kind: <value>` and not the generic
    // `400 normalize error: missing required field: session_id`.
    let kind = match hook_kind {
        "UserPromptSubmit" => EventKind::UserPromptSubmit,
        "PreToolUse" => EventKind::PreToolUse,
        "PostToolUse" => EventKind::PostToolUse,
        "Stop" => EventKind::Stop,
        "Notification" => EventKind::Notification,
        other => return Err(Error::InvalidHookKind(other.to_string())),
    };

    let session_id = value
        .get("session_id")
        .and_then(|v| v.as_str())
        .ok_or(Error::MissingField("session_id"))?
        .to_string();

    let reaction = match kind {
        EventKind::PreToolUse => {
            let tool_name = value
                .get("tool_name")
                .and_then(|v| v.as_str())
                .ok_or(Error::MissingField("tool_name"))?;
            Some(load_reaction(toml_path, tool_name))
        }
        _ => None,
    };

    // Story 5.3 AC #2: extract bowerbird_ppid (injected by the shim). Missing,
    // non-integer, or out-of-range values all yield pid = None — adapter must
    // not fail normalization for a missing/malformed ppid.
    //
    // PID 0 is rejected at the boundary: `kill(0, 0)` has process-group
    // semantics (signals every member of the caller's group), not single-
    // process semantics, so a `bowerbird_ppid: 0` from a malformed or custom
    // producer would make the liveness probe always report "alive" and pin the
    // session as immortal. Treat 0 as if the field were absent.
    let pid = value
        .get("bowerbird_ppid")
        .and_then(|v| v.as_u64())
        .and_then(|n| u32::try_from(n).ok())
        .filter(|n| *n != 0);

    // Story 5.3 AC #3: extract typed notification_type only for Notification
    // hooks; non-Notification kinds with a stray notification_type field
    // ignore it (returns None).
    let notification_type = if matches!(kind, EventKind::Notification) {
        value
            .get("notification_type")
            .and_then(|v| v.as_str())
            .map(|s| match s {
                "permission_prompt" => NotificationType::PermissionPrompt,
                "idle_prompt" => NotificationType::IdlePrompt,
                "auth_success" => NotificationType::AuthSuccess,
                "elicitation_dialog" => NotificationType::ElicitationDialog,
                "elicitation_response" => NotificationType::ElicitationResponse,
                "elicitation_complete" => NotificationType::ElicitationComplete,
                _ => NotificationType::Unknown,
            })
    } else {
        None
    };

    Ok(NormalizeResult {
        envelope: EventEnvelope {
            source: SOURCE.to_string(),
            session_id,
            kind,
            reaction,
            payload,
            pid,
            notification_type,
        },
    })
}

// Surface misconfiguration at startup. normalize() itself degrades to Unknown
// on the same failures; this gives operators a one-shot signal at boot.
pub(crate) fn validate_config(toml_path: &Path) -> Vec<String> {
    let mut issues = Vec::new();
    let contents = match std::fs::read_to_string(toml_path) {
        Ok(c) => c,
        Err(e) => {
            issues.push(format!(
                "could not read {}: {} (all reactions will be Unknown until fixed)",
                toml_path.display(),
                e
            ));
            return issues;
        }
    };
    let config: ToolReactionsFile = match toml::from_str(&contents) {
        Ok(c) => c,
        Err(e) => {
            issues.push(format!(
                "could not parse {}: {} (all reactions will be Unknown until fixed)",
                toml_path.display(),
                e
            ));
            return issues;
        }
    };
    for (tool, value) in &config.tool_reactions {
        if value == "Unknown" {
            continue;
        }
        if matches!(parse_reaction(value), Reaction::Unknown) {
            issues.push(format!(
                "tool '{tool}' has unrecognized reaction value '{value}' (treated as Unknown)"
            ));
        }
    }
    issues
}
