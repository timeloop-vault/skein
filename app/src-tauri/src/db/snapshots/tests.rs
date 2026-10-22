use super::*;
use crate::db::test_support::{fresh_db, room};

#[test]
fn backup_rotation_keeps_one_previous_generation() {
    let (_dir, db) = fresh_db();
    db.save_all(&[room("r1")]).unwrap();
    let _ = db.load_all().unwrap();
    db.backup_last_known_good().unwrap();
    db.save_all(&[room("r1"), room("r2")]).unwrap();
    let bak = db.backup_last_known_good().unwrap();
    let prev = bak.with_extension("bak.1");
    assert_eq!(
        Database::open(&bak)
            .unwrap()
            .load_all()
            .unwrap()
            .rooms
            .len(),
        2
    );
    // The displaced snapshot survives one generation back.
    assert_eq!(
        Database::open(&prev)
            .unwrap()
            .load_all()
            .unwrap()
            .rooms
            .len(),
        1
    );
}

#[test]
fn count_backup_rooms_reads_snapshot_or_none() {
    let (_dir, db) = fresh_db();
    assert!(db.count_backup_rooms().is_none());
    db.save_all(&[room("r1")]).unwrap();
    db.backup_last_known_good().unwrap();
    assert_eq!(db.count_backup_rooms(), Some(1));
}

#[test]
fn backup_snapshot_survives_a_later_wipe() {
    let (_dir, db) = fresh_db();
    db.save_all(&[room("r1")]).unwrap();
    let _ = db.backup_last_known_good().unwrap();
    // Second call finds the data unchanged and skips; it must not fail
    // or disturb the existing .bak.
    let bak = db.backup_last_known_good().unwrap();
    let _ = db.load_all().unwrap();
    db.save_all(&[]).unwrap();
    let restored = Database::open(&bak).unwrap();
    assert_eq!(restored.load_all().unwrap().rooms.len(), 1);
}

#[test]
fn backup_strips_the_image_mirror_but_keeps_baseline_rows() {
    let (_dir, db) = fresh_db();
    db.save_all(&[room("r1")]).unwrap();
    db.insert_review_baseline_if_absent("r1", "shot.png", "binary", None, "h1", 1)
        .unwrap();
    db.set_review_baseline_image("r1", "shot.png", Some(&[1, 2, 3]))
        .unwrap();
    let _ = db.load_all().unwrap();

    let bak = db.backup_last_known_good().unwrap();
    let restored = Database::open(&bak).unwrap();
    assert_eq!(
        restored.review_baseline_image("r1", "shot.png").unwrap(),
        None,
        "the mirror is a rebuildable preview cache, not worth a snapshot copy"
    );
    assert_eq!(
        restored.review_baselines_for_room("r1").unwrap().len(),
        0,
        "baselines are excluded from the snapshot (a missing one over-reports safely)"
    );

    // Snapshotting must not touch the live db.
    assert_eq!(
        db.review_baseline_image("r1", "shot.png").unwrap(),
        Some(vec![1, 2, 3])
    );
}

// ---- #658: the allowlist snapshot ----

fn schema_names(conn: &Connection, kind: &str) -> Vec<String> {
    let mut stmt = conn
        .prepare(
            "SELECT name FROM sqlite_master WHERE type = ?1 \
             AND name NOT LIKE 'sqlite\\_%' ESCAPE '\\' ORDER BY name",
        )
        .unwrap();
    stmt.query_map([kind], |r| r.get::<_, String>(0))
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

fn count(db: &Database, table: &str) -> i64 {
    db.conn
        .lock()
        .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
        .unwrap()
}

fn insert_action(db: &Database, room_id: &str) {
    db.conn
        .lock()
        .execute(
            "INSERT INTO harness_actions \
             (harness_id, room_id, timestamp_ms, kind, payload, source) \
             VALUES ('h1', ?1, 1, 'tool_call', '{}', NULL)",
            params![room_id],
        )
        .unwrap();
}

fn thread_row(id: &str, room_id: &str) -> crate::db::review_threads::ReviewThreadRow {
    crate::db::review_threads::ReviewThreadRow {
        id: id.into(),
        room_id: room_id.into(),
        scope: "line".into(),
        file_path: Some("a.rs".into()),
        commit_sha: None,
        side: Some("new".into()),
        line_start: Some(1),
        line_end: Some(1),
        anchor_hash: None,
        anchor_lines: None,
        resolved_ms: None,
        created_ms: 1,
        updated_ms: 1,
    }
}

fn comment_row(
    id: &str,
    thread: &str,
    room_id: &str,
) -> crate::db::review_threads::ReviewCommentRow {
    crate::db::review_threads::ReviewCommentRow {
        id: id.into(),
        thread_id: thread.into(),
        room_id: room_id.into(),
        author_kind: "user".into(),
        author_id: None,
        body: "hi".into(),
        created_ms: 1,
        updated_ms: 1,
    }
}

/// Seeds the protected tables and every excluded one.
fn seed_everything(db: &Database) {
    db.save_all(&[room("r1"), room("r2")]).unwrap();
    db.insert_review_thread(&thread_row("t1", "r1")).unwrap();
    db.insert_review_comment(&comment_row("c1", "t1", "r1"))
        .unwrap();
    db.set_review_signoff("r1", "deadbeef", Some("main"), Some("ok"), 5)
        .unwrap();
    db.set_review_base_ref("r1", "develop", 6).unwrap();
    db.set_review_viewed("r1", "a.rs", "hash", 7).unwrap();
    db.save_global_todos(&[serde_json::json!({"id": "todo1", "text": "x"})])
        .unwrap();
    insert_action(db, "r1");
    let conn = db.conn.lock();
    conn.execute(
        "INSERT INTO harness_events \
         (harness_id, room_id, from_phase, to_phase, timestamp_ms, has_user_input, source) \
         VALUES ('h1', 'r1', 'idle', 'running', 1, 0, NULL)",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO review_baselines \
         (room_id, path, kind, content, harness_id, captured_ms, touched_ms) \
         VALUES ('r1', 'a.rs', 'text', 'v1', 'h1', 1, 1)",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO review_baseline_images (room_id, path, bytes) \
         VALUES ('r1', 'shot.png', X'89504e47')",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO harness_messages \
         (id, room_id, harness_id, from_room_id, from_harness_id, body, created_ms, read_ms) \
         VALUES ('m1', 'r1', 'h1', 'r2', NULL, 'hi', 1, NULL)",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO agent_tokens (token, room_id, created_ms, revoked_ms) \
         VALUES ('tok', 'r1', 1, NULL)",
        [],
    )
    .unwrap();
}

#[test]
fn every_table_is_classified_for_the_snapshot() {
    let (_dir, db) = fresh_db();
    let tables = schema_names(&db.conn.lock(), "table");
    for t in &tables {
        let n = usize::from(SNAPSHOT_TABLES.contains(&t.as_str()))
            + usize::from(SNAPSHOT_EXCLUDED.contains(&t.as_str()));
        assert_eq!(
            n, 1,
            "table `{t}` is not classified for the .bak snapshot (or is in both lists): \
             classify it in SNAPSHOT_TABLES or SNAPSHOT_EXCLUDED in db/snapshots.rs"
        );
    }
    for t in SNAPSHOT_TABLES.iter().chain(SNAPSHOT_EXCLUDED) {
        assert!(
            tables.iter().any(|x| x == t),
            "snapshot list names `{t}`, which is not in the schema"
        );
    }
}

#[test]
fn snapshot_round_trips_protected_state_and_empties_the_rest() {
    let (_dir, db) = fresh_db();
    seed_everything(&db);
    let _ = db.load_all().unwrap();
    let bak = db.backup_last_known_good().unwrap();

    let restored = Database::open(&bak).unwrap();
    assert_eq!(restored.load_all().unwrap().rooms.len(), 2);
    assert_eq!(restored.review_threads_for_room("r1").unwrap().len(), 1);
    assert_eq!(restored.review_comments_for_room("r1").unwrap().len(), 1);
    assert_eq!(
        restored.review_signoff("r1").unwrap().unwrap().head_sha,
        "deadbeef"
    );
    assert_eq!(
        restored.review_base_ref("r1").unwrap().as_deref(),
        Some("develop")
    );
    assert_eq!(restored.review_viewed_for_room("r1").unwrap().len(), 1);
    assert_eq!(restored.load_global_todos().unwrap().len(), 1);

    for t in SNAPSHOT_EXCLUDED {
        assert_eq!(count(&restored, t), 0, "{t} must be empty in the .bak");
    }
    let live_idx = schema_names(&db.conn.lock(), "index");
    let bak_idx = schema_names(&restored.conn.lock(), "index");
    assert!(live_idx.len() > 1, "expected indexes in the schema");
    for i in &live_idx {
        assert!(bak_idx.contains(i), "index {i} missing from the .bak");
    }
    insert_action(&restored, "r1");
    assert_eq!(count(&restored, "harness_actions"), 1);
    // The live DB kept its excluded rows.
    assert_eq!(count(&db, "harness_actions"), 1);
    drop(restored);

    // Manual restore: the .bak copied over as skein.db opens.
    let restore_dir = tempfile::TempDir::new().unwrap();
    let target = restore_dir.path().join("skein.db");
    std::fs::copy(&bak, &target).unwrap();
    assert_eq!(
        Database::open(&target)
            .unwrap()
            .load_all()
            .unwrap()
            .rooms
            .len(),
        2
    );
}

#[test]
fn unchanged_data_skips_rotation_and_changes_rotate() {
    let (_dir, db) = fresh_db();
    db.save_all(&[room("r1")]).unwrap();
    db.insert_review_thread(&thread_row("t1", "r1")).unwrap();
    let bak = db.backup_last_known_good().unwrap();
    let prev = bak.with_extension("bak.1");
    let tmp = bak.with_extension("bak.tmp");
    let first = std::fs::read(&bak).unwrap();
    assert!(!prev.exists());

    db.backup_last_known_good().unwrap();
    assert_eq!(std::fs::read(&bak).unwrap(), first);
    assert!(!prev.exists());
    assert!(!tmp.exists());

    // A change only in an excluded table does not rotate.
    insert_action(&db, "r1");
    db.backup_last_known_good().unwrap();
    assert_eq!(std::fs::read(&bak).unwrap(), first);
    assert!(!prev.exists());

    // A change only in a non-sessions allowlisted table does.
    db.insert_review_comment(&comment_row("c1", "t1", "r1"))
        .unwrap();
    db.backup_last_known_good().unwrap();
    assert!(prev.exists());
    assert_eq!(std::fs::read(&prev).unwrap(), first);
    let second = std::fs::read(&bak).unwrap();
    assert_ne!(second, first);
    assert!(!tmp.exists());

    // A renamed room rotates again: .bak.1 is the previous generation.
    let mut renamed = room("r1");
    renamed.name = "renamed".into();
    db.save_all(&[renamed]).unwrap();
    db.backup_last_known_good().unwrap();
    assert_eq!(std::fs::read(&prev).unwrap(), second);
    let new = Database::open(&bak).unwrap().load_all().unwrap();
    assert_eq!(new.rooms[0].name, "renamed");
}

#[test]
fn legacy_full_copy_is_replaced_once() {
    let (_dir, db) = fresh_db();
    db.save_all(&[room("r1")]).unwrap();
    insert_action(&db, "r1");
    let bak = db.path.with_extension("db.bak");
    db.conn
        .lock()
        .execute("VACUUM INTO ?1", params![bak.to_string_lossy()])
        .unwrap();
    let legacy = std::fs::read(&bak).unwrap();

    db.backup_last_known_good().unwrap();
    let prev = db.path.with_extension("db.bak.1");
    assert_eq!(std::fs::read(&prev).unwrap(), legacy);
    let fresh = Database::open(&bak).unwrap();
    assert_eq!(count(&fresh, "harness_actions"), 0);
    assert_eq!(fresh.load_all().unwrap().rooms.len(), 1);
}

#[test]
fn snapshot_sees_uncheckpointed_wal_writes() {
    let (_dir, db) = fresh_db();
    db.save_all(&[room("r1"), room("r2"), room("r3")]).unwrap();
    db.save_global_todos(&[serde_json::json!({"id": "t"})])
        .unwrap();
    let bak = db.backup_last_known_good().unwrap();
    let restored = Database::open(&bak).unwrap();
    assert_eq!(restored.load_all().unwrap().rooms.len(), 3);
    assert_eq!(restored.load_global_todos().unwrap().len(), 1);
}

#[test]
fn readonly_uri_encodes_windows_paths() {
    assert_eq!(
        readonly_uri(Path::new(r"C:\Users\a b\50%#x?\skein.db")),
        "file:/C:/Users/a%20b/50%25%23x%3F/skein.db?mode=ro"
    );
    assert_eq!(
        readonly_uri(Path::new(r"\\?\D:\data\skein.db")),
        "file:/D:/data/skein.db?mode=ro"
    );
    assert_eq!(
        readonly_uri(Path::new("/home/u/skein.db")),
        "file:/home/u/skein.db?mode=ro"
    );
    assert_eq!(
        readonly_uri(Path::new(r"\\server\share\skein.db")),
        "file:////server/share/skein.db?mode=ro"
    );
    assert_eq!(
        readonly_uri(Path::new(r"\\?\UNC\server\share\skein.db")),
        "file:////server/share/skein.db?mode=ro"
    );
}

#[test]
fn unattachable_bak_is_replaced_and_stale_tmp_journal_removed() {
    let dir = tempfile::TempDir::new().unwrap();
    let db = Database::open(&dir.path().join("skein.db")).unwrap();
    db.save_all(&[room("r1")]).unwrap();
    let bak = dir.path().join("skein.db.bak");
    std::fs::write(&bak, b"this is not a sqlite database, just junk bytes").unwrap();
    let journal = dir.path().join("skein.db.bak.tmp-journal");
    std::fs::write(&journal, b"stale").unwrap();
    db.backup_last_known_good().unwrap();
    assert!(!journal.exists());
    assert_eq!(db.count_backup_rooms(), Some(1));
}

#[test]
fn snapshot_works_in_a_folder_with_uri_special_characters() {
    let dir = tempfile::TempDir::new().unwrap();
    let sub = dir.path().join("my dir 50% #1");
    std::fs::create_dir(&sub).unwrap();
    let db = Database::open(&sub.join("skein.db")).unwrap();
    db.save_all(&[room("r1")]).unwrap();
    let bak = db.backup_last_known_good().unwrap();
    assert!(bak.exists());
    // Second run attaches the existing .bak through the same URI path.
    db.backup_last_known_good().unwrap();
    assert_eq!(db.count_backup_rooms(), Some(1));
}
