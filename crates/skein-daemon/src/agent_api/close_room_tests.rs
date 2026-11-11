//! `close_room` (#411): guards, sign-off requirements and the frontend archive.

use serde_json::{Value, json};
use tempfile::TempDir;

use super::review_tests::{commit_file, git_repo_with_commit};
use super::room_listing_tests::created_by;
use super::tests::{agent_api_state, caller_for, fixture, harness, room, save};
use super::verbs::{self, VerbError};
use crate::db::{Harness, Room};

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

#[tokio::test]
async fn close_room_frontend_unsaved_files_refusal_keeps_its_code() {
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
    state.set_test_frontend(|kind, _args| match kind {
        "close_room" => Err("unsaved_files: notes.md".to_owned()),
        other => Err(format!("unexpected request_frontend kind {other:?}")),
    });
    let err = verbs::close_room(&state, &caller, &close_room_args("r2"), true)
        .await
        .unwrap_err();
    assert!(
        matches!(&err, VerbError::Refused(m) if m.starts_with("unsaved_files:")),
        "the frontend's own code must survive frontend_error unchanged: {err:?}"
    );
}

#[tokio::test]
async fn close_room_maps_a_missing_webview_to_unavailable() {
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
    let state = agent_api_state(&f); // no test_frontend hook, no AppHandle
    let err = verbs::close_room(&state, &caller, &close_room_args("r2"), true)
        .await
        .unwrap_err();
    assert!(
        matches!(&err, VerbError::Unavailable(m) if m.contains("no webview")),
        "{err:?}"
    );
}

#[tokio::test]
async fn close_room_happy_path_archives_via_the_frontend() {
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
    let captured = std::sync::Arc::new(std::sync::Mutex::new(None::<Value>));
    let captured_clone = std::sync::Arc::clone(&captured);
    state.set_test_frontend(move |kind, args| match kind {
        "close_room" => {
            *captured_clone.lock().unwrap() = Some(args.clone());
            Ok(json!({ "roomId": "r2", "archived": 12_345 }))
        }
        other => Err(format!("unexpected request_frontend kind {other:?}")),
    });

    let out = verbs::close_room(&state, &caller, &close_room_args("r2"), true)
        .await
        .unwrap();
    assert_eq!(out.room_id, "r2");
    assert_eq!(out.archived, 12_345);
    assert_eq!(out.closed_by.room_id, "r1");
    assert_eq!(out.closed_by.harness_id.as_deref(), Some("h1"));

    let sent = captured
        .lock()
        .unwrap()
        .clone()
        .expect("close_room requested");
    assert_eq!(sent["roomId"], json!("r2"));
    assert_eq!(sent["closedBy"]["roomId"], json!("r1"));
    assert_eq!(sent["closedBy"]["harnessId"], json!("h1"));
}

#[tokio::test]
async fn close_room_happy_path_with_no_caller_harness_sends_a_null_harness_id() {
    // Unlike `create_room`'s `createdBy.harnessId` (always a string,
    // `""` when absent — the frontend's `parseCreateArgs` requires the
    // type), `close_room`'s `closedBy.harnessId` is a genuine optional:
    // there is no equivalent frontend contract forcing a string here.
    let f = fixture();
    let tmp = TempDir::new().unwrap();
    git_repo_with_commit(tmp.path());
    let cwd = tmp.path().to_str().unwrap().to_owned();
    let mut target = room_created_by("r2", vec![], "r1");
    target.cwd = Some(cwd.clone());
    save(&f.db, &[room("r1", vec![]), target]);
    crate::review_surface::signoff::set_impl(&f.db, "r2", &cwd, true, None, 1_000).unwrap();

    let caller = caller_for(&f.db, "r1", None);
    assert_eq!(
        caller.harness_id, None,
        "test setup: no harness to attribute to"
    );
    let state = agent_api_state(&f);
    let captured = std::sync::Arc::new(std::sync::Mutex::new(None::<Value>));
    let captured_clone = std::sync::Arc::clone(&captured);
    state.set_test_frontend(move |kind, args| match kind {
        "close_room" => {
            *captured_clone.lock().unwrap() = Some(args.clone());
            Ok(json!({ "roomId": "r2", "archived": 1 }))
        }
        other => Err(format!("unexpected request_frontend kind {other:?}")),
    });

    let out = verbs::close_room(&state, &caller, &close_room_args("r2"), true)
        .await
        .unwrap();
    assert_eq!(out.closed_by.harness_id, None);

    let sent = captured
        .lock()
        .unwrap()
        .clone()
        .expect("close_room requested");
    assert_eq!(sent["closedBy"]["harnessId"], Value::Null);
}
