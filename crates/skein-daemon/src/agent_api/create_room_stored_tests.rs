//! `create_room` (#330, #734): what the daemon stores and says once the guards
//! pass. No client is attached anywhere here, and the repositories are `git2`
//! fixtures.

use serde_json::json;

use super::create_room_tests::{Env, create_room_args, git_env};
use super::review_tests::commit_file;
use super::tests::caller_for;
use super::verbs::{self, MailContext, MailPolicy, VerbError};
use crate::db::Room;

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

fn stored(env: &Env, room_id: &str) -> Room {
    env.store
        .rooms()
        .into_iter()
        .find(|r| r.id == room_id)
        .unwrap_or_else(|| panic!("room {room_id} is not in the store"))
}

fn head_sha(dir: &str) -> String {
    let repo = git2::Repository::open(dir).unwrap();
    repo.head()
        .unwrap()
        .peel_to_commit()
        .unwrap()
        .id()
        .to_string()
}

fn head_branch(dir: &str) -> String {
    let repo = git2::Repository::open(dir).unwrap();
    repo.head().unwrap().shorthand().unwrap().to_owned()
}

fn current_mode(task: &str) -> verbs::CreateRoomArgs {
    verbs::CreateRoomArgs {
        branch_mode: Some("current".to_owned()),
        ..create_room_args(task)
    }
}

fn refusal(err: &VerbError) -> &str {
    match err {
        VerbError::Refused(m) => m,
        other => panic!("expected a refusal, got {other:?}"),
    }
}

#[tokio::test]
async fn current_branch_mode_stores_a_room_in_the_folder_as_it_is() {
    let env = git_env();
    let args = verbs::CreateRoomArgs {
        kind: Some("byoh".to_owned()),
        ..current_mode("  look around ")
    };
    let out = create(&env, &args).await.unwrap();

    let room = stored(&env, &out.room_id);
    let head = head_branch(&env.repo);
    assert!(
        room.id.starts_with('s') && room.id.len() == 6,
        "{}",
        room.id
    );
    assert_eq!(room.name, "repo");
    assert_eq!(room.task, "look around");
    assert_eq!(room.status, "running");
    assert_eq!(room.badge, 0);
    assert_eq!(room.cwd.as_deref(), Some(env.repo.as_str()));
    assert_eq!(room.branch.as_deref(), Some(head.as_str()));
    assert_eq!(room.repo.as_deref(), Some("repo"));
    assert_eq!(room.repo_root.as_deref(), Some(env.repo.as_str()));
    assert_eq!(room.attention, Some(true));
    assert_eq!(room.repo_identity, None);
    let by = room.created_by.expect("created_by");
    assert_eq!((by.room_id.as_str(), by.harness_id.as_str()), ("r1", "h1"));
    assert_eq!((by.prompt_first_line, by.base_sha), (None, None));
    assert_eq!(room.harnesses.len(), 1);
    let h = &room.harnesses[0];
    assert_eq!(room.active_harness_id, h.id);
    assert_eq!((h.name.as_str(), h.kind.as_str()), ("main", "byoh"));
    assert_eq!(h.cwd.as_deref(), Some(env.repo.as_str()));
    assert!(h.cmd.is_some());
    assert_eq!(h.created_by, None, "the harness carries no attribution");
    assert_eq!(out.harness_id, h.id);
}

#[tokio::test]
async fn worktree_mode_cuts_a_worktree_on_the_derived_branch() {
    let env = git_env();
    let out = create(&env, &create_room_args("Do the thing!"))
        .await
        .unwrap();

    let room = stored(&env, &out.room_id);
    let expected_cwd = env.tmp.path().join("repo-wt").join("do-the-thing");
    let cwd = room.cwd.clone().unwrap();
    assert_eq!(
        std::path::Path::new(&cwd).canonicalize().unwrap(),
        expected_cwd.canonicalize().unwrap()
    );
    assert_eq!(room.branch.as_deref(), Some("skein/do-the-thing"));
    assert_eq!(head_branch(&cwd), "skein/do-the-thing");
    assert_eq!(room.repo_root.as_deref(), Some(env.repo.as_str()));
    assert_eq!(room.name, "do-the-thing");
    assert_eq!(room.repo.as_deref(), Some("do-the-thing"));
    // Cut from the repository's HEAD, and the room remembers which commit.
    let base = head_sha(&env.repo);
    assert_eq!(head_sha(&cwd), base);
    assert_eq!(
        room.created_by.unwrap().base_sha.as_deref(),
        Some(base.as_str())
    );
    assert_eq!(room.harnesses[0].cwd.as_deref(), Some(cwd.as_str()));
    assert_eq!(out.cwd.as_deref(), Some(cwd.as_str()));
    assert_eq!(out.branch.as_deref(), Some("skein/do-the-thing"));
    assert_eq!(out.base_behind_upstream, None);
}

#[tokio::test]
async fn an_explicit_branch_and_base_branch_are_used() {
    let env = git_env();
    let first = head_sha(&env.repo);
    {
        let repo = git2::Repository::open(&env.repo).unwrap();
        let commit = repo
            .find_commit(git2::Oid::from_str(&first).unwrap())
            .unwrap();
        repo.branch("older", &commit, false).unwrap();
    }
    commit_file(env.repo_path().as_path(), "b.txt", "two\n", "second");
    assert_ne!(head_sha(&env.repo), first);

    let args = verbs::CreateRoomArgs {
        branch: Some(" agent/explicit ".to_owned()),
        base_branch: Some("older".to_owned()),
        ..create_room_args("whatever")
    };
    let out = create(&env, &args).await.unwrap();

    let room = stored(&env, &out.room_id);
    let cwd = room.cwd.clone().unwrap();
    assert_eq!(room.branch.as_deref(), Some("agent/explicit"));
    assert_eq!(head_sha(&cwd), first, "cut from the base branch, not HEAD");
    assert_eq!(
        room.created_by.unwrap().base_sha.as_deref(),
        Some(first.as_str())
    );
    assert!(
        std::path::Path::new(&cwd).ends_with("explicit"),
        "the folder is named after the branch's last segment: {cwd}"
    );
}

#[tokio::test]
async fn a_base_behind_its_upstream_is_reported() {
    let env = git_env();
    let first = head_sha(&env.repo);
    let branch = head_branch(&env.repo);
    commit_file(env.repo_path().as_path(), "b.txt", "two\n", "second");
    let second = head_sha(&env.repo);
    {
        // The upstream is one commit ahead of the local branch.
        let repo = git2::Repository::open(&env.repo).unwrap();
        repo.reference(
            &format!("refs/remotes/origin/{branch}"),
            git2::Oid::from_str(&second).unwrap(),
            true,
            "test",
        )
        .unwrap();
        repo.reference(
            &format!("refs/heads/{branch}"),
            git2::Oid::from_str(&first).unwrap(),
            true,
            "test",
        )
        .unwrap();
        let mut cfg = repo.config().unwrap();
        cfg.set_str(&format!("branch.{branch}.remote"), "origin")
            .unwrap();
        cfg.set_str(
            &format!("branch.{branch}.merge"),
            &format!("refs/heads/{branch}"),
        )
        .unwrap();
        cfg.set_str("remote.origin.url", "https://example.invalid/x.git")
            .unwrap();
        cfg.set_str("remote.origin.fetch", "+refs/heads/*:refs/remotes/origin/*")
            .unwrap();
    }
    let out = create(&env, &create_room_args("catch up")).await.unwrap();
    assert_eq!(out.base_behind_upstream, Some(1));
    let wire = serde_json::to_value(&out).unwrap();
    assert_eq!(wire["baseBehindUpstream"], json!(1));
}

#[tokio::test]
async fn the_answer_has_the_same_keys_as_before() {
    let env = git_env();
    let args = verbs::CreateRoomArgs {
        kind: Some("byoh".to_owned()),
        ..create_room_args("shape")
    };
    let out = create(&env, &args).await.unwrap();
    let wire = serde_json::to_value(&out).unwrap();
    let mut keys: Vec<&str> = wire
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    // `sessionId` is absent for a kind with no session; `agent`,
    // `messageId` and `baseBehindUpstream` are absent when unset.
    assert_eq!(
        keys,
        [
            "branch",
            "cwd",
            "harnessId",
            "kind",
            "name",
            "repo",
            "roomId"
        ]
    );
    let claude = create(&env, &create_room_args("shape two")).await.unwrap();
    let wire = serde_json::to_value(&claude).unwrap();
    assert!(
        wire["sessionId"].is_string(),
        "claude gets a session id: {wire}"
    );
    assert_eq!(wire["kind"], json!("claude"));
}

#[tokio::test]
async fn refusals_create_nothing() {
    let env = git_env();
    let first = head_branch(&env.repo);
    let wt_dir = env.tmp.path().join("repo-wt").join("taken");
    std::fs::create_dir_all(&wt_dir).unwrap();
    let cases: Vec<(verbs::CreateRoomArgs, &str)> = vec![
        (
            verbs::CreateRoomArgs {
                base_branch: Some("nope".to_owned()),
                ..create_room_args("x")
            },
            "unknown base branch",
        ),
        (
            verbs::CreateRoomArgs {
                branch: Some(first),
                ..create_room_args("x")
            },
            "branch already exists",
        ),
        (
            verbs::CreateRoomArgs {
                branch: Some("a..b".to_owned()),
                ..create_room_args("x")
            },
            "can't contain ..",
        ),
        (
            verbs::CreateRoomArgs {
                branch: Some("feature/taken".to_owned()),
                ..create_room_args("x")
            },
            "worktree folder already exists",
        ),
        (
            verbs::CreateRoomArgs {
                kind: Some("remote".to_owned()),
                ..create_room_args("x")
            },
            "remote harnesses need a host",
        ),
    ];
    for (n, (args, expected)) in cases.into_iter().enumerate() {
        // Five cases stay within the rate cap, refused or not.
        let err = create(&env, &args).await.unwrap_err();
        assert!(
            refusal(&err).contains(expected),
            "case {n}: wanted {expected:?}, got {err:?}"
        );
    }
    assert_eq!(env.room_ids(), vec!["r1".to_owned()]);
    let worktrees = std::fs::read_dir(env.tmp.path().join("repo-wt"))
        .unwrap()
        .count();
    assert_eq!(worktrees, 1, "only the pre-existing folder is there");
}

#[tokio::test]
async fn a_prompt_is_queued_for_the_new_harness() {
    let env = git_env();
    let args = verbs::CreateRoomArgs {
        prompt: Some("\n  start working on the thing \nsecond line".to_owned()),
        ..create_room_args("do the thing")
    };
    let out = create(&env, &args).await.unwrap();
    let message_id = out.message_id.clone().expect("a prompt queues a message");

    let messages = env
        .f
        .db
        .all_harness_messages(&out.room_id, &out.harness_id)
        .unwrap();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].id, message_id);
    assert_eq!(
        messages[0].body,
        "\n  start working on the thing \nsecond line"
    );
    assert_eq!(messages[0].from_room_id, "r1");
    assert_eq!(messages[0].from_harness_id.as_deref(), Some("h1"));
    assert!(messages[0].read_ms.is_none());

    let by = stored(&env, &out.room_id).created_by.unwrap();
    assert_eq!(
        by.prompt_first_line.as_deref(),
        Some("start working on the thing")
    );
}

#[tokio::test]
async fn no_prompt_queues_nothing() {
    let env = git_env();
    let out = create(&env, &create_room_args("quiet")).await.unwrap();
    assert_eq!(out.message_id, None);
    assert_eq!(
        env.f
            .db
            .all_harness_messages(&out.room_id, &out.harness_id)
            .unwrap()
            .len(),
        0
    );
}

#[tokio::test]
async fn a_prompt_with_messaging_off_refuses_before_anything_is_created() {
    let env = git_env();
    let mail = MailContext {
        policy: MailPolicy {
            messaging_enabled: false,
            ..MailPolicy::permissive()
        },
        ..MailContext::permissive()
    };
    let args = verbs::CreateRoomArgs {
        prompt: Some("go".to_owned()),
        ..create_room_args("hi")
    };
    let err = verbs::create_room(&env.state, &env.caller, &args, &mail, true)
        .await
        .unwrap_err();
    assert!(
        refusal(&err).contains("Mail tools are turned off"),
        "{err:?}"
    );
    assert_eq!(env.room_ids(), vec!["r1".to_owned()]);
}

#[tokio::test]
async fn a_prompt_for_an_unreachable_kind_refuses_before_the_worktree_exists() {
    // `copilot` has no MCP connection, so a queued prompt could never be
    // delivered.
    let env = git_env();
    let args = verbs::CreateRoomArgs {
        kind: Some("copilot".to_owned()),
        prompt: Some("go".to_owned()),
        ..create_room_args("hi")
    };
    let err = create(&env, &args).await.unwrap_err();
    assert!(refusal(&err).contains("copilot"), "{err:?}");
    assert_eq!(env.room_ids(), vec!["r1".to_owned()]);
    assert!(
        !env.tmp.path().join("repo-wt").exists(),
        "no worktree was cut"
    );
}

#[tokio::test]
async fn the_caller_without_a_harness_is_recorded_with_an_empty_harness_id() {
    let env = git_env();
    let caller = caller_for(&env.f.db, "r1", None);
    assert_eq!(caller.harness_id, None);
    let out = verbs::create_room(
        &env.state,
        &caller,
        &current_mode("anon"),
        &MailContext::permissive(),
        true,
    )
    .await
    .unwrap();
    let by = stored(&env, &out.room_id).created_by.unwrap();
    assert_eq!((by.room_id.as_str(), by.harness_id.as_str()), ("r1", ""));
}

#[tokio::test]
async fn the_created_event_is_emitted_after_the_room_is_stored() {
    let env = git_env();
    let out = create(&env, &current_mode("announce")).await.unwrap();
    let events = env.captured.named("skein://agent-room-created");
    assert_eq!(
        events,
        vec![json!({
            "roomId": out.room_id,
            "roomName": "repo",
            "harnessName": "main",
            "harnessId": out.harness_id,
            "kind": out.kind,
            "requesterRoomName": "room r1",
        })]
    );
    assert_eq!(*env.captured.room_was_stored.lock().unwrap(), vec![true]);
}

#[tokio::test]
async fn a_refusal_emits_no_created_event() {
    let env = git_env();
    let args = verbs::CreateRoomArgs {
        base_branch: Some("nope".to_owned()),
        ..create_room_args("x")
    };
    create(&env, &args).await.unwrap_err();
    assert_eq!(env.captured.named("skein://agent-room-created").len(), 0);
}

#[tokio::test]
async fn the_new_rooms_token_can_be_minted_and_authenticates() {
    let env = git_env();
    let out = create(&env, &current_mode("token")).await.unwrap();
    let token = env.f.db.ensure_room_token(&out.room_id, 5).unwrap();
    let caller =
        crate::agent_api::auth::authenticate(&env.f.db, Some(&token), Some(&out.harness_id))
            .expect("the new room's token authenticates");
    assert_eq!(caller.room_id, out.room_id);
    assert_eq!(caller.harness_id.as_deref(), Some(out.harness_id.as_str()));
}

#[tokio::test]
async fn the_store_is_required() {
    let f = super::tests::fixture();
    let tmp = tempfile::TempDir::new().unwrap();
    let repo = super::create_room_tests::init_repo(&tmp);
    let mut r1 = super::tests::room("r1", vec![super::tests::harness("h1", "claude", "main")]);
    r1.cwd = Some(repo);
    super::tests::save(&f.db, &[r1]);
    let state = super::tests::agent_api_state(&f);
    let caller = caller_for(&f.db, "r1", Some("h1"));
    let err = verbs::create_room(
        &state,
        &caller,
        &current_mode("no store"),
        &MailContext::permissive(),
        true,
    )
    .await
    .unwrap_err();
    assert!(matches!(err, VerbError::Unavailable(_)), "{err:?}");
}
