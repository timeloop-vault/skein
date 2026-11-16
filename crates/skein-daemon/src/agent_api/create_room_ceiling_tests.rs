//! `create_room` (#330): the open-room ceiling and its per-repository grouping.

use tempfile::TempDir;

use super::create_room_tests::{Env, agent_opened_room, create_room_args, env_with, init_repo};
use super::tests::{agent_api_state_with_room_records, caller_for, fixture, harness, room};
use super::verbs::{self, MailContext, VerbError};

const CAP: usize = 20;

/// `r1`'s repository with `n` agent-opened rooms grouped under it.
fn crowded(n: usize) -> Env {
    env_with(|repo| {
        (0..n)
            .map(|i| agent_opened_room(&format!("extra-{i}"), Some(repo)))
            .collect()
    })
}

async fn create(
    env: &Env,
    args: &verbs::CreateRoomArgs,
) -> Result<verbs::CreateRoomOut, VerbError> {
    verbs::create_room(
        &env.state,
        &env.caller,
        args,
        &MailContext::permissive(),
        true,
    )
    .await
}

fn is_group_cap(err: &VerbError) -> bool {
    matches!(err, VerbError::Refused(m) if m.contains("cap") && m.contains("repository group"))
}

#[tokio::test]
async fn create_room_refuses_once_the_open_room_ceiling_is_hit() {
    let env = crowded(CAP);
    let err = create(&env, &create_room_args("hi")).await.unwrap_err();
    assert!(is_group_cap(&err), "{err:?}");
    assert_eq!(env.room_ids().len(), CAP + 1, "nothing was created");
}

#[tokio::test]
async fn create_room_is_allowed_one_below_the_ceiling() {
    let env = crowded(CAP - 1);
    create(&env, &create_room_args("hi")).await.unwrap();
}

#[tokio::test]
async fn create_room_hand_opened_rooms_do_not_count_toward_the_ceiling() {
    let env = env_with(|repo| {
        (0..25)
            .map(|n| {
                // Hand-opened: created_by stays None, well past the cap.
                let mut r = room(&format!("hand-{n}"), vec![]);
                r.repo_root = Some(repo.to_owned());
                r
            })
            .collect()
    });
    create(&env, &create_room_args("hi")).await.unwrap();
}

#[tokio::test]
async fn create_room_archived_agent_opened_rooms_do_not_count_toward_the_ceiling() {
    let env = env_with(|repo| {
        (0..25)
            .map(|n| {
                let mut r = agent_opened_room(&format!("archived-{n}"), Some(repo));
                r.archived = Some(1);
                r
            })
            .collect()
    });
    create(&env, &create_room_args("hi")).await.unwrap();
}

#[tokio::test]
async fn create_room_ceiling_is_scoped_per_repository_group() {
    let other = TempDir::new().unwrap();
    let other_root = init_repo(&other);
    // 20 agent-opened rooms, all in another repository's group; r1 defaults
    // to its own cwd, a different group, so they must not block it.
    let env = env_with(|_| {
        (0..CAP)
            .map(|n| agent_opened_room(&format!("a-{n}"), Some(&other_root)))
            .collect()
    });
    create(&env, &create_room_args("hi")).await.unwrap();
}

#[tokio::test]
async fn create_room_ceiling_ungrouped_bucket_refuses_a_non_git_create() {
    let env = env_with(|_| {
        (0..CAP)
            .map(|n| agent_opened_room(&format!("ungrouped-{n}"), None))
            .collect()
    });
    let plain = env.tmp.path().join("plain");
    std::fs::create_dir(&plain).unwrap();
    let args = verbs::CreateRoomArgs {
        path: Some(plain.to_str().unwrap().to_owned()),
        branch_mode: Some("current".to_owned()),
        ..create_room_args("hi")
    };
    let err = create(&env, &args).await.unwrap_err();
    assert!(
        matches!(&err, VerbError::Refused(m) if m.contains("cap") && m.contains("ungrouped")),
        "{err:?}"
    );
}

#[tokio::test]
async fn create_room_ceiling_ungrouped_bucket_does_not_block_a_repo_create() {
    let env = env_with(|_| {
        (0..CAP)
            .map(|n| agent_opened_room(&format!("ungrouped-{n}"), None))
            .collect()
    });
    create(&env, &create_room_args("hi")).await.unwrap();
}

#[tokio::test]
async fn create_room_ceiling_repo_group_does_not_block_an_ungrouped_create() {
    let env = crowded(CAP);
    let plain = env.tmp.path().join("plain");
    std::fs::create_dir(&plain).unwrap();
    let args = verbs::CreateRoomArgs {
        path: Some(plain.to_str().unwrap().to_owned()),
        branch_mode: Some("current".to_owned()),
        kind: Some("byoh".to_owned()),
        ..create_room_args("hi")
    };
    create(&env, &args).await.unwrap();
}

#[tokio::test]
async fn create_room_ceiling_counts_a_linked_worktree_against_its_main_checkout_group() {
    let f = fixture();
    let tmp = TempDir::new().unwrap();
    let main_root = init_repo(&tmp);
    let base_branch = {
        let raw = git2::Repository::open(&main_root).unwrap();
        raw.head().unwrap().shorthand().unwrap().to_owned()
    };
    let wt_path = tmp.path().join("linked-wt");
    skein_git::Repo::open(std::path::Path::new(&main_root))
        .unwrap()
        .add_worktree("feature/room-cap-375", &base_branch, &wt_path)
        .unwrap();

    let mut r1 = room("r1", vec![harness("h1", "claude", "main")]);
    r1.cwd = Some(wt_path.to_str().unwrap().to_owned());
    let mut rooms = vec![r1];
    for n in 0..CAP {
        rooms.push(agent_opened_room(&format!("main-{n}"), Some(&main_root)));
    }
    let (state, _store) = agent_api_state_with_room_records(&f, rooms);
    let caller = caller_for(&f.db, "r1", Some("h1"));
    // r1's own folder is the linked worktree, not the main checkout. If the
    // ceiling grouped by the raw worktree path instead of resolving it to
    // the main checkout first, none of the seeded rooms would match and the
    // room would be created: `Refused` is proof the resolution ran first.
    let err = verbs::create_room(
        &state,
        &caller,
        &create_room_args("hi"),
        &MailContext::permissive(),
        true,
    )
    .await
    .unwrap_err();
    assert!(is_group_cap(&err), "{err:?}");
}

/// The same repository reached through two spellings of its path must
/// still be one group. Here the seeded rooms name the checkout through
/// a symlink and the caller sits at its real path — the everyday form of
/// this is macOS, where libgit2 reports a worktree's main checkout under
/// `/private/var` while a room created at the checkout itself keeps the
/// `/var` it was handed. Without canonicalizing, the 20 seeded rooms
/// would count for nothing and the room would be created.
#[cfg(unix)]
#[tokio::test]
async fn create_room_ceiling_matches_a_group_through_a_symlinked_spelling() {
    let parent = TempDir::new().unwrap();
    let real = init_repo(&parent);
    let link = parent.path().join("link");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    let via_link = link.to_str().unwrap().to_owned();
    let f = fixture();
    let mut r1 = room("r1", vec![harness("h1", "claude", "main")]);
    r1.cwd = Some(real);
    let mut rooms = vec![r1];
    for n in 0..CAP {
        rooms.push(agent_opened_room(&format!("l-{n}"), Some(&via_link)));
    }
    let (state, _store) = agent_api_state_with_room_records(&f, rooms);
    let caller = caller_for(&f.db, "r1", Some("h1"));
    let err = verbs::create_room(
        &state,
        &caller,
        &create_room_args("hi"),
        &MailContext::permissive(),
        true,
    )
    .await
    .unwrap_err();
    assert!(is_group_cap(&err), "{err:?}");
}
