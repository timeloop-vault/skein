//! Ports of `harnessInput.test.ts` (send), `.submitRetry.test.ts`,
//! `.recovery.test.ts` and the send half of `.watchdog.test.ts`, against a
//! fake env and a manual clock.
#![allow(clippy::assert_is_empty)]

use super::*;
use crate::prompt_delivery::test_support::{Fake, fake};

const ID: &str = "h1";
const T0: i64 = 1_000_000;

fn manual() -> SendOpts {
    SendOpts::default()
}

fn auto() -> SendOpts {
    SendOpts {
        automatic: true,
        released_by_user: false,
    }
}

fn start(s: &mut Sender, e: &mut Fake, body: &str) -> SendStart {
    s.begin(e, ID, body, manual(), T0)
}

fn code(r: &SendStart) -> RefusalCode {
    match r {
        SendStart::Refused(r) => r.code,
        SendStart::Pasted => panic!("expected a refusal"),
    }
}

/// Advance to `ms` after T0 and run due timers.
fn at(s: &mut Sender, e: &mut Fake, ms: i64) -> Vec<SendEvent> {
    s.on_timer(e, T0 + ms)
}

// ---- harnessInput.test.ts ----

#[test]
fn pastes_immediately_then_submits_after_the_gap() {
    let (mut s, mut e) = (Sender::new(), fake(HarnessKind::Claude));
    assert_eq!(start(&mut s, &mut e, "do the thing"), SendStart::Pasted);
    assert_eq!(e.writes, vec![b"do the thing".to_vec()]);
    assert_eq!(s.next_deadline(), Some(T0 + SUBMIT_GAP_MS));
    assert!(at(&mut s, &mut e, SUBMIT_GAP_MS - 1).is_empty());
    let ev = at(&mut s, &mut e, SUBMIT_GAP_MS);
    assert_eq!(
        ev,
        vec![SendEvent::Submitted {
            harness_id: ID.into()
        }]
    );
    assert_eq!(e.writes, vec![b"do the thing".to_vec(), b"\r".to_vec()]);
    assert_eq!(e.submitted, 1);
}

#[test]
fn refuses_and_never_pastes_without_a_terminal() {
    let (mut s, mut e) = (Sender::new(), fake(HarnessKind::Claude));
    e.facts.pty_live = false;
    assert_eq!(code(&start(&mut s, &mut e, "x")), RefusalCode::NotReady);
    assert!(e.writes.is_empty());
    assert!(!s.in_flight(ID));
}

#[test]
fn rechecks_the_gate_at_call_time() {
    let (mut s, mut e) = (Sender::new(), fake(HarnessKind::Claude));
    e.set_phase(|p| p.phase = ActivityPhase::Running);
    assert_eq!(
        code(&start(&mut s, &mut e, "x")),
        RefusalCode::NotAtStoppingPoint
    );
    assert!(e.writes.is_empty());
}

#[test]
fn multi_line_body_is_one_bracketed_paste_then_one_submit() {
    let (mut s, mut e) = (Sender::new(), fake(HarnessKind::Opencode));
    e.facts.bracketed_paste = true;
    let body = (0..30)
        .map(|i| format!("line {i}"))
        .collect::<Vec<_>>()
        .join("\n");
    assert_eq!(start(&mut s, &mut e, &body), SendStart::Pasted);
    at(&mut s, &mut e, SUBMIT_GAP_MS);
    assert_eq!(e.writes.len(), 2);
    assert_eq!(e.writes[0], paste_bytes(&body, true));
    assert!(e.writes[0].starts_with(b"\x1b[200~"));
    assert_eq!(e.writes[1], b"\r");
}

#[test]
fn multi_line_body_without_paste_mode_is_refused() {
    let (mut s, mut e) = (Sender::new(), fake(HarnessKind::Claude));
    assert_eq!(
        code(&start(&mut s, &mut e, "a\nb")),
        RefusalCode::NeedsPasteMode
    );
}

#[test]
fn escape_stripped_multi_line_body_goes_out_bracketed() {
    let (mut s, mut e) = (Sender::new(), fake(HarnessKind::Claude));
    e.facts.bracketed_paste = true;
    assert_eq!(start(&mut s, &mut e, "a\x1b[201~\nb"), SendStart::Pasted);
    assert_eq!(e.writes[0], b"\x1b[200~a[201~\rb\x1b[201~");
}

// ---- new cases ----

#[test]
fn second_begin_is_refused_while_in_flight() {
    let (mut s, mut e) = (Sender::new(), fake(HarnessKind::Claude));
    assert_eq!(start(&mut s, &mut e, "one"), SendStart::Pasted);
    let r = start(&mut s, &mut e, "two");
    assert_eq!(code(&r), RefusalCode::Busy);
    let SendStart::Refused(r) = r else { panic!() };
    assert_eq!(r.reason, "a prompt is already being delivered");
    assert_eq!(e.writes.len(), 1);
    // Free again once the send has ended.
    s.forget(ID);
    assert_eq!(start(&mut s, &mut e, "two"), SendStart::Pasted);
}

#[test]
fn forget_mid_gap_writes_no_enter() {
    let (mut s, mut e) = (Sender::new(), fake(HarnessKind::Claude));
    start(&mut s, &mut e, "x");
    s.forget(ID);
    assert!(!s.in_flight(ID));
    assert_eq!(s.next_deadline(), None);
    assert!(at(&mut s, &mut e, 60_000).is_empty());
    assert_eq!(e.enters(), 0);
}

#[test]
fn automatic_send_refuses_a_held_draft_but_manual_and_released_do_not() {
    let (mut s, mut e) = (Sender::new(), fake(HarnessKind::Claude));
    e.draft = DraftState::Held {
        reason: "the user is composing".into(),
    };
    let r = s.begin(&mut e, ID, "x", auto(), T0);
    assert_eq!(code(&r), RefusalCode::DraftHeld);
    let SendStart::Refused(r) = r else { panic!() };
    assert_eq!(r.reason, "the user is composing");
    assert!(e.writes.is_empty());
    let released = SendOpts {
        automatic: true,
        released_by_user: true,
    };
    assert_eq!(s.begin(&mut e, ID, "x", released, T0), SendStart::Pasted);
    s.forget(ID);
    assert_eq!(s.begin(&mut e, ID, "x", manual(), T0), SendStart::Pasted);
}

#[test]
fn a_failed_paste_write_refuses_and_a_failed_submit_ends_the_send() {
    let (mut s, mut e) = (Sender::new(), fake(HarnessKind::Claude));
    e.write_ok = false;
    assert_eq!(code(&start(&mut s, &mut e, "x")), RefusalCode::NotReady);
    assert!(!s.in_flight(ID));
    e.write_ok = true;
    start(&mut s, &mut e, "x");
    e.write_ok = false;
    let ev = at(&mut s, &mut e, SUBMIT_GAP_MS);
    assert_eq!(
        ev,
        vec![
            SendEvent::WriteFailed {
                harness_id: ID.into()
            },
            SendEvent::Finished {
                harness_id: ID.into()
            }
        ]
    );
    assert_eq!(e.submitted, 0);
}

#[test]
fn respawn_or_user_input_in_the_gap_skips_the_submit() {
    let (mut s, mut e) = (Sender::new(), fake(HarnessKind::Claude));
    start(&mut s, &mut e, "x");
    e.token = Some("pty-2".into());
    let ev = at(&mut s, &mut e, SUBMIT_GAP_MS);
    assert!(matches!(ev[0], SendEvent::SubmitSkipped { .. }));
    assert_eq!(e.enters(), 0);

    e.token = Some("pty-1".into());
    start(&mut s, &mut e, "x");
    e.seq += 1;
    let ev = at(&mut s, &mut e, 2 * SUBMIT_GAP_MS + 1_000);
    assert!(matches!(ev[0], SendEvent::SubmitSkipped { .. }));
    assert_eq!(e.enters(), 0);
}

// ---- harnessInput.submitRetry.test.ts ----

const PAST: i64 = 8_000 + 5_000;

fn submit_and(s: &mut Sender, e: &mut Fake) {
    start(s, e, "hello");
    at(s, e, SUBMIT_GAP_MS);
    assert_eq!(e.enters(), 1);
}

#[test]
fn retries_at_every_scheduled_delay_three_total_never_a_fourth() {
    let (mut s, mut e) = (Sender::new(), fake(HarnessKind::Claude));
    submit_and(&mut s, &mut e);
    let sub = SUBMIT_GAP_MS;
    for (i, d) in SUBMIT_RETRY_SCHEDULE_MS.iter().enumerate() {
        assert_eq!(s.next_deadline(), Some(T0 + sub + d));
        at(&mut s, &mut e, sub + d - 1);
        assert_eq!(e.enters(), 1 + i);
        let ev = at(&mut s, &mut e, sub + d);
        assert_eq!(e.enters(), 2 + i);
        assert_eq!(
            ev[0],
            SendEvent::RetryEnter {
                harness_id: ID.into(),
                attempt: i + 1
            }
        );
    }
    assert!(!s.in_flight(ID));
    at(&mut s, &mut e, sub + PAST);
    assert_eq!(e.enters(), 4);
}

#[test]
fn stops_once_the_harness_has_left_waiting() {
    let (mut s, mut e) = (Sender::new(), fake(HarnessKind::Claude));
    submit_and(&mut s, &mut e);
    at(&mut s, &mut e, SUBMIT_GAP_MS + 2_000);
    assert_eq!(e.enters(), 2);
    s.on_transition(ID, ActivityPhase::Waiting, ActivityPhase::Running);
    // Back in waiting by the later attempt: still never retried.
    at(&mut s, &mut e, SUBMIT_GAP_MS + PAST);
    assert_eq!(e.enters(), 2);
    assert!(!s.in_flight(ID));
}

#[test]
fn no_enter_in_the_gap_for_a_post_exit_shell_or_a_dead_pty() {
    for dead in [false, true] {
        let (mut s, mut e) = (Sender::new(), fake(HarnessKind::Claude));
        start(&mut s, &mut e, "x");
        if dead {
            e.facts.pty_live = false;
        } else {
            e.facts.runs_its_tool = false;
        }
        let ev = at(&mut s, &mut e, SUBMIT_GAP_MS);
        assert!(matches!(ev[0], SendEvent::SubmitSkipped { .. }));
        assert_eq!(e.enters(), 0);
        assert!(!s.in_flight(ID));
    }
}

#[test]
fn no_retry_enter_for_a_post_exit_shell_or_a_dead_pty() {
    for dead in [false, true] {
        let (mut s, mut e) = (Sender::new(), fake(HarnessKind::Claude));
        submit_and(&mut s, &mut e);
        if dead {
            e.facts.pty_live = false;
        } else {
            e.facts.runs_its_tool = false;
        }
        let ev = at(&mut s, &mut e, SUBMIT_GAP_MS + SUBMIT_RETRY_SCHEDULE_MS[0]);
        assert!(matches!(ev[0], SendEvent::RetryStopped { .. }));
        assert_eq!(e.enters(), 1);
        assert!(!s.in_flight(ID));
    }
}

#[test]
fn a_transition_during_the_gap_is_left_to_the_gap_recheck() {
    let (mut s, mut e) = (Sender::new(), fake(HarnessKind::Claude));
    start(&mut s, &mut e, "x");
    s.on_transition(ID, ActivityPhase::Waiting, ActivityPhase::Running);
    e.set_phase(|p| p.phase = ActivityPhase::Waiting);
    at(&mut s, &mut e, SUBMIT_GAP_MS);
    assert_eq!(e.enters(), 1);
}

#[test]
fn stops_once_the_user_has_typed_mid_schedule() {
    let (mut s, mut e) = (Sender::new(), fake(HarnessKind::Claude));
    submit_and(&mut s, &mut e);
    at(&mut s, &mut e, SUBMIT_GAP_MS + 2_000);
    assert_eq!(e.enters(), 2);
    e.seq += 1;
    at(&mut s, &mut e, SUBMIT_GAP_MS + PAST);
    assert_eq!(e.enters(), 2);
}

#[test]
fn never_retries_for_a_kind_that_does_not_opt_in() {
    let (mut s, mut e) = (Sender::new(), fake(HarnessKind::Opencode));
    submit_and(&mut s, &mut e);
    assert!(!s.in_flight(ID));
    at(&mut s, &mut e, PAST);
    assert_eq!(e.enters(), 1);
}

#[test]
fn stops_retrying_once_nothing_is_watching() {
    let (mut s, mut e) = (Sender::new(), fake(HarnessKind::Claude));
    submit_and(&mut s, &mut e);
    e.set_phase(|p| p.authoritative = false);
    at(&mut s, &mut e, SUBMIT_GAP_MS + PAST);
    assert_eq!(e.enters(), 1);
    e.set_phase(|p| {
        p.authoritative = true;
        p.adapter_silent = true;
    });
}

#[test]
fn stops_when_the_terminal_respawned_mid_schedule() {
    let (mut s, mut e) = (Sender::new(), fake(HarnessKind::Claude));
    submit_and(&mut s, &mut e);
    e.token = Some("pty-2".into());
    let ev = at(&mut s, &mut e, SUBMIT_GAP_MS + PAST);
    assert_eq!(e.enters(), 1);
    assert!(matches!(ev[0], SendEvent::RetryStopped { .. }));
}

#[test]
fn no_submit_if_a_permission_dialog_opens_during_the_gap() {
    let (mut s, mut e) = (Sender::new(), fake(HarnessKind::Claude));
    start(&mut s, &mut e, "hello");
    e.set_phase(|p| p.phase = ActivityPhase::Permission);
    let ev = at(&mut s, &mut e, SUBMIT_GAP_MS);
    assert!(matches!(ev[0], SendEvent::SubmitSkipped { .. }));
    at(&mut s, &mut e, SUBMIT_GAP_MS + PAST);
    assert_eq!(e.enters(), 0);
    assert_eq!(e.submitted, 0);
}

#[test]
fn no_submit_if_the_phase_moves_to_running_during_the_gap() {
    let (mut s, mut e) = (Sender::new(), fake(HarnessKind::Claude));
    start(&mut s, &mut e, "hello");
    e.set_phase(|p| p.phase = ActivityPhase::Running);
    at(&mut s, &mut e, SUBMIT_GAP_MS + PAST);
    assert_eq!(e.enters(), 0);
}

#[test]
fn no_submit_if_the_activity_record_vanishes_during_the_gap() {
    let (mut s, mut e) = (Sender::new(), fake(HarnessKind::Claude));
    start(&mut s, &mut e, "hello");
    e.facts.phase = None;
    at(&mut s, &mut e, SUBMIT_GAP_MS);
    assert_eq!(e.enters(), 0);
}

// ---- #381: sent under a #277 delegation deferral ----

fn deferred() -> Fake {
    let mut e = fake(HarnessKind::Claude);
    e.set_phase(|p| {
        p.phase = ActivityPhase::Running;
        p.delegation_deferred_at = Some(T0 - 5_000);
    });
    e
}

#[test]
fn deferred_send_submits_after_the_gap_and_keeps_retrying_while_armed() {
    let (mut s, mut e) = (Sender::new(), deferred());
    assert_eq!(start(&mut s, &mut e, "hello"), SendStart::Pasted);
    assert_eq!(e.enters(), 0);
    at(&mut s, &mut e, SUBMIT_GAP_MS);
    assert_eq!(e.enters(), 1);
    at(&mut s, &mut e, SUBMIT_GAP_MS + 2_000);
    assert_eq!(e.enters(), 2);
}

#[test]
fn a_disarm_before_a_retry_stops_retries() {
    let (mut s, mut e) = (Sender::new(), deferred());
    start(&mut s, &mut e, "hello");
    at(&mut s, &mut e, SUBMIT_GAP_MS);
    e.set_phase(|p| p.delegation_deferred_at = None);
    at(&mut s, &mut e, SUBMIT_GAP_MS + PAST);
    assert_eq!(e.enters(), 1);
}

#[test]
fn a_re_arm_with_a_different_timestamp_stops_retries() {
    let (mut s, mut e) = (Sender::new(), deferred());
    start(&mut s, &mut e, "hello");
    at(&mut s, &mut e, SUBMIT_GAP_MS);
    e.set_phase(|p| p.delegation_deferred_at = Some(T0 + 1));
    at(&mut s, &mut e, SUBMIT_GAP_MS + PAST);
    assert_eq!(e.enters(), 1);
}

#[test]
fn no_submit_if_the_deferral_is_disarmed_during_the_gap() {
    let (mut s, mut e) = (Sender::new(), deferred());
    start(&mut s, &mut e, "hello");
    e.set_phase(|p| p.delegation_deferred_at = None);
    at(&mut s, &mut e, SUBMIT_GAP_MS + PAST);
    assert_eq!(e.enters(), 0);
}

#[test]
fn no_submit_if_the_deferral_changed_arm_during_the_gap() {
    let (mut s, mut e) = (Sender::new(), deferred());
    start(&mut s, &mut e, "hello");
    e.set_phase(|p| p.delegation_deferred_at = Some(T0));
    let ev = at(&mut s, &mut e, SUBMIT_GAP_MS);
    assert!(
        matches!(&ev[0], SendEvent::SubmitSkipped { why, .. } if why.contains("different delegation"))
    );
}

// ---- harnessInput.watchdog.test.ts (send half) ----

#[test]
fn prompt_submitted_is_noted_once_with_the_submit_after_the_gap() {
    let (mut s, mut e) = (Sender::new(), fake(HarnessKind::Claude));
    start(&mut s, &mut e, "do the thing");
    assert_eq!(e.submitted, 0);
    at(&mut s, &mut e, SUBMIT_GAP_MS);
    assert_eq!(e.submitted, 1);
    // Retries are Enters, not new submits.
    at(&mut s, &mut e, SUBMIT_GAP_MS + 2_000);
    assert_eq!(e.submitted, 1);
}

#[test]
fn a_permission_dialog_refuses_and_writes_nothing() {
    let (mut s, mut e) = (Sender::new(), fake(HarnessKind::Claude));
    e.set_phase(|p| p.phase = ActivityPhase::Permission);
    assert_eq!(
        code(&start(&mut s, &mut e, "anything")),
        RefusalCode::Permission
    );
    assert!(e.writes.is_empty());
    assert_eq!(e.submitted, 0);
}

// ---- harnessInput.recovery.test.ts (send half) ----

#[test]
fn a_degraded_harness_is_refused_until_the_daemon_recovers_it() {
    let (mut s, mut e) = (Sender::new(), fake(HarnessKind::Opencode));
    assert_eq!(start(&mut s, &mut e, "you have mail"), SendStart::Pasted);
    at(&mut s, &mut e, SUBMIT_GAP_MS);
    assert_eq!(e.enters(), 1);
    assert!(!s.in_flight(ID));

    e.set_phase(|p| {
        p.authoritative = false;
        p.adapter_silent = true;
    });
    let r = start(&mut s, &mut e, "you have mail");
    let SendStart::Refused(r) = r else { panic!() };
    assert_eq!(r.code, RefusalCode::NotWatched);
    assert_eq!(r.reason, "no confirmed adapter is watching this harness");

    // The mail layer asks for recovery; the Sender never does.
    assert_eq!(e.recovered, 0);
    e.recover_unheard_silence(ID);
    assert_eq!(start(&mut s, &mut e, "you have mail"), SendStart::Pasted);
    assert_eq!(
        e.writes
            .iter()
            .filter(|w| w.as_slice() == b"you have mail")
            .count(),
        2
    );
    assert_eq!(e.recovered, 1);
}

#[test]
fn due_timers_for_two_harnesses_run_in_one_call() {
    let (mut s, mut e) = (Sender::new(), fake(HarnessKind::Opencode));
    s.begin(&mut e, "a", "x", manual(), T0);
    s.begin(&mut e, "b", "y", manual(), T0 + 10);
    let ev = s.on_timer(&mut e, T0 + 1_000);
    assert_eq!(ev.len(), 4);
    assert_eq!(s.next_deadline(), None);
}
