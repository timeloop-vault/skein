//! Port of `app/src/harnessActivity.screen.test.ts` (#623, the screen-phase
//! tick rule). In TS the reader is a callback returning the reading on
//! each tick; here the client reports it with `set_screen_reading` and the
//! tick uses the stored value, so a TS `reads[id] = x` is a
//! `set_screen_reading` call before the next `advance`.

use crate::harness_phase::screen::SCREEN_UNKNOWN_RELEASE_TICKS;
use crate::harness_phase::test_support::Rig;
use crate::harness_phase::{ActivityPhase, ScreenReading, TransitionSource};

const ID: &str = "s_1";

fn tick(rig: &mut Rig, n: i64) {
    rig.advance(1000 * n);
}

fn phase(rig: &Rig, id: &str) -> ActivityPhase {
    rig.m.get(id).unwrap().phase
}

fn spawn_running(rig: &mut Rig, id: &str) {
    rig.spawned(id);
    rig.m.record_output(id, Some("banner"), rig.now);
    assert_eq!(phase(rig, id), ActivityPhase::Running);
}

fn read(rig: &mut Rig, id: &str, r: ScreenReading) {
    rig.m.set_screen_reading(id, Some(r));
}

#[test]
fn needs_two_waiting_reads_before_running_to_waiting() {
    let mut rig = Rig::new();
    spawn_running(&mut rig, ID);
    rig.m.drain_transitions();
    read(&mut rig, ID, ScreenReading::Waiting);
    tick(&mut rig, 1);
    assert_eq!(phase(&rig, ID), ActivityPhase::Running);
    tick(&mut rig, 1);
    assert_eq!(phase(&rig, ID), ActivityPhase::Waiting);
    let sources: Vec<_> = rig
        .m
        .drain_transitions()
        .into_iter()
        .filter(|t| t.harness_id == ID)
        .map(|t| t.source)
        .collect();
    assert_eq!(sources, vec![TransitionSource::L2bScreen]);
}

#[test]
fn a_single_torn_read_does_not_count() {
    let mut rig = Rig::new();
    spawn_running(&mut rig, ID);
    read(&mut rig, ID, ScreenReading::Waiting);
    tick(&mut rig, 1);
    read(&mut rig, ID, ScreenReading::Running);
    tick(&mut rig, 1);
    read(&mut rig, ID, ScreenReading::Waiting);
    tick(&mut rig, 1);
    assert_eq!(phase(&rig, ID), ActivityPhase::Running);
}

#[test]
fn stays_waiting_while_output_keeps_arriving() {
    let mut rig = Rig::new();
    spawn_running(&mut rig, ID);
    read(&mut rig, ID, ScreenReading::Waiting);
    tick(&mut rig, 2);
    for i in 0..5 {
        let chunk = format!("clock redraw 12:0{i}");
        rig.m.record_output(ID, Some(&chunk), rig.now);
        tick(&mut rig, 1);
    }
    assert_eq!(phase(&rig, ID), ActivityPhase::Waiting);
}

#[test]
fn waiting_to_running_after_two_running_reads() {
    let mut rig = Rig::new();
    spawn_running(&mut rig, ID);
    read(&mut rig, ID, ScreenReading::Waiting);
    tick(&mut rig, 2);
    read(&mut rig, ID, ScreenReading::Running);
    tick(&mut rig, 1);
    assert_eq!(phase(&rig, ID), ActivityPhase::Waiting);
    tick(&mut rig, 1);
    assert_eq!(phase(&rig, ID), ActivityPhase::Running);
}

#[test]
fn unknown_reads_leave_l2b_l2a_behaviour_as_before() {
    let mut rig = Rig::new();
    spawn_running(&mut rig, ID);
    rig.m.record_output(ID, Some("\nPassword: "), rig.now);
    tick(&mut rig, 2);
    assert_eq!(phase(&rig, ID), ActivityPhase::Waiting);
    // Not screen-set, so output that drains the prompt still wakes it.
    let chunk = format!("\n{}", "welcome ".repeat(60));
    rig.m.record_output(ID, Some(&chunk), rig.now);
    assert_eq!(phase(&rig, ID), ActivityPhase::Running);
    tick(&mut rig, 9);
    assert_eq!(phase(&rig, ID), ActivityPhase::Idle);
}

#[test]
fn releases_a_screen_owned_waiting_after_enough_unrecognised_ticks() {
    let mut rig = Rig::new();
    spawn_running(&mut rig, ID);
    read(&mut rig, ID, ScreenReading::Waiting);
    tick(&mut rig, 2);
    read(&mut rig, ID, ScreenReading::Unknown);
    tick(&mut rig, i64::from(SCREEN_UNKNOWN_RELEASE_TICKS) - 1);
    assert_eq!(phase(&rig, ID), ActivityPhase::Waiting);
    tick(&mut rig, 1);
    assert_eq!(phase(&rig, ID), ActivityPhase::Running);
}

#[test]
fn an_unknown_streak_is_broken_by_a_recognised_read() {
    let mut rig = Rig::new();
    spawn_running(&mut rig, ID);
    let release = i64::from(SCREEN_UNKNOWN_RELEASE_TICKS);
    read(&mut rig, ID, ScreenReading::Waiting);
    tick(&mut rig, 2);
    read(&mut rig, ID, ScreenReading::Unknown);
    tick(&mut rig, release - 1);
    read(&mut rig, ID, ScreenReading::Waiting);
    tick(&mut rig, 1);
    read(&mut rig, ID, ScreenReading::Unknown);
    tick(&mut rig, release - 1);
    assert_eq!(phase(&rig, ID), ActivityPhase::Waiting);
}

#[test]
fn a_submit_moves_a_screen_owned_waiting_to_running_at_once() {
    let mut rig = Rig::new();
    spawn_running(&mut rig, ID);
    read(&mut rig, ID, ScreenReading::Waiting);
    tick(&mut rig, 2);
    rig.m.record_input(ID, "hello", false, rig.now);
    assert_eq!(phase(&rig, ID), ActivityPhase::Waiting);
    rig.m.record_input(ID, "\r", false, rig.now);
    assert_eq!(phase(&rig, ID), ActivityPhase::Running);
    rig.m.record_output(ID, Some("x"), rig.now);
    assert_eq!(phase(&rig, ID), ActivityPhase::Running);
}

#[test]
fn ignores_screen_reads_for_an_authoritative_harness() {
    let mut rig = Rig::new();
    spawn_running(&mut rig, ID);
    rig.m.attach_authoritative_source(ID);
    read(&mut rig, ID, ScreenReading::Waiting);
    tick(&mut rig, 3);
    assert_eq!(phase(&rig, ID), ActivityPhase::Running);
}

#[test]
fn leaves_permission_and_exited_alone() {
    let mut rig = Rig::new();
    spawn_running(&mut rig, "s_p");
    rig.m.set_permission_from_adapter(
        "s_p",
        TransitionSource::L2c1ClaudePermission,
        Some("Bash"),
        None,
        None,
        rig.now,
    );
    read(&mut rig, "s_p", ScreenReading::Waiting);
    spawn_running(&mut rig, "s_e");
    rig.m.exited("s_e", Some(0), rig.now);
    read(&mut rig, "s_e", ScreenReading::Waiting);
    tick(&mut rig, 3);
    assert_eq!(phase(&rig, "s_p"), ActivityPhase::Permission);
    assert_eq!(phase(&rig, "s_e"), ActivityPhase::Exited);
}

// NOT PORTED: readScreenPhaseNow / is unknown when unregistered, unscreened
// or the snapshot is null — it tests the webview's `harnessInput` registry
// (xterm rawScreen snapshot), which has no daemon counterpart.
// NOT PORTED: readScreenPhaseNow / never snapshots a kind without the
// screenPhase capability — same: `harnessInput.register` and a `rawScreen`
// mock are webview-only.
