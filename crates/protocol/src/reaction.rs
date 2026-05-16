use serde::{de, Deserialize, Deserializer, Serialize, Serializer};

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
            let n = inner
                .parse::<u16>()
                .map_err(|_| de::Error::custom(format!("invalid Vendor reaction: {s}")))?;
            return Ok(Reaction::Vendor(n));
        }
        Err(de::Error::custom(format!("unknown Reaction: {s}")))
    }
}
