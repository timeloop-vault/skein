//! Background tasks (#445): helpers, live starts and ends, and attach seeding.

use super::adapter::TailState;
use super::background_tasks::{
    BACKGROUND_END, BackgroundOut, BackgroundState, MONITOR_EXPIRY_GRACE_MS,
};
use super::resync_tests::{t425_persistence, t425_state, t425_tick};
use super::tests::{append_lines, drain_brief, test_manager};
use super::*;
use parking_lot::Mutex;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc;
use tempfile::TempDir;

// ── #445: background tasks ──────────────────────────────────────

const T445_TS: &str = "2026-01-01T00:00:00.000Z";

const T445_TS_END: &str = "2026-01-01T00:00:05.000Z";

/// `T445_TS` as epoch ms.
const T445_START_MS: i64 = 1_767_225_600_000;

/// Tags a row as a subagent's own when `agent` is given.
fn t445_row(mut row: serde_json::Value, agent: Option<&str>) -> String {
    if let (Some(agent), Some(obj)) = (agent, row.as_object_mut()) {
        obj.insert("isSidechain".into(), true.into());
        obj.insert("agentId".into(), agent.into());
    }
    row.to_string()
}

pub(super) fn t445_use(
    name: &str,
    id: &str,
    input: &serde_json::Value,
    agent: Option<&str>,
) -> String {
    t445_row(
        serde_json::json!({"type":"assistant","timestamp":T445_TS,"message":{"content":[
            {"type":"tool_use","id":id,"name":name,"input":input}]}}),
        agent,
    )
}

pub(super) fn t445_result(
    use_id: &str,
    text: &str,
    tur: &serde_json::Value,
    agent: Option<&str>,
) -> String {
    t445_row(
        serde_json::json!({"type":"user","timestamp":T445_TS,"toolUseResult":tur,
            "message":{"content":[
                {"type":"tool_result","tool_use_id":use_id,"content":text,"is_error":false}]}}),
        agent,
    )
}

/// A background Bash start: `tool_use` + `tool_result`, the result
/// naming `output` (which the caller creates, or not).
pub(super) fn t445_bash_start(
    use_id: &str,
    task_id: &str,
    output: &Path,
    agent: Option<&str>,
) -> Vec<String> {
    let input =
        serde_json::json!({"command":"make","description":"build","run_in_background":true});
    let text = format!(
        "Command running in background with ID: {task_id}. Output is being written to: {}. You will be notified",
        output.display()
    );
    vec![
        t445_use("Bash", use_id, &input, agent),
        t445_result(
            use_id,
            &text,
            &serde_json::json!({"backgroundTaskId":task_id}),
            agent,
        ),
    ]
}

pub(super) fn t445_monitor_start(use_id: &str, task_id: &str, timeout_ms: u64) -> Vec<String> {
    let input = serde_json::json!({"description":"watch","timeout_ms":timeout_ms,"command":"tail","persistent":false});
    vec![
        t445_use("Monitor", use_id, &input, None),
        t445_result(
            use_id,
            "Monitor started",
            &serde_json::json!({"taskId":task_id,"timeoutMs":timeout_ms,"persistent":false}),
            None,
        ),
    ]
}

pub(super) fn t445_notif(task_id: &str, status: &str, summary: &str) -> String {
    format!(
        "<task-notification>\n<task-id>{task_id}</task-id>\n<tool-use-id>toolu_test_1</tool-use-id>\n\
         <status>{status}</status>\n<summary>{summary}</summary>\n</task-notification>"
    )
}

fn t445_enqueue(text: &str) -> String {
    serde_json::json!({"type":"queue-operation","operation":"enqueue","timestamp":T445_TS_END,"content":text})
        .to_string()
}

fn t445_delivery(text: &str) -> String {
    serde_json::json!({"type":"user","timestamp":T445_TS_END,"message":{"role":"user","content":text}})
        .to_string()
}

/// Both carriers of one notification, as Claude Code writes them.
pub(super) fn t445_notified(text: &str) -> Vec<String> {
    vec![t445_enqueue(text), t445_delivery(text)]
}

pub(super) fn t445_sub_end_turn(agent: &str) -> String {
    t445_row(
        serde_json::json!({"type":"assistant","timestamp":T445_TS_END,
            "message":{"stop_reason":"end_turn","content":[]}}),
        Some(agent),
    )
}

pub(super) fn t445_append(path: &Path, rows: &[String]) {
    let refs: Vec<&str> = rows.iter().map(String::as_str).collect();
    append_lines(path, &refs);
}

pub(super) fn t445_starts(events: &[ClaudeEvent]) -> Vec<&ClaudeEvent> {
    events
        .iter()
        .filter(|e| matches!(e, ClaudeEvent::BackgroundStart { .. }))
        .collect()
}

pub(super) fn t445_ends(events: &[ClaudeEvent]) -> Vec<&ClaudeEvent> {
    events
        .iter()
        .filter(|e| matches!(e, ClaudeEvent::BackgroundEnd { .. }))
        .collect()
}

/// Payloads of every persisted `background_end` row in room `r1`.
pub(super) fn t445_end_rows(db: &crate::db::Database) -> Vec<serde_json::Value> {
    db.recent_harness_actions_by_room_and_kind("r1", BACKGROUND_END, -1, 100)
        .unwrap()
        .iter()
        .map(|a| serde_json::from_str(&a.payload).unwrap())
        .collect()
}

/// A live tail on a main transcript that already exists, with the
/// subagents dir set. The first tick reads from byte 0 as live.
pub(super) fn t445_live(
    dir: &TempDir,
    main_rows: &[String],
) -> (
    Arc<Mutex<TailState>>,
    PathBuf,
    PathBuf,
    Arc<crate::db::Database>,
) {
    let path = dir.path().join("session.jsonl");
    let sub_dir = skein_harness::claude::subagents_dir(&path).unwrap();
    let (db, ap) = t425_persistence(&dir.path().join("t.db"));
    let state = t425_state(&path, Some(ap));
    state.lock().subagents_dir = Some(sub_dir.clone());
    t445_append(&path, main_rows);
    (state, path, sub_dir, db)
}

/// Attach (full path, history fed, no live tick yet) over whatever is
/// on disk, persisting into a fresh db.
pub(super) fn t445_attach(
    path: &Path,
    dir: &TempDir,
) -> (
    ClaudeEventsManager,
    mpsc::Receiver<ClaudeEvent>,
    Arc<crate::db::Database>,
) {
    let (db, ap) = t425_persistence(&dir.path().join("t.db"));
    let (tx, rx) = mpsc::channel();
    let manager = test_manager();
    manager
        .attach_at(
            "h1",
            path.to_path_buf(),
            move |e| tx.send(e).unwrap(),
            Some(ap),
        )
        .unwrap();
    (manager, rx, db)
}

/// Case 1.
#[test]
fn t445_live_start_then_notification_ends_once_with_row() {
    let dir = TempDir::new().unwrap();
    let output = dir.path().join("b0000001a.output");
    let (state, path, _sub, db) = t445_live(&dir, &[]);
    t445_append(
        &path,
        &t445_bash_start("toolu_test_1", "b0000001a", &output, None),
    );

    let events = t425_tick(&state);
    let starts = t445_starts(&events);
    let [
        ClaudeEvent::BackgroundStart {
            task_id,
            tool_use_id,
            task_kind,
            description,
            agent_id,
            initial,
            ..
        },
    ] = starts.as_slice()
    else {
        panic!("expected exactly one BackgroundStart, got {events:?}");
    };
    assert_eq!(task_id, "b0000001a");
    assert_eq!(tool_use_id, "toolu_test_1");
    assert_eq!(*task_kind, BackgroundKind::Bash);
    assert_eq!(description.as_deref(), Some("build"));
    assert!(agent_id.is_none() && !initial);
    assert!(t445_ends(&events).is_empty(), "{events:?}");

    // The notification arrives as the enqueue and the delivery row.
    t445_append(
        &path,
        &t445_notified(&t445_notif("b0000001a", "completed", "done (exit code 0)")),
    );
    let events = t425_tick(&state);
    let ends = t445_ends(&events);
    let [
        ClaudeEvent::BackgroundEnd {
            task_id,
            status,
            exit_code,
            ..
        },
    ] = ends.as_slice()
    else {
        panic!("expected exactly one BackgroundEnd, got {events:?}");
    };
    assert_eq!(task_id, "b0000001a");
    assert_eq!(*status, OutcomeStatus::Completed);
    assert_eq!(*exit_code, Some(0));
    assert!(t445_starts(&events).is_empty(), "{events:?}");

    // Only the delivery row is a user row; the enqueue is not.
    let prompts: Vec<bool> = events
        .iter()
        .filter_map(|e| match e {
            ClaudeEvent::UserPrompt { task_notification } => Some(*task_notification),
            _ => None,
        })
        .collect();
    assert_eq!(prompts, vec![true], "{events:?}");

    let rows = t445_end_rows(&db);
    let [row] = rows.as_slice() else {
        panic!("expected one background_end row, got {rows:?}");
    };
    assert_eq!(row["task_id"], "b0000001a");
    assert_eq!(row["status"], "completed");
    assert_eq!(row["exit_code"], 0);
    assert_eq!(row["duration_ms"], 5000);

    // A later tick re-fires nothing, and a typed prompt is not a notification.
    t445_append(
        &path,
        &[
            serde_json::json!({"type":"user","timestamp":T445_TS_END,"message":{"content":"next"}})
                .to_string(),
        ],
    );
    let events = t425_tick(&state);
    assert!(
        matches!(
            events.as_slice(),
            [ClaudeEvent::UserPrompt {
                task_notification: false
            }]
        ),
        "{events:?}"
    );
    assert_eq!(t445_end_rows(&db).len(), 1);
}

/// Case 2.
#[test]
fn t445_attach_over_outstanding_task_seeds_initial_start() {
    let dir = TempDir::new().unwrap();
    let output = dir.path().join("b0000001a.output");
    fs::write(&output, "still going\n").unwrap();
    let path = dir.path().join("session.jsonl");
    t445_append(
        &path,
        &t445_bash_start("toolu_test_1", "b0000001a", &output, None),
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
        panic!("expected one initial BackgroundStart, got {events:?}");
    };
    assert_eq!(task_id, "b0000001a");
    assert!(initial);
    assert!(t445_ends(&events).is_empty(), "{events:?}");
    assert!(t445_end_rows(&db).is_empty());
}

/// Case 3.
#[test]
fn t445_attach_over_finished_task_seeds_nothing() {
    let dir = TempDir::new().unwrap();
    let output = dir.path().join("b0000001a.output");
    fs::write(&output, "done\n").unwrap();
    let path = dir.path().join("session.jsonl");
    t445_append(
        &path,
        &t445_bash_start("toolu_test_1", "b0000001a", &output, None),
    );
    t445_append(
        &path,
        &t445_notified(&t445_notif("b0000001a", "completed", "done (exit code 0)")),
    );

    let (_mgr, rx, db) = t445_attach(&path, &dir);
    let events = drain_brief(&rx);
    assert!(t445_starts(&events).is_empty(), "{events:?}");
    assert!(t445_ends(&events).is_empty(), "{events:?}");
    assert!(t445_end_rows(&db).is_empty());
}

/// Case 4, attach half: a `[killed]` trailer is the only record.
#[test]
fn t445_attach_killed_trailer_seeds_nothing() {
    let dir = TempDir::new().unwrap();
    let output = dir.path().join("b0000001a.output");
    fs::write(&output, "work\n[killed]\n").unwrap();
    let path = dir.path().join("session.jsonl");
    t445_append(
        &path,
        &t445_bash_start("toolu_test_1", "b0000001a", &output, None),
    );

    let (_mgr, rx, db) = t445_attach(&path, &dir);
    let events = drain_brief(&rx);
    assert!(t445_starts(&events).is_empty(), "{events:?}");
    assert!(t445_ends(&events).is_empty(), "{events:?}");
    assert!(t445_end_rows(&db).is_empty());
}

/// Case 4, live half: `TaskStop` leaves no notification.
#[test]
fn t445_live_task_stop_ends_as_task_stopped() {
    let dir = TempDir::new().unwrap();
    let output = dir.path().join("b0000001a.output");
    let (state, path, _sub, db) = t445_live(&dir, &[]);
    t445_append(
        &path,
        &t445_bash_start("toolu_test_1", "b0000001a", &output, None),
    );
    t425_tick(&state);

    t445_append(
        &path,
        &[
            t445_use(
                "TaskStop",
                "toolu_test_2",
                &serde_json::json!({"task_id":"b0000001a"}),
                None,
            ),
            t445_result(
                "toolu_test_2",
                "Successfully stopped task",
                &serde_json::json!({"task_id":"b0000001a","task_type":"local_bash"}),
                None,
            ),
        ],
    );
    let events = t425_tick(&state);
    let ends = t445_ends(&events);
    assert!(
        matches!(
            ends.as_slice(),
            [ClaudeEvent::BackgroundEnd {
                status: OutcomeStatus::TaskStopped,
                ..
            }]
        ),
        "{events:?}"
    );
    let rows = t445_end_rows(&db);
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0]["status"], "task_stopped");
}

/// Case 5a: the pinned expiry notice, no `<status>`.
#[test]
fn t445_monitor_expiry_notice_ends_as_expired_once() {
    let dir = TempDir::new().unwrap();
    let (state, path, _sub, db) = t445_live(&dir, &[]);
    t445_append(
        &path,
        &t445_monitor_start("toolu_test_1", "b0000004a", 15_000),
    );
    let notice = "<task-notification>\n<task-id>b0000004a</task-id>\n\
        <summary>Monitor \"watch\" expired</summary>\n\
        <event>[Monitor expired after 15s with 1 event delivered. Re-arm it if you still need it.]</event>\n\
        </task-notification>";
    t445_append(&path, &t445_notified(notice));

    // One tick: the start, then the notice, before any sweep.
    let events = t425_tick(&state);
    assert_eq!(t445_starts(&events).len(), 1, "{events:?}");
    let ends = t445_ends(&events);
    assert!(
        matches!(
            ends.as_slice(),
            [ClaudeEvent::BackgroundEnd {
                status: OutcomeStatus::Expired,
                task_kind: BackgroundKind::Monitor,
                ..
            }]
        ),
        "{events:?}"
    );
    assert_eq!(t445_end_rows(&db).len(), 1);

    // The sweep, running on every tick, has nothing left to do.
    t445_append(
        &path,
        &[r#"{"type":"user","message":{"content":"x"}}"#.into()],
    );
    let events = t425_tick(&state);
    assert!(t445_ends(&events).is_empty(), "{events:?}");
    assert_eq!(t445_end_rows(&db).len(), 1);
}

/// Case 5b: a Monitor past its deadline with no notice at all is ended
/// by the tick's sweep, once.
#[test]
fn t445_monitor_without_notice_is_swept_once_by_tick() {
    let dir = TempDir::new().unwrap();
    let (state, path, _sub, db) = t445_live(&dir, &[]);
    // The rows are stamped months ago, so the deadline is long gone.
    t445_append(
        &path,
        &t445_monitor_start("toolu_test_1", "b0000004a", 15_000),
    );

    let events = t425_tick(&state);
    assert_eq!(t445_starts(&events).len(), 1, "{events:?}");
    assert!(
        matches!(
            t445_ends(&events).as_slice(),
            [ClaudeEvent::BackgroundEnd {
                status: OutcomeStatus::Expired,
                ..
            }]
        ),
        "{events:?}"
    );
    assert_eq!(t445_end_rows(&db).len(), 1);

    t445_append(
        &path,
        &[r#"{"type":"user","message":{"content":"x"}}"#.into()],
    );
    let events = t425_tick(&state);
    assert!(t445_ends(&events).is_empty(), "{events:?}");
    assert_eq!(t445_end_rows(&db).len(), 1, "swept again on a later tick");
}

/// Case 5c: the sweep's clock is start + timeout + grace, exactly.
#[test]
fn t445_sweep_waits_for_deadline_plus_grace() {
    let mut bg = BackgroundState::default();
    let mut events = Vec::new();
    let mut rows = Vec::new();
    for row in t445_monitor_start("toolu_test_1", "b0000004a", 15_000) {
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
    // The start is announced, which is what lets the sweep end it.
    assert_eq!(events.len(), 1, "{events:?}");
    events.clear();
    let due = T445_START_MS + 15_000 + MONITOR_EXPIRY_GRACE_MS;
    let sweep = |bg: &mut BackgroundState, now, events: &mut Vec<_>, rows: &mut Vec<_>| {
        bg.sweep(
            now,
            &mut BackgroundOut {
                harness_id: "h1",
                events,
                rows,
            },
        );
    };
    sweep(&mut bg, T445_START_MS + 15_000, &mut events, &mut rows);
    sweep(&mut bg, due - 1, &mut events, &mut rows);
    assert!(events.is_empty() && rows.is_empty());
    sweep(&mut bg, due, &mut events, &mut rows);
    sweep(&mut bg, due + 10_000, &mut events, &mut rows);
    assert!(
        matches!(
            events.as_slice(),
            [ClaudeEvent::BackgroundEnd {
                status: OutcomeStatus::Expired,
                ..
            }]
        ),
        "{events:?}"
    );
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].kind, BACKGROUND_END);
}
