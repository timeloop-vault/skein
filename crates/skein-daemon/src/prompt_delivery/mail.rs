//! Automatic mail delivery (#329, #381, #383, #386, #388, #404, #413): the
//! per-harness state and decisions of `useMailDelivery.ts` /
//! `mailDeliveryCheck.ts`, as a pure, clock-injected layer over the
//! [`Sender`]. No threads and no clock of its own: [`DeliveryCore`]
//! (`core.rs`) routes events in and the driver calls `on_timer` at
//! `next_deadline`. Every entry point takes `&mut self`, so the TS
//! `runSerialized` ordering is automatic.
//!
//! [`DeliveryCore`]: super::core::DeliveryCore

use std::collections::BTreeMap;

use super::env::{DeliveryEnv, DraftState};
use super::gate::{Refusal, RefusalCode, at_safe_stopping_point, can_send};
use super::mail_nudge::{
    decide_mail_nudge, mail_held, mail_nudge_text, mail_pending, release_refusal,
    should_check_on_transition,
};
use super::mail_retry::{MailRetry, next_mail_retry};
use super::mail_settle::{
    NUDGE_SETTLE_MS, PendingSettlement, SettleOutcome, SettlementActivity, evaluate_settlement,
};
use super::send::{SendEvent, SendOpts, SendStart, Sender};
use crate::harness_phase::ActivityPhase;

/// What triggered a check: an event re-arms the retry window from now, a
/// retry tick keeps the window it was already on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Source {
    Event,
    Timer,
}

#[derive(Debug, Clone, Copy)]
struct Retry {
    armed_at: i64,
    due: i64,
}

#[derive(Debug, Default)]
struct MailState {
    /// Unread count as of the last nudge actually sent. 0 = never nudged.
    last_nudged: u32,
    settle: Option<PendingSettlement>,
    retry: Option<Retry>,
    held: bool,
    /// The last logged refusal reason, so a 2 s retry doesn't repeat it.
    last_refusal: Option<String>,
}

#[derive(Debug, Default)]
pub struct Mail {
    states: BTreeMap<String, MailState>,
}

fn refusal(code: RefusalCode, reason: &str) -> Refusal {
    Refusal {
        code,
        reason: reason.to_string(),
    }
}

fn set_held(st: &mut MailState, env: &mut dyn DeliveryEnv, id: &str, held: bool) {
    if st.held != held {
        st.held = held;
        env.publish_held(id, held);
    }
}

fn log_refusal(st: &mut MailState, id: &str, r: &Refusal) {
    if st.last_refusal.as_deref() == Some(r.reason.as_str()) {
        return;
    }
    st.last_refusal = Some(r.reason.clone());
    tracing::info!(harness_id = id, reason = %r.reason, "mail nudge refused (#404, #413)");
}

/// #388: settle a still-pending nudge BEFORE reading the live unread count,
/// so a rollback lands before the decision compares against `last_nudged`.
/// Returns whether a nudge is still settling.
fn settle_pending(
    st: &mut MailState,
    env: &mut dyn DeliveryEnv,
    id: &str,
    activity: Option<SettlementActivity>,
    now_ms: i64,
) -> bool {
    let Some(s) = st.settle else {
        return false;
    };
    let d = evaluate_settlement(now_ms, &s, activity, env.user_input_seq(id));
    match d.outcome {
        SettleOutcome::Confirmed => {
            st.settle = None;
            tracing::info!(harness_id = id, "mail nudge confirmed delivered (#388)");
        }
        SettleOutcome::Rollback => {
            if let Some(v) = d.restore_last_nudged {
                st.last_nudged = v;
            }
            st.settle = None;
            tracing::info!(
                harness_id = id,
                "rolling back mail nudge: no proof the submit landed (#388)"
            );
            // #404: the rollback may itself be why a #259 adapter-silent
            // degrade fired; recover it unless a human drove the harness.
            if d.attempt_recovery {
                env.recover_unheard_silence(id);
                tracing::info!(
                    harness_id = id,
                    "asked to recover a lost first paste (#404)"
                );
            }
        }
        SettleOutcome::Wait => {}
    }
    d.settlement_pending
}

impl Mail {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// A new process or a removed harness: drop every bit of state (a
    /// respawned harness starts at `last_nudged = 0`) and clear the marker.
    pub fn reset(&mut self, env: &mut dyn DeliveryEnv, id: &str) {
        if let Some(st) = self.states.remove(id)
            && st.held
        {
            env.publish_held(id, false);
        }
    }

    /// A phase transition: note a turn start on a pending settlement, drop
    /// the timers of an exited harness, then check when it is worth it.
    pub fn on_transition(
        &mut self,
        sender: &mut Sender,
        env: &mut dyn DeliveryEnv,
        id: &str,
        from: ActivityPhase,
        to: ActivityPhase,
        now_ms: i64,
    ) {
        if let Some(st) = self.states.get_mut(id) {
            if let Some(s) = st.settle.as_mut()
                && from == ActivityPhase::Waiting
                && matches!(to, ActivityPhase::Running | ActivityPhase::Permission)
            {
                s.turn_started = true;
            }
            if to == ActivityPhase::Exited {
                st.retry = None;
                st.settle = None;
            }
        }
        let at_point_now = env
            .gate_facts(id, now_ms)
            .phase
            .is_some_and(|p| at_safe_stopping_point(&p));
        if should_check_on_transition(to, at_point_now) {
            self.automatic(sender, env, id, now_ms);
        }
    }

    /// An event-triggered automatic check.
    pub fn automatic(
        &mut self,
        sender: &mut Sender,
        env: &mut dyn DeliveryEnv,
        id: &str,
        now_ms: i64,
    ) {
        let _ = self.run(sender, env, id, false, Source::Event, now_ms);
    }

    /// The user released held mail (#413): the same attempt with only the
    /// draft guard skipped. `Err` says why nothing was sent.
    ///
    /// # Errors
    /// The refusal that stopped the delivery.
    pub fn deliver_now(
        &mut self,
        sender: &mut Sender,
        env: &mut dyn DeliveryEnv,
        id: &str,
        now_ms: i64,
    ) -> Result<(), Refusal> {
        let r = self.run(sender, env, id, true, Source::Event, now_ms);
        if let Err(e) = &r {
            tracing::info!(harness_id = id, reason = %e.reason, "deliver-now refused (#413)");
        }
        r
    }

    /// Run every settlement deadline and retry tick that is due.
    pub fn on_timer(&mut self, sender: &mut Sender, env: &mut dyn DeliveryEnv, now_ms: i64) {
        let due: Vec<(String, Source)> = self
            .states
            .iter()
            .filter_map(|(id, st)| {
                let settle_due = st
                    .settle
                    .is_some_and(|s| s.sent_at_ms + NUDGE_SETTLE_MS <= now_ms);
                let retry_due = st.retry.is_some_and(|r| r.due <= now_ms);
                // The settlement timer re-runs as an event (re-arms the
                // retry window); a retry tick keeps its window.
                if settle_due {
                    Some((id.clone(), Source::Event))
                } else if retry_due {
                    Some((id.clone(), Source::Timer))
                } else {
                    None
                }
            })
            .collect();
        for (id, source) in due {
            let _ = self.run(sender, env, &id, false, source, now_ms);
        }
    }

    #[must_use]
    pub fn next_deadline(&self) -> Option<i64> {
        self.states
            .values()
            .flat_map(|st| {
                [
                    st.settle.map(|s| s.sent_at_ms + NUDGE_SETTLE_MS),
                    st.retry.map(|r| r.due),
                ]
            })
            .flatten()
            .min()
    }

    /// What a finished send says about a pending settlement. Settlement
    /// stays phase-evidence only (#388), so this only logs.
    pub fn on_send_event(&self, ev: &SendEvent) {
        let (SendEvent::SubmitSkipped { harness_id, .. }
        | SendEvent::WriteFailed { harness_id }
        | SendEvent::RetryStopped { harness_id, .. }) = ev
        else {
            return;
        };
        if self
            .states
            .get(harness_id)
            .is_some_and(|st| st.settle.is_some())
        {
            tracing::info!(harness_id, event = ?ev, "a nudge's send ended early; settling by phase (#388)");
        }
    }

    /// One serialized check plus the #386 retry re-arm.
    fn run(
        &mut self,
        sender: &mut Sender,
        env: &mut dyn DeliveryEnv,
        id: &str,
        released: bool,
        source: Source,
        now_ms: i64,
    ) -> Result<(), Refusal> {
        let (pending, sent) = self.check(sender, env, id, released, now_ms);
        // No state was made for a harness `check` could not even look at.
        let Some(st) = self.states.get_mut(id) else {
            return sent;
        };
        let prior = st.retry.take();
        let armed_at = match source {
            Source::Event => now_ms,
            Source::Timer => prior.map_or(now_ms, |r| r.armed_at),
        };
        let phase = env.gate_facts(id, now_ms).phase.map(|p| p.phase);
        if let MailRetry::Wait { delay_ms } = next_mail_retry(pending, phase, armed_at, now_ms) {
            st.retry = Some(Retry {
                armed_at,
                due: now_ms + delay_ms,
            });
        }
        sent
    }

    /// `checkMail`: returns whether mail is still pending, and for a
    /// release whether it was sent.
    #[allow(clippy::too_many_lines)]
    fn check(
        &mut self,
        sender: &mut Sender,
        env: &mut dyn DeliveryEnv,
        id: &str,
        released: bool,
        now_ms: i64,
    ) -> (bool, Result<(), Refusal>) {
        // A harness with no state yet and nothing to look at (no unread
        // lookup, no phase record) gets none: a stray id must not leak.
        if !self.states.contains_key(id) {
            if env.unread(id).is_none() {
                tracing::warn!(harness_id = id, "mail unread lookup failed");
                return (
                    true,
                    Err(refusal(RefusalCode::NotReady, "unread mail lookup failed")),
                );
            }
            if env.gate_facts(id, now_ms).phase.is_none() {
                return (
                    true,
                    Err(refusal(
                        RefusalCode::NotReady,
                        "no activity record for this harness",
                    )),
                );
            }
        }
        let st = self.states.entry(id.to_string()).or_default();
        let activity = |env: &dyn DeliveryEnv| {
            env.gate_facts(id, now_ms)
                .phase
                .map(|p| SettlementActivity {
                    authoritative: p.authoritative,
                    adapter_silent: p.adapter_silent,
                    phase: p.phase,
                    delegation_deferred_at: p.delegation_deferred_at,
                })
        };
        let act = activity(env);
        let settling = settle_pending(st, env, id, act, now_ms);
        let Some(unread) = env.unread(id) else {
            tracing::warn!(harness_id = id, "mail unread lookup failed");
            return (
                true,
                Err(refusal(RefusalCode::NotReady, "unread mail lookup failed")),
            );
        };
        // Read after a possible recovery: the gate sees the recovered record.
        let facts = env.gate_facts(id, now_ms);
        let Some(p) = facts.phase else {
            return (
                true,
                Err(refusal(
                    RefusalCode::NotReady,
                    "no activity record for this harness",
                )),
            );
        };
        let count = unread.count;
        let body = mail_nudge_text(count, &unread.from);
        // #388: a settlement still pending refuses outright, before even
        // consulting the gate or the draft.
        let gate: Result<(), Refusal> = if settling {
            Err(refusal(
                RefusalCode::Busy,
                "a previous nudge is still settling (#388)",
            ))
        } else {
            can_send(&facts, &body).and_then(|()| match env.draft(id) {
                DraftState::Held { reason } if !released => Err(Refusal {
                    code: RefusalCode::DraftHeld,
                    reason,
                }),
                _ => Ok(()),
            })
        };
        let last_nudged = st.last_nudged;
        let at_point = at_safe_stopping_point(&p);
        let d = decide_mail_nudge(at_point, count, last_nudged, gate.is_ok());
        if !d.nudge {
            st.last_nudged = d.last_nudged;
            if !released {
                let held = mail_held(count, d.last_nudged, gate.as_ref().err());
                set_held(st, env, id, held);
            }
            let pending = mail_pending(count, d.last_nudged);
            if let Err(g) = &gate
                && pending
            {
                log_refusal(st, id, g);
            }
            if !pending {
                st.last_refusal = None;
            }
            let why = release_refusal(at_point, count, d.last_nudged, gate.as_ref().err());
            return (pending, Err(why));
        }
        tracing::info!(
            harness_id = id,
            unread = count,
            last_nudged,
            "nudging (#404)"
        );
        let opts = SendOpts {
            automatic: true,
            released_by_user: released,
        };
        match sender.begin(env, id, &body, opts, now_ms) {
            // The send re-checks the gate and can lose a race: keep the
            // pre-nudge value so the next arrival or check tries again.
            SendStart::Refused(r) => {
                st.last_nudged = last_nudged;
                log_refusal(st, id, &r);
                if !released {
                    set_held(st, env, id, mail_held(count, last_nudged, Some(&r)));
                }
                let pending = mail_pending(count, last_nudged);
                if !pending {
                    st.last_refusal = None;
                }
                (pending, Err(r))
            }
            // #388: pasted is not delivered. Record it provisionally and
            // arm a settlement that confirms or rolls it back.
            SendStart::Pasted => {
                set_held(st, env, id, false);
                st.last_refusal = None;
                st.last_nudged = d.last_nudged;
                let pre_nudge = st
                    .settle
                    .map_or(last_nudged, |s| s.pre_nudge.min(last_nudged));
                st.settle = Some(PendingSettlement {
                    pre_nudge,
                    sent_at_ms: now_ms,
                    deferred_at_send: p.delegation_deferred_at,
                    turn_started: false,
                    input_seq_at_send: env.user_input_seq(id),
                });
                (mail_pending(count, d.last_nudged), Ok(()))
            }
        }
    }
}

#[cfg(test)]
mod tests;
