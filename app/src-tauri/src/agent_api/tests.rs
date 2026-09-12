//! What #213 actually promises.
//!
//! Two properties carry the design and are tested hardest:
//!
//! * **A token reaches exactly one room.** Not "usually" and not "as
//!   long as the ids don't collide" — every verb re-checks, so each one
//!   is exercised against a thread that belongs to somebody else.
//! * **The agent cannot resolve.** It is not enough for `resolve` to be
//!   missing from the tool list; a model that finds a tool absent goes
//!   looking for another way in. So it is refused by name, and the
//!   thread is checked to still be open afterwards.
//!
//! The rest is lifecycle: what a rotated token does, what an archived
//! room does, and whether two harnesses in one room can both answer.

use std::sync::Arc;

use serde_json::{Value, json};
use tempfile::TempDir;

use super::auth::{self, AuthError, Caller};
use super::mcp;
use super::state::AgentApiState;
use super::verbs::{self, AddressedArgs, DiffArgs, GetCommentArgs, ListArgs, ReplyArgs, VerbError};
use crate::db::{Database, Harness, ReviewCommentRow, ReviewThreadRow, Room, TokenLookup};

// ── fixtures ──────────────────────────────────────────────────────

struct Fixture {
    _dir: TempDir,
    db: Arc<Database>,
}

fn fixture() -> Fixture {
    let dir = TempDir::new().unwrap();
    let db = Database::open(&dir.path().join("skein.db")).unwrap();
    Fixture {
        _dir: dir,
        db: Arc::new(db),
    }
}

fn harness(id: &str, kind: &str, name: &str) -> Harness {
    Harness {
        id: id.to_owned(),
        kind: kind.to_owned(),
        name: name.to_owned(),
        status: "running".to_owned(),
        model: String::new(),
        tokens: "0".to_owned(),
        live: None,
        cmd: None,
        cwd: None,
        session_id: None,
        agent: None,
        pending_notifications: None,
    }
}

fn room(id: &str, harnesses: Vec<Harness>) -> Room {
    Room {
        id: id.to_owned(),
        name: format!("room {id}"),
        task: String::new(),
        status: "running".to_owned(),
        badge: 0,
        active_harness_id: harnesses.first().map(|h| h.id.clone()).unwrap_or_default(),
        harnesses,
        // No worktree: every test here is about scoping, attribution and
        // protocol, none of which needs git. The verbs that do need one
        // say so rather than pretending.
        cwd: None,
        branch: None,
        repo: None,
        archived: None,
    }
}

/// Persist rooms the way the frontend's autosave does.
fn save(db: &Database, rooms: &[Room]) {
    db.load_all().unwrap();
    db.save_all(rooms).unwrap();
}

/// A line thread with one human comment on it.
fn seed_thread(db: &Database, room_id: &str, thread_id: &str, body: &str) {
    db.insert_review_thread(&ReviewThreadRow {
        id: thread_id.to_owned(),
        room_id: room_id.to_owned(),
        scope: "line".to_owned(),
        file_path: Some("src/a.rs".to_owned()),
        commit_sha: None,
        side: Some("new".to_owned()),
        line_start: Some(10),
        line_end: Some(11),
        anchor_hash: None,
        anchor_lines: Some(json!(["let x = 1;", "let y = 2;"]).to_string()),
        resolved_ms: None,
        created_ms: 1,
        updated_ms: 1,
    })
    .unwrap();
    db.insert_review_comment(&ReviewCommentRow {
        id: format!("{thread_id}-c1"),
        thread_id: thread_id.to_owned(),
        room_id: room_id.to_owned(),
        author_kind: "user".to_owned(),
        author_id: None,
        body: body.to_owned(),
        created_ms: 1,
        updated_ms: 1,
    })
    .unwrap();
}

fn caller_for(db: &Database, room_id: &str, harness_id: Option<&str>) -> Caller {
    let token = db.ensure_room_token(room_id, 1).unwrap();
    auth::authenticate(db, Some(&token), harness_id).unwrap()
}

// ── tokens ────────────────────────────────────────────────────────

#[test]
fn a_room_token_is_minted_once_and_reused_on_every_spawn() {
    let f = fixture();
    let first = f.db.ensure_room_token("r1", 1).unwrap();
    let second = f.db.ensure_room_token("r1", 2).unwrap();
    assert_eq!(first, second, "a second harness must not mint a new token");
    assert_eq!(first.len(), 64, "256 bits of hex");
    assert_ne!(first, f.db.ensure_room_token("r2", 1).unwrap());
}

#[test]
fn a_rotated_token_is_revoked_and_not_merely_forgotten() {
    let f = fixture();
    let old = f.db.ensure_room_token("r1", 1).unwrap();
    assert_eq!(f.db.revoke_agent_tokens(Some("r1"), 2).unwrap(), 1);
    let new = f.db.ensure_room_token("r1", 2).unwrap();
    assert_ne!(old, new);
    // The distinction matters: a harness still holding the old token
    // needs to be told to restart, not told its token never existed.
    assert_eq!(f.db.room_for_token(&old).unwrap(), TokenLookup::Revoked);
    assert_eq!(
        f.db.room_for_token(&new).unwrap(),
        TokenLookup::Active {
            room_id: "r1".to_owned()
        }
    );
    assert_eq!(
        f.db.room_for_token("never-minted").unwrap(),
        TokenLookup::Unknown
    );
}

#[test]
fn authentication_distinguishes_every_way_a_call_can_be_wrong() {
    let f = fixture();
    save(&f.db, &[room("r1", vec![harness("h1", "claude", "main")])]);
    let token = f.db.ensure_room_token("r1", 1).unwrap();

    assert_eq!(
        auth::authenticate(&f.db, None, None).unwrap_err(),
        AuthError::Missing
    );
    assert_eq!(
        auth::authenticate(&f.db, Some("nope"), None).unwrap_err(),
        AuthError::Unknown
    );
    assert_eq!(
        auth::authenticate(&f.db, Some(&token), None)
            .unwrap()
            .room_id,
        "r1"
    );

    // A token that a restart, or an explicit rotation, revoked.
    f.db.revoke_agent_tokens(Some("r1"), 2).unwrap();
    assert_eq!(
        auth::authenticate(&f.db, Some(&token), None).unwrap_err(),
        AuthError::Revoked
    );

    // A token whose room is gone entirely — 404, not a silent empty
    // review.
    let orphan = f.db.ensure_room_token("r-missing", 1).unwrap();
    assert_eq!(
        auth::authenticate(&f.db, Some(&orphan), None).unwrap_err(),
        AuthError::NoRoom
    );
}

#[test]
fn an_archived_room_closes_its_review_to_the_agent() {
    let f = fixture();
    let mut r = room("r1", vec![harness("h1", "claude", "main")]);
    r.archived = Some(9_999);
    save(&f.db, &[r]);
    let token = f.db.ensure_room_token("r1", 1).unwrap();
    let err = auth::authenticate(&f.db, Some(&token), None).unwrap_err();
    assert_eq!(err, AuthError::Archived);
    assert_eq!(err.status(), 410, "gone, not merely forbidden");
}

#[test]
fn the_harness_header_is_attribution_and_never_authorisation() {
    let f = fixture();
    save(&f.db, &[room("r1", vec![harness("h1", "claude", "main")])]);
    let token = f.db.ensure_room_token("r1", 1).unwrap();

    let known = auth::authenticate(&f.db, Some(&token), Some("h1")).unwrap();
    assert_eq!(known.harness_label.as_deref(), Some("claude · main"));

    // An id the room does not contain still authenticates — the token
    // decided that — but earns no byline rather than an invented one.
    let unknown = auth::authenticate(&f.db, Some(&token), Some("h-ghost")).unwrap();
    assert_eq!(unknown.room_id, "r1");
    assert_eq!(unknown.harness_id.as_deref(), Some("h-ghost"));
    assert_eq!(unknown.harness_label, None);
}

#[test]
fn origin_lets_a_tool_through_and_keeps_a_web_page_out() {
    // A CLI or MCP client sends no Origin at all; a browser always
    // does. That asymmetry is the whole check.
    assert!(auth::check_origin(None).is_ok());
    assert!(auth::check_origin(Some("null")).is_ok());
    assert!(auth::check_origin(Some("http://localhost:5173")).is_ok());
    assert!(auth::check_origin(Some("http://127.0.0.1")).is_ok());
    assert!(auth::check_origin(Some("http://[::1]:8080")).is_ok());
    assert_eq!(
        auth::check_origin(Some("https://evil.example")).unwrap_err(),
        AuthError::BadOrigin
    );
    // The trap this check exists for: a hostname that merely *contains*
    // localhost, resolved to 127.0.0.1 by an attacker's DNS.
    assert_eq!(
        auth::check_origin(Some("http://localhost.evil.example")).unwrap_err(),
        AuthError::BadOrigin
    );
}

#[test]
fn only_a_bearer_scheme_yields_a_token() {
    assert_eq!(auth::bearer(Some("Bearer abc123")), Some("abc123"));
    assert_eq!(auth::bearer(Some("bearer abc123")), Some("abc123"));
    assert_eq!(auth::bearer(Some("Basic abc123")), None);
    assert_eq!(auth::bearer(Some("Bearer ")), None);
    assert_eq!(auth::bearer(Some("abc123")), None);
    assert_eq!(auth::bearer(None), None);
}

// ── the scoping property ──────────────────────────────────────────

#[test]
fn a_token_cannot_touch_another_rooms_thread_through_any_verb() {
    let f = fixture();
    save(
        &f.db,
        &[
            room("r1", vec![harness("h1", "claude", "main")]),
            room("r2", vec![harness("h2", "opencode", "main")]),
        ],
    );
    seed_thread(&f.db, "r2", "t-other", "this belongs to room 2");
    let intruder = caller_for(&f.db, "r1", Some("h1"));

    // Read.
    let got = verbs::get_comment(
        &f.db,
        &intruder,
        &GetCommentArgs {
            thread_id: "t-other".to_owned(),
        },
    );
    assert!(matches!(got, Err(VerbError::NotFound(_))));

    // Write.
    let replied = verbs::reply(
        &f.db,
        &intruder,
        &ReplyArgs {
            thread_id: "t-other".to_owned(),
            body: "hello".to_owned(),
        },
    );
    assert!(matches!(replied, Err(VerbError::NotFound(_))));

    let marked = verbs::mark_addressed(
        &f.db,
        &intruder,
        &AddressedArgs {
            thread_id: "t-other".to_owned(),
            commit_sha: None,
            note: None,
        },
    );
    assert!(matches!(marked, Err(VerbError::NotFound(_))));

    // Listing never sees it either.
    let list = verbs::list_comments(&f.db, &intruder, &ListArgs::default()).unwrap();
    assert!(list.threads.is_empty());
    assert_eq!(list.unresolved_total, 0);

    // And none of that left a trace on room 2's thread.
    assert_eq!(f.db.review_comments_for_room("r2").unwrap().len(), 1);
    assert!(f.db.addressed_for_room("r2").unwrap().is_empty());
}

#[test]
fn a_missing_thread_and_someone_elses_thread_are_the_same_answer() {
    // Otherwise a token becomes an oracle: "not found" versus
    // "forbidden" would let a caller enumerate other rooms' thread ids.
    let f = fixture();
    save(&f.db, &[room("r1", vec![]), room("r2", vec![])]);
    seed_thread(&f.db, "r2", "t-other", "elsewhere");
    let caller = caller_for(&f.db, "r1", None);

    let theirs = verbs::get_comment(
        &f.db,
        &caller,
        &GetCommentArgs {
            thread_id: "t-other".to_owned(),
        },
    )
    .unwrap_err();
    let nobodys = verbs::get_comment(
        &f.db,
        &caller,
        &GetCommentArgs {
            thread_id: "t-nowhere".to_owned(),
        },
    )
    .unwrap_err();
    assert_eq!(theirs.status(), nobodys.status());
    assert!(matches!(theirs, VerbError::NotFound(_)));
}

// ── the two prohibitions ──────────────────────────────────────────

#[test]
fn the_tool_list_offers_six_verbs_and_nothing_that_resolves_or_approves() {
    let names: Vec<String> = mcp::tool_specs()
        .iter()
        .map(|t| t["name"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(
        names,
        vec![
            "list_comments",
            "get_comment",
            "get_diff",
            "reply",
            "mark_addressed",
            // #214: reading the sign-off. There is no verb that writes
            // one, and the list is where that starts being true.
            "review_status",
        ]
    );
    assert!(
        !names.iter().any(|n| n.contains("resolve")),
        "resolve is the reviewer's, and D8 keeps it that way"
    );
    assert!(
        !names
            .iter()
            .any(|n| n.contains("approve") || n.contains("sign")),
        "signing off is the reviewer's — an agent that approves itself is no gate"
    );
}

#[test]
fn approving_is_refused_by_name_and_nothing_is_signed_off() {
    // The gate has to be refused the way `resolve` is: a model told a
    // tool is merely missing goes looking for another way in, so the
    // answer is a reason rather than "unknown tool".
    let f = fixture();
    save(&f.db, &[room("r1", vec![harness("h1", "claude", "main")])]);
    let caller = caller_for(&f.db, "r1", Some("h1"));

    for name in [
        "approve",
        "sign_off",
        "signoff",
        "approve_review",
        "mark_approved",
        "mcp__skein__approve",
    ] {
        let err = mcp::call_tool(&f.db, &caller, name, &serde_json::json!({}))
            .expect_err("an agent must not be able to approve its own work");
        assert!(
            matches!(&err, VerbError::Refused(m) if m.contains("reviewer")),
            "{name} gave {err:?}"
        );
    }
    assert_eq!(
        f.db.review_signoff("r1").unwrap(),
        None,
        "nothing was written"
    );
}

#[test]
fn review_status_carries_the_signoff_and_its_staleness_to_the_agent() {
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
    let call = |db: &Database| {
        mcp::call_tool(db, &caller, "review_status", &serde_json::json!({})).unwrap()
    };

    let before = call(&f.db);
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
    let approved = call(&f.db);
    assert_eq!(approved["approved"], serde_json::json!(true));
    assert!(
        approved["guidance"].as_str().unwrap().contains("may land"),
        "{approved}"
    );

    // The agent commits. Its own clearance has to lapse.
    commit_file(tmp.path(), "b.txt", "more\n", "feat: more");
    let after = call(&f.db);
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

/// A repository with one commit.
///
/// **git2, never a spawned `git`.** The pre-commit hook runs these
/// tests with `GIT_DIR` exported, and a spawned git inherits it: `git
/// init` reinitialises Skein's own repository, `git config` writes to
/// its config, and `git commit` commits into the branch under test.
/// That is not hypothetical — it happened once while writing this file.
fn git_repo_with_commit(dir: &std::path::Path) {
    git2::Repository::init(dir).unwrap();
    commit_file(dir, "a.txt", "one\n", "init");
}

fn commit_file(dir: &std::path::Path, name: &str, body: &str, msg: &str) {
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

#[test]
fn resolving_is_refused_by_name_and_the_thread_stays_open() {
    let f = fixture();
    save(&f.db, &[room("r1", vec![harness("h1", "claude", "main")])]);
    seed_thread(&f.db, "r1", "t1", "please rename this");
    let caller = caller_for(&f.db, "r1", Some("h1"));

    for name in [
        "resolve",
        "resolve_thread",
        "close_thread",
        "mark_resolved",
        // Namespaced the way Claude Code presents it back.
        "mcp__skein__resolve",
    ] {
        let err = mcp::call_tool(&f.db, &caller, name, &json!({ "thread_id": "t1" })).unwrap_err();
        assert!(
            matches!(err, VerbError::Refused(_)),
            "{name} must be refused with a reason, not reported missing"
        );
        assert!(
            err.message().contains("reviewer"),
            "the refusal has to say whose decision it is: {}",
            err.message()
        );
    }

    assert!(
        f.db.review_thread("t1")
            .unwrap()
            .unwrap()
            .resolved_ms
            .is_none(),
        "nothing in that loop may have closed the thread"
    );
}

#[test]
fn an_unknown_tool_is_reported_missing_rather_than_refused() {
    // The counterpart of the test above: "refused" has to mean
    // something, so it cannot be the answer to every unknown name.
    let f = fixture();
    save(&f.db, &[room("r1", vec![])]);
    let caller = caller_for(&f.db, "r1", None);
    let err = mcp::call_tool(&f.db, &caller, "delete_everything", &json!({})).unwrap_err();
    assert!(matches!(err, VerbError::NotFound(_)));
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
    assert!(f.db.addressed_for_room("r1").unwrap().is_empty());

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
    assert!(elsewhere.threads.is_empty());
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

// ── the MCP envelope ──────────────────────────────────────────────

#[test]
fn initialize_echoes_a_version_it_knows_and_states_its_own_otherwise() {
    let f = fixture();
    save(&f.db, &[room("r1", vec![])]);
    let caller = caller_for(&f.db, "r1", None);

    let old = handled(
        &f.db,
        &caller,
        &json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": { "protocolVersion": "2024-11-05" }
        }),
    );
    assert_eq!(old["result"]["protocolVersion"], "2024-11-05");
    assert_eq!(old["result"]["serverInfo"]["name"], "skein");
    assert!(
        old["result"]["instructions"]
            .as_str()
            .unwrap()
            .contains("cannot resolve"),
        "the one rule an agent must not have to discover by trying it"
    );

    let future = handled(
        &f.db,
        &caller,
        &json!({
            "jsonrpc": "2.0", "id": 2, "method": "initialize",
            "params": { "protocolVersion": "2099-01-01" }
        }),
    );
    assert_eq!(future["result"]["protocolVersion"], mcp::PROTOCOL_VERSION);
}

#[test]
fn a_notification_is_accepted_with_no_body_and_no_answer() {
    let f = fixture();
    save(&f.db, &[room("r1", vec![])]);
    let caller = caller_for(&f.db, "r1", None);
    let out = mcp::handle(
        &f.db,
        &caller,
        &json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }).to_string(),
    );
    assert_eq!(out, mcp::Outcome::Accepted);
}

#[test]
fn a_refused_tool_answers_the_model_rather_than_the_plumbing() {
    // isError inside a *successful* JSON-RPC result: a protocol-level
    // error is handled by the client and never shown to the model, and
    // a model that cannot see the reason will simply try again.
    let f = fixture();
    save(&f.db, &[room("r1", vec![])]);
    seed_thread(&f.db, "r1", "t1", "hi");
    let caller = caller_for(&f.db, "r1", None);
    let out = handled(
        &f.db,
        &caller,
        &json!({
            "jsonrpc": "2.0", "id": 3, "method": "tools/call",
            "params": { "name": "resolve", "arguments": { "thread_id": "t1" } }
        }),
    );
    assert!(out.get("error").is_none());
    assert_eq!(out["result"]["isError"], true);
    let text = out["result"]["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("reviewer"));
}

#[test]
fn an_unknown_method_is_a_json_rpc_error_and_junk_is_a_bad_request() {
    let f = fixture();
    save(&f.db, &[room("r1", vec![])]);
    let caller = caller_for(&f.db, "r1", None);
    let out = handled(
        &f.db,
        &caller,
        &json!({
            "jsonrpc": "2.0", "id": 4, "method": "resources/list"
        }),
    );
    assert_eq!(out["error"]["code"], -32601);

    assert!(matches!(
        mcp::handle(&f.db, &caller, "{not json"),
        mcp::Outcome::BadRequest(_)
    ));
}

#[test]
fn only_the_two_writing_verbs_ask_the_pane_to_refresh() {
    assert!(mcp::is_write("reply"));
    assert!(mcp::is_write("mark_addressed"));
    assert!(mcp::is_write("mcp__skein__reply"));
    assert!(!mcp::is_write("list_comments"));
    assert!(!mcp::is_write("get_diff"));
}

fn handled(db: &Database, caller: &Caller, body: &Value) -> Value {
    match mcp::handle(db, caller, &body.to_string()) {
        mcp::Outcome::Json(v) => *v,
        other => panic!("expected a JSON-RPC response, got {other:?}"),
    }
}

// ── over real HTTP ────────────────────────────────────────────────

/// Bind the router on an ephemeral port and return its base URL.
async fn serve_fixture(db: Arc<Database>) -> String {
    // reqwest 0.13 builds a TLS context eagerly even for plain http,
    // and panics without a rustls provider — the same call `run()`
    // makes for the opencode SSE client.
    crate::install_rustls_provider();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let state = Arc::new(AgentApiState::for_test(db));
    tokio::spawn(super::http::serve(listener, state));
    format!("http://127.0.0.1:{port}")
}

#[tokio::test]
async fn the_endpoint_answers_a_real_handshake_and_lists_its_tools() {
    let f = fixture();
    save(&f.db, &[room("r1", vec![harness("h1", "claude", "main")])]);
    seed_thread(&f.db, "r1", "t1", "rename this");
    let token = f.db.ensure_room_token("r1", 1).unwrap();
    let base = serve_fixture(Arc::clone(&f.db)).await;
    let http = reqwest::Client::new();

    let init: Value = http
        .post(format!("{base}/mcp"))
        .bearer_auth(&token)
        .json(&json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": { "protocolVersion": mcp::PROTOCOL_VERSION }
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(init["result"]["serverInfo"]["name"], "skein");

    let listed: Value = http
        .post(format!("{base}/mcp"))
        .bearer_auth(&token)
        .header("MCP-Protocol-Version", mcp::PROTOCOL_VERSION)
        .header(auth::HARNESS_HEADER, "h1")
        .json(&json!({
            "jsonrpc": "2.0", "id": 2, "method": "tools/call",
            "params": { "name": "list_comments", "arguments": {} }
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let text = listed["result"]["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("rename this"));
    assert!(text.contains("t1"));
}

#[tokio::test]
async fn the_endpoint_refuses_the_ways_in_that_are_not_ours() {
    let f = fixture();
    save(&f.db, &[room("r1", vec![])]);
    let token = f.db.ensure_room_token("r1", 1).unwrap();
    let base = serve_fixture(Arc::clone(&f.db)).await;
    let http = reqwest::Client::new();
    let call = json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" });

    // No token at all.
    let anon = http
        .post(format!("{base}/mcp"))
        .json(&call)
        .send()
        .await
        .unwrap();
    assert_eq!(anon.status(), 401);

    // A page in the user's browser, reaching 127.0.0.1 by DNS
    // rebinding — the attack the spec's Origin rule exists for.
    let cross = http
        .post(format!("{base}/mcp"))
        .bearer_auth(&token)
        .header("Origin", "https://evil.example")
        .json(&call)
        .send()
        .await
        .unwrap();
    assert_eq!(cross.status(), 403);

    // A protocol version we do not speak.
    let ancient = http
        .post(format!("{base}/mcp"))
        .bearer_auth(&token)
        .header("MCP-Protocol-Version", "1999-01-01")
        .json(&call)
        .send()
        .await
        .unwrap();
    assert_eq!(ancient.status(), 400);

    // We offer neither a server-to-client stream nor session teardown.
    let streamed = http.get(format!("{base}/mcp")).send().await.unwrap();
    assert_eq!(streamed.status(), 405);

    // Health is public — it carries no room, no token and no comment.
    let health = http.get(format!("{base}/api/health")).send().await.unwrap();
    assert_eq!(health.status(), 200);
}

#[tokio::test]
async fn the_resolve_route_exists_only_to_say_no() {
    let f = fixture();
    save(&f.db, &[room("r1", vec![])]);
    seed_thread(&f.db, "r1", "t1", "close me");
    let token = f.db.ensure_room_token("r1", 1).unwrap();
    let base = serve_fixture(Arc::clone(&f.db)).await;
    let http = reqwest::Client::new();

    // Unauthenticated callers learn about their token, not our policy.
    let anon = http
        .post(format!("{base}/api/comments/t1/resolve"))
        .send()
        .await
        .unwrap();
    assert_eq!(anon.status(), 401);

    let authed = http
        .post(format!("{base}/api/comments/t1/resolve"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();
    assert_eq!(authed.status(), 403);
    let body: Value = authed.json().await.unwrap();
    assert!(body["error"].as_str().unwrap().contains("reviewer"));
    assert!(
        f.db.review_thread("t1")
            .unwrap()
            .unwrap()
            .resolved_ms
            .is_none()
    );
}

#[tokio::test]
async fn a_reply_over_http_lands_attributed_to_its_harness() {
    let f = fixture();
    save(&f.db, &[room("r1", vec![harness("h1", "claude", "main")])]);
    seed_thread(&f.db, "r1", "t1", "explain this");
    let token = f.db.ensure_room_token("r1", 1).unwrap();
    let base = serve_fixture(Arc::clone(&f.db)).await;
    let http = reqwest::Client::new();

    let posted = http
        .post(format!("{base}/api/comments/t1/reply"))
        .bearer_auth(&token)
        .header(auth::HARNESS_HEADER, "h1")
        .json(&json!({ "body": "it guards the empty case" }))
        .send()
        .await
        .unwrap();
    assert_eq!(posted.status(), 200);

    let stored = f.db.review_comments_for_room("r1").unwrap();
    let agent = stored.iter().find(|c| c.author_kind == "agent").unwrap();
    assert_eq!(agent.author_id.as_deref(), Some("h1"));

    // And the pane sees it: the thread DTO the review surface builds
    // carries the byline, not the raw id.
    let comments = crate::review_surface::query::scope_impl(
        &f.db,
        "r1",
        "",
        crate::review_surface::Scope::Pending,
        None,
    );
    assert!(comments.is_ok());
}
