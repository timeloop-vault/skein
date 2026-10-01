//! `create_room` (#330): argument validation, kill switch and rate cap. Also hosts the `create_room` fixtures its siblings share.

use serde_json::json;
use tempfile::TempDir;

use super::auth::Caller;
use super::review_tests::git_repo_with_commit;
use super::tests::{Fixture, agent_api_state, caller_for, fixture, harness, room, save};
use super::verbs::{self, MailContext, VerbError};
use crate::db::{CreatedBy, Room};

// ── opening a room (issue #330) ──────────────────────────────────────
//
// `create_room` is exercised as a plain function (`verbs::create_room`),
// not through `mcp::call_tool` — the properties under test here are the
// verb's own guards and round trips, already covered end to end for the
// dispatch layer by `destroying_a_room_is_refused_by_name` and the tool
// list test above. `AgentApiState::set_test_frontend` (test-only) stands
// in for the webview: `create_room.resolve` and `create_room` answer
// from a closure instead of a real `AppHandle` emit.

pub(super) fn create_room_args(task: &str) -> verbs::CreateRoomArgs {
    verbs::CreateRoomArgs {
        path: None,
        branch_mode: None,
        branch: None,
        base_branch: None,
        task: task.to_owned(),
        kind: None,
        agent: None,
        prompt: None,
    }
}

/// An open room `create_room`'s per-repository-group ceiling (#375)
/// counts: `created_by` set (as if an agent's own `create_room` call
/// had made it), not archived, grouped under `repo_root` (`None` for
/// the ungrouped bucket).
pub(super) fn agent_opened_room(id: &str, repo_root: Option<&str>) -> Room {
    let mut r = room(id, vec![]);
    r.repo_root = repo_root.map(str::to_owned);
    r.created_by = Some(CreatedBy {
        room_id: "creator".to_owned(),
        harness_id: "h".to_owned(),
        prompt_first_line: None,
        base_sha: None,
    });
    r
}

/// A caller room whose folder is a real git checkout with one commit —
/// what the default (worktree) `branchMode` needs at minimum.
pub(super) fn git_room(f: &Fixture) -> (TempDir, Caller) {
    let tmp = TempDir::new().unwrap();
    git_repo_with_commit(tmp.path());
    let mut r = room("r1", vec![harness("h1", "claude", "main")]);
    r.cwd = Some(tmp.path().to_str().unwrap().to_owned());
    save(&f.db, &[r]);
    let caller = caller_for(&f.db, "r1", Some("h1"));
    (tmp, caller)
}

#[tokio::test]
async fn create_room_rejects_an_empty_task() {
    let f = fixture();
    save(&f.db, &[room("r1", vec![harness("h1", "claude", "main")])]);
    let caller = caller_for(&f.db, "r1", Some("h1"));
    let state = agent_api_state(&f);
    let err = verbs::create_room(
        &state,
        &caller,
        &create_room_args("   "),
        &MailContext::permissive(),
        true,
    )
    .await
    .unwrap_err();
    assert!(
        matches!(&err, VerbError::Refused(m) if m.contains("task")),
        "{err:?}"
    );
}

#[tokio::test]
async fn create_room_rejects_an_unknown_branch_mode() {
    let f = fixture();
    save(&f.db, &[room("r1", vec![harness("h1", "claude", "main")])]);
    let caller = caller_for(&f.db, "r1", Some("h1"));
    let state = agent_api_state(&f);
    let args = verbs::CreateRoomArgs {
        branch_mode: Some("yolo".to_owned()),
        ..create_room_args("hi")
    };
    let err = verbs::create_room(&state, &caller, &args, &MailContext::permissive(), true)
        .await
        .unwrap_err();
    assert!(
        matches!(&err, VerbError::Refused(m) if m.contains("branchMode")),
        "{err:?}"
    );
}

#[tokio::test]
async fn create_room_rejects_an_unknown_kind() {
    let f = fixture();
    save(&f.db, &[room("r1", vec![harness("h1", "claude", "main")])]);
    let caller = caller_for(&f.db, "r1", Some("h1"));
    let state = agent_api_state(&f);
    let args = verbs::CreateRoomArgs {
        kind: Some("frobnicator".to_owned()),
        ..create_room_args("hi")
    };
    let err = verbs::create_room(&state, &caller, &args, &MailContext::permissive(), true)
        .await
        .unwrap_err();
    assert!(
        matches!(&err, VerbError::Refused(m) if m.contains("kind")),
        "{err:?}"
    );
}

#[tokio::test]
async fn create_room_rejects_a_relative_path() {
    let f = fixture();
    save(&f.db, &[room("r1", vec![harness("h1", "claude", "main")])]);
    let caller = caller_for(&f.db, "r1", Some("h1"));
    let state = agent_api_state(&f);
    let args = verbs::CreateRoomArgs {
        path: Some("relative/path".to_owned()),
        ..create_room_args("hi")
    };
    let err = verbs::create_room(&state, &caller, &args, &MailContext::permissive(), true)
        .await
        .unwrap_err();
    assert!(
        matches!(&err, VerbError::Refused(m) if m.contains("absolute")),
        "{err:?}"
    );
}

#[tokio::test]
async fn create_room_rejects_a_path_that_does_not_exist() {
    let f = fixture();
    save(&f.db, &[room("r1", vec![harness("h1", "claude", "main")])]);
    let caller = caller_for(&f.db, "r1", Some("h1"));
    let state = agent_api_state(&f);
    let bogus = std::env::temp_dir().join("skein-330-does-not-exist");
    let args = verbs::CreateRoomArgs {
        path: Some(bogus.to_str().unwrap().to_owned()),
        ..create_room_args("hi")
    };
    let err = verbs::create_room(&state, &caller, &args, &MailContext::permissive(), true)
        .await
        .unwrap_err();
    assert!(
        matches!(&err, VerbError::Refused(m) if m.contains("does not exist")),
        "{err:?}"
    );
}

#[tokio::test]
async fn create_room_worktree_mode_refuses_a_non_checkout_folder() {
    let f = fixture();
    save(&f.db, &[room("r1", vec![harness("h1", "claude", "main")])]);
    let caller = caller_for(&f.db, "r1", Some("h1"));
    let state = agent_api_state(&f);
    let tmp = TempDir::new().unwrap(); // exists, but never `git init`-ed
    let args = verbs::CreateRoomArgs {
        path: Some(tmp.path().to_str().unwrap().to_owned()),
        ..create_room_args("hi")
    };
    let err = verbs::create_room(&state, &caller, &args, &MailContext::permissive(), true)
        .await
        .unwrap_err();
    assert!(
        matches!(&err, VerbError::Refused(m) if m.contains("git checkout")),
        "{err:?}"
    );
}

#[tokio::test]
async fn create_room_current_mode_is_allowed_on_a_non_git_folder() {
    let f = fixture();
    save(&f.db, &[room("r1", vec![harness("h1", "claude", "main")])]);
    let caller = caller_for(&f.db, "r1", Some("h1"));
    let state = agent_api_state(&f);
    let tmp = TempDir::new().unwrap();
    state.set_test_frontend(|kind, _args| match kind {
        "create_room.resolve" => Ok(json!({ "kind": "byoh", "agent": null })),
        "create_room" => Ok(json!({
            "roomId": "r2", "name": "n", "harnessId": "h2", "kind": "byoh",
        })),
        other => Err(format!("unexpected request_frontend kind {other:?}")),
    });
    let args = verbs::CreateRoomArgs {
        path: Some(tmp.path().to_str().unwrap().to_owned()),
        branch_mode: Some("current".to_owned()),
        ..create_room_args("hi")
    };
    let out = verbs::create_room(&state, &caller, &args, &MailContext::permissive(), true)
        .await
        .unwrap();
    assert_eq!(out.room_id, "r2");
}

#[tokio::test]
async fn create_room_is_refused_when_the_kill_switch_is_off() {
    let f = fixture();
    let (_tmp, caller) = git_room(&f);
    let state = agent_api_state(&f);
    let err = verbs::create_room(
        &state,
        &caller,
        &create_room_args("hi"),
        &MailContext::permissive(),
        false,
    )
    .await
    .unwrap_err();
    assert!(
        matches!(&err, VerbError::Refused(m) if m.contains("Settings")),
        "{err:?}"
    );
}

#[tokio::test]
async fn create_room_is_rate_limited_to_five_attempts_a_minute() {
    let f = fixture();
    let (_tmp, caller) = git_room(&f);
    let state = agent_api_state(&f);
    // No frontend hook wired: each attempt clears the local guards and
    // only then fails at the (unanswerable) round trip — proof the rate
    // counter is charged before that point, not after.
    for n in 0..5 {
        let err = verbs::create_room(
            &state,
            &caller,
            &create_room_args("hi"),
            &MailContext::permissive(),
            true,
        )
        .await
        .unwrap_err();
        assert!(
            matches!(&err, VerbError::Unavailable(_)),
            "attempt {n}: expected the guards to pass, got {err:?}"
        );
    }
    let err = verbs::create_room(
        &state,
        &caller,
        &create_room_args("hi"),
        &MailContext::permissive(),
        true,
    )
    .await
    .unwrap_err();
    assert!(
        matches!(&err, VerbError::Refused(m) if m.contains("rate limit")),
        "{err:?}"
    );
}
