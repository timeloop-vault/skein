use super::*;
use tempfile::TempDir;

fn fresh() -> (TempDir, Database) {
    let dir = TempDir::new().unwrap();
    let db = Database::open(&dir.path().join("test.db")).unwrap();
    (dir, db)
}

#[test]
fn first_touch_captures_and_a_second_touch_does_not_move_the_baseline() {
    let (_d, db) = fresh();
    assert!(
        db.insert_review_baseline_if_absent("r1", "src/a.rs", "text", Some("v1\n"), "h1", 100)
            .unwrap()
    );
    // The agent writes again. Re-capturing here would silently
    // absorb the pending change — the exact failure #211 names.
    assert!(
        !db.insert_review_baseline_if_absent("r1", "src/a.rs", "text", Some("v2\n"), "h2", 200)
            .unwrap()
    );
    let b = db.review_baseline("r1", "src/a.rs").unwrap().unwrap();
    assert_eq!(b.content.as_deref(), Some("v1\n"));
}

#[test]
fn touch_updates_attribution_only() {
    let (_d, db) = fresh();
    db.insert_review_baseline_if_absent("r1", "a.rs", "text", Some("v1\n"), "h1", 100)
        .unwrap();
    db.touch_review_baseline("r1", "a.rs", "h2", 250).unwrap();
    let b = db.review_baseline("r1", "a.rs").unwrap().unwrap();
    assert_eq!(b.harness_id, "h2");
    assert_eq!(b.touched_ms, 250);
    assert_eq!(b.captured_ms, 100);
    assert_eq!(b.content.as_deref(), Some("v1\n"));
}

#[test]
fn advance_moves_content_but_keeps_attribution() {
    let (_d, db) = fresh();
    db.insert_review_baseline_if_absent("r1", "a.rs", "text", Some("v1\n"), "h1", 100)
        .unwrap();
    db.advance_review_baseline("r1", "a.rs", "text", Some("v2\n"), 300)
        .unwrap();
    let b = db.review_baseline("r1", "a.rs").unwrap().unwrap();
    assert_eq!(b.content.as_deref(), Some("v2\n"));
    assert_eq!(b.harness_id, "h1", "accept must not erase who wrote it");
    assert_eq!(b.captured_ms, 300);
}

#[test]
fn baselines_are_scoped_per_room_and_path_ordered() {
    let (_d, db) = fresh();
    db.insert_review_baseline_if_absent("r1", "z.rs", "text", Some("z"), "h1", 1)
        .unwrap();
    db.insert_review_baseline_if_absent("r1", "a.rs", "text", Some("a"), "h1", 1)
        .unwrap();
    db.insert_review_baseline_if_absent("r2", "other.rs", "text", Some("o"), "h9", 1)
        .unwrap();

    let r1 = db.review_baselines_for_room("r1").unwrap();
    assert_eq!(
        r1.iter().map(|b| b.path.as_str()).collect::<Vec<_>>(),
        ["a.rs", "z.rs"]
    );
    assert_eq!(db.review_baselines_for_room("r2").unwrap().len(), 1);
    assert!(db.review_baseline("r2", "a.rs").unwrap().is_none());
}

#[test]
fn non_text_kinds_round_trip_with_a_null_content() {
    let (_d, db) = fresh();
    db.insert_review_baseline_if_absent("r1", "logo.png", "binary", None, "h1", 1)
        .unwrap();
    db.insert_review_baseline_if_absent("r1", "new.rs", "missing", None, "h1", 1)
        .unwrap();
    let b = db.review_baseline("r1", "logo.png").unwrap().unwrap();
    assert_eq!(b.kind, "binary");
    assert_eq!(b.content, None);
    assert_eq!(
        db.review_baseline("r1", "new.rs").unwrap().unwrap().kind,
        "missing"
    );
}

#[test]
fn baseline_image_upserts_and_clears_independently_of_the_row() {
    let (_d, db) = fresh();
    assert_eq!(db.review_baseline_image("r1", "shot.png").unwrap(), None);

    db.set_review_baseline_image("r1", "shot.png", Some(&[1, 2, 3]))
        .unwrap();
    assert_eq!(
        db.review_baseline_image("r1", "shot.png").unwrap(),
        Some(vec![1, 2, 3])
    );

    // A later write for the same path replaces, not appends.
    db.set_review_baseline_image("r1", "shot.png", Some(&[9]))
        .unwrap();
    assert_eq!(
        db.review_baseline_image("r1", "shot.png").unwrap(),
        Some(vec![9])
    );

    // Clearing removes the row outright rather than storing empty
    // bytes — `None` is what a non-image or over-cap write passes.
    db.set_review_baseline_image("r1", "shot.png", None)
        .unwrap();
    assert_eq!(db.review_baseline_image("r1", "shot.png").unwrap(), None);

    // Clearing a path that was never mirrored is a no-op, not an
    // error — every baseline write calls this unconditionally.
    db.set_review_baseline_image("r1", "never-mirrored.png", None)
        .unwrap();
}

#[test]
fn baseline_images_are_scoped_per_room() {
    let (_d, db) = fresh();
    db.set_review_baseline_image("r1", "shot.png", Some(&[1]))
        .unwrap();
    db.set_review_baseline_image("r2", "shot.png", Some(&[2]))
        .unwrap();
    assert_eq!(
        db.review_baseline_image("r1", "shot.png").unwrap(),
        Some(vec![1])
    );
    assert_eq!(
        db.review_baseline_image("r2", "shot.png").unwrap(),
        Some(vec![2])
    );
}

#[test]
fn cap_allows_multiple_paths_until_the_room_total_is_exceeded() {
    let (_d, db) = fresh();
    let cap = 10u64;
    db.set_review_baseline_image_capped("r1", "a.png", Some(&[0; 4]), cap)
        .unwrap();
    db.set_review_baseline_image_capped("r1", "b.png", Some(&[0; 4]), cap)
        .unwrap();
    // a (4) + b (4) + this write (4) = 12 > 10: refused.
    db.set_review_baseline_image_capped("r1", "c.png", Some(&[0; 4]), cap)
        .unwrap();

    assert_eq!(
        db.review_baseline_image("r1", "a.png").unwrap(),
        Some(vec![0; 4])
    );
    assert_eq!(
        db.review_baseline_image("r1", "b.png").unwrap(),
        Some(vec![0; 4])
    );
    assert_eq!(db.review_baseline_image("r1", "c.png").unwrap(), None);
}

#[test]
fn cap_drops_a_stale_mirror_rather_than_keep_the_old_one() {
    let (_d, db) = fresh();
    let cap = 10u64;
    db.set_review_baseline_image_capped("r1", "a.png", Some(&[1, 2, 3, 4]), cap)
        .unwrap();
    assert_eq!(
        db.review_baseline_image("r1", "a.png").unwrap(),
        Some(vec![1, 2, 3, 4])
    );

    // Overwriting with a blob that alone busts the room cap must
    // remove the old mirror outright, not leave it in place — a
    // stale "before" image would show the wrong diff, which is
    // worse than falling back to "unavailable".
    db.set_review_baseline_image_capped("r1", "a.png", Some(&[0; 20]), cap)
        .unwrap();
    assert_eq!(db.review_baseline_image("r1", "a.png").unwrap(), None);
}

#[test]
fn cap_allows_a_same_size_replacement_of_a_path_already_at_the_cap() {
    let (_d, db) = fresh();
    let cap = 10u64;
    db.set_review_baseline_image_capped("r1", "a.png", Some(&[0; 6]), cap)
        .unwrap();
    db.set_review_baseline_image_capped("r1", "b.png", Some(&[0; 4]), cap)
        .unwrap();
    // Room total is exactly at the cap (6 + 4 = 10).

    // Replacing b.png with a same-size blob must not count b's own
    // old bytes against itself, so this still fits.
    db.set_review_baseline_image_capped("r1", "b.png", Some(&[1; 4]), cap)
        .unwrap();
    assert_eq!(
        db.review_baseline_image("r1", "b.png").unwrap(),
        Some(vec![1; 4])
    );
    assert_eq!(
        db.review_baseline_image("r1", "a.png").unwrap(),
        Some(vec![0; 6])
    );
}

#[test]
fn baselines_survive_a_restart() {
    // #211's mandatory property: an empty tracker after a reload
    // makes pending hunks "silently disappear — the user sees their
    // changes auto-applied".
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("test.db");
    {
        let db = Database::open(&path).unwrap();
        db.insert_review_baseline_if_absent("r1", "src/a.rs", "text", Some("v1\n"), "h1", 100)
            .unwrap();
        db.insert_review_baseline_if_absent("r1", "src/b.rs", "text", Some("keep\n"), "h2", 110)
            .unwrap();
        db.advance_review_baseline("r1", "src/b.rs", "text", Some("accepted\n"), 200)
            .unwrap();
    }
    let db = Database::open(&path).unwrap();
    let rows = db.review_baselines_for_room("r1").unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].content.as_deref(), Some("v1\n"));
    assert_eq!(rows[0].harness_id, "h1");
    assert_eq!(rows[1].content.as_deref(), Some("accepted\n"));
}

#[test]
fn crlf_and_unicode_content_survives_the_round_trip() {
    let (_d, db) = fresh();
    let content = "line\r\nnäst\r\n🧵 skein\r\n";
    db.insert_review_baseline_if_absent("r1", "a.rs", "text", Some(content), "h1", 1)
        .unwrap();
    assert_eq!(
        db.review_baseline("r1", "a.rs")
            .unwrap()
            .unwrap()
            .content
            .as_deref(),
        Some(content)
    );
}

fn nb<'a>(path: &'a str, content: Option<&'a str>, image: Option<&'a [u8]>) -> NewBaseline<'a> {
    NewBaseline {
        path,
        kind: if content.is_some() { "text" } else { "binary" },
        content,
        image,
    }
}

#[test]
fn present_paths_returns_only_existing_rows_of_that_room() {
    let (_d, db) = fresh();
    db.insert_review_baseline_if_absent("r1", "a.rs", "text", Some("a"), "h1", 1)
        .unwrap();
    db.insert_review_baseline_if_absent("r2", "b.rs", "text", Some("b"), "h1", 1)
        .unwrap();
    let paths = ["a.rs", "b.rs", "c.rs"].map(String::from);
    let (present, _) = db.review_baseline_paths_present("r1", &paths).unwrap();
    assert_eq!(present, HashSet::from(["a.rs".to_string()]));
}

#[test]
fn batch_insert_skips_existing_rows_without_changing_them() {
    let (_d, db) = fresh();
    db.insert_review_baseline_if_absent("r1", "old.rs", "text", Some("v1"), "h1", 100)
        .unwrap();
    db.set_review_baseline_image("r1", "old.rs", Some(&[7]))
        .unwrap();
    let rows = [
        nb("old.rs", Some("v2"), Some(&[9, 9])),
        nb("new.rs", Some("n"), None),
    ];
    let (inserted, _) = db
        .insert_review_baselines_if_absent("r1", &rows, "h2", 200)
        .unwrap();
    assert_eq!(inserted, ["new.rs"]);
    let old = db.review_baseline("r1", "old.rs").unwrap().unwrap();
    assert_eq!(old.content.as_deref(), Some("v1"));
    assert_eq!(old.harness_id, "h1");
    assert_eq!(old.captured_ms, 100);
    assert_eq!(old.touched_ms, 100);
    // The skipped row's existing mirror is untouched too.
    assert_eq!(
        db.review_baseline_image("r1", "old.rs").unwrap(),
        Some(vec![7])
    );
    let new = db.review_baseline("r1", "new.rs").unwrap().unwrap();
    assert_eq!(new.harness_id, "h2");
    assert_eq!(new.captured_ms, 200);
}

#[test]
fn batch_insert_mirrors_images_only_for_inserted_rows_under_the_cap() {
    let (_d, db) = fresh();
    db.insert_review_baseline_if_absent("r1", "have.png", "binary", None, "h1", 1)
        .unwrap();
    let rows = [
        nb("have.png", None, Some(&[1, 1])),
        nb("a.png", None, Some(&[1, 2, 3])),
        nb("b.png", None, Some(&[4, 5, 6])),
        nb("c.png", None, Some(&[7])),
    ];
    // Cap of 4: a.png (3) fits, b.png (3+3) does not, c.png (3+1) fits.
    let (inserted, _) = db
        .insert_review_baselines_if_absent_capped("r1", &rows, "h1", 5, 4)
        .unwrap();
    assert_eq!(inserted, ["a.png", "b.png", "c.png"]);
    assert_eq!(db.review_baseline_image("r1", "have.png").unwrap(), None);
    assert_eq!(
        db.review_baseline_image("r1", "a.png").unwrap(),
        Some(vec![1, 2, 3])
    );
    assert_eq!(db.review_baseline_image("r1", "b.png").unwrap(), None);
    assert_eq!(
        db.review_baseline_image("r1", "c.png").unwrap(),
        Some(vec![7])
    );
}

#[test]
fn empty_batches_are_no_ops() {
    let (_d, db) = fresh();
    let (present, _) = db.review_baseline_paths_present("r1", &[]).unwrap();
    assert!(present.is_empty());
    let (inserted, _) = db
        .insert_review_baselines_if_absent("r1", &[], "h1", 1)
        .unwrap();
    assert_eq!(inserted, Vec::<String>::new());
    assert_eq!(db.review_baselines_for_room("r1").unwrap().len(), 0);
}
