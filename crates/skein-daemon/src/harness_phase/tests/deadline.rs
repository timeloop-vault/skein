//! `PhaseMachine::next_deadline` (#761): one test per rule of the tick. Each
//! builds the state, asserts the deadline is the exact threshold instant,
//! that a tick one millisecond earlier changes nothing, and that a tick at
//! the deadline fires the expected transition.

use skein_harness::claude::background::BackgroundKind;

use crate::harness_phase::background_tasks::BackgroundTaskInput;
use crate::harness_phase::constants::{
    ADAPTER_SILENT_AFTER_MS, BACKGROUND_TASK_CEILING_MS, DELEGATION_CEILING_MS,
    DELEGATION_SETTLE_MS, IDLE_AFTER_MS, LAUNCH_SILENT_AFTER_MS, PATTERN_WAITING_AFTER_MS,
    TICK_INTERVAL_MS,
};
use crate::harness_phase::screen::SCREEN_UNKNOWN_RELEASE_TICKS;
use crate::harness_phase::subagents::SubagentInput;
use crate::harness_phase::{
    ActivityPhase, DegradedBy, PhaseMachine, ScreenReading, Transition, TransitionSource,
};

const H: &str = "h";

fn sub(id: &str) -> SubagentInput {
    SubagentInput {
        agent_id: id.to_owned(),
        agent_type: None,
        description: None,
    }
}

fn task(id: &str, kind: BackgroundKind, timeout_ms: Option<i64>) -> BackgroundTaskInput {
    BackgroundTaskInput {
        task_id: id.to_owned(),
        kind,
        description: None,
        command: None,
        timeout_ms,
        persistent: false,
        agent_id: None,
    }
}

/// Everything a tick may change that a reader can see.
fn state(m: &PhaseMachine) -> String {
    format!(
        "{:?}|{:?}|{}",
        m.get(H),
        m.background_tasks().live(H),
        m.subagents().working_count(H)
    )
}

/// Assert the deadline, that one millisecond earlier changes nothing, then
/// tick at the deadline and hand back what fired.
fn fire_at(m: &mut PhaseMachine, now: i64, deadline: i64) -> Vec<Transition> {
    assert_eq!(m.next_deadline(now), Some(deadline));
    m.drain_transitions();
    let before = state(m);
    m.tick(deadline - 1);
    assert_eq!(state(m), before, "tick before the deadline changed state");
    assert_eq!(m.drain_transitions(), []);
    m.tick(deadline);
    m.drain_transitions()
}

fn only(t: &[Transition]) -> (ActivityPhase, TransitionSource) {
    assert_eq!(t.len(), 1, "{t:?}");
    (t[0].to, t[0].source)
}

/// Authoritative, `running`, one live subagent, end of turn at 1000 arms the
/// deferral. The subagent started at 600.
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

/// A non-authoritative shell harness that went `running` on output at 100.
fn shell(chunk: &str) -> PhaseMachine {
    let mut m = PhaseMachine::new();
    m.spawned(H, 0);
    m.record_output(H, Some(chunk), 100);
    assert_eq!(m.get(H).unwrap().phase, ActivityPhase::Running);
    m
}

#[test]
fn l2a_idle() {
    let mut m = shell("building");
    let t = fire_at(&mut m, 100, 100 + IDLE_AFTER_MS);
    assert_eq!(only(&t), (ActivityPhase::Idle, TransitionSource::L2aIdle));
    assert_eq!(m.next_deadline(9_000), None);
}

#[test]
fn l2b_pattern() {
    let mut m = shell("[sudo] password for bob: ");
    let t = fire_at(&mut m, 100, 100 + PATTERN_WAITING_AFTER_MS);
    assert_eq!(
        only(&t),
        (ActivityPhase::Waiting, TransitionSource::L2bPattern)
    );
    assert_eq!(m.next_deadline(1_000), None);
}

#[test]
fn adapter_silent_after_a_submitted_prompt() {
    let mut m = PhaseMachine::new();
    m.spawned(H, 0);
    m.attach_authoritative_source(H);
    m.note_launch_signal(H, TransitionSource::L2c1ClaudeSessionStart, 500);
    m.record_input(H, "\r", false, 1_000);
    let t = fire_at(&mut m, 1_000, 1_000 + ADAPTER_SILENT_AFTER_MS);
    assert!(t.is_empty(), "waiting stays waiting: {t:?}");
    let a = m.get(H).unwrap();
    assert!(!a.authoritative);
    assert_eq!(a.degraded_by, Some(DegradedBy::AdapterSilent));
    // Handed to L2a, which has no output to age.
    assert_eq!(m.next_deadline(12_000), None);
}

#[test]
fn launch_silent_in_spawning() {
    let mut m = PhaseMachine::new();
    m.spawned(H, 0);
    m.attach_authoritative_source(H);
    let t = fire_at(&mut m, 0, LAUNCH_SILENT_AFTER_MS);
    assert_eq!(
        only(&t),
        (ActivityPhase::Running, TransitionSource::LaunchSilent)
    );
    // Now L2a measures from the fallback.
    assert_eq!(
        m.next_deadline(LAUNCH_SILENT_AFTER_MS),
        Some(LAUNCH_SILENT_AFTER_MS + IDLE_AFTER_MS)
    );
}

#[test]
fn settle_stamps_now_then_flushes_after_the_window() {
    let mut m = deferred();
    m.subagents_mut().finish(H, "a");
    m.drain_transitions();
    // Seen empty: the tick must run to stamp the mark.
    assert_eq!(m.next_deadline(5_000), Some(5_000));
    m.tick(5_000);
    assert_eq!(m.get(H).unwrap().delegation_emptied_at, Some(5_000));
    let t = fire_at(&mut m, 5_000, 5_000 + DELEGATION_SETTLE_MS);
    assert_eq!(
        only(&t),
        (ActivityPhase::Waiting, TransitionSource::DelegationSettled)
    );
    assert_eq!(m.next_deadline(70_000), None);
}

#[test]
fn a_stale_settle_mark_is_cleared_by_a_tick_now() {
    let mut m = deferred();
    m.subagents_mut().finish(H, "a");
    m.tick(5_000); // stamp
    m.subagents_mut().record(H, sub("b"), false, 6_000);
    m.note_subagent_started(H, 6_000);
    assert_eq!(m.get(H).unwrap().delegation_emptied_at, Some(5_000));
    assert_eq!(m.next_deadline(6_000), Some(6_000));
    m.tick(6_000);
    assert_eq!(m.get(H).unwrap().delegation_emptied_at, None);
    // Nothing left to write until the subagent ceiling.
    assert_eq!(m.next_deadline(6_000), Some(6_000 + DELEGATION_CEILING_MS));
}

#[test]
fn subagent_ceiling() {
    let mut m = deferred();
    let t = fire_at(&mut m, 1_000, 600 + DELEGATION_CEILING_MS);
    assert_eq!(
        only(&t),
        (ActivityPhase::Waiting, TransitionSource::DelegationCeiling)
    );
    assert_eq!(m.next_deadline(1_000_000), None);
}

#[test]
fn background_task_ceiling() {
    let mut m = PhaseMachine::new();
    m.spawned(H, 0);
    m.attach_authoritative_source(H);
    m.set_running_from_adapter(H, TransitionSource::L2c1ClaudeAssistant, false, 500);
    m.background_tasks_mut()
        .record(H, task("t", BackgroundKind::Bash, None), false, 600);
    m.awaiting_prompt_from_adapter(H, TransitionSource::L2c1ClaudeEndTurn, 1_000);
    assert!(m.get(H).unwrap().delegation_deferred_at.is_some());
    let t = fire_at(&mut m, 1_000, 1_000 + BACKGROUND_TASK_CEILING_MS);
    assert_eq!(
        only(&t),
        (ActivityPhase::Waiting, TransitionSource::DelegationCeiling)
    );
    assert_eq!(m.background_tasks().overdue_count(H), 1);
}

#[test]
fn monitor_expiry() {
    let mut m = PhaseMachine::new();
    m.spawned(H, 0);
    m.attach_authoritative_source(H);
    m.set_running_from_adapter(H, TransitionSource::L2c1ClaudeAssistant, false, 500);
    m.background_tasks_mut().record(
        H,
        task("mon", BackgroundKind::Monitor, Some(5_000)),
        false,
        600,
    );
    m.awaiting_prompt_from_adapter(H, TransitionSource::L2c1ClaudeEndTurn, 1_000);
    let t = fire_at(&mut m, 1_000, 5_600);
    assert_eq!(t, []);
    assert_eq!(m.background_tasks().live(H).len(), 0);
    // The expiry tick saw the set empty and stamped the settle window.
    assert_eq!(m.get(H).unwrap().delegation_emptied_at, Some(5_600));
    assert_eq!(m.next_deadline(5_600), Some(5_600 + DELEGATION_SETTLE_MS));
}

#[test]
fn work_watchdog_without_a_deferral() {
    let mut m = PhaseMachine::new();
    m.spawned(H, 0);
    m.attach_authoritative_source(H);
    m.set_running_from_adapter(H, TransitionSource::L2c1ClaudeAssistant, false, 500);
    m.subagents_mut().record(H, sub("a"), false, 600);
    m.note_subagent_started(H, 600);
    // last_work_at = max(turn signal 500, activity 600).
    let t = fire_at(&mut m, 600, 600 + DELEGATION_CEILING_MS);
    assert_eq!(
        only(&t),
        (ActivityPhase::Waiting, TransitionSource::WorkWatchdog)
    );
}

#[test]
fn a_deferral_under_permission_has_no_deadline() {
    let mut m = deferred();
    m.set_permission_from_adapter(
        H,
        TransitionSource::L2c1ClaudePermission,
        Some("Bash"),
        None,
        None,
        1_200,
    );
    assert_eq!(m.get(H).unwrap().phase, ActivityPhase::Permission);
    assert_eq!(m.next_deadline(1_200), None);
    m.drain_transitions();
    let before = state(&m);
    m.tick(1_200 + 3 * DELEGATION_CEILING_MS);
    assert_eq!(state(&m), before);
    assert_eq!(m.drain_transitions(), []);
}

/// A Hermes-style harness: `running` on output at 0 with a stored reading.
fn hermes(reading: ScreenReading) -> PhaseMachine {
    let mut m = PhaseMachine::new();
    m.spawned(H, 0);
    m.record_output(H, Some("banner"), 0);
    m.set_screen_reading(H, Some(reading));
    m
}

#[test]
fn screen_debounce_needs_two_samples_one_interval_apart() {
    let mut m = hermes(ScreenReading::Waiting);
    // Never stepped: step now.
    assert_eq!(m.next_deadline(0), Some(0));
    m.tick(0);
    assert_eq!(m.get(H).unwrap().phase, ActivityPhase::Running);
    let t = fire_at(&mut m, 0, TICK_INTERVAL_MS);
    assert_eq!(
        only(&t),
        (ActivityPhase::Waiting, TransitionSource::L2bScreen)
    );
}

#[test]
fn settled_screen_has_no_deadline() {
    let mut m = hermes(ScreenReading::Waiting);
    m.tick(0);
    m.tick(1_000); // waiting, screen-owned
    // Ownership then two agreeing samples, one interval apart each.
    assert_eq!(m.next_deadline(1_000), Some(2_000));
    m.tick(2_000);
    assert_eq!(m.next_deadline(2_000), Some(3_000));
    m.tick(3_000);
    assert_eq!(m.next_deadline(3_000), None);
    m.drain_transitions();
    let before = state(&m);
    m.tick(10_000);
    m.tick(500_000);
    assert_eq!(state(&m), before);
    assert_eq!(m.drain_transitions(), []);
    assert_eq!(m.next_deadline(500_000), None);
}

#[test]
fn screen_unknown_release_after_five_samples() {
    let mut m = hermes(ScreenReading::Waiting);
    for t in [0, 1_000, 2_000, 3_000] {
        m.tick(t);
    }
    m.set_screen_reading(H, Some(ScreenReading::Unknown));
    m.drain_transitions();
    let mut at = 3_000;
    for _ in 1..SCREEN_UNKNOWN_RELEASE_TICKS {
        at += TICK_INTERVAL_MS;
        assert_eq!(m.next_deadline(at - 1), Some(at));
        m.tick(at);
        assert_eq!(m.get(H).unwrap().phase, ActivityPhase::Waiting);
    }
    at += TICK_INTERVAL_MS;
    let t = fire_at(&mut m, at - 1, at);
    assert_eq!(
        only(&t),
        (ActivityPhase::Running, TransitionSource::L2bScreenCleared)
    );
}

#[test]
fn a_screen_without_readings_has_no_screen_deadline() {
    // Only L2a's own threshold, never a "now" for the screen step.
    let m = shell("building");
    assert_eq!(m.next_deadline(100), Some(100 + IDLE_AFTER_MS));
}

#[test]
fn settled_harnesses_have_no_deadline() {
    let mut m = shell("building");
    m.tick(100 + IDLE_AFTER_MS);
    assert_eq!(m.get(H).unwrap().phase, ActivityPhase::Idle);
    assert_eq!(m.next_deadline(9_000), None);

    let mut w = PhaseMachine::new();
    w.spawned(H, 0);
    w.attach_authoritative_source(H);
    w.note_launch_signal(H, TransitionSource::L2c1ClaudeSessionStart, 10);
    assert_eq!(w.get(H).unwrap().phase, ActivityPhase::Waiting);
    assert_eq!(w.next_deadline(10), None);

    let mut e = shell("x");
    e.exited(H, Some(0), 200);
    assert_eq!(e.next_deadline(200), None);

    assert_eq!(PhaseMachine::new().next_deadline(0), None);
}

/// The last screen step (at 7700) did not claim the harness; a `waiting`
/// reading arrives at 7800. The idle deadline (8100) falls between steps: the
/// tick there must not run L2a, because the step at 8700 claims the harness
/// (the old 1 Hz tick stepped first and skipped L2a). The harness must not
/// report an L2 deadline either; the screen rule then decides.
#[test]
fn a_reading_that_would_claim_blocks_l2a_between_steps() {
    let mut m = shell("building");
    m.tick(7_700);
    assert_eq!(m.get(H).unwrap().phase, ActivityPhase::Running);
    m.drain_transitions();
    m.set_screen_reading(H, Some(ScreenReading::Waiting));
    // The step instant, not the idle instant (100 + IDLE_AFTER_MS = 8_100).
    assert_eq!(m.next_deadline(7_800), Some(7_700 + TICK_INTERVAL_MS));
    m.tick(100 + IDLE_AFTER_MS);
    assert_eq!(m.get(H).unwrap().phase, ActivityPhase::Running);
    assert_eq!(m.drain_transitions(), []);
    assert_eq!(
        m.next_deadline(100 + IDLE_AFTER_MS),
        Some(7_700 + TICK_INTERVAL_MS)
    );
    m.tick(7_700 + TICK_INTERVAL_MS); // first sample
    assert_eq!(m.get(H).unwrap().phase, ActivityPhase::Running);
    m.tick(7_700 + 2 * TICK_INTERVAL_MS); // confirming sample
    let t = m.drain_transitions();
    assert_eq!(
        only(&t),
        (ActivityPhase::Waiting, TransitionSource::L2bScreen)
    );
}

/// The opposite direction: the last step claimed the harness, then the
/// reading went `unknown`. Until the next step runs the tick keeps reusing
/// the claim (no L2a), and the deadline is that step's instant.
#[test]
fn a_claim_is_reused_until_the_next_step_even_if_the_reading_went() {
    let mut m = PhaseMachine::new();
    m.spawned(H, 0);
    m.record_output(H, Some("building"), 0);
    m.set_screen_reading(H, Some(ScreenReading::Waiting));
    m.tick(7_500); // claims (first sample)
    m.set_screen_reading(H, Some(ScreenReading::Unknown));
    m.drain_transitions();
    m.tick(IDLE_AFTER_MS); // gated: the claim is reused, L2a is skipped
    assert_eq!(m.get(H).unwrap().phase, ActivityPhase::Running);
    assert_eq!(m.drain_transitions(), []);
    assert_eq!(
        m.next_deadline(IDLE_AFTER_MS),
        Some(7_500 + TICK_INTERVAL_MS)
    );
    m.tick(7_500 + TICK_INTERVAL_MS);
    let t = m.drain_transitions();
    assert_eq!(only(&t), (ActivityPhase::Idle, TransitionSource::L2aIdle));
}

#[test]
fn many_ticks_within_one_interval_count_one_screen_sample() {
    let mut m = hermes(ScreenReading::Waiting);
    for t in 0..TICK_INTERVAL_MS {
        m.tick(t);
    }
    assert_eq!(m.get(H).unwrap().phase, ActivityPhase::Running);
    assert!(
        m.drain_transitions()
            .iter()
            .all(|t| t.to != ActivityPhase::Waiting)
    );
    m.tick(TICK_INTERVAL_MS);
    assert_eq!(m.get(H).unwrap().phase, ActivityPhase::Waiting);
}
