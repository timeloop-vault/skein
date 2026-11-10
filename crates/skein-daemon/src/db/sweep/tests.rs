use super::*;
use crate::db::element_anchors;
use crate::db::rooms::Room;
use rusqlite::params;
use tempfile::TempDir;

fn fresh() -> (TempDir, Database) {
    let dir = TempDir::new().unwrap();
    let db = Database::open(&dir.path().join("test.db")).unwrap();
    (dir, db)
}

fn room(id: &str) -> Room {
    Room {
        id: id.into(),
        name: format!("room {id}"),
        task: String::new(),
        status: "idle".into(),
        badge: 0,
        harnesses: Vec::new(),
        active_harness_id: String::new(),
        cwd: None,
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
    }
}

/// Inserts one row into `table` for `room_id`, with whatever dummy
/// values satisfy its `NOT NULL` columns — content doesn't matter,
/// only that a row keyed to this `room_id` exists to sweep (or not).
fn seed_row(db: &Database, table: &str, room_id: &str) {
    let conn = db.conn.lock();
    let sql = match table {
        "harness_events" => {
            "INSERT INTO harness_events \
             (harness_id, room_id, from_phase, to_phase, timestamp_ms, has_user_input, source) \
             VALUES ('h1', ?1, 'idle', 'running', 1, 0, NULL)"
        }
        "harness_actions" => {
            "INSERT INTO harness_actions \
             (harness_id, room_id, timestamp_ms, kind, payload, source) \
             VALUES ('h1', ?1, 1, 'tool_call', '{}', NULL)"
        }
        "review_baselines" => {
            "INSERT INTO review_baselines \
             (room_id, path, kind, content, harness_id, captured_ms, touched_ms) \
             VALUES (?1, 'a.rs', 'text', 'v1', 'h1', 1, 1)"
        }
        "review_baseline_images" => {
            "INSERT INTO review_baseline_images (room_id, path, bytes) \
             VALUES (?1, 'shot.png', X'89504e47')"
        }
        "review_threads" => {
            "INSERT INTO review_threads \
             (id, room_id, scope, file_path, commit_sha, side, line_start, line_end, \
              anchor_hash, anchor_lines, resolved_ms, created_ms, updated_ms) \
             VALUES ('thread-' || ?1, ?1, 'branch', 'a.rs', NULL, NULL, NULL, NULL, \
                     NULL, NULL, NULL, 1, 1)"
        }
        "review_comments" => {
            "INSERT INTO review_comments \
             (id, thread_id, room_id, author_kind, author_id, body, created_ms, updated_ms) \
             VALUES ('comment-' || ?1, 'thread-' || ?1, ?1, 'human', NULL, 'hi', 1, 1)"
        }
        "review_viewed" => {
            "INSERT INTO review_viewed (room_id, path, content_hash, viewed_ms) \
             VALUES (?1, 'a.rs', 'hash', 1)"
        }
        "review_settings" => {
            "INSERT INTO review_settings (room_id, base_ref, updated_ms) \
             VALUES (?1, 'main', 1)"
        }
        "review_addressed" => {
            "INSERT INTO review_addressed \
             (thread_id, room_id, commit_sha, harness_id, note, addressed_ms) \
             VALUES ('thread-' || ?1, ?1, NULL, 'h1', NULL, 1)"
        }
        "review_element_anchors" => element_anchors::SEED_SQL,
        "review_element_proposals" => crate::db::element_proposals::SEED_SQL,
        "agent_tokens" => {
            "INSERT INTO agent_tokens (token, room_id, created_ms, revoked_ms) \
             VALUES ('token-' || ?1, ?1, 1, NULL)"
        }
        "review_signoff" => {
            "INSERT INTO review_signoff (room_id, head_sha, base_ref, note, approved_ms) \
             VALUES (?1, 'deadbeef', NULL, NULL, 1)"
        }
        "harness_messages" => {
            "INSERT INTO harness_messages \
             (id, room_id, harness_id, from_room_id, from_harness_id, body, created_ms, read_ms) \
             VALUES ('msg-' || ?1, ?1, 'h1', 'elsewhere', NULL, 'hi', 1, NULL)"
        }
        other => panic!("seed_row: unhandled table {other}"),
    };
    conn.execute(sql, params![room_id]).unwrap();
}

fn row_count(db: &Database, table: &str, room_id: &str) -> i64 {
    db.conn
        .lock()
        .query_row(
            &format!("SELECT COUNT(*) FROM {table} WHERE room_id = ?1"),
            params![room_id],
            |row| row.get(0),
        )
        .unwrap()
}

/// #658: the sweep's room-keyed list covers every table with a `room_id`.
#[test]
fn room_keyed_tables_matches_schema() {
    let (_d, db) = fresh();
    let conn = db.conn.lock();
    let tables: Vec<String> = conn
        .prepare("SELECT name FROM sqlite_master WHERE type = 'table'")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    let mut with_room = Vec::new();
    for t in &tables {
        let has: bool = conn
            .prepare(&format!("PRAGMA table_info({t})"))
            .unwrap()
            .query_map([], |r| r.get::<_, String>(1))
            .unwrap()
            .map(Result::unwrap)
            .any(|c| c == "room_id");
        if has && t != "sessions" && t != "sessions_quarantine" {
            with_room.push(t.clone());
        }
    }
    for t in &with_room {
        assert!(ROOM_KEYED_TABLES.contains(&t.as_str()), "{t} not swept");
    }
    for t in ROOM_KEYED_TABLES {
        assert!(
            with_room.iter().any(|w| w == t),
            "{t} missing or no room_id"
        );
    }
}

/// #658: neither the loose scan nor the delete may scan a table.
#[test]
fn sweep_queries_use_an_index() {
    let (_d, db) = fresh();
    let conn = db.conn.lock();
    for table in ROOM_KEYED_TABLES {
        for sql in [
            first_room_sql(table),
            next_room_sql(table),
            delete_room_sql(table),
        ] {
            let mut stmt = conn.prepare(&format!("EXPLAIN QUERY PLAN {sql}")).unwrap();
            let details: Vec<String> = stmt
                .query_map(
                    rusqlite::params_from_iter(sql.contains("?1").then_some("x")),
                    |r| r.get::<_, String>(3),
                )
                .unwrap()
                .map(Result::unwrap)
                .collect();
            let plan = details.join(" | ");
            assert!(
                plan.contains("USING INDEX")
                    || plan.contains("USING COVERING INDEX")
                    || plan.contains("USING PRIMARY KEY"),
                "{table}: `{sql}` plan: {plan}"
            );
        }
    }
}

/// #658: several orphans, several rows each, all go; the count is exact.
#[test]
fn sweep_deletes_every_row_of_every_orphan() {
    let (_d, db) = fresh();
    db.save_all(&[room("live")]).unwrap();
    db.load_all().unwrap();
    for room_id in ["live", "o1", "o2", "o3"] {
        seed_row(&db, "harness_events", room_id);
        seed_row(&db, "harness_events", room_id);
        seed_row(&db, "harness_actions", room_id);
    }
    assert_eq!(db.sweep_orphans().unwrap(), 9);
    assert_eq!(row_count(&db, "harness_events", "live"), 2);
    assert_eq!(row_count(&db, "harness_actions", "live"), 1);
    for o in ["o1", "o2", "o3"] {
        assert_eq!(row_count(&db, "harness_events", o), 0);
        assert_eq!(row_count(&db, "harness_actions", o), 0);
    }
}

/// #658: nothing orphaned, nothing deleted.
#[test]
fn sweep_without_orphans_deletes_nothing() {
    let (_d, db) = fresh();
    db.save_all(&[room("live")]).unwrap();
    db.load_all().unwrap();
    for table in ROOM_KEYED_TABLES {
        seed_row(&db, table, "live");
    }
    assert_eq!(db.sweep_orphans().unwrap(), 0);
}

/// #237: every room-keyed table is swept for a room that is gone
/// from `sessions` entirely, while rows for a live room are left
/// untouched — and the returned count matches what was deleted.
#[test]
fn sweep_deletes_orphans_in_every_table_and_keeps_live_rows() {
    let (_d, db) = fresh();
    db.save_all(&[room("live")]).unwrap();
    db.load_all().unwrap();
    for table in ROOM_KEYED_TABLES {
        seed_row(&db, table, "live");
        seed_row(&db, table, "orphan");
    }

    let deleted = db.sweep_orphans().unwrap();

    assert_eq!(
        deleted,
        ROOM_KEYED_TABLES.len(),
        "one orphan row per table should have been swept"
    );
    for table in ROOM_KEYED_TABLES {
        assert_eq!(row_count(&db, table, "orphan"), 0, "table {table}");
        assert_eq!(row_count(&db, table, "live"), 1, "table {table}");
    }
}

/// #417: a retired, archived room still has its `sessions` row, so the
/// sweep never erases its history (unlike Delete forever).
#[test]
fn sweep_keeps_history_of_a_retired_archived_room() {
    let (_d, db) = fresh();
    let mut r = room("old");
    r.archived = Some(1_000);
    r.retired = Some(2_000);
    db.save_all(&[r]).unwrap();
    db.load_all().unwrap();
    for table in ROOM_KEYED_TABLES {
        seed_row(&db, table, "old");
    }

    let deleted = db.sweep_orphans().unwrap();

    assert_eq!(deleted, 0);
    for table in ROOM_KEYED_TABLES {
        assert_eq!(row_count(&db, table, "old"), 1, "table {table}");
    }
}

/// #237: a room parked in `sessions_quarantine` (unparseable, kept
/// for recovery — #167) is not an orphan. Its rows must survive the
/// sweep so the history is still there if the blob is fixed by hand.
#[test]
fn sweep_keeps_rows_for_a_quarantined_room() {
    let (_d, db) = fresh();
    db.save_all(&[room("live"), room("bad")]).unwrap();
    db.conn
        .lock()
        .execute("UPDATE sessions SET data = 'not json' WHERE id = 'bad'", [])
        .unwrap();
    let outcome = db.load_all().unwrap();
    assert_eq!(outcome.skipped.len(), 1, "bad should have been quarantined");
    for table in ROOM_KEYED_TABLES {
        seed_row(&db, table, "bad");
        seed_row(&db, table, "orphan");
    }

    let deleted = db.sweep_orphans().unwrap();

    assert_eq!(
        deleted,
        ROOM_KEYED_TABLES.len(),
        "only the truly orphaned rows should have been swept"
    );
    for table in ROOM_KEYED_TABLES {
        assert_eq!(row_count(&db, table, "bad"), 1, "table {table}");
        assert_eq!(row_count(&db, table, "orphan"), 0, "table {table}");
    }
}

#[test]
fn first_room_query_skips_null_room_ids() {
    assert!(first_room_sql("t").contains("WHERE room_id IS NOT NULL"));
}
