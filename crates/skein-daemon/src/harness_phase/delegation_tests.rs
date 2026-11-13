//! Port of `app/src/harnessActivity.delegation.test.ts`: the subagent-aware
//! end-of-turn deferral (#277), `subscribeDelegationDeferred` (#381) and
//! `sessionSwitched` (#116). One test per vitest case, same order of
//! assertions; `rig.advance` stands in for `vi.advanceTimersByTime`.
//!
//! NOT PORTED (partly): in "a delivered prompt disarms an armed deferral and
//! canSendPrompt now refuses" the two `canSendPrompt` assertions stay in the
//! webview (`harnessInput.ts` is a frontend gate with no Rust twin). The
//! phase and deferral assertions are ported; `at_safe_stopping_point` is
//! asserted in their place, since it is the fact `canSendPrompt` reads.

use super::machine::at_safe_stopping_point;
use super::subagents::SubagentInput;
use super::test_support::Rig;
use super::{ActivityPhase, SessionSwitch, TransitionSource};

const ID: &str = "d_1";

fn sub(id: &str) -> SubagentInput {
    SubagentInput {
        agent_id: id.to_owned(),
        agent_type: Some("explore".to_owned()),
        description: None,
    }
}

/// A Claude harness as `LiveTerminal` + `attachClaudeEvents` leave it:
/// spawned, adapter attached and already running.
fn running_harness() -> Rig {
    let mut rig = Rig::new();
    rig.spawned(ID);
    rig.m.attach_authoritative_source(ID);
    rig.m
        .set_running_from_adapter(ID, TransitionSource::L2c1ClaudeAssistant, false, rig.now);
    rig
}

/// What the translator does for a live `subagent_start`.
fn start_live_subagent(rig: &mut Rig, agent_id: &str) {
    rig.m
        .subagents_mut()
        .record(ID, sub(agent_id), false, rig.now);
    rig.m.note_subagent_started(ID, rig.now);
}

/// What the translator does for `subagent_end`.
fn finish_subagent(rig: &mut Rig, agent_id: &str) {
    rig.m.subagents_mut().finish(ID, agent_id);
    rig.m.note_subagent_activity(ID, rig.now);
}

fn end_turn(rig: &mut Rig) {
    rig.m
        .awaiting_prompt_from_adapter(ID, TransitionSource::L2c1ClaudeEndTurn, rig.now);
}

fn phase(rig: &Rig) -> ActivityPhase {
    rig.m.get(ID).unwrap().phase
}

fn deferred(rig: &Rig) -> bool {
    rig.m.get(ID).unwrap().delegation_deferred_at.is_some()
}

/// Sources of the transitions recorded since the last drain.
fn sources(rig: &mut Rig) -> Vec<TransitionSource> {
    rig.m
        .drain_transitions()
        .into_iter()
        .filter(|t| t.harness_id == ID)
        .map(|t| t.source)
        .collect()
}

/// `(to, source)` of the transitions recorded since the last drain.
fn to_and_source(rig: &mut Rig) -> Vec<(ActivityPhase, TransitionSource)> {
    rig.m
        .drain_transitions()
        .into_iter()
        .filter(|t| t.harness_id == ID)
        .map(|t| (t.to, t.source))
        .collect()
}

// ---- subagent-aware end-of-turn deferral (#277) ----

#[test]
fn awaiting_prompt_with_no_working_subagents_goes_straight_to_waiting_unchanged_behaviour() {
    let mut rig = running_harness();
    end_turn(&mut rig);
    assert_eq!(phase(&rig), ActivityPhase::Waiting);
    assert_eq!(rig.m.get(ID).unwrap().delegation_deferred_at, None);
}

#[test]
fn defers_with_one_working_subagent_phase_stays_running_no_transition_emitted() {
    let mut rig = running_harness();
    start_live_subagent(&mut rig, "a1");
    rig.m.drain_transitions();

    end_turn(&mut rig);

    assert_eq!(phase(&rig), ActivityPhase::Running);
    assert!(deferred(&rig));
    assert_eq!(to_and_source(&mut rig), vec![]);
}

#[test]
fn a_main_session_work_signal_disarms_the_deferral_the_next_empty_end_of_turn_goes_to_waiting_normally()
 {
    let mut rig = running_harness();
    start_live_subagent(&mut rig, "a1");
    end_turn(&mut rig);
    assert!(deferred(&rig));

    // The dominant real-world path: the main session wakes up and works
    // the delegation's result.
    rig.m
        .set_running_from_adapter(ID, TransitionSource::L2c1ClaudeToolUse, false, rig.now);
    assert_eq!(rig.m.get(ID).unwrap().delegation_deferred_at, None);

    finish_subagent(&mut rig, "a1");
    end_turn(&mut rig);
    assert_eq!(phase(&rig), ActivityPhase::Waiting);
}

#[test]
fn settle_deferred_subagent_ends_resolves_to_waiting_after_delegation_settle_ms() {
    let mut rig = running_harness();
    start_live_subagent(&mut rig, "a1");
    end_turn(&mut rig);
    finish_subagent(&mut rig, "a1");

    rig.advance(59_000);
    assert_eq!(phase(&rig), ActivityPhase::Running);

    rig.m.drain_transitions();
    rig.advance(2_000);
    assert_eq!(phase(&rig), ActivityPhase::Waiting);
    assert_eq!(sources(&mut rig), vec![TransitionSource::DelegationSettled]);
}

#[test]
fn ceiling_a_working_subagent_that_never_reports_again_resolves_to_waiting_and_is_presumed_gone() {
    let mut rig = running_harness();
    start_live_subagent(&mut rig, "a1");
    end_turn(&mut rig);
    // No finish, no tool result: the subagent just stops reporting.

    rig.m.drain_transitions();
    rig.advance(15 * 60_000 + 2_000);
    assert_eq!(phase(&rig), ActivityPhase::Waiting);
    assert_eq!(sources(&mut rig), vec![TransitionSource::DelegationCeiling]);
    assert_eq!(rig.m.subagents().working_count(ID), 0);
}

#[test]
fn adapter_detach_disarms_an_armed_deferral_l2a_regains_control_not_the_delegation_timers() {
    let mut rig = running_harness();
    start_live_subagent(&mut rig, "a1");
    end_turn(&mut rig);
    assert!(deferred(&rig));

    // session_end: the adapter's file vanished.
    rig.m.detach_authoritative_source(ID, rig.now);
    assert_eq!(rig.m.get(ID).unwrap().delegation_deferred_at, None);
    assert!(!rig.m.get(ID).unwrap().authoritative);

    rig.m.drain_transitions();
    let mut seen = Vec::new();

    // Prime `last_output_at` the way a real PTY chunk would right after
    // detach, same phase so no transition fires.
    rig.m.record_output(ID, Some("ordinary output\n"), rig.now);
    seen.extend(sources(&mut rig));
    assert_eq!(seen, vec![]);

    // No further PTY output: past IDLE_AFTER_MS, plain L2a is what flips
    // this to idle.
    rig.advance(8_500);
    assert_eq!(phase(&rig), ActivityPhase::Idle);
    seen.extend(sources(&mut rig));
    assert_eq!(seen, vec![TransitionSource::L2aIdle]);

    // A chunk of PTY output moves it straight back to running.
    rig.m.record_output(ID, Some("still here\n"), rig.now);
    assert_eq!(phase(&rig), ActivityPhase::Running);

    // Well past DELEGATION_CEILING_MS with no further output: only plain
    // L2a idle detection fires, never the deferral branch.
    rig.advance(15 * 60_000 + 2_000);
    assert_eq!(phase(&rig), ActivityPhase::Idle);
    seen.extend(sources(&mut rig));
    assert_eq!(
        seen,
        vec![
            TransitionSource::L2aIdle,
            TransitionSource::PtyOutput,
            TransitionSource::L2aIdle,
        ]
    );
    assert!(!seen.contains(&TransitionSource::DelegationCeiling));
    assert!(!seen.contains(&TransitionSource::DelegationSettled));
}

#[test]
fn an_attach_time_subagent_never_defers_anything() {
    let mut rig = running_harness();
    // initial: true, an attach-time replay, not a live start.
    rig.m
        .subagents_mut()
        .record(ID, sub("orphan"), true, rig.now);

    end_turn(&mut rig);

    assert_eq!(phase(&rig), ActivityPhase::Waiting);
    assert_eq!(rig.m.get(ID).unwrap().delegation_deferred_at, None);
}

#[test]
fn permission_outranks_both_timers_settle_still_resolves_once_the_dialog_clears() {
    let mut rig = running_harness();
    start_live_subagent(&mut rig, "a1");
    end_turn(&mut rig);
    finish_subagent(&mut rig, "a1");

    // A permission dialog opens on the main session mid-deferral.
    rig.m.set_permission_from_adapter(
        ID,
        TransitionSource::L2c1ClaudePermission,
        Some("Bash"),
        None,
        None,
        rig.now,
    );
    assert_eq!(phase(&rig), ActivityPhase::Permission);

    // Well past the settle threshold: the tick must leave it alone while
    // the dialog is open.
    rig.advance(70_000);
    assert_eq!(phase(&rig), ActivityPhase::Permission);
    assert!(deferred(&rig));

    // Dialog answered: clears to running; the deferral timers apply again.
    rig.m.clear_permission(
        ID,
        TransitionSource::L2c1ClaudeSubagentToolResult,
        None,
        rig.now,
    );
    assert_eq!(phase(&rig), ActivityPhase::Running);

    rig.advance(61_000);
    assert_eq!(phase(&rig), ActivityPhase::Waiting);
}

#[test]
fn delegated_count_counts_live_starts_and_resets_on_note_user_prompt() {
    let mut rig = running_harness();
    start_live_subagent(&mut rig, "a1");
    start_live_subagent(&mut rig, "a2");
    assert_eq!(rig.m.get(ID).unwrap().delegated_count, 2);

    // An attach-time replay must not count.
    rig.m
        .subagents_mut()
        .record(ID, sub("orphan"), true, rig.now);
    assert_eq!(rig.m.get(ID).unwrap().delegated_count, 2);

    rig.m.note_user_prompt(ID);
    assert_eq!(rig.m.get(ID).unwrap().delegated_count, 0);
}

// ---- subscribeDelegationDeferred (#381) ----

#[test]
fn fires_once_when_a_deferral_arms_not_on_the_idempotent_re_arm() {
    let mut rig = running_harness();
    start_live_subagent(&mut rig, "a1");
    rig.m.drain_delegation_armed();

    end_turn(&mut rig);
    assert_eq!(rig.m.drain_delegation_armed(), vec![ID.to_owned()]);

    // A second end-of-turn while already deferred is a no-op re-arm: must
    // not fire again.
    end_turn(&mut rig);
    assert_eq!(rig.m.drain_delegation_armed(), Vec::<String>::new());
}

#[test]
fn does_not_fire_when_working_count_is_0_that_path_goes_straight_to_waiting() {
    let mut rig = running_harness();
    rig.m.drain_delegation_armed();

    end_turn(&mut rig);

    assert_eq!(rig.m.drain_delegation_armed(), Vec::<String>::new());
    assert_eq!(phase(&rig), ActivityPhase::Waiting);
}

#[test]
fn does_not_fire_on_disarm() {
    let mut rig = running_harness();
    start_live_subagent(&mut rig, "a1");
    end_turn(&mut rig);

    rig.m.drain_delegation_armed();
    rig.m
        .set_running_from_adapter(ID, TransitionSource::L2c1ClaudeToolUse, false, rig.now);
    assert_eq!(rig.m.get(ID).unwrap().delegation_deferred_at, None);
    assert_eq!(rig.m.drain_delegation_armed(), Vec::<String>::new());
}

#[test]
fn a_delivered_prompt_disarms_an_armed_deferral_and_can_send_prompt_now_refuses() {
    let mut rig = running_harness();
    rig.m.adapter_delivered(ID, true, rig.now);
    rig.m.set_injected(ID, true);
    start_live_subagent(&mut rig, "a1");
    end_turn(&mut rig);
    assert!(deferred(&rig));

    // While armed, the harness IS a safe stopping point (#381).
    // NOT PORTED: expect(canSendPrompt(sendInput)).toEqual({ ok: true }) —
    // frontend gate; its input fact is asserted instead.
    assert!(at_safe_stopping_point(rig.m.get(ID).unwrap()));

    // The main transcript's own next user/assistant row, the dominant
    // real-world resolution (#277).
    rig.m
        .set_running_from_adapter(ID, TransitionSource::L2c1ClaudeUserPrompt, false, rig.now);

    assert_eq!(rig.m.get(ID).unwrap().delegation_deferred_at, None);
    assert_eq!(phase(&rig), ActivityPhase::Running);
    // NOT PORTED: expect(canSendPrompt({...}).ok).toBe(false) — frontend
    // gate; running with no deferral is not a safe stopping point.
    assert!(!at_safe_stopping_point(rig.m.get(ID).unwrap()));
}

// ---- sessionSwitched (#116) ----

#[test]
fn running_moves_to_waiting_with_the_session_clear_source() {
    let mut rig = running_harness();
    rig.m.drain_transitions();

    rig.m.session_switched(ID, SessionSwitch::Clear, rig.now);

    assert_eq!(phase(&rig), ActivityPhase::Waiting);
    assert_eq!(
        to_and_source(&mut rig),
        vec![(
            ActivityPhase::Waiting,
            TransitionSource::L2c1ClaudeSessionClear
        )]
    );
}

#[test]
fn running_moves_to_waiting_with_the_session_resume_source() {
    let mut rig = running_harness();
    rig.m.drain_transitions();

    rig.m.session_switched(ID, SessionSwitch::Resume, rig.now);

    assert_eq!(phase(&rig), ActivityPhase::Waiting);
    assert_eq!(
        to_and_source(&mut rig),
        vec![(
            ActivityPhase::Waiting,
            TransitionSource::L2c1ClaudeSessionResume
        )]
    );
}

#[test]
fn running_moves_to_waiting_with_the_session_fork_source() {
    let mut rig = running_harness();
    rig.m.drain_transitions();

    rig.m.session_switched(ID, SessionSwitch::Fork, rig.now);

    assert_eq!(phase(&rig), ActivityPhase::Waiting);
    assert_eq!(
        to_and_source(&mut rig),
        vec![(
            ActivityPhase::Waiting,
            TransitionSource::L2c1ClaudeSessionFork
        )]
    );
}

#[test]
fn startup_keeps_the_phase_running_539_but_still_forgets_subagents() {
    let mut rig = running_harness();
    start_live_subagent(&mut rig, "a1");
    assert_eq!(rig.m.subagents().working_count(ID), 1);

    rig.m.session_switched(ID, SessionSwitch::Startup, rig.now);

    assert_eq!(phase(&rig), ActivityPhase::Running);
    assert_eq!(rig.m.subagents().working_count(ID), 0);
}

#[test]
fn permission_stays_permission() {
    let mut rig = running_harness();
    rig.m.set_permission_from_adapter(
        ID,
        TransitionSource::L2c1ClaudePermission,
        Some("Bash"),
        None,
        None,
        rig.now,
    );
    assert_eq!(phase(&rig), ActivityPhase::Permission);

    rig.m.session_switched(ID, SessionSwitch::Clear, rig.now);

    assert_eq!(phase(&rig), ActivityPhase::Permission);
}

#[test]
fn exited_stays_exited() {
    let mut rig = running_harness();
    rig.m.exited(ID, Some(0), rig.now);
    assert_eq!(phase(&rig), ActivityPhase::Exited);

    rig.m.session_switched(ID, SessionSwitch::Clear, rig.now);

    assert_eq!(phase(&rig), ActivityPhase::Exited);
}

#[test]
fn forgets_a_subagent_registered_before_the_switch_working_count_reads_0_after() {
    let mut rig = running_harness();
    start_live_subagent(&mut rig, "a1");
    assert_eq!(rig.m.subagents().working_count(ID), 1);

    rig.m.session_switched(ID, SessionSwitch::Resume, rig.now);

    assert_eq!(rig.m.subagents().working_count(ID), 0);
}

#[test]
fn disarms_an_armed_delegation_deferral() {
    let mut rig = running_harness();
    start_live_subagent(&mut rig, "a1");
    end_turn(&mut rig);
    assert!(deferred(&rig));

    rig.m.session_switched(ID, SessionSwitch::Clear, rig.now);

    assert_eq!(rig.m.get(ID).unwrap().delegation_deferred_at, None);
}
