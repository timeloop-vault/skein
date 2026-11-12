//! A failing sqlite write: what the caller sees, and that nothing is lost.
//!
//! Failure is injected from a second connection to the same file: triggers
//! on `sessions` that `RAISE(ABORT)`, dropped to heal.

use std::time::{Duration, Instant};

use rusqlite::Connection;
use skein_proto::room_mutations::RoomMutation as M;

use super::{Fixture, room};

/// A second connection with a busy timeout, so heal never loses a race with
/// a store write that is failing at the same moment.
fn side_conn(f: &Fixture) -> Connection {
    let conn = f.raw();
    conn.busy_timeout(Duration::from_secs(10)).unwrap();
    conn
}

fn break_writes(f: &Fixture) {
    side_conn(f)
        .execute_batch(
            "CREATE TRIGGER fail_ins BEFORE INSERT ON sessions \
               BEGIN SELECT RAISE(ABORT, 'injected failure'); END;
             CREATE TRIGGER fail_upd BEFORE UPDATE ON sessions \
               BEGIN SELECT RAISE(ABORT, 'injected failure'); END;
             CREATE TRIGGER fail_del BEFORE DELETE ON sessions \
               BEGIN SELECT RAISE(ABORT, 'injected failure'); END;",
        )
        .unwrap();
}

fn heal(f: &Fixture) {
    side_conn(f)
        .execute_batch("DROP TRIGGER fail_ins; DROP TRIGGER fail_upd; DROP TRIGGER fail_del;")
        .unwrap();
}

fn create_r2() -> Vec<M> {
    vec![M::CreateRoom { room: room("r2") }]
}

#[test]
fn failed_structural_persist_errs_but_keeps_event_and_memory() {
    let f = Fixture::with_rooms(&[room("r1")]);
    break_writes(&f);
    let err = f.store.apply("a", create_r2()).unwrap_err();
    assert!(err.contains("injected failure"), "{err}");

    let (origin, changes) = f.only_event();
    assert_eq!(origin, "a");
    assert_eq!(changes, ["put:r2", "order:r1,r2"]);
    assert_eq!(f.store_ids(), ["r1", "r2"]);
    assert_eq!(f.disk_ids(), ["r1"]);
}

#[test]
fn retry_errs_while_failing_and_lands_after_heal() {
    let f = Fixture::with_rooms(&[room("r1")]);
    break_writes(&f);
    f.store.apply("a", create_r2()).unwrap_err();

    // The same mutations again: the room already exists in memory, so every
    // mutation is skipped, but that must not read as success.
    let err = f.store.apply("a", create_r2()).unwrap_err();
    assert!(err.contains("injected failure"), "{err}");
    assert_eq!(f.disk_ids(), ["r1"]);

    heal(&f);
    let out = f.store.apply("a", create_r2()).unwrap();
    assert_eq!(out.skipped.len(), 1, "the retry itself is a no-op");
    assert_eq!(f.disk_ids(), ["r1", "r2"], "Ok means the row is on disk");
    assert_eq!(f.store_ids(), ["r1", "r2"]);
}

#[test]
fn failed_structural_persist_reaches_sqlite_by_itself_after_heal() {
    let ms = Duration::from_millis;
    let f = Fixture::with_rooms_and_delays(&[room("r1")], ms(20), ms(60));
    break_writes(&f);
    f.store.apply("a", create_r2()).unwrap_err();
    heal(&f);

    let deadline = Instant::now() + Duration::from_secs(5);
    while !f.disk_ids().contains(&"r2".to_owned()) {
        assert!(Instant::now() < deadline, "retry never persisted the row");
        std::thread::sleep(ms(25));
    }
    assert_eq!(f.disk_ids(), ["r1", "r2"]);
}

#[test]
fn load_with_errs_without_running_the_loader_when_persist_fails() {
    let f = Fixture::with_rooms(&[room("r1")]);
    let mut r1 = room("r1");
    r1.task = "unsaved".into();
    f.apply("a", vec![M::UpdateRoom { room: r1 }]);
    break_writes(&f);

    let mut ran = false;
    let err = f
        .store
        .load_with(|| {
            ran = true;
            f.db.load_all()
        })
        .unwrap_err();
    assert!(err.contains("injected failure"), "{err}");
    assert!(
        !ran,
        "loader must not run when the dirty rows cannot persist"
    );
    assert_eq!(f.store.rooms()[0].task, "unsaved", "memory is untouched");

    // Once healed, the same load persists first and then succeeds.
    heal(&f);
    f.store.load_with(|| f.db.load_all()).unwrap();
    assert_eq!(f.disk_room("r1").unwrap().task, "unsaved");
    assert_eq!(f.store.rooms()[0].task, "unsaved");
}
