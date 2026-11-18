//! Scenario tests for the context-limit nudge through [`DeliveryCore`],
//! against the shared fake env and a manual clock.

#![allow(clippy::assert_is_empty)]

use crate::harness_phase::ActivityPhase;
use crate::prompt_delivery::context_nudge::{Mark, render};
use crate::prompt_delivery::core::{DeliveryCore, DeliveryEvent};
use crate::prompt_delivery::env::{DraftState, UnreadMail};
use crate::prompt_delivery::mail_nudge::mail_nudge_text;
use crate::prompt_delivery::test_support::{Fake, T0, fake};
use skein_proto::harness_kind::HarnessKind;

const ID: &str = "h1";
const LIMIT: u64 = 100;

fn mark(row: Option<i64>, armed: bool) -> Mark {
    Mark {
        last_row_id: row,
        armed,
    }
}

/// A waiting opencode harness with a limit of 100, nudging on, and an armed
/// mark that has seen row 1 (50 tokens).
fn setup() -> (DeliveryCore, Fake) {
    let mut e = fake(HarnessKind::Opencode);
    e.context.limit = Some(LIMIT);
    e.context_row = Some((1, 50));
    e.mark = Some(mark(Some(1), true));
    (DeliveryCore::new(), e)
}

fn body(tokens: u64) -> Vec<u8> {
    render(None, tokens, LIMIT).into_bytes()
}

fn waiting(c: &mut DeliveryCore, e: &mut Fake, ms: i64) {
    e.set_phase(|f| f.phase = ActivityPhase::Waiting);
    c.handle(
        e,
        DeliveryEvent::Transition {
            harness_id: ID.into(),
            from: ActivityPhase::Running,
            to: ActivityPhase::Waiting,
        },
        T0 + ms,
    );
}

fn running(c: &mut DeliveryCore, e: &mut Fake, ms: i64) {
    e.set_phase(|f| f.phase = ActivityPhase::Running);
    c.handle(
        e,
        DeliveryEvent::Transition {
            harness_id: ID.into(),
            from: ActivityPhase::Waiting,
            to: ActivityPhase::Running,
        },
        T0 + ms,
    );
}

fn event(c: &mut DeliveryCore, e: &mut Fake, f: fn(String) -> DeliveryEvent, ms: i64) {
    c.handle(e, f(ID.to_owned()), T0 + ms);
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

fn pastes(e: &Fake, text: &[u8]) -> usize {
    e.writes.iter().filter(|w| w.as_slice() == text).count()
}

/// One full over-limit turn: the row lands, the harness reaches waiting, and
/// the send (if any) finishes.
fn turn(c: &mut DeliveryCore, e: &mut Fake, row: (i64, u64), ms: i64) {
    running(c, e, ms);
    e.context_row = Some(row);
    waiting(c, e, ms + 100);
    drain(c, e, ms + 1_000);
}

#[test]
fn a_crossing_nudges_once_with_the_rendered_numbers() {
    let (mut c, mut e) = setup();
    turn(&mut c, &mut e, (2, 150), 0);
    assert_eq!(pastes(&e, &body(150)), 1);
    assert_eq!(e.mark, Some(mark(Some(2), false)));
    // Still over the limit on the next turn: no second nudge.
    turn(&mut c, &mut e, (3, 170), 5_000);
    assert_eq!(e.writes.len(), 2); // paste + Enter, once
    assert_eq!(e.mark, Some(mark(Some(3), false)));
}

#[test]
fn dropping_below_then_crossing_again_nudges_again() {
    let (mut c, mut e) = setup();
    turn(&mut c, &mut e, (2, 150), 0);
    turn(&mut c, &mut e, (3, 40), 5_000);
    assert_eq!(e.mark, Some(mark(Some(3), true)));
    turn(&mut c, &mut e, (4, 180), 10_000);
    assert_eq!(pastes(&e, &body(150)), 1);
    assert_eq!(pastes(&e, &body(180)), 1);
}

#[test]
fn a_disabled_nudge_stores_the_crossing_and_never_fires_for_it() {
    let (mut c, mut e) = setup();
    e.context.enabled = false;
    turn(&mut c, &mut e, (2, 150), 0);
    assert!(e.writes.is_empty());
    assert_eq!(e.mark, Some(mark(Some(2), false)));
    e.context.enabled = true;
    turn(&mut c, &mut e, (3, 160), 5_000);
    assert!(e.writes.is_empty());
}

#[test]
fn no_limit_never_nudges_but_keeps_the_mark_current() {
    let (mut c, mut e) = setup();
    e.context.limit = None;
    turn(&mut c, &mut e, (2, 150), 0);
    assert!(e.writes.is_empty());
    assert_eq!(e.mark, Some(mark(Some(2), true)));
}

#[test]
fn a_custom_body_is_rendered() {
    let (mut c, mut e) = setup();
    e.context.body = Some("{tokens} of {limit}".into());
    turn(&mut c, &mut e, (2, 150), 0);
    assert_eq!(pastes(&e, b"150 of 100"), 1);
}

#[test]
fn a_harness_without_a_context_row_is_never_nudged() {
    let (mut c, mut e) = setup();
    e.context_row = None;
    waiting(&mut c, &mut e, 0);
    drain(&mut c, &mut e, 20_000);
    assert!(e.writes.is_empty());
    assert_eq!(e.mark, Some(mark(Some(1), true)));
}

#[test]
fn mail_in_flight_delays_the_nudge_to_a_recheck() {
    let (mut c, mut e) = setup();
    e.unread = Some(UnreadMail {
        count: 1,
        from: Vec::new(),
    });
    running(&mut c, &mut e, 0);
    e.context_row = Some((2, 150));
    waiting(&mut c, &mut e, 100);
    // Mail went first; the context nudge was refused as busy and not stored.
    assert_eq!(e.writes, vec![mail_nudge_text(1, &[]).into_bytes()]);
    assert_eq!(e.mark, Some(mark(Some(1), true)));
    drain(&mut c, &mut e, 10_000);
    assert_eq!(pastes(&e, &body(150)), 1);
    assert_eq!(e.mark, Some(mark(Some(2), false)));
}

#[test]
fn a_user_draft_holds_the_nudge_until_it_clears() {
    let (mut c, mut e) = setup();
    e.draft = DraftState::Held {
        reason: "draft".into(),
    };
    running(&mut c, &mut e, 0);
    e.context_row = Some((2, 150));
    waiting(&mut c, &mut e, 100);
    drain(&mut c, &mut e, 30_000);
    assert!(e.writes.is_empty());
    assert_eq!(e.mark, Some(mark(Some(1), true)));
    e.draft = DraftState::Clean;
    drain(&mut c, &mut e, 40_000);
    assert_eq!(pastes(&e, &body(150)), 1);
}

#[test]
fn a_refused_nudge_stops_being_retried_after_a_minute() {
    let (mut c, mut e) = setup();
    e.draft = DraftState::Held {
        reason: "draft".into(),
    };
    running(&mut c, &mut e, 0);
    e.context_row = Some((2, 150));
    waiting(&mut c, &mut e, 100);
    drain(&mut c, &mut e, 120_000);
    assert_eq!(c.next_deadline(), None);
    e.draft = DraftState::Clean;
    drain(&mut c, &mut e, 130_000);
    assert!(e.writes.is_empty());
    // The next waiting transition decides it due again.
    turn(&mut c, &mut e, (2, 150), 200_000);
    assert_eq!(pastes(&e, &body(150)), 1);
}

#[test]
fn a_row_landing_after_the_waiting_transition_is_picked_up_by_a_recheck() {
    let (mut c, mut e) = setup();
    running(&mut c, &mut e, 0);
    waiting(&mut c, &mut e, 100);
    assert!(e.writes.is_empty());
    assert_eq!(c.next_deadline(), Some(T0 + 100 + 2_000));
    e.context_row = Some((2, 150));
    drain(&mut c, &mut e, 5_000);
    assert_eq!(pastes(&e, &body(150)), 1);
}

#[test]
fn rechecks_stop_when_the_harness_leaves_waiting() {
    let (mut c, mut e) = setup();
    waiting(&mut c, &mut e, 0);
    assert!(c.next_deadline().is_some());
    running(&mut c, &mut e, 500);
    assert_eq!(c.next_deadline(), None);
    e.context_row = Some((2, 150));
    drain(&mut c, &mut e, 20_000);
    assert!(e.writes.is_empty());
}

#[test]
fn rechecks_stop_when_the_harness_is_forgotten_or_the_quiet_window_ends() {
    let (mut c, mut e) = setup();
    waiting(&mut c, &mut e, 0);
    event(
        &mut c,
        &mut e,
        |h| DeliveryEvent::Forgotten { harness_id: h },
        500,
    );
    assert_eq!(c.next_deadline(), None);
    waiting(&mut c, &mut e, 1_000);
    drain(&mut c, &mut e, 30_000);
    assert_eq!(c.next_deadline(), None);
    // The window is over: a row landing now waits for the next turn.
    e.context_row = Some((2, 150));
    assert!(e.writes.is_empty());
}

#[test]
fn a_restart_does_not_nudge_a_crossing_twice() {
    let (mut c, mut e) = setup();
    turn(&mut c, &mut e, (2, 150), 0);
    assert_eq!(pastes(&e, &body(150)), 1);
    // A new worker over the same persisted marks.
    let mut c2 = DeliveryCore::new();
    waiting(&mut c2, &mut e, 20_000);
    drain(&mut c2, &mut e, 40_000);
    assert_eq!(pastes(&e, &body(150)), 1);
}

#[test]
fn a_harness_first_seen_over_the_limit_is_not_nudged() {
    let (mut c, mut e) = setup();
    e.mark = None;
    e.context_row = Some((2, 150));
    event(
        &mut c,
        &mut e,
        |h| DeliveryEvent::Spawned { harness_id: h },
        0,
    );
    assert_eq!(e.mark, Some(mark(Some(2), false)));
    waiting(&mut c, &mut e, 100);
    drain(&mut c, &mut e, 20_000);
    assert!(e.writes.is_empty());
}

#[test]
fn a_new_harness_whose_first_turn_passes_the_limit_is_nudged() {
    let (mut c, mut e) = setup();
    e.mark = None;
    e.context_row = None;
    event(
        &mut c,
        &mut e,
        |h| DeliveryEvent::Spawned { harness_id: h },
        0,
    );
    assert_eq!(e.mark, Some(mark(None, true)));
    turn(&mut c, &mut e, (1, 150), 1_000);
    assert_eq!(pastes(&e, &body(150)), 1);
}

fn spawn(c: &mut DeliveryCore, e: &mut Fake) {
    event(c, e, |h| DeliveryEvent::Spawned { harness_id: h }, 0);
}

#[test]
fn a_limit_set_after_spawn_nudges_the_next_crossing() {
    let (mut c, mut e) = setup();
    e.mark = None;
    e.context.limit = None;
    e.context_row = Some((1, 60));
    spawn(&mut c, &mut e);
    assert_eq!(e.mark, Some(mark(Some(1), true)));
    e.context.limit = Some(LIMIT);
    turn(&mut c, &mut e, (2, 150), 1_000);
    assert_eq!(pastes(&e, &body(150)), 1);
}

#[test]
fn a_limit_set_while_already_over_nudges_the_next_new_row() {
    let (mut c, mut e) = setup();
    e.mark = None;
    e.context.limit = None;
    e.context_row = Some((1, 60));
    spawn(&mut c, &mut e);
    turn(&mut c, &mut e, (2, 160), 1_000);
    assert!(e.writes.is_empty());
    assert_eq!(e.mark, Some(mark(Some(2), true)));
    e.context.limit = Some(LIMIT);
    turn(&mut c, &mut e, (3, 165), 5_000);
    assert_eq!(pastes(&e, &body(165)), 1);
}

#[test]
fn a_stale_row_at_respawn_is_not_nudged() {
    let (mut c, mut e) = setup();
    e.mark = Some(mark(Some(5), true));
    e.context_row = Some((9, 150));
    spawn(&mut c, &mut e);
    assert_eq!(e.mark, Some(mark(Some(9), true)));
    waiting(&mut c, &mut e, 100);
    drain(&mut c, &mut e, 20_000);
    assert!(e.writes.is_empty());
    turn(&mut c, &mut e, (10, 160), 30_000);
    assert_eq!(pastes(&e, &body(160)), 1);
}

#[test]
fn a_stale_row_over_the_limit_leaves_a_disarmed_mark_disarmed() {
    let (mut c, mut e) = setup();
    e.mark = Some(mark(Some(5), false));
    e.context_row = Some((9, 150));
    spawn(&mut c, &mut e);
    assert_eq!(e.mark, Some(mark(Some(9), false)));
    e.context_row = Some((10, 40));
    spawn(&mut c, &mut e);
    assert_eq!(e.mark, Some(mark(Some(10), true)));
}

#[test]
fn a_skipped_submit_leaves_the_nudge_due_again() {
    let (mut c, mut e) = setup();
    running(&mut c, &mut e, 0);
    e.context_row = Some((2, 150));
    waiting(&mut c, &mut e, 100);
    assert_eq!(e.mark, Some(mark(Some(2), false)));
    // The user types before the submit goes out.
    e.seq += 1;
    drain(&mut c, &mut e, 1_000);
    assert_eq!(e.enters(), 0);
    assert_eq!(e.mark, Some(mark(Some(1), true)));
    // The next stopping point sends it again.
    turn(&mut c, &mut e, (2, 150), 5_000);
    assert_eq!(pastes(&e, &body(150)), 2);
    assert_eq!(e.enters(), 1);
    assert_eq!(e.mark, Some(mark(Some(2), false)));
}

#[test]
fn releasing_a_draft_after_the_window_rechecks() {
    let (mut c, mut e) = setup();
    e.draft = DraftState::Held {
        reason: "draft".into(),
    };
    running(&mut c, &mut e, 0);
    e.context_row = Some((2, 150));
    waiting(&mut c, &mut e, 100);
    drain(&mut c, &mut e, 120_000);
    assert_eq!(c.next_deadline(), None);
    e.draft = DraftState::Clean;
    event(
        &mut c,
        &mut e,
        |h| DeliveryEvent::ClientFactsChanged { harness_id: h },
        130_000,
    );
    assert_eq!(pastes(&e, &body(150)), 1);
    assert_eq!(e.mark, Some(mark(Some(2), false)));
}

#[test]
fn spawning_keeps_an_existing_mark() {
    let (mut c, mut e) = setup();
    e.mark = Some(mark(Some(1), false));
    event(
        &mut c,
        &mut e,
        |h| DeliveryEvent::Spawned { harness_id: h },
        0,
    );
    assert_eq!(e.mark, Some(mark(Some(1), false)));
}
