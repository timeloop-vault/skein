//! Boundary-exact checks of the deferral timers. The arm/re-arm and disarm
//! cases that used to sit here are covered by `delegation.rs`.

use crate::harness_phase::constants::{DELEGATION_CEILING_MS, DELEGATION_SETTLE_MS};
use crate::harness_phase::subagents::SubagentInput;
use crate::harness_phase::{ActivityPhase, PhaseMachine, TransitionSource};

const H: &str = "h";

fn sub(id: &str) -> SubagentInput {
    SubagentInput {
        agent_id: id.to_owned(),
        agent_type: None,
        description: None,
    }
}

/// An authoritative, `running` harness with one live subagent and the
/// deferral armed by an end of turn at t = 1000.
fn deferred() -> PhaseMachine {
    let mut m = PhaseMachine::new();
    m.spawned(H, 0);
    m.attach_authoritative_source(H);
    m.set_running_from_adapter(H, TransitionSource::L2c1ClaudeAssistant, false, 500);
    m.subagents_mut().record(H, sub("a"), false, 600);
    m.note_subagent_started(H, 600);
    m.awaiting_prompt_from_adapter(H, TransitionSource::L2c1ClaudeEndTurn, 1000);
    m
}

#[test]
fn settle_timer_flushes_with_delegation_settled() {
    let mut m = deferred();
    m.subagents_mut().finish(H, "a");
    m.drain_transitions();
    m.tick(10_000); // working set seen empty
    assert_eq!(m.get(H).unwrap().delegation_emptied_at, Some(10_000));
    m.tick(10_000 + DELEGATION_SETTLE_MS - 1);
    assert_eq!(m.get(H).unwrap().phase, ActivityPhase::Running);
    m.tick(10_000 + DELEGATION_SETTLE_MS);
    assert_eq!(m.get(H).unwrap().phase, ActivityPhase::Waiting);
    let t = m.drain_transitions();
    assert_eq!(t.len(), 1);
    assert_eq!(t[0].source, TransitionSource::DelegationSettled);
}

#[test]
fn ceiling_presumes_silent_subagents_gone_with_delegation_ceiling() {
    let mut m = deferred();
    m.drain_transitions();
    m.tick(600 + DELEGATION_CEILING_MS - 1);
    assert_eq!(m.get(H).unwrap().phase, ActivityPhase::Running);
    m.tick(600 + DELEGATION_CEILING_MS);
    let a = m.get(H).unwrap();
    assert_eq!(a.phase, ActivityPhase::Waiting);
    assert_eq!(a.delegation_deferred_at, None);
    assert_eq!(a.delegated_count, 0);
    assert_eq!(m.subagents().working_count(H), 0);
    let t = m.drain_transitions();
    assert_eq!(t.len(), 1);
    assert_eq!(t[0].source, TransitionSource::DelegationCeiling);
}
