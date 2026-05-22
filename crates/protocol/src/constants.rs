pub const SHIM_BINARY_NAME: &str = "bowerbird";

/// Maximum bytes the shim reads from stdin before injecting `hook_kind`
/// and appending the trailing newline that frames the daemon's
/// ingest-socket line.
///
/// Shared by both `crates/shim` (raw stdin cap) and `crates/daemon`
/// (used to compute the daemon's wire-line cap, which adds slack for
/// the shim's framing overhead). Keeping the canonical value here
/// prevents the two crates from drifting silently — if either side
/// raises the cap without the other, payloads at the new boundary
/// fail with confusing errors.
pub const SHIM_STDIN_CAP_BYTES: usize = 1 << 20;
