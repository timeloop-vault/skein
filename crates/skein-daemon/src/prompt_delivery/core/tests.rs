//! Scenario tests for [`DeliveryCore`] against the shared fake env and a
//! manual clock; each asserts the exact bytes written.

#![allow(clippy::assert_is_empty)]

use super::*;
use crate::prompt_delivery::env::{DraftState, UnreadMail};
use crate::prompt_delivery::gate::RefusalCode;
use crate::prompt_delivery::mail_nudge::mail_nudge_text;
use crate::prompt_delivery::mail_retry::MAIL_RETRY_WINDOW_MS;
use crate::prompt_delivery::mail_settle::NUDGE_SETTLE_MS;
use crate::prompt_delivery::submit_retry::SUBMIT_GAP_MS;
use crate::prompt_delivery::test_support::{Fake, T0, fake};
use skein_proto::harness_kind::HarnessKind;

const ID: &str = "h1";

fn nudge(n: u32) -> Vec<u8> {
    mail_nudge_text(n, &[]).into_bytes()
}

fn mail(e: &mut Fake, n: u32) {
    e.unread = Some(UnreadMail {
        count: n,
        from: Vec::new(),
    });
}

/// A waiting opencode harness (no Enter retries) with one unread message.
fn setup() -> (DeliveryCore, Fake) {
    let mut e = fake(HarnessKind::Opencode);
    mail(&mut e, 1);
    (DeliveryCore::new(), e)
}

fn ev_mail() -> DeliveryEvent {
    DeliveryEvent::MailChanged {
        harness_id: ID.into(),
    }
}

fn ev(f: fn(String) -> DeliveryEvent) -> DeliveryEvent {
    f(ID.to_string())
}

fn transition(from: ActivityPhase, to: ActivityPhase) -> DeliveryEvent {
    DeliveryEvent::Transition {
        harness_id: ID.into(),
        from,
        to,
    }
}

fn handle(c: &mut DeliveryCore, e: &mut Fake, ev: DeliveryEvent, ms: i64) -> Reply {
    c.handle(e, ev, T0 + ms)
}

fn timer(c: &mut DeliveryCore, e: &mut Fake, ms: i64) {
    c.on_timer(e, T0 + ms);
}

fn pastes(e: &Fake, n: u32) -> usize {
    e.writes.iter().filter(|w| **w == nudge(n)).count()
}

/// Run every deadline up to `until_ms` after T0, as the worker would.
fn drain(c: &mut DeliveryCore, e: &mut Fake, until_ms: i64) {
    while let Some(d) = c.next_deadline() {
        if d > T0 + until_ms {
            break;
        }
        c.on_timer(e, d);
    }
}

fn phase(e: &mut Fake, p: ActivityPhase) {
    e.set_phase(|f| f.phase = p);
}

#[test]
fn mail_while_waiting_pastes_once_then_one_enter_and_a_repeat_writes_nothing() {
    let (mut c, mut e) = setup();
    handle(&mut c, &mut e, ev_mail(), 0);
    assert_eq!(e.writes, vec![nudge(1)]);
    assert_eq!(c.next_deadline(), Some(T0 + SUBMIT_GAP_MS));
    timer(&mut c, &mut e, SUBMIT_GAP_MS);
    assert_eq!(e.writes, vec![nudge(1), b"\r".to_vec()]);
    assert_eq!(e.submitted, 1);
    handle(&mut c, &mut e, ev_mail(), 500);
    handle(
        &mut c,
        &mut e,
        ev(|h| DeliveryEvent::ClientFactsChanged { harness_id: h }),
        600,
    );
    assert_eq!(e.writes, vec![nudge(1), b"\r".to_vec()]);
    assert_eq!(e.held_log, Vec::<bool>::new());
}

#[test]
fn mail_while_running_waits_for_the_transition_to_waiting() {
    let (mut c, mut e) = setup();
    phase(&mut e, ActivityPhase::Running);
    handle(&mut c, &mut e, ev_mail(), 0);
    assert!(e.writes.is_empty());
    phase(&mut e, ActivityPhase::Waiting);
    handle(
        &mut c,
        &mut e,
        transition(ActivityPhase::Running, ActivityPhase::Waiting),
        1_000,
    );
    assert_eq!(e.writes, vec![nudge(1)]);
    timer(&mut c, &mut e, 1_000 + SUBMIT_GAP_MS);
    assert_eq!(e.writes, vec![nudge(1), b"\r".to_vec()]);
    handle(
        &mut c,
        &mut e,
        transition(ActivityPhase::Running, ActivityPhase::Waiting),
        1_300,
    );
    assert_eq!(e.writes.len(), 2);
}

#[test]
fn a_transition_that_is_not_a_stopping_point_checks_nothing() {
    let (mut c, mut e) = setup();
    phase(&mut e, ActivityPhase::Running);
    handle(
        &mut c,
        &mut e,
        transition(ActivityPhase::Waiting, ActivityPhase::Running),
        0,
    );
    assert!(e.writes.is_empty());
    assert_eq!(c.next_deadline(), None);
}

#[test]
fn permission_question_and_post_exit_shell_are_never_written_even_across_retries() {
    let cases: [fn(&mut Fake); 3] = [
        |e| phase(e, ActivityPhase::Permission),
        |e| e.set_phase(|p| p.question_open = true),
        |e| e.facts.runs_its_tool = false,
    ];
    for setup_case in cases {
        let (mut c, mut e) = setup();
        setup_case(&mut e);
        handle(&mut c, &mut e, ev_mail(), 0);
        // The 2 s retry runs for its whole 60 s window, then stops.
        assert_eq!(c.next_deadline(), Some(T0 + 2_000));
        drain(&mut c, &mut e, MAIL_RETRY_WINDOW_MS + 10_000);
        assert!(e.writes.is_empty());
        assert_eq!(c.next_deadline(), None);
    }
}

#[test]
fn running_with_a_delegation_deferral_armed_delivers() {
    let (mut c, mut e) = setup();
    phase(&mut e, ActivityPhase::Running);
    handle(&mut c, &mut e, ev_mail(), 0);
    assert!(e.writes.is_empty());
    e.set_phase(|p| p.delegation_deferred_at = Some(5));
    handle(
        &mut c,
        &mut e,
        ev(|h| DeliveryEvent::DeferralArmed { harness_id: h }),
        100,
    );
    assert_eq!(e.writes, vec![nudge(1)]);
    timer(&mut c, &mut e, 100 + SUBMIT_GAP_MS);
    assert_eq!(e.writes, vec![nudge(1), b"\r".to_vec()]);
}

#[test]
fn a_permission_to_running_transition_with_a_deferral_armed_delivers() {
    let (mut c, mut e) = setup();
    phase(&mut e, ActivityPhase::Permission);
    handle(&mut c, &mut e, ev_mail(), 0);
    phase(&mut e, ActivityPhase::Running);
    e.set_phase(|p| p.delegation_deferred_at = Some(5));
    handle(
        &mut c,
        &mut e,
        transition(ActivityPhase::Permission, ActivityPhase::Running),
        100,
    );
    assert_eq!(e.writes, vec![nudge(1)]);
}

#[test]
fn a_held_draft_publishes_held_once_and_a_clean_draft_delivers() {
    let (mut c, mut e) = setup();
    e.draft = DraftState::Held {
        reason: "typed".into(),
    };
    handle(&mut c, &mut e, ev_mail(), 0);
    handle(&mut c, &mut e, ev_mail(), 100);
    assert!(e.writes.is_empty());
    assert_eq!(e.held_log, vec![true]);
    e.draft = DraftState::Clean;
    handle(
        &mut c,
        &mut e,
        ev(|h| DeliveryEvent::ClientFactsChanged { harness_id: h }),
        200,
    );
    assert_eq!(e.writes, vec![nudge(1)]);
    assert_eq!(e.held_log, vec![true, false]);
}

#[test]
fn deliver_now_delivers_despite_a_held_draft() {
    let (mut c, mut e) = setup();
    e.draft = DraftState::Held {
        reason: "typed".into(),
    };
    handle(&mut c, &mut e, ev_mail(), 0);
    assert!(e.writes.is_empty());
    let r = handle(
        &mut c,
        &mut e,
        ev(|h| DeliveryEvent::DeliverNow { harness_id: h }),
        100,
    );
    assert_eq!(r, Reply::DeliverNow(Ok(())));
    assert_eq!(e.writes, vec![nudge(1)]);
    assert_eq!(e.held_log, vec![true, false]);
    timer(&mut c, &mut e, 100 + SUBMIT_GAP_MS);
    assert_eq!(e.writes, vec![nudge(1), b"\r".to_vec()]);
}

#[test]
fn deliver_now_names_why_nothing_was_sent() {
    let (mut c, mut e) = setup();
    mail(&mut e, 0);
    let r = handle(
        &mut c,
        &mut e,
        ev(|h| DeliveryEvent::DeliverNow { harness_id: h }),
        0,
    );
    let Reply::DeliverNow(Err(err)) = r else {
        panic!("expected a refusal, got {r:?}");
    };
    assert_eq!(err.code, RefusalCode::NothingToDeliver);
    mail(&mut e, 1);
    phase(&mut e, ActivityPhase::Running);
    let r = handle(
        &mut c,
        &mut e,
        ev(|h| DeliveryEvent::DeliverNow { harness_id: h }),
        10,
    );
    let Reply::DeliverNow(Err(err)) = r else {
        panic!("expected a refusal, got {r:?}");
    };
    assert_eq!(err.code, RefusalCode::NotAtStoppingPoint);
    assert!(e.writes.is_empty());
}

#[test]
fn a_turn_within_the_window_confirms_and_never_renudges() {
    let (mut c, mut e) = setup();
    handle(&mut c, &mut e, ev_mail(), 0);
    timer(&mut c, &mut e, SUBMIT_GAP_MS);
    phase(&mut e, ActivityPhase::Running);
    handle(
        &mut c,
        &mut e,
        transition(ActivityPhase::Waiting, ActivityPhase::Running),
        1_000,
    );
    phase(&mut e, ActivityPhase::Waiting);
    handle(
        &mut c,
        &mut e,
        transition(ActivityPhase::Running, ActivityPhase::Waiting),
        5_000,
    );
    drain(&mut c, &mut e, NUDGE_SETTLE_MS + 10_000);
    assert_eq!(e.writes, vec![nudge(1), b"\r".to_vec()]);
    assert_eq!(e.recovered, 0);
    assert_eq!(c.next_deadline(), None);
}

#[test]
fn no_turn_within_the_window_rolls_back_recovers_and_may_renudge() {
    let (mut c, mut e) = setup();
    handle(&mut c, &mut e, ev_mail(), 0);
    timer(&mut c, &mut e, SUBMIT_GAP_MS);
    assert_eq!(pastes(&e, 1), 1);
    // Still pending: a second automatic nudge is refused until it settles.
    mail(&mut e, 2);
    handle(&mut c, &mut e, ev_mail(), 5_000);
    assert_eq!(e.writes.len(), 2);
    timer(&mut c, &mut e, NUDGE_SETTLE_MS - 1);
    assert_eq!(e.writes.len(), 2);
    mail(&mut e, 1);
    timer(&mut c, &mut e, NUDGE_SETTLE_MS);
    // Rolled back (#404 recovery asked), and the same check re-nudges.
    assert_eq!(e.recovered, 1);
    assert_eq!(pastes(&e, 1), 2);
}

#[test]
fn rollback_with_user_input_does_not_recover_and_a_closed_gate_keeps_it_pending() {
    let (mut c, mut e) = setup();
    handle(&mut c, &mut e, ev_mail(), 0);
    timer(&mut c, &mut e, SUBMIT_GAP_MS);
    e.seq += 1; // the user typed: the gap-time recheck is over, settlement sees it
    phase(&mut e, ActivityPhase::Running);
    timer(&mut c, &mut e, NUDGE_SETTLE_MS);
    assert_eq!(e.recovered, 0);
    assert_eq!(e.writes.len(), 2);
    // Pending again: the retry ticks until the harness is back at waiting.
    assert!(c.next_deadline().is_some());
    phase(&mut e, ActivityPhase::Waiting);
    handle(
        &mut c,
        &mut e,
        transition(ActivityPhase::Running, ActivityPhase::Waiting),
        NUDGE_SETTLE_MS + 100,
    );
    assert_eq!(pastes(&e, 1), 2);
}

#[test]
fn spawned_resets_last_nudged_and_the_held_marker() {
    let (mut c, mut e) = setup();
    handle(&mut c, &mut e, ev_mail(), 0);
    assert_eq!(pastes(&e, 1), 1);
    handle(
        &mut c,
        &mut e,
        ev(|h| DeliveryEvent::Spawned { harness_id: h }),
        500,
    );
    // lastNudged was reset to 0, so the new process is told once more.
    assert_eq!(pastes(&e, 1), 2);

    let (mut c, mut e) = setup();
    e.draft = DraftState::Held {
        reason: "typed".into(),
    };
    handle(&mut c, &mut e, ev_mail(), 0);
    assert_eq!(e.held_log, vec![true]);
    e.draft = DraftState::Clean;
    handle(
        &mut c,
        &mut e,
        ev(|h| DeliveryEvent::Spawned { harness_id: h }),
        10,
    );
    assert_eq!(e.held_log, vec![true, false]);
    assert_eq!(e.writes, vec![nudge(1)]);
}

#[test]
fn spawning_into_a_not_ready_harness_retries_until_it_is_waiting() {
    let (mut c, mut e) = setup();
    phase(&mut e, ActivityPhase::Spawning);
    handle(
        &mut c,
        &mut e,
        ev(|h| DeliveryEvent::Spawned { harness_id: h }),
        0,
    );
    assert!(e.writes.is_empty());
    drain(&mut c, &mut e, 4_000);
    assert!(e.writes.is_empty());
    phase(&mut e, ActivityPhase::Waiting);
    drain(&mut c, &mut e, 6_000);
    assert_eq!(e.writes.first(), Some(&nudge(1)));
}

#[test]
fn forgotten_drops_the_state_and_an_unsent_submit() {
    let (mut c, mut e) = setup();
    handle(&mut c, &mut e, ev_mail(), 0);
    handle(
        &mut c,
        &mut e,
        ev(|h| DeliveryEvent::Forgotten { harness_id: h }),
        50,
    );
    assert_eq!(c.next_deadline(), None);
    timer(&mut c, &mut e, SUBMIT_GAP_MS);
    assert_eq!(e.writes, vec![nudge(1)]);
}

#[test]
fn an_exited_harness_stops_its_timers() {
    let (mut c, mut e) = setup();
    phase(&mut e, ActivityPhase::Running);
    handle(&mut c, &mut e, ev_mail(), 0);
    assert!(c.next_deadline().is_some());
    phase(&mut e, ActivityPhase::Exited);
    handle(
        &mut c,
        &mut e,
        transition(ActivityPhase::Running, ActivityPhase::Exited),
        100,
    );
    assert_eq!(c.next_deadline(), None);
    assert!(e.writes.is_empty());
}

#[test]
fn an_unknown_mailbox_writes_nothing() {
    let (mut c, mut e) = setup();
    e.unread = None;
    handle(&mut c, &mut e, ev_mail(), 0);
    assert!(e.writes.is_empty());
}

#[test]
fn manual_replies_at_paste_time_and_skips_the_draft_check() {
    let (mut c, mut e) = setup();
    e.draft = DraftState::Held {
        reason: "typed".into(),
    };
    let manual = |body: &str| DeliveryEvent::Manual {
        harness_id: ID.into(),
        body: body.into(),
    };
    assert_eq!(
        handle(&mut c, &mut e, manual("review please"), 0),
        Reply::Manual(Ok(()))
    );
    assert_eq!(e.writes, vec![b"review please".to_vec()]);
    timer(&mut c, &mut e, SUBMIT_GAP_MS);
    assert_eq!(e.writes, vec![b"review please".to_vec(), b"\r".to_vec()]);
    // No hold, no retry, no settlement for a manual send.
    assert_eq!(e.held_log, Vec::<bool>::new());
    assert_eq!(c.next_deadline(), None);
}

#[test]
fn manual_is_refused_off_a_stopping_point_and_while_a_send_is_in_flight() {
    let (mut c, mut e) = setup();
    phase(&mut e, ActivityPhase::Running);
    let manual = DeliveryEvent::Manual {
        harness_id: ID.into(),
        body: "x".into(),
    };
    let Reply::Manual(Err(r)) = handle(&mut c, &mut e, manual.clone(), 0) else {
        panic!("expected a refusal");
    };
    assert_eq!(r.code, RefusalCode::NotAtStoppingPoint);
    phase(&mut e, ActivityPhase::Waiting);
    assert_eq!(
        handle(&mut c, &mut e, manual.clone(), 10),
        Reply::Manual(Ok(()))
    );
    let Reply::Manual(Err(r)) = handle(&mut c, &mut e, manual, 20) else {
        panic!("expected a refusal");
    };
    assert_eq!(r.code, RefusalCode::Busy);
}

#[test]
fn a_mail_nudge_waits_out_an_in_flight_manual_send_then_goes() {
    let (mut c, mut e) = setup();
    let manual = DeliveryEvent::Manual {
        harness_id: ID.into(),
        body: "x".into(),
    };
    handle(&mut c, &mut e, manual, 0);
    handle(&mut c, &mut e, ev_mail(), 50);
    assert_eq!(e.writes, vec![b"x".to_vec()]);
    drain(&mut c, &mut e, 2_500);
    assert_eq!(
        e.writes,
        vec![b"x".to_vec(), b"\r".to_vec(), nudge(1), b"\r".to_vec()]
    );
}
