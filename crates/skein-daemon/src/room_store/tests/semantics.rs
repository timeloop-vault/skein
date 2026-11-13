//! What a batch does to the room list, which skips it reports and which
//! events it emits.

use skein_proto::room_mutations::RoomMutation as M;

use super::{Fixture, harness, ids, room, room_with, summarize};

fn skips(o: &skein_proto::room_mutations::RoomsApplyOutcome) -> Vec<(usize, &str)> {
    o.skipped
        .iter()
        .map(|s| (s.index, s.reason.as_str()))
        .collect()
}

fn harness_ids(f: &Fixture, room_id: &str) -> Vec<String> {
    f.store
        .rooms()
        .into_iter()
        .find(|r| r.id == room_id)
        .unwrap()
        .harnesses
        .into_iter()
        .map(|h| h.id)
        .collect()
}

#[test]
fn harness_record_clones_only_the_named_harness() {
    let f = Fixture::with_rooms(&[room_with("r1", &["h1", "h2"]), room_with("r2", &["h1"])]);
    assert_eq!(f.store.harness_record("r1", "h2").unwrap().id, "h2");
    assert!(f.store.harness_record("r1", "nope").is_none());
    assert!(f.store.harness_record("nope", "h1").is_none());
}

#[test]
fn two_clients_writing_different_rooms_do_not_erase_each_other() {
    let f = Fixture::with_rooms(&[room("r1")]);
    f.apply("a", vec![M::CreateRoom { room: room("r2") }]);
    let mut r1 = room("r1");
    r1.task = "updated by b".into();
    f.apply(
        "b",
        vec![
            M::CreateRoom { room: room("r3") },
            M::UpdateRoom { room: r1 },
        ],
    );
    f.store.flush().unwrap();

    let loaded = f.db.load_all().unwrap().rooms;
    let got: Vec<(&str, &str)> = loaded
        .iter()
        .map(|r| (r.id.as_str(), r.task.as_str()))
        .collect();
    assert_eq!(got, [("r1", "updated by b"), ("r2", ""), ("r3", "")]);
}

#[test]
fn put_harness_and_update_room_on_the_same_room_both_survive() {
    for harness_first in [true, false] {
        let f = Fixture::with_rooms(&[room("r1")]);
        let put = M::PutHarness {
            room_id: "r1".into(),
            harness: harness("h2"),
        };
        // b's copy of the room is stale: it knows nothing of h2.
        let mut stale = room("r1");
        stale.task = "from b".into();
        let upd = M::UpdateRoom { room: stale };
        if harness_first {
            f.apply("a", vec![put]);
            f.apply("b", vec![upd]);
        } else {
            f.apply("b", vec![upd]);
            f.apply("a", vec![put]);
        }
        f.store.flush().unwrap();
        let r1 = f.db.load_all().unwrap().rooms.remove(0);
        assert_eq!(r1.task, "from b");
        assert_eq!(
            r1.harnesses
                .iter()
                .map(|h| h.id.as_str())
                .collect::<Vec<_>>(),
            ["h2"]
        );
    }
}

#[test]
fn removing_every_room_after_fill_is_allowed_and_persists() {
    let f = Fixture::with_rooms(&[room("r1"), room("r2")]);
    let o = f.apply(
        "a",
        vec![
            M::RemoveRoom {
                room_id: "r1".into(),
            },
            M::RemoveRoom {
                room_id: "r2".into(),
            },
        ],
    );
    assert!(o.skipped.is_empty());
    assert!(f.store.rooms().is_empty());
    assert_eq!(f.disk_ids().len(), 0);
}

#[test]
fn quarantined_row_is_not_resurrected_by_mutations() {
    let f = Fixture::unfilled();
    f.db.write_room_rows(&[room("r1"), room("r2")], &[], None)
        .unwrap();
    f.db.corrupt_room_for_test("r2");
    let out = f.db.load_all().unwrap();
    assert_eq!(out.skipped.len(), 1);
    f.store.fill(out.rooms).unwrap();

    let mut r2 = room("r2");
    r2.task = "ghost".into();
    let o = f.apply(
        "a",
        vec![
            M::UpdateRoom { room: r2 },
            M::PutHarness {
                room_id: "r2".into(),
                harness: harness("h"),
            },
        ],
    );
    assert_eq!(skips(&o), [(0, "unknown_room"), (1, "unknown_room")]);
    f.store.flush().unwrap();
    assert_eq!(f.disk_ids(), ["r1"]);
    let quarantined: i64 = f
        .raw()
        .query_row("SELECT COUNT(*) FROM sessions_quarantine", [], |r| r.get(0))
        .unwrap();
    assert_eq!(quarantined, 1);
}

#[test]
fn rows_a_batch_did_not_touch_are_byte_identical() {
    let f = Fixture::with_rooms(&[room("r1"), room("r2"), room("r3")]);
    // A blob the typed round trip would rewrite: odd spacing plus a key
    // `Room` does not know. Only a row that is never re-serialized keeps it.
    let odd = r#"{  "id":"r2","name":"room r2","task":"","status":"idle","badge":0,"harnesses":[],"activeHarnessId":"","futureField":{"x":1}  }"#;
    f.raw()
        .execute("UPDATE sessions SET data = ?1 WHERE id = 'r2'", [odd])
        .unwrap();
    let r3_before = f.disk_data("r3").unwrap();

    let mut r1 = room("r1");
    r1.name = "renamed".into();
    f.apply(
        "a",
        vec![
            M::UpdateRoom { room: r1 },
            M::CreateRoom { room: room("r4") },
        ],
    );
    f.store.flush().unwrap();

    assert_eq!(f.disk_data("r2").as_deref(), Some(odd));
    assert_eq!(f.disk_data("r3").unwrap(), r3_before);
    assert_eq!(f.disk_ids(), ["r1", "r2", "r3", "r4"]);
}

#[test]
fn every_skip_rule_reports_its_reason() {
    let f = Fixture::with_rooms(&[room_with("r1", &["h1"])]);
    let o = f.apply(
        "a",
        vec![
            M::CreateRoom { room: room("r1") },
            M::UpdateRoom { room: room("nope") },
            M::RemoveRoom {
                room_id: "nope".into(),
            },
            M::PutHarness {
                room_id: "nope".into(),
                harness: harness("h"),
            },
            M::RemoveHarness {
                room_id: "nope".into(),
                harness_id: "h1".into(),
            },
            M::RemoveHarness {
                room_id: "r1".into(),
                harness_id: "nope".into(),
            },
            M::ReorderHarnesses {
                room_id: "nope".into(),
                harness_ids: ids(&["h1"]),
            },
        ],
    );
    assert_eq!(
        skips(&o),
        [
            (0, "exists"),
            (1, "unknown_room"),
            (2, "absent"),
            (3, "unknown_room"),
            (4, "unknown_room"),
            (5, "absent"),
            (6, "unknown_room"),
        ]
    );
    // Skipped mutations changed nothing.
    assert_eq!(f.store_ids(), ["r1"]);
    assert_eq!(harness_ids(&f, "r1"), ["h1"]);
}

#[test]
fn a_batch_where_everything_was_skipped_emits_no_event_and_writes_nothing() {
    let f = Fixture::with_rooms(&[room("r1")]);
    let before = f.disk_rows();
    let o = f.apply(
        "a",
        vec![
            M::CreateRoom { room: room("r1") },
            M::RemoveRoom {
                room_id: "nope".into(),
            },
        ],
    );
    assert_eq!(o.skipped.len(), 2);
    assert!(f.events().is_empty());
    assert_eq!(f.disk_rows(), before);
}

#[test]
fn skipped_indexes_are_right_in_a_mixed_batch() {
    let f = Fixture::with_rooms(&[room("r1")]);
    let o = f.apply(
        "a",
        vec![
            M::CreateRoom { room: room("r2") }, // 0 ok
            M::CreateRoom { room: room("r2") }, // 1 exists
            M::RemoveRoom {
                room_id: "r2".into(),
            }, // 2 ok
            M::RemoveRoom {
                room_id: "r2".into(),
            }, // 3 absent
            M::UpdateRoom { room: room("r1") }, // 4 ok
            M::UpdateRoom { room: room("r2") }, // 5 unknown_room
        ],
    );
    assert_eq!(
        skips(&o),
        [(1, "exists"), (3, "absent"), (5, "unknown_room")]
    );
    assert_eq!(f.store_ids(), ["r1"]);
    assert_eq!(f.events().len(), 1);
}

#[test]
fn update_room_keeps_stored_harnesses_whatever_the_payload_carries() {
    let f = Fixture::with_rooms(&[room_with("r1", &["h1", "h2"])]);
    let mut payload = room_with("r1", &["other"]);
    payload.task = "new task".into();
    f.apply("a", vec![M::UpdateRoom { room: payload }]);
    let r1 = f.store.rooms().remove(0);
    assert_eq!(r1.task, "new task");
    assert_eq!(harness_ids(&f, "r1"), ["h1", "h2"]);
    // The event carries the final state, stored harnesses included.
    let ev = f.events().remove(0);
    let skein_proto::room_mutations::RoomChange::RoomPut { room: put } = &ev.changes[0] else {
        panic!("expected roomPut");
    };
    assert_eq!(put.harnesses.len(), 2);

    // And an empty payload list does not wipe them either.
    f.apply("a", vec![M::UpdateRoom { room: room("r1") }]);
    assert_eq!(harness_ids(&f, "r1"), ["h1", "h2"]);
}

#[test]
fn put_harness_replaces_by_id_or_appends() {
    let f = Fixture::with_rooms(&[room_with("r1", &["h1", "h2"])]);
    let mut h1 = harness("h1");
    h1.tokens = "99".into();
    f.apply(
        "a",
        vec![
            M::PutHarness {
                room_id: "r1".into(),
                harness: h1,
            },
            M::PutHarness {
                room_id: "r1".into(),
                harness: harness("h3"),
            },
        ],
    );
    assert_eq!(harness_ids(&f, "r1"), ["h1", "h2", "h3"]);
    assert_eq!(f.store.rooms()[0].harnesses[0].tokens, "99");
}

#[test]
fn reorder_rooms_listed_first_unknown_ignored_unlisted_keep_order() {
    let f = Fixture::with_rooms(&[room("a"), room("b"), room("c"), room("d"), room("e")]);
    f.apply(
        "x",
        vec![M::ReorderRooms {
            room_ids: ids(&["d", "ghost", "b"]),
        }],
    );
    assert_eq!(f.store_ids(), ["d", "b", "a", "c", "e"]);
    let (_, changes) = f.only_event();
    assert_eq!(changes, ["order:d,b,a,c,e"]);
    // Order reached sqlite (structural).
    assert_eq!(f.disk_ids(), ["d", "b", "a", "c", "e"]);
}

#[test]
fn reorder_harnesses_listed_first_unknown_ignored_unlisted_keep_order() {
    let f = Fixture::with_rooms(&[room_with("r1", &["a", "b", "c", "d"]), room("r2")]);
    f.apply(
        "x",
        vec![M::ReorderHarnesses {
            room_id: "r1".into(),
            harness_ids: ids(&["c", "ghost", "a"]),
        }],
    );
    assert_eq!(harness_ids(&f, "r1"), ["c", "a", "b", "d"]);
    // Only a roomPut: harness order is not room order.
    let (_, changes) = f.only_event();
    assert_eq!(changes, ["put:r1"]);
    assert_eq!(
        f.disk_room("r1").unwrap().harnesses.len(),
        4,
        "structural, so persisted"
    );
    assert_eq!(
        f.disk_room("r1")
            .unwrap()
            .harnesses
            .iter()
            .map(|h| h.id.clone())
            .collect::<Vec<_>>(),
        ["c", "a", "b", "d"]
    );
}

#[test]
fn one_event_per_batch_echoes_origin_with_deduped_final_state() {
    let f = Fixture::with_rooms(&[room("r1"), room("r2")]);
    let mut r1 = room("r1");
    r1.task = "first".into();
    let mut r1b = room("r1");
    r1b.task = "second".into();
    f.apply(
        "client-7",
        vec![
            M::UpdateRoom { room: r1 },
            M::PutHarness {
                room_id: "r1".into(),
                harness: harness("h"),
            },
            M::UpdateRoom { room: r1b },
        ],
    );
    let evs = f.events();
    assert_eq!(evs.len(), 1);
    assert_eq!(evs[0].origin, "client-7");
    // One roomPut for r1 (deduped), carrying the FINAL state, no order.
    assert_eq!(
        evs[0].changes.iter().map(summarize).collect::<Vec<_>>(),
        ["put:r1"]
    );
    let skein_proto::room_mutations::RoomChange::RoomPut { room } = &evs[0].changes[0] else {
        panic!()
    };
    assert_eq!(room.task, "second");
    assert_eq!(room.harnesses.len(), 1);
}

#[test]
fn roomsorder_only_when_a_room_is_created_removed_or_reordered() {
    let f = Fixture::with_rooms(&[room("r1"), room("r2")]);

    // update / harness edits: no order change.
    f.apply("a", vec![M::UpdateRoom { room: room("r1") }]);
    f.apply(
        "a",
        vec![M::PutHarness {
            room_id: "r1".into(),
            harness: harness("h"),
        }],
    );
    f.apply(
        "a",
        vec![M::RemoveHarness {
            room_id: "r1".into(),
            harness_id: "h".into(),
        }],
    );
    for ev in f.events() {
        assert!(
            ev.changes
                .iter()
                .all(|c| !summarize(c).starts_with("order:")),
            "unexpected order change"
        );
    }

    let shapes = |ms: Vec<M>| {
        let f2 = Fixture::with_rooms(&[room("r1"), room("r2")]);
        f2.apply("a", ms);
        f2.only_event().1
    };
    assert_eq!(
        shapes(vec![M::CreateRoom { room: room("r3") }]),
        ["put:r3", "order:r1,r2,r3"]
    );
    assert_eq!(
        shapes(vec![M::RemoveRoom {
            room_id: "r1".into()
        }]),
        ["removed:r1", "order:r2"]
    );
    assert_eq!(
        shapes(vec![M::ReorderRooms {
            room_ids: ids(&["r2"])
        }]),
        ["order:r2,r1"]
    );
}

#[test]
fn created_then_removed_in_one_batch_ends_absent() {
    let f = Fixture::with_rooms(&[room("r1")]);
    let o = f.apply(
        "a",
        vec![
            M::CreateRoom { room: room("tmp") },
            M::RemoveRoom {
                room_id: "tmp".into(),
            },
        ],
    );
    assert!(o.skipped.is_empty());
    assert_eq!(f.store_ids(), ["r1"]);
    let (_, changes) = f.only_event();
    assert!(!changes.contains(&"put:tmp".to_owned()));
    assert_eq!(f.disk_ids(), ["r1"]);
}

#[test]
fn removed_then_recreated_in_one_batch_ends_present() {
    let f = Fixture::with_rooms(&[room("r1"), room("r2")]);
    let mut again = room("r1");
    again.task = "reborn".into();
    f.apply(
        "a",
        vec![
            M::RemoveRoom {
                room_id: "r1".into(),
            },
            M::CreateRoom { room: again },
        ],
    );
    assert_eq!(f.store_ids(), ["r2", "r1"]);
    let (_, changes) = f.only_event();
    assert!(changes.contains(&"put:r1".to_owned()));
    assert!(!changes.contains(&"removed:r1".to_owned()));
    // Persisted immediately (structural) and not deleted by its own upsert.
    assert_eq!(f.disk_room("r1").unwrap().task, "reborn");
    assert_eq!(f.disk_ids(), ["r2", "r1"]);
}

#[test]
fn removing_a_harness_from_a_room_emits_a_room_put() {
    let f = Fixture::with_rooms(&[room_with("r1", &["h1", "h2"])]);
    f.apply(
        "a",
        vec![M::RemoveHarness {
            room_id: "r1".into(),
            harness_id: "h1".into(),
        }],
    );
    assert_eq!(harness_ids(&f, "r1"), ["h2"]);
    assert_eq!(f.only_event().1, ["put:r1"]);
}
