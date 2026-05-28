//! Atomic install/uninstall of the bowerbird hook entries in Claude Code's
//! `~/.claude/settings.json`.
//!
//! The contract is the same one in `project-context.md` for every config-file
//! mutation the project makes: read → parse → merge → write `.tmp` → fsync →
//! rename. Never open the original for writing. If the process dies after any
//! step before `rename`, the original is intact.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{Map, Value};

use crate::error::InstallError;

/// Hook kinds the bowerbird shim accepts on its CLI (see
/// `crates/shim/src/main.rs::parse_hook_kind`). Listed in a stable order so
/// install output (and the resulting JSON) is deterministic across runs.
/// Chronological-lifecycle order: a user submits, the agent runs tools,
/// the agent stops, the agent waits for input.
pub(crate) const HOOK_KINDS: &[&str] = &[
    "UserPromptSubmit",
    "PreToolUse",
    "PostToolUse",
    "Stop",
    "Notification",
];

/// The four hook kinds installed by pre-Story-5.2 builds. Used by
/// [`install`] to detect a legacy installation and surface a one-line
/// upgrade hint (story 5.2 AC #4). Order is irrelevant — we only check
/// set-membership against the bowerbird entries already in settings.json.
const LEGACY_HOOK_KINDS: &[&str] = &["PreToolUse", "PostToolUse", "Stop", "Notification"];

/// Exponential backoff schedule between rename-race retries. The first
/// attempt has no preceding sleep; each entry here is the wait between one
/// attempt and the next. Four entries → one initial attempt + four retries
/// = five attempts total, matching the story spec ("5 attempts" — the
/// previously-shipped fifth backoff entry pushed the loop to six attempts
/// and produced an off-by-one in the reported `attempts` count).
const RETRY_BACKOFFS: &[Duration] = &[
    Duration::from_millis(25),
    Duration::from_millis(50),
    Duration::from_millis(100),
    Duration::from_millis(200),
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallOutcome {
    /// True when `settings.json` did not exist and was created by this call.
    pub created: bool,
    /// Hook kinds where a bowerbird entry was added. Empty if every kind
    /// already had a bowerbird entry (install is idempotent over the
    /// settings.json dimension).
    pub hook_kinds_added: Vec<&'static str>,
    /// True when, pre-merge, settings.json already had bowerbird entries for
    /// all four legacy hook kinds (PreToolUse, PostToolUse, Stop,
    /// Notification) but was missing the Story 5.2 addition
    /// (UserPromptSubmit). The CLI surfaces this as a one-line hint so an
    /// operator re-running install after a version bump sees that the new
    /// hook was subscribed (story 5.2 AC #4).
    pub legacy_upgrade_detected: bool,
}

/// Outcome of [`seed_tool_reactions`].
///
/// `Wrote` when the bundled `tool-reactions.toml` was atomically written into
/// `bowerbird_dir`'s `adapters/claude/` subdirectory; `AlreadyPresent` when a
/// file was already on disk and the seed step left it untouched. The CLI uses
/// this to print a per-outcome line; downstream callers can branch on it for
/// other UX decisions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SeedOutcome {
    /// The file did not exist; the bundled bytes were written atomically.
    Wrote,
    /// A file already existed at the target path; nothing was written.
    AlreadyPresent,
}

/// Bundled `adapters/claude/tool-reactions.toml` contents embedded into the
/// `bowerbird` CLI binary at compile time. Path is relative to this source
/// file — `crates/adapter-claude/src/install.rs` → workspace root via three
/// `..` segments.
const BUNDLED_TOOL_REACTIONS_TOML: &[u8] =
    include_bytes!("../../../adapters/claude/tool-reactions.toml");

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UninstallOutcome {
    /// True when `settings.json` existed prior to the call.
    pub existed: bool,
    /// Hook kinds where a bowerbird entry was removed.
    pub hook_kinds_removed: Vec<&'static str>,
}

/// Install the bowerbird hook entries into `settings_path`. Creates the file
/// if missing (AC #5). Idempotent: re-running on a file that already contains
/// the entries is a no-op for the JSON.
///
/// The hook command written is `<protocol::SHIM_BINARY_NAME> --hook-kind <kind>`
/// per the shim's CLI surface. The binary name is intentionally PATH-relative
/// so the user controls resolution (AC #1).
pub fn install(settings_path: &Path) -> Result<InstallOutcome, InstallError> {
    let mut state = ReadState::default();
    for attempt in 0..=RETRY_BACKOFFS.len() {
        let (existed, mut value) = read_or_init(settings_path)?;
        // The first attempt records baseline metadata. Subsequent attempts use
        // the prior snapshot to decide whether to back off (someone else wrote
        // between us reading and us renaming).
        if attempt == 0 {
            state.created = !existed;
        }
        let pre_merge_legacy = settings_has_only_legacy_bowerbird_hooks(&value);
        let hook_kinds_added = merge_install_into(&mut value);
        let legacy_upgrade_detected =
            pre_merge_legacy && hook_kinds_added.contains(&"UserPromptSubmit");
        let outcome = InstallOutcome {
            created: state.created,
            hook_kinds_added,
            legacy_upgrade_detected,
        };
        match atomic_write(settings_path, &value)? {
            WriteOutcome::Wrote => return Ok(outcome),
            WriteOutcome::ConcurrentChange => {
                if let Some(backoff) = RETRY_BACKOFFS.get(attempt) {
                    std::thread::sleep(*backoff);
                    continue;
                }
                return Err(InstallError::SettingsAtomicRenameRace {
                    path: settings_path.to_path_buf(),
                    attempts: (RETRY_BACKOFFS.len() + 1) as u32,
                });
            }
        }
    }
    unreachable!("retry loop must either return or exhaust");
}

/// Remove the bowerbird hook entries from `settings_path`. Idempotent:
/// re-running on a file that no longer contains the entries is a no-op.
/// Returns `existed = false` if the file did not exist (AC #4 only requires
/// us to leave a valid JSON file behind; not creating one when none existed
/// is the right thing).
pub fn uninstall(settings_path: &Path) -> Result<UninstallOutcome, InstallError> {
    for attempt in 0..=RETRY_BACKOFFS.len() {
        let (existed, mut value) = match read_existing(settings_path)? {
            Some(v) => (true, v),
            None => {
                return Ok(UninstallOutcome {
                    existed: false,
                    hook_kinds_removed: Vec::new(),
                });
            }
        };
        let hook_kinds_removed = strip_install_from(&mut value);
        let outcome = UninstallOutcome {
            existed,
            hook_kinds_removed,
        };
        match atomic_write(settings_path, &value)? {
            WriteOutcome::Wrote => return Ok(outcome),
            WriteOutcome::ConcurrentChange => {
                if let Some(backoff) = RETRY_BACKOFFS.get(attempt) {
                    std::thread::sleep(*backoff);
                    continue;
                }
                return Err(InstallError::SettingsAtomicRenameRace {
                    path: settings_path.to_path_buf(),
                    attempts: (RETRY_BACKOFFS.len() + 1) as u32,
                });
            }
        }
    }
    unreachable!("retry loop must either return or exhaust");
}

/// Seed `<bowerbird_dir>/adapters/claude/tool-reactions.toml` from the bundled
/// bytes baked into the binary at build time, leaving any pre-existing file
/// untouched.
///
/// Story 5.4 AC #1. The bundled bytes come from
/// `adapters/claude/tool-reactions.toml` at the workspace root via
/// `include_bytes!`, so `cargo install --git --tag` users and tarball users
/// land on the same content without depending on the install command's cwd.
///
/// Writes use the same tmp + fsync + rename idiom as [`atomic_write`] but
/// without the concurrent-writer detection baseline — this is a one-shot
/// "create only if missing," not a JSON merge.
pub fn seed_tool_reactions(bowerbird_dir: &Path) -> Result<SeedOutcome, InstallError> {
    let target = bowerbird_dir
        .join("adapters")
        .join("claude")
        .join("tool-reactions.toml");

    // Decide based on what is *actually* at the path. `symlink_metadata` does
    // NOT follow symlinks, so a dangling symlink reports as a symlink (not
    // `NotFound`) and a directory reports as a directory. Both are non-files we
    // must refuse rather than silently report as "already seeded": the daemon
    // opens this path expecting a TOML file, and a directory or dangling link
    // there means install would claim a working state the runtime can't use.
    match fs::symlink_metadata(&target) {
        Ok(meta) if meta.file_type().is_file() => return Ok(SeedOutcome::AlreadyPresent),
        Ok(meta) => {
            return Err(InstallError::SeedTargetNotFile {
                path: target,
                kind: describe_non_file(meta.file_type()),
            });
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => {
            return Err(InstallError::SeedWrite {
                path: target,
                source: e,
            });
        }
    }

    let parent = target
        .parent()
        .expect("target path is a join of bowerbird_dir + adapters/claude/tool-reactions.toml; parent always exists");
    // 0700 on Unix: this directory holds the user's adapter config; a default
    // umask-derived mode could leave it group/world-readable.
    create_dir_all_private(parent).map_err(|e| InstallError::SeedWrite {
        path: target.clone(),
        source: e,
    })?;

    let tmp_path = tmp_path_for(&target);
    {
        let mut file =
            seed_file_open_for_write(&tmp_path).map_err(|e| InstallError::SeedWrite {
                path: target.clone(),
                source: e,
            })?;
        file.write_all(BUNDLED_TOOL_REACTIONS_TOML)
            .map_err(|e| InstallError::SeedWrite {
                path: target.clone(),
                source: e,
            })?;
        file.sync_all().map_err(|e| InstallError::SeedWrite {
            path: target.clone(),
            source: e,
        })?;
    }

    // Publish via `link(2)`, NOT `rename(2)`. `rename` silently replaces an
    // existing target; `link` fails with `EEXIST` if anything already occupies
    // the path. That closes the check-then-write TOCTOU window: if a concurrent
    // install (or the user) created the file between our `symlink_metadata`
    // probe above and now, we lose the link race and leave their copy
    // untouched instead of clobbering it. The tmp file inherits the 0600 mode
    // from `create_new`; the published link points at the same inode.
    let publish = fs::hard_link(&tmp_path, &target);
    // Always drop the tmp link regardless of outcome — on success the target is
    // a second link to the same inode; on failure we don't want to leak it.
    let _ = fs::remove_file(&tmp_path);
    match publish {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            // The target appeared between our probe and the link. Re-stat and
            // honor the same file-vs-non-file contract as the up-front check.
            return match fs::symlink_metadata(&target) {
                Ok(meta) if meta.file_type().is_file() => Ok(SeedOutcome::AlreadyPresent),
                Ok(meta) => Err(InstallError::SeedTargetNotFile {
                    path: target,
                    kind: describe_non_file(meta.file_type()),
                }),
                Err(e) => Err(InstallError::SeedRename {
                    path: target,
                    source: e,
                }),
            };
        }
        Err(e) => {
            return Err(InstallError::SeedRename {
                path: target,
                source: e,
            });
        }
    }

    // Best-effort parent fsync so the new link is durable; matches `atomic_write`.
    if let Ok(dir) = fs::File::open(parent) {
        let _ = dir.sync_all();
    }
    Ok(SeedOutcome::Wrote)
}

/// Short human-readable descriptor for a non-regular-file [`fs::FileType`],
/// used in the [`InstallError::SeedTargetNotFile`] message.
fn describe_non_file(file_type: fs::FileType) -> &'static str {
    if file_type.is_dir() {
        "directory"
    } else if file_type.is_symlink() {
        "symlink"
    } else {
        "non-regular file"
    }
}

/// `create_dir_all` that stamps every directory it creates with mode 0700 on
/// Unix. 0700 is below any reasonable umask, so the bits survive
/// `mode & !umask`. Existing ancestors are left as-is (same as
/// `fs::create_dir_all`).
#[cfg(unix)]
fn create_dir_all_private(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(path)
}

#[cfg(not(unix))]
fn create_dir_all_private(path: &Path) -> std::io::Result<()> {
    fs::create_dir_all(path)
}

#[cfg(unix)]
fn seed_file_open_for_write(path: &Path) -> std::io::Result<fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
}

#[cfg(not(unix))]
fn seed_file_open_for_write(path: &Path) -> std::io::Result<fs::File> {
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
}

#[derive(Default)]
struct ReadState {
    created: bool,
}

enum WriteOutcome {
    Wrote,
    ConcurrentChange,
}

/// Read `path` into a `Value`. Returns `(false, empty object)` on ENOENT —
/// install's AC #5 contract.
fn read_or_init(path: &Path) -> Result<(bool, Value), InstallError> {
    match fs::read(path) {
        Ok(bytes) => {
            let value: Value =
                serde_json::from_slice(&bytes).map_err(|e| InstallError::SettingsParse {
                    path: path.to_path_buf(),
                    source: e,
                })?;
            if !value.is_object() {
                return Err(InstallError::SettingsNotObject {
                    path: path.to_path_buf(),
                });
            }
            Ok((true, value))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            Ok((false, Value::Object(Map::new())))
        }
        Err(e) => Err(InstallError::SettingsRead {
            path: path.to_path_buf(),
            source: e,
        }),
    }
}

/// Read `path` into a `Value` or return `None` if it does not exist. Distinct
/// from `read_or_init` so uninstall does not invent an empty object only to
/// rewrite it back as `{}`.
fn read_existing(path: &Path) -> Result<Option<Value>, InstallError> {
    match fs::read(path) {
        Ok(bytes) => {
            let value: Value =
                serde_json::from_slice(&bytes).map_err(|e| InstallError::SettingsParse {
                    path: path.to_path_buf(),
                    source: e,
                })?;
            if !value.is_object() {
                return Err(InstallError::SettingsNotObject {
                    path: path.to_path_buf(),
                });
            }
            Ok(Some(value))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(InstallError::SettingsRead {
            path: path.to_path_buf(),
            source: e,
        }),
    }
}

/// Insert the bowerbird hook entry into `value["hooks"][<kind>]` for every
/// known hook kind. Returns the kinds that received a new entry; kinds that
/// already had one are skipped (idempotency).
fn merge_install_into(value: &mut Value) -> Vec<&'static str> {
    let root = value
        .as_object_mut()
        .expect("validated by read_or_init / read_existing");
    let hooks = root
        .entry("hooks")
        .or_insert_with(|| Value::Object(Map::new()));
    let hooks = match hooks.as_object_mut() {
        Some(o) => o,
        None => {
            // The user has a non-object `hooks` value. Replace it — but only
            // because there is no reasonable merge target. This is a tiny
            // surface and a true edge case; an honest replacement is less
            // surprising than silently ignoring the kind.
            *hooks = Value::Object(Map::new());
            hooks.as_object_mut().expect("just set to object")
        }
    };

    let mut added = Vec::new();
    for kind in HOOK_KINDS {
        let array = hooks
            .entry(*kind)
            .or_insert_with(|| Value::Array(Vec::new()));
        let array = match array.as_array_mut() {
            Some(a) => a,
            None => {
                *array = Value::Array(Vec::new());
                array.as_array_mut().expect("just set to array")
            }
        };
        if array_contains_bowerbird(array) {
            continue;
        }
        array.push(bowerbird_hook_group(kind));
        added.push(*kind);
    }
    added
}

/// Remove the bowerbird hook entry from each hook kind. Returns the kinds
/// where a bowerbird entry was actually present. User-authored entries (any
/// entry whose `.hooks` array contains a non-bowerbird `command`) are
/// preserved.
fn strip_install_from(value: &mut Value) -> Vec<&'static str> {
    let Some(root) = value.as_object_mut() else {
        return Vec::new();
    };
    let Some(hooks) = root.get_mut("hooks").and_then(|v| v.as_object_mut()) else {
        return Vec::new();
    };

    let mut removed = Vec::new();
    for kind in HOOK_KINDS {
        let Some(entry) = hooks.get_mut(*kind) else {
            continue;
        };
        let Some(array) = entry.as_array_mut() else {
            continue;
        };
        let before = array.len();
        array.retain(|group| !group_is_bowerbird_only(group));
        // Groups that mixed a bowerbird hook with user hooks: strip just
        // the bowerbird command from each surviving group.
        for group in array.iter_mut() {
            if let Some(group_obj) = group.as_object_mut() {
                if let Some(inner) = group_obj.get_mut("hooks").and_then(|v| v.as_array_mut()) {
                    let inner_before = inner.len();
                    inner.retain(|hook| !is_bowerbird_command_hook(hook));
                    if inner.len() != inner_before {
                        // Mixed group: removed at least one bowerbird hook
                        // from a group that also contains user hooks.
                        if removed.last().is_none_or(|k| *k != *kind) {
                            removed.push(*kind);
                        }
                    }
                }
            }
        }
        if array.len() != before && removed.last().is_none_or(|k| *k != *kind) {
            removed.push(*kind);
        }
    }
    removed
}

fn bowerbird_hook_group(kind: &str) -> Value {
    // Shape mirrors Claude Code's canonical hook entry: a matcher-less group
    // (applies to all matchers) with a single command hook. Picked as the
    // minimal valid form per the architecture's "smallest viable shape"
    // guideline. The matcher field can be added later without breaking the
    // uninstall match (which keys off the command string, not the matcher).
    let mut group = Map::new();
    let mut hook = Map::new();
    hook.insert("type".to_string(), Value::String("command".to_string()));
    hook.insert(
        "command".to_string(),
        Value::String(format!(
            "{} --hook-kind {}",
            protocol::SHIM_BINARY_NAME,
            kind
        )),
    );
    group.insert("hooks".to_string(), Value::Array(vec![Value::Object(hook)]));
    Value::Object(group)
}

fn array_contains_bowerbird(array: &[Value]) -> bool {
    array.iter().any(group_contains_bowerbird)
}

/// True when the pre-merge settings.json has a bowerbird entry under
/// every pre-Story-5.2 hook kind but no bowerbird entry under any newer
/// kind (currently just `UserPromptSubmit`). Drives the install hint that
/// tells operators their re-run upgraded a legacy install (story 5.2 AC #4).
fn settings_has_only_legacy_bowerbird_hooks(value: &Value) -> bool {
    let Some(hooks) = value.get("hooks").and_then(|v| v.as_object()) else {
        return false;
    };
    let legacy_present = LEGACY_HOOK_KINDS.iter().all(|kind| {
        hooks
            .get(*kind)
            .and_then(|v| v.as_array())
            .is_some_and(|a| array_contains_bowerbird(a))
    });
    if !legacy_present {
        return false;
    }
    let post_legacy_present = HOOK_KINDS
        .iter()
        .filter(|k| !LEGACY_HOOK_KINDS.contains(k))
        .any(|kind| {
            hooks
                .get(*kind)
                .and_then(|v| v.as_array())
                .is_some_and(|a| array_contains_bowerbird(a))
        });
    !post_legacy_present
}

fn group_contains_bowerbird(group: &Value) -> bool {
    let Some(group_obj) = group.as_object() else {
        return false;
    };
    let Some(hooks) = group_obj.get("hooks").and_then(|v| v.as_array()) else {
        return false;
    };
    hooks.iter().any(is_bowerbird_command_hook)
}

/// A group is "bowerbird only" when every hook inside it is a bowerbird
/// command hook. Such a group is safe to drop entirely; mixed groups need
/// per-hook stripping so user hooks survive.
fn group_is_bowerbird_only(group: &Value) -> bool {
    let Some(group_obj) = group.as_object() else {
        return false;
    };
    let Some(hooks) = group_obj.get("hooks").and_then(|v| v.as_array()) else {
        return false;
    };
    if hooks.is_empty() {
        return false;
    }
    hooks.iter().all(is_bowerbird_command_hook)
}

fn is_bowerbird_command_hook(hook: &Value) -> bool {
    let Some(obj) = hook.as_object() else {
        return false;
    };
    let Some(cmd) = obj.get("command").and_then(|v| v.as_str()) else {
        return false;
    };
    is_bowerbird_command(cmd)
}

fn is_bowerbird_command(cmd: &str) -> bool {
    // Match by binary name token, not substring: a user-authored command
    // containing the word "bowerbird-shim" inside a longer path (or as an
    // argument) must not be misclassified. The shim is always invoked as the
    // first token. Accept either bare name (PATH-relative, our default) or a
    // trailing path component match for absolute paths a user typed by hand.
    let first_token = cmd.split_whitespace().next().unwrap_or("");
    if first_token == protocol::SHIM_BINARY_NAME {
        return true;
    }
    Path::new(first_token).file_name().and_then(|f| f.to_str()) == Some(protocol::SHIM_BINARY_NAME)
}

/// Serialize `value` and atomically replace `path` with it. Returns
/// `ConcurrentChange` when an external writer modified `path` between our
/// pre-write snapshot and the rename — the caller decides whether to retry.
fn atomic_write(path: &Path, value: &Value) -> Result<WriteOutcome, InstallError> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent).map_err(|e| InstallError::SettingsWriteTmp {
        path: path.to_path_buf(),
        source: e,
    })?;

    // Snapshot the target's identity before we write the tmp file. After
    // rename succeeds we cannot distinguish "we won the race" from "we
    // overwrote a concurrent writer's edit" without this baseline. The
    // identity tuple (inode, mtime, size) also collapses cleanly to `None`
    // when the file did not exist at read time — a later non-`None` baseline
    // trips the comparison just like a same-path inode swap would.
    let pre_rename = stat_identity(path);

    let tmp_path = tmp_path_for(path);
    write_pretty_json(&tmp_path, value)?;
    fsync_file(&tmp_path)?;

    let post_baseline = stat_identity(path);
    if pre_rename != post_baseline {
        // Someone else changed the target between read and now. Drop the
        // tmp file so a stale snapshot does not poison the next attempt.
        let _ = fs::remove_file(&tmp_path);
        return Ok(WriteOutcome::ConcurrentChange);
    }

    fs::rename(&tmp_path, path).map_err(|e| InstallError::SettingsRename {
        path: path.to_path_buf(),
        source: e,
    })?;
    // Best-effort fsync the parent directory so the rename is durable. Failure
    // here does not invalidate the rename itself (POSIX rename is atomic; the
    // fsync just hardens crash safety), so we swallow errors.
    if let Ok(dir) = fs::File::open(parent) {
        let _ = dir.sync_all();
    }
    Ok(WriteOutcome::Wrote)
}

fn write_pretty_json(path: &Path, value: &Value) -> Result<(), InstallError> {
    let serialized =
        serde_json::to_vec_pretty(value).map_err(|e| InstallError::SettingsWriteTmp {
            path: path.to_path_buf(),
            source: std::io::Error::other(e),
        })?;

    let mut file = file_open_for_write(path)?;
    file.write_all(&serialized)
        .map_err(|e| InstallError::SettingsWriteTmp {
            path: path.to_path_buf(),
            source: e,
        })?;
    // Append trailing newline — minimal-diff convention for human-edited files.
    file.write_all(b"\n")
        .map_err(|e| InstallError::SettingsWriteTmp {
            path: path.to_path_buf(),
            source: e,
        })?;
    Ok(())
}

#[cfg(unix)]
fn file_open_for_write(path: &Path) -> Result<fs::File, InstallError> {
    use std::os::unix::fs::OpenOptionsExt;
    fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)
        .map_err(|e| InstallError::SettingsWriteTmp {
            path: path.to_path_buf(),
            source: e,
        })
}

#[cfg(not(unix))]
fn file_open_for_write(path: &Path) -> Result<fs::File, InstallError> {
    fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(path)
        .map_err(|e| InstallError::SettingsWriteTmp {
            path: path.to_path_buf(),
            source: e,
        })
}

fn fsync_file(path: &Path) -> Result<(), InstallError> {
    let file = fs::OpenOptions::new().read(true).open(path).map_err(|e| {
        InstallError::SettingsWriteTmp {
            path: path.to_path_buf(),
            source: e,
        }
    })?;
    file.sync_all().map_err(|e| InstallError::SettingsWriteTmp {
        path: path.to_path_buf(),
        source: e,
    })
}

fn tmp_path_for(path: &Path) -> PathBuf {
    let mut name = path
        .file_name()
        .map(|n| n.to_os_string())
        .unwrap_or_else(|| std::ffi::OsString::from("settings.json"));
    let pid = std::process::id();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    // Process-local monotonic counter avoids tmp-path collisions when two
    // threads in the same process call `tmp_path_for` within the same
    // nanosecond (`SystemTime::now()` resolution is platform-dependent and
    // not guaranteed to advance per call). Without this counter, concurrent
    // installs from the same process can race on the same tmp file: thread A
    // renames it over the target while thread B is still mid-write, leaving
    // B's rename to ENOENT.
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    name.push(format!(".bowerbird-install.{pid}-{nanos}-{seq}.tmp"));
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    parent.join(name)
}

/// File identity for concurrent-write detection. `None` when the file does
/// not exist. Combines (inode, mtime, size) so a same-second overwrite with
/// the same size still trips the comparison via inode change.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
struct Identity {
    #[cfg(unix)]
    inode: u64,
    mtime: Option<std::time::SystemTime>,
    size: u64,
}

fn stat_identity(path: &Path) -> Option<Identity> {
    let meta = fs::metadata(path).ok()?;
    #[cfg(unix)]
    let inode = {
        use std::os::unix::fs::MetadataExt;
        meta.ino()
    };
    Some(Identity {
        #[cfg(unix)]
        inode,
        mtime: meta.modified().ok(),
        size: meta.len(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tempfile::TempDir;

    fn fresh_settings(dir: &TempDir) -> PathBuf {
        dir.path().join("settings.json")
    }

    #[test]
    fn install_creates_settings_when_missing_writes_valid_json() {
        let dir = TempDir::new().unwrap();
        let path = fresh_settings(&dir);
        assert!(!path.exists());

        let outcome = install(&path).expect("install");
        assert!(outcome.created);
        assert_eq!(outcome.hook_kinds_added, HOOK_KINDS);
        let parsed: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        for kind in HOOK_KINDS {
            let array = parsed
                .pointer(&format!("/hooks/{kind}"))
                .and_then(|v| v.as_array())
                .unwrap_or_else(|| panic!("expected /hooks/{kind} array"));
            assert_eq!(array.len(), 1, "kind {kind} should have one group");
            assert!(group_is_bowerbird_only(&array[0]));
        }
    }

    #[test]
    fn install_is_idempotent_when_already_installed() {
        let dir = TempDir::new().unwrap();
        let path = fresh_settings(&dir);
        install(&path).expect("first install");
        let before = fs::read_to_string(&path).unwrap();

        let outcome = install(&path).expect("second install");
        assert!(!outcome.created);
        assert!(outcome.hook_kinds_added.is_empty());
        let after = fs::read_to_string(&path).unwrap();
        assert_eq!(before, after, "re-install should be byte-identical");
    }

    #[test]
    fn install_preserves_unrelated_fields_and_user_hooks() {
        let dir = TempDir::new().unwrap();
        let path = fresh_settings(&dir);
        let initial = json!({
            "theme": "dark",
            "hooks": {
                "PreToolUse": [
                    {
                        "hooks": [
                            {"type": "command", "command": "/my/own/hook --flag"}
                        ]
                    }
                ]
            }
        });
        fs::write(&path, serde_json::to_vec_pretty(&initial).unwrap()).unwrap();

        let outcome = install(&path).expect("install");
        assert!(!outcome.created);
        assert!(outcome.hook_kinds_added.contains(&"PreToolUse"));
        let parsed: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(parsed.get("theme"), Some(&json!("dark")));
        let pre = parsed
            .pointer("/hooks/PreToolUse")
            .unwrap()
            .as_array()
            .unwrap();
        // User's group plus our group = 2.
        assert_eq!(pre.len(), 2);
        let user_present = pre.iter().any(|g| {
            g.pointer("/hooks/0/command").and_then(|v| v.as_str()) == Some("/my/own/hook --flag")
        });
        assert!(user_present, "user hook must survive install");
    }

    #[test]
    fn uninstall_returns_existed_false_when_file_missing() {
        let dir = TempDir::new().unwrap();
        let path = fresh_settings(&dir);
        let outcome = uninstall(&path).expect("uninstall");
        assert!(!outcome.existed);
        assert!(outcome.hook_kinds_removed.is_empty());
        assert!(!path.exists(), "uninstall must not create the file");
    }

    #[test]
    fn uninstall_removes_only_bowerbird_entries() {
        let dir = TempDir::new().unwrap();
        let path = fresh_settings(&dir);
        let mixed = json!({
            "hooks": {
                "PreToolUse": [
                    {"hooks": [{"type": "command", "command": "/my/own/hook"}]},
                    {"hooks": [{"type": "command", "command": format!("{} --hook-kind PreToolUse", protocol::SHIM_BINARY_NAME)}]}
                ]
            }
        });
        fs::write(&path, serde_json::to_vec_pretty(&mixed).unwrap()).unwrap();

        let outcome = uninstall(&path).expect("uninstall");
        assert!(outcome.existed);
        assert!(outcome.hook_kinds_removed.contains(&"PreToolUse"));
        let parsed: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        let pre = parsed
            .pointer("/hooks/PreToolUse")
            .unwrap()
            .as_array()
            .unwrap();
        assert_eq!(pre.len(), 1, "only the user hook must remain");
        assert_eq!(
            pre[0].pointer("/hooks/0/command").and_then(|v| v.as_str()),
            Some("/my/own/hook")
        );
    }

    #[test]
    fn uninstall_strips_bowerbird_from_mixed_group_preserving_user_hooks() {
        let dir = TempDir::new().unwrap();
        let path = fresh_settings(&dir);
        let mixed_group = json!({
            "hooks": {
                "PreToolUse": [
                    {"hooks": [
                        {"type": "command", "command": "/my/own/hook"},
                        {"type": "command", "command": format!("{} --hook-kind PreToolUse", protocol::SHIM_BINARY_NAME)}
                    ]}
                ]
            }
        });
        fs::write(&path, serde_json::to_vec_pretty(&mixed_group).unwrap()).unwrap();

        uninstall(&path).expect("uninstall");
        let parsed: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        let inner = parsed
            .pointer("/hooks/PreToolUse/0/hooks")
            .unwrap()
            .as_array()
            .unwrap();
        assert_eq!(inner.len(), 1);
        assert_eq!(
            inner[0].get("command").and_then(|v| v.as_str()),
            Some("/my/own/hook")
        );
    }

    #[test]
    fn install_rejects_non_object_top_level() {
        let dir = TempDir::new().unwrap();
        let path = fresh_settings(&dir);
        fs::write(&path, b"[]").unwrap();
        let err = install(&path).expect_err("install should refuse non-object");
        assert!(matches!(err, InstallError::SettingsNotObject { .. }));
    }

    #[test]
    fn install_rejects_invalid_json_with_typed_error() {
        let dir = TempDir::new().unwrap();
        let path = fresh_settings(&dir);
        fs::write(&path, b"{not-json").unwrap();
        let err = install(&path).expect_err("install should refuse malformed JSON");
        assert!(matches!(err, InstallError::SettingsParse { .. }));
    }

    #[test]
    fn install_does_not_strip_user_command_containing_shim_name_substring() {
        let dir = TempDir::new().unwrap();
        let path = fresh_settings(&dir);
        let initial = json!({
            "hooks": {
                "PostToolUse": [
                    {"hooks": [{"type": "command", "command": "echo bowerbird-shim"}]}
                ]
            }
        });
        fs::write(&path, serde_json::to_vec_pretty(&initial).unwrap()).unwrap();
        install(&path).expect("install");
        // Uninstall must not delete the user's `echo bowerbird-shim` hook
        // because its first token is `echo`, not `bowerbird-shim`.
        uninstall(&path).expect("uninstall");
        let parsed: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        let post = parsed
            .pointer("/hooks/PostToolUse")
            .unwrap()
            .as_array()
            .unwrap();
        assert_eq!(post.len(), 1);
        assert_eq!(
            post[0].pointer("/hooks/0/command").and_then(|v| v.as_str()),
            Some("echo bowerbird-shim")
        );
    }

    // Story 5.2 review #3 — three cases for `legacy_upgrade_detected`:
    // legacy-four-only → true, fresh install → false, already-upgraded → false.
    fn legacy_four_hook_settings() -> Value {
        let mut hooks = Map::new();
        for kind in ["PreToolUse", "PostToolUse", "Stop", "Notification"] {
            hooks.insert(
                kind.to_string(),
                Value::Array(vec![bowerbird_hook_group(kind)]),
            );
        }
        let mut root = Map::new();
        root.insert("hooks".to_string(), Value::Object(hooks));
        Value::Object(root)
    }

    #[test]
    fn legacy_upgrade_detected_true_when_only_legacy_four_present() {
        let dir = TempDir::new().unwrap();
        let path = fresh_settings(&dir);
        fs::write(
            &path,
            serde_json::to_vec_pretty(&legacy_four_hook_settings()).unwrap(),
        )
        .unwrap();

        let outcome = install(&path).expect("install");
        assert!(
            outcome.legacy_upgrade_detected,
            "pre-5.2 four-hook install must surface the legacy-upgrade hint"
        );
        assert!(
            outcome.hook_kinds_added.contains(&"UserPromptSubmit"),
            "the fifth hook must be added in the same call"
        );
    }

    #[test]
    fn legacy_upgrade_detected_false_on_fresh_install() {
        let dir = TempDir::new().unwrap();
        let path = fresh_settings(&dir);
        let outcome = install(&path).expect("install");
        assert!(outcome.created);
        assert!(
            !outcome.legacy_upgrade_detected,
            "fresh install is not a legacy upgrade"
        );
    }

    #[test]
    fn legacy_upgrade_detected_false_on_already_upgraded_five_hooks() {
        let dir = TempDir::new().unwrap();
        let path = fresh_settings(&dir);
        install(&path).expect("first install");
        let outcome = install(&path).expect("second install");
        assert!(
            outcome.hook_kinds_added.is_empty(),
            "re-running on full five-hook install is a no-op"
        );
        assert!(
            !outcome.legacy_upgrade_detected,
            "an install that already had all five hooks is not a legacy upgrade"
        );
    }

    // Story 5.4 Task 1 — `seed_tool_reactions` writes the bundled file when
    // missing, leaves an existing file untouched, and creates parent dirs.

    #[test]
    fn seed_tool_reactions_writes_when_missing() {
        let dir = TempDir::new().unwrap();
        let outcome = seed_tool_reactions(dir.path()).expect("seed must succeed");
        assert_eq!(outcome, SeedOutcome::Wrote);
        let target = dir
            .path()
            .join("adapters")
            .join("claude")
            .join("tool-reactions.toml");
        let bytes = fs::read(&target).expect("seeded file must be readable");
        assert_eq!(
            bytes, BUNDLED_TOOL_REACTIONS_TOML,
            "seeded bytes must match the bundled file byte-for-byte"
        );
    }

    #[test]
    fn seed_tool_reactions_skips_when_present() {
        let dir = TempDir::new().unwrap();
        let target = dir
            .path()
            .join("adapters")
            .join("claude")
            .join("tool-reactions.toml");
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        let preexisting = b"# user-edited tool reactions\n[tool_reactions]\nFooBar = \"Pause\"\n";
        fs::write(&target, preexisting).unwrap();

        let outcome = seed_tool_reactions(dir.path()).expect("seed must succeed");
        assert_eq!(outcome, SeedOutcome::AlreadyPresent);
        let after = fs::read(&target).expect("user file must remain");
        assert_eq!(
            after, preexisting,
            "pre-existing user file must be left untouched"
        );
    }

    #[test]
    fn seed_tool_reactions_creates_parent_directories() {
        let dir = TempDir::new().unwrap();
        let adapters = dir.path().join("adapters");
        assert!(
            !adapters.exists(),
            "adapters/ should not exist before seeding"
        );
        let outcome = seed_tool_reactions(dir.path()).expect("seed must succeed");
        assert_eq!(outcome, SeedOutcome::Wrote);
        assert!(
            dir.path().join("adapters").join("claude").is_dir(),
            "seed must create the adapters/claude/ directory chain"
        );
    }

    // Story 5.4 review — the created `adapters/` chain must be 0700, not
    // umask-default. Otherwise the user's adapter config dir could be
    // group/world-readable.
    #[cfg(unix)]
    #[test]
    fn seed_tool_reactions_creates_parent_directories_with_private_mode() {
        use std::os::unix::fs::PermissionsExt;
        let dir = TempDir::new().unwrap();
        seed_tool_reactions(dir.path()).expect("seed must succeed");
        for sub in ["adapters", "adapters/claude"] {
            let created = dir.path().join(sub);
            let mode = fs::metadata(&created).unwrap().permissions().mode() & 0o777;
            assert_eq!(
                mode, 0o700,
                "{sub} must be created with mode 0700, got {mode:o}"
            );
        }
    }

    // Story 5.4 review — a non-file object at the target path (here a
    // directory) must fail loudly, not be reported as `AlreadyPresent`. The
    // daemon expects to read a TOML file from that path.
    #[test]
    fn seed_tool_reactions_rejects_directory_at_target() {
        let dir = TempDir::new().unwrap();
        let target = dir
            .path()
            .join("adapters")
            .join("claude")
            .join("tool-reactions.toml");
        fs::create_dir_all(&target).unwrap(); // a DIRECTORY where the file belongs
        let err = seed_tool_reactions(dir.path()).expect_err("must reject a non-file target");
        assert!(
            matches!(err, InstallError::SeedTargetNotFile { kind, .. } if kind == "directory"),
            "expected SeedTargetNotFile(directory), got {err:?}"
        );
    }

    // Story 5.4 review — a (dangling) symlink at the target must be refused and
    // left in place, never replaced. `Path::exists()` would have followed it
    // and returned false for a dangling link, opening the door to clobbering a
    // user's symlink; `symlink_metadata` does not follow it.
    #[cfg(unix)]
    #[test]
    fn seed_tool_reactions_rejects_symlink_target_and_leaves_it_in_place() {
        use std::os::unix::fs::symlink;
        let dir = TempDir::new().unwrap();
        let target = dir
            .path()
            .join("adapters")
            .join("claude")
            .join("tool-reactions.toml");
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        symlink(dir.path().join("does-not-exist"), &target).unwrap();

        let err = seed_tool_reactions(dir.path()).expect_err("must reject a symlink target");
        assert!(
            matches!(err, InstallError::SeedTargetNotFile { kind, .. } if kind == "symlink"),
            "expected SeedTargetNotFile(symlink), got {err:?}"
        );
        assert!(
            fs::symlink_metadata(&target)
                .unwrap()
                .file_type()
                .is_symlink(),
            "the user's symlink must be left untouched"
        );
    }

    #[test]
    fn install_matches_absolute_path_with_shim_basename() {
        // A user may have manually written an absolute path to the shim.
        // Uninstall must recognize and strip that too.
        let dir = TempDir::new().unwrap();
        let path = fresh_settings(&dir);
        let initial = json!({
            "hooks": {
                "PreToolUse": [
                    {"hooks": [{"type": "command", "command": format!("/usr/local/bin/{} --hook-kind PreToolUse", protocol::SHIM_BINARY_NAME)}]}
                ]
            }
        });
        fs::write(&path, serde_json::to_vec_pretty(&initial).unwrap()).unwrap();
        let outcome = uninstall(&path).expect("uninstall");
        assert!(outcome.hook_kinds_removed.contains(&"PreToolUse"));
    }
}
