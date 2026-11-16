//! #388/#404: when a pasted mail nudge counts as delivered, and when it is
//! rolled back. Ports of `settleNudge`, `shouldRecoverSilence` and
//! `evaluateSettlement` (`app/src/mailSettle.ts`). Pure.
//!
//! The evidence is phase evidence only (a turn started, the #277 deferral
//! arm changed). "The message was read" is deliberately NOT evidence.

use crate::harness_phase::ActivityPhase;

/// How long, from the paste, to wait for proof the submit landed. It
/// outlasts the #259 watchdog's latest firing (~11.15 s) and the submit
/// retry schedule's last attempt (8 s after the submit).
pub const NUDGE_SETTLE_MS: i64 = 13_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettleOutcome {
    Confirmed,
    Rollback,
    Wait,
}

/// A nudge that was pasted but not yet proven submitted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PendingSettlement {
    /// `lastNudged` from before the nudge, restored on rollback.
    pub pre_nudge: u32,
    pub sent_at_ms: i64,
    /// The #277 deferral arm the nudge was pasted under.
    pub deferred_at_send: Option<i64>,
    /// A `waiting` -> `running`/`permission` transition since the paste.
    pub turn_started: bool,
    /// The user-input count at paste time.
    pub input_seq_at_send: u64,
}

/// The activity fields the settlement reads, sampled fresh.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SettlementActivity {
    pub authoritative: bool,
    pub adapter_silent: bool,
    pub phase: ActivityPhase,
    pub delegation_deferred_at: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SettlementDecision {
    pub outcome: SettleOutcome,
    /// The value to write back to `lastNudged` on a rollback.
    pub restore_last_nudged: Option<u32>,
    pub attempt_recovery: bool,
    /// Still armed (`Wait`): a second automatic nudge is refused meanwhile.
    pub settlement_pending: bool,
}

/// Rules in order: the harness gone/respawned -> rollback; unwatched ->
/// rollback (outranks a turn start, which a degraded adapter's L2a fallback
/// can manufacture); turn started -> confirmed; the deferral arm changed ->
/// confirmed; under the deadline -> wait; else rollback.
#[must_use]
pub fn settle_nudge(
    elapsed_ms: i64,
    watched: bool,
    turn_started_since_send: bool,
    phase: Option<ActivityPhase>,
    deferred_at_send: Option<i64>,
    deferred_at_now: Option<i64>,
) -> SettleOutcome {
    if matches!(
        phase,
        None | Some(ActivityPhase::Exited | ActivityPhase::Spawning)
    ) || !watched
    {
        return SettleOutcome::Rollback;
    }
    if turn_started_since_send {
        return SettleOutcome::Confirmed;
    }
    if deferred_at_send.is_some() && deferred_at_now != deferred_at_send {
        return SettleOutcome::Confirmed;
    }
    if elapsed_ms < NUDGE_SETTLE_MS {
        return SettleOutcome::Wait;
    }
    SettleOutcome::Rollback
}

/// Recovery is attempted only for a rollback with no user input since the
/// paste.
#[must_use]
pub fn should_recover_silence(outcome: SettleOutcome, user_input_since_send: bool) -> bool {
    outcome == SettleOutcome::Rollback && !user_input_since_send
}

#[must_use]
pub fn evaluate_settlement(
    now_ms: i64,
    s: &PendingSettlement,
    activity: Option<SettlementActivity>,
    input_seq_now: u64,
) -> SettlementDecision {
    let watched = activity.is_some_and(|a| a.authoritative && !a.adapter_silent);
    let outcome = settle_nudge(
        now_ms - s.sent_at_ms,
        watched,
        s.turn_started,
        activity.map(|a| a.phase),
        s.deferred_at_send,
        activity.and_then(|a| a.delegation_deferred_at),
    );
    match outcome {
        SettleOutcome::Confirmed => SettlementDecision {
            outcome,
            restore_last_nudged: None,
            attempt_recovery: false,
            settlement_pending: false,
        },
        SettleOutcome::Rollback => SettlementDecision {
            outcome,
            restore_last_nudged: Some(s.pre_nudge),
            attempt_recovery: should_recover_silence(outcome, input_seq_now != s.input_seq_at_send),
            settlement_pending: false,
        },
        SettleOutcome::Wait => SettlementDecision {
            outcome,
            restore_last_nudged: None,
            attempt_recovery: false,
            settlement_pending: true,
        },
    }
}

#[cfg(test)]
mod tests;
