//! Session state-machine projection — pure functions only.
//!
//! `transition` computes the stored state for a session given the previous
//! state and the incoming event kind. `current_state_for_read` applies the
//! read-time stale-Working fallback so dropped `PostToolUse` hooks do not leave
//! a session permanently stuck.
//!
//! Wall-clock is an input (`now_ms`), never read inside these functions — that
//! preserves the deterministic-test discipline (project-context line 642) and
//! keeps the storage layer a pure function of the event sequence (AC #5).

use protocol::{EventKind, NotificationType, SessionCurrentState, SessionState};

/// Read-time fallback window for stale `Working` states (5 minutes).
///
/// If `now_ms - last_event_at_ms > STALE_WORKING_MS` and the stored state is
/// `Working`, `current_state_for_read` returns `Idle`. The stored row is left
/// untouched — see Dev Notes "Hook unreliability mitigation".
///
/// As of Story 5.3 the canonical Working→Idle transitions are `Stop` (turn
/// ended) and the liveness-probe `SessionEnded` (process gone). PostToolUse
/// now unconditionally returns Working (refined from Story 5.2's "preserve
/// prior"). This fallback's primary role is backstopping a dropped `Stop` —
/// when the daemon observes the session's process is still alive but no Stop
/// has arrived. ADR 0004 §5 flags this fallback for retirement once
/// daemon-observed liveness is the canonical "session ended" signal.
pub(crate) const STALE_WORKING_MS: i64 = 300_000;

/// Compute the new `SessionState` to store after observing `event_kind`.
///
/// Pure function. `pid` is the incoming envelope's PID (Story 5.3 AC #2 — the
/// shim injects `bowerbird_ppid` and the adapter extracts it); `last_pid`
/// follows carry-forward semantics: an envelope with `pid: Some(M)` overwrites,
/// an envelope with `pid: None` preserves prev's `last_pid`. `notification_type`
/// drives the typed-notification branching for `EventKind::Notification` (Story
/// 5.3 AC #7/#8).
///
/// Sentinel kinds (`RecordingStarted`, `RecordingEnded`) are not expected to
/// reach this function — sentinel write paths route directly through their own
/// helpers. If a sentinel is passed defensively, prev is returned unchanged
/// (with `last_pid` carry-forwarded from the new envelope's `pid` overlay).
pub(crate) fn transition(
    prev: Option<&SessionState>,
    event_kind: EventKind,
    notification_type: Option<NotificationType>,
    pid: Option<u32>,
    now_ms: i64,
) -> SessionState {
    // Carry-forward / overwrite-on-Some semantics for last_pid. Applied to
    // EVERY arm — including defensive arms — because last_pid is independent
    // of the state-machine logic. Story 5.3 AC #5/#6.
    let next_last_pid = pid.or(prev.and_then(|s| s.last_pid));

    let next_current = match event_kind {
        EventKind::UserPromptSubmit => SessionCurrentState::Working,
        EventKind::PreToolUse => SessionCurrentState::Working,
        // Story 5.3 AC #9: PostToolUse refined to `→ Working` unconditionally.
        // Refines Story 5.2's "preserve prior" rule — a session in
        // WaitingInput whose tool call completes mid-elicitation now correctly
        // transitions back to Working (closes a Story 5.1 dogfooding finding).
        // Behavioral change recorded in protocol-changelog.md as `type:
        // behavioral`.
        EventKind::PostToolUse => SessionCurrentState::Working,
        EventKind::Stop => SessionCurrentState::Idle,
        // Story 5.3 AC #7/#8: Notification branches on typed notification_type.
        //   PermissionPrompt | IdlePrompt | ElicitationDialog → WaitingInput
        //   AuthSuccess | ElicitationResponse | ElicitationComplete | Unknown | None → preserve prior
        // The "preserve prior" branch still updates last_event_kind /
        // last_event_at_ms (the event happened) — only current_state is
        // preserved.
        EventKind::Notification => match notification_type {
            Some(NotificationType::PermissionPrompt)
            | Some(NotificationType::IdlePrompt)
            | Some(NotificationType::ElicitationDialog) => SessionCurrentState::WaitingInput,
            Some(NotificationType::AuthSuccess)
            | Some(NotificationType::ElicitationResponse)
            | Some(NotificationType::ElicitationComplete)
            | Some(NotificationType::Unknown)
            | None => prev
                .map(|s| s.current_state)
                .unwrap_or(SessionCurrentState::Idle),
        },
        // Story 5.3 AC #10/#11: daemon-observed liveness emits SessionEnded
        // via projection::session::write; the projection transitions to
        // `Ended`. Non-terminal — the next hook event transitions out via the
        // normal arms (e.g. UserPromptSubmit from `claude --resume` → Working).
        EventKind::SessionEnded => SessionCurrentState::Ended,
        // Defensive guard — not an expected code path. Sentinels write to the
        // `__daemon__/__daemon__` row via their own helpers and never reach
        // this function. If they ever do, preserve prior state rather than
        // corrupting the projection.
        //
        // `EventKind::Unknown` is the decode-only wire catch-all from Story
        // 4.4 (Epic 2 retro AI-4). The daemon never CONSTRUCTS Unknown — the
        // adapter normalize layer rejects unknown hook strings at the
        // boundary — but defense-in-depth keeps the projection layer correct
        // if a future code path ever does. Same handling as the sentinels:
        // preserve prior state, do not corrupt the projection.
        EventKind::RecordingStarted | EventKind::RecordingEnded | EventKind::Unknown => {
            return prev
                .cloned()
                .map(|s| SessionState {
                    last_pid: next_last_pid,
                    ..s
                })
                .unwrap_or(SessionState {
                    current_state: SessionCurrentState::Idle,
                    last_event_kind: event_kind,
                    last_event_at_ms: now_ms,
                    last_pid: next_last_pid,
                });
        }
    };

    SessionState {
        current_state: next_current,
        last_event_kind: event_kind,
        last_event_at_ms: now_ms,
        last_pid: next_last_pid,
    }
}

/// Read-time view of `current_state`, applying the stale-Working fallback.
///
/// `Working` older than `STALE_WORKING_MS` is surfaced as `Idle`. All other
/// states (and any `Working` younger than the threshold) pass through. Does
/// not mutate the stored row — the storage layer remains a pure function of
/// the event sequence (AC #5).
pub fn current_state_for_read(stored: &SessionState, now_ms: i64) -> SessionCurrentState {
    // `saturating_sub` so a future-dated `last_event_at_ms` (clock skew, corrupted
    // row, dev fixture) cannot panic on overflow in debug builds. A future
    // timestamp saturates to `i64::MIN`, which is not `> STALE_WORKING_MS`, so we
    // surface the stored state unchanged — i.e., future timestamps are treated
    // as "fresh," never as "stale."
    let age_ms = now_ms.saturating_sub(stored.last_event_at_ms);
    if stored.current_state == SessionCurrentState::Working && age_ms > STALE_WORKING_MS {
        SessionCurrentState::Idle
    } else {
        stored.current_state
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(prev: Option<&SessionState>, kind: EventKind, now_ms: i64) -> SessionState {
        transition(prev, kind, None, None, now_ms)
    }

    #[test]
    fn transition_first_event_pretooluse_yields_working() {
        let next = t(None, EventKind::PreToolUse, 1_000);
        assert_eq!(next.current_state, SessionCurrentState::Working);
        assert_eq!(next.last_event_kind, EventKind::PreToolUse);
        assert_eq!(next.last_event_at_ms, 1_000);
    }

    #[test]
    fn transition_posttooluse_yields_working_unconditionally() {
        // Story 5.3 AC #9 refines Story 5.2: PostToolUse now unconditionally
        // returns Working. Previously preserved prior — a session in
        // WaitingInput whose tool call completed mid-elicitation got stuck.
        // No prior → Working.
        let post_no_prev = t(None, EventKind::PostToolUse, 1_000);
        assert_eq!(post_no_prev.current_state, SessionCurrentState::Working);

        // Prior Idle → Working (was: Idle).
        let prev_idle = SessionState {
            current_state: SessionCurrentState::Idle,
            last_event_kind: EventKind::Stop,
            last_event_at_ms: 0,
            last_pid: None,
        };
        let after = t(Some(&prev_idle), EventKind::PostToolUse, 2_000);
        assert_eq!(after.current_state, SessionCurrentState::Working);

        // Prior WaitingInput → Working (the load-bearing case for AC #9).
        let prev_wi = SessionState {
            current_state: SessionCurrentState::WaitingInput,
            last_event_kind: EventKind::Notification,
            last_event_at_ms: 0,
            last_pid: None,
        };
        let after = t(Some(&prev_wi), EventKind::PostToolUse, 3_000);
        assert_eq!(after.current_state, SessionCurrentState::Working);

        // Prior Ended → Working (resume case from Ended).
        let prev_ended = SessionState {
            current_state: SessionCurrentState::Ended,
            last_event_kind: EventKind::SessionEnded,
            last_event_at_ms: 0,
            last_pid: Some(100),
        };
        let after = t(Some(&prev_ended), EventKind::PostToolUse, 4_000);
        assert_eq!(after.current_state, SessionCurrentState::Working);
    }

    #[test]
    fn transition_user_prompt_submit_yields_working() {
        let next = t(None, EventKind::UserPromptSubmit, 1_000);
        assert_eq!(next.current_state, SessionCurrentState::Working);
        assert_eq!(next.last_event_kind, EventKind::UserPromptSubmit);
        assert_eq!(next.last_event_at_ms, 1_000);
    }

    #[test]
    fn transition_user_prompt_submit_then_pretooluse_stays_working() {
        let after_ups = t(None, EventKind::UserPromptSubmit, 1_000);
        let after_pre = t(Some(&after_ups), EventKind::PreToolUse, 2_000);
        assert_eq!(after_pre.current_state, SessionCurrentState::Working);
        assert_eq!(after_pre.last_event_kind, EventKind::PreToolUse);
    }

    // Story 5.3 AC #7: three input-required notification_type values trigger
    // WaitingInput; the prior state is irrelevant.
    #[test]
    fn transition_notification_input_required_yields_waiting_input() {
        for nt in [
            NotificationType::PermissionPrompt,
            NotificationType::IdlePrompt,
            NotificationType::ElicitationDialog,
        ] {
            let next = transition(None, EventKind::Notification, Some(nt), None, 5_000);
            assert_eq!(
                next.current_state,
                SessionCurrentState::WaitingInput,
                "notification_type {nt:?} must transition to WaitingInput"
            );
        }
    }

    // Story 5.3 AC #8: three transient notification_type values + Unknown +
    // None preserve the prior current_state but still update
    // last_event_kind/last_event_at_ms.
    #[test]
    fn transition_notification_transient_preserves_prior() {
        let prev = SessionState {
            current_state: SessionCurrentState::Working,
            last_event_kind: EventKind::PreToolUse,
            last_event_at_ms: 1_000,
            last_pid: Some(99),
        };
        let cases: &[Option<NotificationType>] = &[
            Some(NotificationType::AuthSuccess),
            Some(NotificationType::ElicitationResponse),
            Some(NotificationType::ElicitationComplete),
            Some(NotificationType::Unknown),
            None,
        ];
        for nt in cases {
            let next = transition(Some(&prev), EventKind::Notification, *nt, None, 2_000);
            assert_eq!(
                next.current_state,
                SessionCurrentState::Working,
                "notification_type {nt:?} must preserve prior current_state"
            );
            assert_eq!(
                next.last_event_kind,
                EventKind::Notification,
                "last_event_kind must update even when current_state is preserved"
            );
            assert_eq!(next.last_event_at_ms, 2_000);
        }
    }

    // Story 5.3 AC #8: transient notification arriving without a prior state
    // defaults current_state to Idle (no prior == nothing to preserve).
    #[test]
    fn transition_notification_transient_without_prev_defaults_to_idle() {
        let next = transition(
            None,
            EventKind::Notification,
            Some(NotificationType::AuthSuccess),
            None,
            5_000,
        );
        assert_eq!(next.current_state, SessionCurrentState::Idle);
        assert_eq!(next.last_event_kind, EventKind::Notification);
    }

    #[test]
    fn transition_stop_clears_working() {
        let after_pre = t(None, EventKind::PreToolUse, 1_000);
        let after_stop = t(Some(&after_pre), EventKind::Stop, 2_000);
        assert_eq!(after_stop.current_state, SessionCurrentState::Idle);
        assert_eq!(after_stop.last_event_kind, EventKind::Stop);
    }

    #[test]
    fn transition_pretooluse_without_posttooluse_keeps_working() {
        let next = t(None, EventKind::PreToolUse, 1_000);
        assert_eq!(next.current_state, SessionCurrentState::Working);
    }

    // Story 5.3 AC #10: SessionEnded transitions current_state to Ended.
    #[test]
    fn transition_session_ended_yields_ended() {
        let prev = SessionState {
            current_state: SessionCurrentState::Working,
            last_event_kind: EventKind::PreToolUse,
            last_event_at_ms: 1_000,
            last_pid: Some(42),
        };
        let next = t(Some(&prev), EventKind::SessionEnded, 5_000);
        assert_eq!(next.current_state, SessionCurrentState::Ended);
        assert_eq!(next.last_event_kind, EventKind::SessionEnded);
        assert_eq!(next.last_event_at_ms, 5_000);
        // carry-forward
        assert_eq!(next.last_pid, Some(42));
    }

    // Story 5.3 AC #12: from Ended, the next hook event transitions out via
    // the normal arms (Ended is non-terminal).
    #[test]
    fn transition_from_ended_resumes_on_hook_event() {
        let ended = SessionState {
            current_state: SessionCurrentState::Ended,
            last_event_kind: EventKind::SessionEnded,
            last_event_at_ms: 0,
            last_pid: Some(7),
        };
        let after_ups = t(Some(&ended), EventKind::UserPromptSubmit, 1_000);
        assert_eq!(after_ups.current_state, SessionCurrentState::Working);

        let after_stop = t(Some(&ended), EventKind::Stop, 2_000);
        assert_eq!(after_stop.current_state, SessionCurrentState::Idle);

        let after_n = transition(
            Some(&ended),
            EventKind::Notification,
            Some(NotificationType::PermissionPrompt),
            None,
            3_000,
        );
        assert_eq!(after_n.current_state, SessionCurrentState::WaitingInput);
    }

    // Story 5.3 AC #5/#6: last_pid carry-forward + overwrite-on-Some.
    #[test]
    fn transition_carry_forward_last_pid() {
        // prev None + envelope None → None
        let n1 = transition(None, EventKind::PreToolUse, None, None, 1_000);
        assert_eq!(n1.last_pid, None);

        // prev None + envelope Some(100) → Some(100)
        let n2 = transition(None, EventKind::PreToolUse, None, Some(100), 1_000);
        assert_eq!(n2.last_pid, Some(100));

        // prev Some(100) + envelope None → carry forward
        let prev = SessionState {
            current_state: SessionCurrentState::Idle,
            last_event_kind: EventKind::Stop,
            last_event_at_ms: 0,
            last_pid: Some(100),
        };
        let n3 = transition(Some(&prev), EventKind::PreToolUse, None, None, 1_000);
        assert_eq!(n3.last_pid, Some(100));

        // prev Some(100) + envelope Some(200) → overwrite
        let n4 = transition(Some(&prev), EventKind::PreToolUse, None, Some(200), 1_000);
        assert_eq!(n4.last_pid, Some(200));
    }

    #[test]
    fn current_state_for_read_returns_working_when_fresh() {
        let stored = SessionState {
            current_state: SessionCurrentState::Working,
            last_event_kind: EventKind::PreToolUse,
            last_event_at_ms: 1_000,
            last_pid: None,
        };
        let now = 1_000 + STALE_WORKING_MS - 1;
        assert_eq!(
            current_state_for_read(&stored, now),
            SessionCurrentState::Working
        );
    }

    #[test]
    fn current_state_for_read_returns_idle_when_stale() {
        let stored = SessionState {
            current_state: SessionCurrentState::Working,
            last_event_kind: EventKind::PreToolUse,
            last_event_at_ms: 1_000,
            last_pid: None,
        };
        let now = 1_000 + STALE_WORKING_MS + 1;
        assert_eq!(
            current_state_for_read(&stored, now),
            SessionCurrentState::Idle
        );
    }

    #[test]
    fn current_state_for_read_returns_idle_at_exactly_threshold() {
        let stored = SessionState {
            current_state: SessionCurrentState::Working,
            last_event_kind: EventKind::PreToolUse,
            last_event_at_ms: 1_000,
            last_pid: None,
        };
        let now = 1_000 + STALE_WORKING_MS;
        assert_eq!(
            current_state_for_read(&stored, now),
            SessionCurrentState::Working
        );
    }

    #[test]
    fn current_state_for_read_does_not_stale_idle() {
        let stored = SessionState {
            current_state: SessionCurrentState::Idle,
            last_event_kind: EventKind::PostToolUse,
            last_event_at_ms: 0,
            last_pid: None,
        };
        let now = i64::MAX / 2;
        assert_eq!(
            current_state_for_read(&stored, now),
            SessionCurrentState::Idle
        );
    }

    #[test]
    fn current_state_for_read_does_not_stale_waiting_input() {
        let stored = SessionState {
            current_state: SessionCurrentState::WaitingInput,
            last_event_kind: EventKind::Notification,
            last_event_at_ms: 0,
            last_pid: None,
        };
        let now = i64::MAX / 2;
        assert_eq!(
            current_state_for_read(&stored, now),
            SessionCurrentState::WaitingInput
        );
    }

    // Story 5.3: Ended must pass through current_state_for_read unchanged —
    // the stale-Working fallback only special-cases Working.
    #[test]
    fn current_state_for_read_does_not_stale_ended() {
        let stored = SessionState {
            current_state: SessionCurrentState::Ended,
            last_event_kind: EventKind::SessionEnded,
            last_event_at_ms: 0,
            last_pid: Some(123),
        };
        let now = i64::MAX / 2;
        assert_eq!(
            current_state_for_read(&stored, now),
            SessionCurrentState::Ended,
            "Ended must not be touched by the stale-Working read-time fallback"
        );
    }

    #[test]
    fn current_state_for_read_does_not_panic_on_future_timestamp() {
        let stored = SessionState {
            current_state: SessionCurrentState::Working,
            last_event_kind: EventKind::PreToolUse,
            last_event_at_ms: i64::MAX,
            last_pid: None,
        };
        assert_eq!(
            current_state_for_read(&stored, 0),
            SessionCurrentState::Working,
            "future-dated timestamps are treated as fresh, not stale"
        );
        assert_eq!(
            current_state_for_read(&stored, i64::MIN),
            SessionCurrentState::Working,
            "extreme negative now must not panic"
        );
    }

    #[test]
    fn transition_defensive_variants_return_prev_unchanged() {
        let prev = SessionState {
            current_state: SessionCurrentState::Working,
            last_event_kind: EventKind::PreToolUse,
            last_event_at_ms: 1_000,
            last_pid: None,
        };

        for kind in [
            EventKind::RecordingStarted,
            EventKind::RecordingEnded,
            EventKind::Unknown,
        ] {
            let next = t(Some(&prev), kind, 2_000);
            assert_eq!(next, prev);
        }
    }

    #[test]
    fn transition_defensive_variants_without_prev_default_to_idle() {
        for kind in [
            EventKind::RecordingStarted,
            EventKind::RecordingEnded,
            EventKind::Unknown,
        ] {
            let next = t(None, kind.clone(), 2_000);
            assert_eq!(next.current_state, SessionCurrentState::Idle);
            assert_eq!(next.last_event_kind, kind);
            assert_eq!(next.last_event_at_ms, 2_000);
            assert_eq!(next.last_pid, None);
        }
    }
}
