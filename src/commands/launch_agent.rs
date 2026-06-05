//! launchd LaunchAgent supervision for the bowerbird daemon (macOS, Story 5.9 /
//! ADR 0007).
//!
//! On macOS, `bowerbird install` registers the daemon as a launchd LaunchAgent
//! so a reboot/login starts it (`RunAtLoad`) and a crash restarts it
//! (`KeepAlive = { SuccessfulExit = false }`), and `bowerbird uninstall` removes
//! that registration symmetrically. The shim is NOT touched — choosing launchd
//! over a shim lazy-spawn is precisely what keeps the shim a pure thin
//! passthrough (project-context.md §"Shim hot-path discipline": no subprocess on
//! the hot path).
//!
//! The CLI stays light: `launchctl` is invoked via `std::process::Command` and
//! the plist is a hand-rendered XML string — no new deps, no `tokio`/`axum`.
//!
//! Everything that talks to launchd is gated behind `#[cfg(target_os =
//! "macos")]`. The plist *renderer* and the path/dir resolvers are
//! cross-platform so their unit tests run on Linux CI too.

use std::path::{Path, PathBuf};

/// Reverse-DNS LaunchAgent label, reused by install, uninstall, bootstrap, and
/// bootout. Matches the [bowerbird-deck](https://github.com/technicalpickles/bowerbird-deck)
/// owner namespace (ADR 0007).
pub const LAUNCH_AGENT_LABEL: &str = "com.technicalpickles.bowerbird.daemon";

/// Resolve the directory that holds per-user LaunchAgent plists.
///
/// Order: `BOWERBIRD_LAUNCH_AGENTS_DIR` env override (test isolation, mirrors
/// `BOWERBIRD_CLAUDE_SETTINGS`) → `$HOME/Library/LaunchAgents`.
pub fn launch_agents_dir() -> anyhow::Result<PathBuf> {
    if let Some(p) = std::env::var_os("BOWERBIRD_LAUNCH_AGENTS_DIR") {
        if !p.is_empty() {
            return Ok(PathBuf::from(p));
        }
    }
    let home = super::home_dir()?;
    Ok(home.join("Library").join("LaunchAgents"))
}

/// Absolute path of the LaunchAgent plist (`<dir>/<label>.plist`).
pub fn plist_path() -> anyhow::Result<PathBuf> {
    Ok(launch_agents_dir()?.join(format!("{LAUNCH_AGENT_LABEL}.plist")))
}

/// Resolve an **absolute** path to the `bowerbird-daemon` binary for the plist.
///
/// `resolve_daemon_bin()` returns the PATH-relative string `"bowerbird-daemon"`,
/// which works for the `setsid` spawn (it inherits the shell's PATH) but FAILS
/// under launchd, whose minimal PATH typically lacks `/usr/local/bin`. The plist
/// must embed an absolute path.
///
/// Order (AC 3):
/// 1. `BOWERBIRD_DAEMON_BIN` if set and absolute (tests / non-standard installs;
///    trusted verbatim, like `resolve_daemon_bin()` does today).
/// 2. A sibling of `std::env::current_exe()` — the CLI and daemon ship together.
/// 3. A `PATH` search canonicalized to absolute.
/// 4. Error (no plist written) if none resolves — better than a plist launchd
///    silently fails to exec.
pub fn resolve_daemon_bin_absolute() -> anyhow::Result<PathBuf> {
    const BIN: &str = "bowerbird-daemon";

    if let Some(p) = std::env::var_os("BOWERBIRD_DAEMON_BIN") {
        if !p.is_empty() {
            let p = PathBuf::from(p);
            if p.is_absolute() {
                return Ok(p);
            }
            anyhow::bail!(
                "BOWERBIRD_DAEMON_BIN is set to a non-absolute path ({}); launchd needs an absolute daemon path",
                p.display()
            );
        }
    }

    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let sibling = dir.join(BIN);
            if is_executable_file(&sibling) {
                return Ok(sibling);
            }
        }
    }

    if let Some(found) = search_path_absolute(BIN) {
        return Ok(found);
    }

    anyhow::bail!(
        "could not resolve an absolute path to `{BIN}` for the launchd plist \
         (set BOWERBIRD_DAEMON_BIN to an absolute path, or ensure `{BIN}` ships \
         alongside the `bowerbird` CLI or is on PATH)"
    )
}

/// Search `$PATH` for `bin`, returning the first hit canonicalized to an
/// absolute path. `None` if `PATH` is unset or no entry contains an executable
/// file by that name.
fn search_path_absolute(bin: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        if dir.as_os_str().is_empty() {
            continue;
        }
        let candidate = dir.join(bin);
        if is_executable_file(&candidate) {
            // Canonicalize so a relative PATH entry (e.g. `./bin`) becomes
            // absolute; fall back to the join if canonicalize fails but the
            // join is already absolute.
            if let Ok(abs) = candidate.canonicalize() {
                return Some(abs);
            }
            if candidate.is_absolute() {
                return Some(candidate);
            }
        }
    }
    None
}

/// Is `path` a regular file with at least one execute bit set? On non-unix this
/// degrades to "is a regular file". Used to keep `resolve_daemon_bin_absolute()`
/// from selecting a non-executable file and to gate the bootstrap path (F4).
pub fn is_executable_file(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        match std::fs::metadata(path) {
            Ok(m) => m.is_file() && (m.permissions().mode() & 0o111 != 0),
            Err(_) => false,
        }
    }
    #[cfg(not(unix))]
    {
        path.is_file()
    }
}

/// Refuse to hand launchd a daemon path it cannot exec (Story 5.9 review F4).
///
/// `resolve_daemon_bin_absolute()` trusts an absolute `BOWERBIRD_DAEMON_BIN`
/// verbatim (so `--no-start` can pre-register before the binary exists — a
/// documented exception, see `install::supervise_or_start`). The bootstrap path,
/// which actually loads the agent, calls this first so a missing/non-executable
/// path fails install with a clear message instead of registering a LaunchAgent
/// that silently fails to start (and then crash-loops under `KeepAlive`).
pub fn ensure_daemon_launchable(daemon_path: &Path) -> anyhow::Result<()> {
    if is_executable_file(daemon_path) {
        return Ok(());
    }
    anyhow::bail!(
        "daemon binary at {} is not an executable file; refusing to bootstrap a LaunchAgent \
         launchd cannot exec (point BOWERBIRD_DAEMON_BIN at an executable bowerbird-daemon, \
         ensure it ships alongside the `bowerbird` CLI or is on PATH, or pass --no-start to \
         pre-register the plist before the binary is in place)",
        daemon_path.display()
    )
}

/// Render the LaunchAgent plist XML. Pure (no syscalls) so it is unit-testable
/// cross-platform.
///
/// `data_dir` is the bowerbird data directory; the daemon's stdout/stderr are
/// redirected to `daemon.out.log` / `daemon.err.log` under it.
///
/// `env` are `(key, value)` pairs emitted as an `EnvironmentVariables` dict.
/// launchd jobs start from a minimal environment and do NOT inherit the shell
/// env present at `bowerbird install` time, so any runtime override the daemon
/// reads (`BOWERBIRD_DATA_DIR`, `BOWERBIRD_INGEST_SOCK`) must be embedded here
/// or the launchd-started daemon resolves against defaults while the CLI
/// computed log paths against the override (Story 5.9 review F1). `BOWERBIRD_TOKEN`
/// is deliberately NOT passed in by the caller — the plist is mode 0644 and a
/// bearer token in a world-readable file is a secret leak; the daemon resolves
/// the token from the keychain/config under launchd instead (ADR 0007).
pub fn render_launch_agent_plist(
    label: &str,
    daemon_path: &Path,
    data_dir: &Path,
    env: &[(&str, &str)],
) -> String {
    let out_log = data_dir.join("daemon.out.log");
    let err_log = data_dir.join("daemon.err.log");

    let label = xml_escape(label);
    let daemon = xml_escape(&daemon_path.to_string_lossy());
    let out = xml_escape(&out_log.to_string_lossy());
    let err = xml_escape(&err_log.to_string_lossy());

    // Optional EnvironmentVariables dict (empty -> omitted entirely).
    let env_block = if env.is_empty() {
        String::new()
    } else {
        let mut inner = String::new();
        for (k, v) in env {
            inner.push_str(&format!(
                "        <key>{}</key>\n        <string>{}</string>\n",
                xml_escape(k),
                xml_escape(v)
            ));
        }
        format!("    <key>EnvironmentVariables</key>\n    <dict>\n{inner}    </dict>\n")
    };

    // `KeepAlive = { SuccessfulExit = false }` (AC 4): restart only on non-zero
    // exit. Story 2.5's graceful shutdown exits 0 on SIGTERM, so a clean
    // `bowerbird stop` stays down; a crash (non-zero) restarts. `RunAtLoad`
    // independently covers the reboot/login case.
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>{label}</string>
    <key>ProgramArguments</key>
    <array>
        <string>{daemon}</string>
    </array>
{env_block}    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <dict>
        <key>SuccessfulExit</key>
        <false/>
    </dict>
    <key>StandardOutPath</key>
    <string>{out}</string>
    <key>StandardErrorPath</key>
    <string>{err}</string>
</dict>
</plist>
"#
    )
}

/// Minimal XML text escaping for plist string values. Paths rarely contain
/// these, but a `&` or `<` in a home dir would otherwise corrupt the plist.
fn xml_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(c),
        }
    }
    out
}

/// Write `xml` to `plist_path` atomically (`.tmp` → rename, mode 0644),
/// creating the parent directory if absent.
pub fn write_launch_agent_plist(plist_path: &Path, xml: &str) -> anyhow::Result<()> {
    use anyhow::Context;

    if let Some(parent) = plist_path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create LaunchAgents dir {}", parent.display()))?;
    }

    // Unique temp name in the SAME directory (F5): two concurrent `bowerbird
    // install` runs writing a fixed `*.plist.tmp` could clobber each other's
    // temp file or race the rename. A pid+sequence suffix keeps each writer's
    // temp file private; the final rename stays atomic (same filesystem).
    let tmp = unique_tmp_path(plist_path);
    if let Err(e) = std::fs::write(&tmp, xml) {
        return Err(anyhow::Error::new(e).context(format!("write {}", tmp.display())));
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Err(e) = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o644)) {
            let _ = std::fs::remove_file(&tmp);
            return Err(anyhow::Error::new(e).context(format!("chmod 0644 {}", tmp.display())));
        }
    }

    if let Err(e) = std::fs::rename(&tmp, plist_path) {
        // Don't leave a stray temp file behind on a failed rename.
        let _ = std::fs::remove_file(&tmp);
        return Err(anyhow::Error::new(e).context(format!(
            "rename {} -> {}",
            tmp.display(),
            plist_path.display()
        )));
    }
    Ok(())
}

/// A per-writer temp path beside `plist_path` (same directory, so the rename is
/// atomic). The pid + process-local sequence keep two concurrent installs from
/// sharing a temp name (F5).
fn unique_tmp_path(plist_path: &Path) -> PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static TMP_SEQ: AtomicU64 = AtomicU64::new(0);

    let seq = TMP_SEQ.fetch_add(1, Ordering::Relaxed);
    let pid = std::process::id();
    let base = plist_path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "launch-agent.plist".to_string());
    plist_path.with_file_name(format!("{base}.{pid}.{seq}.tmp"))
}

/// Remove the LaunchAgent plist file. A missing file is a clean no-op.
pub fn remove_launch_agent_plist(plist_path: &Path) -> anyhow::Result<()> {
    match std::fs::remove_file(plist_path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(anyhow::Error::new(e).context(format!(
            "remove launch agent plist {}",
            plist_path.display()
        ))),
    }
}

// --- launchd interaction (macOS only) ------------------------------------

/// Load the LaunchAgent so launchd starts (via `RunAtLoad`) and supervises the
/// daemon. Modern API (`launchctl bootstrap gui/<uid> <plist>`) with the legacy
/// `launchctl load -w` as a fallback.
///
/// Idempotency is decided by **positive verification**, not by guessing from an
/// exit code (Story 5.9 review F3): a non-success bootstrap is only treated as
/// success when `launchctl print` confirms the job is actually loaded, so an
/// unrelated `Bootstrap failed: 5` no longer masquerades as "already loaded".
/// Callers that need the loaded job to match a freshly-written plist
/// (`install`) `bootout` first, so this only ever bootstraps the current plist.
#[cfg(target_os = "macos")]
pub fn bootstrap_launch_agent(plist_path: &Path) -> anyhow::Result<()> {
    let uid = current_uid();
    let domain = format!("gui/{uid}");
    let plist = plist_path.to_string_lossy().into_owned();

    let modern = run_launchctl(&["bootstrap", &domain, &plist])?;
    if modern.success {
        return Ok(());
    }
    // Already-loaded fast path (explicit stderr). Then try a positive
    // verification — but an UNVERIFIABLE `print` here must NOT short-circuit the
    // legacy fallback (Story 5.9 review pass-5 F5): an old/unsupported
    // environment can fail BOTH `bootstrap` AND `launchctl print` while
    // `load -w` still works, and `agent_loaded(uid)?` would return its "cannot
    // verify" error before we ever try the fallback. So treat "cannot verify" as
    // "keep going" here (`unwrap_or(false)`).
    if already_loaded(&modern) || agent_loaded(uid).unwrap_or(false) {
        return Ok(());
    }

    // Legacy fallback for environments where `bootstrap` is unavailable.
    let legacy = run_launchctl(&["load", "-w", &plist])?;
    // Final verification: both modern and legacy have now been attempted, so an
    // unverifiable `print` IS a real failure — `?` propagates "cannot verify".
    if legacy.success || already_loaded(&legacy) || agent_loaded(uid)? {
        return Ok(());
    }

    anyhow::bail!(
        "launchctl could not load {plist}: bootstrap -> [{}] {}; load -w -> [{}] {}",
        modern.code_display(),
        modern.stderr_trimmed(),
        legacy.code_display(),
        legacy.stderr_trimmed()
    )
}

/// Unload the LaunchAgent. Modern API (`launchctl bootout gui/<uid>/<label>`)
/// with the legacy `launchctl unload` fallback. Booting out an already-unloaded
/// agent is a clean no-op, not an error (AC 7) — confirmed via `launchctl print`
/// rather than an exit-code guess (F3).
#[cfg(target_os = "macos")]
pub fn bootout_launch_agent(plist_path: &Path) -> anyhow::Result<()> {
    let uid = current_uid();
    let target = format!("gui/{uid}/{LAUNCH_AGENT_LABEL}");

    let modern = run_launchctl(&["bootout", &target])?;
    if modern.success {
        return Ok(());
    }
    // `already_unloaded` is the explicit-stderr fast path. Otherwise try a
    // positive "not loaded" check — but an UNVERIFIABLE `print` here must NOT
    // short-circuit the legacy fallback (Story 5.9 review pass-5 F5): an
    // old/unsupported environment can fail BOTH `bootout` AND `launchctl print`
    // while `unload` still works, and `!agent_loaded(uid)?` would return its
    // "cannot verify" error before we try the fallback. Treat "cannot verify" as
    // "keep going" (`unwrap_or(true)` = assume still loaded, so we proceed to
    // the legacy unload rather than bailing).
    if already_unloaded(&modern) || !agent_loaded(uid).unwrap_or(true) {
        return Ok(());
    }

    let plist = plist_path.to_string_lossy().into_owned();
    let legacy = run_launchctl(&["unload", "-w", &plist])?;
    // Final verification: both modern and legacy have now been attempted. Only a
    // positively-verified "not loaded" is a clean no-op; an unverifiable `print`
    // (`?`) propagates so we never claim success while the agent may still linger.
    if legacy.success || already_unloaded(&legacy) || !agent_loaded(uid)? {
        return Ok(());
    }

    anyhow::bail!(
        "launchctl could not unload {target}: bootout -> [{}] {}; unload -> [{}] {}",
        modern.code_display(),
        modern.stderr_trimmed(),
        legacy.code_display(),
        legacy.stderr_trimmed()
    )
}

/// Start an already-loaded agent's job (`launchctl kickstart -k gui/<uid>/<label>`).
/// Used by `bowerbird start` when the LaunchAgent is registered but its ingest
/// socket is down (e.g. after a clean `bowerbird stop`, which
/// `KeepAlive=SuccessfulExit=false` leaves down).
///
/// `-k` (kill-then-restart) is load-bearing (Story 5.9 review pass-3 F3): a job
/// can be "running" yet wedged *before* it binds the ingest socket, and a plain
/// `kickstart` leaves a running job alone — so `bowerbird start` could never
/// repair the wedged daemon and would just time out. `-k` kills any running
/// instance first, then (re)starts it, which is correct for both the
/// loaded-but-down and loaded-but-wedged cases. Best-effort: a non-success exit
/// is returned as `Err` for the caller to surface.
#[cfg(target_os = "macos")]
pub fn kickstart_launch_agent() -> anyhow::Result<()> {
    let uid = current_uid();
    let target = format!("gui/{uid}/{LAUNCH_AGENT_LABEL}");
    let out = run_launchctl(&["kickstart", "-k", &target])?;
    if out.success {
        return Ok(());
    }
    anyhow::bail!(
        "launchctl could not kickstart {target}: [{}] {}",
        out.code_display(),
        out.stderr_trimmed()
    )
}

/// Read the `EnvironmentVariables` the LaunchAgent is registered with (F2).
/// Empty when the plist has no env block or cannot be read — callers fall back
/// to their own resolution in that case.
#[cfg(target_os = "macos")]
pub fn registered_plist_env(plist_path: &Path) -> Vec<(String, String)> {
    match std::fs::read_to_string(plist_path) {
        Ok(xml) => parse_plist_environment(&xml),
        Err(_) => Vec::new(),
    }
}

/// Is the LaunchAgent currently loaded in the user's GUI domain? Positive check
/// via `launchctl print gui/<uid>/<label>` (exit 0 = present). This is the
/// authoritative idempotency signal (F3) and the lifecycle discriminator
/// `install`/`start` use to tell a launchd-owned daemon from a manual one.
///
/// Fallible by design (review pass 2, F3): `Ok(true)` = the job is loaded,
/// `Ok(false)` = `launchctl` ran and reported the job absent, and `Err` = we
/// could not even run `launchctl` (spawn/permission failure) — i.e. "cannot
/// verify." Collapsing "cannot verify" into "not loaded" is exactly what let
/// `bootout` claim success while an agent was still supervising the session.
#[cfg(target_os = "macos")]
pub fn launch_agent_loaded() -> anyhow::Result<bool> {
    agent_loaded(current_uid())
}

#[cfg(target_os = "macos")]
fn agent_loaded(uid: u32) -> anyhow::Result<bool> {
    let target = format!("gui/{uid}/{LAUNCH_AGENT_LABEL}");
    // A spawn failure propagates as Err ("cannot verify"). Exit 0 = loaded.
    let out = run_launchctl(&["print", &target])?;
    if out.success {
        return Ok(true);
    }
    // A *non-zero* `print` is only "not loaded" when launchctl explicitly says
    // the service is absent (exit 113 / "Could not find ..."). Any other
    // non-zero exit — permission, domain-unavailable, transient I/O — is "cannot
    // verify" and must propagate as Err, NOT collapse to Ok(false) (Story 5.9
    // review pass-3 F1). Collapsing it is exactly what let `bootout_launch_agent`
    // accept `!agent_loaded()?` as a clean already-unloaded state and `uninstall`
    // remove the plist while the agent was actually still loaded.
    if print_signals_absent(out.code, &out.stderr) {
        return Ok(false);
    }
    anyhow::bail!(
        "could not verify launchd state for {target}: `launchctl print` exited [{}] {}",
        out.code_display(),
        out.stderr_trimmed()
    )
}

#[cfg(target_os = "macos")]
fn current_uid() -> u32 {
    // `libc` is already linked by the CLI (PID-file stop path). getuid() is
    // infallible.
    unsafe { libc::getuid() }
}

/// Run `launchctl` with `args`, capturing output. Errors only on spawn failure
/// (e.g. `launchctl` not found); a non-zero launchctl exit is reported via the
/// returned struct so the caller can distinguish "already in target state" from
/// a real failure.
#[cfg(target_os = "macos")]
fn run_launchctl(args: &[&str]) -> anyhow::Result<LaunchctlOutput> {
    use anyhow::Context;
    let output = std::process::Command::new("launchctl")
        .args(args)
        .output()
        .with_context(|| format!("invoke `launchctl {}`", args.join(" ")))?;
    Ok(LaunchctlOutput {
        success: output.status.success(),
        code: output.status.code(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    })
}

/// Captured result of a `launchctl` invocation.
#[cfg(target_os = "macos")]
struct LaunchctlOutput {
    success: bool,
    code: Option<i32>,
    stderr: String,
}

#[cfg(target_os = "macos")]
impl LaunchctlOutput {
    fn code_display(&self) -> String {
        self.code
            .map(|c| c.to_string())
            .unwrap_or_else(|| "signal".to_string())
    }
    fn stderr_trimmed(&self) -> &str {
        self.stderr.trim()
    }
}

#[cfg(target_os = "macos")]
fn already_loaded(out: &LaunchctlOutput) -> bool {
    stderr_signals_already_loaded(&out.stderr)
}

#[cfg(target_os = "macos")]
fn already_unloaded(out: &LaunchctlOutput) -> bool {
    signals_already_unloaded(out.code, &out.stderr)
}

// The classification logic below takes primitives (not the macOS-only
// `LaunchctlOutput`) so it compiles and unit-tests on Linux CI too, while the
// `launchctl`-spawning wrappers stay macOS-gated.

/// Does a failed `bootstrap`/`load` carry an explicit "already loaded" signal?
///
/// Narrowed for F3: the previous version treated *any* exit code 5 as
/// already-loaded, which silently swallowed unrelated `Bootstrap failed: 5`
/// I/O errors. We now only fast-path on explicit stderr signatures; everything
/// else falls through to the positive `launchctl print` verification in
/// `bootstrap_launch_agent`.
#[cfg(any(target_os = "macos", test))]
fn stderr_signals_already_loaded(stderr: &str) -> bool {
    let s = stderr.to_ascii_lowercase();
    s.contains("already loaded")
        || s.contains("already bootstrapped")
        || s.contains("service already loaded")
        || s.contains("already in progress")
        || s.contains("failed: 37") // EALREADY from `bootstrap`/`load` — already in target state
}

/// Does a failed `bootout`/`unload` mean the agent was already unloaded?
/// "No such process" (ESRCH=3) or a "could not find"/"not loaded" message all
/// mean "already in the target state."
#[cfg(any(target_os = "macos", test))]
fn signals_already_unloaded(code: Option<i32>, stderr: &str) -> bool {
    let s = stderr.to_ascii_lowercase();
    code == Some(3)
        || s.contains("no such")
        || s.contains("could not find")
        || s.contains("not find")
        || s.contains("not loaded")
}

/// Does a non-zero `launchctl print` mean the service is genuinely *absent*
/// (vs. an unverifiable error)? `launchctl print gui/<uid>/<label>` exits 113
/// ("Could not find specified service") with a "Could not find service … in
/// domain" message for an unregistered label. Anything else (permission,
/// domain-not-available-for-uid, transient I/O) is "cannot verify" and must NOT
/// be read as "not loaded" (Story 5.9 review pass-3 F1).
#[cfg(any(target_os = "macos", test))]
fn print_signals_absent(code: Option<i32>, stderr: &str) -> bool {
    let s = stderr.to_ascii_lowercase();
    code == Some(113)
        || s.contains("could not find")
        || s.contains("no such")
        || s.contains("not find")
        || s.contains("not loaded")
}

/// Parse the `EnvironmentVariables` dict out of a rendered LaunchAgent plist,
/// returning the `(key, value)` pairs. Pure / cross-platform so it unit-tests on
/// Linux CI too.
///
/// This is a deliberately small, format-specific scan rather than a general
/// plist parser: we render the plist ourselves in `render_launch_agent_plist`,
/// so the shape is fixed (`<key>EnvironmentVariables</key>` then a `<dict>` of
/// `<key>..</key><string>..</string>` pairs, with no nested dict). `bowerbird
/// start` reads it so the launchd-managed start probes the socket / waits for
/// `server.json` where launchd will actually run the daemon — the plist env
/// wins for the launchd-spawned process, so the current CLI env may differ
/// (Story 5.9 review pass-3 F2).
#[cfg(any(target_os = "macos", test))]
pub fn parse_plist_environment(xml: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let Some(env_at) = xml.find("<key>EnvironmentVariables</key>") else {
        return out;
    };
    let rest = &xml[env_at..];
    let Some(dict_start) = rest.find("<dict>") else {
        return out;
    };
    let Some(dict_end) = rest.find("</dict>") else {
        return out;
    };
    if dict_end < dict_start {
        return out;
    }
    let body = &rest[dict_start + "<dict>".len()..dict_end];

    // Walk `<key>..</key>` then the next `<string>..</string>`.
    let mut idx = 0;
    while let Some(k0) = body[idx..].find("<key>") {
        let kstart = idx + k0 + "<key>".len();
        let Some(k1) = body[kstart..].find("</key>") else {
            break;
        };
        let key = xml_unescape(&body[kstart..kstart + k1]);
        let after_key = kstart + k1 + "</key>".len();
        let Some(s0) = body[after_key..].find("<string>") else {
            break;
        };
        let sstart = after_key + s0 + "<string>".len();
        let Some(s1) = body[sstart..].find("</string>") else {
            break;
        };
        let val = xml_unescape(&body[sstart..sstart + s1]);
        out.push((key, val));
        idx = sstart + s1 + "</string>".len();
    }
    out
}

/// Inverse of [`xml_escape`] for the entity set we emit. Used when reading back
/// the plist we rendered (F2).
#[cfg(any(target_os = "macos", test))]
fn xml_unescape(s: &str) -> String {
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn plist_is_well_formed_and_carries_required_keys() {
        let xml = render_launch_agent_plist(
            LAUNCH_AGENT_LABEL,
            Path::new("/usr/local/bin/bowerbird-daemon"),
            Path::new("/Users/example/.bowerbird"),
            &[("BOWERBIRD_DATA_DIR", "/Users/example/.bowerbird")],
        );

        // XML/doctype/root shape.
        assert!(
            xml.starts_with("<?xml version=\"1.0\""),
            "xml prolog: {xml}"
        );
        assert!(xml.contains("<!DOCTYPE plist"), "doctype: {xml}");
        assert!(xml.contains("<plist version=\"1.0\">"));
        assert!(xml.trim_end().ends_with("</plist>"));

        // Label.
        assert!(xml.contains(&format!("<string>{LAUNCH_AGENT_LABEL}</string>")));

        // Absolute ProgramArguments path.
        assert!(xml.contains("<key>ProgramArguments</key>"));
        assert!(xml.contains("<string>/usr/local/bin/bowerbird-daemon</string>"));

        // RunAtLoad = true.
        assert!(xml.contains("<key>RunAtLoad</key>\n    <true/>"));

        // KeepAlive = { SuccessfulExit = false } — the load-bearing AC 4 choice.
        assert!(xml.contains("<key>KeepAlive</key>"));
        assert!(
            xml.contains("<key>SuccessfulExit</key>\n        <false/>"),
            "KeepAlive must be SuccessfulExit=false, not true: {xml}"
        );
        assert!(!xml.contains("<key>KeepAlive</key>\n    <true/>"));

        // std out/err under the data dir.
        assert!(xml.contains("<string>/Users/example/.bowerbird/daemon.out.log</string>"));
        assert!(xml.contains("<string>/Users/example/.bowerbird/daemon.err.log</string>"));

        // F1: EnvironmentVariables carries the resolved data dir so the
        // launchd-started daemon resolves DB/socket the same place the CLI
        // pointed the logs — and the bearer token is deliberately NOT embedded
        // in this 0644 file.
        assert!(
            xml.contains("<key>EnvironmentVariables</key>"),
            "env block: {xml}"
        );
        assert!(
            xml.contains("<key>BOWERBIRD_DATA_DIR</key>"),
            "data dir env: {xml}"
        );
        assert!(
            !xml.contains("BOWERBIRD_TOKEN"),
            "the bearer token must never be written into the world-readable plist: {xml}"
        );
    }

    #[test]
    fn plist_omits_env_block_when_no_overrides() {
        let xml = render_launch_agent_plist(
            LAUNCH_AGENT_LABEL,
            Path::new("/usr/local/bin/bowerbird-daemon"),
            Path::new("/Users/example/.bowerbird"),
            &[],
        );
        assert!(
            !xml.contains("<key>EnvironmentVariables</key>"),
            "empty env must omit the dict entirely: {xml}"
        );
        // The structure stays well-formed with no env block.
        assert!(xml.contains("<key>ProgramArguments</key>"));
        assert!(xml.contains("<key>RunAtLoad</key>\n    <true/>"));
    }

    #[test]
    fn plist_escapes_xml_metacharacters_in_paths() {
        let xml = render_launch_agent_plist(
            "com.example.test",
            Path::new("/opt/a&b/bowerbird-daemon"),
            Path::new("/home/a<b>/.bowerbird"),
            &[],
        );
        assert!(xml.contains("/opt/a&amp;b/bowerbird-daemon"));
        assert!(xml.contains("/home/a&lt;b&gt;/.bowerbird/daemon.out.log"));
        // Raw metacharacters must not leak into the rendered XML.
        assert!(!xml.contains("a&b/"));
        assert!(!xml.contains("a<b>"));
    }

    #[test]
    fn daemon_bin_resolver_prefers_absolute_env_override() {
        // Save/restore the env var so the test is order-independent.
        let prev = std::env::var_os("BOWERBIRD_DAEMON_BIN");
        std::env::set_var("BOWERBIRD_DAEMON_BIN", "/custom/abs/bowerbird-daemon");
        let resolved = resolve_daemon_bin_absolute().expect("absolute override resolves");
        assert_eq!(resolved, PathBuf::from("/custom/abs/bowerbird-daemon"));

        // A non-absolute override is rejected (launchd needs absolute).
        std::env::set_var("BOWERBIRD_DAEMON_BIN", "bowerbird-daemon");
        assert!(
            resolve_daemon_bin_absolute().is_err(),
            "non-absolute override must error"
        );

        match prev {
            Some(v) => std::env::set_var("BOWERBIRD_DAEMON_BIN", v),
            None => std::env::remove_var("BOWERBIRD_DAEMON_BIN"),
        }
    }

    #[test]
    fn daemon_bin_resolver_finds_current_exe_sibling() {
        // With no env override, the resolver should find a sibling of the test
        // binary. The test harness's current_exe lives in target/debug/deps or
        // target/debug; a sibling `bowerbird-daemon` may or may not exist there,
        // so we only assert the resolver does not panic and returns an absolute
        // path when it succeeds.
        let prev = std::env::var_os("BOWERBIRD_DAEMON_BIN");
        std::env::remove_var("BOWERBIRD_DAEMON_BIN");
        if let Ok(p) = resolve_daemon_bin_absolute() {
            assert!(
                p.is_absolute(),
                "resolved daemon path must be absolute: {}",
                p.display()
            );
        }
        if let Some(v) = prev {
            std::env::set_var("BOWERBIRD_DAEMON_BIN", v);
        }
    }

    #[test]
    fn ensure_daemon_launchable_accepts_executable_rejects_others() {
        let dir = tempfile::tempdir().expect("tempdir");

        // A regular file with no exec bit is rejected.
        let plain = dir.path().join("not-exec");
        std::fs::write(&plain, b"#!/bin/sh\n").expect("write plain");
        assert!(!is_executable_file(&plain));
        assert!(ensure_daemon_launchable(&plain).is_err());

        // A missing path is rejected (the unlaunchable-registration case F4 guards).
        let missing = dir.path().join("nope");
        assert!(!is_executable_file(&missing));
        assert!(ensure_daemon_launchable(&missing).is_err());

        // An executable file is accepted.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let exec = dir.path().join("daemon");
            std::fs::write(&exec, b"#!/bin/sh\n").expect("write exec");
            std::fs::set_permissions(&exec, std::fs::Permissions::from_mode(0o755))
                .expect("chmod +x");
            assert!(is_executable_file(&exec));
            assert!(ensure_daemon_launchable(&exec).is_ok());
        }
    }

    #[test]
    fn already_loaded_matches_explicit_signals_not_bare_code_5() {
        // F3: a bare `Bootstrap failed: 5` with no "already" wording is a real
        // failure now, NOT swallowed as already-loaded.
        assert!(
            !stderr_signals_already_loaded("Bootstrap failed: 5: Input/output error"),
            "bare exit-5 I/O error must not be treated as already-loaded"
        );

        for msg in [
            "Load failed: 37: Operation already in progress",
            "service already loaded",
            "Bootstrap failed: 37",
            "Service is already bootstrapped",
        ] {
            assert!(
                stderr_signals_already_loaded(msg),
                "should match explicit signal: {msg}"
            );
        }
    }

    #[test]
    fn unique_tmp_path_is_distinct_per_call_and_stays_in_dir() {
        // F5: two writers must not share a temp name; the temp must live in the
        // plist's own directory so the rename is atomic (same filesystem).
        let plist = Path::new("/Users/example/Library/LaunchAgents/com.example.daemon.plist");
        let a = unique_tmp_path(plist);
        let b = unique_tmp_path(plist);
        assert_ne!(a, b, "successive temp paths must differ");
        assert_eq!(
            a.parent(),
            plist.parent(),
            "temp must stay beside the plist"
        );
        assert_eq!(b.parent(), plist.parent());
        for p in [&a, &b] {
            let name = p.file_name().unwrap().to_string_lossy();
            assert!(
                name.starts_with("com.example.daemon.plist."),
                "temp name keeps the plist basename prefix: {name}"
            );
            assert!(name.ends_with(".tmp"), "temp name ends with .tmp: {name}");
        }
    }

    #[test]
    fn print_absent_only_for_explicit_absent_signals() {
        // F1: an explicit "could not find" / 113 / "no such" => genuinely absent.
        assert!(print_signals_absent(
            Some(113),
            "Could not find service \"com.example\" in domain for gui"
        ));
        assert!(print_signals_absent(Some(1), "No such process"));
        assert!(print_signals_absent(None, "could not find service"));

        // An unrelated non-zero error must NOT read as absent — `agent_loaded`
        // must surface it as "cannot verify" (Err), not "not loaded".
        assert!(
            !print_signals_absent(Some(1), "Operation not permitted"),
            "a permission error is unverifiable, not absent"
        );
        assert!(
            !print_signals_absent(Some(5), "Bootstrap failed: 5: Input/output error"),
            "a transient I/O error is unverifiable, not absent"
        );
    }

    #[test]
    fn parse_plist_environment_round_trips_rendered_env() {
        // F2: the env we render must read back exactly (the start-time probe and
        // readiness wait depend on it).
        let xml = render_launch_agent_plist(
            LAUNCH_AGENT_LABEL,
            Path::new("/usr/local/bin/bowerbird-daemon"),
            Path::new("/data/dir"),
            &[
                ("BOWERBIRD_DATA_DIR", "/data/dir"),
                ("BOWERBIRD_INGEST_SOCK", "/data/dir/ingest.sock"),
            ],
        );
        let env = parse_plist_environment(&xml);
        assert_eq!(
            env,
            vec![
                ("BOWERBIRD_DATA_DIR".to_string(), "/data/dir".to_string()),
                (
                    "BOWERBIRD_INGEST_SOCK".to_string(),
                    "/data/dir/ingest.sock".to_string()
                ),
            ]
        );
    }

    #[test]
    fn parse_plist_environment_empty_when_no_env_block() {
        // No env block (the empty-overrides render) => no pairs, and a daemon-only
        // plist (the start test's `<plist/>` stub) is also handled gracefully.
        let xml = render_launch_agent_plist(
            LAUNCH_AGENT_LABEL,
            Path::new("/usr/local/bin/bowerbird-daemon"),
            Path::new("/data/dir"),
            &[],
        );
        assert!(parse_plist_environment(&xml).is_empty());
        assert!(parse_plist_environment("<plist/>\n").is_empty());
    }

    #[test]
    fn parse_plist_environment_unescapes_metacharacters() {
        let xml = render_launch_agent_plist(
            LAUNCH_AGENT_LABEL,
            Path::new("/usr/local/bin/bowerbird-daemon"),
            Path::new("/a&b"),
            &[("BOWERBIRD_DATA_DIR", "/a&b/<x>")],
        );
        let env = parse_plist_environment(&xml);
        assert_eq!(
            env,
            vec![("BOWERBIRD_DATA_DIR".to_string(), "/a&b/<x>".to_string())]
        );
    }

    #[test]
    fn already_unloaded_matches_no_such_and_not_found() {
        assert!(signals_already_unloaded(
            Some(3),
            "Boot-out failed: 3: No such process"
        ));
        assert!(signals_already_unloaded(
            Some(113),
            "Could not find service in domain"
        ));
        assert!(
            !signals_already_unloaded(Some(1), "Boot-out failed: 1: Operation not permitted"),
            "a permission error is a real failure"
        );
    }
}
