//! Shared session-state filter parsing + matching (Story 5.8, ADR 0008).
//!
//! The `?state=` REST query param (`api::sessions::list`) and the WS
//! `Subscribe.states` snapshot filter (`api::ws` → `projection::snapshot`)
//! filter on the SAME predicate — the read-time-derived `current_state` — so
//! the token semantics are defined exactly once here.
//!
//! Tokens are case-insensitive `SessionCurrentState` wire variants; an empty
//! filter means "match everything" (the default, preserving pre-5.8 behavior).

use protocol::SessionCurrentState;

/// Parse one session-state token. Trimmed and lowercased, so `Working`,
/// `working`, and ` WORKING ` all map to [`SessionCurrentState::Working`].
///
/// Returns `Err(message)` — naming the bad token and the accepted set — on an
/// empty or unrecognized token. A token containing a comma is rejected with a
/// hint, because the only caller that would produce one is a presenter
/// CSV-joining into a single WS `states` array element (see
/// [`parse_states_array`]); the REST CSV path splits *before* it reaches here,
/// so one grammar input is always one token.
///
/// Parsed here rather than typed as a `Vec<SessionCurrentState>` on the wire:
/// `SessionCurrentState` decodes permissively (`#[serde(other)] Unknown`), so a
/// typed field would silently map a typo to `Unknown`. Parsing daemon-side
/// keeps the loud-reject the strict-inbound policy wants.
pub fn parse_state_token(tok: &str) -> Result<SessionCurrentState, String> {
    let t = tok.trim().to_ascii_lowercase();
    match t.as_str() {
        "idle" => Ok(SessionCurrentState::Idle),
        "working" => Ok(SessionCurrentState::Working),
        "waitinginput" => Ok(SessionCurrentState::WaitingInput),
        "ended" => Ok(SessionCurrentState::Ended),
        "unknown" => Ok(SessionCurrentState::Unknown),
        _ if tok.contains(',') => Err(format!(
            "invalid state token {tok:?}: WS `states` is an array of single \
             tokens, not a comma-separated string — pass [\"working\",\"idle\"], \
             not [\"working,idle\"]"
        )),
        _ => Err(format!(
            "invalid state token {tok:?}; accepted tokens are \
             idle, working, waitinginput, ended, unknown \
             (case-insensitive)"
        )),
    }
}

/// Parse a comma-separated list of session-state tokens (the REST `?state=`
/// CSV) into the set to filter on. Splits on `,`, then parses each token via
/// [`parse_state_token`].
///
/// Returns `Err` on any empty token (e.g. a trailing comma) or unrecognized
/// token. Callers treat an absent param as "no filter" and never call this with
/// an empty string (which would split to one empty token → `Err`).
pub fn parse_state_filter(raw: &str) -> Result<Vec<SessionCurrentState>, String> {
    raw.split(',').map(parse_state_token).collect()
}

/// Parse the WS `Subscribe.states` array (Story 5.8, ADR 0008) — each element
/// is validated as a single token via [`parse_state_token`], so a comma-bearing
/// element like `"working,ended"` is rejected rather than silently accepted as
/// two tokens (the bug a CSV-join would have hidden). An empty array means "no
/// filter" and returns an empty vec without erroring (the v1.0 default).
pub fn parse_states_array(states: &[String]) -> Result<Vec<SessionCurrentState>, String> {
    states.iter().map(|s| parse_state_token(s)).collect()
}

/// True if `derived` passes the filter. An empty filter matches everything
/// (the default-unfiltered behavior). **Operates on the read-derived
/// `current_state`, never the stored one** — both call sites pass the value
/// returned by `current_state_for_read`.
pub fn state_matches(filter: &[SessionCurrentState], derived: SessionCurrentState) -> bool {
    filter.is_empty() || filter.contains(&derived)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_single_valid() {
        assert_eq!(
            parse_state_filter("working").unwrap(),
            vec![SessionCurrentState::Working]
        );
    }

    #[test]
    fn parse_multi_mixed_case_with_whitespace() {
        assert_eq!(
            parse_state_filter("Working, idle ,ENDED").unwrap(),
            vec![
                SessionCurrentState::Working,
                SessionCurrentState::Idle,
                SessionCurrentState::Ended,
            ]
        );
    }

    #[test]
    fn parse_all_five_tokens() {
        assert_eq!(
            parse_state_filter("idle,working,waitinginput,ended,unknown").unwrap(),
            vec![
                SessionCurrentState::Idle,
                SessionCurrentState::Working,
                SessionCurrentState::WaitingInput,
                SessionCurrentState::Ended,
                SessionCurrentState::Unknown,
            ]
        );
    }

    #[test]
    fn parse_unrecognized_token_errors() {
        let err = parse_state_filter("running").unwrap_err();
        assert!(err.contains("running"), "message must name the bad token");
        assert!(err.contains("accepted tokens"), "message must list the set");
    }

    #[test]
    fn parse_empty_token_in_list_errors() {
        // A trailing comma yields an empty token — corrupt input, not "no filter".
        assert!(parse_state_filter("working,").is_err());
    }

    #[test]
    fn parse_empty_string_errors() {
        // Present-but-empty `?state=` is malformed; absent is the caller's
        // "no filter" path and never reaches this function.
        assert!(parse_state_filter("").is_err());
    }

    #[test]
    fn parse_states_array_valid_per_element() {
        assert_eq!(
            parse_states_array(&["working".into(), "Idle".into()]).unwrap(),
            vec![SessionCurrentState::Working, SessionCurrentState::Idle]
        );
    }

    #[test]
    fn parse_states_array_empty_is_no_filter() {
        assert_eq!(parse_states_array(&[]).unwrap(), Vec::new());
    }

    #[test]
    fn parse_states_array_rejects_comma_bearing_element() {
        // The finding: a CSV-joined element must NOT parse as two tokens.
        let err = parse_states_array(&["working,ended".into()]).unwrap_err();
        assert!(
            err.contains("working,ended"),
            "message must name the bad element"
        );
        assert!(
            err.contains("array of single tokens"),
            "message must explain the array-not-CSV contract"
        );
    }

    #[test]
    fn parse_states_array_rejects_unknown_element() {
        assert!(parse_states_array(&["working".into(), "running".into()]).is_err());
    }

    #[test]
    fn state_matches_empty_filter_matches_all() {
        for s in [
            SessionCurrentState::Idle,
            SessionCurrentState::Working,
            SessionCurrentState::WaitingInput,
            SessionCurrentState::Ended,
            SessionCurrentState::Unknown,
        ] {
            assert!(state_matches(&[], s));
        }
    }

    #[test]
    fn state_matches_non_empty_filter() {
        let filter = [SessionCurrentState::Working, SessionCurrentState::Idle];
        assert!(state_matches(&filter, SessionCurrentState::Working));
        assert!(state_matches(&filter, SessionCurrentState::Idle));
        assert!(!state_matches(&filter, SessionCurrentState::Ended));
    }
}
