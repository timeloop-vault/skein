//! The event seq (#757): changes carry a rising seq, and a snapshot plus the
//! events after its seq reproduces the store.

use std::sync::Arc;

use skein_proto::Replay;
use skein_proto::room_commands::RoomCommand as C;
use skein_proto::room_mutations::{
    ROOMS_CHANGED_EVENT, RoomChange, RoomMutation as M, RoomsChanged,
};
use skein_proto::rooms::Room;

use super::{Fixture, RecordingSink, json_of, room, room_with};
use crate::db::test_support::fresh_db;
use crate::event_seq::SeqEventSink;
use crate::event_sink::EventSink;
use crate::room_store::RoomStore;

/// Every recorded rooms-changed event, in delivery order (other events skipped).
fn changed(f: &Fixture) -> Vec<RoomsChanged> {
    f.sink
        .0
        .lock()
        .unwrap()
        .iter()
        .filter(|(name, _)| name == ROOMS_CHANGED_EVENT)
        .map(|(_, payload)| serde_json::from_str(payload).unwrap())
        .collect()
}

/// What the webview's `applyRoomChanges` does, without the identity
/// preservation and the client-only harness keys: put replaces or appends,
/// remove filters, order lists the named rooms first and keeps the rest.
fn apply_changes(rooms: &mut Vec<Room>, changes: &[RoomChange]) {
    for c in changes {
        match c {
            RoomChange::RoomPut { room } => match rooms.iter().position(|r| r.id == room.id) {
                Some(i) => rooms[i] = room.clone(),
                None => rooms.push(room.clone()),
            },
            RoomChange::RoomRemoved { room_id } => rooms.retain(|r| r.id != *room_id),
            RoomChange::RoomsOrder { room_ids } => {
                let mut out: Vec<Room> = Vec::new();
                for id in room_ids {
                    if let Some(r) = rooms.iter().find(|r| r.id == *id)
                        && !out.iter().any(|o| o.id == *id)
                    {
                        out.push(r.clone());
                    }
                }
                for r in rooms.iter() {
                    if !out.iter().any(|o| o.id == r.id) {
                        out.push(r.clone());
                    }
                }
                *rooms = out;
            }
        }
    }
}

fn rename(id: &str, name: &str) -> C {
    C::RenameRoom {
        room_id: id.into(),
        name: name.into(),
    }
}

#[test]
fn changes_carry_a_strictly_rising_seq_across_every_write_path() {
    let f = Fixture::with_rooms(&[room_with("r1", &["h1"]), room("r2")]);
    f.apply("a", vec![M::CreateRoom { room: room("r3") }]);
    f.seq.emit_json("unrelated", "{}".into()).unwrap();
    f.execute("b", vec![rename("r1", "one")]);
    f.seq.emit_json("unrelated", "{}".into()).unwrap();
    f.seq.emit_json("unrelated", "{}".into()).unwrap();
    f.store
        .edit_harness("c", "r1", "h1", |h| h.name = "renamed".into())
        .unwrap();
    f.apply(
        "d",
        vec![M::RemoveRoom {
            room_id: "r2".into(),
        }],
    );

    let seqs: Vec<u64> = changed(&f).iter().map(|e| e.seq).collect();
    assert_eq!(seqs, [1, 3, 6, 7]);
    assert_eq!(f.seq.current(), 7);
}

#[test]
fn a_snapshot_plus_the_later_events_reproduces_the_store() {
    let f = Fixture::with_rooms(&[room_with("r1", &["h1"]), room("r2")]);
    f.apply("a", vec![M::CreateRoom { room: room("r3") }]);
    f.execute("b", vec![rename("r2", "two")]);

    let (mut client, at) = f.store.snapshot();
    assert_eq!(json_of(&client), json_of(&f.store.rooms()));
    assert_eq!(at, 2);
    f.seq.emit_json("unrelated", "{}".into()).unwrap();

    f.apply("c", vec![M::CreateRoom { room: room("r4") }]);
    f.apply(
        "c",
        vec![M::ReorderRooms {
            room_ids: vec!["r4".into(), "r1".into()],
        }],
    );
    f.store
        .edit_harness("d", "r1", "h1", |h| h.name = "x".into())
        .unwrap();
    f.apply(
        "e",
        vec![M::RemoveRoom {
            room_id: "r2".into(),
        }],
    );
    assert_ne!(json_of(&client), json_of(&f.store.rooms()));

    // Events at or below the snapshot seq are not needed, and are skipped.
    for ev in changed(&f).iter().filter(|e| e.seq > at) {
        apply_changes(&mut client, &ev.changes);
    }
    assert_eq!(json_of(&client), json_of(&f.store.rooms()));
}

#[test]
fn a_load_reports_the_seq_the_store_is_at() {
    let f = Fixture::unfilled();
    f.db.write_room_rows(&[room("r1")], &[], None).unwrap();
    f.seq.emit_json("unrelated", "{}".into()).unwrap();
    f.seq.emit_json("unrelated", "{}".into()).unwrap();
    let out = crate::rooms::load_rooms(&f.db, &f.store).unwrap();
    assert_eq!(out.seq, 2);
    assert_eq!(f.store.snapshot().1, out.seq);

    f.apply("a", vec![M::CreateRoom { room: room("r2") }]);
    let again = crate::rooms::load_rooms(&f.db, &f.store).unwrap();
    assert_eq!(again.seq, 3);
    assert_eq!(f.store.snapshot().1, again.seq);
}

#[test]
fn an_old_cursor_gaps_after_the_log_wraps_and_a_fresh_snapshot_recovers() {
    let (_dir, db) = fresh_db();
    let sink = Arc::new(RecordingSink::default());
    let seq = SeqEventSink::with_caps(sink, 3, usize::MAX);
    let store = RoomStore::new(Arc::new(db), seq.clone());
    store.fill(vec![room("r1")]).unwrap();
    let instance = seq.instance_id().to_owned();
    let (_, at) = store.snapshot();
    assert_eq!(at, 0);

    for i in 0..6 {
        store
            .apply(
                "a",
                vec![M::CreateRoom {
                    room: room(&format!("n{i}")),
                }],
            )
            .unwrap();
    }
    assert!(matches!(
        seq.subscribe(&instance, at),
        Replay::Gap { seq: 6 }
    ));

    let (mut client, at) = store.snapshot();
    assert_eq!(at, 6);
    store
        .apply("a", vec![M::CreateRoom { room: room("late") }])
        .unwrap();
    let Replay::Events { events, seq: head } = seq.subscribe(&instance, at) else {
        panic!("expected a replay from the fresh snapshot");
    };
    assert_eq!(head, 7);
    for e in events {
        assert_eq!(e.event, ROOMS_CHANGED_EVENT);
        let ev: RoomsChanged = serde_json::from_str(&e.payload).unwrap();
        apply_changes(&mut client, &ev.changes);
    }
    assert_eq!(json_of(&client), json_of(&store.rooms()));
}
