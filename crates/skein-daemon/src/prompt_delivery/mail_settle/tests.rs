//! Port of `mailSettle.test.ts`.

use super::*;
use ActivityPhase::{Exited, Idle, Permission, Running, Spawning, Waiting};
use SettleOutcome::{Confirmed, Rollback, Wait};

struct Inp {
    elapsed: i64,
    watched: bool,
    turn: bool,
    phase: Option<ActivityPhase>,
    deferred_send: Option<i64>,
    deferred_now: Option<i64>,
}

impl Default for Inp {
    fn default() -> Self {
        Self {
            elapsed: 0,
            watched: true,
            turn: false,
            phase: Some(Waiting),
            deferred_send: None,
            deferred_now: None,
        }
    }
}

fn settle(over: impl FnOnce(&mut Inp)) -> SettleOutcome {
    let mut i = Inp::default();
    over(&mut i);
    settle_nudge(
        i.elapsed,
        i.watched,
        i.turn,
        i.phase,
        i.deferred_send,
        i.deferred_now,
    )
}

#[test]
fn waits_until_the_deadline_then_rolls_back_at_exactly_the_deadline() {
    assert_eq!(settle(|_| {}), Wait);
    assert_eq!(settle(|i| i.elapsed = NUDGE_SETTLE_MS - 1), Wait);
    assert_eq!(settle(|i| i.elapsed = NUDGE_SETTLE_MS), Rollback);
    assert_eq!(settle(|i| i.elapsed = NUDGE_SETTLE_MS + 1_000), Rollback);
}

#[test]
fn confirms_on_a_turn_start_even_at_zero_elapsed_or_past_the_deadline() {
    assert_eq!(settle(|i| i.turn = true), Confirmed);
    assert_eq!(
        settle(|i| {
            i.turn = true;
            i.deferred_send = Some(100);
            i.deferred_now = Some(100);
            i.elapsed = NUDGE_SETTLE_MS + 1;
        }),
        Confirmed
    );
}

#[test]
fn confirms_when_the_deferral_arm_changed_or_disarmed() {
    assert_eq!(
        settle(|i| {
            i.elapsed = 500;
            i.deferred_send = Some(100);
            i.deferred_now = Some(200);
        }),
        Confirmed
    );
    assert_eq!(
        settle(|i| {
            i.elapsed = 500;
            i.deferred_send = Some(100);
            i.deferred_now = None;
        }),
        Confirmed
    );
}

#[test]
fn an_unchanged_or_absent_arm_is_not_proof() {
    assert_eq!(
        settle(|i| {
            i.elapsed = 500;
            i.deferred_send = Some(100);
            i.deferred_now = Some(100);
        }),
        Wait
    );
    assert_eq!(settle(|i| i.elapsed = 500), Wait);
}

#[test]
fn a_gone_or_respawned_harness_beats_every_other_rule() {
    for phase in [None, Some(Exited), Some(Spawning)] {
        assert_eq!(
            settle(|i| {
                i.phase = phase;
                i.turn = true;
                i.deferred_send = Some(100);
                i.deferred_now = Some(200);
            }),
            Rollback
        );
    }
}

#[test]
fn unwatched_beats_a_turn_start_and_an_arm_change_and_never_waits() {
    assert_eq!(
        settle(|i| {
            i.watched = false;
            i.turn = true;
        }),
        Rollback
    );
    assert_eq!(
        settle(|i| {
            i.watched = false;
            i.deferred_send = Some(100);
            i.deferred_now = Some(200);
        }),
        Rollback
    );
    assert_eq!(settle(|i| i.watched = false), Rollback);
}

#[test]
fn still_waits_in_running_permission_and_idle_with_no_evidence() {
    for p in [Running, Permission, Idle] {
        assert_eq!(settle(|i| i.phase = Some(p)), Wait);
    }
}

#[test]
fn recovery_only_for_a_rollback_without_user_input() {
    assert!(should_recover_silence(Rollback, false));
    assert!(!should_recover_silence(Rollback, true));
    assert!(!should_recover_silence(Confirmed, false));
    assert!(!should_recover_silence(Wait, false));
}

fn watched() -> SettlementActivity {
    SettlementActivity {
        authoritative: true,
        adapter_silent: false,
        phase: Waiting,
        delegation_deferred_at: None,
    }
}

fn pending(turn: bool) -> PendingSettlement {
    PendingSettlement {
        pre_nudge: 3,
        sent_at_ms: 0,
        deferred_at_send: None,
        turn_started: turn,
        input_seq_at_send: 1,
    }
}

#[test]
fn evaluate_rollback_restores_pre_nudge_and_recovers_without_input() {
    let r = evaluate_settlement(NUDGE_SETTLE_MS + 1, &pending(false), Some(watched()), 1);
    assert_eq!(r.outcome, Rollback);
    assert_eq!(r.restore_last_nudged, Some(3));
    assert!(r.attempt_recovery);
    assert!(!r.settlement_pending);
}

#[test]
fn evaluate_rollback_with_user_input_does_not_recover() {
    let r = evaluate_settlement(NUDGE_SETTLE_MS + 1, &pending(false), Some(watched()), 2);
    assert_eq!(r.outcome, Rollback);
    assert_eq!(r.restore_last_nudged, Some(3));
    assert!(!r.attempt_recovery);
}

#[test]
fn evaluate_confirmed_and_wait() {
    let c = evaluate_settlement(500, &pending(true), Some(watched()), 1);
    assert_eq!(c.outcome, Confirmed);
    assert_eq!(c.restore_last_nudged, None);
    assert!(!c.attempt_recovery && !c.settlement_pending);
    let w = evaluate_settlement(500, &pending(false), Some(watched()), 1);
    assert_eq!(w.outcome, Wait);
    assert_eq!(w.restore_last_nudged, None);
    assert!(!w.attempt_recovery && w.settlement_pending);
}

#[test]
fn evaluate_no_activity_rolls_back() {
    let r = evaluate_settlement(500, &pending(false), None, 1);
    assert_eq!(r.outcome, Rollback);
    assert_eq!(r.restore_last_nudged, Some(3));
}

#[test]
fn evaluate_404_unwatched_with_a_turn_still_rolls_back() {
    let a = SettlementActivity {
        adapter_silent: true,
        ..watched()
    };
    let r = evaluate_settlement(500, &pending(true), Some(a), 1);
    assert_eq!(r.outcome, Rollback);
    assert_eq!(r.restore_last_nudged, Some(3));
}
