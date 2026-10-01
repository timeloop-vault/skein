//! Background tasks (#445): sweep, ownership, ordering and resync cases.

use super::background_tasks::{BackgroundOut, BackgroundState, now_ms};
use super::background_tests::{
    t445_append, t445_attach, t445_bash_start, t445_end_rows, t445_ends, t445_live,
    t445_monitor_start, t445_notif, t445_notified, t445_result, t445_starts, t445_sub_end_turn,
    t445_use,
};
use super::resync_tests::t425_tick;
use super::tests::{drain, drain_brief};
use super::*;
use std::fs;
use tempfile::TempDir;

/// Case 6, live: the subagent's own task is dropped, without a row,
/// when the subagent ends.
#[test]
fn t445_subagent_end_drops_its_task_without_a_row() {
    let dir = TempDir::new().unwrap();
    let output = dir.path().join("b0000001a.output");
    let (state, _path, sub_dir, db) = t445_live(
        &dir,
        &[r#"{"type":"user","message":{"content":"go"}}"#.to_owned()],
    );
    fs::create_dir_all(&sub_dir).unwrap();
    let sub_path = sub_dir.join("agent-a1.jsonl");
    t445_append(
        &sub_path,
        &t445_bash_start("toolu_test_1", "b0000001a", &output, Some("a1")),
    );

    let events = t425_tick(&state);
    let starts = t445_starts(&events);
    let [
        ClaudeEvent::BackgroundStart {
            agent_id, initial, ..
        },
    ] = starts.as_slice()
    else {
        panic!("expected one BackgroundStart, got {events:?}");
    };
    assert_eq!(agent_id.as_deref(), Some("a1"));
    assert!(!initial);

    t445_append(&sub_path, &[t445_sub_end_turn("a1")]);
    let events = t425_tick(&state);
    let order: Vec<&str> = events
        .iter()
        .filter_map(|e| match e {
            ClaudeEvent::SubagentEnd { .. } => Some("subagent_end"),
            ClaudeEvent::BackgroundEnd { .. } => Some("background_end"),
            _ => None,
        })
        .collect();
    assert_eq!(order, ["subagent_end", "background_end"], "{events:?}");
    assert!(
        matches!(
            t445_ends(&events).as_slice(),
            [ClaudeEvent::BackgroundEnd {
                status: OutcomeStatus::SubagentEnded,
                agent_id: Some(a),
                ..
            }] if a == "a1"
        ),
        "{events:?}"
    );
    assert!(t445_end_rows(&db).is_empty(), "no row for a dropped task");
}

/// Case 6, attach: a finished subagent's unfinished task is not seeded.
#[test]
fn t445_attach_finished_subagent_task_is_not_seeded() {
    let dir = TempDir::new().unwrap();
    let output = dir.path().join("b0000001a.output");
    fs::write(&output, "running\n").unwrap();
    let path = dir.path().join("session.jsonl");
    t445_append(
        &path,
        &[r#"{"type":"user","message":{"content":"go"}}"#.to_owned()],
    );
    let sub_dir = skein_harness::claude::subagents_dir(&path).unwrap();
    fs::create_dir_all(&sub_dir).unwrap();
    let sub_path = sub_dir.join("agent-a1.jsonl");
    t445_append(
        &sub_path,
        &t445_bash_start("toolu_test_1", "b0000001a", &output, Some("a1")),
    );
    t445_append(&sub_path, &[t445_sub_end_turn("a1")]);

    let (_mgr, rx, db) = t445_attach(&path, &dir);
    let events = drain_brief(&rx);
    assert!(t445_starts(&events).is_empty(), "{events:?}");
    assert!(t445_ends(&events).is_empty(), "{events:?}");
    assert_eq!(t445_end_rows(&db), Vec::<serde_json::Value>::new());
}

/// Case 6, attach, other side: the same task under a subagent that is
/// still running is seeded, with its owner.
#[test]
fn t445_attach_live_subagent_task_is_seeded_with_owner() {
    let dir = TempDir::new().unwrap();
    let output = dir.path().join("b0000001a.output");
    fs::write(&output, "running\n").unwrap();
    let path = dir.path().join("session.jsonl");
    t445_append(
        &path,
        &[r#"{"type":"user","message":{"content":"go"}}"#.to_owned()],
    );
    let sub_dir = skein_harness::claude::subagents_dir(&path).unwrap();
    fs::create_dir_all(&sub_dir).unwrap();
    t445_append(
        &sub_dir.join("agent-a1.jsonl"),
        &t445_bash_start("toolu_test_1", "b0000001a", &output, Some("a1")),
    );

    let (_mgr, rx, _db) = t445_attach(&path, &dir);
    let events = drain_brief(&rx);
    assert!(
        matches!(
            t445_starts(&events).as_slice(),
            [ClaudeEvent::BackgroundStart {
                agent_id: Some(a),
                initial: true,
                ..
            }] if a == "a1"
        ),
        "{events:?}"
    );
}

/// Case 7: history is never replayed as live.
#[test]
fn t445_attach_history_is_not_replayed_as_live() {
    let dir = TempDir::new().unwrap();
    let out = |id: &str| dir.path().join(format!("{id}.output"));
    let path = dir.path().join("session.jsonl");
    // Three finished (notification, TaskStop, failure), one outstanding.
    t445_append(
        &path,
        &t445_bash_start("toolu_test_1", "b0000001a", &out("b0000001a"), None),
    );
    t445_append(
        &path,
        &t445_notified(&t445_notif("b0000001a", "completed", "done (exit code 0)")),
    );
    t445_append(
        &path,
        &t445_bash_start("toolu_test_2", "b0000002a", &out("b0000002a"), None),
    );
    t445_append(
        &path,
        &[
            t445_use(
                "TaskStop",
                "toolu_test_s",
                &serde_json::json!({"task_id":"b0000002a"}),
                None,
            ),
            t445_result(
                "toolu_test_s",
                "stopped",
                &serde_json::json!({"task_id":"b0000002a"}),
                None,
            ),
        ],
    );
    t445_append(
        &path,
        &t445_bash_start("toolu_test_3", "b0000003a", &out("b0000003a"), None),
    );
    t445_append(
        &path,
        &t445_notified(&t445_notif(
            "b0000003a",
            "failed",
            "failed with exit code 2",
        )),
    );
    fs::write(out("b0000004a"), "running\n").unwrap();
    t445_append(
        &path,
        &t445_bash_start("toolu_test_4", "b0000004a", &out("b0000004a"), None),
    );

    let (_mgr, rx, db) = t445_attach(&path, &dir);
    let events = drain_brief(&rx);
    let starts = t445_starts(&events);
    let [
        ClaudeEvent::BackgroundStart {
            task_id, initial, ..
        },
    ] = starts.as_slice()
    else {
        panic!("expected only the outstanding task, got {events:?}");
    };
    assert_eq!(task_id, "b0000004a");
    assert!(initial);
    assert!(t445_ends(&events).is_empty(), "{events:?}");

    // A live tick over an unrelated row says nothing about the history.
    t445_append(
        &path,
        &[r#"{"type":"assistant","message":{"stop_reason":"end_turn","content":[]}}"#.to_owned()],
    );
    let more = drain(&rx);
    assert!(
        more.iter()
            .any(|e| matches!(e, ClaudeEvent::AwaitingPrompt)),
        "the tick never ran: {more:?}"
    );
    assert!(t445_starts(&more).is_empty(), "{more:?}");
    assert!(t445_ends(&more).is_empty(), "{more:?}");
    assert_eq!(t445_end_rows(&db), Vec::<serde_json::Value>::new());
}

/// Order independence at the adapter: a subagent's task terminal sits in
/// the MAIN file, which a tick reads before the subagent file that holds
/// the start.
#[test]
fn t445_terminal_in_main_before_start_in_subagent_same_tick() {
    let dir = TempDir::new().unwrap();
    let output = dir.path().join("b0000001a.output");
    let (state, _path, sub_dir, db) = t445_live(
        &dir,
        &t445_notified(&t445_notif("b0000001a", "completed", "done (exit code 0)")),
    );
    fs::create_dir_all(&sub_dir).unwrap();
    t445_append(
        &sub_dir.join("agent-a1.jsonl"),
        &t445_bash_start("toolu_test_1", "b0000001a", &output, Some("a1")),
    );

    let events = t425_tick(&state);
    assert_eq!(t445_starts(&events).len(), 1, "{events:?}");
    assert!(
        matches!(
            t445_ends(&events).as_slice(),
            [ClaudeEvent::BackgroundEnd {
                status: OutcomeStatus::Completed,
                exit_code: Some(0),
                agent_id: Some(a),
                ..
            }] if a == "a1"
        ),
        "{events:?}"
    );
    assert_eq!(t445_end_rows(&db).len(), 1);
    assert_eq!(
        state.lock().background.tasks.outstanding(now_ms()),
        Vec::<&skein_harness::claude::background::BackgroundTask>::new()
    );
}

/// Gate: an end for a task whose start was never announced updates the
/// tracker and says nothing.
#[test]
fn t445_end_for_unannounced_task_is_silent() {
    let dir = TempDir::new().unwrap();
    let output = dir.path().join("b0000001a.output");
    let mut bg = BackgroundState::default();
    for row in t445_bash_start("toolu_test_1", "b0000001a", &output, None) {
        bg.feed(&serde_json::from_str(&row).unwrap(), None, None);
    }
    let mut events = Vec::new();
    let mut rows = Vec::new();
    for row in t445_notified(&t445_notif("b0000001a", "completed", "done (exit code 0)")) {
        bg.feed(
            &serde_json::from_str(&row).unwrap(),
            None,
            Some(&mut BackgroundOut {
                harness_id: "h1",
                events: &mut events,
                rows: &mut rows,
            }),
        );
    }
    assert!(events.is_empty() && rows.is_empty(), "{events:?}");
    assert_eq!(
        bg.tasks.outstanding(now_ms()),
        Vec::<&skein_harness::claude::background::BackgroundTask>::new()
    );
}

/// Attach over a Monitor that passed its deadline with no notice: it
/// is ended in the tracker at attach, so no tick ever sweeps it into
/// an end nobody was told the start of.
#[test]
fn t445_attach_past_deadline_monitor_then_tick_is_silent() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("session.jsonl");
    t445_append(
        &path,
        &t445_monitor_start("toolu_test_1", "b0000004a", 15_000),
    );

    let (_mgr, rx, db) = t445_attach(&path, &dir);
    let events = drain_brief(&rx);
    assert!(t445_starts(&events).is_empty(), "{events:?}");
    assert!(t445_ends(&events).is_empty(), "{events:?}");

    t445_append(
        &path,
        &[r#"{"type":"user","message":{"content":"x"}}"#.to_owned()],
    );
    let more = drain(&rx);
    assert!(!more.is_empty(), "the tick never ran");
    assert!(t445_starts(&more).is_empty(), "{more:?}");
    assert!(t445_ends(&more).is_empty(), "{more:?}");
    assert_eq!(t445_end_rows(&db), Vec::<serde_json::Value>::new());
}

/// Resync: a task announced live that the new content lacks gets
/// exactly one `unknown` end, and no row.
#[test]
fn t445_resync_closes_announced_task_missing_from_new_content() {
    let dir = TempDir::new().unwrap();
    let output = dir.path().join("b0000001a.output");
    let (state, path, _sub, db) = t445_live(&dir, &[]);
    t445_append(
        &path,
        &t445_bash_start("toolu_test_1", "b0000001a", &output, None),
    );
    let events = t425_tick(&state);
    assert_eq!(t445_starts(&events).len(), 1, "{events:?}");

    // Replaced by something shorter: the truncation path resyncs.
    fs::write(
        &path,
        "{\"type\":\"user\",\"message\":{\"content\":\"x\"}}
",
    )
    .unwrap();
    let events = t425_tick(&state);
    assert!(
        matches!(
            t445_ends(&events).as_slice(),
            [ClaudeEvent::BackgroundEnd {
                task_id,
                status: OutcomeStatus::Unknown,
                ..
            }] if task_id == "b0000001a"
        ),
        "{events:?}"
    );
    assert_eq!(t445_end_rows(&db), Vec::<serde_json::Value>::new());

    // Nothing is left to end.
    t445_append(
        &path,
        &[r#"{"type":"user","message":{"content":"y"}}"#.to_owned()],
    );
    assert!(t445_ends(&t425_tick(&state)).is_empty());
}

/// Resync after the sweep already ended a Monitor: the rebuilt tracker
/// has it outstanding again, but it is neither ended nor started a
/// second time.
#[test]
fn t445_resync_after_sweep_does_not_end_twice() {
    let dir = TempDir::new().unwrap();
    let padding = serde_json::json!({"type":"user","message":{"content":"p".repeat(200)}});
    let mut rows = vec![padding.to_string()];
    rows.extend(t445_monitor_start("toolu_test_1", "b0000004a", 15_000));
    let (state, path, _sub, db) = t445_live(&dir, &rows);
    let events = t425_tick(&state);
    assert_eq!(t445_starts(&events).len(), 1, "{events:?}");
    assert_eq!(t445_ends(&events).len(), 1, "{events:?}");
    assert_eq!(t445_end_rows(&db).len(), 1);

    // Same Monitor rows, minus the padding: shorter, so it resyncs.
    fs::write(
        &path,
        format!(
            "{}
",
            rows[1..].join(
                "
"
            )
        ),
    )
    .unwrap();
    let events = t425_tick(&state);
    assert!(t445_starts(&events).is_empty(), "{events:?}");
    assert!(t445_ends(&events).is_empty(), "{events:?}");
    assert_eq!(t445_end_rows(&db).len(), 1, "a second row");
}

/// Re-seed: a subagent file that owns an announced task shrinks to a
/// finished transcript. The task ends once as `subagent_ended` (the
/// owner is finished), with the owner's id, no row and no start.
#[test]
fn t445_subagent_reseed_closes_owned_announced_task() {
    let dir = TempDir::new().unwrap();
    let output = dir.path().join("b0000001a.output");
    let (state, _path, sub_dir, db) = t445_live(
        &dir,
        &[r#"{"type":"user","message":{"content":"go"}}"#.to_owned()],
    );
    fs::create_dir_all(&sub_dir).unwrap();
    let sub_path = sub_dir.join("agent-a1.jsonl");
    t445_append(
        &sub_path,
        &t445_bash_start("toolu_test_1", "b0000001a", &output, Some("a1")),
    );
    let events = t425_tick(&state);
    assert_eq!(t445_starts(&events).len(), 1, "{events:?}");

    fs::write(
        &sub_path,
        format!(
            "{}
",
            t445_sub_end_turn("a1")
        ),
    )
    .unwrap();
    let events = t425_tick(&state);
    assert!(t445_starts(&events).is_empty(), "{events:?}");
    assert!(
        matches!(
            t445_ends(&events).as_slice(),
            [ClaudeEvent::BackgroundEnd {
                status: OutcomeStatus::SubagentEnded,
                agent_id: Some(a),
                ..
            }] if a == "a1"
        ),
        "{events:?}"
    );
    assert_eq!(t445_end_rows(&db), Vec::<serde_json::Value>::new());
}

/// Case 2c: a resync that is the first to see an outstanding task
/// announces it once, as an initial start.
#[test]
fn t445_resync_announces_never_announced_outstanding_task_once() {
    let dir = TempDir::new().unwrap();
    let output = dir.path().join("b0000001a.output");
    fs::write(
        &output, "running
",
    )
    .unwrap();
    let padding = serde_json::json!({"type":"user","message":{"content":"p".repeat(3000)}});
    let (state, path, _sub, _db) = t445_live(&dir, &[padding.to_string()]);
    t425_tick(&state);

    let rows = t445_bash_start("toolu_test_1", "b0000001a", &output, None);
    fs::write(
        &path,
        format!(
            "{}
",
            rows.join(
                "
"
            )
        ),
    )
    .unwrap();
    let events = t425_tick(&state);
    assert!(
        matches!(
            t445_starts(&events).as_slice(),
            [ClaudeEvent::BackgroundStart {
                task_id,
                initial: true,
                ..
            }] if task_id == "b0000001a"
        ),
        "{events:?}"
    );
    assert!(t445_ends(&events).is_empty(), "{events:?}");

    t445_append(
        &path,
        &[r#"{"type":"user","message":{"content":"y"}}"#.to_owned()],
    );
    let events = t425_tick(&state);
    assert!(t445_starts(&events).is_empty(), "{events:?}");
    assert!(t445_ends(&events).is_empty(), "{events:?}");
}

/// Resync keeps ownership: an announced subagent-owned task that the
/// new content (and the vanished subagent file) no longer shows ends
/// as `subagent_ended` with its `agent_id`, not `unknown`.
#[test]
fn t445_resync_carries_owner_to_the_closing_end() {
    let dir = TempDir::new().unwrap();
    let output = dir.path().join("b0000001a.output");
    let padding = serde_json::json!({"type":"user","message":{"content":"p".repeat(500)}});
    let (state, path, sub_dir, db) = t445_live(&dir, &[padding.to_string()]);
    fs::create_dir_all(&sub_dir).unwrap();
    let sub_path = sub_dir.join("agent-a1.jsonl");
    t445_append(
        &sub_path,
        &t445_bash_start("toolu_test_1", "b0000001a", &output, Some("a1")),
    );
    let events = t425_tick(&state);
    assert_eq!(t445_starts(&events).len(), 1, "{events:?}");

    fs::remove_file(&sub_path).unwrap();
    fs::write(
        &path,
        "{\"type\":\"user\",\"message\":{\"content\":\"x\"}}
",
    )
    .unwrap();
    let events = t425_tick(&state);
    assert!(
        matches!(
            t445_ends(&events).as_slice(),
            [ClaudeEvent::BackgroundEnd {
                status: OutcomeStatus::SubagentEnded,
                agent_id: Some(a),
                ..
            }] if a == "a1"
        ),
        "{events:?}"
    );
    assert_eq!(t445_end_rows(&db), Vec::<serde_json::Value>::new());
}
