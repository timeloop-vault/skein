use super::*;

// Recorded on Hermes v0.21.7+184, trimmed (see the fixtures' origin in
// the issue #625 notes). Paths and system prompts are scrubbed.
const SCHEMA: &str = include_str!("../fixtures/statedb-schema.sql");
const ROWS: &str = include_str!("../fixtures/statedb-rows.json");

const A_PLAIN: &str = "20261021_112149_a33ff7";
const B_APPROVED: &str = "20261021_112625_cf3d58";
const B_DENIED: &str = "20261021_112815_de7abe";
const C_CTRL_C: &str = "20261021_112905_ab91db";
const D_PARENT: &str = "20261021_112948_9ebf73";
const D_CHILD: &str = "20261021_113003_775a22";
const E_WRITE_PATCH: &str = "20261021_113020_5c8e84";

fn sql_value(v: &Value) -> rusqlite::types::Value {
    use rusqlite::types::Value as S;
    match v {
        Value::Null => S::Null,
        Value::Bool(b) => S::Integer(i64::from(*b)),
        Value::Number(n) => n
            .as_i64()
            .map_or_else(|| S::Real(n.as_f64().unwrap()), S::Integer),
        Value::String(s) => S::Text(s.clone()),
        other => S::Text(other.to_string()),
    }
}

fn insert(conn: &Connection, table: &str, rows: &Value) {
    for row in rows.as_array().unwrap() {
        let obj = row.as_object().unwrap();
        let cols: Vec<&str> = obj.keys().map(String::as_str).collect();
        let marks: Vec<String> = (1..=cols.len()).map(|i| format!("?{i}")).collect();
        let sql = format!(
            "INSERT INTO {table} ({}) VALUES ({})",
            cols.join(", "),
            marks.join(", ")
        );
        let vals: Vec<_> = obj.values().map(sql_value).collect();
        conn.execute(&sql, rusqlite::params_from_iter(vals))
            .unwrap();
    }
}

fn recorded() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(SCHEMA).unwrap();
    let data: Value = serde_json::from_str(ROWS).unwrap();
    insert(&conn, "sessions", &data["sessions"]);
    insert(&conn, "messages", &data["messages"]);
    insert(&conn, "session_model_usage", &data["session_model_usage"]);
    conn
}

fn all(conn: &Connection, id: &str) -> (Vec<HermesActionRow>, i64) {
    actions_after(conn, id, 0).unwrap()
}

fn kinds(rows: &[HermesActionRow]) -> Vec<&str> {
    rows.iter().map(|r| r.kind.as_str()).collect()
}

fn add_msg(conn: &Connection, sql_tail: &str) {
    conn.execute(
        &format!(
            "INSERT INTO messages (session_id, role, content, tool_call_id, tool_calls, timestamp, active) VALUES {sql_tail}"
        ),
        [],
    )
    .unwrap();
}

#[test]
fn plain_turn_is_just_the_prompt() {
    let conn = recorded();
    let (rows, cursor) = all(&conn, A_PLAIN);
    assert_eq!(kinds(&rows), ["user_prompt"]);
    assert_eq!(rows[0].payload["prompt"], "say hi in two words");
    // The assistant's text is consumed (cursor passes it), not emitted.
    assert_eq!(cursor, 2);
    // seconds -> ms
    assert_eq!(rows[0].timestamp_ms, 1_792_574_509_276);
}

#[test]
fn approved_terminal_call_pairs_with_its_result() {
    let conn = recorded();
    let (rows, _) = all(&conn, B_APPROVED);
    assert_eq!(kinds(&rows), ["user_prompt", "tool_call"]);
    let p = &rows[1].payload;
    assert_eq!(p["tool"], "bash");
    assert_eq!(p["input"]["command"], "rm -rf ./victim1");
    assert_eq!(p["is_error"], false);
    assert_eq!(p["result"]["stdout"], "");
    assert_eq!(p["result"]["exit_code"], 0);
    assert_eq!(rows[1].message_id, 4);
    // started at the assistant row, ended at the tool row (5.1 s later,
    // most of it the approval prompt).
    assert_eq!(p["duration_ms"], 5_126);
    assert_eq!(rows[1].timestamp_ms, p["started_at_ms"].as_i64().unwrap());
}

#[test]
fn denied_terminal_call_is_an_error() {
    let conn = recorded();
    let (rows, _) = all(&conn, B_DENIED);
    assert_eq!(kinds(&rows), ["user_prompt", "tool_call"]);
    let p = &rows[1].payload;
    assert_eq!(p["tool"], "bash");
    assert_eq!(p["input"]["command"], "rm -rf ./victim3");
    assert_eq!(p["is_error"], true);
    assert!(
        p["error"]
            .as_str()
            .unwrap()
            .starts_with("BLOCKED: User denied")
    );
}

#[test]
fn interrupted_turn_with_no_tools_emits_only_the_prompt() {
    let conn = recorded();
    let (rows, cursor) = all(&conn, C_CTRL_C);
    assert_eq!(kinds(&rows), ["user_prompt"]);
    assert_eq!(cursor, 16);
}

#[test]
fn delegation_is_a_call_and_the_synthetic_result_prompt_is_not_a_prompt() {
    let conn = recorded();
    let (rows, _) = all(&conn, D_PARENT);
    assert_eq!(kinds(&rows), ["user_prompt", "tool_call"]);
    assert_eq!(rows[1].payload["tool"], "delegate_task");
    assert_eq!(rows[1].payload["result"]["status"], "dispatched");
    // The child session is its own session: its prompt is the goal.
    let (child, _) = all(&conn, D_CHILD);
    assert_eq!(kinds(&child), ["user_prompt"]);
    assert_eq!(
        child[0].payload["prompt"],
        "Reply with the single word PONG and nothing else"
    );
}

#[test]
fn write_file_and_patch_become_patch_rows_with_the_diff() {
    let conn = recorded();
    let (rows, _) = all(&conn, E_WRITE_PATCH);
    assert_eq!(kinds(&rows), ["user_prompt", "patch", "patch"]);

    let write = &rows[1].payload;
    assert_eq!(write["tool"], "write");
    assert_eq!(write["files"], json!(["hello.txt"]));
    assert_eq!(write["input"]["content"], "hi");
    assert!(
        write.get("patch_info").is_none(),
        "write_file returns no diff"
    );

    let patch = &rows[2].payload;
    assert_eq!(patch["tool"], "edit");
    assert_eq!(patch["files"], json!(["hello.txt"]));
    assert_eq!(patch["is_error"], false);
    let info = &patch["patch_info"];
    assert_eq!(info["file"], "hello.txt");
    assert!(info.get("additions").is_none());
    let diff = info["diff"].as_str().unwrap();
    assert!(diff.contains("@@ -1 +1 @@"), "{diff}");
    assert!(diff.ends_with("-hi+bye"), "{diff}");
}

#[test]
fn cursor_is_idempotent_and_resumable() {
    let conn = recorded();
    for id in [
        A_PLAIN,
        B_APPROVED,
        B_DENIED,
        C_CTRL_C,
        D_PARENT,
        D_CHILD,
        E_WRITE_PATCH,
    ] {
        let (first, cursor) = all(&conn, id);
        assert_ne!(first, vec![]);
        let (again, cursor2) = actions_after(&conn, id, cursor).unwrap();
        assert_eq!(again, vec![], "{id}");
        assert_eq!(cursor2, cursor);
    }
    // Resuming mid-session yields only the rest.
    let (rest, _) = actions_after(&conn, E_WRITE_PATCH, 27).unwrap();
    assert_eq!(kinds(&rest), ["patch"]);
    assert_eq!(rest[0].message_id, 28);
}

#[test]
fn a_call_still_awaiting_approval_is_held_then_emitted_once() {
    let conn = recorded();
    let sid = "20261021_999999_aaaaaa";
    conn.execute(
        "INSERT INTO sessions (id, source, started_at) VALUES (?1, 'cli', 1.0)",
        [sid],
    )
    .unwrap();
    add_msg(
        &conn,
        &format!("('{sid}', 'user', 'go', NULL, NULL, 10.0, 1)"),
    );
    let calls =
        r#"[{"id":"c1","function":{"name":"terminal","arguments":"{\"command\": \"ls\"}"}}]"#;
    add_msg(
        &conn,
        &format!("('{sid}', 'assistant', '', NULL, '{calls}', 11.0, 1)"),
    );
    let (rows, cursor) = all(&conn, sid);
    assert_eq!(kinds(&rows), ["user_prompt"]);
    // Held in front of the assistant row, not past it.
    let user_id = rows[0].message_id;
    assert_eq!(cursor, user_id);
    // Polling again while it waits changes nothing.
    let (rows2, cursor2) = actions_after(&conn, sid, cursor).unwrap();
    assert_eq!(rows2, vec![]);
    assert_eq!(cursor2, cursor);
    // The result lands: the one row appears, once.
    add_msg(
        &conn,
        &format!(
            "('{sid}', 'tool', '{{\"output\": \"x\", \"exit_code\": 0}}', 'c1', NULL, 12.0, 1)"
        ),
    );
    let (rows3, cursor3) = actions_after(&conn, sid, cursor).unwrap();
    assert_eq!(kinds(&rows3), ["tool_call"]);
    assert_eq!(rows3[0].payload["result"]["stdout"], "x");
    let (rows4, _) = actions_after(&conn, sid, cursor3).unwrap();
    assert_eq!(rows4, vec![]);
}

#[test]
fn an_orphaned_call_followed_by_more_turns_is_emitted_without_a_result() {
    let conn = recorded();
    let sid = "20261021_999999_bbbbbb";
    conn.execute(
        "INSERT INTO sessions (id, source, started_at) VALUES (?1, 'cli', 1.0)",
        [sid],
    )
    .unwrap();
    let calls =
        r#"[{"id":"c1","function":{"name":"terminal","arguments":"{\"command\": \"sleep 99\"}"}}]"#;
    add_msg(
        &conn,
        &format!("('{sid}', 'assistant', '', NULL, '{calls}', 11.0, 1)"),
    );
    add_msg(
        &conn,
        &format!("('{sid}', 'user', 'never mind', NULL, NULL, 20.0, 1)"),
    );
    let (rows, _) = all(&conn, sid);
    assert_eq!(kinds(&rows), ["tool_call", "user_prompt"]);
    assert!(rows[0].payload["result"].is_null());
    assert_eq!(rows[0].payload["is_error"], false);
    assert!(rows[0].payload.get("ended_at_ms").is_none());
}

#[test]
fn rewound_rows_are_skipped() {
    let conn = recorded();
    conn.execute("UPDATE messages SET active = 0 WHERE id = 1", [])
        .unwrap();
    let (rows, cursor) = all(&conn, A_PLAIN);
    assert_eq!(rows, vec![]);
    assert_eq!(cursor, 2);
}

#[test]
fn session_cost_reads_the_session_row_and_sums_side_tasks_apart() {
    let conn = recorded();
    let c = session_cost(&conn, A_PLAIN).unwrap().unwrap();
    assert_eq!(c.model.as_deref(), Some("anthropic/claude-haiku-4.5"));
    assert_eq!(c.estimated_cost_usd, Some(0.014_403_75));
    assert_eq!(c.cost_status.as_deref(), Some("estimated"));
    assert_eq!(
        (
            c.input_tokens,
            c.output_tokens,
            c.cache_read_tokens,
            c.cache_write_tokens
        ),
        (10, 76, 0, 11_211)
    );
    assert_eq!(c.reasoning_tokens, 67);
    assert_eq!(c.api_call_count, 1);
    // title_generation only lives in session_model_usage.
    assert!((c.auxiliary_cost_usd - 0.000_457).abs() < 1e-9);

    let b = session_cost(&conn, B_APPROVED).unwrap().unwrap();
    assert!((b.auxiliary_cost_usd - (0.000_501 + 0.000_388)).abs() < 1e-9);
}

#[test]
fn session_cost_without_spend_and_for_unknown_session() {
    let conn = recorded();
    let c = session_cost(&conn, C_CTRL_C).unwrap().unwrap();
    assert_eq!(c.estimated_cost_usd, None);
    assert_eq!(c.api_call_count, 0);
    assert!(session_cost(&conn, "nope").unwrap().is_none());
}

#[test]
fn session_cost_falls_back_to_main_task_usage_rows() {
    let conn = recorded();
    conn.execute(
        "UPDATE sessions SET estimated_cost_usd = NULL WHERE id = ?1",
        [A_PLAIN],
    )
    .unwrap();
    let c = session_cost(&conn, A_PLAIN).unwrap().unwrap();
    assert_eq!(c.estimated_cost_usd, Some(0.014_403_75));
}
