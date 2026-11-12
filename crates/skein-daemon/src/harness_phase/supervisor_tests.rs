//! Port of `app/src/harnessActivity.supervisor.test.ts` (#423): the facts
//! the supervisor reads and its two guarded recovery methods. One test per
//! TS case, same name in `snake_case`, same assertions in the same order.
//! `vi.setSystemTime(t)` is [`at`]: the rig clock jumps to `t`.

#![allow(clippy::unnecessary_wraps)]

use super::test_support::Rig;
use super::{ActivityPhase, AdapterEvent, TransitionSource, TurnSignal, TurnSignalKind};

fn at(rig: &mut Rig, t: i64) {
    rig.skip(t - rig.now);
}

fn work(at: i64) -> Option<TurnSignal> {
    Some(TurnSignal {
        kind: TurnSignalKind::Work,
        at,
    })
}

#[test]
fn records_last_turn_signal_even_when_the_harness_is_not_authoritative() {
    let mut r = Rig::new();
    r.spawned("a");
    assert_eq!(r.m.get("a").unwrap().last_turn_signal, None);
    at(&mut r, 1_000);
    r.m.set_running_from_adapter("a", TransitionSource::L2c1ClaudeAssistant, false, r.now);
    assert_eq!(r.m.get("a").unwrap().last_turn_signal, work(1_000));
    at(&mut r, 2_000);
    r.m.awaiting_prompt_from_adapter("a", TransitionSource::L2c1ClaudeEndTurn, r.now);
    assert_eq!(
        r.m.get("a").unwrap().last_turn_signal,
        Some(TurnSignal {
            kind: TurnSignalKind::End,
            at: 2_000
        })
    );
    r.m.forget("a");
}

#[test]
fn records_a_work_signal_that_permission_then_ignores() {
    let mut r = Rig::new();
    r.spawned("a");
    r.m.set_permission_from_adapter(
        "a",
        TransitionSource::L2c1ClaudePermission,
        Some("Bash"),
        None,
        None,
        r.now,
    );
    at(&mut r, 3_000);
    r.m.set_running_from_adapter("a", TransitionSource::L2c1ClaudeAssistant, false, r.now);
    assert_eq!(r.m.get("a").unwrap().phase, ActivityPhase::Permission);
    assert_eq!(r.m.get("a").unwrap().last_turn_signal, work(3_000));
    r.m.forget("a");
}

#[test]
fn stamps_authority_lost_at_on_detach_phase_since_on_a_real_change_permission_at_while_in_permission()
 {
    let mut r = Rig::new();
    at(&mut r, 10_000);
    r.spawned("a");
    r.m.attach_authoritative_source("a");
    assert_eq!(r.m.get("a").unwrap().authority_lost_at, None);
    at(&mut r, 11_000);
    r.m.set_permission_from_adapter(
        "a",
        TransitionSource::L2c1ClaudePermission,
        Some("Bash"),
        None,
        None,
        r.now,
    );
    assert_eq!(r.m.get("a").unwrap().phase_since, 11_000);
    assert_eq!(r.m.get("a").unwrap().permission_at, Some(11_000));
    at(&mut r, 12_000);
    r.m.set_permission_from_adapter(
        "a",
        TransitionSource::L2c1ClaudePermission,
        Some("Bash"),
        None,
        None,
        r.now,
    );
    assert_eq!(r.m.get("a").unwrap().phase_since, 11_000);
    assert_eq!(r.m.get("a").unwrap().permission_at, Some(12_000));
    r.m.release_permission("a", TransitionSource::AdapterDetached, r.now);
    assert_eq!(r.m.get("a").unwrap().permission_at, None);
    at(&mut r, 13_000);
    r.m.detach_authoritative_source("a", r.now);
    assert_eq!(r.m.get("a").unwrap().authority_lost_at, Some(13_000));
    r.m.forget("a");
}

#[test]
fn stamps_last_adapter_event_on_every_adapter_delivered_call_including_no_ops() {
    let mut r = Rig::new();
    r.spawned("a");
    r.m.attach_authoritative_source("a");
    at(&mut r, 20_000);
    r.m.adapter_delivered("a", true, r.now);
    at(&mut r, 21_000);
    r.m.adapter_delivered("a", false, r.now);
    assert_eq!(
        r.m.get("a").unwrap().last_adapter_event,
        Some(AdapterEvent {
            at: 21_000,
            restores_authority: false
        })
    );
    r.m.forget("a");
}

#[test]
fn supervisor_regrant_authority_applies_only_when_authority_is_lost_and_nothing_else_explains_it() {
    let mut r = Rig::new();
    r.spawned("a");
    r.m.attach_authoritative_source("a");
    assert!(!r.m.supervisor_regrant_authority("a")); // already authoritative
    r.m.detach_authoritative_source("a", r.now);
    assert!(r.m.supervisor_regrant_authority("a"));
    assert!(r.m.get("a").unwrap().authoritative);
    assert!(!r.m.supervisor_regrant_authority("nope"));

    r.m.detach_authoritative_source("a", r.now);
    r.m.exited("a", Some(0), r.now);
    assert!(!r.m.supervisor_regrant_authority("a"));
    assert!(!r.m.get("a").unwrap().authoritative);
    r.m.forget("a");
}

#[test]
fn supervisor_regrant_authority_refuses_while_a_watchdog_diagnosis_stands() {
    let mut r = Rig::new();
    r.spawned("a");
    r.m.attach_authoritative_source("a");
    r.m.record_input("a", "\r", false, r.now);
    r.advance(60_000);
    assert!(r.m.get("a").unwrap().adapter_silent);
    assert!(r.m.get("a").unwrap().authority_lost_at.is_some());
    assert!(!r.m.supervisor_regrant_authority("a"));
    r.m.forget("a");
}

#[test]
fn supervisor_set_waiting_moves_running_and_idle_and_nothing_else() {
    let mut r = Rig::new();
    r.spawned("running");
    r.m.set_running_from_adapter(
        "running",
        TransitionSource::L2c1ClaudeAssistant,
        false,
        r.now,
    );
    assert!(r.m.supervisor_set_waiting("running", r.now));
    assert_eq!(r.m.get("running").unwrap().phase, ActivityPhase::Waiting);
    assert!(!r.m.supervisor_set_waiting("running", r.now)); // waiting

    r.spawned("spawning");
    assert!(!r.m.supervisor_set_waiting("spawning", r.now));

    r.spawned("permission");
    r.m.set_permission_from_adapter(
        "permission",
        TransitionSource::L2c1ClaudePermission,
        None,
        None,
        None,
        r.now,
    );
    assert!(!r.m.supervisor_set_waiting("permission", r.now));
    assert_eq!(
        r.m.get("permission").unwrap().phase,
        ActivityPhase::Permission
    );

    r.spawned("exited");
    r.m.exited("exited", Some(0), r.now);
    assert!(!r.m.supervisor_set_waiting("exited", r.now));
    assert_eq!(r.m.get("exited").unwrap().phase, ActivityPhase::Exited);

    for id in ["running", "spawning", "permission", "exited"] {
        r.m.forget(id);
    }
}

#[test]
fn supervisor_set_waiting_reports_its_own_transition_source() {
    let mut r = Rig::new();
    r.spawned("a");
    r.m.set_running_from_adapter("a", TransitionSource::L2c1ClaudeAssistant, false, r.now);
    // The TS test subscribes after the running transition.
    r.m.drain_transitions();
    r.m.supervisor_set_waiting("a", r.now);
    let seen: Vec<_> = r.m.drain_transitions().iter().map(|t| t.source).collect();
    assert_eq!(seen, vec![TransitionSource::SupervisorRecovered]);
    r.m.forget("a");
}

// NOT PORTED: on_tick_calls_listeners_with_the_ticks_now_survives_a_throwing_listener_and_unsubscribes
// — `onTick` is a listener list on the TS store's setInterval. The Rust
// machine has no listeners: `tick(now)` is a plain call and the driver calls
// `supervise(&facts, now)` after it, so there is nothing to subscribe to,
// no listener that can throw and nothing to unsubscribe.
