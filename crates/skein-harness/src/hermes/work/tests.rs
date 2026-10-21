use std::collections::HashSet;

use rusqlite::{Connection, params};

use super::*;

const MAIN: &str = "20261021_140039_19c310";
const OTHER: &str = "20261021_150000_aaaaaa";

fn db() -> Connection {
    let c = Connection::open_in_memory().unwrap();
    c.execute_batch(
        "CREATE TABLE sessions (id TEXT PRIMARY KEY, source TEXT NOT NULL, \
           parent_session_id TEXT, started_at REAL NOT NULL, ended_at REAL, title TEXT);
         CREATE TABLE async_delegations (delegation_id TEXT PRIMARY KEY, \
           origin_session TEXT NOT NULL, parent_session_id TEXT, state TEXT NOT NULL, \
           dispatched_at REAL NOT NULL, owner_pid INTEGER, task_json TEXT);",
    )
    .unwrap();
    c
}

fn deleg(c: &Connection, id: &str, parent: &str, state: &str, at: f64, pid: i64) {
    c.execute(
        "INSERT INTO async_delegations VALUES (?1, ?2, ?2, ?3, ?4, ?5, ?6)",
        params![
            id,
            parent,
            state,
            at,
            pid,
            r#"{"goal": "ping it", "goals": ["ping it"]}"#
        ],
    )
    .unwrap();
}

fn child(c: &Connection, id: &str, parent: &str, at: f64, ended: Option<f64>) {
    c.execute(
        "INSERT INTO sessions VALUES (?1, 'tui', ?2, ?3, ?4, 'Subagent: ping it')",
        params![id, parent, at, ended],
    )
    .unwrap();
}

fn alive(pid: u32) -> bool {
    pid == 100
}

fn ids(c: &Connection, session: &str) -> Vec<String> {
    running_delegations(c, session, alive)
        .unwrap()
        .into_iter()
        .map(|d| d.child_session_id)
        .collect()
}

#[test]
fn running_with_live_owner_lists_child() {
    let c = db();
    deleg(&c, "deleg_1", MAIN, "running", 1000.0, 100);
    child(&c, "c1", MAIN, 1000.1, None);
    let got = running_delegations(&c, MAIN, alive).unwrap();
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].child_session_id, "c1");
    assert_eq!(got[0].description.as_deref(), Some("ping it"));
}

#[test]
fn completed_delegation_is_none() {
    let c = db();
    deleg(&c, "deleg_1", MAIN, "completed", 1000.0, 100);
    child(&c, "c1", MAIN, 1000.1, None);
    assert_eq!(ids(&c, MAIN), Vec::<String>::new());
}

#[test]
fn orphan_with_dead_owner_is_none() {
    let c = db();
    deleg(&c, "deleg_1", MAIN, "running", 1000.0, 999);
    child(&c, "c1", MAIN, 1000.1, None);
    assert_eq!(ids(&c, MAIN), Vec::<String>::new());
}

#[test]
fn ended_child_is_excluded() {
    let c = db();
    deleg(&c, "deleg_1", MAIN, "running", 1000.0, 100);
    child(&c, "done", MAIN, 1000.1, Some(1050.0));
    child(&c, "live", MAIN, 1000.2, None);
    assert_eq!(ids(&c, MAIN), ["live"]);
}

#[test]
fn other_sessions_rows_are_excluded() {
    let c = db();
    deleg(&c, "deleg_1", OTHER, "running", 1000.0, 100);
    child(&c, "c1", OTHER, 1000.1, None);
    assert_eq!(ids(&c, MAIN), Vec::<String>::new());
    assert_eq!(ids(&c, OTHER), ["c1"]);
}

#[test]
fn crashed_older_child_and_compression_child_are_excluded() {
    let c = db();
    deleg(&c, "deleg_old", MAIN, "unknown", 500.0, 999);
    child(&c, "stale", MAIN, 500.1, None);
    deleg(&c, "deleg_new", MAIN, "running", 1000.0, 100);
    c.execute(
        "INSERT INTO sessions VALUES ('compressed', 'tui', ?1, 1000.5, NULL, 'My task')",
        params![MAIN],
    )
    .unwrap();
    child(&c, "live", MAIN, 1000.3, None);
    assert_eq!(ids(&c, MAIN), ["live"]);
}

#[test]
fn live_count_counts_running_rows_with_live_owner_only() {
    let c = db();
    deleg(&c, "d1", MAIN, "running", 1000.0, 100);
    deleg(&c, "d2", MAIN, "running", 1001.0, 999);
    deleg(&c, "d3", MAIN, "completed", 1002.0, 100);
    deleg(&c, "d4", OTHER, "running", 1003.0, 100);
    assert_eq!(live_delegation_count(&c, MAIN, alive).unwrap(), 1);
    assert_eq!(live_delegation_count(&c, OTHER, alive).unwrap(), 1);
    assert_eq!(live_delegation_count(&c, "nobody", alive).unwrap(), 0);
}

#[test]
fn live_count_missing_table_or_column_is_zero() {
    let c = Connection::open_in_memory().unwrap();
    assert_eq!(live_delegation_count(&c, MAIN, alive).unwrap(), 0);
    c.execute_batch("CREATE TABLE async_delegations (delegation_id TEXT);")
        .unwrap();
    assert_eq!(live_delegation_count(&c, MAIN, alive).unwrap(), 0);
}

#[test]
fn missing_tables_or_columns_are_empty() {
    let c = Connection::open_in_memory().unwrap();
    assert_eq!(ids(&c, MAIN), Vec::<String>::new());
    c.execute_batch("CREATE TABLE async_delegations (delegation_id TEXT);")
        .unwrap();
    assert_eq!(ids(&c, MAIN), Vec::<String>::new());
}

#[test]
fn several_live_delegations_use_child_titles() {
    let c = db();
    deleg(&c, "d1", MAIN, "running", 1000.0, 100);
    deleg(&c, "d2", MAIN, "running", 1001.0, 100);
    child(&c, "c1", MAIN, 1000.1, None);
    let got = running_delegations(&c, MAIN, alive).unwrap();
    assert_eq!(got[0].description.as_deref(), Some("ping it"));
}

const PROCS: &str = r#"[
  {"session_id": "proc_a", "command": "sleep 60", "pid": 4242, "cwd": "/x",
   "started_at": 1792584000.5, "task_id": "t", "parent_session_id": "20261021_140039_19c310",
   "notify_on_complete": true, "brand_new_field": {"a": 1}},
  {"session_id": "proc_b", "pid": 7, "parent_session_id": "20261021_150000_aaaaaa"},
  {"session_id": "proc_c"},
  "junk",
  {"pid": 9, "parent_session_id": "20261021_140039_19c310"}
]"#;

#[test]
fn processes_filtered_to_the_session_and_unknown_fields_ignored() {
    let got = processes_for_session(PROCS, MAIN);
    assert_eq!(
        got,
        [HermesProcess {
            id: "proc_a".into(),
            pid: Some(4242),
            command: Some("sleep 60".into()),
            started_at: Some(1_792_584_000.5),
        }]
    );
}

#[test]
fn processes_garbage_empty_and_bom() {
    for t in [
        "",
        "[]",
        "{\"a\":1}",
        "[{\"session_id\": \"proc_a\", \"par",
        "not json",
    ] {
        assert_eq!(processes_for_session(t, MAIN), Vec::new(), "{t:?}");
    }
    let bom = format!("\u{feff}{PROCS}");
    assert_eq!(processes_for_session(&bom, MAIN).len(), 1);
}

#[test]
fn read_processes_absent_file_is_empty_and_present_file_reads() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("processes.json");
    assert_eq!(read_processes(&path, MAIN), Vec::new());
    std::fs::write(&path, PROCS).unwrap();
    assert_eq!(read_processes(&path, MAIN).len(), 1);
}

fn messages_db() -> Connection {
    let c = Connection::open_in_memory().unwrap();
    c.execute_batch(
        "CREATE TABLE messages (id INTEGER PRIMARY KEY, session_id TEXT NOT NULL,            role TEXT NOT NULL, content TEXT, tool_name TEXT);",
    )
    .unwrap();
    c
}

fn msg(c: &Connection, session: &str, role: &str, content: Option<&str>) {
    c.execute(
        "INSERT INTO messages (session_id, role, content) VALUES (?1, ?2, ?3)",
        params![session, role, content],
    )
    .unwrap();
}

#[test]
fn notify_ids_follow_the_tool_result() {
    let c = messages_db();
    // Default notify: no key (recorded). Notify on: true (recorded).
    msg(
        &c,
        MAIN,
        "tool",
        Some(
            r#"{"output": "Background process started", "session_id": "proc_off", "pid": 1, "exit_code": 0, "error": null}"#,
        ),
    );
    msg(
        &c,
        MAIN,
        "tool",
        Some(
            r#"{"output": "Background process started", "session_id": "proc_on", "pid": 2, "exit_code": 0, "error": null, "notify_on_complete": true}"#,
        ),
    );
    // Delegated child: kept internally but reported false.
    msg(
        &c,
        MAIN,
        "tool",
        Some(r#"{"session_id": "proc_child", "notify_on_complete": false}"#),
    );
    // Another session, a non-tool row, and an assistant echo.
    msg(
        &c,
        OTHER,
        "tool",
        Some(r#"{"session_id": "proc_other", "notify_on_complete": true}"#),
    );
    msg(
        &c,
        MAIN,
        "assistant",
        Some(r#"{"session_id": "proc_say", "notify_on_complete": true}"#),
    );
    let got = notify_process_ids(&c, MAIN).unwrap();
    assert_eq!(got, HashSet::from(["proc_on".to_string()]));
}

#[test]
fn notify_ids_skip_malformed_rows() {
    let c = messages_db();
    msg(&c, MAIN, "tool", Some("not json notify_on_complete"));
    msg(&c, MAIN, "tool", Some(r#"["notify_on_complete"]"#));
    msg(&c, MAIN, "tool", Some(r#"{"notify_on_complete": true}"#));
    msg(
        &c,
        MAIN,
        "tool",
        Some(r#"{"session_id": 5, "notify_on_complete": true}"#),
    );
    msg(
        &c,
        MAIN,
        "tool",
        Some(r#"{"session_id": "p", "notify_on_complete": "true"}"#),
    );
    msg(&c, MAIN, "tool", None);
    assert!(notify_process_ids(&c, MAIN).unwrap().is_empty());
}

#[test]
fn notify_ids_after_reads_only_new_rows() {
    let c = messages_db();
    msg(
        &c,
        MAIN,
        "tool",
        Some(r#"{"session_id": "p1", "notify_on_complete": true}"#),
    );
    msg(&c, MAIN, "tool", Some("plain output"));
    let (ids, cur) = notify_process_ids_after(&c, MAIN, 0).unwrap();
    assert_eq!(ids, HashSet::from(["p1".to_string()]));
    assert_eq!(cur, Some(2));
    // Nothing new: empty, no cursor.
    assert_eq!(
        notify_process_ids_after(&c, MAIN, 2).unwrap(),
        (HashSet::new(), None)
    );
    // A non-matching row still advances the cursor.
    msg(&c, MAIN, "tool", Some("more output"));
    assert_eq!(
        notify_process_ids_after(&c, MAIN, 2).unwrap(),
        (HashSet::new(), Some(3))
    );
    msg(
        &c,
        MAIN,
        "tool",
        Some(r#"{"session_id": "p2", "notify_on_complete": true}"#),
    );
    let (ids, cur) = notify_process_ids_after(&c, MAIN, 3).unwrap();
    assert_eq!(ids, HashSet::from(["p2".to_string()]));
    assert_eq!(cur, Some(4));
}

#[test]
fn notify_ids_missing_table_or_column_is_empty() {
    let c = Connection::open_in_memory().unwrap();
    assert!(notify_process_ids(&c, MAIN).unwrap().is_empty());
    c.execute_batch("CREATE TABLE messages (id INTEGER, session_id TEXT);")
        .unwrap();
    assert!(notify_process_ids(&c, MAIN).unwrap().is_empty());
}
