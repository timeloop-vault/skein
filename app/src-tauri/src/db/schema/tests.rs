use super::*;
use crate::db::harness_log::action_kind;
use tempfile::TempDir;

#[test]
fn schema_is_idempotent_across_open_calls() {
    // Open the same path twice — the second `Database::open`
    // must not fail on `CREATE TABLE IF NOT EXISTS`.
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("test.db");
    let _db1 = Database::open(&path).unwrap();
    let db2 = Database::open(&path).unwrap();
    db2.record_harness_event("h1", "r1", "running", "idle", 100, false, None)
        .unwrap();
    assert_eq!(
        db2.recent_harness_events_by_harness("h1", 0, 10)
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn action_schema_is_idempotent_across_open_calls() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("test.db");
    let _db1 = Database::open(&path).unwrap();
    let db2 = Database::open(&path).unwrap();
    db2.record_harness_action("h1", "r1", 100, action_kind::TOOL_CALL, "{}", None)
        .unwrap();
    assert_eq!(
        db2.recent_harness_actions_by_harness("h1", 0, 10)
            .unwrap()
            .len(),
        1
    );
}
