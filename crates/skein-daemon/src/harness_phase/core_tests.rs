//! Port of `app/src/harnessActivity.test.ts` (the store-level cases) and
//! `harnessActivity.clearPermission-idle.test.ts`. One `#[test]` per vitest
//! case, same order.
//!
//! The label / status-aggregation cases (`activityToStatus`, `statusLabel`,
//! `delegationSummary`, `effectiveStatus`, `higherPriorityStatus`,
//! `aggregateRoomStatus`) exercise frontend display code that has no
//! counterpart in the daemon; they are listed as NOT PORTED where they
//! sit in the TS file.

use super::test_support::Rig;
use super::{ActivityPhase, TransitionSource, is_decisive_input};

const ID: &str = "h_1";

fn setup() -> Rig {
    let mut r = Rig::new();
    r.spawned(ID);
    r
}

fn phase(r: &Rig) -> ActivityPhase {
    r.m.get(ID).unwrap().phase
}

fn bash_permission(r: &mut Rig) {
    let now = r.now;
    r.m.set_permission_from_adapter(
        ID,
        TransitionSource::L2c1ClaudePermission,
        Some("Bash"),
        None,
        None,
        now,
    );
}

// ---- describe("permission phase") ----

#[test]
fn permission_enters_permission_with_the_tool_name() {
    let mut r = setup();
    bash_permission(&mut r);
    let a = r.m.get(ID).unwrap();
    assert_eq!(a.phase, ActivityPhase::Permission);
    assert_eq!(a.permission_tool.as_deref(), Some("Bash"));
}

#[test]
fn permission_is_not_knocked_out_by_pty_output_while_authoritative() {
    let mut r = setup();
    r.m.attach_authoritative_source(ID);
    bash_permission(&mut r);
    let now = r.now;
    r.m.record_output(ID, Some("the dialog repaints\n"), now);
    assert_eq!(phase(&r), ActivityPhase::Permission);
}

#[test]
fn permission_is_not_knocked_out_by_pty_output_when_non_authoritative_either() {
    // Defensive: today only authoritative harnesses enter `permission`,
    // but record_output must not fight the phase regardless of that flag.
    let mut r = setup();
    bash_permission(&mut r);
    let now = r.now;
    r.m.record_output(ID, Some("(y/n)?"), now);
    assert_eq!(phase(&r), ActivityPhase::Permission);
}

#[test]
fn permission_is_not_cleared_by_a_still_working_adapter_event() {
    let mut r = setup();
    bash_permission(&mut r);
    let now = r.now;
    r.m.set_running_from_adapter(ID, TransitionSource::L2c1ClaudeAssistant, false, now);
    assert_eq!(phase(&r), ActivityPhase::Permission);
    r.m.set_running_from_adapter(ID, TransitionSource::L2c1ClaudeToolUse, false, now);
    assert_eq!(phase(&r), ActivityPhase::Permission);
}

#[test]
fn permission_is_cleared_by_a_tool_result_which_also_drops_the_stale_tool_name() {
    let mut r = setup();
    bash_permission(&mut r);
    let now = r.now;
    r.m.set_running_from_adapter(ID, TransitionSource::L2c1ClaudeToolResult, true, now);
    let a = r.m.get(ID).unwrap();
    assert_eq!(a.phase, ActivityPhase::Running);
    assert_eq!(a.permission_tool, None);
}

#[test]
fn permission_is_released_when_the_adapter_detaches_and_nothing_else_is() {
    let mut r = setup();
    bash_permission(&mut r);
    let now = r.now;
    r.m.release_permission(ID, TransitionSource::AdapterDetached, now);
    assert_eq!(phase(&r), ActivityPhase::Running);

    r.m.set_waiting_from_adapter(ID, TransitionSource::L2c1ClaudeEndTurn, now);
    r.m.release_permission(ID, TransitionSource::AdapterDetached, now);
    assert_eq!(phase(&r), ActivityPhase::Waiting);
}

#[test]
fn permission_is_cleared_by_a_fresh_user_prompt() {
    let mut r = setup();
    bash_permission(&mut r);
    let now = r.now;
    r.m.set_running_from_adapter(ID, TransitionSource::L2c1ClaudeUserPrompt, true, now);
    assert_eq!(phase(&r), ActivityPhase::Running);
}

#[test]
fn permission_awaiting_prompt_always_leaves_permission_landing_on_waiting() {
    let mut r = setup();
    bash_permission(&mut r);
    let now = r.now;
    r.m.set_waiting_from_adapter(ID, TransitionSource::L2c1ClaudeEndTurn, now);
    assert_eq!(phase(&r), ActivityPhase::Waiting);
}

#[test]
fn permission_is_cleared_by_decisive_user_input() {
    let mut r = setup();
    bash_permission(&mut r);
    let now = r.now;
    r.m.record_input(ID, "\r", false, now);
    assert_eq!(phase(&r), ActivityPhase::Running);
}

#[test]
fn permission_is_not_cleared_by_an_arrow_key_or_a_focus_escape() {
    let mut r = setup();
    bash_permission(&mut r);
    let now = r.now;
    r.m.record_input(ID, "\x1b[A", false, now); // up arrow
    assert_eq!(phase(&r), ActivityPhase::Permission);
    r.m.record_input(ID, "\x1b[I", false, now); // focus-in
    assert_eq!(phase(&r), ActivityPhase::Permission);
    r.m.record_input(ID, "\x1b[O", false, now); // focus-out
    assert_eq!(phase(&r), ActivityPhase::Permission);
}

#[test]
fn permission_exited_wins_over_permission() {
    let mut r = setup();
    bash_permission(&mut r);
    let now = r.now;
    r.m.exited(ID, Some(0), now);
    assert_eq!(phase(&r), ActivityPhase::Exited);
    // No further adapter call can revive it.
    bash_permission(&mut r);
    assert_eq!(phase(&r), ActivityPhase::Exited);
}

// ---- describe("clearPermission (#298)") ----

fn subagent_result(r: &mut Rig, agent_id: Option<&str>) {
    let now = r.now;
    r.m.clear_permission(
        ID,
        TransitionSource::L2c1ClaudeSubagentToolResult,
        agent_id,
        now,
    );
}

fn reviewer_b_permission(r: &mut Rig) {
    let now = r.now;
    r.m.set_permission_from_adapter(
        ID,
        TransitionSource::L2c1ClaudePermission,
        Some("Bash"),
        Some("reviewer"),
        Some("B"),
        now,
    );
}

#[test]
fn clear_permission_moves_permission_to_running() {
    let mut r = setup();
    bash_permission(&mut r);
    subagent_result(&mut r, None);
    assert_eq!(phase(&r), ActivityPhase::Running);
}

// The #277 boundary: a subagent tool result must never move a phase other
// than `permission`. (The `idle` case is `clear_permission_is_a_no_op_from_idle`
// below, from harnessActivity.clearPermission-idle.test.ts.)
#[test]
fn clear_permission_is_a_no_op_from_waiting() {
    let mut r = setup();
    let now = r.now;
    r.m.set_waiting_from_adapter(ID, TransitionSource::L2c1ClaudeEndTurn, now);
    subagent_result(&mut r, None);
    assert_eq!(phase(&r), ActivityPhase::Waiting);
}

#[test]
fn clear_permission_is_a_no_op_from_running() {
    let mut r = setup();
    let now = r.now;
    r.m.set_running_from_adapter(ID, TransitionSource::L2c1ClaudeAssistant, false, now);
    subagent_result(&mut r, None);
    assert_eq!(phase(&r), ActivityPhase::Running);
}

#[test]
fn clear_permission_is_a_no_op_from_spawning() {
    let mut r = setup();
    assert_eq!(phase(&r), ActivityPhase::Spawning);
    subagent_result(&mut r, None);
    assert_eq!(phase(&r), ActivityPhase::Spawning);
}

#[test]
fn clear_permission_is_a_no_op_from_exited() {
    let mut r = setup();
    let now = r.now;
    r.m.exited(ID, Some(0), now);
    subagent_result(&mut r, None);
    assert_eq!(phase(&r), ActivityPhase::Exited);
}

#[test]
fn clear_permission_stores_permission_agent_type_and_clears_it_when_the_phase_leaves_permission() {
    let mut r = setup();
    let now = r.now;
    r.m.set_permission_from_adapter(
        ID,
        TransitionSource::L2c1ClaudePermission,
        Some("Bash"),
        Some("explore"),
        None,
        now,
    );
    assert_eq!(
        r.m.get(ID).unwrap().permission_agent_type.as_deref(),
        Some("explore")
    );
    subagent_result(&mut r, None);
    let a = r.m.get(ID).unwrap();
    assert_eq!(a.phase, ActivityPhase::Running);
    assert_eq!(a.permission_agent_type, None);
    assert_eq!(a.permission_tool, None);
}

// The headline case: two subagents running concurrently, B's dialog is
// open, and A's own tool result must not clear it.
#[test]
fn clear_permission_leaves_a_different_subagents_dialog_open() {
    let mut r = setup();
    reviewer_b_permission(&mut r);
    subagent_result(&mut r, Some("A"));
    assert_eq!(phase(&r), ActivityPhase::Permission);
}

#[test]
fn clear_permission_clears_its_own_subagents_dialog() {
    let mut r = setup();
    reviewer_b_permission(&mut r);
    subagent_result(&mut r, Some("B"));
    assert_eq!(phase(&r), ActivityPhase::Running);
}

#[test]
fn clear_permission_clears_a_main_session_dialog_regardless_of_who_reports() {
    let mut r = setup();
    bash_permission(&mut r);
    assert_eq!(r.m.get(ID).unwrap().permission_agent_id, None);
    subagent_result(&mut r, Some("B"));
    assert_eq!(phase(&r), ActivityPhase::Running);
}

#[test]
fn clear_permission_stores_permission_agent_id_and_clears_it_when_the_phase_leaves_permission() {
    let mut r = setup();
    reviewer_b_permission(&mut r);
    assert_eq!(
        r.m.get(ID).unwrap().permission_agent_id.as_deref(),
        Some("B")
    );
    subagent_result(&mut r, Some("B"));
    assert_eq!(r.m.get(ID).unwrap().permission_agent_id, None);
}

// harnessActivity.clearPermission-idle.test.ts: reaching a real `idle`
// needs the L2a tick, which the rig fires.
#[test]
fn clear_permission_is_a_no_op_from_idle() {
    let mut r = Rig::new();
    r.spawned("ci_1");
    let now = r.now;
    // Non-authoritative harness: one chunk of output moves spawning ->
    // running, then silence past IDLE_AFTER_MS (8s) lets the tick flip it
    // to idle.
    r.m.record_output("ci_1", Some("hello\n"), now);
    assert_eq!(r.m.get("ci_1").unwrap().phase, ActivityPhase::Running);
    r.advance(9_000);
    assert_eq!(r.m.get("ci_1").unwrap().phase, ActivityPhase::Idle);

    let now = r.now;
    r.m.clear_permission(
        "ci_1",
        TransitionSource::L2c1ClaudeSubagentToolResult,
        None,
        now,
    );
    assert_eq!(r.m.get("ci_1").unwrap().phase, ActivityPhase::Idle);
}

// ---- describe("isDecisiveInput") ----

#[test]
fn is_decisive_input_treats_enter_return_as_decisive() {
    assert!(is_decisive_input("\r"));
    assert!(is_decisive_input("\n"));
}

#[test]
fn is_decisive_input_treats_a_lone_esc_as_decisive_but_not_an_escape_sequence() {
    assert!(is_decisive_input("\x1b"));
    assert!(!is_decisive_input("\x1b[A"));
    assert!(!is_decisive_input("\x1b[I"));
    assert!(!is_decisive_input("\x1b[O"));
}

#[test]
fn is_decisive_input_treats_ctrl_c_as_decisive() {
    assert!(is_decisive_input("\x03"));
}

#[test]
fn is_decisive_input_treats_a_single_digit_or_y_n_as_decisive() {
    for c in ["0", "1", "9", "y", "Y", "n", "N"] {
        assert!(is_decisive_input(c), "{c}");
    }
}

#[test]
fn is_decisive_input_does_not_treat_ordinary_text_as_decisive() {
    assert!(!is_decisive_input("a"));
    assert!(!is_decisive_input("no"));
    assert!(!is_decisive_input(""));
    assert!(!is_decisive_input(" "));
}

// ---- describe("activityToStatus / statusLabel") ----
// NOT PORTED: maps the permission phase to the permission status — activityToStatus is frontend display code with no daemon counterpart
// NOT PORTED: reads as 'permission needed' plus the tool when known — statusLabel is frontend display code
// NOT PORTED: names the subagent when the dialog belongs to one (#298) — statusLabel is frontend display code
// NOT PORTED: passes other statuses through unchanged — statusLabel is frontend display code
// NOT PORTED: reads 'delegating · N agents' for a running harness with working subagents (#277) — statusLabel is frontend display code
// NOT PORTED: ignores the working-subagent count for any status but running — statusLabel is frontend display code

// ---- describe("delegationSummary") ----
// NOT PORTED: is null for zero — delegationSummary is frontend notification wording
// NOT PORTED: is singular for one, plural otherwise — delegationSummary is frontend notification wording

// ---- describe("effectiveStatus") ----
// NOT PORTED: does not downgrade permission when acknowledged — effectiveStatus is frontend display code
// NOT PORTED: still downgrades acknowledged waiting to idle — effectiveStatus is frontend display code

// ---- describe("higherPriorityStatus (room aggregate ranking)") ----
// NOT PORTED: ranks permission above waiting — higherPriorityStatus is frontend display code
// NOT PORTED: ranks waiting above running, idle and exited — higherPriorityStatus is frontend display code

// ---- describe("aggregateRoomStatus (#290)") ----
// NOT PORTED: returns idle for a room with no harnesses — aggregateRoomStatus is frontend display code
// NOT PORTED: returns idle when no harness in the room has an activity record — aggregateRoomStatus is frontend display code
// NOT PORTED: aggregates across a mix of recorded and unrecorded harnesses — aggregateRoomStatus is frontend display code
// NOT PORTED: downgrades waiting to idle with no pending notifications — aggregateRoomStatus is frontend display code
// NOT PORTED: keeps waiting when notifications are pending — aggregateRoomStatus is frontend display code

// ---- describe("forget and subscribers") ----

// #555: forget drops the store entry. The daemon's outbox is the machine's
// own, so "subscriber survives forget" becomes "every step still marks the id
// changed". The final `off()` / unsubscribe tail has no outbox equivalent
// (there are no per-id subscriptions to remove).
#[test]
fn forget_keeps_a_subscriber_across_forget_and_respawn() {
    let mut r = Rig::new();
    let id = "h_forget";
    r.spawned(id);
    let _ = r.m.drain_changed(); // subscribe() happens after the spawn

    r.m.forget(id);
    assert_eq!(r.m.drain_changed(), vec![id.to_owned()]);
    assert!(r.m.get(id).is_none());

    r.spawned(id);
    assert_eq!(r.m.drain_changed(), vec![id.to_owned()]);
    let now = r.now;
    r.m.set_permission_from_adapter(
        id,
        TransitionSource::L2c1ClaudePermission,
        Some("Bash"),
        None,
        None,
        now,
    );
    assert_eq!(r.m.drain_changed(), vec![id.to_owned()]);
    // NOT PORTED: `off()` then forget/spawned leaving the call count at 3 —
    // the outbox has no per-id unsubscribe.
    r.m.forget(id);
}
