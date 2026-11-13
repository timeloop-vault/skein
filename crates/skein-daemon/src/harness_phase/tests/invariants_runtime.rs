//! Port of `app/src/supervisor/runtime.test.ts` (#423): the runtime around
//! the pure supervisor, driven through the real machine. The TS module mocks
//! (`liveAttach`, `mailState`) are [`SupervisorFacts`]; `logBoth` is the
//! `message` of each [`Violation`] that `advance_supervised` returns.

use crate::harness_phase::invariants::{
    HarnessFacts, MailState, RECOVERY_MIN_INTERVAL_MS, SupervisorFacts,
};
use crate::harness_phase::test_support::Rig;
use crate::harness_phase::{
    ActivityPhase, TransitionSource, TurnSignal, TurnSignalKind, Violation,
};

/// `spawn()`: spawned, attached, running.
fn spawn(r: &mut Rig, id: &str) {
    r.spawned(id);
    r.m.attach_authoritative_source(id);
    r.m.set_running_from_adapter(id, TransitionSource::L2c1ClaudeAssistant, false, r.now);
}

/// The store's own paths re-grant authority when the adapter speaks, so the
/// stuck state the supervisor exists for is forged by writing the record.
fn forge_lost_authority(r: &mut Rig, id: &str) {
    if let Some(a) = r.m.store.get_mut(id) {
        a.authoritative = false;
    }
}

fn live_attach(id: &str) -> SupervisorFacts {
    let mut f = SupervisorFacts::default();
    f.harnesses.insert(
        id.to_owned(),
        HarnessFacts {
            live_attach: Some(true),
            ..HarnessFacts::default()
        },
    );
    f
}

fn matching<'a>(log: &'a [Violation], s: &str) -> Vec<&'a str> {
    log.iter()
        .map(|v| v.message.as_str())
        .filter(|l| l.contains(s))
        .collect()
}

#[test]
fn regrants_authority_to_a_speaking_adapter() {
    let mut r = Rig::new();
    let facts = live_attach("sv");
    let mut log = Vec::new();
    spawn(&mut r, "sv");
    r.m.detach_authoritative_source("sv", r.now);
    log.extend(r.advance_supervised(1000, &facts));
    r.m.adapter_delivered("sv", true, r.now);
    forge_lost_authority(&mut r, "sv");
    assert!(!r.m.get("sv").unwrap().authoritative);
    log.extend(r.advance_supervised(20_000, &facts));
    assert!(r.m.get("sv").unwrap().authoritative);
    assert_eq!(
        matching(&log, "code=adapter_without_authority violation").len(),
        1
    );
    assert_eq!(matching(&log, "recovery_outcome").len(), 1);
    assert!(matching(&log, "recovery_outcome")[0].contains("applied=true"));
    assert_eq!(r.m.supervisor_violations("sv").len(), 1);
}

#[test]
fn moves_a_stale_running_harness_to_waiting_with_the_supervisor_source() {
    let mut r = Rig::new();
    let facts = SupervisorFacts::default();
    spawn(&mut r, "sv");
    // An end of turn the store recorded but never acted on.
    let now = r.now;
    let a = r.m.store.get_mut("sv").unwrap();
    a.authoritative = false;
    a.last_turn_signal = Some(TurnSignal {
        kind: TurnSignalKind::End,
        at: now,
    });
    // The TS test subscribes after spawn().
    r.m.drain_transitions();
    let log = r.advance_supervised(30_000, &facts);
    let sources: Vec<_> =
        r.m.drain_transitions()
            .iter()
            .filter(|t| t.to == ActivityPhase::Waiting)
            .map(|t| t.source)
            .collect();
    assert_eq!(r.m.get("sv").unwrap().phase, ActivityPhase::Waiting);
    assert_eq!(sources, vec![TransitionSource::SupervisorRecovered]);
    assert_eq!(
        matching(&log, "code=ended_turn_not_waiting recover recovery=").len(),
        1
    );
}

#[test]
fn does_not_recover_an_old_end_of_turn_once_the_user_has_submitted_again() {
    let mut r = Rig::new();
    let facts = SupervisorFacts::default();
    let mut log = Vec::new();
    spawn(&mut r, "sv");
    let now = r.now;
    let a = r.m.store.get_mut("sv").unwrap();
    a.authoritative = false;
    a.last_turn_signal = Some(TurnSignal {
        kind: TurnSignalKind::End,
        at: now,
    });
    log.extend(r.advance_supervised(1000, &facts));
    r.m.record_input("sv", "next\r", false, r.now);
    log.extend(r.advance_supervised(30_000, &facts));
    assert_eq!(matching(&log, "code=ended_turn_not_waiting").len(), 0);
    assert_ne!(r.m.get("sv").unwrap().phase, ActivityPhase::Waiting);
}

#[test]
fn never_touches_permission_or_exited_harnesses() {
    let mut r = Rig::new();
    let facts = SupervisorFacts::default();
    spawn(&mut r, "p");
    r.m.detach_authoritative_source("p", r.now);
    r.m.set_waiting_from_adapter("p", TransitionSource::L2c1ClaudeEndTurn, r.now);
    r.m.set_permission_from_adapter(
        "p",
        TransitionSource::L2c1ClaudePermission,
        Some("Bash"),
        None,
        None,
        r.now,
    );
    spawn(&mut r, "e");
    r.m.detach_authoritative_source("e", r.now);
    r.m.set_waiting_from_adapter("e", TransitionSource::L2c1ClaudeEndTurn, r.now);
    r.m.exited("e", Some(0), r.now);
    let log = r.advance_supervised(30_000, &facts);
    assert_eq!(r.m.get("p").unwrap().phase, ActivityPhase::Permission);
    assert_eq!(r.m.get("e").unwrap().phase, ActivityPhase::Exited);
    assert_eq!(matching(&log, "code=ended_turn_not_waiting").len(), 0);
}

#[test]
fn caps_recovery_attempts_at_three_then_reports_exhaustion_once() {
    let mut r = Rig::new();
    let facts = live_attach("sv");
    let mut log = Vec::new();
    spawn(&mut r, "sv");
    // A recurring condition: authority is lost again after each repair.
    for _ in 0..8 {
        r.m.detach_authoritative_source("sv", r.now);
        log.extend(r.advance_supervised(1000, &facts));
        r.m.adapter_delivered("sv", true, r.now);
        forge_lost_authority(&mut r, "sv");
        log.extend(r.advance_supervised(RECOVERY_MIN_INTERVAL_MS + 2000, &facts));
    }
    assert_eq!(
        matching(&log, "code=adapter_without_authority recover recovery=").len(),
        3
    );
    assert_eq!(matching(&log, "recovery_exhausted").len(), 1);
}

#[test]
fn only_logs_a_detect_only_invariant() {
    let mut r = Rig::new();
    let mut facts = SupervisorFacts::default();
    spawn(&mut r, "sv");
    r.m.set_waiting_from_adapter("sv", TransitionSource::L2c1ClaudeEndTurn, r.now);
    facts.harnesses.insert(
        "sv".to_owned(),
        HarnessFacts {
            mail: Some(MailState {
                unread: 2,
                last_refusal: None,
                last_nudge_at: None,
            }),
            ..HarnessFacts::default()
        },
    );
    let log = r.advance_supervised(90_000, &facts);
    assert_eq!(matching(&log, "code=mail_held violation").len(), 1);
    assert_eq!(r.m.get("sv").unwrap().phase, ActivityPhase::Waiting);
    assert_eq!(matching(&log, " recover recovery=").len(), 0);
}

// NOT PORTED: is_idempotent — `startSupervisor()` registers one `onTick`
// listener and returns the same stop function on a second call. The Rust
// supervisor has no start/stop: `supervise` is a plain call the driver makes
// after `tick`, so there is no registration to repeat.
