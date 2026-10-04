//! The refusal-by-name set: the tool list, and every verb the agent must be told no to (approve, resolve, destroy, `archive_room`), plus the dispatched-not-refused counterparts.

use serde_json::{Value, json};

use super::mcp;
use super::tests::{agent_api_state, caller_for, fixture, harness, room, save, seed_thread};
use super::verbs::{self, MailContext, VerbError};

// ── the two prohibitions ──────────────────────────────────────────

#[test]
fn the_tool_list_offers_twenty_two_verbs_and_nothing_that_resolves_approves_or_deletes() {
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
            // #327: the mailbox.
            "send_message",
            "read_messages",
            // #364: the read-only history counterpart to read_messages.
            "message_history",
            // #330: open a whole new room.
            "create_room",
            // #411: archive (only) a room this room created.
            "close_room",
            // #411: add a harness to this room, or one it created.
            "open_harness",
            // #411: stop a harness the way closing its tab does.
            "close_harness",
            // #354/#356: the verbs whose answer is not scoped to the
            // caller's own room.
            "find_rooms_for_path",
            "list_rooms",
            "get_room",
            "list_harnesses",
            // #512: the design pane.
            "list_design_harnesses",
            "get_design_state",
            "open_design_entry",
            "set_design_device",
            "show_element",
            "invoke_element",
            "show_changes",
            // #535: which Skein build this is.
            "skein_info",
        ]
    );
    assert!(
        !names.iter().any(|n| n.contains("resolve")),
        "resolve is the reviewer's, and D8 keeps it that way"
    );
    assert!(
        // "design" (#512) contains "sign"; strip it before looking.
        !names
            .iter()
            .map(|n| n.replace("design", ""))
            .any(|n| n.contains("approve") || n.contains("sign")),
        "signing off is the reviewer's — an agent that approves itself is no gate"
    );
    assert!(
        !names
            .iter()
            .any(|n| n.contains("archive") || n.contains("delete")),
        "destroying a room stays the user's decision, same as DESTROY_ALIASES's \
         remaining refusals; close_room (#411) is the one narrow, guarded exception"
    );
}

#[test]
fn create_room_tool_schema_property_names_match_the_wire_args() {
    // The bug this guards against: the advertised `inputSchema` once
    // named `branch_mode`/`base_branch` — the Rust field names — while
    // `CreateRoomArgs` only ever deserializes the camelCase wire shape.
    // A client that followed the schema faithfully had those two
    // arguments silently vanish rather than erroring.
    let specs = mcp::tool_specs();
    let create_room = specs
        .iter()
        .find(|t| t["name"] == "create_room")
        .expect("create_room must be in the tool list");
    let props = create_room["inputSchema"]["properties"]
        .as_object()
        .expect("inputSchema.properties must be an object");
    let keys: Vec<&str> = props.keys().map(String::as_str).collect();

    assert!(keys.contains(&"branchMode"), "{keys:?}");
    assert!(keys.contains(&"baseBranch"), "{keys:?}");
    assert!(
        !keys.contains(&"branch_mode") && !keys.contains(&"base_branch"),
        "the schema must not advertise the Rust field spelling: {keys:?}"
    );

    // Every advertised property must actually be a field `CreateRoomArgs`
    // accepts — build one object naming all of them (as plain strings;
    // `CreateRoomArgs` validates enum-ish values like `kind`/`branchMode`
    // itself, not serde) and confirm the whole thing parses.
    let mut sample = serde_json::Map::new();
    for key in &keys {
        sample.insert((*key).to_owned(), json!("x"));
    }
    let parsed: Result<verbs::CreateRoomArgs, _> = serde_json::from_value(Value::Object(sample));
    assert!(
        parsed.is_ok(),
        "every advertised schema key must deserialize into CreateRoomArgs: {parsed:?}"
    );

    // And the old, wrong spelling must now fail loudly — `deny_unknown_fields`
    // turns a silently-dropped argument into a refused call.
    let wrong = json!({ "task": "hi", "branch_mode": "current" });
    assert!(
        serde_json::from_value::<verbs::CreateRoomArgs>(wrong).is_err(),
        "an unknown field must be a hard error, not a silent drop"
    );
}

#[test]
fn find_rooms_for_path_tool_schema_matches_the_wire_args() {
    let specs = mcp::tool_specs();
    let tool = specs
        .iter()
        .find(|t| t["name"] == "find_rooms_for_path")
        .expect("find_rooms_for_path must be in the tool list");
    let props = tool["inputSchema"]["properties"]
        .as_object()
        .expect("inputSchema.properties must be an object");
    let keys: Vec<&str> = props.keys().map(String::as_str).collect();
    assert_eq!(keys, vec!["path"]);
    assert_eq!(tool["inputSchema"]["required"], json!(["path"]));

    let parsed: Result<verbs::FindRoomsForPathArgs, _> =
        serde_json::from_value(json!({ "path": "C:/repo-wt/task-1" }));
    assert!(
        parsed.is_ok(),
        "the advertised property must deserialize into FindRoomsForPathArgs: {parsed:?}"
    );
    assert_eq!(
        tool["annotations"]["readOnlyHint"],
        json!(true),
        "this verb only reads, and must say so the same way list_comments/get_comment/get_diff do"
    );
}

#[tokio::test]
async fn approving_is_refused_by_name_and_nothing_is_signed_off() {
    // The gate has to be refused the way `resolve` is: a model told a
    // tool is merely missing goes looking for another way in, so the
    // answer is a reason rather than "unknown tool".
    let f = fixture();
    save(&f.db, &[room("r1", vec![harness("h1", "claude", "main")])]);
    let caller = caller_for(&f.db, "r1", Some("h1"));
    let state = agent_api_state(&f);

    for name in [
        "approve",
        "sign_off",
        "signoff",
        "approve_review",
        "mark_approved",
        "mcp__skein__approve",
    ] {
        let err = mcp::call_tool(
            &state,
            &caller,
            name,
            &serde_json::json!({}),
            &MailContext::permissive(),
        )
        .await
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

#[tokio::test]
async fn approving_is_refused_by_name_even_when_it_names_another_room() {
    // #356 added `room`-taking verbs to this same dispatch table. Proof
    // that the by-name refusal still runs before any argument is even
    // looked at: naming a different room's id in `room` must not let
    // `approve`/`sign_off` slip through as if it were one of the new
    // cross-room reads.
    let f = fixture();
    save(
        &f.db,
        &[
            room("r1", vec![harness("h1", "claude", "main")]),
            room("r2", vec![harness("h2", "claude", "main")]),
        ],
    );
    let caller = caller_for(&f.db, "r1", Some("h1"));
    let state = agent_api_state(&f);

    for name in ["approve", "sign_off"] {
        let err = mcp::call_tool(
            &state,
            &caller,
            name,
            &serde_json::json!({ "room": "r2" }),
            &MailContext::permissive(),
        )
        .await
        .expect_err("an agent must not be able to approve another room's work either");
        assert!(
            matches!(&err, VerbError::Refused(m) if m.contains("reviewer")),
            "{name} gave {err:?}"
        );
    }
    assert_eq!(f.db.review_signoff("r2").unwrap(), None);
}

#[tokio::test]
async fn resolving_is_refused_by_name_and_the_thread_stays_open() {
    let f = fixture();
    save(&f.db, &[room("r1", vec![harness("h1", "claude", "main")])]);
    seed_thread(&f.db, "r1", "t1", "please rename this");
    let caller = caller_for(&f.db, "r1", Some("h1"));
    let state = agent_api_state(&f);

    for name in [
        "resolve",
        "resolve_thread",
        "close_thread",
        "mark_resolved",
        // Namespaced the way Claude Code presents it back.
        "mcp__skein__resolve",
    ] {
        let err = mcp::call_tool(
            &state,
            &caller,
            name,
            &json!({ "thread_id": "t1" }),
            &MailContext::permissive(),
        )
        .await
        .unwrap_err();
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

#[tokio::test]
async fn destroying_a_room_is_refused_by_name() {
    // `create_room` gave an agent the power to open a room, and these
    // names must not let it destroy one outright — that stays the
    // user's decision (see `CLAUDE.md`'s "no git mutations" note).
    // `close_room` (#411) is deliberately NOT in this list any more: it
    // is a real, narrowly guarded verb now, covered by its own tests
    // below.
    let f = fixture();
    save(&f.db, &[room("r1", vec![harness("h1", "claude", "main")])]);
    let caller = caller_for(&f.db, "r1", Some("h1"));
    let state = agent_api_state(&f);

    for name in ["remove_worktree", "delete_room"] {
        let err = mcp::call_tool(
            &state,
            &caller,
            name,
            &json!({}),
            &MailContext::permissive(),
        )
        .await
        .unwrap_err();
        assert!(
            matches!(&err, VerbError::Refused(m) if m.contains("user")),
            "{name} gave {err:?}"
        );
    }
}

#[tokio::test]
async fn archive_room_is_refused_by_name_and_points_at_close_room() {
    // #411: `archive_room` stays refused, but as an alias for the real
    // verb now, so its refusal must name `close_room` rather than
    // repeating the generic "that's the user's decision" of
    // `remove_worktree`/`delete_room` above.
    let f = fixture();
    save(&f.db, &[room("r1", vec![harness("h1", "claude", "main")])]);
    let caller = caller_for(&f.db, "r1", Some("h1"));
    let state = agent_api_state(&f);

    for name in ["archive_room", "mcp__skein__archive_room"] {
        let err = mcp::call_tool(
            &state,
            &caller,
            name,
            &json!({}),
            &MailContext::permissive(),
        )
        .await
        .unwrap_err();
        assert!(
            matches!(&err, VerbError::Refused(m) if m.contains("close_room")),
            "{name} gave {err:?}"
        );
    }
}

#[tokio::test]
async fn close_room_is_dispatched_through_mcp_not_refused_by_name() {
    // The counterpart of the two tests above: `close_room` must reach
    // the real verb, not the alias refusal — an unknown target room
    // proves dispatch happened, since `NotFound` only comes from inside
    // `verbs::close_room` itself.
    let f = fixture();
    save(&f.db, &[room("r1", vec![harness("h1", "claude", "main")])]);
    let caller = caller_for(&f.db, "r1", Some("h1"));
    let state = agent_api_state(&f);
    let err = mcp::call_tool(
        &state,
        &caller,
        "close_room",
        &json!({ "room": "nope" }),
        &MailContext::permissive(),
    )
    .await
    .unwrap_err();
    assert!(
        matches!(&err, VerbError::NotFound(m) if m.contains("nope")),
        "{err:?}"
    );
}

#[tokio::test]
async fn open_harness_is_dispatched_through_mcp_not_refused_by_name() {
    // Same proof as `close_room`'s counterpart above: an unknown target
    // room can only come from inside `verbs::open_harness` itself, so
    // reaching it proves dispatch happened.
    let f = fixture();
    save(&f.db, &[room("r1", vec![harness("h1", "claude", "main")])]);
    let caller = caller_for(&f.db, "r1", Some("h1"));
    let state = agent_api_state(&f);
    let err = mcp::call_tool(
        &state,
        &caller,
        "open_harness",
        &json!({ "room": "nope" }),
        &MailContext::permissive(),
    )
    .await
    .unwrap_err();
    assert!(
        matches!(&err, VerbError::NotFound(m) if m.contains("nope")),
        "{err:?}"
    );
}

#[tokio::test]
async fn close_harness_is_dispatched_through_mcp() {
    // Same proof as `close_room`'s and `open_harness`'s counterparts
    // above: an unknown target harness can only come from inside
    // `verbs::close_harness` itself, so reaching it proves dispatch
    // happened. (No alias refuses `close_harness` by name — it was never
    // part of D9's blanket destroy list.)
    let f = fixture();
    save(&f.db, &[room("r1", vec![harness("h1", "claude", "main")])]);
    let caller = caller_for(&f.db, "r1", Some("h1"));
    let state = agent_api_state(&f);
    let err = mcp::call_tool(
        &state,
        &caller,
        "close_harness",
        &json!({ "harness": "nope" }),
        &MailContext::permissive(),
    )
    .await
    .unwrap_err();
    assert!(
        matches!(&err, VerbError::NotFound(m) if m.contains("nope")),
        "{err:?}"
    );
}

#[tokio::test]
async fn an_unknown_tool_is_reported_missing_rather_than_refused() {
    // The counterpart of the test above: "refused" has to mean
    // something, so it cannot be the answer to every unknown name.
    let f = fixture();
    save(&f.db, &[room("r1", vec![])]);
    let caller = caller_for(&f.db, "r1", None);
    let state = agent_api_state(&f);
    let err = mcp::call_tool(
        &state,
        &caller,
        "delete_everything",
        &json!({}),
        &MailContext::permissive(),
    )
    .await
    .unwrap_err();
    assert!(matches!(err, VerbError::NotFound(_)));
}
