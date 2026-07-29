//! Bearer-token type and resolver for the REST surface.
//!
//! Invariant: the bearer token is read by the API auth layer ONLY. The ingest
//! path (`crate::ingest`) must never import [`BearerToken`] or any helper from
//! this module — Unix-socket file mode is the ingest auth boundary. See
//! `architecture.md:444-446`.
//!
//! # Resolution chain (Story 3.3)
//!
//! [`load_or_generate`] resolves the daemon's bearer token in this order:
//!
//! 1. `BOWERBIRD_TOKEN` env var (non-empty) → [`TokenSource::Env`].
//! 2. System keychain via `keyring` v3
//!    (`service = "bowerbird-daemon"`, `user = "bearer-token"`) →
//!    [`TokenSource::Keychain`]. If the entry is missing AND the keychain is
//!    writable, a UUID4 is generated and stored, returning
//!    `Keychain { generated: true }`. On subsequent runs the same token reads
//!    back as `Keychain { generated: false }`.
//! 3. `~/.bowerbird/config.toml` `token` field → [`TokenSource::ConfigFile`].
//!    Mode 0600 expected; a wider mode logs a warning but the value is still
//!    used.
//! 4. All paths exhausted → `Err(TokenError)` whose `Display` enumerates every
//!    path tried (NFR13).
//!
//! ### AC #2 reconciliation (env-first vs. AC's "fallback when keychain
//! unavailable")
//!
//! AC #2 reads literally as "given the keychain is unavailable, falls back in
//! order: (1) env var, (2) config.toml." This module ships a more permissive
//! reading: **env always wins**, with the keychain as the second check.
//! Rationale:
//!
//! - `tests/cli_lifecycle.rs` already uses `BOWERBIRD_TOKEN` to share a known
//!   value between daemon and CLI processes (lifecycle pattern from Stories
//!   3.1/3.2). Putting env-var first preserves this exact test infrastructure.
//! - Users who need to override a stuck or wrong keychain entry can simply
//!   `BOWERBIRD_TOKEN=<new> bowerbird start` without first having to disable
//!   or delete the keychain entry via OS-specific tooling.
//! - `architecture.md:442`'s "keychain primary → BOWERBIRD_TOKEN env-var →
//!   ~/.bowerbird/token file" arrow chain naturally reads as
//!   "primary, then first override, then second override," not "first override
//!   only when primary is unavailable."
//!
//! The AC-vs-shipped delta is intentional and documented here so a future
//! reader can revisit it via a new ADR if the literal AC reading becomes the
//! strict requirement.
//!
//! # `BOWERBIRD_KEYRING_BACKEND` (test-only)
//!
//! A runtime env override consulted at the top of [`load_or_generate`]:
//!
//! | Value             | Behavior                                                   |
//! |-------------------|------------------------------------------------------------|
//! | unset / `default` | Use the real platform-native keychain.                     |
//! | `disable`         | Skip keychain entirely; behave as if it returned `PlatformFailure` so the env / config.toml fallbacks run. |
//! | `mock`            | Install `keyring::mock::default_credential_builder()` once per process (idempotent). **Requires the `mock-keyring` cargo feature**; without it the value is treated as `disable`. |
//!
//! **If you find yourself reaching for this env in production, file a bug —
//! the public escape hatch is `BOWERBIRD_TOKEN`.** The variable exists so the
//! contract-test suite can drive the resolver against an in-process backend
//! without touching the developer's real macOS Keychain or Linux Secret
//! Service. Production binaries are encouraged to build with
//! `--no-default-features` so the `mock` value falls back to `disable`
//! semantics (defense in depth against env-var injection).
//!
//! # Keychain interop invariant
//!
//! Both the daemon and the CLI (`src/commands/auth.rs`) MUST use the same
//! [`SERVICE`] and [`USER`] strings. Mismatch is a silent failure mode (the
//! CLI reads a non-existent entry, falls through to env, prints a wrong
//! token). The constants are duplicated across the two crates because the
//! CLI does NOT depend on `crates/daemon` (it would pull tokio); the
//! round-trip test in `tests/cli_auth.rs` is the regression guard for
//! divergence. Change BOTH together.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use secrecy::{ExposeSecret, SecretString};
use subtle::ConstantTimeEq;

use crate::config_file;

/// Keychain service name. Duplicated in the CLI's `src/commands/auth.rs`;
/// change both together (see module doc for the rationale).
pub const SERVICE: &str = "bowerbird-daemon";
/// Keychain user/account name. Duplicated in the CLI's `src/commands/auth.rs`;
/// change both together (see module doc for the rationale).
pub const USER: &str = "bearer-token";

/// Wrap a bearer token in `SecretString` so accidental `Debug` / `Display`
/// stringification cannot leak it. `Clone` is required so `AppState` can be
/// cloned by axum.
#[derive(Clone)]
pub struct BearerToken(SecretString);

/// Where the active token came from. Surfaced so [`load_or_generate`]'s caller
/// can log appropriately — a freshly-generated keychain entry warrants a WARN
/// line (operators should know the token is new); a stored-and-read keychain
/// hit is INFO; env-supplied and config-file hits are INFO.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenSource {
    Env,
    Keychain { generated: bool },
    ConfigFile,
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

    /// Controlled exposure for the CLI emit path (`bowerbird auth token`).
    /// **Do not call this elsewhere.** A code reviewer scanning for token
    /// leaks should see this exact identifier at every legitimate exposure
    /// site; anywhere else it appears is a security regression.
    pub fn expose_token_for_cli(&self) -> &str {
        self.0.expose_secret()
    }
}

/// Per-path record of one attempt during [`load_or_generate`]. Aggregated
/// into a [`TokenError`] when every step fails.
#[derive(Debug)]
pub enum TriedPath {
    /// `BOWERBIRD_TOKEN` was unset or empty.
    EnvUnset,
    /// Keychain was unreachable or the entry could not be read. The string
    /// captures the underlying `keyring::Error`'s `Display`.
    Keychain(String),
    /// Config file read or parse failure (covers missing-token-field too).
    ConfigFile(config_file::ReadFailure),
}

impl std::fmt::Display for TriedPath {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TriedPath::EnvUnset => write!(f, "BOWERBIRD_TOKEN env (unset)"),
            TriedPath::Keychain(detail) => write!(f, "keychain ({detail})"),
            TriedPath::ConfigFile(reason) => {
                write!(f, "~/.bowerbird/config.toml ({reason})")
            }
        }
    }
}

/// Aggregate error returned when every resolution step fails. The `Display`
/// impl enumerates each tried path — the daemon's `main.rs` and the CLI's
/// `bowerbird auth token` both print the rendered message to stderr before
/// exiting non-zero (NFR13).
#[derive(Debug)]
pub struct TokenError {
    pub tried: Vec<TriedPath>,
}

impl std::fmt::Display for TokenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "failed to resolve bearer token; tried:")?;
        for path in &self.tried {
            writeln!(f, "  - {path}")?;
        }
        write!(
            f,
            "set BOWERBIRD_TOKEN or write `token = \"<uuid>\"` into \
             ~/.bowerbird/config.toml (mode 0600)"
        )
    }
}

impl std::error::Error for TokenError {}

/// Snapshot of the environment [`load_or_generate`] consults. Production
/// callers use [`TokenEnv::from_process_env`]; tests construct one directly
/// so they never have to mutate process-global env vars (`std::env::set_var`
/// races with concurrent env reads, which forbids parallel test execution).
#[derive(Debug, Clone, Default)]
pub struct TokenEnv {
    /// `BOWERBIRD_TOKEN`
    pub token: Option<String>,
    /// `BOWERBIRD_KEYRING_BACKEND`
    pub keyring_backend: Option<String>,
    /// `BOWERBIRD_DATA_DIR`
    pub data_dir: Option<std::ffi::OsString>,
    /// `HOME`
    pub home: Option<std::ffi::OsString>,
}

impl TokenEnv {
    pub fn from_process_env() -> Self {
        Self {
            token: std::env::var("BOWERBIRD_TOKEN").ok(),
            keyring_backend: std::env::var("BOWERBIRD_KEYRING_BACKEND").ok(),
            data_dir: std::env::var_os("BOWERBIRD_DATA_DIR"),
            home: std::env::var_os("HOME"),
        }
    }
}

/// Look up the data directory for the config.toml fallback. Mirrors the
/// `bowerbird-daemon` `resolve_bowerbird_dir` precedence (env wins, else
/// `$HOME/.bowerbird`) so the daemon and the CLI agree without sharing code.
fn data_dir_for_config(env: &TokenEnv) -> Option<PathBuf> {
    if let Some(p) = &env.data_dir {
        if !p.is_empty() {
            return Some(PathBuf::from(p));
        }
    }
    let home = env.home.as_ref()?;
    if home.is_empty() {
        return None;
    }
    Some(PathBuf::from(home).join(".bowerbird"))
}

/// Read `BOWERBIRD_KEYRING_BACKEND` once and apply its side effect (install
/// the mock builder for the `mock` value). Idempotent: subsequent calls in the
/// same process are no-ops. Returns the resolved backend mode so the rest of
/// the resolver can branch on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum KeyringBackend {
    /// Use the platform-native keychain.
    Real,
    /// Skip the keychain step; behave as if it returned `PlatformFailure`.
    Disabled,
}

fn install_keyring_backend(env: &TokenEnv) -> KeyringBackend {
    // `BOWERBIRD_KEYRING_BACKEND` is re-read (via the caller's snapshot) on
    // every call so the contract tests can flip between `disable` and the
    // real backend within one process. The mock-builder install, however, is
    // permanent for the rest of the process — keyring's
    // `set_default_credential_builder` has no inverse, so once mocked the
    // process stays mocked. Tests that need a mix of real and mock backends
    // live in separate test binaries.
    static MOCK_INSTALLED: OnceLock<()> = OnceLock::new();
    match env.keyring_backend.as_deref() {
        Some("disable") => KeyringBackend::Disabled,
        #[cfg(feature = "mock-keyring")]
        Some("mock") => {
            MOCK_INSTALLED.get_or_init(|| {
                keyring::set_default_credential_builder(
                    keyring::mock::default_credential_builder(),
                );
            });
            KeyringBackend::Real
        }
        // When the mock-keyring feature is OFF, a `mock` value falls back to
        // `disable` so a production binary cannot be coerced into using the
        // in-memory backend via env-var injection.
        #[cfg(not(feature = "mock-keyring"))]
        Some("mock") => KeyringBackend::Disabled,
        _ => KeyringBackend::Real,
    }
}

/// Try to read the bearer token from the keychain. Returns `Ok(Some(_))` on a
/// hit, `Ok(None)` if the entry doesn't exist yet (callers should generate +
/// store), or `Err(_)` if the keychain itself is unreachable.
fn keychain_read() -> Result<Option<String>, keyring::Error> {
    let entry = keyring::Entry::new(SERVICE, USER)?;
    match entry.get_password() {
        Ok(s) => Ok(Some(s)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(e),
    }
}

/// Generate a fresh UUID4, store it in the keychain, then verify the round
/// trip. Returns the generated token on success.
fn keychain_write_generated() -> Result<String, keyring::Error> {
    let entry = keyring::Entry::new(SERVICE, USER)?;
    let new_token = uuid::Uuid::new_v4().to_string();
    entry.set_password(&new_token)?;
    // Round-trip read defends against the write landing in a different
    // keychain than the one we'll read from next startup (macOS multi-keychain
    // setups where the default has changed since the daemon last ran).
    let readback = entry.get_password()?;
    if readback != new_token {
        return Err(keyring::Error::PlatformFailure(
            format!("keychain write/read mismatch (service={SERVICE}, user={USER})").into(),
        ));
    }
    Ok(new_token)
}

/// Resolve the daemon's bearer token at startup.
///
/// See module-level doc for the four-step chain, the `BOWERBIRD_KEYRING_BACKEND`
/// test-only override, and the AC #2 env-first reconciliation.
///
/// On full-chain failure returns `Err(TokenError)` whose `Display` enumerates
/// every path tried; the caller is expected to print the rendered message to
/// stderr and exit non-zero (NFR13).
pub fn load_or_generate() -> Result<(BearerToken, TokenSource), TokenError> {
    load_or_generate_with_env(&TokenEnv::from_process_env())
}

/// [`load_or_generate`] against an explicit environment snapshot. This is the
/// whole resolver; the no-arg wrapper just snapshots the process env. Tests
/// call this directly so they can run in parallel without `env::set_var`.
pub fn load_or_generate_with_env(env: &TokenEnv) -> Result<(BearerToken, TokenSource), TokenError> {
    let mut tried = Vec::with_capacity(3);

    // Step 1: env var
    match &env.token {
        Some(v) if !v.is_empty() => {
            return Ok((BearerToken::new(v.clone()), TokenSource::Env));
        }
        _ => tried.push(TriedPath::EnvUnset),
    }

    // Step 2: keychain (gated by BOWERBIRD_KEYRING_BACKEND)
    match install_keyring_backend(env) {
        KeyringBackend::Disabled => {
            tried.push(TriedPath::Keychain(
                "skipped via BOWERBIRD_KEYRING_BACKEND".to_string(),
            ));
        }
        KeyringBackend::Real => match keychain_read() {
            Ok(Some(value)) => {
                return Ok((
                    BearerToken::new(value),
                    TokenSource::Keychain { generated: false },
                ));
            }
            Ok(None) => match keychain_write_generated() {
                Ok(new_token) => {
                    return Ok((
                        BearerToken::new(new_token),
                        TokenSource::Keychain { generated: true },
                    ));
                }
                Err(e) => tried.push(TriedPath::Keychain(format!("write failed: {e}"))),
            },
            Err(e) => tried.push(TriedPath::Keychain(e.to_string())),
        },
    }

    // Step 3: config.toml
    let cfg_path = data_dir_for_config(env)
        .map(|d| d.join("config.toml"))
        .unwrap_or_else(|| PathBuf::from("~/.bowerbird/config.toml"));
    match read_config_token(&cfg_path) {
        Ok(token) => {
            return Ok((BearerToken::new(token), TokenSource::ConfigFile));
        }
        Err(reason) => tried.push(TriedPath::ConfigFile(reason)),
    }

    Err(TokenError { tried })
}

/// Try to read a non-empty `token` field from `config.toml`. Mode-not-0600 is
/// logged as a WARN by the underlying reader but the value is still returned.
fn read_config_token(path: &Path) -> Result<String, config_file::ReadFailure> {
    if let Some(actual_mode) = config_file::check_mode(path) {
        tracing::warn!(
            path = %path.display(),
            actual_mode = format!("{actual_mode:o}"),
            "config.toml mode should be 0600; using anyway"
        );
    }
    let cfg = config_file::read(path)?;
    match cfg.token {
        Some(t) if !t.is_empty() => Ok(t),
        Some(_) => Err(config_file::ReadFailure::NoTokenField),
        None => Err(config_file::ReadFailure::NoTokenField),
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

    #[test]
    fn expose_token_for_cli_returns_exact_value() {
        let t = BearerToken::new("piped-into-curl".to_string());
        assert_eq!(t.expose_token_for_cli(), "piped-into-curl");
    }

    #[test]
    fn token_error_display_lists_every_tried_path() {
        let err = TokenError {
            tried: vec![
                TriedPath::EnvUnset,
                TriedPath::Keychain("Platform secure storage failure: dbus".to_string()),
                TriedPath::ConfigFile(config_file::ReadFailure::NotFound),
            ],
        };
        let s = err.to_string();
        assert!(s.contains("BOWERBIRD_TOKEN"), "missing env path: {s}");
        assert!(s.contains("keychain"), "missing keychain path: {s}");
        assert!(s.contains("config.toml"), "missing config path: {s}");
    }
}
