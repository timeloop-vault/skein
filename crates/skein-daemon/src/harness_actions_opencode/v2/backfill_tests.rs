use std::collections::HashSet;

use rusqlite::Connection;
use serde_json::Value;
use skein_harness::opencode::v2::{drain_sse_data, parse_action_event};

use super::backfill::{backfill_from_conn, backfill_from_path};
use crate::db::Database;
use crate::db::test_support::fresh_db;
use crate::harness_actions_opencode::v2::V2ActionTracker;

const SCHEMA: &str =
    include_str!("../../../../skein-harness/src/opencode/v2/fixtures/store/schema.sql");
const MSG_C: &str =
    include_str!("../../../../skein-harness/src/opencode/v2/fixtures/store/messages_C.jsonl");
const SSE_C: &str = include_str!("../../../../skein-harness/src/opencode/v2/fixtures/store-C.sse");
const SID: &str = "ses_e11430847ffewN4itXfZB0L5GQ";
const H: &str = "h1";
const R: &str = "r1";

/// The v2 tables of a real 2.3.2 database (foreign keys are switched off,
/// so `session_v2` need not exist) with `messages_C` loaded.
fn opencode_db() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch("PRAGMA foreign_keys = OFF;").unwrap();
    conn.execute_batch(SCHEMA).unwrap();
    for line in MSG_C.lines() {
        let v: Value = serde_json::from_str(line).unwrap();
        conn.execute(
            "INSERT INTO session_message \
             (id, session_id, type, seq, time_created, time_updated, data) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            rusqlite::params![
                v["id"].as_str().unwrap(),
                v["session_id"].as_str().unwrap(),
                v["type"].as_str().unwrap(),
                v["seq"].as_i64().unwrap(),
                v["time_created"].as_i64().unwrap(),
                v["time_updated"].as_i64().unwrap(),
                v["data"].to_string(),
            ],
        )
        .unwrap();
    }
    conn
}

fn all_rows(db: &Database) -> Vec<crate::db::HarnessAction> {
    db.recent_harness_actions_by_harness(H, -1, 10_000).unwrap()
}

fn sources(db: &Database) -> Vec<String> {
    all_rows(db).into_iter().filter_map(|r| r.source).collect()
}

#[test]
fn backfill_inserts_rows_once() {
    let (_d, db) = fresh_db();
    let conn = opencode_db();
    let n = backfill_from_conn(&conn, SID, H, R, &db);
    assert!(n > 0);
    let rows = all_rows(&db);
    let kinds: HashSet<&str> = rows.iter().map(|r| r.kind.as_str()).collect();
    for k in ["user_prompt", "turn_cost"] {
        assert!(kinds.contains(k), "missing {k} in {kinds:?}");
    }
    assert!(kinds.len() >= 3, "{kinds:?}");
    assert!(
        rows.iter()
            .all(|r| r.harness_kind.as_deref() == Some("opencode"))
    );
    // A re-attach inserts nothing.
    assert_eq!(backfill_from_conn(&conn, SID, H, R, &db), 0);
    assert_eq!(all_rows(&db).len(), n);
}

#[test]
fn backfill_adds_only_what_the_live_path_missed() {
    let (_d, db) = fresh_db();
    let conn = opencode_db();
    // The live path records the first half of the session's rows.
    let mut buf = SSE_C.as_bytes().to_vec();
    let mut tracker = V2ActionTracker::default();
    let live: Vec<_> = drain_sse_data(&mut buf)
        .iter()
        .filter_map(|p| parse_action_event(p))
        .flat_map(|a| tracker.observe(&a))
        .collect();
    assert!(live.len() > 2);
    for r in &live[..live.len() / 2] {
        db.record_harness_action(
            H,
            R,
            r.timestamp_ms,
            r.kind,
            &r.payload,
            r.source.as_deref(),
            Some("opencode"),
        )
        .unwrap();
    }
    let before = all_rows(&db).len();
    let added = backfill_from_conn(&conn, SID, H, R, &db);
    assert!(added > 0);

    let src = sources(&db);
    let distinct: HashSet<&String> = src.iter().collect();
    assert_eq!(src.len(), distinct.len(), "duplicate source: {src:?}");
    assert_eq!(all_rows(&db).len(), before + added);
    for r in &live {
        assert!(src.contains(r.source.as_ref().unwrap()));
    }
    assert_eq!(backfill_from_conn(&conn, SID, H, R, &db), 0);
}

#[test]
fn v1_only_database_yields_nothing() {
    let (_d, db) = fresh_db();
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch("CREATE TABLE message (id TEXT);")
        .unwrap();
    assert_eq!(backfill_from_conn(&conn, SID, H, R, &db), 0);
    assert!(all_rows(&db).is_empty());
}

#[test]
fn other_sessions_and_other_harnesses_are_not_mixed_in() {
    let (_d, db) = fresh_db();
    let conn = opencode_db();
    assert_eq!(backfill_from_conn(&conn, "ses_other", H, R, &db), 0);
    // Another harness's identical sources do not suppress this one's rows.
    let n = backfill_from_conn(&conn, SID, "h2", R, &db);
    assert!(n > 0);
    assert_eq!(backfill_from_conn(&conn, SID, H, R, &db), n);
}

#[test]
fn missing_database_file_yields_nothing() {
    let dir = tempfile::TempDir::new().unwrap();
    let (_d, db) = fresh_db();
    let missing = dir.path().join("opencode.db");
    assert_eq!(backfill_from_path(&missing, SID, H, R, &db), 0);
    assert!(all_rows(&db).is_empty());
}
