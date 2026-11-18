use crate::db::test_support::fresh_db;
use crate::harness_log::context_sizes;

#[test]
fn upsert_then_get_and_overwrite() {
    let (_dir, db) = fresh_db();
    assert_eq!(db.context_nudge_mark("h1").unwrap(), None);
    db.set_context_nudge_mark("h1", "r1", None, true).unwrap();
    let m = db.context_nudge_mark("h1").unwrap().unwrap();
    assert_eq!(
        (m.room_id.as_str(), m.last_row_id, m.armed),
        ("r1", None, true)
    );
    db.set_context_nudge_mark("h1", "r1", Some(7), false)
        .unwrap();
    let m = db.context_nudge_mark("h1").unwrap().unwrap();
    assert_eq!((m.last_row_id, m.armed), (Some(7), false));
}

#[test]
fn sweep_removes_marks_of_an_orphaned_room() {
    let (_dir, db) = fresh_db();
    db.set_context_nudge_mark("h1", "ghost", Some(1), true)
        .unwrap();
    db.sweep_orphans().unwrap();
    assert_eq!(db.context_nudge_mark("h1").unwrap(), None);
}

fn put(db: &crate::db::Database, h: &str, kind: &str, payload: &str) {
    db.record_harness_action(h, "r1", 0, "turn_cost", payload, None, Some(kind))
        .unwrap();
}

#[test]
fn newest_context_tokens_agrees_with_context_sizes() {
    let (_dir, db) = fresh_db();
    assert_eq!(db.newest_context_tokens("c").unwrap(), None);
    put(&db, "c", "claude", r#"{"usage":{"input_tokens":1}}"#);
    put(
        &db,
        "c",
        "claude",
        r#"{"usage":{"input_tokens":2,"cache_read_input_tokens":30}}"#,
    );
    put(&db, "hm", "hermes", r#"{"usage":{"input_tokens":5}}"#);
    let (id, tokens) = db.newest_context_tokens("c").unwrap().unwrap();
    let sizes = context_sizes(&db, &["c".to_owned()]).unwrap();
    assert_eq!(sizes.len(), 1);
    assert_eq!(sizes[0].tokens, tokens);
    assert_eq!(tokens, 32);
    let newest = db.latest_turn_costs(&["c".to_owned()]).unwrap();
    assert_eq!(newest[0].row_id, id);
    assert_eq!(db.newest_context_tokens("hm").unwrap(), None);
}
