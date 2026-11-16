//! Layer-level tests of [`Mail`]: the #386 retry window, the #388
//! settlement bookkeeping, and the #413 held marker, driving `Mail` and a
//! `Sender` directly on a manual clock.

#![allow(clippy::assert_is_empty)]

use super::*;
use crate::prompt_delivery::env::UnreadMail;
use crate::prompt_delivery::mail_retry::{MAIL_RETRY_INTERVAL_MS, MAIL_RETRY_WINDOW_MS};
use crate::prompt_delivery::test_support::{Fake, T0, fake};
use skein_proto::harness_kind::HarnessKind;

const ID: &str = "h1";

fn setup(n: u32) -> (Mail, Sender, Fake) {
    let mut e = fake(HarnessKind::Opencode);
    e.unread = Some(UnreadMail {
        count: n,
        from: Vec::new(),
    });
    (Mail::new(), Sender::new(), e)
}

fn running(e: &mut Fake) {
    e.set_phase(|p| p.phase = ActivityPhase::Running);
}

#[test]
fn an_event_rearms_the_retry_window_but_a_tick_keeps_it() {
    let (mut m, mut s, mut e) = setup(1);
    running(&mut e);
    m.automatic(&mut s, &mut e, ID, T0);
    assert_eq!(m.next_deadline(), Some(T0 + MAIL_RETRY_INTERVAL_MS));
    // A tick keeps the window armed at T0 ...
    m.on_timer(&mut s, &mut e, T0 + MAIL_RETRY_INTERVAL_MS);
    assert_eq!(m.next_deadline(), Some(T0 + 2 * MAIL_RETRY_INTERVAL_MS));
    let last = T0 + MAIL_RETRY_WINDOW_MS - 1;
    m.on_timer(&mut s, &mut e, last);
    assert_eq!(m.next_deadline(), Some(last + MAIL_RETRY_INTERVAL_MS));
    m.on_timer(&mut s, &mut e, last + MAIL_RETRY_INTERVAL_MS);
    assert_eq!(m.next_deadline(), None);
    // ... while an event re-arms it from now.
    m.automatic(&mut s, &mut e, ID, T0 + MAIL_RETRY_WINDOW_MS + 10);
    assert_eq!(
        m.next_deadline(),
        Some(T0 + MAIL_RETRY_WINDOW_MS + 10 + MAIL_RETRY_INTERVAL_MS)
    );
    assert!(e.writes.is_empty());
}

#[test]
fn nothing_pending_arms_no_retry() {
    let (mut m, mut s, mut e) = setup(0);
    m.automatic(&mut s, &mut e, ID, T0);
    assert_eq!(m.next_deadline(), None);
}

#[test]
fn a_pending_settlement_blocks_a_second_nudge_for_new_mail() {
    let (mut m, mut s, mut e) = setup(1);
    m.automatic(&mut s, &mut e, ID, T0);
    s.on_timer(&mut e, T0 + 150);
    assert_eq!(e.writes.len(), 2);
    e.unread = Some(UnreadMail {
        count: 2,
        from: Vec::new(),
    });
    m.automatic(&mut s, &mut e, ID, T0 + 9_000);
    assert_eq!(e.writes.len(), 2);
    // The settlement is not a draft hold.
    assert!(e.held_log.is_empty());
}

#[test]
fn a_read_lowers_last_nudged_so_new_mail_nudges_again() {
    let (mut m, mut s, mut e) = setup(2);
    m.automatic(&mut s, &mut e, ID, T0);
    s.on_timer(&mut e, T0 + 150);
    // Turn started: confirmed at the next check.
    m.on_transition(
        &mut s,
        &mut e,
        ID,
        ActivityPhase::Waiting,
        ActivityPhase::Running,
        T0 + 1_000,
    );
    // The agent read everything, then one more message arrived.
    e.unread = Some(UnreadMail {
        count: 0,
        from: Vec::new(),
    });
    m.automatic(&mut s, &mut e, ID, T0 + 2_000);
    e.unread = Some(UnreadMail {
        count: 1,
        from: vec!["Alpha".into()],
    });
    m.automatic(&mut s, &mut e, ID, T0 + 3_000);
    assert_eq!(
        e.writes.last().map(Vec::as_slice),
        Some(b"You have 1 new message in Skein from Alpha. Call read_messages.".as_slice())
    );
}

#[test]
fn reset_clears_a_held_marker_once_and_only_when_held() {
    let (mut m, mut s, mut e) = setup(1);
    m.reset(&mut e, ID);
    assert!(e.held_log.is_empty());
    e.draft = DraftState::Held {
        reason: "typed".into(),
    };
    m.automatic(&mut s, &mut e, ID, T0);
    m.reset(&mut e, ID);
    m.reset(&mut e, ID);
    assert_eq!(e.held_log, vec![true, false]);
}

#[test]
fn a_failed_send_keeps_the_pre_nudge_value_so_the_retry_tries_again() {
    let (mut m, mut s, mut e) = setup(1);
    e.write_ok = false;
    m.automatic(&mut s, &mut e, ID, T0);
    assert!(e.writes.is_empty());
    assert_eq!(m.next_deadline(), Some(T0 + MAIL_RETRY_INTERVAL_MS));
    e.write_ok = true;
    m.on_timer(&mut s, &mut e, T0 + MAIL_RETRY_INTERVAL_MS);
    assert_eq!(e.writes.len(), 1);
}

#[test]
fn an_unknown_harness_leaves_no_state() {
    let (mut m, mut s, mut e) = setup(1);
    // No unread lookup.
    e.unread = None;
    m.automatic(&mut s, &mut e, "ghost", T0);
    assert!(m.states.is_empty());
    // No phase record.
    e.unread = Some(UnreadMail {
        count: 1,
        from: Vec::new(),
    });
    e.facts.phase = None;
    assert!(m.deliver_now(&mut s, &mut e, "ghost", T0).is_err());
    m.automatic(&mut s, &mut e, "ghost", T0);
    assert!(m.states.is_empty());
    assert_eq!(m.next_deadline(), None);
}
