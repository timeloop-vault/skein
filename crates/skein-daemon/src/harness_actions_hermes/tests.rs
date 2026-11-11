use super::*;
use rusqlite::{Connection, params};

const SCHEMA: &str =
    include_str!("../../../../crates/skein-harness/src/hermes/fixtures/statedb-schema.sql");
const SID: &str = "sess-1";
const HID: &str = "h-1";

struct Env {
    dir: tempfile::TempDir,
    db: Arc<Database>,
}

impl Env {
    fn new() -> Self {
        let dir = tempfile::TempDir::new().unwrap();
        let db = Arc::new(Database::open(&dir.path().join("skein.db")).unwrap());
        let conn = Connection::open(dir.path().join("state.db")).unwrap();
        conn.execute_batch(SCHEMA).unwrap();
        conn.execute(
            "INSERT INTO sessions (id, source, started_at, model) VALUES (?1, 'tui', 1.0, 'm-1')",
            params![SID],
        )
        .unwrap();
        Self { dir, db }
    }

    fn state(&self) -> Connection {
        Connection::open(self.dir.path().join("state.db")).unwrap()
    }

    fn tail(&self) -> Tail {
        self.tail_resumed(false)
    }

    fn tail_resumed(&self, resumed: bool) -> Tail {
        Tail::new(
            Arc::clone(&self.db),
            None,
            HID.into(),
            "r-1".into(),
            SID.into(),
            self.dir.path().join("state.db"),
            resumed,
        )
    }

    fn rows(&self) -> Vec<(i64, String, Value)> {
        self.db
            .recent_harness_actions_by_harness(HID, -1, 100)
            .unwrap()
            .into_iter()
            .rev()
            .map(|r| (r.id, r.kind, serde_json::from_str(&r.payload).unwrap()))
            .collect()
    }

    fn kinds(&self) -> Vec<String> {
        self.rows().into_iter().map(|r| r.1).collect()
    }
}

fn msg(conn: &Connection, role: &str, content: &str, calls: Option<&str>, call_id: Option<&str>) {
    conn.execute(
        "INSERT INTO messages (session_id, role, content, tool_calls, tool_call_id, timestamp) \
         VALUES (?1, ?2, ?3, ?4, ?5, 100.0)",
        params![SID, role, content, calls, call_id],
    )
    .unwrap();
}

/// A user prompt, then one terminal call with its result.
fn first_turn(conn: &Connection) {
    msg(conn, "user", "hello", None, None);
    let calls =
        r#"[{"id":"c1","function":{"name":"terminal","arguments":"{\"command\":\"ls\"}"}}]"#;
    msg(conn, "assistant", "", Some(calls), None);
    msg(
        conn,
        "tool",
        r#"{"output":"a","exit_code":0}"#,
        None,
        Some("c1"),
    );
}

fn usage(conn: &Connection, input: i64, output: i64, usd: f64) {
    conn.execute(
        "UPDATE sessions SET input_tokens = ?1, output_tokens = ?2, estimated_cost_usd = ?3 WHERE id = ?4",
        params![input, output, usd, SID],
    )
    .unwrap();
}

#[test]
fn rows_are_recorded_once_across_attaches() {
    let env = Env::new();
    first_turn(&env.state());
    env.tail().poll();
    assert_eq!(env.kinds(), ["user_prompt", "tool_call"]);

    // A second attach (restart) re-reads nothing.
    env.tail().poll();
    assert_eq!(env.kinds().len(), 2);

    msg(&env.state(), "user", "again", None, None);
    env.tail().poll();
    assert_eq!(env.kinds(), ["user_prompt", "tool_call", "user_prompt"]);
}

#[test]
fn cursor_is_recovered_from_recorded_message_ids() {
    let env = Env::new();
    assert_eq!(env.tail().inner.lock().cursor, 0);
    first_turn(&env.state());
    let t = env.tail();
    t.poll();
    // The tool_call row is keyed by its assistant row (id 2).
    let max = env.db.max_recorded_message_id(HID, SID, SOURCE).unwrap();
    assert_eq!(max, Some(2));
    assert_eq!(env.tail().inner.lock().cursor, 2);
    // Another session or source recorded nothing.
    assert_eq!(
        env.db
            .max_recorded_message_id(HID, "other", SOURCE)
            .unwrap(),
        None
    );
    assert_eq!(env.db.max_recorded_message_id(HID, SID, "x").unwrap(), None);
}

#[test]
fn payloads_carry_message_and_session_ids() {
    let env = Env::new();
    first_turn(&env.state());
    env.tail().poll();
    for (_, _, p) in env.rows() {
        assert!(p["message_id"].as_i64().is_some());
        assert_eq!(p["session_id"], SID);
    }
}

#[test]
fn a_held_call_is_recorded_when_its_result_lands() {
    let env = Env::new();
    let calls = r#"[{"id":"c1","function":{"name":"terminal","arguments":"{}"}}]"#;
    msg(&env.state(), "assistant", "", Some(calls), None);
    let t = env.tail();
    t.poll();
    assert_eq!(env.kinds().len(), 0);
    msg(
        &env.state(),
        "tool",
        r#"{"output":"","exit_code":0}"#,
        None,
        Some("c1"),
    );
    t.inner.lock().last_stat = None;
    t.poll();
    assert_eq!(env.kinds(), ["tool_call"]);
}

#[test]
fn an_unchanged_state_db_is_not_requeried() {
    let env = Env::new();
    let t = env.tail();
    t.poll();
    assert!(t.inner.lock().last_stat.is_some());
    assert_eq!(t.inner.lock().skips, 0);
    t.poll();
    assert_eq!(t.inner.lock().skips, 1);
}

#[test]
fn a_missing_state_db_is_quiet() {
    let dir = tempfile::TempDir::new().unwrap();
    let db = Arc::new(Database::open(&dir.path().join("skein.db")).unwrap());
    let t = Tail::new(
        db,
        None,
        HID.into(),
        "r".into(),
        SID.into(),
        dir.path().join("nope.db"),
        false,
    );
    t.poll();
    assert!(!t.turn_end().unwrap());
    assert!(!t.inner.lock().warned);
}

#[test]
fn turn_cost_is_the_delta_and_survives_a_restart() {
    let env = Env::new();
    first_turn(&env.state());
    usage(&env.state(), 100, 20, 0.5);
    let t = env.tail();
    assert!(t.turn_end().unwrap());
    // Tool rows precede the cost row.
    assert_eq!(env.kinds(), ["user_prompt", "tool_call", "turn_cost"]);
    let (_, _, p) = env.rows().pop().unwrap();
    assert_eq!(p["tokens"]["total"], 120);
    assert!((p["cost"].as_f64().unwrap() - 0.5).abs() < 1e-9);

    // Nothing grew: no row.
    assert!(!t.turn_end().unwrap());
    assert_eq!(env.rows().iter().filter(|r| r.1 == "turn_cost").count(), 1);

    // Second turn, in a NEW tail (restart): only the growth is billed.
    usage(&env.state(), 150, 50, 0.75);
    let t2 = env.tail();
    assert!(t2.turn_end().unwrap());
    let (_, _, p2) = env.rows().pop().unwrap();
    assert_eq!(p2["tokens"]["input"], 50);
    assert_eq!(p2["tokens"]["output"], 30);
    assert_eq!(p2["tokens"]["total"], 80);
    assert!((p2["cost"].as_f64().unwrap() - 0.25).abs() < 1e-9);
    assert_eq!(p2["session_tokens"]["input"], 150);
    assert_ne!(p, p2);
    assert!(!env.tail().turn_end().unwrap());
}

#[test]
fn turn_cost_payload_matches_what_the_feed_reads() {
    let env = Env::new();
    usage(&env.state(), 10, 5, 0.01);
    env.tail().turn_end().unwrap();
    let (_, kind, p) = env.rows().pop().unwrap();
    assert_eq!(kind, "turn_cost");
    let o = p.as_object().unwrap();
    // feedFlatten: opencode step iff "tokens" or "reason" is present, and
    // NOT the Claude shape.
    assert!(o.contains_key("tokens") && o.contains_key("reason"));
    assert!(!o.contains_key("usage") && !o.contains_key("stop_reason"));
    // stepFromOpencode: terminal iff reason != "tool-calls".
    assert_ne!(o["reason"], "tool-calls");
    assert!(o["cost"].is_number());
    for k in ["total", "input", "output", "reasoning"] {
        assert!(o["tokens"][k].is_number(), "tokens.{k}");
    }
    assert!(o["tokens"]["cache"]["read"].is_number());
    assert!(o["tokens"]["cache"]["write"].is_number());
    assert_eq!(o["model"], "m-1");
    assert_eq!(o["session_id"], SID);
}

#[test]
fn a_resumed_session_starts_from_its_current_totals() {
    let env = Env::new();
    usage(&env.state(), 100, 20, 0.5);
    let t = env.tail_resumed(true);
    assert!(!t.turn_end().unwrap(), "prior spend is not this turn's");
    usage(&env.state(), 110, 25, 0.6);
    assert!(t.turn_end().unwrap());
    let (_, _, p) = env.rows().pop().unwrap();
    assert_eq!(p["tokens"]["total"], 15);
    // Restart before any row was recorded: re-snapshot.
    let env2 = Env::new();
    usage(&env2.state(), 100, 20, 0.5);
    drop(env2.tail_resumed(true));
    assert!(!env2.tail_resumed(true).turn_end().unwrap());
}

#[test]
fn a_fresh_session_starts_from_zero() {
    let env = Env::new();
    usage(&env.state(), 100, 20, 0.5);
    assert!(env.tail_resumed(false).turn_end().unwrap());
    let (_, _, p) = env.rows().pop().unwrap();
    assert_eq!(p["tokens"]["total"], 120);
}

fn save_room_with_cwd(env: &Env) {
    let room = crate::db::Room {
        id: "r-1".into(),
        name: "room".into(),
        task: String::new(),
        status: "idle".into(),
        badge: 0,
        harnesses: Vec::new(),
        active_harness_id: String::new(),
        cwd: Some(env.dir.path().to_string_lossy().into_owned()),
        branch: None,
        repo: None,
        archived: None,
        repo_root: None,
        attention: None,
        created_by: None,
        closed_by: None,
        todos: None,
        retired: None,
        repo_identity: None,
    };
    env.db.load_all().unwrap();
    env.db.save_all(&[room]).unwrap();
}

/// An assistant `write_file` call with its landed result.
fn write_file_turn(conn: &Connection, id: &str, path: &str) {
    let calls = format!(
        r#"[{{"id":"{id}","function":{{"name":"write_file","arguments":"{{\"path\":\"{path}\",\"content\":\"x\"}}"}}}}]"#
    );
    msg(conn, "assistant", "", Some(&calls), None);
    msg(conn, "tool", r#"{"bytes_written":1}"#, None, Some(id));
}

#[test]
fn replayed_patch_rows_capture_no_baseline_but_live_ones_do() {
    let env = Env::new();
    save_room_with_cwd(&env);
    write_file_turn(&env.state(), "w1", "old.txt");
    let t = env.tail();
    t.poll(); // Attach replay: history, recorded but not live.
    assert_eq!(env.kinds(), ["patch"]);
    assert!(env.db.review_baseline("r-1", "old.txt").unwrap().is_none());

    write_file_turn(&env.state(), "w2", "new.txt");
    t.poll(); // Discovered after attach: live.
    assert_eq!(env.kinds(), ["patch", "patch"]);
    assert!(env.db.review_baseline("r-1", "new.txt").unwrap().is_some());
    assert!(env.db.review_baseline("r-1", "old.txt").unwrap().is_none());
}

#[test]
fn a_stopped_tail_records_no_turn_cost() {
    let env = Env::new();
    first_turn(&env.state());
    usage(&env.state(), 100, 20, 0.5);
    let t = env.tail();
    t.stopped.store(true, Ordering::Relaxed);
    assert!(!t.turn_end().unwrap());
    assert_eq!(env.kinds().len(), 0);
}

#[test]
fn the_cursor_only_passes_fully_recorded_messages() {
    let env = Env::new();
    first_turn(&env.state());
    let t = env.tail();
    // Make recording the second row (the tool call) fail.
    Connection::open(env.dir.path().join("skein.db"))
        .unwrap()
        .execute_batch(
            "CREATE TRIGGER fail_tool BEFORE INSERT ON harness_actions              WHEN NEW.kind = 'tool_call' BEGIN SELECT RAISE(ABORT, 'no'); END;",
        )
        .unwrap();
    t.poll();
    let cursor = t.inner.lock().cursor;
    assert_eq!(env.kinds(), ["user_prompt"]);
    assert!(cursor < 2, "cursor {cursor} passed an unrecorded message");
    assert_eq!(cursor, 1, "the recorded prompt is passed");
}
