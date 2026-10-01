use super::*;
use tempfile::TempDir;

fn fresh() -> (TempDir, Database) {
    let dir = TempDir::new().unwrap();
    let db = Database::open(&dir.path().join("test.db")).unwrap();
    (dir, db)
}

fn line_thread(id: &str, path: &str, start: i64, end: i64) -> ReviewThreadRow {
    ReviewThreadRow {
        id: id.into(),
        room_id: "r1".into(),
        scope: "line".into(),
        file_path: Some(path.into()),
        commit_sha: None,
        side: Some("new".into()),
        line_start: Some(start),
        line_end: Some(end),
        anchor_hash: Some("deadbeef".into()),
        anchor_lines: Some(r#"["let x = 1;"]"#.into()),
        resolved_ms: None,
        created_ms: 100,
        updated_ms: 100,
    }
}

fn comment(id: &str, thread_id: &str, body: &str, ms: i64) -> ReviewCommentRow {
    ReviewCommentRow {
        id: id.into(),
        thread_id: thread_id.into(),
        room_id: "r1".into(),
        author_kind: "user".into(),
        author_id: None,
        body: body.into(),
        created_ms: ms,
        updated_ms: ms,
    }
}

#[test]
fn a_thread_round_trips_with_its_anchor_intact() {
    let (_d, db) = fresh();
    let t = line_thread("t1", "src/a.rs", 12, 14);
    db.insert_review_thread(&t).unwrap();
    assert_eq!(db.review_thread("t1").unwrap().as_ref(), Some(&t));
    assert_eq!(db.review_threads_for_room("r1").unwrap(), vec![t]);
    // Another room's review is not this one's.
    assert!(db.review_threads_for_room("r2").unwrap().is_empty());
}

#[test]
fn replies_are_rows_on_the_same_thread_in_order() {
    // D5: flat comments with replies. A reply has nowhere to nest.
    let (_d, db) = fresh();
    db.insert_review_thread(&line_thread("t1", "a.rs", 1, 1))
        .unwrap();
    db.insert_review_comment(&comment("c1", "t1", "why this?", 100))
        .unwrap();
    db.insert_review_comment(&comment("c2", "t1", "because X", 200))
        .unwrap();
    let all = db.review_comments_for_room("r1").unwrap();
    let bodies: Vec<&str> = all.iter().map(|c| c.body.as_str()).collect();
    assert_eq!(bodies, vec!["why this?", "because X"]);
}

#[test]
fn a_comment_carries_its_author_from_the_first_migration() {
    // v1 only writes `user`, but #213's agent replies must be a row
    // change and not a migration (D7).
    let (_d, db) = fresh();
    db.insert_review_thread(&line_thread("t1", "a.rs", 1, 1))
        .unwrap();
    let mut agent = comment("c1", "t1", "addressed in 9531fc3", 100);
    agent.author_kind = "agent".into();
    agent.author_id = Some("h-claude-1".into());
    db.insert_review_comment(&agent).unwrap();
    let back = &db.review_comments_for_room("r1").unwrap()[0];
    assert_eq!(back.author_kind, "agent");
    assert_eq!(back.author_id.as_deref(), Some("h-claude-1"));
}

#[test]
fn a_reply_bumps_the_thread_but_not_its_creation_time() {
    let (_d, db) = fresh();
    db.insert_review_thread(&line_thread("t1", "a.rs", 1, 1))
        .unwrap();
    db.insert_review_comment(&comment("c1", "t1", "first", 500))
        .unwrap();
    let t = db.review_thread("t1").unwrap().unwrap();
    assert_eq!(t.created_ms, 100);
    assert_eq!(t.updated_ms, 500);
}

#[test]
fn deleting_the_last_comment_takes_the_thread_with_it() {
    // An empty thread would leave an anchor marker in the gutter
    // with nothing to read under it.
    let (_d, db) = fresh();
    db.insert_review_thread(&line_thread("t1", "a.rs", 1, 1))
        .unwrap();
    db.insert_review_comment(&comment("c1", "t1", "one", 100))
        .unwrap();
    db.insert_review_comment(&comment("c2", "t1", "two", 200))
        .unwrap();

    assert_eq!(db.delete_review_comment("c2").unwrap(), None);
    assert!(db.review_thread("t1").unwrap().is_some());

    assert_eq!(
        db.delete_review_comment("c1").unwrap().as_deref(),
        Some("t1")
    );
    assert!(db.review_thread("t1").unwrap().is_none());
}

#[test]
fn deleting_a_comment_that_is_already_gone_is_not_an_error() {
    let (_d, db) = fresh();
    assert_eq!(db.delete_review_comment("nope").unwrap(), None);
}

#[test]
fn deleting_a_thread_takes_every_comment_on_it() {
    let (_d, db) = fresh();
    db.insert_review_thread(&line_thread("t1", "a.rs", 1, 1))
        .unwrap();
    db.insert_review_comment(&comment("c1", "t1", "one", 100))
        .unwrap();
    db.insert_review_comment(&comment("c2", "t1", "two", 200))
        .unwrap();
    assert!(db.delete_review_thread("t1").unwrap());
    assert!(db.review_comments_for_room("r1").unwrap().is_empty());
}

#[test]
fn resolve_and_reopen_are_both_reachable() {
    let (_d, db) = fresh();
    db.insert_review_thread(&line_thread("t1", "a.rs", 1, 1))
        .unwrap();
    assert!(db.set_review_thread_resolved("t1", Some(900), 900).unwrap());
    assert_eq!(
        db.review_thread("t1").unwrap().unwrap().resolved_ms,
        Some(900)
    );
    assert!(db.set_review_thread_resolved("t1", None, 950).unwrap());
    assert_eq!(db.review_thread("t1").unwrap().unwrap().resolved_ms, None);
    // A thread that no longer exists reports that rather than lying.
    assert!(!db.set_review_thread_resolved("gone", Some(1), 1).unwrap());
}

#[test]
fn editing_a_comment_reports_whether_it_landed() {
    let (_d, db) = fresh();
    db.insert_review_thread(&line_thread("t1", "a.rs", 1, 1))
        .unwrap();
    db.insert_review_comment(&comment("c1", "t1", "typo", 100))
        .unwrap();
    assert!(db.update_review_comment("c1", "fixed", 300).unwrap());
    let back = &db.review_comments_for_room("r1").unwrap()[0];
    assert_eq!(back.body, "fixed");
    assert_eq!(back.updated_ms, 300);
    assert_eq!(back.created_ms, 100, "creation time is not an edit time");
    assert!(!db.update_review_comment("gone", "x", 1).unwrap());
}

#[test]
fn re_anchoring_stores_the_new_coordinates() {
    // A thread must search from where it was last seen, not from
    // where it was first written, or the distance tie-break decays
    // into noise over a multi-round review.
    let (_d, db) = fresh();
    db.insert_review_thread(&line_thread("t1", "a.rs", 12, 14))
        .unwrap();
    db.update_review_thread_anchor("t1", 40, 42).unwrap();
    let t = db.review_thread("t1").unwrap().unwrap();
    assert_eq!((t.line_start, t.line_end), (Some(40), Some(42)));
    assert_eq!(
        t.anchor_lines.as_deref(),
        Some(r#"["let x = 1;"]"#),
        "the anchor text itself never moves — only its coordinates"
    );
}

#[test]
fn a_review_level_thread_carries_no_anchor_at_all() {
    let (_d, db) = fresh();
    let t = ReviewThreadRow {
        id: "t1".into(),
        room_id: "r1".into(),
        scope: "review".into(),
        file_path: None,
        commit_sha: None,
        side: None,
        line_start: None,
        line_end: None,
        anchor_hash: None,
        anchor_lines: None,
        resolved_ms: None,
        created_ms: 1,
        updated_ms: 1,
    };
    db.insert_review_thread(&t).unwrap();
    assert_eq!(db.review_thread("t1").unwrap(), Some(t));
}

#[test]
fn threads_and_comments_survive_reopening_the_database() {
    // Persistence is the point: a review that evaporates on restart
    // is worse than none, because the user believes it is recorded.
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("test.db");
    {
        let db = Database::open(&path).unwrap();
        db.insert_review_thread(&line_thread("t1", "a.rs", 3, 3))
            .unwrap();
        db.insert_review_comment(&comment("c1", "t1", "rename this", 100))
            .unwrap();
    }
    let db = Database::open(&path).unwrap();
    assert_eq!(db.review_threads_for_room("r1").unwrap().len(), 1);
    assert_eq!(
        db.review_comments_for_room("r1").unwrap()[0].body,
        "rename this"
    );
}

// ── viewed markers ────────────────────────────────────────────

#[test]
fn a_viewed_marker_records_what_was_looked_at_not_merely_that_it_was() {
    let (_d, db) = fresh();
    db.set_review_viewed("r1", "a.rs", "hash-v1", 100).unwrap();
    assert_eq!(
        db.review_viewed_for_room("r1").unwrap(),
        vec![("a.rs".to_string(), "hash-v1".to_string())]
    );
    // Looking again at newer content replaces the mark.
    db.set_review_viewed("r1", "a.rs", "hash-v2", 200).unwrap();
    assert_eq!(
        db.review_viewed_for_room("r1").unwrap(),
        vec![("a.rs".to_string(), "hash-v2".to_string())]
    );
    db.clear_review_viewed("r1", "a.rs").unwrap();
    assert!(db.review_viewed_for_room("r1").unwrap().is_empty());
}

// ── base ref ──────────────────────────────────────────────────

#[test]
fn the_base_ref_is_unset_until_chosen_and_then_sticks() {
    let (_d, db) = fresh();
    assert_eq!(
        db.review_base_ref("r1").unwrap(),
        None,
        "unset means fall back to the repo own guess"
    );
    db.set_review_base_ref("r1", "main", 100).unwrap();
    assert_eq!(db.review_base_ref("r1").unwrap().as_deref(), Some("main"));
    // A stacked branch points at the branch below it.
    db.set_review_base_ref("r1", "feat/211-review-baseline", 200)
        .unwrap();
    assert_eq!(
        db.review_base_ref("r1").unwrap().as_deref(),
        Some("feat/211-review-baseline")
    );
    assert_eq!(db.review_base_ref("r2").unwrap(), None);
}
