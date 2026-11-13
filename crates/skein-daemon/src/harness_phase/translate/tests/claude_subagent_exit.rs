//! Port of `app/src/harnessEvents.subagentExit.test.ts` (#440): a subagent's
//! exit (`subagent_end`) releases the #277 delegation deferral, and with it
//! the held mail.
//!
//! Not ported (webview logic): `statusLabel(..)` strings, `canSendPrompt`
//! and `decideMailNudge`. Their phase inputs are asserted instead: the
//! label's `delegating · N agent` reading is `phase == Running` with
//! `subagents.working_count == N`, plain `running` / `waiting` is
//! `working_count == 0`, and `decide(..).nudge` is `at_safe_stopping_point`
//! (`nudge: true` iff true, given a passing gate). The `gate` `{ ok: true }`
//! assertion in the held-mail case is the send gate and is NOT PORTED.

use super::*;
use crate::harness_phase::constants::DELEGATION_SETTLE_MS;
use crate::harness_phase::machine::at_safe_stopping_point;
use crate::harness_phase::test_support::Rig;
use crate::harness_phase::types::{ActivityPhase, TransitionSource};

const ID: &str = "se_1";

/// A Claude harness mid-turn with its tail attached, injection on.
fn attached() -> (Rig, ClaudeTranslator) {
    let mut rig = Rig::new();
    rig.spawned(ID);
    let mut t = ClaudeTranslator::new();
    t.attached(&mut rig.m, ID, rig.now);
    rig.m.set_injected(ID, true);
    t.apply(&mut rig.m, ID, &ClaudeEvent::AssistantTurn, rig.now);
    (rig, t)
}

fn start_sub(rig: &mut Rig, t: &mut ClaudeTranslator) {
    let e = ClaudeEvent::SubagentStart {
        agent_id: "a1".into(),
        agent_type: Some("explore".into()),
        description: None,
        initial: false,
    };
    t.apply(&mut rig.m, ID, &e, rig.now);
}

fn end_sub(rig: &mut Rig, t: &mut ClaudeTranslator) {
    let e = ClaudeEvent::SubagentEnd {
        agent_id: "a1".into(),
        agent_type: Some("explore".into()),
        description: None,
    };
    t.apply(&mut rig.m, ID, &e, rig.now);
}

fn end_turn(rig: &mut Rig, t: &mut ClaudeTranslator) {
    t.apply(&mut rig.m, ID, &ClaudeEvent::AwaitingPrompt, rig.now);
}

fn phase(rig: &Rig) -> ActivityPhase {
    rig.m.get(ID).expect("record").phase
}

fn working(rig: &Rig) -> usize {
    rig.m.subagents().working_count(ID)
}

#[test]
fn subagent_end_after_a_deferred_end_of_turn_settles_to_waiting_after_delegation_settle_ms() {
    let (mut rig, mut t) = attached();
    start_sub(&mut rig, &mut t);
    end_turn(&mut rig, &mut t);
    assert_eq!(phase(&rig), ActivityPhase::Running);
    assert_eq!(working(&rig), 1); // "delegating · 1 agent"

    end_sub(&mut rig, &mut t);
    assert_eq!(working(&rig), 0);
    assert_eq!(phase(&rig), ActivityPhase::Running); // label "running"

    rig.m.drain_transitions(); // subscribe
    rig.advance(DELEGATION_SETTLE_MS + 1_000);
    assert_eq!(phase(&rig), ActivityPhase::Waiting);
    let sources: Vec<_> = rig
        .m
        .drain_transitions()
        .into_iter()
        .map(|t| t.source)
        .collect();
    assert_eq!(sources, vec![TransitionSource::DelegationSettled]);
    assert_eq!(working(&rig), 0); // label "waiting"
}

#[test]
fn the_main_sessions_own_next_end_of_turn_after_subagent_end_goes_straight_to_waiting() {
    let (mut rig, mut t) = attached();
    start_sub(&mut rig, &mut t);
    end_turn(&mut rig, &mut t);
    end_sub(&mut rig, &mut t);
    // The main session wakes to work the result, then ends its turn.
    t.apply(&mut rig.m, ID, &ClaudeEvent::AssistantTurn, rig.now);
    end_turn(&mut rig, &mut t);
    assert_eq!(phase(&rig), ActivityPhase::Waiting);
    assert_eq!(rig.m.get(ID).expect("record").delegation_deferred_at, None);
}

#[test]
fn held_mail_delivered_at_arming_381_and_still_deliverable_once_the_exit_settles_the_deferral() {
    let (mut rig, mut t) = attached();
    // Seed the proof-of-life the send gate needs, as a real tail would.
    rig.m.adapter_delivered(ID, true, rig.now);
    start_sub(&mut rig, &mut t);
    end_turn(&mut rig, &mut t);

    let safe = |rig: &Rig| at_safe_stopping_point(rig.m.get(ID).expect("record"));

    // While the deferral is armed the harness is a safe stopping point.
    assert_eq!(phase(&rig), ActivityPhase::Running);
    assert!(safe(&rig));

    // The exit settles the deferral to a plain `waiting`, which stays sendable.
    end_sub(&mut rig, &mut t);
    rig.advance(DELEGATION_SETTLE_MS + 1_000);
    assert_eq!(phase(&rig), ActivityPhase::Waiting);
    assert!(safe(&rig));
}

#[test]
fn without_a_subagent_end_the_deferral_never_settles_early_delegating_persists_past_the_settle_window()
 {
    let (mut rig, mut t) = attached();
    start_sub(&mut rig, &mut t);
    end_turn(&mut rig, &mut t);
    rig.advance(DELEGATION_SETTLE_MS + 1_000);
    // Pre-fix field symptom: no exit event, so the working set stays
    // non-empty and the label stays "delegating".
    assert_eq!(phase(&rig), ActivityPhase::Running);
    assert_eq!(working(&rig), 1); // "delegating · 1 agent"
}
