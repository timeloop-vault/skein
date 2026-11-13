//! Port of `app/src/harnessEvents.authority.test.ts` (#422): attach
//! generations and authority restored when a live adapter speaks after its
//! own `session_end`. The TS "attach rejection" and "stale cleanup" cases
//! are the translator's `live` bit here: one [`ClaudeTranslator`] per
//! attach, `detached` for a cleanup or a late rejection, `superseded` for a
//! newer attach taking over.
//!
//! NOT PORTED (webview plumbing, all asserting the `logBoth` mock):
//! the `#422` / `adapter recovered` log-line counts inside
//! "a live adapter speaking after session_end takes authority back",
//! "does not re-grant authority once exited", "an adapterSilent harness
//! takes the old recovery branch" and "session_end then connected re-arms
//! without a #422 line". Their authority and phase assertions are ported.
//! NOT PORTED: opencode "a stale cleanup does not detach the newer
//! attach", "a session_end from a replaced attach's channel is ignored" and
//! "a session_end on a closed attach's channel is ignored": the opencode
//! translator carries no attach generation; `translate/opencode.rs` leaves
//! that guard to the caller (`beginAttach`/`isLive`).

use super::*;
use crate::harness_events_opencode::OpencodeEvent;
use crate::harness_phase::DegradedBy;
use crate::harness_phase::test_support::Rig;
use crate::harness_phase::translate::opencode::OpencodeTranslator;
use crate::harness_phase::types::ActivityPhase;

const ID: &str = "ev_1";

fn fresh() -> Rig {
    let mut rig = Rig::new();
    rig.spawned(ID);
    rig
}

fn attach(rig: &mut Rig) -> ClaudeTranslator {
    let mut t = ClaudeTranslator::new();
    t.attached(&mut rig.m, ID, rig.now);
    t
}

fn auth(rig: &Rig) -> bool {
    rig.m.get(ID).expect("record").authoritative
}

fn send(rig: &mut Rig, t: &mut ClaudeTranslator, e: &ClaudeEvent) {
    t.apply(&mut rig.m, ID, e, rig.now);
}

#[test]
fn a_late_attach_rejection_of_a_replaced_attach_leaves_the_new_one_authoritative() {
    let mut rig = fresh();
    let mut a = attach(&mut rig);
    a.detached(&mut rig.m, ID, rig.now); // cleanupA
    let _b = attach(&mut rig);
    a.detached(&mut rig.m, ID, rig.now); // rejectA arrives late
    assert!(auth(&rig));
}

#[test]
fn a_stale_cleanup_does_not_detach_the_newer_attach() {
    let mut rig = fresh();
    let mut a = attach(&mut rig);
    let _b = attach(&mut rig);
    a.superseded();
    a.detached(&mut rig.m, ID, rig.now); // cleanupA
    assert!(auth(&rig));
}

#[test]
fn a_session_end_on_a_closed_attachs_channel_does_nothing_to_the_new_attach() {
    let mut rig = fresh();
    let mut a = attach(&mut rig);
    a.detached(&mut rig.m, ID, rig.now);
    let _b = attach(&mut rig);
    let phase = rig.m.get(ID).expect("record").phase;
    send(&mut rig, &mut a, &ClaudeEvent::SessionEnd);
    assert!(auth(&rig));
    assert_eq!(rig.m.get(ID).expect("record").phase, phase);
}

#[test]
fn a_live_adapter_speaking_after_session_end_takes_authority_back() {
    let mut rig = fresh();
    let mut t = attach(&mut rig);
    send(&mut rig, &mut t, &ClaudeEvent::SessionEnd);
    assert!(!auth(&rig));
    send(&mut rig, &mut t, &ClaudeEvent::AwaitingPrompt);
    assert!(auth(&rig));
    assert_eq!(rig.m.get(ID).expect("record").phase, ActivityPhase::Waiting);
}

#[test]
fn claude_attach_rejection_after_its_own_cleanup_does_not_throw_or_touch_a_harness_with_no_attach()
{
    let mut rig = fresh();
    let mut t = attach(&mut rig);
    t.detached(&mut rig.m, ID, rig.now); // cleanup
    t.detached(&mut rig.m, ID, rig.now); // late rejection
    assert!(!auth(&rig));
}

#[test]
fn adapter_delivered_does_not_re_grant_authority_once_exited() {
    let mut rig = fresh();
    let mut t = attach(&mut rig);
    send(&mut rig, &mut t, &ClaudeEvent::SessionEnd);
    rig.m.exited(ID, Some(0), rig.now);
    send(&mut rig, &mut t, &ClaudeEvent::AwaitingPrompt);
    assert!(!auth(&rig));
}

#[test]
fn an_adapter_silent_harness_takes_the_old_recovery_branch() {
    let mut rig = fresh();
    let mut t = attach(&mut rig);
    let cur = rig.m.store.get_mut(ID).expect("record");
    cur.adapter_silent = true;
    cur.authoritative = false;
    cur.degraded_by = Some(DegradedBy::AdapterSilent);
    send(&mut rig, &mut t, &ClaudeEvent::AwaitingPrompt);
    assert!(auth(&rig));
}

#[test]
fn opencode_session_end_then_connected_re_arms_without_a_422_line() {
    let mut rig = fresh();
    let mut t = OpencodeTranslator::new();
    t.attached(&mut rig.m, ID, rig.now);
    t.apply(&mut rig.m, ID, &OpencodeEvent::Connected, rig.now);
    t.apply(&mut rig.m, ID, &OpencodeEvent::SessionEnd, rig.now);
    assert!(!auth(&rig));
    t.apply(&mut rig.m, ID, &OpencodeEvent::Connected, rig.now);
    assert!(auth(&rig));
}
