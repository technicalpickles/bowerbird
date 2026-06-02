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
/// `cwd` (Story 5.7) follows the SAME carry-forward / overwrite-on-Some shape
/// as `last_pid` (a `String`, so it clones). `started_at` is the INVERSE:
/// set-once / keep-earliest — the FIRST event a session projects sets it to
/// `now_ms`, every later event preserves it. Both are independent of the
/// state machine; no `current_state` arm reads them.
///
/// Sentinel kinds (`RecordingStarted`, `RecordingEnded`) are not expected to
/// reach this function — sentinel write paths route directly through their own
/// helpers. If a sentinel is passed defensively, prev is returned unchanged
/// (with `last_pid` / `cwd` carry-forwarded and `started_at` kept-earliest from
/// the overlay).
pub(crate) fn transition(
    prev: Option<&SessionState>,
    event_kind: EventKind,
    notification_type: Option<NotificationType>,
    pid: Option<u32>,
    cwd: Option<String>,
    now_ms: i64,
) -> SessionState {
    // Carry-forward / overwrite-on-Some semantics for last_pid. Applied to
    // EVERY arm — including defensive arms — because last_pid is independent
    // of the state-machine logic. Story 5.3 AC #5/#6.
    let next_last_pid = pid.or(prev.and_then(|s| s.last_pid));

    // Story 5.7. `cwd` carry-forward mirrors `last_pid` (overwrite-on-Some).
    // `started_at` is set-once / keep-earliest — the prior value wins, falling
    // back to the current event's clock only when there is no prior. Anti-
    // footgun: do NOT write `started_at` with cwd's `.or(prev...)` direction —
    // that would reset start time to every event's clock.
    let next_cwd = cwd.or_else(|| prev.and_then(|s| s.cwd.clone()));
    let next_started_at = prev.and_then(|s| s.started_at).or(Some(now_ms));

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
        // Story 5.3 AC #7/#8, reworked by Story 5.6 / ADR 0005: Notification
        // branches on typed notification_type into THREE rules.
        //   PermissionPrompt | ElicitationDialog → WaitingInput (hard block)
        //   IdlePrompt → Idle, EXCEPT prior WaitingInput is preserved
        //   AuthSuccess | ElicitationResponse | ElicitationComplete | Unknown | None → preserve prior (but Ended → Idle)
        //
        // IdlePrompt (Story 5.6 / ADR 0005, refined by code-review D3): the idle
        // nudge fires ~60s after a turn ends, so its arrival is positive
        // evidence the turn is OVER — Claude does not ping idle mid-work. So it
        // resolves to `Idle`, which also covers a dropped `Stop` (a finished
        // session whose `Stop` was lost still lands on `Idle` instead of pinning
        // to a stale `Working` that the idle nudge would otherwise keep
        // refreshing past the read-time stale-`Working` fallback). The ONE
        // exception: a prior `WaitingInput` (a still-pending permission /
        // elicitation block) is preserved — an idle nudge neither creates nor
        // clears a real block. So IdlePrompt never *transitions a session into*
        // WaitingInput; PermissionPrompt and ElicitationDialog (incl.
        // AskUserQuestion) are the ONLY types that do.
        //
        // Truly-transient types (AuthSuccess / ElicitationResponse /
        // ElicitationComplete / Unknown / None) preserve prior `current_state`,
        // EXCEPT a prior `Ended` resurrects to `Idle`: a hook arriving at all is
        // evidence the process is alive (ADR 0004 non-terminal `Ended`;
        // code-review D1). (No prior → Idle too.)
        //
        // All three rules still update last_event_kind / last_event_at_ms (the
        // event happened) — only `current_state` follows the rule above.
        EventKind::Notification => match notification_type {
            Some(NotificationType::PermissionPrompt)
            | Some(NotificationType::ElicitationDialog) => SessionCurrentState::WaitingInput,
            Some(NotificationType::IdlePrompt) => match prev.map(|s| s.current_state) {
                Some(SessionCurrentState::WaitingInput) => SessionCurrentState::WaitingInput,
                _ => SessionCurrentState::Idle,
            },
            Some(NotificationType::AuthSuccess)
            | Some(NotificationType::ElicitationResponse)
            | Some(NotificationType::ElicitationComplete)
            | Some(NotificationType::Unknown)
            | None => match prev.map(|s| s.current_state) {
                Some(SessionCurrentState::Ended) | None => SessionCurrentState::Idle,
                Some(other) => other,
            },
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
            // `match` (not `.map().unwrap_or()`) so the owned `next_cwd`
            // String moves into exactly one arm — the borrow checker rejects
            // moving it into both a map closure and an unwrap_or value.
            return match prev.cloned() {
                Some(s) => SessionState {
                    last_pid: next_last_pid,
                    cwd: next_cwd,
                    started_at: next_started_at,
                    ..s
                },
                None => SessionState {
                    current_state: SessionCurrentState::Idle,
                    last_event_kind: event_kind,
                    last_event_at_ms: now_ms,
                    last_pid: next_last_pid,
                    cwd: next_cwd,
                    started_at: next_started_at,
                },
            };
        }
    };

    SessionState {
        current_state: next_current,
        last_event_kind: event_kind,
        last_event_at_ms: now_ms,
        last_pid: next_last_pid,
        cwd: next_cwd,
        started_at: next_started_at,
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
        transition(prev, kind, None, None, None, now_ms)
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
            cwd: None,
            started_at: None,
        };
        let after = t(Some(&prev_idle), EventKind::PostToolUse, 2_000);
        assert_eq!(after.current_state, SessionCurrentState::Working);

        // Prior WaitingInput → Working (the load-bearing case for AC #9).
        let prev_wi = SessionState {
            current_state: SessionCurrentState::WaitingInput,
            last_event_kind: EventKind::Notification,
            last_event_at_ms: 0,
            last_pid: None,
            cwd: None,
            started_at: None,
        };
        let after = t(Some(&prev_wi), EventKind::PostToolUse, 3_000);
        assert_eq!(after.current_state, SessionCurrentState::Working);

        // Prior Ended → Working (resume case from Ended).
        let prev_ended = SessionState {
            current_state: SessionCurrentState::Ended,
            last_event_kind: EventKind::SessionEnded,
            last_event_at_ms: 0,
            last_pid: Some(100),
            cwd: None,
            started_at: None,
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

    // Story 5.3 AC #7 (narrowed by Story 5.6 / ADR 0005): the input-required
    // notification_type values trigger WaitingInput; the prior state is
    // irrelevant. As of Story 5.6 only PermissionPrompt and ElicitationDialog
    // are input-required — IdlePrompt moved to the transient bucket.
    #[test]
    fn transition_notification_input_required_yields_waiting_input() {
        for nt in [
            NotificationType::PermissionPrompt,
            NotificationType::ElicitationDialog,
        ] {
            let next = transition(None, EventKind::Notification, Some(nt), None, None, 5_000);
            assert_eq!(
                next.current_state,
                SessionCurrentState::WaitingInput,
                "notification_type {nt:?} must transition to WaitingInput"
            );
        }
    }

    // Story 5.3 AC #8: the truly-transient notification_type values + Unknown +
    // None preserve the prior current_state but still update
    // last_event_kind/last_event_at_ms. NOTE: IdlePrompt is NOT in this set as
    // of Story 5.6 / ADR 0005 code-review D3 — it has its own rule (→ Idle
    // unless prior WaitingInput), tested separately below.
    #[test]
    fn transition_notification_transient_preserves_prior() {
        let prev = SessionState {
            current_state: SessionCurrentState::Working,
            last_event_kind: EventKind::PreToolUse,
            last_event_at_ms: 1_000,
            last_pid: Some(99),
            cwd: None,
            started_at: None,
        };
        let cases: &[Option<NotificationType>] = &[
            Some(NotificationType::AuthSuccess),
            Some(NotificationType::ElicitationResponse),
            Some(NotificationType::ElicitationComplete),
            Some(NotificationType::Unknown),
            None,
        ];
        for nt in cases {
            let next = transition(Some(&prev), EventKind::Notification, *nt, None, None, 2_000);
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
            None,
            5_000,
        );
        assert_eq!(next.current_state, SessionCurrentState::Idle);
        assert_eq!(next.last_event_kind, EventKind::Notification);
    }

    // Story 5.6 / ADR 0005: the common idle-nudge-after-Stop path. A turn ends
    // (Stop → Idle); ~60s later Claude emits an idle_prompt; the session must
    // stay Idle (the deck's WaitingInput wall must drain), and the event still
    // updates last_event_kind/last_event_at_ms.
    #[test]
    fn transition_notification_idle_prompt_prior_idle_yields_idle() {
        let prev = SessionState {
            current_state: SessionCurrentState::Idle,
            last_event_kind: EventKind::Stop,
            last_event_at_ms: 1_000,
            last_pid: Some(42),
            cwd: None,
            started_at: None,
        };
        let next = transition(
            Some(&prev),
            EventKind::Notification,
            Some(NotificationType::IdlePrompt),
            None,
            None,
            2_000,
        );
        assert_eq!(
            next.current_state,
            SessionCurrentState::Idle,
            "idle_prompt after a normal turn-end must read as Idle, not WaitingInput"
        );
        assert_eq!(
            next.last_event_kind,
            EventKind::Notification,
            "the idle_prompt event still happened — last_event_kind must update"
        );
        assert_eq!(next.last_event_at_ms, 2_000);
    }

    // Story 5.6 / ADR 0005 code-review D3: the dropped-Stop case. A session is
    // Working (mid-turn) and its `Stop` hook is dropped; ~60s later an
    // idle_prompt arrives. The idle nudge is positive evidence the turn ended,
    // so the session must resolve to Idle — NOT preserve a stale Working that
    // the nudge's timestamp refresh would otherwise keep alive past the
    // read-time stale-Working fallback.
    #[test]
    fn transition_notification_idle_prompt_prior_working_yields_idle() {
        let prev = SessionState {
            current_state: SessionCurrentState::Working,
            last_event_kind: EventKind::PreToolUse,
            last_event_at_ms: 1_000,
            last_pid: Some(42),
            cwd: None,
            started_at: None,
        };
        let next = transition(
            Some(&prev),
            EventKind::Notification,
            Some(NotificationType::IdlePrompt),
            None,
            None,
            2_000,
        );
        assert_eq!(
            next.current_state,
            SessionCurrentState::Idle,
            "idle_prompt after a dropped Stop must resolve a stale Working to Idle"
        );
        assert_eq!(next.last_event_kind, EventKind::Notification);
        assert_eq!(next.last_event_at_ms, 2_000);
    }

    // Story 5.6 / ADR 0005: the load-bearing "don't clobber a real block" case.
    // A genuine permission_prompt left the session in WaitingInput; the user
    // then sat idle long enough to trigger an idle nudge. idle_prompt resolves
    // to Idle in general (D3), but a prior WaitingInput is the ONE exception —
    // it is preserved so the nudge does not mask a real pending block.
    #[test]
    fn transition_notification_idle_prompt_prior_waiting_input_preserved() {
        let prev = SessionState {
            current_state: SessionCurrentState::WaitingInput,
            last_event_kind: EventKind::Notification,
            last_event_at_ms: 1_000,
            last_pid: Some(7),
            cwd: None,
            started_at: None,
        };
        let next = transition(
            Some(&prev),
            EventKind::Notification,
            Some(NotificationType::IdlePrompt),
            None,
            None,
            2_000,
        );
        assert_eq!(
            next.current_state,
            SessionCurrentState::WaitingInput,
            "an idle nudge must not clobber a still-pending permission/elicitation block"
        );
        assert_eq!(next.last_event_kind, EventKind::Notification);
        assert_eq!(next.last_event_at_ms, 2_000);
    }

    // Story 5.6 / ADR 0005: idle_prompt with no prior defaults to Idle (via the
    // existing `.unwrap_or(Idle)` in the preserve-prior branch).
    #[test]
    fn transition_notification_idle_prompt_without_prev_defaults_to_idle() {
        let next = transition(
            None,
            EventKind::Notification,
            Some(NotificationType::IdlePrompt),
            None,
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
            cwd: None,
            started_at: None,
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
            cwd: None,
            started_at: None,
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
            None,
            3_000,
        );
        assert_eq!(after_n.current_state, SessionCurrentState::WaitingInput);
    }

    // Story 5.6 / ADR 0005: a preserve-prior notification arriving for an
    // `Ended` session is evidence the process is alive (Claude fired the hook),
    // so it must resurrect the session to `Idle` rather than preserve `Ended` —
    // honoring ADR 0004's non-terminal-`Ended` contract. Without this, a stray
    // `idle_prompt` after the liveness probe marked a session `Ended` would
    // leave it hidden, and because `current_state` would not change, no
    // `state.session.*` frame would be emitted. Covers every preserve-prior
    // type (idle_prompt joining the branch is what made this reachable via the
    // most common stray hook).
    #[test]
    fn transition_from_ended_preserve_prior_notification_yields_idle() {
        let ended = SessionState {
            current_state: SessionCurrentState::Ended,
            last_event_kind: EventKind::SessionEnded,
            last_event_at_ms: 0,
            last_pid: Some(7),
            cwd: None,
            started_at: None,
        };
        for nt in [
            NotificationType::IdlePrompt,
            NotificationType::AuthSuccess,
            NotificationType::ElicitationResponse,
            NotificationType::ElicitationComplete,
            NotificationType::Unknown,
        ] {
            let next = transition(
                Some(&ended),
                EventKind::Notification,
                Some(nt),
                None,
                None,
                5_000,
            );
            assert_eq!(
                next.current_state,
                SessionCurrentState::Idle,
                "Ended + {nt:?} must resurrect to Idle, not preserve Ended"
            );
            assert_eq!(next.last_event_kind, EventKind::Notification);
            assert_eq!(next.last_event_at_ms, 5_000);
        }
        // A `None` notification_type from Ended also resurrects to Idle.
        let after_none = transition(
            Some(&ended),
            EventKind::Notification,
            None,
            None,
            None,
            5_000,
        );
        assert_eq!(after_none.current_state, SessionCurrentState::Idle);
    }

    // Story 5.3 AC #5/#6: last_pid carry-forward + overwrite-on-Some.
    #[test]
    fn transition_carry_forward_last_pid() {
        // prev None + envelope None → None
        let n1 = transition(None, EventKind::PreToolUse, None, None, None, 1_000);
        assert_eq!(n1.last_pid, None);

        // prev None + envelope Some(100) → Some(100)
        let n2 = transition(None, EventKind::PreToolUse, None, Some(100), None, 1_000);
        assert_eq!(n2.last_pid, Some(100));

        // prev Some(100) + envelope None → carry forward
        let prev = SessionState {
            current_state: SessionCurrentState::Idle,
            last_event_kind: EventKind::Stop,
            last_event_at_ms: 0,
            last_pid: Some(100),
            cwd: None,
            started_at: None,
        };
        let n3 = transition(Some(&prev), EventKind::PreToolUse, None, None, None, 1_000);
        assert_eq!(n3.last_pid, Some(100));

        // prev Some(100) + envelope Some(200) → overwrite
        let n4 = transition(
            Some(&prev),
            EventKind::PreToolUse,
            None,
            Some(200),
            None,
            1_000,
        );
        assert_eq!(n4.last_pid, Some(200));
    }

    // Story 5.7 AC #4/#5: cwd carry-forward + overwrite-on-Some (mirrors
    // last_pid exactly; the only difference is `String` so it clones).
    #[test]
    fn transition_carry_forward_cwd() {
        let a = || Some("/Users/x/repo-a".to_string());
        let b = || Some("/Users/x/repo-b".to_string());

        // prev None + envelope None → None
        let n1 = transition(None, EventKind::PreToolUse, None, None, None, 1_000);
        assert_eq!(n1.cwd, None);

        // prev None + envelope Some(a) → Some(a)
        let n2 = transition(None, EventKind::PreToolUse, None, None, a(), 1_000);
        assert_eq!(n2.cwd, a());

        let prev = SessionState {
            current_state: SessionCurrentState::Idle,
            last_event_kind: EventKind::Stop,
            last_event_at_ms: 0,
            last_pid: None,
            cwd: a(),
            started_at: Some(1),
        };

        // prev Some(a) + envelope None → carry forward Some(a)
        let n3 = transition(Some(&prev), EventKind::PreToolUse, None, None, None, 2_000);
        assert_eq!(n3.cwd, a());

        // prev Some(a) + envelope Some(b) → overwrite Some(b)
        let n4 = transition(Some(&prev), EventKind::PreToolUse, None, None, b(), 2_000);
        assert_eq!(n4.cwd, b());
    }

    // Story 5.7 AC #12: started_at is set-once / keep-earliest — the INVERSE of
    // cwd. The first event sets it to now_ms; later events preserve it even as
    // now_ms advances. State-independent (holds across event kinds). Regression
    // guard against the cwd-vs-started_at direction footgun.
    #[test]
    fn transition_set_once_started_at() {
        // First event for a session (prev None) sets started_at = now_ms.
        for kind in [
            EventKind::UserPromptSubmit,
            EventKind::PreToolUse,
            EventKind::PostToolUse,
            EventKind::Stop,
        ] {
            let first = transition(None, kind.clone(), None, None, None, 1_000);
            assert_eq!(
                first.started_at,
                Some(1_000),
                "first event ({kind:?}) must set started_at to its own clock"
            );

            // A later event (prev Some(t0)) with a larger now_ms preserves t0.
            let later = transition(Some(&first), kind.clone(), None, None, None, 9_999);
            assert_eq!(
                later.started_at,
                Some(1_000),
                "later event ({kind:?}) must preserve the earliest started_at, not adopt now_ms"
            );
        }

        // Notification path too (set-once is independent of the state machine).
        let n_first = transition(
            None,
            EventKind::Notification,
            Some(NotificationType::PermissionPrompt),
            None,
            None,
            2_000,
        );
        assert_eq!(n_first.started_at, Some(2_000));
        let n_later = transition(
            Some(&n_first),
            EventKind::Notification,
            Some(NotificationType::IdlePrompt),
            None,
            None,
            5_000,
        );
        assert_eq!(n_later.started_at, Some(2_000));
    }

    #[test]
    fn current_state_for_read_returns_working_when_fresh() {
        let stored = SessionState {
            current_state: SessionCurrentState::Working,
            last_event_kind: EventKind::PreToolUse,
            last_event_at_ms: 1_000,
            last_pid: None,
            cwd: None,
            started_at: None,
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
            cwd: None,
            started_at: None,
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
            cwd: None,
            started_at: None,
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
            cwd: None,
            started_at: None,
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
            cwd: None,
            started_at: None,
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
            cwd: None,
            started_at: None,
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
            cwd: None,
            started_at: None,
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
        // `started_at` must be Some here: a defensive event carries the prior
        // forward (set-once / keep-earliest), so a None prior would be filled
        // in with `now_ms` and break the `next == prev` equality. cwd: Some
        // exercises the carry-forward too.
        let prev = SessionState {
            current_state: SessionCurrentState::Working,
            last_event_kind: EventKind::PreToolUse,
            last_event_at_ms: 1_000,
            last_pid: None,
            cwd: Some("/Users/x/repo".to_string()),
            started_at: Some(500),
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
