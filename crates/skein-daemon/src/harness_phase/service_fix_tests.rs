//! `PhaseService` review fixes (#732): clean restart, adapter live gates,
//! the Hermes binding rule, a late input, a panicking ticker.

use std::sync::Arc;
use std::sync::atomic::{AtomicI64, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use super::client_input::ClientPhaseInput;
use super::service::PhaseService;
use super::translate::hermes::HermesHookOutcome;
use super::types::ActivityPhase;
use super::{ScreenReading, SupervisorFacts};
use crate::agent_api::hermes_event::HarnessHermesHook;
use crate::harness_events_claude::ClaudeEvent;
use crate::harness_events_opencode::OpencodeEvent;
use crate::harness_work_hermes::HermesWorkEvent;

const T0: i64 = 1_000_000;

fn svc() -> (Arc<PhaseService>, Arc<AtomicI64>) {
    let now = Arc::new(AtomicI64::new(T0));
    let c = Arc::clone(&now);
    (
        PhaseService::with_clock(Arc::new(move || c.load(Ordering::SeqCst))),
        now,
    )
}

fn phase(s: &PhaseService, id: &str) -> ActivityPhase {
    s.get(id).unwrap().phase
}

fn sub_start() -> ClaudeEvent {
    ClaudeEvent::SubagentStart {
        agent_id: "a1".into(),
        agent_type: None,
        description: None,
        initial: false,
    }
}

fn hermes_subagent() -> HermesWorkEvent {
    HermesWorkEvent::SubagentStart {
        agent_id: "a1".into(),
        description: None,
        initial: false,
    }
}

// ---- 1: a restart starts clean ----------------------------------------

#[test]
fn a_respawn_forgets_the_old_subagents_so_the_end_of_turn_is_not_deferred() {
    let (s, _) = svc();
    s.spawned("h", "room-1", false);
    s.claude_attached("h");
    s.claude_event("h", &sub_start());
    assert_eq!(s.work_snapshot(["h"])["h"].subagents, 1);
    s.spawned("h", "room-1", false);
    assert_eq!(s.work_snapshot(["h"])["h"].subagents, 0);
    s.claude_attached("h");
    s.claude_event("h", &ClaudeEvent::AssistantTurn);
    assert_eq!(phase(&s, "h"), ActivityPhase::Running);
    s.claude_event("h", &ClaudeEvent::AwaitingPrompt);
    assert_eq!(phase(&s, "h"), ActivityPhase::Waiting);
}

#[test]
fn a_respawn_drops_the_stored_screen_reading_and_keeps_the_meta() {
    let (s, _) = svc();
    s.spawned("h", "room-1", false);
    s.client_input(
        "h",
        ClientPhaseInput::ScreenReading {
            reading: Some(ScreenReading::Waiting),
        },
    );
    s.with(|i, _| assert!(i.machine.has_screen_state("h")));
    s.spawned("h", "room-1", false);
    s.with(|i, _| {
        assert!(!i.machine.has_screen_state("h"));
        assert!(i.meta.contains_key("h"));
    });
    // The comparator still knows the id: a client row is compared, not skipped.
    s.shadow_client_transition("h", "a", "b", Some("l2a-idle"), T0);
    assert_eq!(s.shadow_stats().skipped, 0);
}

// ---- 3: the adapters' live gates --------------------------------------

#[test]
fn an_opencode_event_after_detach_changes_nothing() {
    let (s, _) = svc();
    s.spawned("h", "room-1", false);
    s.opencode_attached("h");
    s.opencode_detached("h");
    s.opencode_event("h", &OpencodeEvent::SessionBusy);
    assert_eq!(phase(&s, "h"), ActivityPhase::Spawning);
}

#[test]
fn an_opencode_event_after_a_restart_without_a_new_attach_changes_nothing() {
    let (s, _) = svc();
    s.spawned("h", "room-1", false);
    s.opencode_attached("h");
    s.spawned("h", "room-1", false);
    s.opencode_event("h", &OpencodeEvent::SessionBusy);
    assert_eq!(phase(&s, "h"), ActivityPhase::Spawning);
    s.with(|i, _| assert!(i.opencode.is_empty()));
}

#[test]
fn events_never_create_a_translator() {
    let (s, _) = svc();
    s.spawned("h", "room-1", false);
    s.opencode_event("h", &OpencodeEvent::SessionBusy);
    s.claude_event("h", &ClaudeEvent::AssistantTurn);
    assert_eq!(phase(&s, "h"), ActivityPhase::Spawning);
    s.with(|i, _| assert!(i.opencode.is_empty() && i.claude.is_empty()));
}

// ---- 4: the Hermes binding --------------------------------------------

#[test]
fn hermes_inputs_without_a_binding_are_dropped() {
    let (s, _) = svc();
    s.spawned("h", "room-1", false);
    let hook = HarnessHermesHook {
        room_id: "room-1".into(),
        harness_id: "h".into(),
        event: "pre_llm_call".into(),
        ..HarnessHermesHook::default()
    };
    assert_eq!(s.hermes_hook("h", &hook), HermesHookOutcome::default());
    s.hermes_work("h", &hermes_subagent());
    s.client_input(
        "h",
        ClientPhaseInput::HermesLaunchPoll {
            screen_waiting: true,
        },
    );
    assert_eq!(phase(&s, "h"), ActivityPhase::Spawning);
    assert_eq!(s.work_snapshot(["h"])["h"].subagents, 0);
    s.with(|i, _| assert!(i.hermes.is_empty()));
}

#[test]
fn a_new_hermes_binding_disposes_the_old_one_first() {
    let (s, _) = svc();
    s.spawned("h", "room-1", false);
    s.hermes_attached("h", None);
    s.hermes_work("h", &hermes_subagent());
    assert_eq!(s.work_snapshot(["h"])["h"].subagents, 1);
    s.hermes_attached("h", Some("s2".into()));
    assert_eq!(s.work_snapshot(["h"])["h"].subagents, 0);
    assert!(s.get("h").unwrap().authoritative);
}

// ---- 7: a late input does not resurrect --------------------------------

#[test]
fn a_late_exit_for_a_forgotten_harness_is_ignored() {
    let (s, _) = svc();
    s.spawned("h", "room-1", false);
    s.forget("h");
    s.exited("h", Some(0));
    assert!(s.get("h").is_none());
    s.exited("never-seen", None);
    assert!(s.get("never-seen").is_none());
}

#[test]
fn the_comparator_does_not_learn_an_id_from_a_forgotten_harness_transition() {
    let (s, _) = svc();
    s.spawned("h", "room-1", false);
    s.forget("h");
    // Bypass the service guard: the machine itself still records a late exit.
    s.with(|i, now| i.machine.exited("h", Some(0), now));
    s.shadow_client_transition("h", "running", "exited", Some("pty-exit"), T0);
    assert_eq!(s.shadow_stats().skipped, 1);
    assert_eq!(s.shadow_stats().client_only, 0);
}

// ---- 8b: view reset ----------------------------------------------------

#[test]
fn view_reset_leaves_the_machine_alone_and_resyncs_the_comparator() {
    let (s, _) = svc();
    s.spawned("h", "room-1", false);
    s.record_output("h", "x");
    assert_eq!(phase(&s, "h"), ActivityPhase::Running);
    s.client_input("h", ClientPhaseInput::ViewReset);
    assert_eq!(phase(&s, "h"), ActivityPhase::Running);
    s.shadow_client_transition("h", "spawning", "running", Some("pty-output"), T0);
    assert_eq!(s.shadow_stats().skipped, 1);
}

// ---- 6: the ticker survives a panic ------------------------------------

#[test]
fn the_ticker_keeps_ticking_after_the_facts_closure_panics() {
    let (s, _) = svc();
    let calls = Arc::new(AtomicUsize::new(0));
    let c = Arc::clone(&calls);
    let handle = s
        .start_ticker_every(
            Duration::from_millis(5),
            Arc::new(move || {
                assert!(c.fetch_add(1, Ordering::SeqCst) != 0, "first call panics");
                SupervisorFacts::default()
            }),
        )
        .expect("thread");
    let deadline = Instant::now() + Duration::from_secs(5);
    while calls.load(Ordering::SeqCst) < 3 && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(calls.load(Ordering::SeqCst) >= 3);
    drop(s);
    handle
        .join()
        .expect("ticker exits once the service is gone");
}

#[test]
fn a_restart_after_a_view_reset_does_not_skip_the_spawning_row() {
    let (s, _) = svc();
    s.spawned("h", "room-1", false);
    s.client_input("h", ClientPhaseInput::ViewReset);
    s.spawned("h", "room-1", false);
    s.shadow_client_transition("h", "spawning", "waiting", Some("l2c1-claude-end-turn"), T0);
    assert_eq!(s.shadow_stats().skipped, 0);
}
