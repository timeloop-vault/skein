//! A change to a field of the wire view reaches the stream in the same call,
//! with no tick and no driver (#761): nothing sweeps the machine once a
//! second any more, so the service marks the harness itself
//! (`Inner::mark_if_view_changed`).

use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};

use serde_json::Value;

use crate::harness_events_claude::ClaudeEvent;
use crate::harness_phase::client_input::ClientPhaseInput;
use crate::harness_phase::service::PhaseService;
use crate::harness_phase::stream::HARNESS_PHASE_EVENT;
use crate::harness_phase::test_support::RecordingSink;

const T0: i64 = 1_000_000;

struct Rig {
    svc: Arc<PhaseService>,
    sink: Arc<RecordingSink>,
    now: Arc<AtomicI64>,
}

impl Rig {
    fn new() -> Self {
        let now = Arc::new(AtomicI64::new(T0));
        let c = Arc::clone(&now);
        let (svc, sink) =
            PhaseService::test_with_clock_and_sink(Arc::new(move || c.load(Ordering::SeqCst)));
        svc.spawned("h", "room-1", false);
        Self { svc, sink, now }
    }

    fn advance(&self, ms: i64) {
        self.now.fetch_add(ms, Ordering::SeqCst);
    }

    fn updates(&self) -> Vec<Value> {
        self.svc.flush_outputs();
        self.sink
            .events()
            .into_iter()
            .inspect(|(e, _)| assert_eq!(e, HARNESS_PHASE_EVENT))
            .map(|(_, p)| serde_json::from_str::<Value>(&p).unwrap())
            .filter(|v| v["harnessId"] == "h")
            .collect()
    }

    /// Run `call` and return the one update it emitted, which must have no
    /// transition (the point of these tests).
    fn one_update(&self, call: impl FnOnce(&PhaseService)) -> Value {
        let before = self.updates().len();
        call(&self.svc);
        let all = self.updates();
        assert_eq!(all.len(), before + 1, "exactly one update from the call");
        let last = all.last().unwrap().clone();
        assert_eq!(
            last["transitions"].as_array().map(Vec::len),
            Some(0),
            "no phase transition expected: {last}"
        );
        last
    }

    fn input(data: &str) -> impl FnOnce(&PhaseService) + use<> {
        let data = data.to_owned();
        move |s| {
            s.client_input(
                "h",
                ClientPhaseInput::Input {
                    data,
                    empty_composer: false,
                },
            );
        }
    }
}

/// The reviewer's scenario: an end of turn with a subagent outstanding arms
/// the deferral (the view carries `end`), then a work signal arrives while
/// the phase is already `running`: only `lastTurnSignal` changes.
#[test]
fn a_work_signal_while_running_emits_the_new_turn_signal() {
    let r = Rig::new();
    r.svc.claude_attached("h");
    r.svc.claude_event("h", &ClaudeEvent::AssistantTurn);
    r.svc.claude_event(
        "h",
        &ClaudeEvent::SubagentStart {
            agent_id: "a1".into(),
            agent_type: None,
            description: None,
            initial: false,
        },
    );
    r.advance(10);
    r.svc.claude_event("h", &ClaudeEvent::AwaitingPrompt);
    let armed = r.updates().last().unwrap().clone();
    assert_eq!(armed["view"]["phase"], "running");
    assert_eq!(armed["view"]["lastTurnSignal"]["kind"], "end");
    r.advance(10);
    let u = r.one_update(|s| {
        s.claude_event(
            "h",
            &ClaudeEvent::ToolUseStart {
                name: "Read".into(),
            },
        );
    });
    assert_eq!(u["view"]["phase"], "running");
    assert_eq!(u["view"]["lastTurnSignal"]["kind"], "work");
}

#[test]
fn a_delegation_count_change_emits() {
    let r = Rig::new();
    r.svc.claude_attached("h");
    r.svc.claude_event("h", &ClaudeEvent::AssistantTurn);
    let _ = r.updates();
    let u = r.one_update(|s| s.with_for("h", |m, now| m.machine.note_subagent_started("h", now)));
    assert_eq!(u["view"]["delegatedCount"], 1);
    let u = r.one_update(|s| s.with_for("h", |m, _| m.machine.note_user_prompt("h")));
    assert_eq!(u["view"]["delegatedCount"], 0);
}

#[test]
fn a_keystroke_and_a_submit_emit() {
    let r = Rig::new();
    r.svc.record_output("h", "hello");
    let u = r.one_update(Rig::input("a"));
    assert_eq!(u["view"]["hasUserInput"], true);
    assert!(u["view"]["lastSubmitAt"].is_null());
    r.advance(5);
    let u = r.one_update(Rig::input("\r"));
    assert_eq!(u["view"]["lastSubmitAt"], T0 + 5);
    r.advance(5);
    let u = r.one_update(Rig::input("\r"));
    assert_eq!(u["view"]["lastSubmitAt"], T0 + 10);
}

#[test]
fn a_seam_submit_emits() {
    let r = Rig::new();
    r.svc.record_output("h", "hello");
    let _ = r.updates();
    let u = r.one_update(|s| s.prompt_submitted("h"));
    assert_eq!(u["view"]["lastSubmitAt"], T0);
}

#[test]
fn a_second_permission_request_with_another_tool_emits() {
    let r = Rig::new();
    r.svc.claude_attached("h");
    r.svc.claude_event("h", &ClaudeEvent::AssistantTurn);
    r.svc
        .claude_permission_hook("h", Some("Bash"), Some("worker"), None);
    let first = r.updates().last().unwrap().clone();
    assert_eq!(first["view"]["phase"], "permission");
    assert_eq!(first["view"]["permissionTool"], "Bash");
    let u = r.one_update(|s| s.claude_permission_hook("h", Some("Edit"), Some("other"), None));
    assert_eq!(u["view"]["permissionTool"], "Edit");
    assert_eq!(u["view"]["permissionAgentType"], "other");
}
