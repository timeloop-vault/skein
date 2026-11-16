//! The final check-and-apply of the daemon-executed verbs is atomic against
//! each other and never writes back a stale room (#734). The test frontend's
//! `files.dirty` handler runs in the middle of a verb, so it stands in for a
//! client or another verb landing in the window the question opens.

use std::sync::Arc;

use serde_json::{Value, json};
use skein_proto::room_mutations::RoomMutation;

use super::close_room_tests::{room_created_by, signed_off_room};
use super::open_harness_tests::open_harness_args;
use super::state::AgentApiState;
use super::tests::{agent_api_state_with_room_records, caller_for, fixture, harness, room};
use super::verbs::{self, MailContext, VerbError};
use crate::db::Room;
use crate::room_store::RoomStore;

fn stored(store: &RoomStore, id: &str) -> Room {
    store.rooms().into_iter().find(|r| r.id == id).unwrap()
}

/// Answer `files.dirty` clean, after running `before` against the store: a
/// client (or another verb) acting while the question is out.
fn clean_after(
    state: &AgentApiState,
    store: &Arc<RoomStore>,
    before: impl Fn(&RoomStore) + Send + Sync + 'static,
) {
    let store = Arc::clone(store);
    state.set_test_frontend(move |kind, _| {
        assert_eq!(kind, "files.dirty");
        before(&store);
        Ok(json!({ "dirty": [] }) as Value)
    });
}

/// A client-style whole-room update renaming `room_id`.
fn rename(store: &RoomStore, room_id: &str, name: &str) {
    let mut room = stored(store, room_id);
    room.name = name.to_owned();
    store
        .apply("client", vec![RoomMutation::UpdateRoom { room }])
        .unwrap();
}

fn close_args(harness: &str) -> verbs::CloseHarnessArgs {
    verbs::CloseHarnessArgs {
        harness: harness.to_owned(),
    }
}

#[tokio::test]
async fn close_room_keeps_a_rename_made_while_it_asked_about_unsaved_files() {
    let f = fixture();
    let (_tmp, state, store) = signed_off_room(&f, vec![harness("hf", "files", "files")]);
    clean_after(&state, &store, |s| rename(s, "r2", "renamed"));
    let caller = caller_for(&f.db, "r1", Some("h1"));
    verbs::close_room(
        &state,
        &caller,
        &verbs::CloseRoomArgs {
            room: "r2".to_owned(),
        },
        true,
    )
    .await
    .unwrap();
    let r2 = stored(&store, "r2");
    assert!(r2.archived.is_some());
    assert_eq!(r2.name, "renamed");
}

#[tokio::test]
async fn close_harness_repoint_keeps_a_rename_made_while_it_asked() {
    let f = fixture();
    let mut r2 = room_created_by(
        "r2",
        vec![
            harness("hf", "files", "files"),
            harness("h2b", "claude", "two"),
            harness("h2c", "claude", "three"),
        ],
        "r1",
    );
    r2.active_harness_id = "hf".to_owned();
    let (state, store) = agent_api_state_with_room_records(
        &f,
        vec![room("r1", vec![harness("h1", "claude", "main")]), r2],
    );
    clean_after(&state, &store, |s| rename(s, "r2", "renamed"));
    let caller = caller_for(&f.db, "r1", Some("h1"));
    verbs::close_harness(&state, &caller, &close_args("hf"), true)
        .await
        .unwrap();
    let r2 = stored(&store, "r2");
    assert_eq!(r2.name, "renamed");
    assert_eq!(r2.active_harness_id, "h2b");
    assert_eq!(r2.harnesses.len(), 2);
}

#[tokio::test]
async fn close_harness_refuses_the_last_harness_when_another_close_won_the_window() {
    let f = fixture();
    let (state, store) = agent_api_state_with_room_records(
        &f,
        vec![
            room("r1", vec![harness("h1", "claude", "main")]),
            room_created_by(
                "r2",
                vec![
                    harness("hf", "files", "files"),
                    harness("h2b", "claude", "two"),
                ],
                "r1",
            ),
        ],
    );
    // While the question for `hf` is out, the other harness is closed.
    clean_after(&state, &store, |s| {
        s.apply(
            "client",
            vec![RoomMutation::RemoveHarness {
                room_id: "r2".to_owned(),
                harness_id: "h2b".to_owned(),
            }],
        )
        .unwrap();
    });
    let caller = caller_for(&f.db, "r1", Some("h1"));
    let err = verbs::close_harness(&state, &caller, &close_args("hf"), true)
        .await
        .unwrap_err();
    assert!(
        matches!(&err, VerbError::Refused(m) if m.starts_with("last_harness:")),
        "{err:?}"
    );
    let r2 = stored(&store, "r2");
    assert_eq!(r2.harnesses.len(), 1);
    assert!(
        r2.harnesses.iter().any(|h| h.id == r2.active_harness_id),
        "the active harness is one that exists"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_closes_of_the_two_harnesses_leave_one() {
    let f = fixture();
    let (state, store) = agent_api_state_with_room_records(
        &f,
        vec![
            room("r1", vec![harness("h1", "claude", "main")]),
            room_created_by(
                "r2",
                vec![
                    harness("h2a", "claude", "one"),
                    harness("h2b", "claude", "two"),
                ],
                "r1",
            ),
        ],
    );
    let caller = caller_for(&f.db, "r1", Some("h1"));
    let (args_a, args_b) = (close_args("h2a"), close_args("h2b"));
    let (a, b) = tokio::join!(
        verbs::close_harness(&state, &caller, &args_a, true),
        verbs::close_harness(&state, &caller, &args_b, true),
    );
    let refused = [&a, &b]
        .iter()
        .filter(|r| matches!(r, Err(VerbError::Refused(m)) if m.starts_with("last_harness:")))
        .count();
    assert_eq!(
        usize::from(a.is_ok()) + usize::from(b.is_ok()),
        1,
        "{a:?} {b:?}"
    );
    assert_eq!(refused, 1, "{a:?} {b:?}");
    let r2 = stored(&store, "r2");
    assert_eq!(r2.harnesses.len(), 1);
    assert!(r2.harnesses.iter().any(|h| h.id == r2.active_harness_id));
}

fn files_args(room: &str) -> verbs::OpenHarnessArgs {
    verbs::OpenHarnessArgs {
        kind: Some("files".to_owned()),
        ..open_harness_args(room)
    }
}

#[tokio::test]
async fn sequential_opens_get_distinct_names() {
    let f = fixture();
    let (state, store) = agent_api_state_with_room_records(
        &f,
        vec![room("r1", vec![harness("h1", "claude", "main")])],
    );
    let caller = caller_for(&f.db, "r1", Some("h1"));
    let mail = MailContext::permissive();
    let a = verbs::open_harness(&state, &caller, &files_args("r1"), &mail, true)
        .await
        .unwrap();
    let b = verbs::open_harness(&state, &caller, &files_args("r1"), &mail, true)
        .await
        .unwrap();
    assert_ne!(a.name, b.name);
    assert_eq!(stored(&store, "r1").harnesses.len(), 3);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_opens_cannot_exceed_the_cap_or_share_a_name() {
    let f = fixture();
    let mut harnesses: Vec<_> = (0..6)
        .map(|i| harness(&format!("x{i}"), "claude", &format!("n{i}")))
        .collect();
    harnesses.insert(0, harness("h1", "claude", "main"));
    let (state, store) = agent_api_state_with_room_records(&f, vec![room("r1", harnesses)]);
    let caller = caller_for(&f.db, "r1", Some("h1"));
    let mail = MailContext::permissive();
    let args = files_args("r1");
    let (r1, r2, r3) = tokio::join!(
        verbs::open_harness(&state, &caller, &args, &mail, true),
        verbs::open_harness(&state, &caller, &args, &mail, true),
        verbs::open_harness(&state, &caller, &args, &mail, true),
    );
    let ok = [r1.is_ok(), r2.is_ok(), r3.is_ok()]
        .iter()
        .filter(|ok| **ok)
        .count();
    assert_eq!(ok, 1, "7 harnesses, cap 8: one open fits");
    for r in [&r1, &r2, &r3] {
        if let Err(e) = r {
            assert!(
                matches!(e, VerbError::Refused(m) if m.starts_with("room_full:")),
                "{e:?}"
            );
        }
    }
    let names: Vec<String> = stored(&store, "r1")
        .harnesses
        .iter()
        .map(|h| h.name.clone())
        .collect();
    assert_eq!(names.len(), 8);
}
