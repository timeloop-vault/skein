//! Tokens and scoping: a room token reaches exactly one room, and every verb re-checks it.

use super::auth::{self, AuthError};
use super::tests::{caller_for, fixture, harness, room, save, seed_thread};
use super::verbs::{self, AddressedArgs, GetCommentArgs, ListArgs, ReplyArgs, VerbError};
use crate::db::TokenLookup;

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
    assert_eq!(list.threads, Vec::new());
    assert_eq!(list.unresolved_total, 0);

    // And none of that left a trace on room 2's thread.
    assert_eq!(f.db.review_comments_for_room("r2").unwrap().len(), 1);
    assert_eq!(f.db.addressed_for_room("r2").unwrap(), Vec::new());
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
