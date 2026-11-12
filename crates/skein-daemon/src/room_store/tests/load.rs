//! `load_rooms` and `load_with`: what a load installs, and that it cannot
//! interleave with an `apply`.

use std::sync::mpsc;
use std::time::Duration;

use skein_proto::room_mutations::RoomMutation as M;

use super::{Fixture, json_of, room};
use crate::rooms::load_rooms;

const BOUND: Duration = Duration::from_secs(20);

fn sample_rooms() -> Vec<skein_proto::rooms::Room> {
    vec![room("r1"), room("r2"), room("r3")]
}

#[test]
fn load_rooms_fills_the_store_in_load_order() {
    let f = Fixture::unfilled();
    f.db.write_room_rows(&sample_rooms(), &[], None).unwrap();
    let out = load_rooms(&f.db, &f.store).unwrap();
    assert_eq!(out.rooms.len(), 3);
    assert_eq!(json_of(&f.store.rooms()), json_of(&out.rooms));
    assert_eq!(f.store_ids(), ["r1", "r2", "r3"]);

    // The store is now ready: mutations are accepted.
    f.apply("a", vec![M::CreateRoom { room: room("r4") }]);
    assert_eq!(f.store_ids(), ["r1", "r2", "r3", "r4"]);
}

#[test]
fn apply_is_refused_before_any_load_rooms() {
    let f = Fixture::unfilled();
    f.db.write_room_rows(&sample_rooms(), &[], None).unwrap();
    let err = f
        .store
        .apply("a", vec![M::CreateRoom { room: room("r9") }])
        .unwrap_err();
    assert!(err.contains("not ready"), "{err}");
    assert!(f.store.rooms().is_empty());
    assert!(f.events().is_empty());
}

#[test]
fn load_rooms_leaves_a_quarantined_row_out_of_the_store() {
    let f = Fixture::unfilled();
    f.db.write_room_rows(&sample_rooms(), &[], None).unwrap();
    f.db.corrupt_room_for_test("r2");
    let out = load_rooms(&f.db, &f.store).unwrap();
    assert_eq!(out.skipped.len(), 1);
    assert_eq!(out.skipped[0].id, "r2");
    assert_eq!(f.store_ids(), ["r1", "r3"]);
    assert_eq!(json_of(&f.store.rooms()), json_of(&out.rooms));

    // The store works afterwards, without resurrecting the bad row.
    f.apply("a", vec![M::CreateRoom { room: room("r4") }]);
    assert_eq!(f.store_ids(), ["r1", "r3", "r4"]);
}

#[test]
fn second_load_rooms_persists_dirty_changes_first() {
    let f = Fixture::unfilled();
    f.db.write_room_rows(&sample_rooms(), &[], None).unwrap();
    let first = load_rooms(&f.db, &f.store).unwrap();
    assert!(first.first_load);

    let mut r2 = room("r2");
    r2.task = "dirty".into();
    f.apply("a", vec![M::UpdateRoom { room: r2 }]);
    // Debounce is minutes long: not in sqlite yet.
    assert_eq!(f.disk_room("r2").unwrap().task, "");

    let second = load_rooms(&f.db, &f.store).unwrap();
    assert!(!second.first_load);
    let task_of = |rooms: &[skein_proto::rooms::Room]| {
        rooms.iter().find(|r| r.id == "r2").unwrap().task.clone()
    };
    assert_eq!(task_of(&second.rooms), "dirty");
    assert_eq!(task_of(&f.store.rooms()), "dirty");
    assert_eq!(f.disk_room("r2").unwrap().task, "dirty");
    assert_eq!(f.store_ids(), ["r1", "r2", "r3"]);
}

#[test]
fn load_with_blocks_a_concurrent_apply_until_the_loader_returns() {
    let f = Fixture::with_rooms(&[room("r1")]);
    let (entered_tx, entered_rx) = mpsc::channel::<()>();
    let (release_tx, release_rx) = mpsc::channel::<()>();
    let (loaded_tx, loaded_rx) = mpsc::channel::<Result<usize, String>>();
    let (applied_tx, applied_rx) = mpsc::channel::<Result<(), String>>();

    let store = f.store.clone();
    let db = f.db.clone();
    std::thread::spawn(move || {
        let res = store.load_with(|| {
            entered_tx.send(()).unwrap();
            release_rx.recv_timeout(BOUND).unwrap();
            db.load_all()
        });
        loaded_tx.send(res.map(|o| o.rooms.len())).unwrap();
    });
    entered_rx.recv_timeout(BOUND).unwrap();

    let store = f.store.clone();
    std::thread::spawn(move || {
        let res = store
            .apply("b", vec![M::CreateRoom { room: room("r2") }])
            .map(|_| ());
        applied_tx.send(res).unwrap();
    });

    // The loader still holds the store: the apply cannot have completed.
    assert_eq!(
        applied_rx.recv_timeout(Duration::from_millis(300)),
        Err(mpsc::RecvTimeoutError::Timeout)
    );
    release_tx.send(()).unwrap();
    assert_eq!(loaded_rx.recv_timeout(BOUND).unwrap(), Ok(1));
    applied_rx.recv_timeout(BOUND).unwrap().unwrap();

    assert_eq!(f.store_ids(), ["r1", "r2"], "the apply was not lost");
    let fresh: Vec<String> =
        f.db.load_all()
            .unwrap()
            .rooms
            .into_iter()
            .map(|r| r.id)
            .collect();
    assert_eq!(fresh, ["r1", "r2"]);
}
