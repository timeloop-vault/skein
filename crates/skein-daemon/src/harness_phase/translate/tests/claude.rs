// NOT PORTED (harnessEvents.test.ts, webview plumbing: `guardChannelHandler`
// wraps a Tauri Channel handler, and a Rust translator has no such wedge):
// keeps_handling_later_messages_after_an_earlier_one_throws,
// logs_the_harness_id_event_kind_and_error_message_once_per_throw,
// rate_limits_logging_to_the_first_5_throws_per_attach,
// truncates_a_long_stack_to_about_1500_chars.

use super::*;
use crate::harness_phase::types::ActivityPhase;

const T0: i64 = 1_000_000;

fn setup() -> (PhaseMachine, ClaudeTranslator) {
    let mut m = PhaseMachine::new();
    m.spawned("h", T0);
    let mut t = ClaudeTranslator::new();
    t.attached(&mut m, "h", T0);
    (m, t)
}

fn phase(m: &PhaseMachine) -> ActivityPhase {
    m.get("h").expect("record").phase
}

#[test]
fn assistant_turn_runs() {
    let (mut m, mut t) = setup();
    t.apply(&mut m, "h", &ClaudeEvent::AssistantTurn, T0 + 10);
    assert_eq!(phase(&m), ActivityPhase::Running);
}

#[test]
fn end_turn_without_work_waits() {
    let (mut m, mut t) = setup();
    t.apply(&mut m, "h", &ClaudeEvent::AssistantTurn, T0 + 10);
    t.apply(&mut m, "h", &ClaudeEvent::AwaitingPrompt, T0 + 20);
    assert_eq!(phase(&m), ActivityPhase::Waiting);
    let last = m.drain_transitions().pop().expect("transition");
    assert_eq!(last.source.as_str(), "l2c1-claude-end-turn");
}

#[test]
fn end_turn_with_live_subagent_defers() {
    let (mut m, mut t) = setup();
    t.apply(&mut m, "h", &ClaudeEvent::AssistantTurn, T0 + 10);
    let start = ClaudeEvent::SubagentStart {
        agent_id: "a1".into(),
        agent_type: None,
        description: None,
        initial: false,
    };
    t.apply(&mut m, "h", &start, T0 + 20);
    t.apply(&mut m, "h", &ClaudeEvent::AwaitingPrompt, T0 + 30);
    assert_eq!(phase(&m), ActivityPhase::Running);
    assert!(m.get("h").expect("record").delegation_deferred_at.is_some());
}

#[test]
fn closed_translator_drops_events() {
    let (mut m, mut t) = setup();
    t.detached(&mut m, "h", T0 + 5);
    t.apply(&mut m, "h", &ClaudeEvent::AssistantTurn, T0 + 10);
    assert_eq!(phase(&m), ActivityPhase::Spawning);
}
