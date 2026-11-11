//! `create_room` (#330): the open-room ceiling and its per-repository grouping.

use serde_json::json;
use tempfile::TempDir;

use super::create_room_tests::{agent_opened_room, create_room_args};
use super::review_tests::git_repo_with_commit;
use super::tests::{agent_api_state, caller_for, fixture, harness, room, save};
use super::verbs::{self, MailContext, VerbError};

#[tokio::test]
async fn create_room_refuses_once_the_open_room_ceiling_is_hit() {
    let f = fixture();
    let tmp = TempDir::new().unwrap();
    git_repo_with_commit(tmp.path());
    let repo_root = tmp.path().to_str().unwrap().to_owned();
    let mut r1 = room("r1", vec![harness("h1", "claude", "main")]);
    r1.cwd = Some(repo_root.clone());
    let mut rooms = vec![r1];
    for n in 0..20 {
        rooms.push(agent_opened_room(&format!("extra-{n}"), Some(&repo_root)));
    }
    save(&f.db, &rooms); // 20 agent-opened rooms in the same group, none archived
    let caller = caller_for(&f.db, "r1", Some("h1"));
    let state = agent_api_state(&f);
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
        matches!(&err, VerbError::Refused(m) if m.contains("cap") && m.contains("repository group")),
        "{err:?}"
    );
}

#[tokio::test]
async fn create_room_hand_opened_rooms_do_not_count_toward_the_ceiling() {
    let f = fixture();
    let tmp = TempDir::new().unwrap();
    git_repo_with_commit(tmp.path());
    let repo_root = tmp.path().to_str().unwrap().to_owned();
    let mut r1 = room("r1", vec![harness("h1", "claude", "main")]);
    r1.cwd = Some(repo_root.clone());
    let mut rooms = vec![r1];
    for n in 0..25 {
        // Hand-opened: created_by stays None, well past the cap.
        let mut r = room(&format!("hand-{n}"), vec![]);
        r.repo_root = Some(repo_root.clone());
        rooms.push(r);
    }
    save(&f.db, &rooms);
    let caller = caller_for(&f.db, "r1", Some("h1"));
    let state = agent_api_state(&f);
    // No frontend hook wired: reaching the unanswerable round trip is
    // proof the ceiling did not fire, the same way the rate-limit test
    // above proves its own guard fired before the round trip.
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
        "hand-opened rooms must not trip the ceiling: {err:?}"
    );
}

#[tokio::test]
async fn create_room_archived_agent_opened_rooms_do_not_count_toward_the_ceiling() {
    let f = fixture();
    let tmp = TempDir::new().unwrap();
    git_repo_with_commit(tmp.path());
    let repo_root = tmp.path().to_str().unwrap().to_owned();
    let mut r1 = room("r1", vec![harness("h1", "claude", "main")]);
    r1.cwd = Some(repo_root.clone());
    let mut rooms = vec![r1];
    for n in 0..25 {
        let mut r = agent_opened_room(&format!("archived-{n}"), Some(&repo_root));
        r.archived = Some(1);
        rooms.push(r);
    }
    save(&f.db, &rooms);
    let caller = caller_for(&f.db, "r1", Some("h1"));
    let state = agent_api_state(&f);
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
        "archived rooms must not trip the ceiling: {err:?}"
    );
}

#[tokio::test]
async fn create_room_ceiling_is_scoped_per_repository_group() {
    let f = fixture();
    let repo_a = TempDir::new().unwrap();
    git_repo_with_commit(repo_a.path());
    let repo_a_root = repo_a.path().to_str().unwrap().to_owned();

    let repo_b = TempDir::new().unwrap();
    git_repo_with_commit(repo_b.path());

    let mut r1 = room("r1", vec![harness("h1", "claude", "main")]);
    r1.cwd = Some(repo_b.path().to_str().unwrap().to_owned());
    let mut rooms = vec![r1];
    for n in 0..20 {
        rooms.push(agent_opened_room(&format!("a-{n}"), Some(&repo_a_root)));
    }
    save(&f.db, &rooms); // 20 agent-opened rooms, all in repo A's group
    let caller = caller_for(&f.db, "r1", Some("h1"));
    let state = agent_api_state(&f);
    state.set_test_frontend(|kind, _args| match kind {
        "create_room.resolve" => Ok(json!({ "kind": "claude", "agent": null })),
        "create_room" => Ok(json!({
            "roomId": "r2", "name": "n", "harnessId": "h2", "kind": "claude",
        })),
        other => Err(format!("unexpected request_frontend kind {other:?}")),
    });
    // r1 defaults to its own cwd (repo B) — a different group, so repo
    // A's 20 agent-opened rooms must not block it.
    let out = verbs::create_room(
        &state,
        &caller,
        &create_room_args("hi"),
        &MailContext::permissive(),
        true,
    )
    .await
    .unwrap();
    assert_eq!(out.room_id, "r2");
}

#[tokio::test]
async fn create_room_ceiling_ungrouped_bucket_refuses_a_non_git_create() {
    let f = fixture();
    let mut rooms = vec![room("r1", vec![harness("h1", "claude", "main")])];
    for n in 0..20 {
        rooms.push(agent_opened_room(&format!("ungrouped-{n}"), None));
    }
    save(&f.db, &rooms); // 20 agent-opened rooms with no repo_root at all
    let caller = caller_for(&f.db, "r1", Some("h1"));
    let state = agent_api_state(&f);
    let tmp = TempDir::new().unwrap(); // never `git init`-ed
    let args = verbs::CreateRoomArgs {
        path: Some(tmp.path().to_str().unwrap().to_owned()),
        branch_mode: Some("current".to_owned()),
        ..create_room_args("hi")
    };
    let err = verbs::create_room(&state, &caller, &args, &MailContext::permissive(), true)
        .await
        .unwrap_err();
    assert!(
        matches!(&err, VerbError::Refused(m) if m.contains("cap") && m.contains("ungrouped")),
        "{err:?}"
    );
}

#[tokio::test]
async fn create_room_ceiling_ungrouped_bucket_does_not_block_a_repo_create() {
    let f = fixture();
    let repo = TempDir::new().unwrap();
    git_repo_with_commit(repo.path());
    let mut rooms = vec![room("r1", vec![harness("h1", "claude", "main")])];
    for n in 0..20 {
        rooms.push(agent_opened_room(&format!("ungrouped-{n}"), None));
    }
    save(&f.db, &rooms);
    let caller = caller_for(&f.db, "r1", Some("h1"));
    let state = agent_api_state(&f);
    state.set_test_frontend(|kind, _args| match kind {
        "create_room.resolve" => Ok(json!({ "kind": "claude", "agent": null })),
        "create_room" => Ok(json!({
            "roomId": "r2", "name": "n", "harnessId": "h2", "kind": "claude",
        })),
        other => Err(format!("unexpected request_frontend kind {other:?}")),
    });
    let args = verbs::CreateRoomArgs {
        path: Some(repo.path().to_str().unwrap().to_owned()),
        ..create_room_args("hi")
    };
    let out = verbs::create_room(&state, &caller, &args, &MailContext::permissive(), true)
        .await
        .unwrap();
    assert_eq!(out.room_id, "r2");
}

#[tokio::test]
async fn create_room_ceiling_repo_group_does_not_block_an_ungrouped_create() {
    let f = fixture();
    let repo = TempDir::new().unwrap();
    git_repo_with_commit(repo.path());
    let repo_root = repo.path().to_str().unwrap().to_owned();
    let mut rooms = vec![room("r1", vec![harness("h1", "claude", "main")])];
    for n in 0..20 {
        rooms.push(agent_opened_room(&format!("grouped-{n}"), Some(&repo_root)));
    }
    save(&f.db, &rooms);
    let caller = caller_for(&f.db, "r1", Some("h1"));
    let state = agent_api_state(&f);
    state.set_test_frontend(|kind, _args| match kind {
        "create_room.resolve" => Ok(json!({ "kind": "byoh", "agent": null })),
        "create_room" => Ok(json!({
            "roomId": "r2", "name": "n", "harnessId": "h2", "kind": "byoh",
        })),
        other => Err(format!("unexpected request_frontend kind {other:?}")),
    });
    let tmp = TempDir::new().unwrap(); // never `git init`-ed
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
async fn create_room_ceiling_counts_a_linked_worktree_against_its_main_checkout_group() {
    let f = fixture();
    let main_dir = TempDir::new().unwrap();
    git_repo_with_commit(main_dir.path());
    let main_root = main_dir.path().to_str().unwrap().to_owned();
    let base_branch = {
        let raw = git2::Repository::open(main_dir.path()).unwrap();
        raw.head().unwrap().shorthand().unwrap().to_owned()
    };

    let wt_parent = TempDir::new().unwrap();
    let wt_path = wt_parent.path().join("linked-wt");
    let main_repo = skein_git::Repo::open(main_dir.path()).unwrap();
    main_repo
        .add_worktree("feature/room-cap-375", &base_branch, &wt_path)
        .unwrap();

    let mut r1 = room("r1", vec![harness("h1", "claude", "main")]);
    r1.cwd = Some(wt_path.to_str().unwrap().to_owned());
    let mut rooms = vec![r1];
    for n in 0..20 {
        rooms.push(agent_opened_room(&format!("main-{n}"), Some(&main_root)));
    }
    save(&f.db, &rooms); // 20 agent-opened rooms grouped under the MAIN checkout
    let caller = caller_for(&f.db, "r1", Some("h1"));
    let state = agent_api_state(&f);
    // r1's own folder is the linked worktree, not the main checkout. If
    // the ceiling grouped by the raw worktree path instead of resolving
    // it to the main checkout first, none of the seeded rooms (grouped
    // under `main_root`) would match and this would reach the
    // (unanswerable) round trip instead — `Refused` here is proof the
    // resolution ran before the count.
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
        matches!(&err, VerbError::Refused(m) if m.contains("cap") && m.contains("repository group")),
        "{err:?}"
    );
}
/// The same repository reached through two spellings of its path must
/// still be one group. Here the seeded rooms name the checkout through
/// a symlink and the caller sits at its real path — the everyday form of
/// this is macOS, where libgit2 reports a worktree's main checkout under
/// `/private/var` while a room created at the checkout itself keeps the
/// `/var` it was handed. Without canonicalizing, the 20 seeded rooms
/// would count for nothing and this would reach the (unanswerable)
/// round trip instead of `Refused`.
#[cfg(unix)]
#[tokio::test]
async fn create_room_ceiling_matches_a_group_through_a_symlinked_spelling() {
    let f = fixture();
    let parent = TempDir::new().unwrap();
    let real = parent.path().join("real");
    std::fs::create_dir(&real).unwrap();
    git_repo_with_commit(&real);
    let link = parent.path().join("link");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    let via_link = link.to_str().unwrap().to_owned();

    let mut r1 = room("r1", vec![harness("h1", "claude", "main")]);
    r1.cwd = Some(real.to_str().unwrap().to_owned());
    let mut rooms = vec![r1];
    for n in 0..20 {
        rooms.push(agent_opened_room(&format!("l-{n}"), Some(&via_link)));
    }
    save(&f.db, &rooms);
    let caller = caller_for(&f.db, "r1", Some("h1"));
    let state = agent_api_state(&f);
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
        matches!(&err, VerbError::Refused(m) if m.contains("cap") && m.contains("repository group")),
        "{err:?}"
    );
}
