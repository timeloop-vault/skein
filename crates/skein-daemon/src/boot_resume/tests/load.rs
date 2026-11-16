//! The load wiring: the barrier, the retry and what gets stored.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::json;
use skein_proto::rooms::Harness;

use super::{find, harness, hooked_room, legacy_opencode_cmd, room, sample, sv};
use crate::boot_resume::BootResume;
use crate::db::test_support::fresh_db;
use crate::event_sink::EventSink;
use crate::room_store::RoomStore;

struct Quiet;

impl EventSink for Quiet {
    fn emit_json(&self, _: &str, _: String) -> Result<(), String> {
        Ok(())
    }
}

fn setup() -> (tempfile::TempDir, Arc<crate::db::Database>, Arc<RoomStore>) {
    let (dir, db) = fresh_db();
    let db = Arc::new(db);
    let store = RoomStore::new(Arc::clone(&db), Arc::new(Quiet));
    (dir, db, store)
}

fn no_probe(_: &Harness) -> Result<bool, String> {
    panic!("no claim, so no probe")
}

#[test]
fn a_failed_flush_still_returns_the_rooms_the_store_holds() {
    let (_dir, db, store) = setup();
    let mut seed = hooked_room();
    seed[0].harnesses.remove(0);
    db.write_room_rows(&seed, &[], None).unwrap();
    let mut boot = BootResume::detached();
    // The busy check breaks the table after the rooms were read. Only a
    // cmd changes (not structural), so the edit succeeds and the flush fails.
    let broken = Arc::clone(&db);
    boot.busy = Arc::new(move |_| {
        crate::db::test_support::break_room_writes(&broken);
        false
    });

    let out = boot.load_rooms_with(&db, &store, no_probe).unwrap();
    assert_eq!(find(&out.rooms, "o").cmd, Some(sv(&["opencode"])));
    assert_eq!(
        serde_json::to_value(&out.rooms).unwrap(),
        serde_json::to_value(store.rooms()).unwrap(),
        "the client baseline must be what the store holds"
    );
}

#[test]
fn a_failed_preparation_is_retried_by_the_next_load_only() {
    let (_dir, db, store) = setup();
    // "o" has a legacy cmd (non-structural); "m" has an old shell-mode record
    // with no `launch`, which the migration fills (structural, so the edit
    // itself persists, and fails while the table is broken).
    let mut seed = hooked_room();
    seed[0].harnesses.remove(0);
    let mut m = harness("m", "claude", Some("m-sid"), &["pwsh.exe"], &json!({}));
    m.launch = None;
    seed[0].harnesses.push(m);
    db.write_room_rows(&seed, &[], None).unwrap();
    let mut boot = BootResume::detached();
    let broken = Arc::clone(&db);
    boot.busy = Arc::new(move |_| {
        crate::db::test_support::break_room_writes(&broken);
        false
    });

    let first = boot.load_rooms_with(&db, &store, no_probe).unwrap();
    assert_eq!(find(&first.rooms, "o").cmd, Some(legacy_opencode_cmd()));
    assert!(find(&first.rooms, "m").launch.is_none());

    // The disk is back and holds the original rows; the next load retries.
    crate::db::test_support::repair_room_writes(&db);
    db.write_room_rows(&seed, &[], None).unwrap();
    boot.busy = Arc::new(|_| false);
    let second = boot.load_rooms_with(&db, &store, no_probe).unwrap();
    assert_eq!(find(&second.rooms, "o").cmd, Some(sv(&["opencode"])));
    assert!(find(&second.rooms, "m").launch.is_some());
    assert_eq!(find(&store.rooms(), "o").cmd, Some(sv(&["opencode"])));
    assert_eq!(
        find(&db.load_all().unwrap().rooms, "o").cmd,
        Some(sv(&["opencode"]))
    );

    // Done now: a legacy cmd put back is kept.
    db.write_room_rows(&seed, &[], None).unwrap();
    let third = boot.load_rooms_with(&db, &store, no_probe).unwrap();
    assert_eq!(find(&third.rooms, "o").cmd, Some(legacy_opencode_cmd()));
}

#[test]
fn first_load_prepares_stores_and_returns_later_loads_do_not() {
    let (_dir, db, store) = setup();
    db.write_room_rows(&sample(), &[], None).unwrap();
    let boot = BootResume::detached();

    let first = boot.load_rooms_with(&db, &store, no_probe).unwrap();
    assert_eq!(find(&first.rooms, "o2").cmd, Some(sv(&["opencode"])));
    assert_eq!(store.rooms().len(), 2);
    // Persisted through the per-row path.
    let reread = db.load_all().unwrap();
    assert_eq!(find(&reread.rooms, "o2").cmd, Some(sv(&["opencode"])));

    // A second load must not re-prepare: put a legacy cmd back and see it kept.
    db.write_room_rows(&hooked_room(), &[], None).unwrap();
    let second = boot.load_rooms_with(&db, &store, no_probe).unwrap();
    assert_eq!(find(&second.rooms, "o").cmd, Some(legacy_opencode_cmd()));
}

#[test]
fn a_second_boot_over_a_prepared_store_writes_nothing() {
    let (_dir, db, store) = setup();
    db.write_room_rows(&sample(), &[], None).unwrap();
    let first = BootResume::detached()
        .load_rooms_with(&db, &store, no_probe)
        .unwrap();
    let before = serde_json::to_value(db.load_all().unwrap().rooms).unwrap();
    // A fresh process: a new `BootResume` prepares again, over the same rows.
    let second = BootResume::detached()
        .load_rooms_with(&db, &store, no_probe)
        .unwrap();
    assert_eq!(
        serde_json::to_value(&second.rooms).unwrap(),
        serde_json::to_value(&first.rooms).unwrap()
    );
    assert_eq!(
        serde_json::to_value(db.load_all().unwrap().rooms).unwrap(),
        before
    );
}

#[test]
fn an_empty_first_load_does_not_use_up_the_preparation() {
    let (_dir, db, store) = setup();
    let boot = BootResume::detached();

    let empty = boot.load_rooms_with(&db, &store, no_probe).unwrap();
    assert!(empty.rooms.is_empty());

    db.write_room_rows(&sample(), &[], None).unwrap();
    let later = boot.load_rooms_with(&db, &store, no_probe).unwrap();
    assert_eq!(find(&later.rooms, "o2").cmd, Some(sv(&["opencode"])));
}

#[test]
fn a_retry_skips_a_running_harness() {
    let (_dir, db, store) = setup();
    let seed = vec![room(
        "r",
        false,
        vec![
            harness(
                "up",
                "opencode",
                None,
                &["opencode", "--port", "1"],
                &json!({}),
            ),
            harness(
                "down",
                "opencode",
                None,
                &["opencode", "--port", "2"],
                &json!({}),
            ),
        ],
    )];
    db.write_room_rows(&seed, &[], None).unwrap();
    let mut boot = BootResume::detached();
    boot.busy = Arc::new(|id| id == "up");

    let out = boot.load_rooms_with(&db, &store, no_probe).unwrap();
    assert_eq!(
        find(&out.rooms, "up").cmd,
        Some(sv(&["opencode", "--port", "1"]))
    );
    assert_eq!(find(&out.rooms, "down").cmd, Some(sv(&["opencode"])));
}

#[test]
fn a_concurrent_load_waits_for_the_preparation_and_returns_prepared_rows() {
    use std::sync::mpsc;
    use std::time::Duration;

    let (_dir, db, store) = setup();
    let mut seed = hooked_room();
    seed[0].harnesses.push(harness(
        "c",
        "claude",
        Some("c-sid"),
        &["claude", "--session-id", "c-sid"],
        &json!({}),
    ));
    db.write_room_rows(&seed, &[], None).unwrap();
    let boot = Arc::new(BootResume::detached());
    let b_probes = Arc::new(AtomicUsize::new(0));
    let probes = Arc::new(AtomicUsize::new(0));

    let (entered_tx, entered_rx) = mpsc::channel::<()>();
    // Dropping the sender releases the blocked `recv`.
    let (release_tx, release_rx) = mpsc::channel::<()>();
    let a = {
        let (boot, db, store, probes) = (
            Arc::clone(&boot),
            Arc::clone(&db),
            Arc::clone(&store),
            Arc::clone(&probes),
        );
        std::thread::spawn(move || {
            let probe = move |_: &Harness| {
                probes.fetch_add(1, Ordering::SeqCst);
                let _ = entered_tx.send(());
                let _ = release_rx.recv();
                Ok(true)
            };
            boot.load_rooms_with(&db, &store, probe)
        })
    };
    entered_rx.recv_timeout(Duration::from_secs(10)).unwrap();

    let (done_tx, done_rx) = mpsc::channel();
    let b = {
        let (boot, db, store, b_probes) = (
            Arc::clone(&boot),
            Arc::clone(&db),
            Arc::clone(&store),
            Arc::clone(&b_probes),
        );
        std::thread::spawn(move || {
            let probe = |_: &Harness| {
                b_probes.fetch_add(1, Ordering::SeqCst);
                Ok(true)
            };
            let out = boot.load_rooms_with(&db, &store, probe);
            done_tx.send(()).unwrap();
            out
        })
    };
    assert!(
        done_rx.recv_timeout(Duration::from_millis(300)).is_err(),
        "a concurrent load returned while the preparation was still running"
    );

    drop(release_tx);
    let out_a = a.join().unwrap().unwrap();
    let out_b = b.join().unwrap().unwrap();

    for rooms in [out_a.rooms, out_b.rooms, store.rooms()] {
        assert_eq!(find(&rooms, "o").cmd, Some(sv(&["opencode"])));
        assert_eq!(
            find(&rooms, "c").cmd,
            Some(sv(&["claude", "--session-id", "c-sid"]))
        );
    }
    assert_eq!(b_probes.load(Ordering::SeqCst), 0, "B must not prepare");
    assert_eq!(probes.load(Ordering::SeqCst), 1, "one preparation");
}
