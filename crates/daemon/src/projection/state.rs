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

use protocol::{EventKind, SessionCurrentState, SessionState};

/// Read-time fallback window for stale `Working` states (5 minutes).
///
/// If `now_ms - last_event_at_ms > STALE_WORKING_MS` and the stored state is
/// `Working`, `current_state_for_read` returns `Idle`. The stored row is left
/// untouched — see Dev Notes "Hook unreliability mitigation".
pub(crate) const STALE_WORKING_MS: i64 = 300_000;

/// Compute the new `SessionState` to store after observing `event_kind`.
///
/// Pure function. Sentinel kinds (`RecordingStarted`, `RecordingEnded`) are
/// not expected to reach this function — `projection::session::write` is for
/// adapter-normalized events, and the sentinel write paths route through
/// `write_recording_started` / `write_recording_ended` directly. If a sentinel
/// is passed defensively, `prev` is returned unchanged (defaulting to `Idle`
/// when `prev` is `None`).
pub(crate) fn transition(
    prev: Option<&SessionState>,
    event_kind: EventKind,
    now_ms: i64,
) -> SessionState {
    let next_current = match event_kind {
        EventKind::PreToolUse => SessionCurrentState::Working,
        EventKind::PostToolUse => SessionCurrentState::Idle,
        EventKind::Stop => SessionCurrentState::Idle,
        EventKind::Notification => SessionCurrentState::WaitingInput,
        // Defensive guard — not an expected code path. Sentinels write to the
        // `__daemon__/__daemon__` row via their own helpers and never reach
        // this function. If they ever do, preserve prior state rather than
        // corrupting the projection.
        EventKind::RecordingStarted | EventKind::RecordingEnded => {
            return prev.cloned().unwrap_or(SessionState {
                current_state: SessionCurrentState::Idle,
                last_event_kind: event_kind,
                last_event_at_ms: now_ms,
            });
        }
    };

    SessionState {
        current_state: next_current,
        last_event_kind: event_kind,
        last_event_at_ms: now_ms,
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

    #[test]
    fn transition_first_event_pretooluse_yields_working() {
        let next = transition(None, EventKind::PreToolUse, 1_000);
        assert_eq!(next.current_state, SessionCurrentState::Working);
        assert_eq!(next.last_event_kind, EventKind::PreToolUse);
        assert_eq!(next.last_event_at_ms, 1_000);
    }

    #[test]
    fn transition_pretooluse_then_posttooluse_yields_idle() {
        let after_pre = transition(None, EventKind::PreToolUse, 1_000);
        let after_post = transition(Some(&after_pre), EventKind::PostToolUse, 2_000);
        assert_eq!(after_post.current_state, SessionCurrentState::Idle);
        assert_eq!(after_post.last_event_kind, EventKind::PostToolUse);
        assert_eq!(after_post.last_event_at_ms, 2_000);
    }

    #[test]
    fn transition_notification_yields_waiting_input() {
        let next = transition(None, EventKind::Notification, 5_000);
        assert_eq!(next.current_state, SessionCurrentState::WaitingInput);
    }

    #[test]
    fn transition_stop_clears_working() {
        let after_pre = transition(None, EventKind::PreToolUse, 1_000);
        let after_stop = transition(Some(&after_pre), EventKind::Stop, 2_000);
        assert_eq!(after_stop.current_state, SessionCurrentState::Idle);
        assert_eq!(after_stop.last_event_kind, EventKind::Stop);
    }

    #[test]
    fn transition_pretooluse_without_posttooluse_keeps_working() {
        // The storage-level state stays Working — the stale fallback lives in
        // `current_state_for_read`, not here.
        let next = transition(None, EventKind::PreToolUse, 1_000);
        assert_eq!(next.current_state, SessionCurrentState::Working);
    }

    #[test]
    fn current_state_for_read_returns_working_when_fresh() {
        let stored = SessionState {
            current_state: SessionCurrentState::Working,
            last_event_kind: EventKind::PreToolUse,
            last_event_at_ms: 1_000,
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
        };
        let now = 1_000 + STALE_WORKING_MS + 1;
        assert_eq!(
            current_state_for_read(&stored, now),
            SessionCurrentState::Idle
        );
    }

    #[test]
    fn current_state_for_read_returns_idle_at_exactly_threshold() {
        // Boundary: strict `>` means at exactly `STALE_WORKING_MS` we stay Working.
        let stored = SessionState {
            current_state: SessionCurrentState::Working,
            last_event_kind: EventKind::PreToolUse,
            last_event_at_ms: 1_000,
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
        };
        let now = i64::MAX / 2;
        assert_eq!(
            current_state_for_read(&stored, now),
            SessionCurrentState::WaitingInput
        );
    }

    #[test]
    fn current_state_for_read_does_not_panic_on_future_timestamp() {
        // Clock skew or corrupted persisted data can leave a stored timestamp
        // greater than `now_ms`. The read path must tolerate it without
        // overflowing — see `saturating_sub` rationale in source.
        let stored = SessionState {
            current_state: SessionCurrentState::Working,
            last_event_kind: EventKind::PreToolUse,
            last_event_at_ms: i64::MAX,
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
    fn transition_recording_started_returns_prev_unchanged() {
        let prev = SessionState {
            current_state: SessionCurrentState::Working,
            last_event_kind: EventKind::PreToolUse,
            last_event_at_ms: 1_000,
        };
        let next = transition(Some(&prev), EventKind::RecordingStarted, 2_000);
        assert_eq!(next, prev);
    }
}
