//! Port of `app/src/backgroundTasks.test.ts` (#446). Fake timers become
//! explicit `now_ms` arguments.

use skein_harness::claude::background::BackgroundKind;

use super::{BackgroundTaskEntry, BackgroundTaskInput, BackgroundTasks, deadline_of};

const H: &str = "h_bg";

fn task(id: &str) -> BackgroundTaskInput {
    BackgroundTaskInput {
        task_id: id.to_owned(),
        kind: BackgroundKind::Bash,
        description: None,
        command: None,
        timeout_ms: None,
        persistent: false,
        agent_id: None,
    }
}

fn monitor(id: &str, timeout_ms: i64) -> BackgroundTaskInput {
    BackgroundTaskInput {
        kind: BackgroundKind::Monitor,
        timeout_ms: Some(timeout_ms),
        ..task(id)
    }
}

fn ids(list: &[BackgroundTaskEntry]) -> Vec<&str> {
    list.iter().map(|e| e.task_id.as_str()).collect()
}

fn entry(kind: BackgroundKind, persistent: bool, timeout_ms: Option<i64>) -> BackgroundTaskEntry {
    BackgroundTaskEntry {
        task_id: "x".into(),
        kind,
        description: None,
        command: None,
        timeout_ms,
        persistent,
        agent_id: None,
        started_at: 100,
        from_attach: false,
        overdue: false,
        overdue_reported: false,
    }
}

#[test]
fn record_is_idempotent_and_preserves_the_original_started_at() {
    let mut b = BackgroundTasks::new();
    b.record(H, task("t1"), false, 1_000);
    b.record(H, task("t1"), false, 5_000);
    assert_eq!(b.live(H).len(), 1);
    assert_eq!(b.live(H)[0].started_at, 1_000);
}

#[test]
fn an_attach_only_entry_is_listed_but_not_working_a_live_re_record_promotes_it_replay_never_demotes()
 {
    let mut b = BackgroundTasks::new();
    b.record(H, task("t1"), true, 1_000);
    assert_eq!(b.live(H).len(), 1);
    assert_eq!(b.working_count(H), 0);
    b.record(H, task("t1"), false, 1_000);
    assert_eq!(b.working_count(H), 1);
    b.record(H, task("t1"), true, 1_000);
    assert_eq!(b.working_count(H), 1);
    assert!(!b.live(H)[0].from_attach);
}

#[test]
fn live_is_oldest_first_and_unchanged_without_changes() {
    // The TS test also asserts referential stability, a React
    // `useSyncExternalStore` concern with no Rust counterpart; equality
    // across two calls is the closest check.
    let mut b = BackgroundTasks::new();
    b.record(H, task("late"), false, 1_000);
    b.record(H, task("early"), false, 500);
    let a = b.live(H);
    assert_eq!(ids(&a), ["early", "late"]);
    assert_eq!(b.live(H), a);
}

#[test]
fn finish_removes_unknown_id_is_a_no_op_forget_clears() {
    let mut b = BackgroundTasks::new();
    b.record(H, task("t1"), false, 1_000);
    b.record(H, task("t2"), false, 1_001);
    b.finish(H, "t1");
    assert_eq!(ids(&b.live(H)), ["t2"]);
    assert!(!b.finish(H, "nope"));
    b.forget(H);
    assert_eq!(b.live(H).len(), 0);
    assert!(!b.forget(H));
}

#[test]
fn deadline_of_only_gives_a_non_persistent_monitor_with_a_timeout_a_deadline() {
    let t = Some(50);
    assert_eq!(deadline_of(&entry(BackgroundKind::Bash, false, t)), None);
    assert_eq!(
        deadline_of(&entry(BackgroundKind::PowerShell, false, t)),
        None
    );
    assert_eq!(
        deadline_of(&entry(BackgroundKind::Monitor, false, t)),
        Some(150)
    );
    assert_eq!(deadline_of(&entry(BackgroundKind::Monitor, true, t)), None);
    assert_eq!(
        deadline_of(&entry(BackgroundKind::Monitor, false, None)),
        None
    );
}

#[test]
fn expire_due_removes_only_due_monitors_and_returns_the_working_count_removed() {
    let mut b = BackgroundTasks::new();
    b.record(H, monitor("due", 1_000), false, 1_000);
    b.record(H, monitor("later", 9_000), false, 1_000);
    b.record(H, task("bash"), false, 1_000);
    b.record(H, monitor("orphan", 100), true, 1_000);
    // At 1_500 only the attach-only orphan (deadline 1_100) is past its
    // deadline: removed, but not counted (the TS return is 0).
    assert_eq!(b.expire_due(H, 1_500), (0, true));
    assert_eq!(b.expire_due(H, 2_000), (1, true));
    let mut left: Vec<String> = b.live(H).into_iter().map(|e| e.task_id).collect();
    left.sort();
    assert_eq!(left, ["bash", "later"]);
}

#[test]
fn mark_overdue_keeps_the_entry_but_drops_it_from_working_count() {
    let mut b = BackgroundTasks::new();
    b.record(H, task("t1"), false, 1_000);
    b.record(H, task("t2"), false, 1_000);
    b.record(H, task("o"), true, 1_000);
    assert_eq!(b.mark_overdue(H, &["t1", "o", "missing"]), 1);
    assert_eq!(b.mark_overdue(H, &["t1"]), 0);
    assert_eq!(b.live(H).len(), 3);
    assert_eq!(b.working_count(H), 1);
    assert_eq!(b.overdue_count(H), 1);
}

#[test]
fn overdue_is_sticky_across_a_re_record() {
    let mut b = BackgroundTasks::new();
    b.record(H, task("t1"), false, 1_000);
    b.mark_overdue(H, &["t1"]);
    b.record(H, task("t1"), false, 1_000);
    assert!(b.live(H)[0].overdue);
    assert_eq!(b.working_count(H), 0);
}

#[test]
fn take_unreported_overdue_counts_each_overdue_task_once_silently() {
    let mut b = BackgroundTasks::new();
    b.record(H, task("a"), false, 1_000);
    b.record(H, task("o"), true, 1_000);
    assert_eq!(b.take_unreported_overdue(H), 0);
    b.mark_overdue(H, &["a"]);
    // Silent: the method returns only the count, never a change flag,
    // and leaves the visible entries as they were.
    let before = b.live(H);
    assert_eq!(b.take_unreported_overdue(H), 1);
    assert_eq!(b.take_unreported_overdue(H), 0);
    let after = b.live(H);
    assert_eq!(
        after
            .iter()
            .map(|e| (&e.task_id, e.overdue, e.started_at))
            .collect::<Vec<_>>(),
        before
            .iter()
            .map(|e| (&e.task_id, e.overdue, e.started_at))
            .collect::<Vec<_>>()
    );
    // Survives a re-record; a newly overdue task is counted on its own.
    b.record(H, task("a"), false, 1_000);
    b.record(H, task("b"), false, 1_000);
    b.mark_overdue(H, &["b"]);
    assert_eq!(b.take_unreported_overdue(H), 1);
    assert_eq!(b.overdue_count(H), 2);
}

#[test]
fn presume_gone_marks_working_entries_overdue_without_deleting() {
    let mut b = BackgroundTasks::new();
    b.record(H, task("a"), false, 1_000);
    b.record(H, task("b"), false, 1_000);
    b.record(H, task("o"), true, 1_000);
    assert_eq!(b.presume_gone(H), 2);
    assert_eq!(b.presume_gone(H), 0);
    assert_eq!(b.live(H).len(), 3);
    assert_eq!(b.working_count(H), 0);
    assert_eq!(b.overdue_count(H), 2);
}

#[test]
fn subscribe_fires_on_changes_not_on_no_ops() {
    // The TS test counts subscriber callbacks. Subscriptions are not
    // ported; the equivalent is counting which calls report a change.
    let mut b = BackgroundTasks::new();
    let mut changes = 0;
    let mut tally = |changed: bool| changes += usize::from(changed);

    tally(b.record(H, task("t1"), false, 1_000)); // 1
    tally(b.finish(H, "nope")); // no-op
    tally(b.mark_overdue(H, &["nope"]) > 0); // no-op
    assert_eq!(b.expire_due(H, 99_999), (0, false)); // no-op
    tally(b.presume_gone(H) > 0); // 2
    tally(b.presume_gone(H) > 0); // no-op
    tally(b.finish(H, "t1")); // 3
    assert_eq!(changes, 3);
}

#[test]
fn deadline_of_saturates_on_a_huge_timeout() {
    let mut e = entry(BackgroundKind::Monitor, false, Some(i64::MAX));
    e.started_at = 1_000;
    assert_eq!(deadline_of(&e), Some(i64::MAX));
}
