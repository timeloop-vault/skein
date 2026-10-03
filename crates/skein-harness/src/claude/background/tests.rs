use super::*;
use serde_json::json;

const TS: &str = "2026-01-01T00:00:00.000Z";
const TS_MS: i64 = 1_767_225_600_000;

fn use_row(name: &str, id: &str, input: &Value) -> Value {
    json!({"type":"assistant","timestamp":TS,"message":{"content":[
            {"type":"tool_use","id":id,"name":name,"input":input}]}})
}

fn result_row(use_id: &str, text: &str, is_error: bool, tur: &Value) -> Value {
    json!({"type":"user","timestamp":TS,"toolUseResult":tur,"message":{"content":[
            {"type":"tool_result","tool_use_id":use_id,"content":text,"is_error":is_error}]}})
}

fn enqueue(text: &str) -> Value {
    json!({"type":"queue-operation","operation":"enqueue","timestamp":TS,"content":text})
}

fn delivery(text: &str) -> Value {
    json!({"type":"user","message":{"role":"user","content":text}})
}

fn notif(id: &str, status: &str, summary: &str) -> String {
    format!(
        "<task-notification>\n<task-id>{id}</task-id>\n<tool-use-id>toolu_1</tool-use-id>\n\
             <output-file>/tmp/x/tasks/{id}.output</output-file>\n<status>{status}</status>\n\
             <summary>{summary}</summary>\n</task-notification>"
    )
}

fn bash_start(state: &mut BackgroundTasks, id: &str) -> Vec<BackgroundTransition> {
    let input =
        json!({"command":"make","description":"build","timeout":120_000,"run_in_background":true});
    state.observe(&use_row("Bash", "toolu_1", &input));
    let text = format!(
        "Command running in background with ID: {id}. Output is being written to: /tmp/x/tasks/{id}.output. You will be notified"
    );
    state.observe(&result_row(
        "toolu_1",
        &text,
        false,
        &json!({"backgroundTaskId":id}),
    ))
}

fn monitor_start(state: &mut BackgroundTasks, id: &str, persistent: bool) {
    let input = json!({"description":"watch","timeout_ms":600_000,"command":"tail","persistent":persistent});
    state.observe(&use_row("Monitor", "toolu_m", &input));
    state.observe(&result_row(
        "toolu_m",
        "Monitor started (task x)",
        false,
        &json!({"taskId":id,"timeoutMs":600_000,"persistent":persistent}),
    ));
}

#[test]
fn explicit_bash_start() {
    let mut s = BackgroundTasks::default();
    let out = bash_start(&mut s, "b0000001a");
    let [BackgroundTransition::Started(t)] = out.as_slice() else {
        panic!("{out:?}")
    };
    assert_eq!(t.task_id, "b0000001a");
    assert_eq!(t.tool_use_id, "toolu_1");
    assert_eq!(t.kind, BackgroundKind::Bash);
    assert!(!t.auto_backgrounded);
    assert_eq!(t.description.as_deref(), Some("build"));
    assert_eq!(t.command.as_deref(), Some("make"));
    assert_eq!(t.timeout_ms, Some(120_000));
    assert_eq!(
        t.output_file.as_deref(),
        Some("/tmp/x/tasks/b0000001a.output")
    );
    assert_eq!(t.started_ms, Some(TS_MS));
}

#[test]
fn powershell_start() {
    let mut s = BackgroundTasks::default();
    s.observe(&use_row(
        "PowerShell",
        "toolu_p",
        &json!({"command":"dir","run_in_background":true}),
    ));
    let out = s.observe(&result_row(
        "toolu_p",
        "Command running in background with ID: b0000002a.",
        false,
        &json!({"backgroundTaskId":"b0000002a"}),
    ));
    let [BackgroundTransition::Started(t)] = out.as_slice() else {
        panic!("{out:?}")
    };
    assert_eq!(t.kind, BackgroundKind::PowerShell);
    assert_eq!(t.output_file, None);
}

#[test]
fn auto_backgrounded_start() {
    let mut s = BackgroundTasks::default();
    s.observe(&use_row(
        "Bash",
        "toolu_a",
        &json!({"command":"sleep 999","description":"slow"}),
    ));
    let out = s.observe(&result_row(
            "toolu_a",
            "Command did not complete within its 120s timeout and was moved to the background (ID: b0000003a)",
            false,
            &json!({"backgroundTaskId":"b0000003a","timedOutAfterMs":120_000}),
        ));
    let [BackgroundTransition::Started(t)] = out.as_slice() else {
        panic!("{out:?}")
    };
    assert!(t.auto_backgrounded);
    assert_eq!(t.timeout_ms, Some(120_000));
    assert_eq!(t.output_file, None);
}

#[test]
fn monitor_start_persistent_or_not() {
    let mut s = BackgroundTasks::default();
    monitor_start(&mut s, "b0000004a", false);
    monitor_start(&mut s, "b0000005a", true);
    let a = s.task("b0000004a").unwrap();
    assert_eq!(
        (a.kind, a.persistent, a.timeout_ms),
        (BackgroundKind::Monitor, false, Some(600_000))
    );
    assert!(s.task("b0000005a").unwrap().persistent);
}

#[test]
fn error_result_is_not_a_start() {
    let mut s = BackgroundTasks::default();
    s.observe(&use_row(
        "Bash",
        "toolu_1",
        &json!({"command":"x","run_in_background":true}),
    ));
    let out = s.observe(&result_row(
        "toolu_1",
        "boom",
        true,
        &json!({"backgroundTaskId":"b0000006a"}),
    ));
    assert_eq!(out, Vec::<BackgroundTransition>::new());
    assert!(s.task("b0000006a").is_none());
}

#[test]
fn foreground_result_is_not_a_start() {
    let mut s = BackgroundTasks::default();
    s.observe(&use_row("Bash", "toolu_1", &json!({"command":"ls"})));
    let out = s.observe(&result_row(
        "toolu_1",
        "file",
        false,
        &json!({"stdout":"file"}),
    ));
    assert_eq!(out, Vec::<BackgroundTransition>::new());
}

#[test]
fn status_and_exit_code_table() {
    let cases = [
        (
            "completed",
            "Background command \"build\" completed (exit code 0)",
            OutcomeStatus::Completed,
            Some(0),
        ),
        (
            "failed",
            "Background command \"build\" failed with exit code 2",
            OutcomeStatus::Failed,
            Some(2),
        ),
        (
            "killed",
            "Background command \"build\" was stopped",
            OutcomeStatus::Killed,
            None,
        ),
        (
            "stopped",
            "Background shell command didn't finish before the previous session ended",
            OutcomeStatus::Stopped,
            None,
        ),
        (
            "completed",
            "Monitor \"watch\" ended without producing output (exit 1)",
            OutcomeStatus::Completed,
            Some(1),
        ),
        (
            "completed",
            "Monitor \"watch\" stream ended",
            OutcomeStatus::Completed,
            None,
        ),
        ("paused", "something new", OutcomeStatus::Unknown, None),
    ];
    for (status, summary, want, code) in cases {
        let n = parse_task_notification(&notif("b0000001a", status, summary)).unwrap();
        let NotificationClass::Terminal(o) = classify_notification(&n) else {
            panic!("{status}")
        };
        assert_eq!((o.status, o.exit_code), (want, code), "{summary}");
        assert_eq!(o.summary.as_deref(), Some(summary));
    }
}

#[test]
fn multi_line_summary_and_any_tag_order() {
    let text = "<task-notification>\n<summary>Background command \"echo a\necho b\" completed (exit code 3)</summary>\n<status>completed</status>\n<task-id>b0000001a</task-id>\n</task-notification>";
    let n = parse_task_notification(text).unwrap();
    assert_eq!(n.task_id, "b0000001a");
    assert!(n.summary.as_deref().unwrap().contains("echo b"));
    let NotificationClass::Terminal(o) = classify_notification(&n) else {
        panic!()
    };
    assert_eq!(o.exit_code, Some(3));
}

#[test]
fn non_notifications_do_not_parse() {
    assert!(parse_task_notification("hello").is_none());
    assert!(parse_task_notification("<task-notification>\n</task-notification>").is_none());
}

#[test]
fn pinned_expiry_strings() {
    for event in [
        "[Monitor expired after 10m with no events delivered. Re-arm it if you still need it.]",
        "[Monitor expired after 15s with 1 event delivered. Re-arm it if you still need it.]",
    ] {
        let text = format!(
            "<task-notification>\n<task-id>b0000004a</task-id>\n<summary>Monitor event: \"watch\"</summary>\n<event>{event}</event>\n</task-notification>"
        );
        let n = parse_task_notification(&text).unwrap();
        let NotificationClass::Terminal(o) = classify_notification(&n) else {
            panic!("{event}")
        };
        assert_eq!(o.status, OutcomeStatus::Expired);
    }
}

#[test]
fn monitor_event_is_not_terminal() {
    let text = "<task-notification>\n<task-id>b0000004a</task-id>\n<summary>Monitor event: \"watch\"</summary>\n<event>line one</event>\nIf this matters, notify.\n</task-notification>";
    let n = parse_task_notification(text).unwrap();
    assert_eq!(
        classify_notification(&n),
        NotificationClass::MonitorEvent("line one".into())
    );
}

#[test]
fn three_carriers() {
    let text = notif("b0000001a", "completed", "done (exit code 0)");
    let rows = [
        enqueue(&text),
        delivery(&text),
        json!({"type":"user","message":{"content":[{"type":"text","text":text}]}}),
        json!({"type":"attachment","attachment":{"type":"queued_command","prompt":text}}),
    ];
    for row in &rows {
        assert_eq!(task_notification_text(row), Some(text.as_str()));
        let mut s = BackgroundTasks::default();
        bash_start(&mut s, "b0000001a");
        let out = s.observe(row);
        assert!(
            matches!(out.as_slice(), [BackgroundTransition::Ended { .. }]),
            "{row}"
        );
    }
    assert!(
        task_notification_text(&json!({"type":"queue-operation","operation":"dequeue"})).is_none()
    );
    assert!(
        task_notification_text(
            &json!({"type":"attachment","attachment":{"type":"other","prompt":text}})
        )
        .is_none()
    );
    assert!(task_notification_text(&delivery("plain prompt")).is_none());
}

#[test]
fn enqueue_then_delivery_ends_once() {
    let text = notif("b0000001a", "completed", "done (exit code 0)");
    let mut s = BackgroundTasks::default();
    bash_start(&mut s, "b0000001a");
    assert_eq!(s.observe(&enqueue(&text)).len(), 1);
    assert_eq!(
        s.observe(&delivery(&text)),
        Vec::<BackgroundTransition>::new()
    );
    assert_eq!(s.outstanding(TS_MS), Vec::<&BackgroundTask>::new());
}

#[test]
fn duplicate_start_is_ignored() {
    let mut s = BackgroundTasks::default();
    assert_eq!(bash_start(&mut s, "b0000001a").len(), 1);
    assert_eq!(
        bash_start(&mut s, "b0000001a"),
        Vec::<BackgroundTransition>::new()
    );
}

#[test]
fn task_stop_ends_the_task() {
    let mut s = BackgroundTasks::default();
    bash_start(&mut s, "b0000001a");
    s.observe(&use_row(
        "TaskStop",
        "toolu_s",
        &json!({"task_id":"b0000001a"}),
    ));
    let out = s.observe(&result_row(
        "toolu_s",
        "Successfully stopped task",
        false,
        &json!({"task_id":"b0000001a","task_type":"local_bash"}),
    ));
    let [BackgroundTransition::Ended { outcome, .. }] = out.as_slice() else {
        panic!("{out:?}")
    };
    assert_eq!(outcome.status, OutcomeStatus::TaskStopped);
    assert_eq!(s.outstanding(TS_MS), Vec::<&BackgroundTask>::new());
}

#[test]
fn error_task_stop_ends_nothing() {
    let mut s = BackgroundTasks::default();
    bash_start(&mut s, "b0000001a");
    s.observe(&use_row(
        "TaskStop",
        "toolu_s",
        &json!({"task_id":"b0000001a"}),
    ));
    s.observe(&result_row("toolu_s", "no such task", true, &json!({})));
    assert_eq!(s.outstanding(TS_MS).len(), 1);
}

#[test]
fn agent_notification_is_ignored() {
    let text = "<task-notification>\n<task-id>a0123456789abcdef</task-id>\n<status>completed</status>\n<summary>Agent done</summary>\n<note>extra</note>\n</task-notification>";
    let n = parse_task_notification(text).unwrap();
    assert_eq!(n.task_id, "a0123456789abcdef");
    let mut s = BackgroundTasks::default();
    bash_start(&mut s, "b0000001a");
    assert_eq!(
        s.observe(&enqueue(text)),
        Vec::<BackgroundTransition>::new()
    );
    assert_eq!(s.outstanding(TS_MS).len(), 1);
}

#[test]
fn monitor_event_yields_once_from_enqueue_only() {
    let text = "<task-notification>\n<task-id>b0000004a</task-id>\n<summary>Monitor event: \"watch\"</summary>\n<event>hit</event>\n</task-notification>";
    let mut s = BackgroundTasks::default();
    monitor_start(&mut s, "b0000004a", false);
    let out = s.observe(&enqueue(text));
    assert_eq!(
        out,
        vec![BackgroundTransition::MonitorEvent {
            task_id: "b0000004a".into(),
            line: "hit".into()
        }]
    );
    assert_eq!(
        s.observe(&delivery(text)),
        Vec::<BackgroundTransition>::new()
    );
    assert_eq!(s.outstanding(TS_MS).len(), 1);
}

#[test]
fn monitor_deadline_excludes_non_persistent_only() {
    let mut s = BackgroundTasks::default();
    monitor_start(&mut s, "b0000004a", false);
    monitor_start(&mut s, "b0000005a", true);
    bash_start(&mut s, "b0000001a");
    let ids = |now| {
        s.outstanding(now)
            .iter()
            .map(|t| t.task_id.clone())
            .collect::<Vec<_>>()
    };
    assert_eq!(ids(TS_MS + 600_000).len(), 3);
    assert_eq!(ids(TS_MS + 600_001), vec!["b0000005a", "b0000001a"]);
}

#[test]
fn terminal_before_start_gives_same_end_state() {
    let text = notif("b0000001a", "completed", "done (exit code 0)");
    let mut late = BackgroundTasks::default();
    assert_eq!(
        late.observe(&enqueue(&text)),
        Vec::<BackgroundTransition>::new()
    );
    let out = bash_start(&mut late, "b0000001a");
    assert!(matches!(
        out.as_slice(),
        [BackgroundTransition::Started(_), BackgroundTransition::Ended { outcome, .. }]
            if outcome.status == OutcomeStatus::Completed && outcome.exit_code == Some(0)
    ));
    assert_eq!(
        late.observe(&delivery(&text)),
        Vec::<BackgroundTransition>::new()
    );
    let mut normal = BackgroundTasks::default();
    bash_start(&mut normal, "b0000001a");
    normal.observe(&enqueue(&text));
    assert_eq!(late.outstanding(TS_MS), Vec::<&BackgroundTask>::new());
    assert_eq!(normal.outstanding(TS_MS), Vec::<&BackgroundTask>::new());
}

#[test]
fn task_stop_before_start_is_remembered() {
    let mut s = BackgroundTasks::default();
    s.observe(&use_row(
        "TaskStop",
        "toolu_s",
        &json!({"task_id":"b0000001a"}),
    ));
    s.observe(&result_row(
        "toolu_s",
        "ok",
        false,
        &json!({"task_id":"b0000001a"}),
    ));
    let out = bash_start(&mut s, "b0000001a");
    assert_eq!(out.len(), 2);
    assert_eq!(s.outstanding(TS_MS), Vec::<&BackgroundTask>::new());
}

#[test]
fn expire_overdue_respects_grace_and_persistence() {
    let mut s = BackgroundTasks::default();
    monitor_start(&mut s, "b0000004a", false);
    monitor_start(&mut s, "b0000005a", true);
    bash_start(&mut s, "b0000001a");
    assert_eq!(
        s.expire_overdue(TS_MS + 600_000 + 999, 1000),
        Vec::<BackgroundTransition>::new()
    );
    let out = s.expire_overdue(TS_MS + 600_000 + 1000, 1000);
    let [BackgroundTransition::Ended { task, outcome }] = out.as_slice() else {
        panic!("{out:?}")
    };
    assert_eq!(task.task_id, "b0000004a");
    assert_eq!(outcome, &Outcome::new(OutcomeStatus::Expired));
    assert_eq!(
        s.expire_overdue(TS_MS + 10_000_000, 0),
        Vec::<BackgroundTransition>::new()
    );
}

#[test]
fn serialized_names() {
    let mut s = BackgroundTasks::default();
    s.observe(&use_row(
        "PowerShell",
        "toolu_p",
        &json!({"command":"dir","run_in_background":true}),
    ));
    let out = s.observe(&result_row(
        "toolu_p",
        "ID: b0000002a",
        false,
        &json!({"backgroundTaskId":"b0000002a"}),
    ));
    let v = serde_json::to_value(&out[0]).unwrap();
    assert_eq!(v["transition"], "started");
    assert_eq!(v["task_kind"], "powershell");
    assert_eq!(
        serde_json::to_value(OutcomeStatus::SubagentEnded).unwrap(),
        "subagent_ended"
    );
    assert_eq!(
        serde_json::to_value(OutcomeStatus::TaskStopped).unwrap(),
        "task_stopped"
    );
}

#[test]
fn explicit_end() {
    let mut s = BackgroundTasks::default();
    bash_start(&mut s, "b0000001a");
    assert!(s.end("nope", Outcome::new(OutcomeStatus::Killed)).is_none());
    assert!(
        s.end("b0000001a", Outcome::new(OutcomeStatus::Killed))
            .is_some()
    );
    assert!(
        s.end("b0000001a", Outcome::new(OutcomeStatus::Killed))
            .is_none()
    );
}

#[test]
fn output_path_fallback() {
    let mut s = BackgroundTasks::default();
    assert_eq!(s.output_path("b0000009a"), None);
    // An auto-backgrounded start prints no path of its own.
    s.observe(&use_row("Bash", "toolu_a", &json!({"command":"sleep 9"})));
    s.observe(&result_row(
        "toolu_a",
        "moved to the background (ID: b0000003a)",
        false,
        &json!({"backgroundTaskId":"b0000003a","timedOutAfterMs":1000}),
    ));
    assert_eq!(s.output_path("b0000003a"), None);
    // Another task's notification teaches the directory.
    let win = "<task-notification>\n<task-id>b0000003a</task-id>\n<output-file>C:\\tmp\\tasks\\b0000003a.output</output-file>\n<status>completed</status>\n</task-notification>";
    s.observe(&enqueue(win));
    assert_eq!(
        s.output_path("b0000009a"),
        Some(PathBuf::from("C:\\tmp\\tasks\\b0000009a.output"))
    );
    assert_eq!(
        s.output_path("b0000003a"),
        Some(PathBuf::from("C:\\tmp\\tasks\\b0000003a.output"))
    );
}

#[test]
fn scan_skips_junk_lines() {
    let rows = [
        use_row(
            "Bash",
            "toolu_1",
            &json!({"command":"make","run_in_background":true}),
        )
        .to_string(),
        String::new(),
        "{not json".to_owned(),
        result_row(
            "toolu_1",
            "ID: b0000001a",
            false,
            &json!({"backgroundTaskId":"b0000001a"}),
        )
        .to_string(),
    ];
    let s = BackgroundTasks::scan(&rows.join("\n"));
    assert_eq!(s.outstanding(TS_MS).len(), 1);
    assert!(s.known_ids().contains("b0000001a"));
}

#[test]
fn trailer_table() {
    let cases = [
        ("[exited with code 0]", OutputTrailer::Exited(0)),
        ("  [exited with code 137]  ", OutputTrailer::Exited(137)),
        ("[exited with code -1]", OutputTrailer::Exited(-1)),
        ("[killed]", OutputTrailer::Killed),
        ("[exited with code x]", OutputTrailer::None),
        ("just output", OutputTrailer::None),
        ("", OutputTrailer::None),
    ];
    for (line, want) in cases {
        assert_eq!(parse_output_trailer(line), want, "{line:?}");
    }
}

#[test]
fn trailer_read_from_large_file_tail() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("b0000001a.output");
    let mut body = "x".repeat(1_000_000);
    body.push_str("\nlast real line\n[exited with code 4]\n\n");
    std::fs::write(&path, &body).unwrap();
    assert_eq!(read_output_trailer(&path), OutputTrailer::Exited(4));

    std::fs::write(&path, "only output\n").unwrap();
    assert_eq!(read_output_trailer(&path), OutputTrailer::None);
    std::fs::write(&path, "out\n[killed]\n").unwrap();
    assert_eq!(read_output_trailer(&path), OutputTrailer::Killed);
    std::fs::write(&path, "").unwrap();
    assert_eq!(read_output_trailer(&path), OutputTrailer::None);
    assert_eq!(
        read_output_trailer(&dir.path().join("missing")),
        OutputTrailer::Unreadable
    );
}

/// Field report on epic #439 (Claude Code 2.1.287): a 30-minute Monitor
/// delivers each event as enqueue + dequeue + an `origin.kind:
/// "task-notification"` user row (no `<status>`) + a text-only `end_turn`.
/// None of that may end the Monitor; only its deadline does.
#[test]
fn monitor_event_rows_keep_the_monitor_live_until_its_deadline() {
    let fixture = include_str!("fixtures/monitor_events.jsonl");
    let mut s = BackgroundTasks::default();
    let mut events = 0;
    let mut started = 0;
    let mut live_after_event = Vec::new();
    for line in fixture.lines() {
        let row: Value = serde_json::from_str(line).unwrap();
        for t in s.observe(&row) {
            match t {
                BackgroundTransition::Started(_) => started += 1,
                BackgroundTransition::MonitorEvent { task_id, line } => {
                    assert_eq!(task_id, "b0000004a");
                    events += 1;
                    assert_eq!(line, format!("line {}", events - 1));
                }
                other @ BackgroundTransition::Ended { .. } => panic!("ended early: {other:?}"),
            }
        }
        if row["type"] == "assistant" && row["message"]["stop_reason"] == "end_turn" {
            live_after_event.push(s.outstanding(TS_MS + 60_000).len());
        }
    }
    assert_eq!((started, events), (1, 4));
    assert_eq!(live_after_event, vec![1; 4]);

    // Alive just inside the deadline, listed no longer past it, and the
    // sweep ends it as expired once, after the grace.
    let deadline = TS_MS + 1_800_000;
    assert_eq!(s.outstanding(deadline).len(), 1);
    assert_eq!(
        s.expire_overdue(deadline + 999, 1000),
        Vec::<BackgroundTransition>::new()
    );
    let out = s.expire_overdue(deadline + 1000, 1000);
    let [BackgroundTransition::Ended { outcome, .. }] = out.as_slice() else {
        panic!("{out:?}")
    };
    assert_eq!(outcome, &Outcome::new(OutcomeStatus::Expired));
}
