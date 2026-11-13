//! When and how a batch reaches sqlite.

use std::time::{Duration, Instant};

use serde_json::{Value, json};
use skein_proto::room_mutations::RoomMutation as M;
use skein_proto::rooms::{Harness, Room};

use super::{Fixture, RoomStore, harness, json_of, room, room_with};
use crate::room_store::{ROOMS_PERSIST_DELAY, ROOMS_PERSIST_MAX_WAIT, persist_deadline};

#[test]
fn apply_before_fill_errs_and_writes_nothing() {
    let f = Fixture::unfilled();
    let err = f
        .store
        .apply("a", vec![M::CreateRoom { room: room("r1") }])
        .unwrap_err();
    assert!(err.contains("not ready"), "{err}");
    f.store.flush().unwrap();
    assert_eq!(f.disk_ids().len(), 0);
    assert!(f.events().is_empty());
    assert!(f.store.rooms().is_empty());
}

#[test]
fn structural_batch_is_in_sqlite_when_apply_returns() {
    let f = Fixture::with_rooms(&[room("r1")]);
    f.apply("a", vec![M::CreateRoom { room: room("r2") }]);
    assert_eq!(f.disk_ids(), ["r1", "r2"], "no flush needed");
}

#[test]
fn non_structural_change_waits_for_flush() {
    let f = Fixture::with_rooms(&[room_with("r1", &["h1"])]);
    let mut r1 = room("r1");
    r1.task = "debounced".into();
    f.apply("a", vec![M::UpdateRoom { room: r1 }]);
    let mut h = harness("h1");
    h.tokens = "42".into();
    f.apply(
        "a",
        vec![M::PutHarness {
            room_id: "r1".into(),
            harness: h,
        }],
    );

    // The fixture's debounce is minutes long, so nothing is written yet.
    let on_disk = f.disk_room("r1").unwrap();
    assert_eq!(on_disk.task, "");
    assert_eq!(on_disk.harnesses[0].tokens, "0");

    f.store.flush().unwrap();
    let on_disk = f.disk_room("r1").unwrap();
    assert_eq!(on_disk.task, "debounced");
    assert_eq!(on_disk.harnesses[0].tokens, "42");
}

#[test]
fn non_structural_change_reaches_sqlite_by_itself() {
    let ms = Duration::from_millis;
    let f = Fixture::with_rooms_and_delays(&[room("r1")], ms(30), ms(100));
    let mut r1 = room("r1");
    r1.task = "eventually".into();
    f.apply("a", vec![M::UpdateRoom { room: r1 }]);
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if f.disk_room("r1").unwrap().task == "eventually" {
            return;
        }
        assert!(Instant::now() < deadline, "debounced write never arrived");
        std::thread::sleep(Duration::from_millis(25));
    }
}

fn patched(base: &impl serde::Serialize, key: &str, value: Value) -> Value {
    let mut v = json_of(base);
    v[key] = value;
    v
}

#[test]
fn room_fields_flip_the_structural_classification() {
    let structural = [
        ("name", json!("renamed")),
        ("archived", json!(5)),
        ("retired", json!(6)),
        ("cwd", json!("/x")),
        ("repoRoot", json!("/y")),
        ("branch", json!("b")),
        ("createdBy", json!({"roomId": "c", "harnessId": "h"})),
        ("closedBy", json!({"roomId": "c", "at": 1})),
    ];
    let debounced = [
        ("task", json!("t")),
        ("status", json!("busy")),
        ("badge", json!(3)),
        ("activeHarnessId", json!("h1")),
        ("attention", json!(true)),
        ("repo", json!("o/r")),
        ("todos", json!([{"a": 1}])),
        ("repoIdentity", json!({"rootCommits": ["x"]})),
    ];
    for (expect_structural, table) in [(true, &structural[..]), (false, &debounced[..])] {
        for (key, value) in table {
            let f = Fixture::with_rooms(&[room_with("r1", &["h1"])]);
            let next: Room =
                serde_json::from_value(patched(&room("r1"), key, value.clone())).unwrap();
            f.apply("a", vec![M::UpdateRoom { room: next }]);
            let disk = json_of(&f.disk_room("r1").unwrap());
            let changed = disk.get(key) == Some(value);
            assert_eq!(
                changed, expect_structural,
                "room field {key}: structural={expect_structural}"
            );
        }
    }
}

#[test]
fn harness_fields_flip_the_structural_classification() {
    let structural = [
        ("kind", json!("opencode")),
        ("name", json!("n2")),
        ("agent", json!("ag")),
        ("cwd", json!("/h")),
        (
            "remote",
            json!({"host": "h", "tool": "claude", "session": "s"}),
        ),
        ("sessionId", json!("sid")),
        ("launch", json!({"mode": "shell", "argv": ["bash"]})),
        ("createdBy", json!({"roomId": "c"})),
    ];
    let debounced = [
        ("status", json!("idle")),
        ("model", json!("m")),
        ("tokens", json!("9")),
        ("live", json!(true)),
        ("cmd", json!(["x"])),
        ("designEntry", json!("a.html")),
        ("designDevice", json!({"w": 1})),
        ("pendingNotifications", json!(2)),
        ("shellClaim", json!({"sessionId": "s"})),
        ("envProfile", json!("p")),
        ("hermesProfile", json!("q")),
        ("opencodeVersion", json!({"major": 2, "version": "2.0"})),
    ];
    for (expect_structural, table) in [(true, &structural[..]), (false, &debounced[..])] {
        for (key, value) in table {
            let f = Fixture::with_rooms(&[room_with("r1", &["h1"])]);
            let next: Harness =
                serde_json::from_value(patched(&harness("h1"), key, value.clone())).unwrap();
            f.apply(
                "a",
                vec![M::PutHarness {
                    room_id: "r1".into(),
                    harness: next,
                }],
            );
            let disk = json_of(&f.disk_room("r1").unwrap().harnesses[0]);
            let changed = disk.get(key) == Some(value);
            assert_eq!(
                changed, expect_structural,
                "harness field {key}: structural={expect_structural}"
            );
        }
    }
}

#[test]
fn adding_a_harness_is_structural() {
    let f = Fixture::with_rooms(&[room("r1")]);
    f.apply(
        "a",
        vec![M::PutHarness {
            room_id: "r1".into(),
            harness: harness("h1"),
        }],
    );
    assert_eq!(f.disk_room("r1").unwrap().harnesses.len(), 1);
}

#[test]
fn persist_deadline_table() {
    let t0 = Instant::now();
    let ms = Duration::from_millis;
    // (first unpersisted, last change) offsets -> expected deadline offset
    let cases = [
        (0, 0, 500),
        (0, 300, 800),
        (0, 1000, 1500),
        (0, 1800, 2000), // capped by the max wait
        (0, 5000, 2000),
        (400, 400, 900),
        (400, 2000, 2400),
    ];
    for (first, last, want) in cases {
        assert_eq!(
            persist_deadline(t0 + ms(first), t0 + ms(last)),
            t0 + ms(want),
            "first={first} last={last}"
        );
    }
    assert_eq!(ROOMS_PERSIST_DELAY, ms(500));
    assert_eq!(ROOMS_PERSIST_MAX_WAIT, ms(2000));
}

#[test]
fn created_then_removed_before_a_persist_writes_no_row() {
    let f = Fixture::with_rooms(&[room("r1")]);
    // Non-structural first so the pending state is genuinely unpersisted.
    let mut r1 = room("r1");
    r1.task = "dirty".into();
    f.apply("a", vec![M::UpdateRoom { room: r1 }]);
    f.apply(
        "a",
        vec![
            M::CreateRoom { room: room("tmp") },
            M::RemoveRoom {
                room_id: "tmp".into(),
            },
        ],
    );
    f.store.flush().unwrap();
    assert_eq!(f.disk_ids(), ["r1"]);
    assert_eq!(f.disk_room("r1").unwrap().task, "dirty");
}

#[test]
fn fill_on_a_dirty_store_flushes_first() {
    let f = Fixture::with_rooms(&[room("r1")]);
    let mut r1 = room("r1");
    r1.task = "pending".into();
    f.apply("a", vec![M::UpdateRoom { room: r1 }]);
    assert_eq!(f.disk_room("r1").unwrap().task, "");

    f.store.fill(vec![room("r1"), room("r9")]).unwrap();
    assert_eq!(
        f.disk_room("r1").unwrap().task,
        "pending",
        "flushed by fill"
    );
    assert_eq!(f.store_ids(), ["r1", "r9"], "list replaced");

    // The replaced list starts clean: nothing stale is written later.
    f.store.flush().unwrap();
    assert_eq!(f.disk_ids(), ["r1"]);
}

#[test]
fn dropping_the_store_persists_pending_changes() {
    let Fixture {
        _dir,
        path,
        db,
        sink,
        store,
    } = Fixture::with_rooms(&[room("r1")]);
    let mut r1 = room("r1");
    r1.task = "on drop".into();
    store.apply("a", vec![M::UpdateRoom { room: r1 }]).unwrap();
    drop(store);
    drop(sink);
    let conn = rusqlite::Connection::open(&path).unwrap();
    let data: String = conn
        .query_row("SELECT data FROM sessions WHERE id = 'r1'", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert!(data.contains("on drop"), "{data}");
    drop(db);
}

#[test]
fn a_store_built_twice_on_one_db_sees_what_the_first_wrote() {
    let f = Fixture::with_rooms(&[room("r1")]);
    f.apply("a", vec![M::CreateRoom { room: room("r2") }]);
    let second = RoomStore::new(std::sync::Arc::clone(&f.db), f.sink.clone());
    second.fill(f.db.load_all().unwrap().rooms).unwrap();
    assert_eq!(
        second.rooms().into_iter().map(|r| r.id).collect::<Vec<_>>(),
        ["r1", "r2"]
    );
}
