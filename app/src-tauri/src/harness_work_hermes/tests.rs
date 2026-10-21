use rusqlite::{Connection, params};
use serde_json::json;

use super::*;

fn work(subs: &[(&str, &str)], tasks: &[(&str, &str)]) -> Work {
    Work {
        subagents: subs
            .iter()
            .map(|(k, v)| ((*k).into(), Some((*v).into())))
            .collect(),
        tasks: tasks
            .iter()
            .map(|(k, v)| ((*k).into(), Some((*v).into())))
            .collect(),
        live_delegations: 0,
    }
}

#[test]
fn idle_edge_only_on_one_or_more_to_zero() {
    assert!(idle_edge(Some(1), 0));
    assert!(idle_edge(Some(3), 0));
    assert!(!idle_edge(None, 0), "first pass never fires");
    assert!(!idle_edge(Some(0), 0), "0 -> 0 never repeats");
    assert!(!idle_edge(Some(2), 1));
    assert!(!idle_edge(Some(0), 1));
}

#[test]
fn idle_is_emitted_between_ends_and_starts() {
    let a = work(&[("c1", "g")], &[]);
    let b = work(&[("c2", "h")], &[]);
    let evs = diff(&a, &b, false, true);
    assert_eq!(evs.len(), 3);
    assert!(matches!(evs[0], HermesWorkEvent::SubagentEnd { .. }));
    assert_eq!(evs[1], HermesWorkEvent::DelegationsIdle);
    assert!(matches!(evs[2], HermesWorkEvent::SubagentStart { .. }));
    assert!(!diff(&a, &b, false, false).contains(&HermesWorkEvent::DelegationsIdle));
}

#[test]
fn first_pass_marks_starts_initial() {
    let evs = diff(
        &Work::default(),
        &work(&[("c1", "goal")], &[("p1", "sleep")]),
        true,
        false,
    );
    assert_eq!(
        evs,
        vec![
            HermesWorkEvent::SubagentStart {
                agent_id: "c1".into(),
                description: Some("goal".into()),
                initial: true
            },
            HermesWorkEvent::BackgroundStart {
                task_id: "p1".into(),
                command: Some("sleep".into()),
                initial: true
            },
        ]
    );
}

#[test]
fn later_start_and_end_and_noop() {
    let a = work(&[("c1", "g")], &[("p1", "x")]);
    assert_eq!(diff(&a, &a, false, false).len(), 0);
    let b = work(&[("c2", "h")], &[]);
    assert_eq!(
        diff(&a, &b, false, false),
        vec![
            HermesWorkEvent::SubagentEnd {
                agent_id: "c1".into()
            },
            HermesWorkEvent::BackgroundEnd {
                task_id: "p1".into()
            },
            HermesWorkEvent::SubagentStart {
                agent_id: "c2".into(),
                description: Some("h".into()),
                initial: false
            },
        ]
    );
}

#[test]
fn serde_shape_is_camel_case_tagged() {
    let v = |e: &HermesWorkEvent| serde_json::to_value(e).unwrap();
    assert_eq!(
        v(&HermesWorkEvent::SubagentStart {
            agent_id: "x".into(),
            description: None,
            initial: true
        }),
        json!({"kind":"subagentStart","agentId":"x","description":null,"initial":true})
    );
    assert_eq!(
        v(&HermesWorkEvent::SubagentEnd {
            agent_id: "x".into()
        }),
        json!({"kind":"subagentEnd","agentId":"x"})
    );
    assert_eq!(
        v(&HermesWorkEvent::BackgroundStart {
            task_id: "p".into(),
            command: Some("c".into()),
            initial: false
        }),
        json!({"kind":"backgroundStart","taskId":"p","command":"c","initial":false})
    );
    assert_eq!(
        v(&HermesWorkEvent::BackgroundEnd {
            task_id: "p".into()
        }),
        json!({"kind":"backgroundEnd","taskId":"p"})
    );
    assert_eq!(
        v(&HermesWorkEvent::DelegationsIdle),
        json!({"kind":"delegationsIdle"})
    );
}

const SID: &str = "20261021_140039_19c310";

fn fixture(dir: &Path) -> PathBuf {
    let db = dir.join("state.db");
    let c = Connection::open(&db).unwrap();
    c.execute_batch(
        "CREATE TABLE sessions (id TEXT PRIMARY KEY, source TEXT NOT NULL, \
           parent_session_id TEXT, started_at REAL NOT NULL, ended_at REAL, title TEXT);
         CREATE TABLE async_delegations (delegation_id TEXT PRIMARY KEY, \
           origin_session TEXT NOT NULL, parent_session_id TEXT, state TEXT NOT NULL, \
           dispatched_at REAL NOT NULL, owner_pid INTEGER, task_json TEXT);
         CREATE TABLE messages (id INTEGER PRIMARY KEY, session_id TEXT NOT NULL,            role TEXT NOT NULL, content TEXT);",
    )
    .unwrap();
    for (id, notify) in [
        ("proc_a", "true"),
        ("proc_dead", "true"),
        ("proc_quiet", "false"),
    ] {
        c.execute(
            "INSERT INTO messages (session_id, role, content) VALUES (?1, 'tool', ?2)",
            params![
                SID,
                format!(r#"{{"session_id":"{id}","notify_on_complete":{notify}}}"#)
            ],
        )
        .unwrap();
    }
    let deleg = |id: &str, state: &str, pid: i64| {
        c.execute(
            "INSERT INTO async_delegations VALUES (?1, ?2, ?2, ?3, 1000.0, ?4, ?5)",
            params![id, SID, state, pid, r#"{"goal":"ping it"}"#],
        )
        .unwrap();
    };
    deleg("deleg_run", "running", 100);
    deleg("deleg_done", "completed", 100);
    deleg("deleg_dead", "running", 999);
    c.execute(
        "INSERT INTO sessions VALUES ('child1', 'tui', ?1, 1000.1, NULL, 'Subagent: ping it')",
        params![SID],
    )
    .unwrap();
    std::fs::write(
        dir.join("processes.json"),
        json!([
            {"session_id":"proc_a","pid":100,"command":"sleep 99","parent_session_id":SID},
            {"session_id":"proc_quiet","pid":100,"command":"dev server","parent_session_id":SID,"notify_on_complete":false},
            {"session_id":"proc_dead","pid":999,"command":"gone","parent_session_id":SID},
            {"session_id":"proc_other","pid":100,"command":"x","parent_session_id":"other"},
        ])
        .to_string(),
    )
    .unwrap();
    db
}

#[test]
fn attach_snapshot_from_a_profile_dir() {
    let dir = tempfile::tempdir().unwrap();
    let db = fixture(dir.path());
    let mut notify = NotifyState::default();
    let got = read_work(&db, SID, |pid| pid == 100, &mut notify).unwrap();
    assert_eq!(
        got,
        Work {
            subagents: [("child1".to_string(), Some("ping it".to_string()))].into(),
            tasks: [("proc_a".to_string(), Some("sleep 99".to_string()))].into(),
            live_delegations: 1,
        }
    );
}

#[test]
fn missing_db_and_files_read_as_no_work() {
    let dir = tempfile::tempdir().unwrap();
    let got = read_work(
        &dir.path().join("state.db"),
        SID,
        |_| true,
        &mut NotifyState::default(),
    )
    .unwrap();
    assert_eq!(got, Work::default());
}

#[test]
fn messages_are_not_scanned_without_processes_and_catch_up_later() {
    let dir = tempfile::tempdir().unwrap();
    let db = fixture(dir.path());
    let procs = dir.path().join("processes.json");
    let saved = std::fs::read_to_string(&procs).unwrap();
    std::fs::write(&procs, "[]").unwrap();
    let mut notify = NotifyState::default();
    read_work(&db, SID, |pid| pid == 100, &mut notify).unwrap();
    assert_eq!(notify.cursor, 0);
    assert!(notify.ids.is_empty());
    // A process appears: the first needed query catches up from the start.
    std::fs::write(&procs, saved).unwrap();
    let got = read_work(&db, SID, |pid| pid == 100, &mut notify).unwrap();
    assert_eq!(notify.cursor, 3);
    assert!(got.tasks.contains_key("proc_a"));
    // Incremental: nothing new leaves the cursor and ids alone.
    read_work(&db, SID, |pid| pid == 100, &mut notify).unwrap();
    assert_eq!(notify.cursor, 3);
    assert!(notify.ids.contains("proc_a"));
}

#[test]
fn pid_exists_knows_this_process_and_not_a_bogus_one() {
    assert!(pid_exists(std::process::id()));
    assert!(!pid_exists(u32::MAX - 1));
}
