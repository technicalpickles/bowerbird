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
///
/// As of Story 5.2 the canonical Working→Idle transition is `Stop`
/// (`PostToolUse` now preserves prior state — see `transition` below), so this
/// fallback's primary role is backstopping a dropped `Stop`. The original
/// role (backstopping a dropped `PostToolUse` in the pre-5.2 era) is still
/// covered by the same logic — the threshold doesn't care which specific event
/// was dropped, only that the session looks Working long past anything plausible.
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
        EventKind::UserPromptSubmit => SessionCurrentState::Working,
        EventKind::PreToolUse => SessionCurrentState::Working,
        // Story 5.2: PostToolUse no longer flips to Idle. The agent is alive
        // between tool calls (composing the next call, thinking) — the only
        // event that ends a turn is `Stop`. Preserve the prior `current_state`
        // and update `last_event_kind`/`last_event_at_ms`. The degenerate
        // PostToolUse-without-prior-state case defaults to Working (the agent
        // was clearly active a moment ago).
        EventKind::PostToolUse => prev
            .map(|s| s.current_state)
            .unwrap_or(SessionCurrentState::Working),
        EventKind::Stop => SessionCurrentState::Idle,
        EventKind::Notification => SessionCurrentState::WaitingInput,
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
    fn transition_posttooluse_preserves_working() {
        // Story 5.2: PostToolUse no longer flips to Idle. The session stays
        // Working between tool calls; only `Stop` ends a turn. `last_event_kind`
        // and `last_event_at_ms` still update so REST readers see freshness.
        let after_pre = transition(None, EventKind::PreToolUse, 1_000);
        let after_post = transition(Some(&after_pre), EventKind::PostToolUse, 2_000);
        assert_eq!(after_post.current_state, SessionCurrentState::Working);
        assert_eq!(after_post.last_event_kind, EventKind::PostToolUse);
        assert_eq!(after_post.last_event_at_ms, 2_000);
    }

    #[test]
    fn transition_posttooluse_without_prev_defaults_to_working() {
        // Degenerate path: a PostToolUse without a prior projection row.
        // Shouldn't happen in practice but Working is the right fallback —
        // the agent was clearly active a moment ago.
        let after_post = transition(None, EventKind::PostToolUse, 1_000);
        assert_eq!(after_post.current_state, SessionCurrentState::Working);
        assert_eq!(after_post.last_event_kind, EventKind::PostToolUse);
        assert_eq!(after_post.last_event_at_ms, 1_000);
    }

    #[test]
    fn transition_user_prompt_submit_yields_working() {
        let next = transition(None, EventKind::UserPromptSubmit, 1_000);
        assert_eq!(next.current_state, SessionCurrentState::Working);
        assert_eq!(next.last_event_kind, EventKind::UserPromptSubmit);
        assert_eq!(next.last_event_at_ms, 1_000);
    }

    #[test]
    fn transition_user_prompt_submit_then_pretooluse_stays_working() {
        let after_ups = transition(None, EventKind::UserPromptSubmit, 1_000);
        let after_pre = transition(Some(&after_ups), EventKind::PreToolUse, 2_000);
        assert_eq!(after_pre.current_state, SessionCurrentState::Working);
        assert_eq!(after_pre.last_event_kind, EventKind::PreToolUse);
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
