//! Port of `app/src/harnessEventsHermes.test.ts`,
//! `harnessEventsHermes.idle.test.ts` and `hermesWork.test.ts`, plus the
//! smoke tests the translator started with. The reducer table
//! (`hermesHookAdapter.test.ts`) is in `hermes_hook_tests.rs`.
//!
//! `hermesCapture.test.ts` and `hermesProfiles.test.ts` exercise session
//! capture and profile handling, which stay in the webview; nothing in them
//! reaches `harness_phase`.

#![allow(clippy::assert_is_empty)]

use skein_harness::claude::background::BackgroundKind;

use super::super::background_tasks::BackgroundTaskInput;
use super::super::test_support::Rig;
use super::super::types::{ActivityPhase, TransitionSource};
use super::hermes::{HermesHookOutcome, HermesTranslator, RECENTLY_ENDED_MS};
use crate::agent_api::hermes_event::HarnessHermesHook;
use crate::harness_work_hermes::HermesWorkEvent;

const ID: &str = "hh";

#[allow(clippy::unnecessary_wraps)]
fn s(v: &str) -> Option<String> {
    Some(v.to_owned())
}

/// `base` of the TS tables: a CLI `pre_llm_call` of session `S`, turn `T`.
fn base() -> HarnessHermesHook {
    HarnessHermesHook {
        room_id: "r".to_owned(),
        harness_id: ID.to_owned(),
        event: "pre_llm_call".to_owned(),
        session_id: s("S"),
        turn_id: s("T"),
        platform: s("cli"),
        ..HarnessHermesHook::default()
    }
}

fn with_event(event: &str) -> HarnessHermesHook {
    HarnessHermesHook {
        event: event.to_owned(),
        ..base()
    }
}

struct Fx {
    rig: Rig,
    t: HermesTranslator,
    id: &'static str,
}

/// `setup()`: spawn the harness, then create the binding (attach).
fn setup_for(id: &'static str, bound: Option<&str>) -> Fx {
    let mut rig = Rig::new();
    rig.spawned(id);
    let mut t = HermesTranslator::new(bound.map(str::to_owned));
    let now = rig.now;
    t.attached(&mut rig.m, id, now);
    Fx { rig, t, id }
}

fn setup() -> Fx {
    setup_for(ID, None)
}

impl Fx {
    fn handle(&mut self, hook: &HarnessHermesHook) -> HermesHookOutcome {
        let now = self.rig.now;
        self.t.apply_hook(&mut self.rig.m, self.id, hook, now)
    }

    fn work(&mut self, e: &HermesWorkEvent) {
        let now = self.rig.now;
        self.t.apply_work(&mut self.rig.m, self.id, e, now);
    }

    fn dispose(&mut self) {
        let now = self.rig.now;
        self.t.detached(&mut self.rig.m, self.id, now);
    }

    fn phase(&self) -> ActivityPhase {
        self.rig.m.get(self.id).expect("record").phase
    }

    fn subagents_working(&self) -> usize {
        self.rig.m.subagents().working_count(self.id)
    }

    fn tasks_working(&self) -> usize {
        self.rig.m.background_tasks().working_count(self.id)
    }

    fn delegated_count(&self) -> u32 {
        self.rig.m.get(self.id).expect("record").delegated_count
    }
}

fn sub_start(id: &str, description: Option<&str>, initial: bool) -> HermesWorkEvent {
    HermesWorkEvent::SubagentStart {
        agent_id: id.to_owned(),
        description: description.map(str::to_owned),
        initial,
    }
}

fn sub_end(id: &str) -> HermesWorkEvent {
    HermesWorkEvent::SubagentEnd {
        agent_id: id.to_owned(),
    }
}

fn bg_start(id: &str, command: &str, initial: bool) -> HermesWorkEvent {
    HermesWorkEvent::BackgroundStart {
        task_id: id.to_owned(),
        command: s(command),
        initial,
    }
}

fn bg_end(id: &str) -> HermesWorkEvent {
    HermesWorkEvent::BackgroundEnd {
        task_id: id.to_owned(),
    }
}

// --- smoke tests the translator started with -----------------------------

#[test]
fn session_start_runs_and_records_launch_signal() {
    let mut fx = setup();
    let hook = HarnessHermesHook {
        platform: s("cli"),
        ..with_event("on_session_start")
    };
    fx.rig.advance(100);
    let out = fx.handle(&hook);
    assert_eq!(fx.phase(), ActivityPhase::Running);
    assert_eq!(
        fx.rig.m.get(ID).expect("record").launch_signal_at,
        Some(fx.rig.now)
    );
    assert_eq!(out.session_started, Some(("S".to_owned(), true)));
    let sources: Vec<_> = fx
        .rig
        .m
        .drain_transitions()
        .into_iter()
        .map(|t| t.source)
        .collect();
    assert!(sources.contains(&TransitionSource::L2c3HermesRunning));
}

#[test]
fn session_end_waits_and_reports_turn_end() {
    let mut fx = setup();
    fx.handle(&with_event("on_session_start"));
    let out = fx.handle(&with_event("on_session_end"));
    assert_eq!(fx.phase(), ActivityPhase::Waiting);
    assert!(out.turn_ended);
}

// --- harnessEventsHermes.test.ts: "Hermes hook binding" -------------------

#[test]
fn drives_running_permission_waiting_and_the_action_commands() {
    let mut fx = setup();
    assert!(fx.rig.m.get(ID).expect("record").authoritative);
    let out = fx.handle(&with_event("on_session_start"));
    assert_eq!(fx.phase(), ActivityPhase::Running);
    // onSessionCaptured(["S"]) is the outcome's session_started.
    assert_eq!(out.session_started, Some(("S".to_owned(), true)));
    // NOT PORTED: calls[0] == hermes_actions_attach {harnessId, roomId,
    // sessionId, hermesProfile, resumed} -- mocked `invoke` plumbing, the
    // Rust API exposes only `session_started`/`attached_for` (asserted).
    assert_eq!(fx.t.attached_for(), Some("S"));
    fx.handle(&HarnessHermesHook {
        surface: s("cli"),
        tool_call_id: s("c"),
        tool_name: s("terminal"),
        ..with_event("pre_approval_request")
    });
    assert_eq!(fx.phase(), ActivityPhase::Permission);
    fx.handle(&with_event("pre_llm_call"));
    assert_eq!(fx.phase(), ActivityPhase::Permission);
    fx.handle(&HarnessHermesHook {
        surface: s("cli"),
        tool_call_id: s("c"),
        ..with_event("post_approval_response")
    });
    assert_eq!(fx.phase(), ActivityPhase::Running);
    let out = fx.handle(&HarnessHermesHook {
        completed: Some(true),
        ..with_event("on_session_end")
    });
    assert_eq!(fx.phase(), ActivityPhase::Waiting);
    // calls.last == hermes_actions_turn_end is the outcome's turn_ended.
    assert!(out.turn_ended);
    fx.dispose();
    // NOT PORTED: calls contain hermes_actions_detach -- mocked invoke.
    assert!(!fx.rig.m.get(ID).expect("record").authoritative);
}

#[test]
fn attaches_a_resumed_session_immediately_with_resumed_true() {
    let fx = setup_for(ID, Some("OLD"));
    // The TS asserts the hermes_actions_attach call args {sessionId: "OLD",
    // resumed: true}; the Rust equivalent is the bound session being the
    // attached one from construction (the `resumed` flag is client plumbing).
    assert_eq!(fx.t.attached_for(), Some("OLD"));
}

#[test]
fn a_session_id_from_the_hook_claims_it_and_stops_this_harness_s_capture_poll() {
    let mut fx = setup();
    let out = fx.handle(&HarnessHermesHook {
        session_id: s("HOOK"),
        ..with_event("on_session_start")
    });
    // NOT PORTED: the capture poll (startHermesCapture timers, the claim
    // invoke, the other harness's capture) is webview code. What the Rust
    // side owes it is the hook's session id as the claim to make.
    assert_eq!(out.session_started, Some(("HOOK".to_owned(), true)));
    assert_eq!(fx.t.attached_for(), Some("HOOK"));
}

#[test]
fn a_child_s_approval_answered_after_the_turn_ended_settles_to_waiting() {
    let mut fx = setup();
    fx.handle(&with_event("on_session_start"));
    fx.handle(&HarnessHermesHook {
        child_session_id: s("C"),
        ..with_event("subagent_start")
    });
    fx.handle(&with_event("on_session_end"));
    // #628: the child still runs, so the end defers
    assert_eq!(fx.phase(), ActivityPhase::Running);
    let child = HarnessHermesHook {
        surface: s("cli"),
        session_id: s("C"),
        tool_call_id: s("k"),
        ..base()
    };
    fx.handle(&HarnessHermesHook {
        event: "pre_approval_request".to_owned(),
        ..child.clone()
    });
    assert_eq!(fx.phase(), ActivityPhase::Permission);
    fx.handle(&HarnessHermesHook {
        event: "post_approval_response".to_owned(),
        ..child
    });
    assert_eq!(fx.phase(), ActivityPhase::Waiting);
}

#[test]
fn leaves_spawning_on_two_ready_screen_reads_without_a_hook() {
    let mut fx = setup();
    assert_eq!(fx.phase(), ActivityPhase::Spawning);
    let now = fx.rig.now;
    assert!(fx.t.poll_launch(&mut fx.rig.m, ID, true, now));
    assert_eq!(fx.phase(), ActivityPhase::Spawning);
    assert!(!fx.t.poll_launch(&mut fx.rig.m, ID, true, now));
    assert_eq!(fx.phase(), ActivityPhase::Waiting);
}

#[test]
fn defers_a_turn_end_while_a_delegated_child_runs_and_waits_after_its_wake_turn() {
    let mut fx = setup();
    fx.handle(&with_event("on_session_start"));
    fx.handle(&HarnessHermesHook {
        child_session_id: s("C"),
        ..with_event("subagent_start")
    });
    assert_eq!(fx.subagents_working(), 1);
    fx.handle(&with_event("on_session_end"));
    assert_eq!(fx.phase(), ActivityPhase::Running);
    assert!(
        fx.rig
            .m
            .get(ID)
            .expect("record")
            .delegation_deferred_at
            .is_some()
    );
    // a child hook never touches the parent phase
    fx.handle(&HarnessHermesHook {
        session_id: s("C"),
        platform: s("subagent"),
        ..with_event("on_session_end")
    });
    assert_eq!(fx.phase(), ActivityPhase::Running);
    fx.handle(&HarnessHermesHook {
        child_session_id: s("C"),
        ..with_event("subagent_stop")
    });
    assert_eq!(fx.subagents_working(), 0);
    fx.handle(&with_event("pre_llm_call"));
    fx.handle(&with_event("on_session_end"));
    assert_eq!(fx.phase(), ActivityPhase::Waiting);
    assert_eq!(
        fx.rig.m.get(ID).expect("record").delegation_deferred_at,
        None
    );
}

#[test]
fn attaches_the_work_stream_and_applies_a_background_process_from_it() {
    let mut fx = setup();
    fx.handle(&with_event("on_session_start"));
    // NOT PORTED: the hermes_work_attach call and its {harnessId, sessionId,
    // resumed} args, and the hermes_work_detach call on dispose -- mocked
    // invoke plumbing. The events the channel delivers are applied below.
    fx.work(&bg_start("p1", "sleep 9", false));
    fx.handle(&with_event("on_session_end"));
    assert_eq!(fx.phase(), ActivityPhase::Running);
    fx.work(&bg_end("p1"));
    assert_eq!(fx.tasks_working(), 0);
    fx.dispose();
}

#[test]
fn clears_the_old_session_s_delegated_work_on_a_session_change_and_on_dispose() {
    let mut fx = setup();
    fx.handle(&with_event("on_session_start"));
    fx.handle(&HarnessHermesHook {
        child_session_id: s("C"),
        ..with_event("subagent_start")
    });
    let now = fx.rig.now;
    fx.rig.m.background_tasks_mut().record(
        ID,
        BackgroundTaskInput {
            task_id: "p1".to_owned(),
            kind: BackgroundKind::Bash,
            description: None,
            command: None,
            timeout_ms: None,
            persistent: false,
            agent_id: None,
        },
        false,
        now,
    );
    assert_eq!(fx.subagents_working(), 1);
    fx.handle(&HarnessHermesHook {
        session_id: s("S2"),
        ..with_event("on_session_start")
    });
    assert_eq!(fx.subagents_working(), 0);
    assert_eq!(fx.tasks_working(), 0);
    fx.handle(&HarnessHermesHook {
        session_id: s("S2"),
        child_session_id: s("D"),
        ..with_event("subagent_start")
    });
    assert_eq!(fx.subagents_working(), 1);
    fx.dispose();
    assert_eq!(fx.subagents_working(), 0);
}

// --- harnessEventsHermes.idle.test.ts -------------------------------------

#[test]
fn finishes_a_hook_started_subagent_whose_stop_was_lost_and_the_deferral_settles() {
    let mut fx = setup_for("hi", None);
    let on = |event: &str| HarnessHermesHook {
        harness_id: "hi".to_owned(),
        event: event.to_owned(),
        ..base()
    };
    fx.handle(&on("on_session_start"));
    fx.handle(&HarnessHermesHook {
        child_session_id: s("C"),
        ..on("subagent_start")
    });
    fx.handle(&on("on_session_end"));
    assert_eq!(fx.phase(), ActivityPhase::Running);
    assert_eq!(fx.subagents_working(), 1);

    fx.work(&HermesWorkEvent::DelegationsIdle);
    assert_eq!(fx.subagents_working(), 0);

    // subscribeTransitions starts after this point.
    fx.rig.m.drain_transitions();
    fx.rig.advance(61_000);
    assert_eq!(fx.phase(), ActivityPhase::Waiting);
    let sources: Vec<_> = fx
        .rig
        .m
        .drain_transitions()
        .into_iter()
        .filter(|t| t.harness_id == "hi")
        .map(|t| t.source)
        .collect();
    assert_eq!(sources, [TransitionSource::DelegationSettled]);
    fx.dispose();
}

// --- hermesWork.test.ts: "Hermes work stream mapping (#628)" --------------

#[test]
fn an_initial_subagent_is_listed_but_not_working_a_live_one_is() {
    let mut fx = setup_for("hw", None);
    fx.work(&sub_start("old", None, true));
    assert_eq!(fx.rig.m.subagents().live("hw").len(), 1);
    assert_eq!(fx.subagents_working(), 0);
    fx.work(&sub_start("new", Some("g"), false));
    assert_eq!(fx.subagents_working(), 1);
    fx.work(&sub_end("old"));
    fx.work(&sub_end("new"));
    assert!(fx.rig.m.subagents().live("hw").is_empty());
}

#[test]
fn an_initial_background_task_is_not_working_a_live_one_is_as_a_bash_task() {
    let mut fx = setup_for("hw", None);
    fx.work(&bg_start("a", "x", true));
    fx.work(&bg_start("b", "y", false));
    assert_eq!(fx.tasks_working(), 1);
    let kinds: Vec<_> = fx
        .rig
        .m
        .background_tasks()
        .live("hw")
        .iter()
        .map(|t| t.kind)
        .collect();
    assert_eq!(kinds, [BackgroundKind::Bash, BackgroundKind::Bash]);
    fx.work(&bg_end("a"));
    fx.work(&bg_end("b"));
    assert!(fx.rig.m.background_tasks().live("hw").is_empty());
}

#[test]
fn recently_ended_echo_ignores_a_live_start_within_the_window_counts_one_after_it() {
    let mut fx = setup_for("hw", None);
    fx.work(&sub_start("e", None, false));
    fx.work(&sub_end("e"));
    // The TS spies on reclaimDelegationFromAdapter; its only caller on this
    // path sits right after `note_subagent_started`, so `delegated_count` is
    // the observable that an echo did not reach it.
    let before = fx.delegated_count();
    fx.rig.advance(RECENTLY_ENDED_MS - 1);
    fx.work(&sub_start("e", None, false));
    assert_eq!(fx.subagents_working(), 0);
    assert_eq!(fx.delegated_count(), before);
    fx.rig.advance(2);
    fx.work(&sub_start("e", None, false));
    assert_eq!(fx.subagents_working(), 1);
    assert_eq!(fx.delegated_count(), before + 1);
}

#[test]
fn recently_ended_echo_an_initial_start_is_unaffected_and_clear_work_forgets_the_tombstone() {
    let mut fx = setup_for("hw", Some("S0"));
    fx.work(&sub_start("f", None, false));
    fx.work(&sub_end("f"));
    fx.work(&sub_start("f", None, true));
    assert_eq!(fx.rig.m.subagents().live("hw").len(), 1);
    // clearHermesWork: `clear_work` is private; a session change is the
    // translator's own caller of it (the old session's work is dropped).
    fx.handle(&HarnessHermesHook {
        harness_id: "hw".to_owned(),
        session_id: s("S1"),
        ..with_event("on_session_start")
    });
    fx.work(&sub_start("f", None, false));
    assert_eq!(fx.subagents_working(), 1);
}

#[test]
fn double_reporting_hook_and_disk_is_harmless() {
    let mut fx = setup_for("hw", None);
    fx.work(&sub_start("c", None, false));
    fx.work(&sub_start("c", None, false));
    assert_eq!(fx.subagents_working(), 1);
    assert_eq!(fx.delegated_count(), 1);
    fx.work(&sub_end("c"));
    fx.work(&sub_end("c"));
    fx.work(&sub_end("never"));
    fx.work(&bg_end("never"));
    assert_eq!(fx.subagents_working(), 0);
}
