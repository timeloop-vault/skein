//! Ports of `mailNudge.test.ts`, `mailHold.test.ts` (the pure half) and
//! `mailRetry.test.ts`'s `mailPending`.

use super::*;

fn d(at: bool, unread: u32, last: u32, ok: bool) -> (bool, u32) {
    let r = decide_mail_nudge(at, unread, last, ok);
    (r.nudge, r.last_nudged)
}

fn refused(code: RefusalCode) -> Refusal {
    Refusal {
        code,
        reason: "why".into(),
    }
}

#[test]
fn nudges_when_unread_grew_past_last_nudged() {
    assert_eq!(d(true, 2, 0, true), (true, 2));
}

#[test]
fn does_not_nudge_twice_for_the_same_count() {
    assert_eq!(d(true, 2, 2, true), (false, 2));
}

#[test]
fn records_nothing_off_a_stopping_point_or_with_a_closed_gate() {
    assert_eq!(d(false, 3, 1, true), (false, 1));
    assert_eq!(d(false, 2, 0, true), (false, 0));
    assert_eq!(d(true, 3, 1, false), (false, 1));
}

#[test]
fn nudges_at_a_deferred_stopping_point() {
    assert_eq!(d(true, 3, 1, true), (true, 3));
}

#[test]
fn a_full_read_resets_so_new_mail_nudges_again() {
    let after_read = d(true, 0, 3, true);
    assert_eq!(after_read, (false, 0));
    assert_eq!(d(true, 1, after_read.1, true), (true, 1));
}

#[test]
fn a_partial_read_only_nudges_once_unread_exceeds_what_is_left() {
    let after = d(true, 1, 3, true);
    assert_eq!(after, (false, 1));
    assert_eq!(d(true, 2, after.1, true), (true, 2));
}

#[test]
fn after_a_restart_with_stored_mail_nudges_once() {
    let first = d(true, 2, 0, true);
    assert_eq!(first, (true, 2));
    assert_eq!(d(true, 2, first.1, true), (false, 2));
}

#[test]
fn never_nudges_for_zero_unread() {
    assert_eq!(d(true, 0, 0, true), (false, 0));
}

#[test]
fn held_then_cleared_nudges_with_the_same_count() {
    let held = d(true, 2, 0, false);
    assert_eq!(held, (false, 0));
    assert_eq!(d(true, 2, held.1, true), (true, 2));
}

#[test]
fn transition_check_predicate() {
    use ActivityPhase::{Exited, Idle, Permission, Running, Waiting};
    assert!(should_check_on_transition(Waiting, false));
    assert!(should_check_on_transition(Waiting, true));
    assert!(should_check_on_transition(Running, true));
    assert!(!should_check_on_transition(Running, false));
    assert!(!should_check_on_transition(Permission, false));
    assert!(!should_check_on_transition(Idle, false));
    assert!(!should_check_on_transition(Exited, false));
}

#[test]
fn nudge_text_matches_the_ts_wording() {
    assert_eq!(
        mail_nudge_text(1, &[]),
        "You have 1 new message in Skein. Call read_messages."
    );
    assert_eq!(
        mail_nudge_text(3, &[]),
        "You have 3 new messages in Skein. Call read_messages."
    );
    assert_eq!(
        mail_nudge_text(2, &["Alpha".into()]),
        "You have 2 new messages in Skein from Alpha. Call read_messages."
    );
    assert_eq!(
        mail_nudge_text(2, &["Alpha".into(), "Beta".into()]),
        "You have 2 new messages in Skein from Alpha, Beta. Call read_messages."
    );
}

#[test]
fn held_only_with_undelivered_mail_and_a_draft_refusal() {
    let draft = refused(RefusalCode::DraftHeld);
    let other = refused(RefusalCode::NotReady);
    assert!(mail_held(2, 0, Some(&draft)));
    assert!(!mail_held(2, 0, Some(&other)));
    assert!(!mail_held(2, 0, None));
    assert!(!mail_held(2, 2, Some(&draft)));
    assert!(!mail_held(0, 0, Some(&draft)));
    assert!(is_draft_hold(Some(&draft)));
    assert!(!is_draft_hold(None));
}

#[test]
fn release_refusal_prefers_the_gate_then_explains() {
    let g = refused(RefusalCode::NotReady);
    assert_eq!(release_refusal(true, 2, 0, Some(&g)), g);
    assert_eq!(
        release_refusal(true, 0, 0, None).reason,
        "no undelivered mail"
    );
    assert_eq!(
        release_refusal(true, 2, 2, None).reason,
        "no undelivered mail"
    );
    assert_eq!(
        release_refusal(false, 2, 0, None).reason,
        "the harness isn't at a safe stopping point"
    );
    assert_eq!(
        release_refusal(true, 2, 0, None).reason,
        "nothing to deliver"
    );
}

#[test]
fn pending_is_growth_past_last_nudged() {
    assert!(mail_pending(1, 0));
    assert!(!mail_pending(2, 2));
    assert!(!mail_pending(0, 0));
}
