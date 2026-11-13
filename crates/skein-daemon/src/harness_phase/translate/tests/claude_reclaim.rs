//! Port of `app/src/harnessEvents.reclaimDelegation.test.ts` (#634): a LIVE
//! subagent/background start flushed after the main end_turn pulls a
//! `waiting` harness back into the #277 deferral, silently.
//!
//! Not ported from the first case: the `statusLabel(...)` equality (webview
//! label text; asserted as phase `running` with one working subagent) and
//! `isNotifiable(classifyTransition(..))` (webview `harnessNotifyLogic.ts`;
//! the source it is evaluated on, `delegation-resumed`, is asserted).

use super::*;
use crate::harness_phase::test_support::Rig;
use crate::harness_phase::types::{ActivityPhase, TransitionSource, TurnSignal, TurnSignalKind};
use skein_harness::claude::background::{BackgroundKind, OutcomeStatus};

const ID: &str = "rc_1";

fn attached() -> (Rig, ClaudeTranslator) {
    let mut rig = Rig::new();
    rig.spawned(ID);
    let mut t = ClaudeTranslator::new();
    t.attached(&mut rig.m, ID, rig.now);
    t.apply(&mut rig.m, ID, &ClaudeEvent::AssistantTurn, rig.now);
    (rig, t)
}

fn waiting_harness() -> (Rig, ClaudeTranslator) {
    let (mut rig, mut t) = attached();
    t.apply(&mut rig.m, ID, &ClaudeEvent::AwaitingPrompt, rig.now);
    assert_eq!(phase(&rig), ActivityPhase::Waiting);
    (rig, t)
}

fn phase(rig: &Rig) -> ActivityPhase {
    rig.m.get(ID).expect("record").phase
}

fn sub(initial: bool) -> ClaudeEvent {
    ClaudeEvent::SubagentStart {
        agent_id: "a1".into(),
        agent_type: Some("explore".into()),
        description: None,
        initial,
    }
}

fn bg(initial: bool) -> ClaudeEvent {
    ClaudeEvent::BackgroundStart {
        task_id: "t1".into(),
        tool_use_id: "tu_t1".into(),
        task_kind: BackgroundKind::Bash,
        description: None,
        command: Some("sleep 999".into()),
        timeout_ms: None,
        persistent: false,
        auto_backgrounded: false,
        agent_id: None,
        initial,
    }
}

fn seen(rig: &mut Rig) -> Vec<(ActivityPhase, ActivityPhase, TransitionSource)> {
    rig.m
        .drain_transitions()
        .into_iter()
        .filter(|t| t.harness_id == ID)
        .map(|t| (t.from, t.to, t.source))
        .collect()
}

#[test]
fn live_subagent_start_after_a_zero_work_end_turn_running_deferral_armed_silent_then_settles() {
    let (mut rig, mut t) = waiting_harness();
    seen(&mut rig); // subscribe
    t.apply(&mut rig.m, ID, &sub(false), rig.now);
    let a = rig.m.get(ID).expect("record");
    assert_eq!(a.phase, ActivityPhase::Running);
    assert!(a.delegation_deferred_at.is_some());
    assert_eq!(rig.m.subagents().working_count(ID), 1); // "delegating · 1 agent"
    let mut log = seen(&mut rig);
    assert_eq!(
        log,
        vec![(
            ActivityPhase::Waiting,
            ActivityPhase::Running,
            TransitionSource::DelegationResumed
        )]
    );

    let end = ClaudeEvent::SubagentEnd {
        agent_id: "a1".into(),
        agent_type: Some("explore".into()),
        description: None,
    };
    t.apply(&mut rig.m, ID, &end, rig.now);
    rig.advance(61_000);
    assert_eq!(phase(&rig), ActivityPhase::Waiting);
    log.extend(seen(&mut rig));
    assert_eq!(log[1].2, TransitionSource::DelegationSettled);
}

#[test]
fn live_background_start_does_the_same() {
    let (mut rig, mut t) = waiting_harness();
    seen(&mut rig);
    t.apply(&mut rig.m, ID, &bg(false), rig.now);
    assert_eq!(phase(&rig), ActivityPhase::Running);
    assert!(
        rig.m
            .get(ID)
            .expect("record")
            .delegation_deferred_at
            .is_some()
    );
    let mut log = seen(&mut rig);
    assert_eq!(
        log.iter().map(|s| s.2).collect::<Vec<_>>(),
        vec![TransitionSource::DelegationResumed]
    );
    let end = ClaudeEvent::BackgroundEnd {
        task_id: "t1".into(),
        task_kind: BackgroundKind::Bash,
        agent_id: None,
        status: OutcomeStatus::Completed,
        exit_code: None,
    };
    t.apply(&mut rig.m, ID, &end, rig.now);
    rig.advance(61_000);
    assert_eq!(phase(&rig), ActivityPhase::Waiting);
    log.extend(seen(&mut rig));
    assert_eq!(log[1].2, TransitionSource::DelegationSettled);
}

#[test]
fn initial_attach_starts_never_reclaim() {
    let (mut rig, mut t) = waiting_harness();
    t.apply(&mut rig.m, ID, &sub(true), rig.now);
    t.apply(&mut rig.m, ID, &bg(true), rig.now);
    assert_eq!(phase(&rig), ActivityPhase::Waiting);
    assert_eq!(rig.m.get(ID).expect("record").delegation_deferred_at, None);
}

#[test]
fn permission_a_work_signal_or_a_non_authoritative_harness_are_left_alone() {
    let (mut p, mut tp) = waiting_harness();
    apply_hook_permission(&mut p.m, ID, Some("Bash"), None, None, p.now);
    tp.apply(&mut p.m, ID, &sub(false), p.now);
    assert_eq!(phase(&p), ActivityPhase::Permission);

    let (mut w, mut tw) = waiting_harness();
    let now = w.now;
    w.m.store.get_mut(ID).expect("record").last_turn_signal = Some(TurnSignal {
        kind: TurnSignalKind::Work,
        at: now,
    });
    tw.apply(&mut w.m, ID, &sub(false), now);
    assert_eq!(phase(&w), ActivityPhase::Waiting);
    let e = w.m.store.get_mut(ID).expect("record");
    e.last_turn_signal = Some(TurnSignal {
        kind: TurnSignalKind::End,
        at: now,
    });
    e.authoritative = false;
    // Any event re-asserts authority (adapter_delivered), so call the gate directly.
    w.m.reclaim_delegation_from_adapter(ID, now);
    assert_eq!(phase(&w), ActivityPhase::Waiting);
}
