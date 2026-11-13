//! Port of `app/src/harnessActivity.watchdog.test.ts`, first half: the #259
//! silent-adapter watchdog and the #273 launch signal. The #404 recovery,
//! #422 re-grant and #644 suites are in `watchdog_recover_tests.rs`.

use super::test_support::Rig;
use super::{ActivityPhase, DegradedBy, TransitionSource};

const ID: &str = "w_1";

/// A Claude harness as `LiveTerminal` leaves it: spawned, adapter attached,
/// TUI output flowing (which authority keeps from moving the phase).
fn attached_harness(rig: &mut Rig) -> &'static str {
    rig.spawned(ID);
    rig.m.attach_authoritative_source(ID);
    rig.m.record_output(ID, Some("welcome banner"), rig.now);
    ID
}

// ---- silent-adapter watchdog ------------------------------------------

#[test]
fn hands_a_harness_back_to_l2a_when_the_adapter_stays_silent_after_a_prompt() {
    let mut rig = Rig::new();
    let id = attached_harness(&mut rig);

    // #644: only an Enter at the prompt (past the launch signal) arms it.
    rig.m
        .note_launch_signal(id, TransitionSource::L2c1ClaudeSessionStart, rig.now);
    rig.m.record_input(id, "\r", false, rig.now);
    rig.advance(9_000);
    assert_eq!(rig.m.get(id).unwrap().phase, ActivityPhase::Waiting);
    assert!(rig.m.get(id).unwrap().authoritative);

    rig.advance(2_000);
    assert!(!rig.m.get(id).unwrap().authoritative);
    assert_eq!(
        rig.m.get(id).unwrap().degraded_by,
        Some(DegradedBy::AdapterSilent)
    );

    // And L2a actually runs it from here: output -> running, quiet -> idle.
    rig.m.record_output(id, Some("redraw"), rig.now);
    assert_eq!(rig.m.get(id).unwrap().phase, ActivityPhase::Running);
    rig.advance(9_000);
    assert_eq!(rig.m.get(id).unwrap().phase, ActivityPhase::Idle);
}

#[test]
fn never_arms_the_prompt_gated_watchdog_on_a_fresh_harness_nobody_has_prompted() {
    // Claude writes no transcript until the first prompt, so silence
    // before one is healthy; give it a launch signal so this isolates the
    // ADAPTER_SILENT_AFTER_MS watchdog, which only arms on `\r`/`\n`.
    let mut rig = Rig::new();
    let id = attached_harness(&mut rig);
    rig.m
        .note_launch_signal(id, TransitionSource::L2c1ClaudeSessionStart, rig.now);
    rig.m.record_input(id, "h", false, rig.now);
    rig.advance(60_000);
    assert_eq!(rig.m.get(id).unwrap().phase, ActivityPhase::Waiting);
    assert!(rig.m.get(id).unwrap().authoritative);
}

#[test]
fn stands_down_once_the_adapter_has_delivered() {
    let mut rig = Rig::new();
    let id = attached_harness(&mut rig);
    rig.m.adapter_delivered(id, true, rig.now);
    rig.m.record_input(id, "\r", false, rig.now);
    rig.advance(60_000);
    assert!(rig.m.get(id).unwrap().authoritative);
}

#[test]
fn gives_authority_back_when_a_silenced_adapter_speaks_after_all() {
    let mut rig = Rig::new();
    let id = attached_harness(&mut rig);
    rig.m
        .note_launch_signal(id, TransitionSource::L2c1ClaudeSessionStart, rig.now);
    rig.m.record_input(id, "\r", false, rig.now);
    rig.advance(11_000);
    assert!(!rig.m.get(id).unwrap().authoritative);
    assert_eq!(
        rig.m.get(id).unwrap().degraded_by,
        Some(DegradedBy::AdapterSilent)
    );

    rig.m.adapter_delivered(id, true, rig.now);
    rig.m
        .set_waiting_from_adapter(id, TransitionSource::L2c1ClaudeEndTurn, rig.now);
    assert!(rig.m.get(id).unwrap().authoritative);
    assert!(!rig.m.get(id).unwrap().adapter_silent);
    assert_eq!(rig.m.get(id).unwrap().degraded_by, None);

    // Authoritative again, so L2a leaves the adapter's phase alone.
    rig.advance(60_000);
    assert_eq!(rig.m.get(id).unwrap().phase, ActivityPhase::Waiting);
}

#[test]
fn does_not_watch_harnesses_without_an_adapter() {
    let mut rig = Rig::new();
    rig.spawned(ID);
    rig.m.record_input(ID, "\r", false, rig.now);
    assert_eq!(rig.m.get(ID).unwrap().prompt_submitted_at, None);
}

// ---- launch signal (#273) ---------------------------------------------

#[test]
fn moves_a_fresh_authoritative_harness_to_waiting_and_records_launch_signal_at() {
    let mut rig = Rig::new();
    let id = attached_harness(&mut rig);
    rig.m
        .note_launch_signal(id, TransitionSource::L2c1ClaudeSessionStart, rig.now);
    let a = rig.m.get(id).unwrap();
    assert_eq!(a.phase, ActivityPhase::Waiting);
    assert!(a.launch_signal_at.is_some());
}

#[test]
fn is_idempotent_across_a_phantom_second_session_start_fire() {
    let mut rig = Rig::new();
    let id = attached_harness(&mut rig);
    rig.m.drain_transitions();
    rig.m
        .note_launch_signal(id, TransitionSource::L2c1ClaudeSessionStart, rig.now);
    rig.m
        .note_launch_signal(id, TransitionSource::L2c1ClaudeSessionStart, rig.now);
    assert_eq!(rig.m.get(id).unwrap().phase, ActivityPhase::Waiting);
    let sources: Vec<_> = rig
        .m
        .drain_transitions()
        .into_iter()
        .filter(|t| t.harness_id == id)
        .map(|t| t.source)
        .collect();
    assert_eq!(sources, vec![TransitionSource::L2c1ClaudeSessionStart]);
}

#[test]
fn records_the_signal_without_moving_phase_once_the_tail_has_already_spoken() {
    let mut rig = Rig::new();
    let id = attached_harness(&mut rig);
    rig.m.adapter_delivered(id, true, rig.now);
    rig.m
        .set_running_from_adapter(id, TransitionSource::L2c1ClaudeAssistant, false, rig.now);
    rig.m
        .note_launch_signal(id, TransitionSource::L2c1ClaudeSessionStart, rig.now);
    let a = rig.m.get(id).unwrap();
    assert_eq!(a.phase, ActivityPhase::Running);
    assert!(a.launch_signal_at.is_some());
}

#[test]
fn leaves_permission_alone_it_outranks_a_launch_signal() {
    let mut rig = Rig::new();
    let id = attached_harness(&mut rig);
    rig.m.set_permission_from_adapter(
        id,
        TransitionSource::L2c1ClaudePermission,
        Some("Bash"),
        None,
        None,
        rig.now,
    );
    rig.m
        .note_launch_signal(id, TransitionSource::L2c1ClaudeSessionStart, rig.now);
    assert_eq!(rig.m.get(id).unwrap().phase, ActivityPhase::Permission);
}

#[test]
fn degrades_a_harness_out_of_spawning_after_launch_silent_after_ms_with_no_launch_signal() {
    let mut rig = Rig::new();
    let id = attached_harness(&mut rig);
    rig.m.drain_transitions();

    rig.advance(14_000);
    assert_eq!(rig.m.get(id).unwrap().phase, ActivityPhase::Spawning);
    assert!(rig.m.get(id).unwrap().authoritative);

    rig.advance(2_000);
    assert_eq!(rig.m.get(id).unwrap().phase, ActivityPhase::Running);
    assert!(!rig.m.get(id).unwrap().authoritative);
    let sources: Vec<_> = rig
        .m
        .drain_transitions()
        .into_iter()
        .filter(|t| t.harness_id == id)
        .map(|t| t.source)
        .collect();
    assert_eq!(sources, vec![TransitionSource::LaunchSilent]);
}

#[test]
fn restores_authority_and_moves_to_waiting_when_a_launch_signal_arrives_after_the_launch_silent_degrade()
 {
    let mut rig = Rig::new();
    let id = attached_harness(&mut rig);
    rig.advance(16_000);
    assert!(!rig.m.get(id).unwrap().authoritative);
    assert_eq!(
        rig.m.get(id).unwrap().degraded_by,
        Some(DegradedBy::LaunchSilent)
    );

    rig.m
        .note_launch_signal(id, TransitionSource::L2c1ClaudeSessionStart, rig.now);
    assert!(rig.m.get(id).unwrap().authoritative);
    assert!(!rig.m.get(id).unwrap().adapter_silent);
    assert_eq!(rig.m.get(id).unwrap().degraded_by, None);
    assert_eq!(rig.m.get(id).unwrap().phase, ActivityPhase::Waiting);
}

#[test]
fn does_not_degrade_a_harness_that_already_has_a_launch_signal() {
    let mut rig = Rig::new();
    let id = attached_harness(&mut rig);
    rig.m
        .note_launch_signal(id, TransitionSource::L2c1ClaudeSessionStart, rig.now);
    rig.advance(20_000);
    assert!(rig.m.get(id).unwrap().authoritative);
    assert!(!rig.m.get(id).unwrap().adapter_silent);
}

// Regression: a launch signal must NOT recover a harness the #259 watchdog
// gave up on; the two watchdogs diagnose different things.
#[test]
fn does_not_recover_a_harness_the_259_adapter_silent_watchdog_gave_up_on() {
    let mut rig = Rig::new();
    let id = attached_harness(&mut rig);
    rig.m
        .note_launch_signal(id, TransitionSource::L2c1ClaudeSessionStart, rig.now);
    rig.m.record_input(id, "\r", false, rig.now);
    rig.advance(11_000);
    rig.m.record_output(id, Some("some redraw"), rig.now);
    assert_eq!(rig.m.get(id).unwrap().phase, ActivityPhase::Running);
    assert!(rig.m.get(id).unwrap().adapter_silent);
    assert_eq!(
        rig.m.get(id).unwrap().degraded_by,
        Some(DegradedBy::AdapterSilent)
    );

    // The user later types `/clear`; the matcher-less SessionStart hook
    // fires again even though the tail is still reading the wrong file.
    rig.m
        .note_launch_signal(id, TransitionSource::L2c1ClaudeSessionStart, rig.now);

    assert!(!rig.m.get(id).unwrap().authoritative);
    assert!(rig.m.get(id).unwrap().adapter_silent);
    assert_eq!(
        rig.m.get(id).unwrap().degraded_by,
        Some(DegradedBy::AdapterSilent)
    );
    assert_ne!(rig.m.get(id).unwrap().phase, ActivityPhase::Waiting);
    assert_eq!(rig.m.get(id).unwrap().phase, ActivityPhase::Running);
    // The fact itself is still true and worth recording.
    assert!(rig.m.get(id).unwrap().launch_signal_at.is_some());
}

// #273's own timer must never degrade a harness whose adapter has already
// proven itself.
#[test]
fn never_degrades_an_authoritative_harness_that_the_adapter_confirms_before_the_launch_silent_timer()
 {
    let mut rig = Rig::new();
    let id = attached_harness(&mut rig);
    rig.m.adapter_delivered(id, true, rig.now);
    rig.advance(20_000);
    assert!(rig.m.get(id).unwrap().authoritative);
    assert!(!rig.m.get(id).unwrap().adapter_silent);
    assert_eq!(rig.m.get(id).unwrap().degraded_by, None);
}
