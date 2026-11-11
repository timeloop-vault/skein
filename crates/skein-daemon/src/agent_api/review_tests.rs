//! The review verbs: `review_status` and the sign-off it carries, replies, `mark_addressed`, and listing.

use serde_json::Value;

use super::auth::Caller;
use super::mcp;
use super::state::AgentApiState;
use super::tests::{agent_api_state, caller_for, fixture, harness, room, save, seed_thread};
use super::verbs::{self, AddressedArgs, DiffArgs, ListArgs, MailContext, ReplyArgs, VerbError};
use crate::db::ReviewCommentRow;

#[tokio::test]
async fn review_status_carries_the_signoff_and_its_staleness_to_the_agent() {
    // The verb exists so the agent can gate landing on it, so the
    // answer has to survive the round trip through MCP — including the
    // stale case, which is the one that must never read as approved.
    let f = fixture();
    let tmp = tempfile::TempDir::new().unwrap();
    let cwd = tmp.path().to_str().unwrap().to_owned();
    git_repo_with_commit(tmp.path());

    let mut r = room("r1", vec![harness("h1", "claude", "main")]);
    r.cwd = Some(cwd.clone());
    save(&f.db, &[r]);
    let caller = caller_for(&f.db, "r1", Some("h1"));
    let state = agent_api_state(&f);

    let before = call_review_status(&state, &caller).await;
    assert_eq!(before["approved"], serde_json::json!(false));
    assert_eq!(before["stale"], serde_json::json!(false));
    assert!(
        before["guidance"]
            .as_str()
            .unwrap()
            .contains("not signed off"),
        "{before}"
    );

    crate::review_surface::signoff::set_impl(&f.db, "r1", &cwd, true, None, 1_000).unwrap();
    let approved = call_review_status(&state, &caller).await;
    assert_eq!(approved["approved"], serde_json::json!(true));
    assert!(
        approved["guidance"].as_str().unwrap().contains("may land"),
        "{approved}"
    );

    // The agent commits. Its own clearance has to lapse.
    commit_file(tmp.path(), "b.txt", "more\n", "feat: more");
    let after = call_review_status(&state, &caller).await;
    assert_eq!(after["approved"], serde_json::json!(false));
    assert_eq!(after["stale"], serde_json::json!(true));
    assert!(
        after["guidance"].as_str().unwrap().contains("do not land"),
        "{after}"
    );
    assert_ne!(after["approved_sha"], after["head_sha"]);
}

#[test]
fn a_room_with_no_worktree_says_so_rather_than_answering_unapproved() {
    // "Not approved" and "there is nothing here to approve" are
    // different answers, and an agent acting on the first would wait
    // forever for a sign-off nobody can grant.
    let f = fixture();
    save(&f.db, &[room("r1", vec![harness("h1", "claude", "main")])]);
    let caller = caller_for(&f.db, "r1", Some("h1"));
    let err = verbs::review_status(&f.db, &caller).unwrap_err();
    assert!(
        matches!(&err, VerbError::Unavailable(m) if m.contains("no worktree")),
        "got {err:?}"
    );
}

async fn call_review_status(state: &AgentApiState, caller: &Caller) -> Value {
    mcp::call_tool(
        state,
        caller,
        "review_status",
        &serde_json::json!({}),
        &MailContext::permissive(),
    )
    .await
    .unwrap()
}

/// A repository with one commit.
///
/// **git2, never a spawned `git`.** The pre-commit hook runs these
/// tests with `GIT_DIR` exported, and a spawned git inherits it: `git
/// init` reinitialises Skein's own repository, `git config` writes to
/// its config, and `git commit` commits into the branch under test.
/// That is not hypothetical — it happened once while writing this file.
pub(super) fn git_repo_with_commit(dir: &std::path::Path) {
    git2::Repository::init(dir).unwrap();
    commit_file(dir, "a.txt", "one\n", "init");
}

pub(super) fn commit_file(dir: &std::path::Path, name: &str, body: &str, msg: &str) {
    std::fs::write(dir.join(name), body).unwrap();
    let repo = git2::Repository::open(dir).unwrap();
    let mut index = repo.index().unwrap();
    index.add_path(std::path::Path::new(name)).unwrap();
    index.write().unwrap();
    let tree = repo.find_tree(index.write_tree().unwrap()).unwrap();
    let sig = git2::Signature::now("t", "t@example.com").unwrap();
    let parents = repo
        .head()
        .ok()
        .and_then(|h| h.peel_to_commit().ok())
        .map(|c| vec![c])
        .unwrap_or_default();
    let refs: Vec<&git2::Commit> = parents.iter().collect();
    repo.commit(Some("HEAD"), &sig, &sig, msg, &tree, &refs)
        .unwrap();
}

// ── replies and attribution ───────────────────────────────────────

#[test]
fn a_reply_carries_the_harness_that_wrote_it() {
    let f = fixture();
    save(&f.db, &[room("r1", vec![harness("h1", "claude", "main")])]);
    seed_thread(&f.db, "r1", "t1", "why is this here?");
    let caller = caller_for(&f.db, "r1", Some("h1"));

    let out = verbs::reply(
        &f.db,
        &caller,
        &ReplyArgs {
            thread_id: "t1".to_owned(),
            body: "because of the ordering below".to_owned(),
        },
    )
    .unwrap();
    assert_eq!(out.author, "claude · main");

    let stored = f.db.review_comments_for_room("r1").unwrap();
    let agent = stored.iter().find(|c| c.author_kind == "agent").unwrap();
    assert_eq!(agent.author_id.as_deref(), Some("h1"));
    assert_eq!(agent.body, "because of the ordering below");
}

#[test]
fn an_empty_reply_is_refused_before_it_reaches_the_database() {
    let f = fixture();
    save(&f.db, &[room("r1", vec![])]);
    seed_thread(&f.db, "r1", "t1", "hmm");
    let caller = caller_for(&f.db, "r1", None);
    let err = verbs::reply(
        &f.db,
        &caller,
        &ReplyArgs {
            thread_id: "t1".to_owned(),
            body: "   \n".to_owned(),
        },
    )
    .unwrap_err();
    assert!(matches!(err, VerbError::Refused(_)));
    assert_eq!(f.db.review_comments_for_room("r1").unwrap().len(), 1);
}

#[test]
fn two_harnesses_in_one_room_can_both_answer_the_same_thread() {
    let f = fixture();
    save(
        &f.db,
        &[room(
            "r1",
            vec![
                harness("h1", "claude", "main"),
                harness("h2", "opencode", "second"),
            ],
        )],
    );
    seed_thread(&f.db, "r1", "t1", "two of you are in here");

    // Both harnesses share the room's one token — the token says which
    // room, the header says which harness.
    let a = caller_for(&f.db, "r1", Some("h1"));
    let b = caller_for(&f.db, "r1", Some("h2"));
    verbs::reply(
        &f.db,
        &a,
        &ReplyArgs {
            thread_id: "t1".to_owned(),
            body: "from claude".to_owned(),
        },
    )
    .unwrap();
    verbs::reply(
        &f.db,
        &b,
        &ReplyArgs {
            thread_id: "t1".to_owned(),
            body: "from opencode".to_owned(),
        },
    )
    .unwrap();

    let listed = verbs::list_comments(&f.db, &a, &ListArgs::default()).unwrap();
    let thread = &listed.threads[0];
    assert_eq!(thread.comments.len(), 3);
    assert_eq!(thread.comments[0].author, "reviewer");
    let authors: Vec<&str> = thread.comments[1..]
        .iter()
        .map(|c| c.author.as_str())
        .collect();
    assert!(authors.contains(&"claude · main"));
    assert!(authors.contains(&"opencode · second"));
}

// ── addressed ─────────────────────────────────────────────────────

#[test]
fn mark_addressed_is_a_claim_and_a_reviewer_reply_withdraws_it() {
    let f = fixture();
    save(&f.db, &[room("r1", vec![harness("h1", "claude", "main")])]);
    seed_thread(&f.db, "r1", "t1", "this needs a test");
    let caller = caller_for(&f.db, "r1", Some("h1"));

    let out = verbs::mark_addressed(
        &f.db,
        &caller,
        &AddressedArgs {
            thread_id: "t1".to_owned(),
            commit_sha: Some("abc1234".to_owned()),
            note: Some("added a table test".to_owned()),
        },
    )
    .unwrap();
    assert!(
        out.note_to_agent.contains("reviewer"),
        "every mark_addressed answer restates who closes threads"
    );
    // Claimed, but emphatically not resolved.
    assert!(
        f.db.review_thread("t1")
            .unwrap()
            .unwrap()
            .resolved_ms
            .is_none()
    );

    let listed = verbs::list_comments(&f.db, &caller, &ListArgs::default()).unwrap();
    let addressed = listed.threads[0].addressed.as_ref().unwrap();
    assert_eq!(addressed.commit_sha.as_deref(), Some("abc1234"));
    assert_eq!(addressed.by, "claude · main");

    // The reviewer comes back with more to say: the claim goes.
    f.db.insert_review_comment(&ReviewCommentRow {
        id: "c-user-2".to_owned(),
        thread_id: "t1".to_owned(),
        room_id: "r1".to_owned(),
        author_kind: "user".to_owned(),
        author_id: None,
        body: "not quite — the edge case is still open".to_owned(),
        created_ms: 5,
        updated_ms: 5,
    })
    .unwrap();
    assert_eq!(f.db.addressed_for_room("r1").unwrap(), Vec::new());

    // An agent reply, by contrast, leaves an existing claim standing.
    verbs::mark_addressed(
        &f.db,
        &caller,
        &AddressedArgs {
            thread_id: "t1".to_owned(),
            commit_sha: None,
            note: None,
        },
    )
    .unwrap();
    verbs::reply(
        &f.db,
        &caller,
        &ReplyArgs {
            thread_id: "t1".to_owned(),
            body: "fixed the edge case too".to_owned(),
        },
    )
    .unwrap();
    assert_eq!(f.db.addressed_for_room("r1").unwrap().len(), 1);
}

// ── listing ───────────────────────────────────────────────────────

#[test]
fn listing_defaults_to_what_is_still_open() {
    let f = fixture();
    save(&f.db, &[room("r1", vec![])]);
    seed_thread(&f.db, "r1", "t-open", "still open");
    seed_thread(&f.db, "r1", "t-done", "settled");
    f.db.set_review_thread_resolved("t-done", Some(7), 7)
        .unwrap();
    let caller = caller_for(&f.db, "r1", None);

    let default = verbs::list_comments(&f.db, &caller, &ListArgs::default()).unwrap();
    assert_eq!(default.threads.len(), 1);
    assert_eq!(default.threads[0].thread_id, "t-open");
    assert_eq!(default.unresolved_total, 1);

    let all = verbs::list_comments(
        &f.db,
        &caller,
        &ListArgs {
            status: Some("all".to_owned()),
            file: None,
        },
    )
    .unwrap();
    assert_eq!(all.threads.len(), 2);
    // The count is the room's, not the filter's — it is the number the
    // agent is being measured against.
    assert_eq!(all.unresolved_total, 1);

    let elsewhere = verbs::list_comments(
        &f.db,
        &caller,
        &ListArgs {
            status: None,
            file: Some("src/nothing.rs".to_owned()),
        },
    )
    .unwrap();
    assert_eq!(elsewhere.threads, Vec::new());
}

#[test]
fn a_room_with_no_worktree_says_so_instead_of_returning_an_empty_diff() {
    let f = fixture();
    save(&f.db, &[room("r1", vec![])]);
    let caller = caller_for(&f.db, "r1", None);
    let err = verbs::get_diff(&f.db, &caller, &DiffArgs::default()).unwrap_err();
    assert!(matches!(err, VerbError::Unavailable(_)));
    assert!(err.message().contains("folder"));
}

#[test]
fn a_file_in_a_non_git_room_is_not_in_the_diff_rather_than_unavailable() {
    let f = fixture();
    let dir = tempfile::TempDir::new().unwrap();
    let mut r = room("r1", vec![]);
    r.cwd = Some(dir.path().to_str().unwrap().to_owned());
    save(&f.db, &[r]);
    let caller = caller_for(&f.db, "r1", None);
    let err = verbs::get_diff(
        &f.db,
        &caller,
        &DiffArgs {
            file: Some("a.txt".to_owned()),
            scope: None,
            commit_sha: None,
        },
    )
    .unwrap_err();
    assert!(matches!(err, VerbError::NotFound(_)), "{err:?}");
    assert!(err.message().contains("a.txt is not in this review's diff"));
}

#[test]
fn the_commit_scope_refuses_to_guess_which_commit() {
    let f = fixture();
    let mut r = room("r1", vec![]);
    r.cwd = Some("C:/nowhere".to_owned());
    save(&f.db, &[r]);
    let caller = caller_for(&f.db, "r1", None);
    let err = verbs::get_diff(
        &f.db,
        &caller,
        &DiffArgs {
            file: None,
            scope: Some("commit".to_owned()),
            commit_sha: None,
        },
    )
    .unwrap_err();
    assert!(matches!(err, VerbError::Refused(_)));

    let unknown = verbs::get_diff(
        &f.db,
        &caller,
        &DiffArgs {
            file: None,
            scope: Some("sideways".to_owned()),
            commit_sha: None,
        },
    )
    .unwrap_err();
    assert!(unknown.message().contains("branch"));
}
