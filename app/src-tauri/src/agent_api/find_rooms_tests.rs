//! `find_rooms_for_path` (#354): matching, `safe_to_remove` and the MCP call.

use serde_json::json;

use super::mcp;
use super::tests::{agent_api_state, caller_for, fixture, harness, room, save};
use super::verbs::{self, FindRoomsForPathArgs, MailContext, VerbError};
use crate::db::{Database, Room};

// ── finding rooms by path, issue #354 ───────────────────────────────

pub(super) fn room_with_cwd(id: &str, cwd: &str) -> Room {
    let mut r = room(id, vec![]);
    r.cwd = Some(cwd.to_owned());
    r
}

fn find_paths(db: &Database, path: &str) -> Result<verbs::FindRoomsForPathOut, VerbError> {
    verbs::find_rooms_for_path(
        db,
        &FindRoomsForPathArgs {
            path: path.to_owned(),
        },
    )
}

#[test]
fn find_rooms_for_path_matches_an_exact_cwd() {
    let f = fixture();
    save(&f.db, &[room_with_cwd("r1", "C:/repo-wt/task-1")]);
    let out = find_paths(&f.db, "C:/repo-wt/task-1").unwrap();
    assert_eq!(out.rooms.len(), 1);
    assert_eq!(out.rooms[0].room_id, "r1");
    assert_eq!(out.rooms[0].match_kind, "cwd");
    assert!(!out.rooms[0].archived);
    assert!(!out.rooms[0].safe_to_remove);
    assert_eq!(out.unreadable_rooms, 0);
}

#[test]
fn find_rooms_for_path_matches_a_parent_of_the_room_cwd() {
    // The query is the `<repo>-wt` directory that CONTAINS the room's
    // own worktree folder — the shape a caller checks before removing
    // a whole `-wt` tree.
    let f = fixture();
    save(&f.db, &[room_with_cwd("r1", "C:/repo-wt/task-1")]);
    let out = find_paths(&f.db, "C:/repo-wt").unwrap();
    assert_eq!(out.rooms.len(), 1);
    assert_eq!(out.rooms[0].match_kind, "contains_room");
}

#[test]
fn find_rooms_for_path_matches_a_child_of_the_room_cwd() {
    // The query is a subfolder underneath the room's own cwd.
    let f = fixture();
    save(&f.db, &[room_with_cwd("r1", "C:/repo-wt/task-1")]);
    let out = find_paths(&f.db, "C:/repo-wt/task-1/src/lib").unwrap();
    assert_eq!(out.rooms.len(), 1);
    assert_eq!(out.rooms[0].match_kind, "inside_room");
}

#[test]
fn find_rooms_for_path_reports_an_archived_room_as_safe_to_remove() {
    let f = fixture();
    let mut r = room_with_cwd("r1", "C:/repo-wt/task-1");
    r.archived = Some(1);
    save(&f.db, &[r]);
    let out = find_paths(&f.db, "C:/repo-wt/task-1").unwrap();
    assert_eq!(out.rooms.len(), 1);
    assert!(out.rooms[0].archived);
    assert!(out.rooms[0].safe_to_remove);
}

#[test]
fn find_rooms_for_path_keeps_a_retired_room_flagged_and_safe_to_remove() {
    let f = fixture();
    let mut r = room_with_cwd("r1", "C:/repo-wt/task-1");
    r.archived = Some(1);
    r.retired = Some(2);
    save(&f.db, &[r, room_with_cwd("r2", "C:/repo-wt/task-2")]);
    let out = find_paths(&f.db, "C:/repo-wt").unwrap();
    let retired = out.rooms.iter().find(|m| m.room_id == "r1").unwrap();
    assert!(retired.retired && retired.archived && retired.safe_to_remove);
    let open = out.rooms.iter().find(|m| m.room_id == "r2").unwrap();
    assert!(!open.retired && !open.safe_to_remove);
}

#[test]
fn find_rooms_for_path_flags_a_repo_mismatch_without_changing_safe_to_remove() {
    let f = fixture();
    let plain = tempfile::TempDir::new().unwrap();
    let cwd = plain.path().to_string_lossy().into_owned();
    let ident = crate::db::RepoIdentity {
        root_commits: vec!["deadbeef".into()],
        origin_url: None,
    };
    // Open room whose folder now holds no repo: mismatch, still not safe.
    let mut open = room_with_cwd("r1", &cwd);
    open.repo_identity = Some(ident.clone());
    // Same identity but the folder is gone: unknown, not a mismatch.
    let mut gone = room_with_cwd("r2", &plain.path().join("nope").to_string_lossy());
    gone.repo_identity = Some(ident);
    // No identity recorded: unknown.
    let bare = room_with_cwd("r3", &plain.path().join("bare").to_string_lossy());
    save(&f.db, &[open, gone, bare]);
    let out = find_paths(&f.db, &cwd).unwrap();
    let by = |id: &str| out.rooms.iter().find(|m| m.room_id == id).unwrap();
    assert!(by("r1").repo_mismatch && !by("r1").safe_to_remove);
    assert!(!by("r2").repo_mismatch);
    assert!(!by("r3").repo_mismatch);
}

#[test]
fn find_rooms_for_path_never_marks_an_open_room_safe_to_remove() {
    let f = fixture();
    save(
        &f.db,
        &[
            room_with_cwd("r1", "C:/repo-wt/task-1"),
            room_with_cwd("r2", "C:/repo-wt/task-2"),
        ],
    );
    let out = find_paths(&f.db, "C:/repo-wt").unwrap();
    assert_eq!(out.rooms.len(), 2);
    for m in &out.rooms {
        assert!(!m.archived, "room {} should read as open", m.room_id);
        assert!(
            !m.safe_to_remove,
            "an open room ({}) must never read safe_to_remove",
            m.room_id
        );
    }
}

#[test]
fn find_rooms_for_path_finds_nothing_for_an_unrelated_path() {
    let f = fixture();
    save(&f.db, &[room_with_cwd("r1", "C:/repo-wt/task-1")]);
    let out = find_paths(&f.db, "C:/somewhere/else").unwrap();
    assert!(out.rooms.is_empty());
    assert_eq!(out.unreadable_rooms, 0);
}

#[test]
fn find_rooms_for_path_reports_unreadable_rooms_but_still_returns_parseable_matches() {
    let f = fixture();
    save(
        &f.db,
        &[
            room_with_cwd("r1", "C:/repo-wt/task-1"),
            room_with_cwd("r-bad", "C:/repo-wt/task-2"),
        ],
    );
    f.db.corrupt_room_for_test("r-bad");

    let out = find_paths(&f.db, "C:/repo-wt").unwrap();
    let ids: Vec<&str> = out.rooms.iter().map(|m| m.room_id.as_str()).collect();
    assert_eq!(
        ids,
        vec!["r1"],
        "a room Skein cannot parse must not silently appear as a match"
    );
    assert!(
        out.unreadable_rooms >= 1,
        "an unparseable room must be counted, so its absence from `rooms` is never \
         read as permission to remove its folder"
    );
}

#[test]
fn find_rooms_for_path_matches_windows_case_separator_and_trailing_slash_variants() {
    let f = fixture();
    save(&f.db, &[room_with_cwd("r1", "C:/repo-wt/task-1")]);
    for query in [
        "c:/repo-wt/task-1",
        "C:\\repo-wt\\task-1",
        "C:/REPO-WT/TASK-1",
        "C:/repo-wt/task-1/",
        "c:\\Repo-Wt\\Task-1\\",
    ] {
        let out = find_paths(&f.db, query).unwrap();
        assert_eq!(
            out.rooms.len(),
            1,
            "expected {query:?} to match the room's cwd"
        );
        assert_eq!(out.rooms[0].match_kind, "cwd");
    }
}

#[test]
fn find_rooms_for_path_respects_path_segment_boundaries() {
    let f = fixture();
    save(&f.db, &[room_with_cwd("r1", "C:/repo-wt/foo")]);
    let out = find_paths(&f.db, "C:/repo-wt/foobar").unwrap();
    assert!(
        out.rooms.is_empty(),
        "foobar must not match under foo: {out:?}"
    );

    let f2 = fixture();
    save(&f2.db, &[room_with_cwd("r1", "C:/repo-wt/foobar")]);
    let out2 = find_paths(&f2.db, "C:/repo-wt/foo").unwrap();
    assert!(
        out2.rooms.is_empty(),
        "foo must not match under foobar: {out2:?}"
    );
}

#[test]
fn find_rooms_for_path_refuses_an_empty_path() {
    let f = fixture();
    save(&f.db, &[room_with_cwd("r1", "C:/repo-wt/task-1")]);
    for bad in ["", "   "] {
        let err = find_paths(&f.db, bad).unwrap_err();
        assert!(matches!(err, VerbError::Refused(_)));
    }
}

#[test]
fn find_rooms_for_path_skips_a_room_with_no_cwd() {
    let f = fixture();
    save(
        &f.db,
        &[room("r1", vec![]), room_with_cwd("r2", "C:/repo-wt/task-1")],
    );
    let out = find_paths(&f.db, "C:/repo-wt/task-1").unwrap();
    assert_eq!(out.rooms.len(), 1);
    assert_eq!(out.rooms[0].room_id, "r2");
}

#[test]
fn find_rooms_for_path_orders_exact_matches_before_overlapping_matches() {
    let f = fixture();
    save(
        &f.db,
        &[
            // r1's cwd sits under the query (query CONTAINS_ROOM).
            room_with_cwd("r1", "C:/repo-wt/task-1"),
            // r2 is an EXACT match on the query itself.
            room_with_cwd("r2", "C:/repo-wt"),
        ],
    );
    let out = find_paths(&f.db, "C:/repo-wt").unwrap();
    let ids: Vec<&str> = out.rooms.iter().map(|m| m.room_id.as_str()).collect();
    assert_eq!(
        ids,
        vec!["r2", "r1"],
        "the exact match must sort before the overlapping match regardless of room order"
    );
}

#[tokio::test]
async fn find_rooms_for_path_call_through_mcp() {
    let f = fixture();
    save(
        &f.db,
        &[
            room_with_cwd("r1", "C:/repo-wt/task-1"),
            room("r2", vec![harness("h2", "claude", "main")]),
        ],
    );
    // Called from r2's token — proof the answer is NOT scoped to the
    // caller's own room, unlike every other verb in this file.
    let caller = caller_for(&f.db, "r2", Some("h2"));
    let state = agent_api_state(&f);

    let result = mcp::call_tool(
        &state,
        &caller,
        "find_rooms_for_path",
        &json!({ "path": "C:/repo-wt/task-1" }),
        &MailContext::permissive(),
    )
    .await
    .unwrap();
    assert_eq!(result["rooms"][0]["room_id"], "r1");
    assert_eq!(result["rooms"][0]["match"], "cwd");
    assert_eq!(result["rooms"][0]["safe_to_remove"], false);
}
