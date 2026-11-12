//! Port of `app/src/harnessEvents.localCommand.test.ts` (#463): a local
//! slash command is not a turn. The TS file is itself a downstream contract
//! test: the tail emits NO event for a local command's rows, so the
//! translator is never called for them. The decision itself is pinned in
//! `crates/skein-harness/src/claude/local_command.rs` (tracker tests,
//! e.g. `peer_message_after_local_command_is_a_turn_start` at :194) and in
//! `harness_events_claude/parse_tests.rs:314,324,345,390` (`local_command_*`).
//!
//! The TS `decide()` wraps `decideMailNudge(canSendPrompt(..))`, which is
//! webview logic (send gate, nudge bookkeeping) and not ported; its only
//! phase input is `atSafeStoppingPoint`, asserted here as `safe()`, which
//! is the `nudge: true` / `nudge: false` half of the original assertions.

use super::*;
use crate::harness_phase::machine::at_safe_stopping_point;
use crate::harness_phase::test_support::Rig;
use crate::harness_phase::types::ActivityPhase;

const ID: &str = "lc_1";

/// A Claude harness at the end of a turn (`waiting`) with its tail attached
/// and #215 injection on.
fn waiting() -> (Rig, ClaudeTranslator) {
    let mut rig = Rig::new();
    rig.spawned(ID);
    let mut t = ClaudeTranslator::new();
    t.attached(&mut rig.m, ID, rig.now);
    rig.m.set_injected(ID, true);
    t.apply(&mut rig.m, ID, &ClaudeEvent::AssistantTurn, rig.now);
    t.apply(&mut rig.m, ID, &ClaudeEvent::AwaitingPrompt, rig.now);
    rig.m.adapter_delivered(ID, true, rig.now);
    (rig, t)
}

fn phase(rig: &Rig) -> ActivityPhase {
    rig.m.get(ID).expect("record").phase
}

fn safe(rig: &Rig) -> bool {
    at_safe_stopping_point(rig.m.get(ID).expect("record"))
}

#[test]
fn a_local_command_leaves_a_waiting_harness_waiting_and_its_mail_deliverable() {
    let (rig, _t) = waiting();
    assert_eq!(phase(&rig), ActivityPhase::Waiting);

    // The local command's rows produce NO ClaudeEvent, so nothing is sent.

    assert_eq!(phase(&rig), ActivityPhase::Waiting);
    assert!(safe(&rig));
}

#[test]
fn held_mail_goes_out_once_the_harness_is_back_at_a_stopping_point() {
    let (mut rig, mut t) = waiting();

    // The #463 failure shape, for contrast.
    let prompt = ClaudeEvent::UserPrompt {
        task_notification: false,
    };
    t.apply(&mut rig.m, ID, &prompt, rig.now);
    assert_eq!(phase(&rig), ActivityPhase::Running);
    assert!(!safe(&rig));

    t.apply(&mut rig.m, ID, &ClaudeEvent::AssistantTurn, rig.now);
    t.apply(&mut rig.m, ID, &ClaudeEvent::AwaitingPrompt, rig.now);
    assert_eq!(phase(&rig), ActivityPhase::Waiting);
    assert!(safe(&rig));
}
