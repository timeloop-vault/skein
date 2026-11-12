//! Port of `app/src/harnessActivity.background.test.ts` (#446, #441):
//! background tasks hold a main session's end of turn like subagents do, and
//! a running harness with outstanding work but no signal is presumed stale.
//!
//! The TS file drives the real `attachClaudeEvents` translator on a mocked
//! Channel; here the events go through the real
//! [`ClaudeTranslator::apply`]. `rig.advance` stands in for
//! `vi.advanceTimersByTime`.

use std::ops::{Deref, DerefMut};

use skein_harness::claude::background::{BackgroundKind, OutcomeStatus};

use super::constants::{BACKGROUND_TASK_CEILING_MS, DELEGATION_CEILING_MS, DELEGATION_SETTLE_MS};
use super::machine::at_safe_stopping_point;
use super::test_support::Rig;
use super::translate::claude::ClaudeTranslator;
use super::{ActivityPhase, TransitionSource};
use crate::harness_events_claude::ClaudeEvent;

const MIN: i64 = 60_000;
const ID: &str = "bg_1";

type Seen = Vec<(ActivityPhase, TransitionSource)>;

/// The rig plus the attached Claude translator (the TS `send` channel).
/// Derefs to the rig so `rig.m`, `rig.advance(..)` read as in the TS table.
struct Bg {
    rig: Rig,
    t: ClaudeTranslator,
}

impl Deref for Bg {
    type Target = Rig;
    fn deref(&self) -> &Rig {
        &self.rig
    }
}

impl DerefMut for Bg {
    fn deref_mut(&mut self) -> &mut Rig {
        &mut self.rig
    }
}

/// The TS `send(event)`.
fn send(b: &mut Bg, event: &ClaudeEvent) {
    let now = b.rig.now;
    b.t.apply(&mut b.rig.m, ID, event, now);
}

/// Claude harness mid-turn with its tail attached.
fn attached() -> Bg {
    let mut rig = Rig::new();
    rig.spawned(ID);
    let mut t = ClaudeTranslator::new();
    t.attached(&mut rig.m, ID, rig.now);
    let mut b = Bg { rig, t };
    send(&mut b, &ClaudeEvent::AssistantTurn);
    b
}

struct Task {
    kind: BackgroundKind,
    timeout_ms: Option<u64>,
    persistent: bool,
    initial: bool,
}

impl Default for Task {
    fn default() -> Self {
        Self {
            kind: BackgroundKind::Bash,
            timeout_ms: None,
            persistent: false,
            initial: false,
        }
    }
}

/// A `background_start` event.
fn start_task(rig: &mut Bg, over: &Task) {
    send(
        rig,
        &ClaudeEvent::BackgroundStart {
            task_id: "t1".to_owned(),
            tool_use_id: "tu_t1".to_owned(),
            task_kind: over.kind,
            description: None,
            command: Some("sleep 999".to_owned()),
            timeout_ms: over.timeout_ms,
            persistent: over.persistent,
            auto_backgrounded: false,
            agent_id: None,
            initial: over.initial,
        },
    );
}

/// A `background_end` event (`agent_id` null).
fn end_task(rig: &mut Bg) {
    send(
        rig,
        &ClaudeEvent::BackgroundEnd {
            task_id: "t1".to_owned(),
            task_kind: BackgroundKind::Bash,
            agent_id: None,
            status: OutcomeStatus::Completed,
            exit_code: None,
        },
    );
}

/// An `awaiting_prompt` event.
fn end_turn(rig: &mut Bg) {
    send(rig, &ClaudeEvent::AwaitingPrompt);
}

/// A `user_prompt` event.
fn prompt(rig: &mut Bg) {
    send(
        rig,
        &ClaudeEvent::UserPrompt {
            task_notification: false,
        },
    );
}

/// A live `subagent_start` event.
fn start_sub(rig: &mut Bg) {
    send(
        rig,
        &ClaudeEvent::SubagentStart {
            agent_id: "a1".to_owned(),
            agent_type: Some("explore".to_owned()),
            description: None,
            initial: false,
        },
    );
}

/// A `subagent_end` event.
fn end_sub(rig: &mut Bg) {
    send(
        rig,
        &ClaudeEvent::SubagentEnd {
            agent_id: "a1".to_owned(),
            agent_type: Some("explore".to_owned()),
            description: None,
        },
    );
}

/// atSafeStoppingPoint of the live record (false when gone).
fn safe_now(rig: &Rig) -> bool {
    rig.m.get(ID).is_some_and(at_safe_stopping_point)
}

fn phase(rig: &Rig) -> Option<ActivityPhase> {
    rig.m.get(ID).map(|a| a.phase)
}

/// Transitions recorded since the last call; the TS `watch(id)` array
/// starts empty at subscription, so callers drain once to subscribe.
fn take(rig: &mut Rig) -> Seen {
    rig.m
        .drain_transitions()
        .into_iter()
        .filter(|t| t.harness_id == ID)
        .map(|t| (t.to, t.source))
        .collect()
}

fn watch(rig: &mut Rig) {
    rig.m.drain_transitions();
}

/// `shouldCheckOnTransition` from `mailNudge.ts`.
fn should_check_on_transition(to: ActivityPhase, at_stopping_point_now: bool) -> bool {
    to == ActivityPhase::Waiting || at_stopping_point_now
}

#[test]
fn t1a_outstanding_bash_task_defers_end_prompt_next_end_of_turn_waiting_end_turn() {
    let mut rig = attached();
    start_task(&mut rig, &Task::default());
    watch(&mut rig);
    let mut t = Vec::new();
    end_turn(&mut rig);
    assert_eq!(phase(&rig), Some(ActivityPhase::Running));
    t.extend(take(&mut rig));
    assert_eq!(t, vec![]);
    end_task(&mut rig);
    prompt(&mut rig);
    end_turn(&mut rig);
    assert_eq!(phase(&rig), Some(ActivityPhase::Waiting));
    t.extend(take(&mut rig));
    assert_eq!(
        t.last(),
        Some(&(ActivityPhase::Waiting, TransitionSource::L2c1ClaudeEndTurn))
    );
}

#[test]
fn t1b_background_end_alone_then_silence_waiting_via_delegation_settled() {
    let mut rig = attached();
    start_task(&mut rig, &Task::default());
    end_turn(&mut rig);
    end_task(&mut rig);
    watch(&mut rig);
    rig.advance(DELEGATION_SETTLE_MS - 3_000);
    assert_eq!(phase(&rig), Some(ActivityPhase::Running));
    rig.advance(5_000);
    assert_eq!(phase(&rig), Some(ActivityPhase::Waiting));
    assert_eq!(
        take(&mut rig),
        vec![(ActivityPhase::Waiting, TransitionSource::DelegationSettled)]
    );
}

#[test]
fn t2_monitor_with_a_deadline_expires_from_the_registry_then_settles() {
    let mut rig = attached();
    start_task(
        &mut rig,
        &Task {
            kind: BackgroundKind::Monitor,
            timeout_ms: Some(60_000),
            persistent: false,
            initial: false,
        },
    );
    end_turn(&mut rig);
    watch(&mut rig);
    rig.advance(50_000);
    assert_eq!(phase(&rig), Some(ActivityPhase::Running));
    assert_eq!(rig.m.background_tasks().live(ID).len(), 1);
    rig.advance(12_000);
    assert_eq!(rig.m.background_tasks().live(ID).len(), 0);
    assert_eq!(phase(&rig), Some(ActivityPhase::Running));
    rig.advance(DELEGATION_SETTLE_MS + 3_000);
    assert_eq!(phase(&rig), Some(ActivityPhase::Waiting));
    assert_eq!(
        take(&mut rig),
        vec![(ActivityPhase::Waiting, TransitionSource::DelegationSettled)]
    );
}

#[test]
fn t3_bash_ceiling_flushes_with_delegation_ceiling_task_stays_overdue_later_turns_dont_re_defer() {
    let mut rig = attached();
    start_task(&mut rig, &Task::default());
    end_turn(&mut rig);
    watch(&mut rig);
    rig.advance(BACKGROUND_TASK_CEILING_MS - 3_000);
    assert_eq!(phase(&rig), Some(ActivityPhase::Running));
    assert_eq!(take(&mut rig), vec![]);
    rig.advance(5_000);
    assert_eq!(phase(&rig), Some(ActivityPhase::Waiting));
    assert_eq!(
        take(&mut rig),
        vec![(ActivityPhase::Waiting, TransitionSource::DelegationCeiling)]
    );
    assert_eq!(rig.m.background_tasks().live(ID).len(), 1);
    assert!(rig.m.background_tasks().live(ID)[0].overdue);
    assert_eq!(rig.m.background_tasks().working_count(ID), 0);
    assert_eq!(rig.m.background_tasks().overdue_count(ID), 1);

    prompt(&mut rig);
    end_turn(&mut rig);
    assert_eq!(phase(&rig), Some(ActivityPhase::Waiting));
    assert_eq!(rig.m.get(ID).unwrap().delegation_deferred_at, None);
}

#[test]
fn t3b_a_ceiling_flush_resets_delegated_count_so_a_later_waiting_claims_nothing_finished() {
    let mut rig = attached();
    start_sub(&mut rig);
    end_sub(&mut rig);
    start_task(&mut rig, &Task::default());
    end_turn(&mut rig);
    assert_eq!(rig.m.get(ID).unwrap().delegated_count, 1);
    rig.advance(BACKGROUND_TASK_CEILING_MS + 5_000);
    assert_eq!(phase(&rig), Some(ActivityPhase::Waiting));
    assert_eq!(rig.m.get(ID).unwrap().delegated_count, 0);
}

#[test]
fn t4_a_persistent_monitor_with_a_timeout_uses_the_bash_ceiling_not_its_deadline() {
    let mut rig = attached();
    start_task(
        &mut rig,
        &Task {
            kind: BackgroundKind::Monitor,
            timeout_ms: Some(60_000),
            persistent: true,
            initial: false,
        },
    );
    end_turn(&mut rig);
    watch(&mut rig);
    rig.advance(5 * MIN);
    assert_eq!(phase(&rig), Some(ActivityPhase::Running));
    assert_eq!(rig.m.background_tasks().live(ID).len(), 1);
    rig.advance(BACKGROUND_TASK_CEILING_MS - 5 * MIN + 3_000);
    assert_eq!(phase(&rig), Some(ActivityPhase::Waiting));
    assert_eq!(
        take(&mut rig),
        vec![(ActivityPhase::Waiting, TransitionSource::DelegationCeiling)]
    );
}

#[test]
fn t5_attach_seeded_tasks_never_defer() {
    let mut rig = attached();
    start_task(
        &mut rig,
        &Task {
            initial: true,
            ..Task::default()
        },
    );
    assert_eq!(rig.m.background_tasks().live(ID).len(), 1);
    assert_eq!(rig.m.background_tasks().working_count(ID), 0);
    watch(&mut rig);
    end_turn(&mut rig);
    assert_eq!(phase(&rig), Some(ActivityPhase::Waiting));
    assert_eq!(
        take(&mut rig),
        vec![(ActivityPhase::Waiting, TransitionSource::L2c1ClaudeEndTurn)]
    );
}

#[test]
fn t6a_subagent_and_task_count_together_subagent_end_alone_keeps_it_deferred() {
    let mut rig = attached();
    start_sub(&mut rig);
    start_task(&mut rig, &Task::default());
    end_turn(&mut rig);
    end_sub(&mut rig);
    watch(&mut rig);
    rig.advance(5 * MIN);
    assert_eq!(phase(&rig), Some(ActivityPhase::Running));
    assert_eq!(take(&mut rig), vec![]);
    end_task(&mut rig);
    rig.advance(DELEGATION_SETTLE_MS + 3_000);
    assert_eq!(phase(&rig), Some(ActivityPhase::Waiting));
    assert_eq!(
        take(&mut rig),
        vec![(ActivityPhase::Waiting, TransitionSource::DelegationSettled)]
    );
}

#[test]
fn t6b_subagent_ceiling_alone_does_not_flush_while_a_task_is_within_its_ceiling_flush_once_both_released()
 {
    let mut rig = attached();
    start_sub(&mut rig); // t0
    rig.advance(10 * MIN);
    start_task(&mut rig, &Task::default()); // t0+10m; arming happens now, so its ceiling is t0+25m
    end_turn(&mut rig);
    watch(&mut rig);
    rig.advance(DELEGATION_CEILING_MS - 10 * MIN + 5_000); // t0+15m: subagent presumed gone
    assert_eq!(rig.m.subagents().working_count(ID), 0);
    assert_eq!(phase(&rig), Some(ActivityPhase::Running));
    assert_eq!(take(&mut rig), vec![]);
    rig.advance(10 * MIN + 5_000); // past the task's ceiling
    assert_eq!(phase(&rig), Some(ActivityPhase::Waiting));
    assert_eq!(
        take(&mut rig),
        vec![(ActivityPhase::Waiting, TransitionSource::DelegationCeiling)]
    );
}

#[test]
fn t7a_441_watchdog_flushes_running_with_work_after_a_silent_ceiling() {
    let mut rig = attached();
    start_sub(&mut rig);
    start_task(&mut rig, &Task::default());
    watch(&mut rig);
    rig.advance(DELEGATION_CEILING_MS - 5_000);
    assert_eq!(phase(&rig), Some(ActivityPhase::Running));
    assert_eq!(take(&mut rig), vec![]);
    rig.advance(10_000);
    assert_eq!(phase(&rig), Some(ActivityPhase::Waiting));
    assert_eq!(
        take(&mut rig),
        vec![(ActivityPhase::Waiting, TransitionSource::WorkWatchdog)]
    );
    assert!(rig.m.background_tasks().live(ID)[0].overdue);
    assert_eq!(rig.m.background_tasks().overdue_count(ID), 1);
    assert_eq!(rig.m.subagents().working_count(ID), 0);
}

#[test]
fn t7b_fresh_main_transcript_work_keeps_resetting_the_watchdog() {
    let mut rig = attached();
    start_task(&mut rig, &Task::default());
    for _ in 0..4 {
        rig.advance(10 * MIN);
        // `tool_use_start`
        send(
            &mut rig,
            &ClaudeEvent::ToolUseStart {
                name: "Bash".to_owned(),
            },
        );
    }
    assert_eq!(phase(&rig), Some(ActivityPhase::Running));
    assert_eq!(rig.m.background_tasks().working_count(ID), 1);
    rig.advance(DELEGATION_CEILING_MS + 5_000);
    assert_eq!(phase(&rig), Some(ActivityPhase::Waiting));
}

#[test]
fn t8a_stale_state_release_hands_held_mail_a_safe_stopping_point_transition() {
    let mut rig = attached();
    start_task(&mut rig, &Task::default());
    assert!(!safe_now(&rig));
    watch(&mut rig);
    rig.advance(DELEGATION_CEILING_MS - 5_000);
    assert!(!safe_now(&rig));
    rig.advance(10_000);
    // The TS listener evaluates `safeNow` inside the transition callback;
    // the outbox is drained right after the same tick, so the record is the
    // same one.
    let seen: Vec<(ActivityPhase, bool, bool)> = take(&mut rig)
        .into_iter()
        .map(|(to, _)| {
            let safe = safe_now(&rig);
            (to, safe, should_check_on_transition(to, safe))
        })
        .collect();
    assert_eq!(seen, vec![(ActivityPhase::Waiting, true, true)]);
}

#[test]
fn t8b_bash_ceiling_release_hands_held_mail_a_safe_stopping_point_transition() {
    let mut rig = attached();
    start_task(&mut rig, &Task::default());
    end_turn(&mut rig);
    assert!(safe_now(&rig)); // deferred
    watch(&mut rig);
    rig.advance(BACKGROUND_TASK_CEILING_MS + 5_000);
    let seen: Vec<(ActivityPhase, bool, bool)> = take(&mut rig)
        .into_iter()
        .map(|(to, _)| {
            let safe = safe_now(&rig);
            (to, safe, should_check_on_transition(to, safe))
        })
        .collect();
    assert_eq!(seen, vec![(ActivityPhase::Waiting, true, true)]);
}
