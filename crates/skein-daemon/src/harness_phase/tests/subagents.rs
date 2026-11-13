//! Port of `app/src/subagents.test.ts` (#298, #277). The TS `nextId()`
//! per-test harness ids are unnecessary here: each test owns a fresh
//! registry.

use super::{SubagentEntry, SubagentInput, Subagents};

fn input(id: &str, ty: Option<&str>, desc: Option<&str>) -> SubagentInput {
    SubagentInput {
        agent_id: id.to_owned(),
        agent_type: ty.map(str::to_owned),
        description: desc.map(str::to_owned),
    }
}

fn ids(list: &[SubagentEntry]) -> Vec<&str> {
    list.iter().map(|e| e.agent_id.as_str()).collect()
}

const H: &str = "h_1";

#[test]
fn record_is_idempotent_and_preserves_the_original_started_at() {
    let mut s = Subagents::new();
    s.record(
        H,
        input("a1", Some("explore"), Some("look around")),
        false,
        1_000,
    );
    let first = s.live(H);
    assert_eq!(first.len(), 1);
    let started_at = first[0].started_at;

    // A restart replay of the same agent id must not double-count or
    // reset when it started.
    s.record(
        H,
        input("a1", Some("explore"), Some("look around")),
        false,
        9_000,
    );
    let second = s.live(H);
    assert_eq!(second.len(), 1);
    assert_eq!(second[0].started_at, started_at);
}

#[test]
fn relabel_updates_an_existing_entry_and_preserves_started_at_and_from_attach() {
    let mut s = Subagents::new();
    s.record(H, input("a1", None, None), true, 1_000);
    let before = s.live(H)[0].clone();
    assert!(s.relabel(H, "a1", Some("explore".into()), Some("look around".into())));
    let after = s.live(H);
    assert_eq!(after.len(), 1);
    assert_eq!(
        after[0],
        SubagentEntry {
            agent_id: "a1".into(),
            agent_type: Some("explore".into()),
            description: Some("look around".into()),
            started_at: before.started_at,
            from_attach: true,
        }
    );
}

#[test]
fn relabel_for_an_unknown_agent_id_is_a_no_op() {
    let mut s = Subagents::new();
    assert!(!s.relabel(H, "nope", Some("explore".into()), None));
    assert_eq!(s.live(H).len(), 0);
}

#[test]
fn finish_removes_the_entry() {
    let mut s = Subagents::new();
    s.record(H, input("a1", Some("explore"), None), false, 1_000);
    assert!(s.finish(H, "a1"));
    assert_eq!(s.live(H).len(), 0);
}

#[test]
fn finish_for_an_unknown_agent_id_is_a_no_op_not_a_panic() {
    let mut s = Subagents::new();
    assert!(!s.finish(H, "does-not-exist"));
    assert_eq!(s.live(H).len(), 0);
}

#[test]
fn forget_clears_every_subagent_tracked_for_the_harness() {
    let mut s = Subagents::new();
    s.record(H, input("a1", Some("explore"), None), false, 1_000);
    s.record(H, input("a2", Some("build"), None), false, 1_001);
    assert!(s.forget(H));
    assert_eq!(s.live(H).len(), 0);
}

#[test]
fn forget_for_a_harness_with_nothing_tracked_is_a_no_op_not_a_panic() {
    let mut s = Subagents::new();
    assert!(!s.forget(H));
}

#[test]
fn live_returns_entries_oldest_first_stable_across_insertion_order() {
    let mut s = Subagents::new();
    s.record(H, input("a1", Some("explore"), None), false, 1_000);
    s.record(H, input("a2", Some("build"), None), false, 2_000);
    s.record(H, input("a3", Some("plan"), None), false, 3_000);
    assert_eq!(ids(&s.live(H)), ["a1", "a2", "a3"]);

    // Re-recording the middle one doesn't reorder it: started_at is
    // preserved, and order is by started_at.
    s.record(H, input("a2", Some("build"), None), false, 4_000);
    assert_eq!(ids(&s.live(H)), ["a1", "a2", "a3"]);
}

#[test]
fn tracks_harnesses_independently() {
    let mut s = Subagents::new();
    s.record("a", input("x", Some("explore"), None), false, 1_000);
    assert_eq!(s.live("b").len(), 0);
    assert_eq!(s.live("a").len(), 1);
}

// #277: `from_attach` / `working_count` / `presume_gone`, the counting
// layer the delegation deferral reads, distinct from `live` (which stays
// an honest disk mirror per #276's contract).

#[test]
fn a_live_start_initial_false_is_never_from_attach() {
    let mut s = Subagents::new();
    s.record(H, input("a1", Some("explore"), None), false, 1_000);
    assert!(!s.live(H)[0].from_attach);
    assert_eq!(s.working_count(H), 1);
}

#[test]
fn an_attach_time_start_initial_true_is_from_attach_and_not_counted_as_working() {
    let mut s = Subagents::new();
    s.record(H, input("a1", Some("explore"), None), true, 1_000);
    assert!(s.live(H)[0].from_attach);
    assert_eq!(s.working_count(H), 0);
    // live() itself stays an honest mirror: the orphan is still
    // reported, only the counting excludes it.
    assert_eq!(s.live(H).len(), 1);
}

#[test]
fn an_attach_seeded_id_promoted_by_a_later_live_start_counts_as_working() {
    let mut s = Subagents::new();
    s.record(H, input("a1", Some("explore"), None), true, 1_000);
    assert_eq!(s.working_count(H), 0);

    s.record(H, input("a1", Some("explore"), None), false, 2_000);
    assert!(!s.live(H)[0].from_attach);
    assert_eq!(s.working_count(H), 1);
}

#[test]
fn promotion_is_sticky_a_later_replay_with_initial_true_does_not_revert_it() {
    let mut s = Subagents::new();
    s.record(H, input("a1", Some("explore"), None), false, 1_000);
    s.record(H, input("a1", Some("explore"), None), true, 2_000);
    assert!(!s.live(H)[0].from_attach);
    assert_eq!(s.working_count(H), 1);
}

#[test]
fn working_count_is_0_for_a_harness_with_nothing_tracked() {
    assert_eq!(Subagents::new().working_count(H), 0);
}

#[test]
fn presume_gone_drops_only_the_working_entries_and_reports_the_count() {
    let mut s = Subagents::new();
    s.record(H, input("attached", Some("explore"), None), true, 1_000);
    s.record(H, input("live1", Some("build"), None), false, 1_001);
    s.record(H, input("live2", Some("plan"), None), false, 1_002);

    let dropped = s.presume_gone(H);
    assert_eq!(dropped, 2);
    assert_eq!(s.working_count(H), 0);
    // The attach-only entry was never counted as working, so
    // presume_gone leaves it alone.
    assert_eq!(ids(&s.live(H)), ["attached"]);
}

#[test]
fn presume_gone_for_a_harness_with_nothing_tracked_is_a_no_op_not_a_panic() {
    assert_eq!(Subagents::new().presume_gone(H), 0);
}
