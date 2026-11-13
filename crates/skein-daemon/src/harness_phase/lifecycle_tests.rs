//! Ports of `app/src/harnessActivity.reattach.test.ts` and
//! `harnessActivity.resume.test.ts`. (`clearPermission-idle` lives in
//! `core_tests.rs` with the other `clear_permission` cases.)
//!
//! The resume cases were driven through the real `attachClaudeEvents`
//! translator over a mocked Tauri `Channel`; here the translator's effect on
//! the store is spelled out: attach marks the source authoritative, every
//! event calls `adapter_delivered`, then `assistant_turn` is
//! `set_running_from_adapter` and `awaiting_prompt` is
//! `awaiting_prompt_from_adapter` (harnessEventsClaude.ts `translate`).

use super::constants::{ADAPTER_SILENT_AFTER_MS, DELEGATION_CEILING_MS, LAUNCH_SILENT_AFTER_MS};
use super::test_support::Rig;
use super::{ActivityPhase, TransitionSource};

// ---- harnessActivity.reattach.test.ts: describe("reattached") ----

#[test]
fn reattached_moves_a_non_authoritative_harness_from_spawning_to_idle_with_a_non_notifying_source()
{
    let mut r = Rig::new();
    r.spawned("ra1");
    let now = r.now;
    r.m.reattached("ra1", now);
    assert_eq!(r.m.get("ra1").unwrap().phase, ActivityPhase::Idle);
    let seen: Vec<_> =
        r.m.drain_transitions()
            .iter()
            .map(|t| (t.from, t.to, t.source))
            .collect();
    assert_eq!(
        seen,
        vec![(
            ActivityPhase::Spawning,
            ActivityPhase::Idle,
            TransitionSource::Reattached
        )]
    );
    // NOT PORTED: `isNotifiable(classifyTransition("spawning", "idle"),
    // Reattached) === false` — the notification classifier is frontend code
    // (harnessNotifyLogic.ts) with no daemon counterpart.
    r.m.forget("ra1");
}

#[test]
fn reattached_leaves_an_authoritative_harness_to_its_adapter() {
    let mut r = Rig::new();
    r.spawned("ra2");
    r.m.attach_authoritative_source("ra2");
    let now = r.now;
    r.m.reattached("ra2", now);
    assert_eq!(r.m.get("ra2").unwrap().phase, ActivityPhase::Spawning);
    r.m.forget("ra2");
}

#[test]
fn reattached_does_not_touch_a_harness_that_already_left_spawning() {
    let mut r = Rig::new();
    r.spawned("ra3");
    let now = r.now;
    r.m.record_output("ra3", Some("x\n"), now);
    r.m.reattached("ra3", now);
    assert_eq!(r.m.get("ra3").unwrap().phase, ActivityPhase::Running);
    r.m.forget("ra3");
}

// ---- harnessActivity.resume.test.ts: describe("resume onto a transcript
// that ends mid tool call (#336)") ----

// NOT PORTED: sends freshProcess in the attach payload — asserts the args of the mocked Tauri `claude_events_attach` invoke (frontend wiring)

#[test]
fn a_resumed_harness_whose_replay_is_awaiting_prompt_reads_waiting_with_no_prompt_typed() {
    let mut r = Rig::new();
    let id = "resume_2";
    r.spawned(id);
    // attachClaudeEvents(id, ..., true)
    r.m.attach_authoritative_source(id);
    let now = r.now;
    r.m.record_output(id, Some("welcome banner"), now);

    // send({ kind: "awaiting_prompt" })
    r.m.adapter_delivered(id, true, now);
    r.m.awaiting_prompt_from_adapter(id, TransitionSource::L2c1ClaudeEndTurn, now);

    assert_eq!(r.m.get(id).unwrap().phase, ActivityPhase::Waiting);
    r.m.forget(id);
}

// NOT PORTED: the real attachAdapters sends freshProcess: true for a Claude harness — asserts the args of the mocked Tauri `claude_events_attach` invoke made by terminalSpawnAdapters.ts

#[test]
fn an_adapter_reported_assistant_turn_with_no_further_events_never_decays_to_waiting() {
    let mut r = Rig::new();
    let id = "resume_4";
    r.spawned(id);
    // attachClaudeEvents(id, ..., false) then send({ kind: "assistant_turn" })
    r.m.attach_authoritative_source(id);
    let now = r.now;
    r.m.adapter_delivered(id, true, now);
    r.m.set_running_from_adapter(id, TransitionSource::L2c1ClaudeAssistant, false, now);
    assert_eq!(r.m.get(id).unwrap().phase, ActivityPhase::Running);

    let past = ADAPTER_SILENT_AFTER_MS
        .max(LAUNCH_SILENT_AFTER_MS)
        .max(DELEGATION_CEILING_MS);
    r.advance(past * 2);

    assert_eq!(r.m.get(id).unwrap().phase, ActivityPhase::Running);
    r.m.forget(id);
}

#[test]
fn forget_drops_a_stored_screen_reading_even_without_a_record() {
    let mut rig = Rig::new();
    rig.m
        .set_screen_reading("ghost", Some(super::ScreenReading::Waiting));
    assert!(rig.m.has_screen_state("ghost"));
    rig.m.forget("ghost");
    assert!(!rig.m.has_screen_state("ghost"));
}
