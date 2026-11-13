//! Adapter work/end-of-turn signals and the #277/#446 deferral and
//! delegation counters. Port of `app/src/harnessActivityDeferral.ts`
//! (itself the #458 split of the store): an `impl PhaseMachine` block, time
//! injected as a trailing `now_ms`. The tick rules those arm live in
//! `work.rs`.

use super::machine::PhaseMachine;
use super::types::{ActivityPhase, TransitionSource, TurnSignal, TurnSignalKind};

impl PhaseMachine {
    /// L2c adapter says the harness is doing work. Bypasses the
    /// `record_output` throttle so the transition fires on the adapter
    /// event boundary, not on the next PTY chunk. The `source` identifies
    /// which adapter event drove the transition (e.g. `l2c1-claude-assistant`,
    /// `l2c2-opencode-busy`) and flows through to the persisted event log for
    /// L7 attribution.
    ///
    /// #86: a harness sitting in `permission` is hard-blocked on a dialog the
    /// user hasn't answered yet. Most adapter events that call this just mean
    /// "still working" (a fresh assistant turn, the next tool queued) and must
    /// not paper over that, so by default this is a no-op while in
    /// `permission`. Only a signal that genuinely means the gate is gone (a
    /// tool result landed, the user submitted a fresh prompt) should pass
    /// `clears_permission: true`; the translator at each call site decides
    /// which it is.
    pub fn set_running_from_adapter(
        &mut self,
        id: &str,
        source: TransitionSource,
        clears_permission: bool,
        now_ms: i64,
    ) {
        // #277 Rule 2: any main-transcript work signal disarms a deferred
        // end-of-turn, even one this call is about to no-op on below (still
        // `permission` without `clears_permission`): the disarm must not
        // depend on `set_phase` actually running.
        self.disarm_delegation(id);
        let Some(cur) = self.store.get_mut(id) else {
            return;
        };
        // #423: record what the adapter said before any guard can drop it.
        cur.last_turn_signal = Some(TurnSignal {
            kind: TurnSignalKind::Work,
            at: now_ms,
        });
        if cur.phase == ActivityPhase::Exited {
            return;
        }
        if cur.phase == ActivityPhase::Permission && !clears_permission {
            return;
        }
        self.set_phase(id, ActivityPhase::Running, source, now_ms);
    }

    /// L2c adapter says the harness is awaiting user input. Bypasses the
    /// chunk throttle for the same reason. Always allowed to leave
    /// `permission`: unlike `set_running_from_adapter`, every signal that
    /// calls this (Claude's end-of-turn, opencode's session-idle) means the
    /// harness is unambiguously done with whatever it was doing, permission
    /// gate included. #86.
    pub fn set_waiting_from_adapter(&mut self, id: &str, source: TransitionSource, now_ms: i64) {
        // #277 Rule 2: unconditional disarm, same reasoning as
        // `set_running_from_adapter`.
        self.disarm_delegation(id);
        let Some(cur) = self.store.get_mut(id) else {
            return;
        };
        // #423: see `set_running_from_adapter`.
        cur.last_turn_signal = Some(TurnSignal {
            kind: TurnSignalKind::End,
            at: now_ms,
        });
        if cur.phase == ActivityPhase::Exited {
            return;
        }
        self.set_phase(id, ActivityPhase::Waiting, source, now_ms);
    }

    /// #175: opencode's assumed-idle baseline after an SSE (re)connect. Same
    /// store effect as `set_waiting_from_adapter` (a turn that ended during an
    /// outage must still land in `waiting`) except that it is a guess, not an
    /// observed end of turn: its source is excluded from notifications
    /// (`isNotifiable`). It does leave `permission`: the reconnect already
    /// forgot the pending ids (a reconnect replays nothing), so nothing could
    /// ever clear the dialog otherwise.
    pub fn set_baseline_waiting_from_adapter(
        &mut self,
        id: &str,
        source: TransitionSource,
        now_ms: i64,
    ) {
        self.disarm_delegation(id);
        let Some(cur) = self.store.get_mut(id) else {
            return;
        };
        cur.last_turn_signal = Some(TurnSignal {
            kind: TurnSignalKind::End,
            at: now_ms,
        });
        if cur.phase == ActivityPhase::Exited {
            return;
        }
        self.set_phase(id, ActivityPhase::Waiting, source, now_ms);
    }

    /// #277 (epic #298): the Claude translator's `awaiting_prompt` arm calls
    /// this instead of `set_waiting_from_adapter` directly. The main
    /// transcript ended its turn, but that is a lie about the harness being
    /// done if it just delegated work still running in the background,
    /// measured true in 94% of 127 real sessions.
    ///
    /// No outstanding work (no working subagents or background tasks):
    /// identical to today, straight to `waiting`. One or more: arm the
    /// deferral (idempotent: a second end-of-turn while already deferred
    /// doesn't reset when it was armed) and change NO phase at all. The
    /// harness is already `running`, or `permission`, which outranks this
    /// regardless, so simply not calling `set_phase` leaves it alone either
    /// way. See the tick's Rules 3/4 (`work.rs`) for how a deferral eventually
    /// resolves without a further adapter event.
    pub fn awaiting_prompt_from_adapter(
        &mut self,
        id: &str,
        source: TransitionSource,
        now_ms: i64,
    ) {
        let outstanding = self.outstanding_work(id);
        let Some(cur) = self.store.get_mut(id) else {
            return;
        };
        // #423: an end of turn is recorded even when it only arms the
        // deferral and changes no phase.
        cur.last_turn_signal = Some(TurnSignal {
            kind: TurnSignalKind::End,
            at: now_ms,
        });
        if cur.phase == ActivityPhase::Exited {
            return;
        }
        if outstanding == 0 {
            self.set_waiting_from_adapter(id, source, now_ms);
            return;
        }
        if cur.delegation_deferred_at.is_none() {
            cur.delegation_deferred_at = Some(now_ms);
            // #381: notify arm-only listeners (the `delegation_armed` outbox).
            // Must sit inside this guard, not after it, so the idempotent
            // re-arm case (a second end-of-turn while already deferred) never
            // fires it twice.
            self.delegation_armed.push(id.to_owned());
        }
    }

    /// #634: a LIVE subagent or background task start can be flushed after
    /// the main session's `end_turn` already read as `waiting` with no work.
    /// Waiting while delegated work runs is the lie #277 exists to prevent,
    /// so re-enter the deferral: arm it and go back to `running`. Call AFTER
    /// the registry record so the new entry counts. Gated tightly: only an
    /// authoritative `waiting` that came from an end of turn.
    /// `last_turn_signal` is untouched so the next real end of turn or the
    /// settle flows normally. waiting -> running is never notifiable. Mail
    /// held (#381) may already have been delivered on the false waiting;
    /// harmless, deferral counts as deliverable, hence the listener fires.
    pub fn reclaim_delegation_from_adapter(&mut self, id: &str, now_ms: i64) {
        let Some(cur) = self.store.get(id) else {
            return;
        };
        if cur.phase != ActivityPhase::Waiting || !cur.authoritative {
            return;
        }
        if cur.last_turn_signal.map(|s| s.kind) != Some(TurnSignalKind::End)
            || cur.delegation_deferred_at.is_some()
        {
            return;
        }
        if self.outstanding_work(id) == 0 {
            return;
        }
        self.set_phase_with(
            id,
            ActivityPhase::Running,
            TransitionSource::DelegationResumed,
            move |a| {
                a.delegation_deferred_at = Some(now_ms);
                a.delegation_emptied_at = None;
            },
            now_ms,
        );
        self.delegation_armed.push(id.to_owned());
    }

    /// #277: a subagent started LIVE (never an attach-time replay: the
    /// translator only calls this for `event.initial != true`). Bumps
    /// `delegated_count` (what the notification wording counts) and
    /// `delegation_activity_at` (what the Rule 4 ceiling measures silence
    /// against). Silent mutation, no emit, the way `record_output` bumps
    /// `last_output_at`: subscribers must not re-render on subagent chatter,
    /// only on the harness's own phase.
    pub fn note_subagent_started(&mut self, id: &str, now_ms: i64) {
        let Some(cur) = self.store.get_mut(id) else {
            return;
        };
        cur.delegated_count += 1;
        cur.delegation_activity_at = now_ms;
    }

    /// #277: a subagent produced a tool result or reached its own end of
    /// turn. Bumps `delegation_activity_at` only: proof of life for the
    /// Rule 4 ceiling. Doesn't touch `delegated_count`: that counts starts,
    /// not activity. Silent mutation, no emit, same reasoning as
    /// `note_subagent_started`.
    pub fn note_subagent_activity(&mut self, id: &str, now_ms: i64) {
        let Some(cur) = self.store.get_mut(id) else {
            return;
        };
        cur.delegation_activity_at = now_ms;
    }

    /// #277: the user submitted a fresh prompt to this harness: resets
    /// `delegated_count` to 0, since it counts delegations since the LAST
    /// prompt. Silent mutation, no emit, same reasoning as
    /// `note_subagent_started`.
    pub fn note_user_prompt(&mut self, id: &str) {
        let Some(cur) = self.store.get_mut(id) else {
            return;
        };
        cur.delegated_count = 0;
    }
}
