//! `close_room` (#411): guards, sign-off requirements and the daemon-side archive (#734).

use serde_json::{Value, json};
use tempfile::TempDir;

use super::auth::{self, AuthError};
use super::review_tests::{commit_file, git_repo_with_commit};
use super::room_listing_tests::created_by;
use super::state::AgentApiState;
use super::tests::{
    agent_api_state, agent_api_state_with_room_records, caller_for, fixture, harness, room, save,
};
use super::verbs::{self, VerbError};
use crate::db::{Harness, Room};
use crate::room_store::RoomStore;
use skein_proto::room_mutations::RoomMutation;

// ── closing a room (issue #411) ───────────────────────────────────────
//
// `close_room` is exercised as a plain function (`verbs::close_room`),
// the same way `create_room`'s own guards are — dispatch through MCP is
// already covered above (`close_room_is_dispatched_through_mcp_not_
// refused_by_name`, `archive_room_is_refused_by_name_and_points_at_
// close_room`).

fn close_room_args(room: &str) -> verbs::CloseRoomArgs {
    verbs::CloseRoomArgs {
        room: room.to_owned(),
    }
}

/// A room `create_room` opened for `creator_room_id` — `created_by` set,
/// not archived, no `cwd` unless the caller sets one. Mirrors
/// `agent_opened_room` above, but keeps the caller's given harnesses
/// instead of an empty list, since `close_room` tests need a real room
/// to check sign-off against.
pub(super) fn room_created_by(id: &str, harnesses: Vec<Harness>, creator_room_id: &str) -> Room {
    let mut r = room(id, harnesses);
    r.created_by = Some(created_by(creator_room_id, "ch"));
    r
}

#[tokio::test]
async fn close_room_is_refused_when_the_kill_switch_is_off() {
    let f = fixture();
    save(
        &f.db,
        &[
            room("r1", vec![harness("h1", "claude", "main")]),
            room_created_by("r2", vec![], "r1"),
        ],
    );
    let caller = caller_for(&f.db, "r1", Some("h1"));
    let state = agent_api_state(&f);
    let err = verbs::close_room(&state, &caller, &close_room_args("r2"), false)
        .await
        .unwrap_err();
    assert!(
        matches!(&err, VerbError::Refused(m) if m.starts_with("disabled:")),
        "{err:?}"
    );
}

#[tokio::test]
async fn close_room_is_rate_limited_to_five_attempts_a_minute() {
    let f = fixture();
    save(&f.db, &[room("r1", vec![harness("h1", "claude", "main")])]);
    let caller = caller_for(&f.db, "r1", Some("h1"));
    let state = agent_api_state(&f);
    // No target room exists at all — proof the rate counter is charged
    // before the target is even looked up, not after.
    for n in 0..5 {
        let err = verbs::close_room(&state, &caller, &close_room_args("missing"), true)
            .await
            .unwrap_err();
        assert!(
            matches!(&err, VerbError::NotFound(_)),
            "attempt {n}: expected the guards to pass through to NotFound, got {err:?}"
        );
    }
    let err = verbs::close_room(&state, &caller, &close_room_args("missing"), true)
        .await
        .unwrap_err();
    assert!(
        matches!(&err, VerbError::Refused(m) if m.starts_with("rate_limited:")),
        "{err:?}"
    );
}

#[tokio::test]
async fn close_room_unknown_room_is_not_found() {
    let f = fixture();
    save(&f.db, &[room("r1", vec![harness("h1", "claude", "main")])]);
    let caller = caller_for(&f.db, "r1", Some("h1"));
    let state = agent_api_state(&f);
    let err = verbs::close_room(&state, &caller, &close_room_args("nope"), true)
        .await
        .unwrap_err();
    assert!(
        matches!(&err, VerbError::NotFound(m) if m.contains("nope")),
        "{err:?}"
    );
}

#[tokio::test]
async fn close_room_cannot_close_itself() {
    let f = fixture();
    save(&f.db, &[room("r1", vec![harness("h1", "claude", "main")])]);
    let caller = caller_for(&f.db, "r1", Some("h1"));
    let state = agent_api_state(&f);
    let err = verbs::close_room(&state, &caller, &close_room_args("r1"), true)
        .await
        .unwrap_err();
    assert!(
        matches!(&err, VerbError::Refused(m) if m.starts_with("self:")),
        "{err:?}"
    );
}

#[tokio::test]
async fn close_room_refuses_a_room_with_no_created_by_at_all() {
    // A hand-created room — the New Room dialog, not create_room — has
    // no createdBy at all, so no room may close it.
    let f = fixture();
    save(
        &f.db,
        &[
            room("r1", vec![harness("h1", "claude", "main")]),
            room("r2", vec![]),
        ],
    );
    let caller = caller_for(&f.db, "r1", Some("h1"));
    let state = agent_api_state(&f);
    let err = verbs::close_room(&state, &caller, &close_room_args("r2"), true)
        .await
        .unwrap_err();
    assert!(
        matches!(&err, VerbError::Refused(m) if m.starts_with("not_creator:")),
        "{err:?}"
    );
}

#[tokio::test]
async fn close_room_refuses_a_room_created_by_a_different_room() {
    let f = fixture();
    save(
        &f.db,
        &[
            room("r1", vec![harness("h1", "claude", "main")]),
            room_created_by("r2", vec![], "r-someone-else"),
        ],
    );
    let caller = caller_for(&f.db, "r1", Some("h1"));
    let state = agent_api_state(&f);
    let err = verbs::close_room(&state, &caller, &close_room_args("r2"), true)
        .await
        .unwrap_err();
    assert!(
        matches!(&err, VerbError::Refused(m) if m.starts_with("not_creator:")),
        "{err:?}"
    );
}

#[tokio::test]
async fn close_room_refuses_an_already_archived_room() {
    let f = fixture();
    let mut target = room_created_by("r2", vec![], "r1");
    target.archived = Some(1);
    save(
        &f.db,
        &[room("r1", vec![harness("h1", "claude", "main")]), target],
    );
    let caller = caller_for(&f.db, "r1", Some("h1"));
    let state = agent_api_state(&f);
    let err = verbs::close_room(&state, &caller, &close_room_args("r2"), true)
        .await
        .unwrap_err();
    assert!(
        matches!(&err, VerbError::Refused(m) if m.starts_with("archived:")),
        "{err:?}"
    );
}

#[tokio::test]
async fn close_room_refuses_a_room_with_no_worktree_as_not_signed_off() {
    let f = fixture();
    save(
        &f.db,
        &[
            room("r1", vec![harness("h1", "claude", "main")]),
            room_created_by("r2", vec![], "r1"),
        ],
    );
    let caller = caller_for(&f.db, "r1", Some("h1"));
    let state = agent_api_state(&f);
    let err = verbs::close_room(&state, &caller, &close_room_args("r2"), true)
        .await
        .unwrap_err();
    assert!(
        matches!(&err, VerbError::Refused(m) if m.starts_with("not_signed_off:")),
        "{err:?}"
    );
}

#[tokio::test]
async fn close_room_refuses_a_worktree_room_that_is_not_signed_off() {
    let f = fixture();
    let tmp = TempDir::new().unwrap();
    git_repo_with_commit(tmp.path());
    let mut target = room_created_by("r2", vec![], "r1");
    target.cwd = Some(tmp.path().to_str().unwrap().to_owned());
    save(
        &f.db,
        &[room("r1", vec![harness("h1", "claude", "main")]), target],
    );
    let caller = caller_for(&f.db, "r1", Some("h1"));
    let state = agent_api_state(&f);
    let err = verbs::close_room(&state, &caller, &close_room_args("r2"), true)
        .await
        .unwrap_err();
    assert!(
        matches!(&err, VerbError::Refused(m) if m.starts_with("not_signed_off:")),
        "{err:?}"
    );
}

#[tokio::test]
async fn close_room_refuses_a_stale_signoff() {
    let f = fixture();
    let tmp = TempDir::new().unwrap();
    git_repo_with_commit(tmp.path());
    let cwd = tmp.path().to_str().unwrap().to_owned();
    let mut target = room_created_by("r2", vec![], "r1");
    target.cwd = Some(cwd.clone());
    save(
        &f.db,
        &[room("r1", vec![harness("h1", "claude", "main")]), target],
    );
    crate::review_surface::signoff::set_impl(&f.db, "r2", &cwd, true, None, 1_000).unwrap();
    // The agent commits after being approved — the approval must not
    // silently stretch over code the reviewer never saw.
    commit_file(tmp.path(), "b.txt", "more\n", "feat: more");

    let caller = caller_for(&f.db, "r1", Some("h1"));
    let state = agent_api_state(&f);
    let err = verbs::close_room(&state, &caller, &close_room_args("r2"), true)
        .await
        .unwrap_err();
    assert!(
        matches!(&err, VerbError::Refused(m) if m.starts_with("stale_signoff:")),
        "{err:?}"
    );
}

// ── the archive itself runs in the daemon (#734) ──────────────────────
//
// No client is attached in any of these unless a test says so: the verb
// archives against the RoomStore on its own.

/// `r1` (the caller) and a signed-off `r2` it created, holding `harnesses`,
/// both in the db and the store, with the token-revoking observer on.
pub(super) fn signed_off_room(
    f: &super::tests::Fixture,
    harnesses: Vec<Harness>,
) -> (TempDir, AgentApiState, std::sync::Arc<RoomStore>) {
    let tmp = TempDir::new().unwrap();
    git_repo_with_commit(tmp.path());
    let cwd = tmp.path().to_str().unwrap().to_owned();
    let mut target = room_created_by("r2", harnesses, "r1");
    target.cwd = Some(cwd.clone());
    let (state, store) = agent_api_state_with_room_records(
        f,
        vec![room("r1", vec![harness("h1", "claude", "main")]), target],
    );
    crate::review_surface::signoff::set_impl(&f.db, "r2", &cwd, true, None, 1_000).unwrap();
    (tmp, state, store)
}

fn stored(store: &RoomStore, id: &str) -> Room {
    store.rooms().into_iter().find(|r| r.id == id).unwrap()
}

#[tokio::test]
async fn close_room_archives_in_the_store_and_revokes_the_token() {
    let f = fixture();
    let (_tmp, state, store) = signed_off_room(&f, vec![harness("h2", "claude", "main")]);
    let r2_token = f.db.ensure_room_token("r2", 1).unwrap();
    let caller = caller_for(&f.db, "r1", Some("h1"));

    let out = verbs::close_room(&state, &caller, &close_room_args("r2"), true)
        .await
        .unwrap();
    assert_eq!(out.room_id, "r2");
    assert_eq!(out.closed_by.room_id, "r1");
    assert_eq!(out.closed_by.harness_id.as_deref(), Some("h1"));

    let r2 = stored(&store, "r2");
    assert_eq!(r2.archived, Some(out.archived));
    assert!(out.archived > 0);
    let closed_by = r2.closed_by.expect("closedBy is stamped");
    assert_eq!(closed_by.room_id, "r1");
    assert_eq!(closed_by.harness_id.as_deref(), Some("h1"));
    assert_eq!(closed_by.at, out.archived);
    assert!(stored(&store, "r1").archived.is_none());

    assert!(matches!(
        auth::authenticate(&f.db, Some(&r2_token), None),
        Err(AuthError::Revoked)
    ));
}

#[tokio::test]
async fn close_room_with_no_caller_harness_stamps_no_harness_id() {
    let f = fixture();
    let (_tmp, state, store) = signed_off_room(&f, vec![]);
    let caller = caller_for(&f.db, "r1", None);
    assert_eq!(
        caller.harness_id, None,
        "test setup: no harness to attribute to"
    );

    let out = verbs::close_room(&state, &caller, &close_room_args("r2"), true)
        .await
        .unwrap();
    assert_eq!(out.closed_by.harness_id, None);
    assert_eq!(stored(&store, "r2").closed_by.unwrap().harness_id, None);
}

#[tokio::test]
async fn close_room_without_a_room_store_is_unavailable() {
    let f = fixture();
    let tmp = TempDir::new().unwrap();
    git_repo_with_commit(tmp.path());
    let cwd = tmp.path().to_str().unwrap().to_owned();
    let mut target = room_created_by("r2", vec![], "r1");
    target.cwd = Some(cwd.clone());
    save(
        &f.db,
        &[room("r1", vec![harness("h1", "claude", "main")]), target],
    );
    crate::review_surface::signoff::set_impl(&f.db, "r2", &cwd, true, None, 1_000).unwrap();
    let caller = caller_for(&f.db, "r1", Some("h1"));
    let state = agent_api_state(&f);
    let err = verbs::close_room(&state, &caller, &close_room_args("r2"), true)
        .await
        .unwrap_err();
    assert!(matches!(&err, VerbError::Unavailable(_)), "{err:?}");
}

#[tokio::test]
async fn close_room_refuses_when_the_store_no_longer_has_it_open() {
    // The db row said open when the guards read it; the store is the
    // authority and had it archived in the meantime.
    let f = fixture();
    let (_tmp, state, store) = signed_off_room(&f, vec![]);
    let mut r2 = stored(&store, "r2");
    r2.archived = Some(7);
    store
        .apply("test", vec![RoomMutation::UpdateRoom { room: r2 }])
        .unwrap();
    let caller = caller_for(&f.db, "r1", Some("h1"));
    let err = verbs::close_room(&state, &caller, &close_room_args("r2"), true)
        .await
        .unwrap_err();
    assert!(
        matches!(&err, VerbError::Refused(m) if m.starts_with("archived:")),
        "{err:?}"
    );
}

/// A client that records every request kind it is asked and answers
/// `files.dirty` with `dirty`.
fn files_client(
    state: &AgentApiState,
    dirty: Value,
) -> std::sync::Arc<std::sync::Mutex<Vec<(String, Value)>>> {
    let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let seen_clone = std::sync::Arc::clone(&seen);
    state.set_test_frontend(move |kind, args| {
        seen_clone
            .lock()
            .unwrap()
            .push((kind.to_owned(), args.clone()));
        match kind {
            "files.dirty" => Ok(dirty.clone()),
            other => Err(format!("unexpected request_frontend kind {other:?}")),
        }
    });
    seen
}

#[tokio::test]
async fn close_room_refuses_unsaved_files_a_client_reports() {
    let f = fixture();
    let (_tmp, state, store) = signed_off_room(
        &f,
        vec![
            harness("h2", "claude", "main"),
            harness("hf", "files", "files"),
        ],
    );
    let seen = files_client(
        &state,
        json!({ "dirty": [{ "harnessId": "hf", "names": ["notes.md", "a.rs"] }] }),
    );
    let caller = caller_for(&f.db, "r1", Some("h1"));
    let err = verbs::close_room(&state, &caller, &close_room_args("r2"), true)
        .await
        .unwrap_err();
    assert!(
        matches!(&err, VerbError::Refused(m)
            if m.starts_with("unsaved_files:") && m.contains("notes.md") && m.contains("a.rs")),
        "{err:?}"
    );
    assert!(stored(&store, "r2").archived.is_none());
    let seen = seen.lock().unwrap();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].0, "files.dirty");
    assert_eq!(seen[0].1, json!({ "harnessIds": ["hf"] }));
}

#[tokio::test]
async fn close_room_proceeds_when_the_client_reports_the_buffers_clean() {
    let f = fixture();
    let (_tmp, state, store) = signed_off_room(&f, vec![harness("hf", "files", "files")]);
    let _seen = files_client(&state, json!({ "dirty": [] }));
    let caller = caller_for(&f.db, "r1", Some("h1"));
    verbs::close_room(&state, &caller, &close_room_args("r2"), true)
        .await
        .unwrap();
    assert!(stored(&store, "r2").archived.is_some());
}

#[tokio::test]
async fn close_room_with_a_files_harness_and_no_client_proceeds() {
    let f = fixture();
    let (_tmp, state, store) = signed_off_room(&f, vec![harness("hf", "files", "files")]);
    let caller = caller_for(&f.db, "r1", Some("h1"));
    verbs::close_room(&state, &caller, &close_room_args("r2"), true)
        .await
        .unwrap();
    assert!(stored(&store, "r2").archived.is_some());
}

#[tokio::test]
async fn close_room_never_asks_the_client_without_a_files_harness() {
    let f = fixture();
    let (_tmp, state, store) = signed_off_room(&f, vec![harness("h2", "claude", "main")]);
    let seen = files_client(&state, json!({ "dirty": [] }));
    let caller = caller_for(&f.db, "r1", Some("h1"));
    verbs::close_room(&state, &caller, &close_room_args("r2"), true)
        .await
        .unwrap();
    assert!(stored(&store, "r2").archived.is_some());
    assert!(seen.lock().unwrap().is_empty());
}

#[tokio::test]
async fn close_room_refuses_when_the_client_cannot_answer_files_dirty() {
    let f = fixture();
    let (_tmp, state, store) = signed_off_room(&f, vec![harness("hf", "files", "files")]);
    state.set_test_frontend(|_, _| Err("agent request 1 (files.dirty) timed out after 5s".into()));
    let caller = caller_for(&f.db, "r1", Some("h1"));
    let err = verbs::close_room(&state, &caller, &close_room_args("r2"), true)
        .await
        .unwrap_err();
    assert!(matches!(&err, VerbError::Unavailable(_)), "{err:?}");
    assert!(stored(&store, "r2").archived.is_none());
}
