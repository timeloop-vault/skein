//! Silence recovery and the #423 supervisor repairs, plus the supervisor's
//! driver. Port of `app/src/harnessActivitySupervisor.ts` and
//! `app/src/supervisor/runtime.ts`. The invariants themselves are in
//! `invariants.rs`.

use super::invariants::{
    Effect, HarnessSupervisorState, LogLevel, Recovery, SupervisorEntry, SupervisorFacts,
    SupervisorSnapshot, Violation, format_effect, format_recovery_outcome,
};
use super::machine::PhaseMachine;
use super::types::{ActivityPhase, DegradedBy, TransitionSource};

fn log_line(level: LogLevel, message: &str) {
    match level {
        LogLevel::Info => tracing::info!(target: "skein::supervisor", "{message}"),
        LogLevel::Warn => tracing::warn!(target: "skein::supervisor", "{message}"),
        LogLevel::Error => tracing::error!(target: "skein::supervisor", "{message}"),
    }
}

impl PhaseMachine {
    /// #404: undo a #259 adapter-silent degrade that was actually caused
    /// by a mail nudge (or any #238 seam submit) whose own submit never
    /// landed: #388's settlement rolls that nudge back, but by then the
    /// watchdog has already stripped authority from a harness that was
    /// never given a real prompt to answer, and nothing else would ever
    /// call `adapter_delivered` for it again. `useMailDelivery.ts`'s
    /// rollback branch is the only caller, and only once it has also
    /// confirmed no human drove the harness in the meantime
    /// (`mailSettle.ts`'s `shouldRecoverSilence`): a keystroke since the
    /// nudge is evidence this diagnosis might be wrong.
    ///
    /// Recovers only when every one of these holds, deliberately as
    /// narrow as `note_launch_signal`'s own recovery guard, since restoring
    /// authority on weak evidence reintroduces exactly the false
    /// "watching" reading #259 exists to prevent:
    ///
    ///  - the harness still exists and isn't `exited` (nothing to
    ///    recover into) or `permission` (#86 outranks this everywhere
    ///    else this interaction shows up, and this is no exception);
    ///  - `adapter_silent && degraded_by == AdapterSilent`: the #259
    ///    watchdog is what fired, not #273's launch-silent timer. A
    ///    harness that degraded before it ever got a launch signal has no
    ///    "lost first paste" story to recover;
    ///  - `!adapter_heard`: the tail never spoke at all since spawn, so
    ///    nothing but this one lost prompt explains the silence. A tail
    ///    that DID speak and later went silent is a different, real
    ///    failure this must not paper over;
    ///  - `launch_signal_at` is set: the CLI proved it was alive at its
    ///    own prompt, the same proof `can_send_prompt` accepted to let the
    ///    nudge be sent in the first place;
    ///  - `!silence_recovered`: once per spawn. A second silent seam
    ///    submit on the same spawn, after a recovery already happened, is
    ///    real evidence the tail is watching the wrong file, not another
    ///    lost race, so it must stay degraded.
    ///
    /// Effect, mirroring `adapter_delivered`/`note_launch_signal`'s own
    /// recovery paths: `authoritative` back to `true`, `adapter_silent`
    /// and `degraded_by` cleared, plus `prompt_submitted_at` cleared so the
    /// very next seam submit re-arms the watchdog from a clean slate
    /// (rather than reading as already-armed against a timestamp from the
    /// lost attempt), and `silence_recovered` set so this can't fire twice.
    /// Moves phase to `waiting` when it isn't already there: the launch
    /// signal already proved the CLI is sitting at its own prompt.
    /// Returns whether it actually recovered, so the caller can log the
    /// outcome and gate a retry on it.
    ///
    /// "Recovered" means only that the gate is reopened (`authoritative`
    /// back to `true`), never that the harness is actually proven idle at
    /// its prompt. If the diagnosis is wrong (a broken tail, #362, that
    /// was never going to report anything regardless of whether the nudge
    /// landed) this lets one extra automatic nudge through per spawn,
    /// bounded by `silence_recovered` above, and `decideMailNudge` already
    /// dedupes on the live unread count once the agent actually reads its
    /// mail, so a wrong recovery costs at most one stray paste, not a
    /// runaway. That is the accepted trade-off against the alternative:
    /// a harness stuck permanently refused because its one real proof of
    /// life was the lost nudge this exists to recover.
    pub fn recover_unheard_silence(&mut self, id: &str, now_ms: i64) -> bool {
        let Some(cur) = self.store.get_mut(id) else {
            return false;
        };
        if cur.phase == ActivityPhase::Exited || cur.phase == ActivityPhase::Permission {
            return false;
        }
        if !cur.adapter_silent || cur.degraded_by != Some(DegradedBy::AdapterSilent) {
            return false;
        }
        if cur.adapter_heard || cur.launch_signal_at.is_none() {
            return false;
        }
        if cur.silence_recovered {
            return false;
        }
        tracing::info!(
            target: "skein::activity",
            "[skein] harness {id}: recovering from a lost first paste — the silence was never the adapter's fault (#404)"
        );
        cur.authoritative = true;
        cur.adapter_silent = false;
        cur.degraded_by = None;
        cur.prompt_submitted_at = None;
        cur.silence_recovered = true;
        if cur.phase != ActivityPhase::Waiting {
            self.set_phase(
                id,
                ActivityPhase::Waiting,
                TransitionSource::SilenceRecovered,
                now_ms,
            );
        }
        true
    }

    /// #423: the supervisor's authority repair, an adapter that is
    /// speaking again but was never handed authority back. Guards are
    /// re-checked here, at call time, not trusted from the caller's earlier
    /// read: the harness exists, authority is really gone, no watchdog
    /// diagnosis (`adapter_silent`) is standing (that recovery belongs to
    /// `adapter_delivered`) and it hasn't exited. Changes no phase. Returns
    /// whether it applied.
    pub fn supervisor_regrant_authority(&mut self, id: &str) -> bool {
        let Some(cur) = self.store.get_mut(id) else {
            return false;
        };
        if cur.authoritative || cur.adapter_silent || cur.phase == ActivityPhase::Exited {
            return false;
        }
        tracing::info!(
            target: "skein::activity",
            "[skein] harness {id}: supervisor restoring adapter authority (#423)"
        );
        cur.authoritative = true;
        true
    }

    /// #423: the supervisor's phase repair, a harness whose end of turn
    /// was seen but whose phase never followed. Only from `running` or
    /// `idle`, and never while a #277 deferral is armed (that harness is
    /// legitimately `running` until its subagents finish). `permission`,
    /// `exited`, `spawning` and `waiting` are left alone. Guards are
    /// re-checked at call time. Returns whether it applied.
    pub fn supervisor_set_waiting(&mut self, id: &str, now_ms: i64) -> bool {
        let Some(cur) = self.store.get(id) else {
            return false;
        };
        if cur.phase != ActivityPhase::Running && cur.phase != ActivityPhase::Idle {
            return false;
        }
        if cur.delegation_deferred_at.is_some() {
            return false;
        }
        self.set_phase(
            id,
            ActivityPhase::Waiting,
            TransitionSource::SupervisorRecovered,
            now_ms,
        );
        true
    }

    /// #423: the runtime around the pure supervisor (`runtime.ts`'s `tick`).
    /// The driver calls this after [`Self::tick`]. It snapshots every harness
    /// in the store, evaluates the six invariants, logs the effects and
    /// applies the two recoveries; it never writes to a PTY and never touches
    /// the mail or input seams. Returns every line the TS runtime would have
    /// logged, in order. Facts the machine cannot know (live attach, mail,
    /// transcript samples) come from `facts`.
    pub fn supervise(&mut self, facts: &SupervisorFacts, now_ms: i64) -> Vec<Violation> {
        let mut out = Vec::new();
        let live = self.ids();
        self.supervisor.entries.retain(|id, _| live.contains(id));

        let no_facts = super::invariants::HarnessFacts::default();
        for id in live {
            let Some(a) = self.get(&id).cloned() else {
                continue;
            };
            let mut entry = match self.supervisor.entries.remove(&id) {
                Some(e) if e.spawned_at == a.spawned_at => e,
                _ => SupervisorEntry {
                    spawned_at: a.spawned_at,
                    state: HarnessSupervisorState::default(),
                },
            };
            let f = facts.harnesses.get(&id).unwrap_or(&no_facts);
            let snapshot = SupervisorSnapshot {
                harness_id: id.clone(),
                phase: a.phase,
                phase_since: a.phase_since,
                last_output_at: a.last_output_at,
                authoritative: a.authoritative,
                adapter_silent: a.adapter_silent,
                live_attach: f.live_attach.unwrap_or(false),
                last_adapter_event: a.last_adapter_event,
                authority_lost_at: a.authority_lost_at,
                last_turn_signal: a.last_turn_signal,
                subagents_working: self.subagents.working_count(&id),
                background_working: self.background_tasks.working_count(&id),
                delegation_deferred_at: a.delegation_deferred_at,
                delegation_emptied_at: a.delegation_emptied_at,
                permission_at: a.permission_at,
                last_submit_at: a.last_submit_at,
                permission_agent_id: a.permission_agent_id.clone(),
                mail: f.mail.clone(),
                transcript: f.transcript.clone(),
            };

            let effects = entry.state.evaluate(&snapshot, now_ms);
            for effect in effects {
                let (level, message) = format_effect(&id, &effect);
                log_line(level, &message);
                let code = effect.code();
                let recover = match &effect {
                    Effect::Recover { recovery, .. } => Some(*recovery),
                    _ => None,
                };
                out.push(Violation {
                    harness_id: id.clone(),
                    code,
                    effect: Some(effect),
                    recovery_applied: None,
                    level,
                    message,
                });
                let Some(recovery) = recover else {
                    continue;
                };
                let applied = match recovery {
                    Recovery::RegrantAuthority => self.supervisor_regrant_authority(&id),
                    Recovery::SetWaiting => self.supervisor_set_waiting(&id, now_ms),
                };
                let (level, message) = format_recovery_outcome(&id, code, recovery, applied);
                log_line(level, &message);
                out.push(Violation {
                    harness_id: id.clone(),
                    code,
                    effect: None,
                    recovery_applied: Some(applied),
                    level,
                    message,
                });
            }
            self.supervisor.entries.insert(id, entry);
        }
        out
    }

    /// `supervisorViolations`: one harness's violation history, newest last.
    #[must_use]
    pub fn supervisor_violations(&self, id: &str) -> &[super::invariants::ViolationRecord] {
        self.supervisor.violations(id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness_phase::types::{TurnSignal, TurnSignalKind};

    #[test]
    fn ended_turn_not_waiting_repairs_to_waiting() {
        let mut m = PhaseMachine::new();
        m.spawned("h", 0);
        m.record_output("h", Some("x"), 100);
        m.store.get_mut("h").unwrap().last_turn_signal = Some(TurnSignal {
            kind: TurnSignalKind::End,
            at: 200,
        });
        let facts = SupervisorFacts::default();
        // First sight starts the grace clock; nothing is reported yet.
        assert_eq!(m.supervise(&facts, 20_000).len(), 0);
        let out = m.supervise(&facts, 30_000);
        assert!(out.iter().any(|v| v.recovery_applied == Some(true)));
        assert_eq!(m.get("h").unwrap().phase, ActivityPhase::Waiting);
        let last = m.drain_transitions().pop().unwrap();
        assert_eq!(last.source, TransitionSource::SupervisorRecovered);
    }
}
