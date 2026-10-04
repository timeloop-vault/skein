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
    db2.record_harness_action("h1", "r1", 100, action_kind::TOOL_CALL, "{}", None, None)
        .unwrap();
    assert_eq!(
        db2.recent_harness_actions_by_harness("h1", 0, 10)
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn harness_kind_column_is_added_to_a_legacy_actions_table() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("legacy.db");
    {
        // The table exactly as it was before #538.
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute(
            "CREATE TABLE harness_actions (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                harness_id TEXT NOT NULL,
                room_id TEXT NOT NULL,
                timestamp_ms INTEGER NOT NULL,
                kind TEXT NOT NULL,
                payload TEXT NOT NULL,
                source TEXT
            )",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO harness_actions (harness_id, room_id, timestamp_ms, kind, payload)              VALUES ('h1', 'r1', 50, 'tool_call', '{}')",
            [],
        )
        .unwrap();
    }
    let db = Database::open(&path).unwrap();
    let rows = db.recent_harness_actions_by_harness("h1", 0, 10).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].harness_kind, None);
    db.record_harness_action(
        "h1",
        "r1",
        100,
        action_kind::TOOL_CALL,
        "{}",
        None,
        Some("claude"),
    )
    .unwrap();
    drop(db);

    // A second setup over the migrated file is a no-op, rows intact.
    let db = Database::open(&path).unwrap();
    let rows = db.recent_harness_actions_by_harness("h1", 0, 10).unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].harness_kind.as_deref(), Some("claude"));
    assert_eq!(rows[1].harness_kind, None);
}
