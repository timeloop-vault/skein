//! Port of `app/src/harnessActivityCore.test.ts`: `phaseSnapshot` (#356) and
//! `atSafeStoppingPoint` (#381). A bare machine stands in for the store.

use std::collections::BTreeMap;

use crate::harness_phase::types::HarnessActivity;
use crate::harness_phase::{ActivityPhase, PhaseMachine, at_safe_stopping_point};

fn mk_activity(phase: ActivityPhase) -> HarnessActivity {
    HarnessActivity::fresh(phase, None, 0)
}

fn with_deferral(phase: ActivityPhase) -> HarnessActivity {
    HarnessActivity {
        delegation_deferred_at: Some(123),
        ..mk_activity(phase)
    }
}

fn machine(entries: &[(&str, ActivityPhase)]) -> PhaseMachine {
    let mut m = PhaseMachine::new();
    for (id, phase) in entries {
        m.insert(id, mk_activity(*phase));
    }
    m
}

#[test]
fn phase_snapshot_is_empty_for_an_empty_store() {
    assert_eq!(PhaseMachine::new().phase_snapshot(), BTreeMap::new());
}

#[test]
fn phase_snapshot_maps_every_harness_id_to_its_own_phase() {
    let m = machine(&[
        ("h1", ActivityPhase::Running),
        ("h2", ActivityPhase::Waiting),
    ]);
    assert_eq!(
        m.phase_snapshot(),
        BTreeMap::from([
            ("h1".to_owned(), ActivityPhase::Running),
            ("h2".to_owned(), ActivityPhase::Waiting),
        ])
    );
}

#[test]
fn phase_snapshot_includes_every_phase_as_is_exited_and_permission_included() {
    let m = machine(&[
        ("h1", ActivityPhase::Exited),
        ("h2", ActivityPhase::Permission),
    ]);
    assert_eq!(
        m.phase_snapshot(),
        BTreeMap::from([
            ("h1".to_owned(), ActivityPhase::Exited),
            ("h2".to_owned(), ActivityPhase::Permission),
        ])
    );
}

#[test]
fn phase_snapshot_a_harness_never_spawned_here_is_simply_absent_not_guessed_at() {
    let m = machine(&[("h1", ActivityPhase::Idle)]);
    let result = m.phase_snapshot();
    assert_eq!(result.keys().cloned().collect::<Vec<_>>(), vec!["h1"]);
    assert!(!result.contains_key("h2"));
}

#[test]
fn at_safe_stopping_point_waiting_is_always_a_stopping_point() {
    assert!(at_safe_stopping_point(&mk_activity(ActivityPhase::Waiting)));
}

#[test]
fn at_safe_stopping_point_running_with_an_armed_deferral_is_a_stopping_point() {
    assert!(at_safe_stopping_point(&with_deferral(
        ActivityPhase::Running
    )));
}

#[test]
fn at_safe_stopping_point_running_with_no_deferral_armed_is_not_a_stopping_point() {
    assert!(!at_safe_stopping_point(&mk_activity(
        ActivityPhase::Running
    )));
}

#[test]
fn at_safe_stopping_point_permission_is_never_a_stopping_point_even_with_a_deferral() {
    assert!(!at_safe_stopping_point(&with_deferral(
        ActivityPhase::Permission
    )));
}

#[test]
fn at_safe_stopping_point_phase_is_not_a_stopping_point_deferral_or_not() {
    for phase in [
        ActivityPhase::Spawning,
        ActivityPhase::Idle,
        ActivityPhase::Exited,
    ] {
        assert!(!at_safe_stopping_point(&mk_activity(phase)), "{phase:?}");
        assert!(!at_safe_stopping_point(&with_deferral(phase)), "{phase:?}");
    }
}
