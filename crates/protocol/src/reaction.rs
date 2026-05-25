use serde::{de, Deserialize, Deserializer, Serialize, Serializer};

/// Tool reaction emitted on `PreToolUse` events.
///
/// `Pause` and `Continue` are the named variants the daemon ships today;
/// `Vendor(u16)` is the open-ended namespace for tool-supplied reactions
/// (round-trips via the `"Vendor(<n>)"` string form). **`Unknown` is the
/// decode-only catch-all** the additive-compat policy requires:
/// `Deserialize` maps any unrecognized JSON string into `Reaction::Unknown`
/// instead of returning an error, so v1.0 presenters continue to decode
/// events whose reaction is a future v1.x addition (e.g. `"Block"`).
///
/// This catch-all behavior is the load-bearing fix Story 4.4 lands (Epic 2
/// retro AI-4); prior to 2026-05-25 the final deserialize branch returned
/// `Err(...)`, which would have broken the "additive within v1.x" claim the
/// moment a new reaction string shipped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reaction {
    Pause,
    Continue,
    Vendor(u16),
    Unknown,
}

impl Serialize for Reaction {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let s = match self {
            Reaction::Pause => "Pause".to_owned(),
            Reaction::Continue => "Continue".to_owned(),
            Reaction::Vendor(n) => format!("Vendor({n})"),
            Reaction::Unknown => "Unknown".to_owned(),
        };
        serializer.serialize_str(&s)
    }
}

impl<'de> Deserialize<'de> for Reaction {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        if s == "Pause" {
            return Ok(Reaction::Pause);
        }
        if s == "Continue" {
            return Ok(Reaction::Continue);
        }
        if s == "Unknown" {
            return Ok(Reaction::Unknown);
        }
        if let Some(inner) = s.strip_prefix("Vendor(").and_then(|t| t.strip_suffix(')')) {
            // A `Vendor(...)` prefix with a non-u16 inner is still a malformed
            // payload, not an additive-compat case; preserve the error to surface
            // protocol drift. Truly-unknown reaction strings fall through to
            // `Unknown` below per the additive-compat policy.
            let n = inner
                .parse::<u16>()
                .map_err(|_| de::Error::custom(format!("invalid Vendor reaction: {s}")))?;
            return Ok(Reaction::Vendor(n));
        }
        // Additive-compat catch-all (Story 4.4, Epic 2 retro AI-4). A future
        // v1.x daemon may emit a new reaction string (e.g. `"Block"`); v1.0
        // presenters must decode it as `Reaction::Unknown` rather than failing
        // the whole `Event` parse. The daemon never CONSTRUCTS `Unknown` (it
        // only emits `Pause | Continue | Vendor(_)`), so this branch is
        // strictly a wire-decode safety net.
        Ok(Reaction::Unknown)
    }
}
