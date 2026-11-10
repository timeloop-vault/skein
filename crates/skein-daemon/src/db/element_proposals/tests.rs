use super::*;
use crate::db::{ReviewElementAnchorRow, ReviewThreadRow};
use tempfile::TempDir;

fn thread(id: &str, room: &str) -> ReviewThreadRow {
    ReviewThreadRow {
        id: id.into(),
        room_id: room.into(),
        scope: "element".into(),
        file_path: Some("a.html".into()),
        commit_sha: None,
        side: None,
        line_start: None,
        line_end: None,
        anchor_hash: None,
        anchor_lines: None,
        resolved_ms: None,
        created_ms: 7,
        updated_ms: 7,
    }
}

fn anchor(id: &str, room: &str) -> ReviewElementAnchorRow {
    ReviewElementAnchorRow {
        thread_id: id.into(),
        room_id: room.into(),
        file_path: "a.html".into(),
        anchor_json: "{}".into(),
        last_seen_json: None,
        updated_ms: 7,
    }
}

#[test]
fn a_proposal_round_trips_and_goes_with_its_thread() {
    let (_dir, db) = crate::db::test_support::fresh_db();
    db.insert_review_element_thread_proposal(
        &thread("t1", "r1"),
        &anchor("t1", "r1"),
        Some("{\"a\":1}"),
    )
    .unwrap();
    db.insert_review_element_thread(&thread("t2", "r1"), &anchor("t2", "r1"))
        .unwrap();
    db.insert_review_element_thread_proposal(&thread("t3", "r2"), &anchor("t3", "r2"), Some("{}"))
        .unwrap();

    assert_eq!(
        db.review_element_proposal("t1").unwrap().as_deref(),
        Some("{\"a\":1}")
    );
    assert_eq!(db.review_element_proposal("t2").unwrap(), None);
    assert_eq!(
        db.review_element_proposals_for_room("r1").unwrap(),
        vec![("t1".to_owned(), "{\"a\":1}".to_owned())]
    );

    assert!(db.delete_review_thread("t1").unwrap());
    assert_eq!(db.review_element_proposal("t1").unwrap(), None);
    assert_eq!(
        db.review_element_proposal("t3").unwrap().as_deref(),
        Some("{}")
    );
}

#[test]
fn a_failed_thread_insert_leaves_no_proposal() {
    let (_dir, db) = crate::db::test_support::fresh_db();
    db.insert_review_element_thread(&thread("t1", "r1"), &anchor("t1", "r1"))
        .unwrap();
    // Same thread id again: the thread insert fails, the tx rolls back.
    assert!(
        db.insert_review_element_thread_proposal(
            &thread("t1", "r1"),
            &anchor("t1", "r1"),
            Some("{}")
        )
        .is_err()
    );
    assert_eq!(db.review_element_proposal("t1").unwrap(), None);
}

#[test]
fn an_old_db_without_the_table_gains_it_on_open() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("test.db");
    {
        let db = Database::open(&path).unwrap();
        db.conn
            .lock()
            .execute("DROP TABLE review_element_proposals", [])
            .unwrap();
    }
    let db = Database::open(&path).unwrap();
    db.insert_review_element_thread_proposal(&thread("t1", "r1"), &anchor("t1", "r1"), Some("{}"))
        .unwrap();
    assert_eq!(
        db.review_element_proposal("t1").unwrap().as_deref(),
        Some("{}")
    );
}
