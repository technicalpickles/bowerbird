use std::path::PathBuf;

/// Errors returned by the install/uninstall settings.json surface.
///
/// `pub` so binaries can branch on the failure mode (e.g. surface a different
/// stderr message for race-exhaustion vs JSON parse failure). The internal
/// normalize-path variants remain `pub(crate)` via [`NormalizeError`].
#[derive(Debug, thiserror::Error)]
pub enum InstallError {
    #[error("failed to read {path}: {source}")]
    SettingsRead {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to parse {path} as JSON: {source}")]
    SettingsParse {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("{path} is not a JSON object (top-level value must be an object)")]
    SettingsNotObject { path: PathBuf },
    #[error("failed to write {path}: {source}")]
    SettingsWriteTmp {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error(
        "atomic rename for {path} lost {attempts} race(s) against concurrent writer; \
         settings.json was modified by another process during install"
    )]
    SettingsAtomicRenameRace { path: PathBuf, attempts: u32 },
    #[error("failed to rename tmp file over {path}: {source}")]
    SettingsRename {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum NormalizeError {
    #[error("invalid UTF-8 in payload: {0}")]
    InvalidUtf8(#[from] std::str::Utf8Error),
    #[error("json parse error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("missing required field: {0}")]
    MissingField(&'static str),
    #[error("unrecognized hook kind: {0}")]
    InvalidHookKind(String),
}

impl From<NormalizeError> for protocol::Error {
    fn from(e: NormalizeError) -> Self {
        match e {
            NormalizeError::InvalidHookKind(k) => protocol::Error::UnknownHookKind(k),
            other => protocol::Error::Serde(other.to_string()),
        }
    }
}
