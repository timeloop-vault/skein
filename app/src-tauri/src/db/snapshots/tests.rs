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
    // Second call must overwrite, not fail (VACUUM INTO refuses
    // to write over an existing file on its own).
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
        1,
        "the baseline row itself must survive; only the mirror is stripped"
    );

    // Stripping the snapshot must not touch the live db.
    assert_eq!(
        db.review_baseline_image("r1", "shot.png").unwrap(),
        Some(vec![1, 2, 3])
    );
}
