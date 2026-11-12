use super::*;
use crate::db::test_support::{fresh_db, room};

fn ids(db: &Database) -> Vec<String> {
    db.load_all()
        .unwrap()
        .rooms
        .into_iter()
        .map(|r| r.id)
        .collect()
}

fn raw(db: &Database, id: &str) -> String {
    db.conn
        .lock()
        .query_row("SELECT data FROM sessions WHERE id = ?1", [id], |r| {
            r.get(0)
        })
        .unwrap()
}

fn s(v: &[&str]) -> Vec<String> {
    v.iter().map(|x| (*x).to_string()).collect()
}

#[test]
fn a_new_room_is_appended() {
    let (_d, db) = fresh_db();
    db.save_all(&[room("a"), room("b")]).unwrap();
    db.write_room_rows(&[room("c")], &[], None).unwrap();
    assert_eq!(ids(&db), s(&["a", "b", "c"]));
}

#[test]
fn an_existing_room_is_replaced_in_place() {
    let (_d, db) = fresh_db();
    db.save_all(&[room("a"), room("b"), room("c")]).unwrap();
    let mut b = room("b");
    b.name = "renamed".into();
    db.write_room_rows(&[b], &[], None).unwrap();
    let rooms = db.load_all().unwrap().rooms;
    assert_eq!(rooms[1].id, "b");
    assert_eq!(rooms[1].name, "renamed");
    assert_eq!(ids(&db), s(&["a", "b", "c"]));
}

#[test]
fn removed_ids_are_gone_and_absent_ones_ignored() {
    let (_d, db) = fresh_db();
    db.save_all(&[room("a"), room("b")]).unwrap();
    db.write_room_rows(&[], &s(&["a", "nope"]), None).unwrap();
    assert_eq!(ids(&db), s(&["b"]));
}

#[test]
fn order_puts_listed_first_and_unlisted_after_in_old_order() {
    let (_d, db) = fresh_db();
    db.save_all(&[room("a"), room("b"), room("c"), room("d")])
        .unwrap();
    db.write_room_rows(&[], &[], Some(&s(&["c", "ghost", "a"])))
        .unwrap();
    assert_eq!(ids(&db), s(&["c", "a", "b", "d"]));
}

#[test]
fn upsert_removal_and_order_combine_in_one_call() {
    let (_d, db) = fresh_db();
    db.save_all(&[room("a"), room("b")]).unwrap();
    db.write_room_rows(&[room("c")], &s(&["a"]), Some(&s(&["c", "b"])))
        .unwrap();
    assert_eq!(ids(&db), s(&["c", "b"]));
}

#[test]
fn successive_calls_on_different_rooms_keep_both() {
    let (_d, db) = fresh_db();
    db.save_all(&[room("a"), room("b")]).unwrap();
    let mut a = room("a");
    a.name = "first".into();
    let mut b = room("b");
    b.name = "second".into();
    db.write_room_rows(&[a], &[], None).unwrap();
    db.write_room_rows(&[b], &[], None).unwrap();
    let rooms = db.load_all().unwrap().rooms;
    assert_eq!(rooms[0].name, "first");
    assert_eq!(rooms[1].name, "second");
}

#[test]
fn untouched_rows_are_byte_identical() {
    let (_d, db) = fresh_db();
    db.save_all(&[room("a"), room("b"), room("c")]).unwrap();
    let before = (raw(&db, "a"), raw(&db, "c"));
    db.write_room_rows(&[room("d")], &s(&["b"]), Some(&s(&["d"])))
        .unwrap();
    assert_eq!((raw(&db, "a"), raw(&db, "c")), before);
}
