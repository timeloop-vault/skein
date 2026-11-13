//! `PhaseService` (#732): the driver around the pure machine, on a fake
//! clock.

use std::sync::atomic::{AtomicI64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::agent_api::hermes_event::HarnessHermesHook;
use crate::harness_events_claude::ClaudeEvent;
use crate::harness_events_opencode::OpencodeEvent;
use crate::harness_phase::client_input::ClientPhaseInput;
use crate::harness_phase::constants::{ADAPTER_SILENT_AFTER_MS, IDLE_AFTER_MS, TICK_INTERVAL_MS};
use crate::harness_phase::service::PhaseService;
use crate::harness_phase::sinks::{ClaudePhaseSink, Tee};
use crate::harness_phase::types::ActivityPhase;
use crate::harness_phase::{ScreenReading, SessionSwitch, SupervisorFacts};
use crate::harness_work_hermes::HermesWorkEvent;
use crate::stream_sink::StreamSink;

const T0: i64 = 1_000_000;

struct Rig {
    svc: Arc<PhaseService>,
    now: Arc<AtomicI64>,
}

impl Rig {
    fn new() -> Self {
        let now = Arc::new(AtomicI64::new(T0));
        let c = Arc::clone(&now);
        let svc = PhaseService::test_with_clock(Arc::new(move || c.load(Ordering::SeqCst)));
        Self { svc, now }
    }

    fn advance(&self, ms: i64) {
        self.now.fetch_add(ms, Ordering::SeqCst);
    }

    /// Advance in whole tick intervals, ticking at each, like the thread does.
    fn run(&self, ms: i64) {
        let mut left = ms;
        while left > 0 {
            let step = left.min(TICK_INTERVAL_MS);
            self.advance(step);
            self.svc.tick_once(&SupervisorFacts::default());
            left -= step;
        }
    }

    fn spawn(&self, id: &str) {
        self.svc.spawned(id, "room-1", false);
    }

    fn phase(&self, id: &str) -> ActivityPhase {
        self.svc.get(id).unwrap().phase
    }

    fn input(&self, id: &str, i: ClientPhaseInput) {
        self.svc.client_input(id, i);
    }
}

#[test]
fn spawned_output_then_tick_to_idle_produces_rust_transitions() {
    let r = Rig::new();
    r.spawn("h");
    assert_eq!(r.phase("h"), ActivityPhase::Spawning);
    r.svc.record_output("h", "hello");
    assert_eq!(r.phase("h"), ActivityPhase::Running);
    r.run(IDLE_AFTER_MS + TICK_INTERVAL_MS);
    assert_eq!(r.phase("h"), ActivityPhase::Idle);
}

#[test]
fn claude_events_reach_the_machine_through_the_translator() {
    let r = Rig::new();
    r.spawn("h");
    // Before an attach the translator is not live: events are dropped.
    r.svc.claude_event(
        "h",
        &ClaudeEvent::UserPrompt {
            task_notification: false,
        },
    );
    assert_eq!(r.phase("h"), ActivityPhase::Spawning);
    r.svc.claude_attached("h");
    assert!(r.svc.get("h").unwrap().authoritative);
    r.svc.claude_event(
        "h",
        &ClaudeEvent::UserPrompt {
            task_notification: false,
        },
    );
    assert_eq!(r.phase("h"), ActivityPhase::Running);
    r.advance(500);
    r.svc.claude_event("h", &ClaudeEvent::AwaitingPrompt);
    assert_eq!(r.phase("h"), ActivityPhase::Waiting);
    r.svc.claude_detached("h");
    assert!(!r.svc.get("h").unwrap().authoritative);
}

#[test]
fn opencode_events_reach_the_machine() {
    let r = Rig::new();
    r.spawn("h");
    r.svc.opencode_attached("h");
    r.svc.opencode_event("h", &OpencodeEvent::SessionBusy);
    assert_eq!(r.phase("h"), ActivityPhase::Running);
    r.svc.opencode_event("h", &OpencodeEvent::SessionIdle);
    assert_eq!(r.phase("h"), ActivityPhase::Waiting);
    r.svc.opencode_detached("h");
    assert!(!r.svc.get("h").unwrap().authoritative);
}

#[test]
fn hermes_hook_and_work_reach_the_machine() {
    let r = Rig::new();
    r.spawn("h");
    r.svc.hermes_attached("h", None);
    let hook = HarnessHermesHook {
        room_id: "room-1".into(),
        harness_id: "h".into(),
        event: "pre_llm_call".into(),
        ..HarnessHermesHook::default()
    };
    let out = r.svc.hermes_hook("h", &hook);
    assert!(!out.turn_ended);
    assert_eq!(r.phase("h"), ActivityPhase::Running);
    r.svc.hermes_work(
        "h",
        &HermesWorkEvent::SubagentStart {
            agent_id: "a1".into(),
            description: None,
            initial: false,
        },
    );
    assert_eq!(r.svc.work_snapshot(["h"])["h"].subagents, 1);
    r.svc.hermes_detached("h");
    assert_eq!(r.svc.work_snapshot(["h"])["h"].subagents, 0);
}

#[test]
fn claude_permission_hook_enters_permission() {
    let r = Rig::new();
    r.spawn("h");
    r.svc
        .claude_permission_hook("h", Some("Bash"), Some("reviewer"), Some("ag1"));
    let a = r.svc.get("h").unwrap();
    assert_eq!(a.phase, ActivityPhase::Permission);
    assert_eq!(a.permission_tool.as_deref(), Some("Bash"));
    assert_eq!(a.permission_agent_id.as_deref(), Some("ag1"));
}

#[test]
fn client_input_input_records_user_input() {
    let r = Rig::new();
    r.spawn("h");
    r.input(
        "h",
        ClientPhaseInput::Input {
            data: "a".into(),
            empty_composer: false,
        },
    );
    assert!(r.svc.get("h").unwrap().has_user_input);
}

#[test]
fn client_input_prompt_submitted_stamps_the_submit() {
    let r = Rig::new();
    r.spawn("h");
    r.input("h", ClientPhaseInput::PromptSubmitted);
    assert_eq!(r.svc.get("h").unwrap().last_submit_at, Some(T0));
}

#[test]
fn client_input_mute_induced_output_drops_the_next_output() {
    let r = Rig::new();
    r.spawn("h");
    r.input("h", ClientPhaseInput::MuteInducedOutput);
    r.svc.record_output("h", "redraw");
    assert_eq!(r.svc.get("h").unwrap().last_output_at, None);
    assert_eq!(r.phase("h"), ActivityPhase::Spawning);
}

#[test]
fn client_input_screen_reading_drives_the_screen_phase() {
    let r = Rig::new();
    r.spawn("h");
    r.svc.record_output("h", "x");
    r.input(
        "h",
        ClientPhaseInput::ScreenReading {
            reading: Some(ScreenReading::Waiting),
        },
    );
    r.run(3 * TICK_INTERVAL_MS);
    assert_eq!(r.phase("h"), ActivityPhase::Waiting);
}

#[test]
fn client_input_hermes_launch_poll_stands_in_for_the_launch_signal() {
    let r = Rig::new();
    r.spawn("h");
    // A poll without a binding is dropped (fix 4), so bind first.
    r.svc.hermes_attached("h", None);
    for _ in 0..2 {
        r.input(
            "h",
            ClientPhaseInput::HermesLaunchPoll {
                screen_waiting: true,
            },
        );
    }
    assert_eq!(r.phase("h"), ActivityPhase::Waiting);
    assert!(r.svc.get("h").unwrap().launch_signal_at.is_some());
}

#[test]
fn client_input_launch_signal_moves_spawning_to_waiting() {
    let r = Rig::new();
    r.spawn("h");
    r.input("h", ClientPhaseInput::LaunchSignal);
    assert_eq!(r.phase("h"), ActivityPhase::Waiting);
}

#[test]
fn client_input_session_switched_moves_running_to_waiting() {
    let r = Rig::new();
    r.spawn("h");
    r.svc.record_output("h", "x");
    r.input(
        "h",
        ClientPhaseInput::SessionSwitched {
            switch: SessionSwitch::Clear,
        },
    );
    assert_eq!(r.phase("h"), ActivityPhase::Waiting);
}

#[test]
fn client_input_recover_unheard_silence_regrants_authority() {
    let r = Rig::new();
    r.spawn("h");
    r.input("h", ClientPhaseInput::LaunchSignal);
    r.svc.claude_attached("h");
    r.input("h", ClientPhaseInput::PromptSubmitted);
    r.run(ADAPTER_SILENT_AFTER_MS + TICK_INTERVAL_MS);
    let degraded = r.svc.get("h").unwrap();
    assert!(degraded.adapter_silent && !degraded.authoritative);
    r.input("h", ClientPhaseInput::RecoverUnheardSilence);
    let a = r.svc.get("h").unwrap();
    assert!(a.authoritative && a.silence_recovered && !a.adapter_silent);
}

#[test]
fn respawn_replaces_the_record_and_the_old_adapter_state() {
    let r = Rig::new();
    r.spawn("h");
    r.svc.claude_attached("h");
    r.svc.exited("h", Some(1));
    assert_eq!(r.phase("h"), ActivityPhase::Exited);
    r.svc.spawned("h", "room-1", true);
    let a = r.svc.get("h").unwrap();
    assert_eq!(
        (a.phase, a.injected, a.authoritative),
        (ActivityPhase::Spawning, true, false)
    );
    // The old translator is gone: its events are dropped until re-attached.
    r.svc.claude_event("h", &ClaudeEvent::AssistantTurn);
    assert_eq!(r.phase("h"), ActivityPhase::Spawning);
}

#[test]
fn forget_clears_everything() {
    let r = Rig::new();
    r.spawn("h");
    r.svc.claude_attached("h");
    r.svc.opencode_attached("h");
    r.svc.hermes_attached("h", Some("s1".into()));
    r.svc.claude_event(
        "h",
        &ClaudeEvent::SubagentStart {
            agent_id: "a1".into(),
            agent_type: None,
            description: None,
            initial: false,
        },
    );
    assert_eq!(r.svc.work_snapshot(["h"])["h"].subagents, 1);
    r.svc.forget("h");
    assert!(r.svc.get("h").is_none());
    assert!(r.svc.phase_snapshot().is_empty());
    assert_eq!(r.svc.work_snapshot(["h"])["h"].subagents, 0);
    r.svc.with(|s, _| {
        assert!(s.claude.is_empty() && s.opencode.is_empty() && s.hermes.is_empty());
        assert!(s.meta.is_empty());
    });
}

#[test]
fn the_ticker_ticks_and_stops_when_the_service_is_dropped() {
    let r = Rig::new();
    let calls = Arc::new(AtomicUsize::new(0));
    let c = Arc::clone(&calls);
    let handle = r
        .svc
        .start_ticker_every(
            Duration::from_millis(5),
            Arc::new(move || {
                c.fetch_add(1, Ordering::SeqCst);
                SupervisorFacts::default()
            }),
        )
        .expect("thread");
    let deadline = Instant::now() + Duration::from_secs(5);
    while calls.load(Ordering::SeqCst) < 2 && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(calls.load(Ordering::SeqCst) >= 2);
    drop(r);
    handle
        .join()
        .expect("ticker exits once the service is gone");
}

// ---- sinks ------------------------------------------------------------

struct Recorder(Arc<Mutex<Vec<ClaudeEvent>>>);

impl StreamSink<ClaudeEvent> for Recorder {
    fn send(&self, item: ClaudeEvent) -> Result<(), String> {
        self.0.lock().unwrap().push(item);
        Ok(())
    }
}

struct Closed;

impl StreamSink<ClaudeEvent> for Closed {
    fn send(&self, _item: ClaudeEvent) -> Result<(), String> {
        Err("channel closed".into())
    }
}

fn phase_sink(r: &Rig) -> ClaudePhaseSink {
    ClaudePhaseSink {
        service: Arc::clone(&r.svc),
        harness_id: "h".into(),
    }
}

#[test]
fn tee_delivers_to_both_sinks() {
    let r = Rig::new();
    r.spawn("h");
    r.svc.claude_attached("h");
    let seen = Arc::new(Mutex::new(Vec::new()));
    let tee = Tee::new(Recorder(Arc::clone(&seen)), phase_sink(&r));
    assert!(tee.send(ClaudeEvent::AssistantTurn).is_ok());
    assert_eq!(seen.lock().unwrap().len(), 1);
    assert_eq!(r.phase("h"), ActivityPhase::Running);
}

#[test]
fn tee_reports_the_client_result_and_survives_a_failing_client_sink() {
    let r = Rig::new();
    r.spawn("h");
    r.svc.claude_attached("h");
    let tee = Tee::new(Closed, phase_sink(&r));
    assert_eq!(
        tee.send(ClaudeEvent::AssistantTurn),
        Err("channel closed".to_owned())
    );
    // The phase sink still heard it.
    assert_eq!(r.phase("h"), ActivityPhase::Running);
    assert!(tee.send(ClaudeEvent::AwaitingPrompt).is_err());
    assert_eq!(r.phase("h"), ActivityPhase::Waiting);
}
