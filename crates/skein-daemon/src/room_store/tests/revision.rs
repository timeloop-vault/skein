//! The per-room revision (#756): who bumps it, when, and what it guards.

use serde_json::{Map, Value, json};
use skein_proto::room_commands::RoomCommand as C;
use skein_proto::room_mutations::{RoomChange, RoomMutation as M};

use super::{Fixture, harness, refusals, room, room_with};

fn rev(f: &Fixture, id: &str) -> u64 {
    f.stored_room(id).revision
}

fn map(v: &Value) -> Map<String, Value> {
    v.as_object().unwrap().clone()
}

fn patch_task(room_id: &str, expected: u64, task: &str) -> C {
    C::PatchRoom {
        room_id: room_id.into(),
        expected_revision: expected,
        fields: map(&json!({"task": task})),
    }
}

fn rename(room_id: &str, name: &str) -> C {
    C::RenameRoom {
        room_id: room_id.into(),
        name: name.into(),
    }
}

#[test]
fn legacy_apply_bumps_once_per_touched_room_per_call() {
    let f = Fixture::with_rooms(&[room_with("r1", &["h1"]), room("r2")]);
    let mut r1 = room("r1");
    r1.task = "a".into();
    let mut h = harness("h1");
    h.tokens = "9".into();
    f.apply(
        "a",
        vec![
            M::UpdateRoom { room: r1 },
            M::PutHarness {
                room_id: "r1".into(),
                harness: h,
            },
        ],
    );
    assert_eq!(rev(&f, "r1"), 1, "two mutations, one bump");
    assert_eq!(rev(&f, "r2"), 0, "untouched room keeps its revision");
    f.apply("a", vec![M::CreateRoom { room: room("r3") }]);
    assert_eq!(rev(&f, "r3"), 1);
    assert_eq!(rev(&f, "r1"), 1);
}

#[test]
fn execute_bumps_once_per_touched_room_per_call() {
    let f = Fixture::with_rooms(&[room("r1"), room("r2")]);
    let out = f.execute("a", vec![rename("r1", "x"), rename("r1", "y")]);
    assert_eq!(rev(&f, "r1"), 1);
    assert_eq!(rev(&f, "r2"), 0);
    f.execute("a", vec![rename("r1", "z")]);
    assert_eq!(rev(&f, "r1"), 2);
    assert_eq!(out.revisions.len(), 1);
}

#[test]
fn edit_harness_bumps_once_and_a_no_change_edit_does_not() {
    let f = Fixture::with_rooms(&[room_with("r1", &["h1", "h2"]), room_with("r2", &["h3"])]);
    assert!(
        f.store
            .edit_harness("a", "r1", "h1", |h| h.session_id = Some("s".into()))
            .unwrap()
    );
    assert_eq!(rev(&f, "r1"), 1);
    assert_eq!(rev(&f, "r2"), 0);

    // Same value again: nothing changed, nothing bumped, no new event.
    let events = f.events().len();
    assert!(
        !f.store
            .edit_harness("a", "r1", "h1", |h| h.session_id = Some("s".into()))
            .unwrap()
    );
    assert_eq!(rev(&f, "r1"), 1);
    assert_eq!(f.events().len(), events);
}

#[test]
fn edit_harnesses_bumps_each_touched_room_once() {
    let f = Fixture::with_rooms(&[
        room_with("r1", &["h1", "h2"]),
        room_with("r2", &["h3"]),
        room_with("r3", &["h4"]),
    ]);
    let n = f
        .store
        .edit_harnesses("a", |room_id, h| {
            if room_id != "r3" {
                h.status = "idle".into();
            }
        })
        .unwrap();
    assert_eq!(n, 3, "three harnesses changed");
    assert_eq!(rev(&f, "r1"), 1, "two harnesses in the room, one bump");
    assert_eq!(rev(&f, "r2"), 1);
    assert_eq!(rev(&f, "r3"), 0);
}

#[test]
fn a_command_that_changes_nothing_does_not_bump() {
    let f = Fixture::with_rooms(&[room_with("r1", &["h1"])]);
    f.execute("a", vec![rename("r1", "same")]);
    assert_eq!(rev(&f, "r1"), 1);
    let out = f.execute(
        "a",
        vec![
            rename("r1", "same"),
            C::SetHarnessAgent {
                room_id: "r1".into(),
                harness_id: "h1".into(),
                agent: None,
            },
            patch_task("r1", 1, ""),
            C::ReorderHarnesses {
                room_id: "r1".into(),
                harness_ids: vec!["h1".into()],
            },
        ],
    );
    assert!(out.refused.is_empty());
    assert_eq!(out.revisions.len(), 0);
    assert_eq!(rev(&f, "r1"), 1);
    assert_eq!(f.events().len(), 1, "only the first rename emitted");
}

#[test]
fn a_patch_that_sets_the_value_already_there_does_not_bump() {
    let f = Fixture::with_rooms(&[room("r1")]);
    let out = f.execute("a", vec![patch_task("r1", 0, "")]);
    assert_eq!(out.refused.len() + out.revisions.len(), 0);
    assert_eq!(rev(&f, "r1"), 0);
}

#[test]
fn a_legacy_update_cannot_set_or_lower_the_revision() {
    let f = Fixture::with_rooms(&[room("r1")]);
    f.execute("a", vec![rename("r1", "n1")]);
    f.execute("a", vec![rename("r1", "n2")]);
    assert_eq!(rev(&f, "r1"), 2);

    for bogus in [0, 1, 999] {
        let mut r = room("r1");
        r.task = format!("t{bogus}");
        r.revision = bogus;
        f.apply("a", vec![M::UpdateRoom { room: r }]);
    }
    // Three real updates: stored 2 + 3 bumps, whatever the payloads said.
    assert_eq!(rev(&f, "r1"), 5);
    assert_eq!(f.stored_room("r1").task, "t999");
}

#[test]
fn a_legacy_create_starts_at_revision_one_whatever_the_payload_said() {
    let f = Fixture::with_rooms(&[]);
    let mut r = room("r1");
    r.revision = 42;
    f.apply("a", vec![M::CreateRoom { room: r }]);
    assert_eq!(rev(&f, "r1"), 1);
}

#[test]
fn the_revision_is_in_the_put_event() {
    let f = Fixture::with_rooms(&[room("r1")]);
    f.execute("a", vec![rename("r1", "x")]);
    f.execute("a", vec![rename("r1", "y")]);
    let seen: Vec<u64> = f
        .events()
        .iter()
        .flat_map(|e| e.changes.iter())
        .filter_map(|c| match c {
            RoomChange::RoomPut { room } => Some(room.revision),
            _ => None,
        })
        .collect();
    assert_eq!(seen, [1, 2]);
}

#[test]
fn the_revision_survives_a_persist_and_a_reload() {
    let f = Fixture::with_rooms(&[room("r1")]);
    f.execute("a", vec![rename("r1", "x")]);
    f.execute("a", vec![patch_task("r1", 1, "t")]);
    f.store.flush().unwrap();
    assert_eq!(f.disk_room("r1").unwrap().revision, 2);

    let out = f.db.load_all().unwrap();
    assert!(out.skipped.is_empty());
    let g = Fixture::unfilled();
    g.store.fill(out.rooms).unwrap();
    assert_eq!(rev(&g, "r1"), 2);
    g.execute("a", vec![rename("r1", "y")]);
    assert_eq!(rev(&g, "r1"), 3, "counting continues after a reload");
}

#[test]
fn a_stale_patch_is_refused_and_changes_nothing_then_the_current_revision_lands() {
    let f = Fixture::with_rooms(&[room("r1")]);
    f.execute("a", vec![rename("r1", "x")]);
    assert_eq!(rev(&f, "r1"), 1);
    let before = f.events().len();

    let out = f.execute("b", vec![patch_task("r1", 0, "stale")]);
    assert_eq!(refusals(&out), [(0, "stale_revision")]);
    assert_eq!(out.revisions.len(), 0);
    let r = f.stored_room("r1");
    assert_eq!((r.task.as_str(), r.revision), ("", 1));
    assert_eq!(f.events().len(), before, "no event for a refused patch");

    let out = f.execute("b", vec![patch_task("r1", 1, "fresh")]);
    assert!(out.refused.is_empty());
    let r = f.stored_room("r1");
    assert_eq!((r.task.as_str(), r.revision), ("fresh", 2));
}

#[test]
fn a_patch_ahead_of_the_room_is_stale_too() {
    let f = Fixture::with_rooms(&[room("r1")]);
    let out = f.execute("a", vec![patch_task("r1", 5, "x")]);
    assert_eq!(refusals(&out), [(0, "stale_revision")]);
}

#[test]
fn a_stale_harness_patch_is_refused() {
    let f = Fixture::with_rooms(&[room_with("r1", &["h1"])]);
    f.execute("a", vec![rename("r1", "x")]);
    let out = f.execute(
        "b",
        vec![C::PatchHarness {
            room_id: "r1".into(),
            harness_id: "h1".into(),
            expected_revision: 0,
            fields: map(&json!({"name":"late"})),
        }],
    );
    assert_eq!(refusals(&out), [(0, "stale_revision")]);
    assert_eq!(f.stored_room("r1").harnesses[0].name, "h1");
}

#[test]
fn two_patches_in_one_batch_with_the_same_expected_revision_both_land() {
    let f = Fixture::with_rooms(&[room_with("r1", &["h1"])]);
    let out = f.execute(
        "a",
        vec![
            patch_task("r1", 0, "t"),
            C::PatchRoom {
                room_id: "r1".into(),
                expected_revision: 0,
                fields: map(&json!({"badge": 4})),
            },
            C::PatchHarness {
                room_id: "r1".into(),
                harness_id: "h1".into(),
                expected_revision: 0,
                fields: map(&json!({"name":"hh"})),
            },
        ],
    );
    assert!(out.refused.is_empty(), "{:?}", refusals(&out));
    let r = f.stored_room("r1");
    assert_eq!(
        (r.task.as_str(), r.badge, r.harnesses[0].name.as_str()),
        ("t", 4, "hh")
    );
    assert_eq!(r.revision, 1, "bumped once for the batch");
    assert_eq!(out.revisions.len(), 1);
}

/// The issue's acceptance test (#756): two clients, one room, different
/// fields. Both land; a revision-guarded patch from the one that was behind
/// is refused; a later guarded patch does not undo an intent's result.
#[test]
fn two_clients_editing_different_fields_both_land() {
    let f = Fixture::with_rooms(&[room_with("r1", &["h1"])]);
    let seen = rev(&f, "r1"); // both clients last saw this

    // The legacy whole-record path could not do this: the second stale-copy write would revert the first.
    let from_a = f.execute("client-a", vec![rename("r1", "renamed by A")]);
    let from_b = f.execute(
        "client-b",
        vec![C::SetHarnessAgent {
            room_id: "r1".into(),
            harness_id: "h1".into(),
            agent: Some("reviewer".into()),
        }],
    );
    assert!(from_a.refused.is_empty() && from_b.refused.is_empty());
    let r = f.stored_room("r1");
    assert_eq!(r.name, "renamed by A");
    assert_eq!(r.harnesses[0].agent.as_deref(), Some("reviewer"));
    assert_eq!(r.revision, seen + 2);

    // B's guarded patch still carries the revision it saw: refused.
    let out = f.execute("client-b", vec![patch_task("r1", seen, "from B")]);
    assert_eq!(refusals(&out), [(0, "stale_revision")]);
    assert_eq!(f.stored_room("r1").task, "");

    // A, now current, patches `todos`: B's intent result is still there.
    let current = rev(&f, "r1");
    let out = f.execute(
        "client-a",
        vec![C::PatchRoom {
            room_id: "r1".into(),
            expected_revision: current,
            fields: map(&json!({"todos":[{"id":"t1","text":"x"}]})),
        }],
    );
    assert!(out.refused.is_empty());
    let r = f.stored_room("r1");
    assert_eq!(r.todos.as_ref().map(Vec::len), Some(1));
    assert_eq!(r.harnesses[0].agent.as_deref(), Some("reviewer"));
    assert_eq!(r.name, "renamed by A");
    assert_eq!(r.revision, current + 1);
}
