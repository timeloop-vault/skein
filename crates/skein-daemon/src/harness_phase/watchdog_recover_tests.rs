//! Port of `app/src/harnessActivity.watchdog.test.ts`, second half: the
//! #404 `recoverUnheardSilence`, #422 authority re-grant and #644 Enter
//! suites. The #259 watchdog and #273 launch signal are in
//! `watchdog_tests.rs`.

use super::constants::ADAPTER_SILENT_AFTER_MS;
use super::test_support::Rig;
use super::{ActivityPhase, DegradedBy, TransitionSource};

const ID: &str = "w_1";

fn attached_harness(rig: &mut Rig) -> &'static str {
    rig.spawned(ID);
    rig.m.attach_authoritative_source(ID);
    rig.m.record_output(ID, Some("welcome banner"), rig.now);
    ID
}

fn launch(rig: &mut Rig, id: &str) {
    rig.m
        .note_launch_signal(id, TransitionSource::L2c1ClaudeSessionStart, rig.now);
}

// ---- recoverUnheardSilence (#404) -------------------------------------

/// An adapter-silent harness whose only proof of life was the launch
/// signal, degraded exactly the way `degrade_silent_adapter` leaves it,
/// then ordinary PTY output moves it `waiting -> running`.
fn adapter_silent_after_launch_signal(rig: &mut Rig) -> &'static str {
    let id = attached_harness(rig);
    launch(rig, id);
    rig.m.note_prompt_submitted(id, rig.now);
    rig.advance(11_000);
    assert!(rig.m.get(id).unwrap().adapter_silent);
    assert_eq!(
        rig.m.get(id).unwrap().degraded_by,
        Some(DegradedBy::AdapterSilent)
    );
    assert!(!rig.m.get(id).unwrap().adapter_heard);
    rig.m.record_output(id, Some("some redraw"), rig.now);
    assert_eq!(rig.m.get(id).unwrap().phase, ActivityPhase::Running);
    id
}

#[test]
fn recovers_the_adapter_silent_never_heard_launch_signalled_case() {
    let mut rig = Rig::new();
    let id = adapter_silent_after_launch_signal(&mut rig);
    rig.m.drain_transitions();

    assert!(rig.m.recover_unheard_silence(id, rig.now));

    let a = rig.m.get(id).unwrap();
    assert!(a.authoritative);
    assert!(!a.adapter_silent);
    assert_eq!(a.degraded_by, None);
    assert_eq!(a.prompt_submitted_at, None);
    assert!(a.silence_recovered);
    assert_eq!(a.phase, ActivityPhase::Waiting);
    let sources: Vec<_> = rig
        .m
        .drain_transitions()
        .into_iter()
        .filter(|t| t.harness_id == id)
        .map(|t| t.source)
        .collect();
    assert_eq!(sources, vec![TransitionSource::SilenceRecovered]);
}

#[test]
fn refuses_once_the_adapter_has_actually_spoken_adapter_heard() {
    let mut rig = Rig::new();
    let id = adapter_silent_after_launch_signal(&mut rig);
    // Write `adapter_heard` directly so this isolates only that one guard,
    // with the degrade fields left exactly as the degrade set them.
    rig.m.store.get_mut(id).unwrap().adapter_heard = true;
    assert!(!rig.m.recover_unheard_silence(id, rig.now));
}

#[test]
fn refuses_when_degraded_by_is_launch_silent_not_adapter_silent() {
    let mut rig = Rig::new();
    let id = attached_harness(&mut rig);
    rig.advance(16_000);
    assert_eq!(
        rig.m.get(id).unwrap().degraded_by,
        Some(DegradedBy::LaunchSilent)
    );
    assert!(!rig.m.recover_unheard_silence(id, rig.now));
}

#[test]
fn refuses_when_degraded_by_is_null_not_degraded_at_all() {
    let mut rig = Rig::new();
    let id = attached_harness(&mut rig);
    launch(&mut rig, id);
    assert_eq!(rig.m.get(id).unwrap().degraded_by, None);
    assert!(!rig.m.recover_unheard_silence(id, rig.now));
}

#[test]
fn refuses_when_launch_signal_at_is_null() {
    let mut rig = Rig::new();
    let id = attached_harness(&mut rig);
    // Synthetic state: since #644 the watchdog can't arm in spawning, so
    // forcing waiting is the only way to get an armed prompt with no
    // launch signal.
    rig.m.store.get_mut(id).unwrap().phase = ActivityPhase::Waiting;
    rig.m.note_prompt_submitted(id, rig.now);
    rig.advance(11_000);
    assert_eq!(
        rig.m.get(id).unwrap().degraded_by,
        Some(DegradedBy::AdapterSilent)
    );
    assert_eq!(rig.m.get(id).unwrap().launch_signal_at, None);
    assert!(!rig.m.recover_unheard_silence(id, rig.now));
}

#[test]
fn refuses_in_permission_phase() {
    let mut rig = Rig::new();
    let id = adapter_silent_after_launch_signal(&mut rig);
    rig.m.set_permission_from_adapter(
        id,
        TransitionSource::L2c1ClaudePermission,
        Some("Bash"),
        None,
        None,
        rig.now,
    );
    assert!(!rig.m.recover_unheard_silence(id, rig.now));
}

#[test]
fn refuses_once_exited() {
    let mut rig = Rig::new();
    let id = adapter_silent_after_launch_signal(&mut rig);
    rig.m.exited(id, Some(0), rig.now);
    assert!(!rig.m.recover_unheard_silence(id, rig.now));
}

#[test]
fn refuses_a_second_time_on_the_same_spawn() {
    let mut rig = Rig::new();
    let id = adapter_silent_after_launch_signal(&mut rig);
    assert!(rig.m.recover_unheard_silence(id, rig.now));
    // Degrade it again the same way, to prove the refusal is really
    // `silence_recovered`, not one of the other guards.
    rig.m.note_prompt_submitted(id, rig.now);
    rig.advance(11_000);
    assert_eq!(
        rig.m.get(id).unwrap().degraded_by,
        Some(DegradedBy::AdapterSilent)
    );
    assert!(!rig.m.recover_unheard_silence(id, rig.now));
}

#[test]
fn resets_silence_recovered_on_respawn() {
    let mut rig = Rig::new();
    let id = adapter_silent_after_launch_signal(&mut rig);
    assert!(rig.m.recover_unheard_silence(id, rig.now));
    assert!(rig.m.get(id).unwrap().silence_recovered);

    // A fresh spawn under the same id (not realistic in practice, but
    // exercises the field's reset in isolation).
    rig.spawned(id);
    assert!(!rig.m.get(id).unwrap().silence_recovered);
}

// ---- adapterDelivered authority re-grant (#422) -----------------------

#[test]
fn restores_authority_when_the_adapter_speaks_after_a_detach() {
    let mut rig = Rig::new();
    let id = attached_harness(&mut rig);
    rig.m.detach_authoritative_source(id, rig.now);
    assert!(!rig.m.get(id).unwrap().authoritative);

    rig.m.adapter_delivered(id, true, rig.now);
    assert!(rig.m.get(id).unwrap().authoritative);
}

#[test]
fn does_not_restore_authority_for_restores_authority_false() {
    let mut rig = Rig::new();
    let id = attached_harness(&mut rig);
    rig.m.adapter_delivered(id, true, rig.now);
    rig.m.detach_authoritative_source(id, rig.now);

    rig.m.adapter_delivered(id, false, rig.now);
    assert!(!rig.m.get(id).unwrap().authoritative);
}

// ---- Enter during spawning (#644) -------------------------------------

#[test]
fn does_not_arm_on_a_trust_dialog_enter_the_later_launch_signal_leaves_it_healthy() {
    let mut rig = Rig::new();
    let id = attached_harness(&mut rig);
    rig.m.record_input(id, "\r", false, rig.now);
    assert_eq!(rig.m.get(id).unwrap().prompt_submitted_at, None);
    launch(&mut rig, id);
    rig.advance(ADAPTER_SILENT_AFTER_MS + 2_000);
    let a = rig.m.get(id).unwrap();
    assert!(a.authoritative);
    assert!(!a.adapter_silent);
    assert_eq!(a.phase, ActivityPhase::Waiting);
    assert_eq!(a.prompt_submitted_at, None);
}

#[test]
fn does_not_degrade_when_the_dialog_is_answered_long_before_the_launch_signal() {
    let mut rig = Rig::new();
    let id = attached_harness(&mut rig);
    rig.m.record_input(id, "\r", false, rig.now);
    rig.advance(12_000);
    assert_eq!(rig.m.get(id).unwrap().phase, ActivityPhase::Spawning);
    launch(&mut rig, id);
    rig.advance(ADAPTER_SILENT_AFTER_MS + 2_000);
    let a = rig.m.get(id).unwrap();
    assert!(a.authoritative);
    assert!(!a.adapter_silent);
    assert_eq!(a.phase, ActivityPhase::Waiting);
}

#[test]
fn note_prompt_submitted_while_spawning_does_not_arm() {
    let mut rig = Rig::new();
    let id = attached_harness(&mut rig);
    rig.m.note_prompt_submitted(id, rig.now);
    assert_eq!(rig.m.get(id).unwrap().prompt_submitted_at, None);
}

#[test]
fn still_arms_and_degrades_for_an_enter_after_the_launch_signal() {
    let mut rig = Rig::new();
    let id = attached_harness(&mut rig);
    launch(&mut rig, id);
    rig.m.record_input(id, "\r", false, rig.now);
    assert!(rig.m.get(id).unwrap().prompt_submitted_at.is_some());
    rig.advance(ADAPTER_SILENT_AFTER_MS + 2_000);
    assert!(rig.m.get(id).unwrap().adapter_silent);
    assert_eq!(
        rig.m.get(id).unwrap().degraded_by,
        Some(DegradedBy::AdapterSilent)
    );
}

// ---- Enter on an empty composer (#644) --------------------------------

fn waiting_harness(rig: &mut Rig) -> &'static str {
    let id = attached_harness(rig);
    launch(rig, id);
    id
}

#[test]
fn empty_composer_does_not_arm_and_leaves_the_harness_healthy() {
    let mut rig = Rig::new();
    let id = waiting_harness(&mut rig);
    rig.m.record_input(id, "\r", true, rig.now);
    rig.advance(ADAPTER_SILENT_AFTER_MS + 2_000);
    let a = rig.m.get(id).unwrap();
    assert!(a.authoritative);
    assert!(!a.adapter_silent);
    assert_eq!(a.prompt_submitted_at, None);
    assert_eq!(a.phase, ActivityPhase::Waiting);
}

#[test]
fn arms_and_degrades_when_empty_composer_is_false() {
    let mut rig = Rig::new();
    let id = waiting_harness(&mut rig);
    rig.m.record_input(id, "\r", false, rig.now);
    rig.advance(ADAPTER_SILENT_AFTER_MS + 2_000);
    let a = rig.m.get(id).unwrap();
    assert!(a.adapter_silent);
    assert_eq!(a.degraded_by, Some(DegradedBy::AdapterSilent));
}

#[test]
fn still_records_last_submit_at() {
    let mut rig = Rig::new();
    let id = waiting_harness(&mut rig);
    rig.m.record_input(id, "\r", true, rig.now);
    assert!(rig.m.get(id).unwrap().last_submit_at.is_some());
}

#[test]
fn a_typed_prompt_enter_after_an_empty_one_arms() {
    let mut rig = Rig::new();
    let id = waiting_harness(&mut rig);
    rig.m.record_input(id, "\r", true, rig.now);
    rig.m.record_input(id, "\r", false, rig.now);
    assert!(rig.m.get(id).unwrap().prompt_submitted_at.is_some());
}
