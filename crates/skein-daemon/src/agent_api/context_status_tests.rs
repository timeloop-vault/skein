//! `context_status` (#689): the caller's own context size and the limit,
//! scoped to the caller's own harness.

use std::sync::Arc;

use serde_json::{Value, json};

use super::groups::{ToolGroup, ToolGroups};
use super::http_route_tests::serve_fixture;
use super::mcp;
use super::tests::{Fixture, caller_for, fixture, harness, room, save};
use super::verbs::{self, MailContext, VerbError};

const CLAUDE_USAGE: &str = r#"{"usage":{"input_tokens":1000,"cache_read_input_tokens":500}}"#;

/// Two rooms, one claude harness each, and a turn cost for `h1` only.
fn two_rooms(with_cost: bool) -> Fixture {
    let f = fixture();
    save(
        &f.db,
        &[
            room("r1", vec![harness("h1", "claude", "main")]),
            room("r2", vec![harness("h2", "claude", "other")]),
        ],
    );
    if with_cost {
        for (h, r) in [("h1", "r1"), ("h2", "r2")] {
            f.db.record_harness_action(h, r, 5, "turn_cost", CLAUDE_USAGE, None, Some("claude"))
                .unwrap();
        }
    }
    f
}

fn tokens_of_h1(f: &Fixture) -> u64 {
    let caller = caller_for(&f.db, "r1", Some("h1"));
    verbs::context_status(&f.db, &caller, None)
        .unwrap()
        .context_tokens
        .expect("a turn_cost row exists")
}

#[test]
fn returns_own_tokens_and_the_limit() {
    let f = two_rooms(true);
    let tokens = tokens_of_h1(&f);
    assert!(tokens > 0);
    let caller = caller_for(&f.db, "r1", Some("h1"));
    let out = verbs::context_status(&f.db, &caller, Some(tokens + 1)).unwrap();
    assert_eq!(out.harness_id, "h1");
    assert_eq!(out.context_tokens, Some(tokens));
    assert_eq!(out.limit, Some(tokens + 1));
    let v = serde_json::to_value(&out).unwrap();
    assert_eq!(
        v,
        json!({
            "harness_id": "h1",
            "context_tokens": tokens,
            "limit": tokens + 1,
            "over_limit": false,
        })
    );
}

#[test]
fn limit_is_null_when_unset_and_never_over() {
    let f = two_rooms(true);
    let caller = caller_for(&f.db, "r1", Some("h1"));
    let out = verbs::context_status(&f.db, &caller, None).unwrap();
    assert_eq!(out.limit, None);
    assert!(!out.over_limit);
    assert!(serde_json::to_value(&out).unwrap()["limit"].is_null());
}

#[test]
fn tokens_are_null_without_a_turn_cost_row() {
    let f = two_rooms(false);
    let caller = caller_for(&f.db, "r1", Some("h1"));
    let out = verbs::context_status(&f.db, &caller, Some(1)).unwrap();
    assert_eq!(out.context_tokens, None);
    assert!(!out.over_limit, "no tokens is never over a limit");
    assert!(serde_json::to_value(&out).unwrap()["context_tokens"].is_null());
}

#[test]
fn over_limit_flips_exactly_at_the_limit() {
    let f = two_rooms(true);
    let tokens = tokens_of_h1(&f);
    let caller = caller_for(&f.db, "r1", Some("h1"));
    let at = |limit| {
        verbs::context_status(&f.db, &caller, Some(limit))
            .unwrap()
            .over_limit
    };
    assert!(!at(tokens + 1));
    assert!(at(tokens), "tokens == limit is over");
    assert!(at(tokens - 1));
}

#[test]
fn a_missing_harness_header_is_refused() {
    let f = two_rooms(true);
    let caller = caller_for(&f.db, "r1", None);
    let err = verbs::context_status(&f.db, &caller, Some(1)).unwrap_err();
    assert!(matches!(err, VerbError::Refused(_)), "{err:?}");
    assert_eq!(err.status(), 403);
}

#[test]
fn another_rooms_harness_is_refused_and_leaks_nothing() {
    let f = two_rooms(true);
    // r1's token, claiming to be r2's harness (which has a cost row).
    let caller = caller_for(&f.db, "r1", Some("h2"));
    let err = verbs::context_status(&f.db, &caller, Some(1)).unwrap_err();
    assert!(matches!(err, VerbError::NotFound(_)), "{err:?}");
    assert_eq!(err.status(), 404);
    let msg = err.message();
    assert!(
        !msg.contains("h2") && !msg.chars().any(|c| c.is_ascii_digit()),
        "{msg}"
    );
    // An id that exists nowhere reads exactly the same.
    let ghost = caller_for(&f.db, "r1", Some("nope"));
    let err2 = verbs::context_status(&f.db, &ghost, Some(1)).unwrap_err();
    assert_eq!(err2, err);
}

#[tokio::test]
async fn the_mcp_tool_answers_for_the_caller_and_is_refused_with_directory_off() {
    let f = two_rooms(true);
    let caller = caller_for(&f.db, "r1", Some("h1"));
    let state = super::tests::agent_api_state(&f);
    let v = mcp::call_tool(
        &state,
        &caller,
        "context_status",
        &json!({}),
        &MailContext::permissive(),
    )
    .await
    .unwrap();
    assert_eq!(v["harness_id"], "h1");
    assert!(v["context_tokens"].is_u64());
    assert!(v["limit"].is_null());
    assert_eq!(v["over_limit"], false);

    *state.test_tool_groups.lock() = Some(ToolGroups {
        directory: false,
        ..ToolGroups::default()
    });
    let err = mcp::call_tool(
        &state,
        &caller,
        "context_status",
        &json!({}),
        &MailContext::permissive(),
    )
    .await
    .unwrap_err();
    assert_eq!(err.status(), 403);
    assert!(err.message().contains("turned off in Skein's Settings"));
    assert!(err.message().contains(ToolGroup::Directory.label()));
}

#[tokio::test]
async fn the_http_route_answers_and_refuses_without_a_harness_header() {
    let f = two_rooms(true);
    let token = f.db.ensure_room_token("r1", 1).unwrap();
    let base = serve_fixture(Arc::clone(&f.db)).await;
    let http = reqwest::Client::new();

    let ok = http
        .get(format!("{base}/api/context"))
        .bearer_auth(&token)
        .header("X-Skein-Harness", "h1")
        .send()
        .await
        .unwrap();
    assert_eq!(ok.status(), 200);
    let v = ok.json::<Value>().await.unwrap();
    assert_eq!(v["harness_id"], "h1");
    assert!(v["context_tokens"].is_u64());

    let anon_harness = http
        .get(format!("{base}/api/context"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();
    assert_eq!(anon_harness.status(), 403);

    let foreign = http
        .get(format!("{base}/api/context"))
        .bearer_auth(&token)
        .header("X-Skein-Harness", "h2")
        .send()
        .await
        .unwrap();
    assert_eq!(foreign.status(), 404);

    let no_token = http
        .get(format!("{base}/api/context"))
        .send()
        .await
        .unwrap();
    assert_eq!(no_token.status(), 401);
}
