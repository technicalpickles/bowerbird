//! Bearer-token type and loader for the REST surface.
//!
//! Invariant: the bearer token is read by the API auth layer ONLY. The ingest
//! path (`crate::ingest`) must never import [`BearerToken`] or any helper from
//! this module — Unix-socket file mode is the ingest auth boundary. See
//! `architecture.md:444-446`.
//!
//! V1 resolution order in [`load_or_generate`]:
//!
//! 1. `BOWERBIRD_TOKEN` env var (non-empty) — [`TokenSource::Env`].
//! 2. Fresh UUID4 — [`TokenSource::Generated`], logged at WARN by the caller.
//!
//! Story 3.3 will extend the chain with keychain + file fallback. The
//! validation layer below (`BearerToken::verify`) does not change across that
//! migration; only the issuance source does.

use secrecy::{ExposeSecret, SecretString};
use subtle::ConstantTimeEq;

/// Wrap a bearer token in `SecretString` so accidental `Debug` / `Display`
/// stringification cannot leak it. `Clone` is required so `AppState` can be
/// cloned by axum.
#[derive(Clone)]
pub struct BearerToken(SecretString);

/// Where the active token came from. Surfaced so [`load_or_generate`]'s caller
/// can log appropriately — generated tokens warrant a WARN line, env-supplied
/// tokens don't.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenSource {
    Env,
    Generated,
}

impl BearerToken {
    pub fn new(s: String) -> Self {
        Self(SecretString::from(s))
    }

    pub fn generate_uuid4() -> Self {
        Self::new(uuid::Uuid::new_v4().to_string())
    }

    /// Constant-time compare against a candidate token. Wrong-length tokens
    /// still take O(min(left, right)) cycles via `subtle::ConstantTimeEq` —
    /// do NOT short-circuit on length, which leaks the token length.
    pub fn verify(&self, candidate: &str) -> bool {
        let stored = self.0.expose_secret().as_bytes();
        let candidate = candidate.as_bytes();
        stored.ct_eq(candidate).into()
    }
}

/// Resolve the daemon's bearer token at startup.
///
/// Returns the token plus a [`TokenSource`] tag. The caller is expected to
/// emit a WARN log (without the token value) when the source is `Generated`,
/// matching the bind-address WARN pattern in `main.rs`.
pub fn load_or_generate() -> (BearerToken, TokenSource) {
    match std::env::var("BOWERBIRD_TOKEN") {
        Ok(v) if !v.is_empty() => (BearerToken::new(v), TokenSource::Env),
        _ => (BearerToken::generate_uuid4(), TokenSource::Generated),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verify_accepts_exact_match() {
        let t = BearerToken::new("abc123".to_string());
        assert!(t.verify("abc123"));
    }

    #[test]
    fn verify_rejects_wrong_value() {
        let t = BearerToken::new("abc123".to_string());
        assert!(!t.verify("abc124"));
    }

    #[test]
    fn verify_rejects_wrong_length() {
        let t = BearerToken::new("abc123".to_string());
        assert!(!t.verify("abc1234"));
        assert!(!t.verify("abc12"));
        assert!(!t.verify(""));
    }

    #[test]
    fn generate_uuid4_yields_unique_tokens() {
        let a = BearerToken::generate_uuid4();
        let b = BearerToken::generate_uuid4();
        // UUID4 collision is astronomically unlikely; assert by verifying one
        // against the other's exposed value.
        let a_str = a.0.expose_secret().to_string();
        assert!(!b.verify(&a_str));
    }
}
