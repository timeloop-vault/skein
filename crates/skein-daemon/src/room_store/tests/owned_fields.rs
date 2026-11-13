//! Daemon-owned harness fields (#755): `edit_harness(es)` writes them in
//! place, and a client's whole-record put cannot change them.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use skein_proto::room_mutations::{RoomChange, RoomMutation as M};
use skein_proto::rooms::{Harness, OpencodeVersion};

use super::{Fixture, harness, room, room_with};

const ERR_NOT_READY: &str = "not ready";

fn version(major: u32) -> OpencodeVersion {
    OpencodeVersion {
        major,
        version: format!("{major}.0.0"),
    }
}

fn put(room_id: &str, harness: Harness) -> Vec<M> {
    vec![M::PutHarness {
        room_id: room_id.into(),
        harness,
    }]
}

fn stored(f: &Fixture, room_id: &str, harness_id: &str) -> Harness {
    f.store
        .rooms()
        .into_iter()
        .find(|r| r.id == room_id)
        .unwrap()
        .harnesses
        .into_iter()
        .find(|h| h.id == harness_id)
        .unwrap()
}

fn set_session(h: &mut Harness) {
    h.session_id = Some("s1".into());
}

#[test]
fn daemon_edit_then_stale_client_put_keeps_daemon_fields() {
    let f = Fixture::with_rooms(&[room_with("r1", &["h1"])]);
    let mut stale = stored(&f, "r1", "h1");
    assert_eq!(stale.session_id, None);

    let changed = f
        .store
        .edit_harness("daemon", "r1", "h1", |h| {
            h.session_id = Some("s1".into());
            h.opencode_version = Some(version(1));
        })
        .unwrap();
    assert!(changed);

    stale.name = "renamed".into();
    f.apply("client", put("r1", stale));

    let h = stored(&f, "r1", "h1");
    assert_eq!(h.name, "renamed");
    assert_eq!(h.session_id.as_deref(), Some("s1"));
    assert_eq!(h.opencode_version, Some(version(1)));
}

#[test]
fn client_put_then_daemon_edit_keeps_both() {
    let f = Fixture::with_rooms(&[room_with("r1", &["h1"])]);
    let mut h = stored(&f, "r1", "h1");
    h.name = "renamed".into();
    h.agent = Some("planner".into());
    f.apply("client", put("r1", h));

    assert!(
        f.store
            .edit_harness("daemon", "r1", "h1", set_session)
            .unwrap()
    );

    let h = stored(&f, "r1", "h1");
    assert_eq!(h.name, "renamed");
    assert_eq!(h.agent.as_deref(), Some("planner"));
    assert_eq!(h.session_id.as_deref(), Some("s1"));
}

#[test]
fn client_put_cannot_clear_or_change_daemon_fields() {
    let f = Fixture::with_rooms(&[room_with("r1", &["h1"])]);
    f.store
        .edit_harness("daemon", "r1", "h1", |h| {
            h.session_id = Some("s1".into());
            h.opencode_version = Some(version(1));
        })
        .unwrap();

    // Clearing.
    let mut h = stored(&f, "r1", "h1");
    h.session_id = None;
    h.opencode_version = None;
    h.tokens = "7".into();
    f.apply("client", put("r1", h));
    let h = stored(&f, "r1", "h1");
    assert_eq!(h.session_id.as_deref(), Some("s1"));
    assert_eq!(h.opencode_version, Some(version(1)));
    assert_eq!(h.tokens, "7", "the rest of the put still lands");

    // Changing.
    let mut h = stored(&f, "r1", "h1");
    h.session_id = Some("other".into());
    h.opencode_version = Some(version(2));
    f.apply("client", put("r1", h));
    let h = stored(&f, "r1", "h1");
    assert_eq!(h.session_id.as_deref(), Some("s1"));
    assert_eq!(h.opencode_version, Some(version(1)));
}

#[test]
fn new_harness_and_new_room_keep_what_they_carry() {
    let f = Fixture::with_rooms(&[room_with("r1", &["h1"])]);

    let mut h2 = harness("h2");
    h2.session_id = Some("s2".into());
    h2.opencode_version = Some(version(3));
    f.apply("client", put("r1", h2));
    let h2 = stored(&f, "r1", "h2");
    assert_eq!(h2.session_id.as_deref(), Some("s2"));
    assert_eq!(h2.opencode_version, Some(version(3)));

    let mut r2 = room_with("r2", &["h3"]);
    r2.harnesses[0].session_id = Some("s3".into());
    r2.harnesses[0].opencode_version = Some(version(4));
    f.apply("client", vec![M::CreateRoom { room: r2 }]);
    let h3 = stored(&f, "r2", "h3");
    assert_eq!(h3.session_id.as_deref(), Some("s3"));
    assert_eq!(h3.opencode_version, Some(version(4)));
}

#[test]
fn edit_emits_one_event_with_the_origin_and_edited_room() {
    let f = Fixture::with_rooms(&[room_with("r1", &["h1"]), room_with("r2", &["h2"])]);
    assert!(
        f.store
            .edit_harness("daemon", "r1", "h1", set_session)
            .unwrap()
    );

    let evs = f.events();
    assert_eq!(evs.len(), 1);
    assert_eq!(evs[0].origin, "daemon");
    assert_eq!(evs[0].changes.len(), 1);
    match &evs[0].changes[0] {
        RoomChange::RoomPut { room } => {
            assert_eq!(room.id, "r1");
            assert_eq!(room.harnesses[0].session_id.as_deref(), Some("s1"));
        }
        other => panic!("expected RoomPut, got {other:?}"),
    }
}

#[test]
fn unknown_targets_and_no_op_edits_return_false_and_emit_nothing() {
    let f = Fixture::with_rooms(&[room_with("r1", &["h1"])]);
    let ran = AtomicUsize::new(0);

    assert!(
        !f.store
            .edit_harness("d", "nope", "h1", |_| {
                ran.fetch_add(1, Ordering::SeqCst);
            })
            .unwrap()
    );
    assert!(
        !f.store
            .edit_harness("d", "r1", "nope", |_| {
                ran.fetch_add(1, Ordering::SeqCst);
            })
            .unwrap()
    );
    assert_eq!(ran.load(Ordering::SeqCst), 0, "closure never ran");
    assert!(!f.store.edit_harness("d", "r1", "h1", |_| {}).unwrap());
    assert!(
        !f.store
            .edit_harness("d", "r1", "h1", |h| h.name = "h1".into())
            .unwrap(),
        "writing the same value is a no-op"
    );
    assert!(f.events().is_empty());
}

#[test]
fn session_id_edit_is_on_disk_without_a_flush() {
    let f = Fixture::with_rooms(&[room_with("r1", &["h1"])]);
    f.store.edit_harness("d", "r1", "h1", set_session).unwrap();
    let on_disk = f.disk_room("r1").unwrap();
    assert_eq!(on_disk.harnesses[0].session_id.as_deref(), Some("s1"));
}

#[test]
fn non_structural_edit_waits_for_flush() {
    let f = Fixture::with_rooms(&[room_with("r1", &["h1"])]);
    assert!(
        f.store
            .edit_harness("d", "r1", "h1", |h| h.tokens = "99".into())
            .unwrap()
    );
    assert_eq!(f.disk_room("r1").unwrap().harnesses[0].tokens, "0");
    f.store.flush().unwrap();
    assert_eq!(f.disk_room("r1").unwrap().harnesses[0].tokens, "99");
}

#[test]
fn observers_run_after_a_change_and_not_after_a_no_op() {
    let f = Fixture::with_rooms(&[room_with("r1", &["h1"])]);
    let runs = Arc::new(AtomicUsize::new(0));
    let seen = Arc::clone(&runs);
    f.store.observe(move |_| {
        seen.fetch_add(1, Ordering::SeqCst);
    });

    f.store.edit_harness("d", "r1", "h1", |_| {}).unwrap();
    f.store.edit_harnesses("d", |_, _| {}).unwrap();
    assert_eq!(runs.load(Ordering::SeqCst), 0);

    f.store.edit_harness("d", "r1", "h1", set_session).unwrap();
    assert_eq!(runs.load(Ordering::SeqCst), 1);
    f.store
        .edit_harnesses("d", |_, h| h.tokens = "5".into())
        .unwrap();
    assert_eq!(runs.load(Ordering::SeqCst), 2);
}

#[test]
fn edit_harnesses_visits_all_and_batches_rooms_into_one_event() {
    let f = Fixture::with_rooms(&[
        room_with("r1", &["h1", "h2"]),
        room_with("r2", &["h3"]),
        room("r3"),
    ]);
    let mut visited = Vec::new();
    let n = f
        .store
        .edit_harnesses("daemon", |room_id, h| {
            visited.push(format!("{room_id}/{}", h.id));
            if h.id != "h2" {
                h.session_id = Some(format!("s-{}", h.id));
            }
        })
        .unwrap();
    assert_eq!(visited, ["r1/h1", "r1/h2", "r2/h3"]);
    assert_eq!(n, 2, "h2 was left alone");

    let (origin, changes) = f.only_event();
    assert_eq!(origin, "daemon");
    assert_eq!(changes, ["put:r1", "put:r2"]);
    assert_eq!(stored(&f, "r1", "h1").session_id.as_deref(), Some("s-h1"));
    assert_eq!(stored(&f, "r1", "h2").session_id, None);
    assert_eq!(stored(&f, "r2", "h3").session_id.as_deref(), Some("s-h3"));
}

#[test]
fn edits_before_a_load_are_refused() {
    let f = Fixture::unfilled();
    let err = f
        .store
        .edit_harness("d", "r1", "h1", set_session)
        .unwrap_err();
    assert!(err.contains(ERR_NOT_READY), "{err}");
    let err = f.store.edit_harnesses("d", |_, _| {}).unwrap_err();
    assert!(err.contains(ERR_NOT_READY), "{err}");
    assert!(f.events().is_empty());
}

/// The id is the record's identity; an edit cannot rename it. Test builds
/// trip the `debug_assert`; release builds restore the id (not covered here).
#[test]
#[should_panic(expected = "edit must not change the harness id")]
fn edit_that_changes_the_id_is_refused() {
    let f = Fixture::with_rooms(&[room_with("r1", &["h1"])]);
    let _ = f
        .store
        .edit_harness("d", "r1", "h1", |h| h.id = "renamed".into());
}
