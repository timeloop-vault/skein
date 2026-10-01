//! `create_room` (#330): the prompt guard, frontend round trips and happy paths.

use std::sync::atomic::{AtomicBool, Ordering};

use serde_json::{Value, json};
use tempfile::TempDir;

use super::create_room_tests::{create_room_args, git_room};
use super::review_tests::git_repo_with_commit;
use super::tests::{agent_api_state, caller_for, fixture, room, save};
use super::verbs::{self, MailContext, MailPolicy, VerbError};

#[tokio::test]
async fn create_room_with_a_prompt_refuses_up_front_when_messaging_is_off() {
    let f = fixture();
    let (_tmp, caller) = git_room(&f);
    let state = agent_api_state(&f);
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
    // No frontend hook wired: reaching it would itself prove this guard
    // did not fire before round-trip 1, as the brief requires.
    let err = verbs::create_room(&state, &caller, &args, &mail, true)
        .await
        .unwrap_err();
    assert!(
        matches!(&err, VerbError::Refused(m) if m.contains("messaging")),
        "{err:?}"
    );
}

#[tokio::test]
async fn create_room_with_a_prompt_refuses_an_unreachable_resolved_kind_before_creating_anything() {
    // The brief's own example: `copilot` has no MCP connection at all,
    // so a queued prompt could never be delivered. The resolve round
    // trip has to run to learn that — the *creation* round trip must
    // not.
    let f = fixture();
    let (_tmp, caller) = git_room(&f);
    let state = agent_api_state(&f);
    let round2_called = std::sync::Arc::new(AtomicBool::new(false));
    let round2_flag = std::sync::Arc::clone(&round2_called);
    state.set_test_frontend(move |kind, _args| match kind {
        "create_room.resolve" => Ok(json!({ "kind": "copilot", "agent": null })),
        "create_room" => {
            round2_flag.store(true, Ordering::SeqCst);
            Ok(json!({}))
        }
        other => Err(format!("unexpected request_frontend kind {other:?}")),
    });
    let args = verbs::CreateRoomArgs {
        prompt: Some("go".to_owned()),
        ..create_room_args("hi")
    };
    let err = verbs::create_room(&state, &caller, &args, &MailContext::permissive(), true)
        .await
        .unwrap_err();
    assert!(
        matches!(&err, VerbError::Refused(m) if m.contains("copilot")),
        "{err:?}"
    );
    assert!(
        !round2_called.load(Ordering::SeqCst),
        "round-trip 2 must never be requested once the prompt is unreachable"
    );
}

#[tokio::test]
async fn create_room_maps_a_frontend_refusal_to_refused() {
    let f = fixture();
    let (_tmp, caller) = git_room(&f);
    let state = agent_api_state(&f);
    state.set_test_frontend(|kind, _args| match kind {
        "create_room.resolve" => Err("that folder is not readable".to_owned()),
        other => Err(format!("unexpected request_frontend kind {other:?}")),
    });
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
        matches!(&err, VerbError::Refused(m) if m.contains("not readable")),
        "{err:?}"
    );
}

#[tokio::test]
async fn create_room_maps_a_missing_webview_to_unavailable() {
    let f = fixture();
    let (_tmp, caller) = git_room(&f);
    let state = agent_api_state(&f); // no test_frontend hook, no AppHandle
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
        matches!(&err, VerbError::Unavailable(m) if m.contains("no webview")),
        "{err:?}"
    );
}

#[tokio::test]
async fn create_room_sends_an_empty_string_harness_id_when_the_caller_has_none() {
    // `Caller.harness_id` is `None` whenever the request carried no
    // `X-Skein-Harness`. `createdBy.harnessId` still has to be *a
    // string* — the frontend's `parseCreateArgs` requires the type, even
    // though it accepts it empty — so this must send `""`, never `null`.
    let f = fixture();
    let tmp = TempDir::new().unwrap();
    git_repo_with_commit(tmp.path());
    let mut r = room("r1", vec![]);
    r.cwd = Some(tmp.path().to_str().unwrap().to_owned());
    save(&f.db, &[r]);
    let caller = caller_for(&f.db, "r1", None);
    assert_eq!(
        caller.harness_id, None,
        "test setup: no harness to attribute to"
    );
    let state = agent_api_state(&f);

    let captured = std::sync::Arc::new(std::sync::Mutex::new(None::<Value>));
    let captured_clone = std::sync::Arc::clone(&captured);
    state.set_test_frontend(move |kind, args| match kind {
        "create_room.resolve" => Ok(json!({ "kind": "claude", "agent": null })),
        "create_room" => {
            *captured_clone.lock().unwrap() = Some(args.clone());
            Ok(json!({ "roomId": "r2", "name": "n", "harnessId": "h2", "kind": "claude" }))
        }
        other => Err(format!("unexpected request_frontend kind {other:?}")),
    });

    verbs::create_room(
        &state,
        &caller,
        &create_room_args("hi"),
        &MailContext::permissive(),
        true,
    )
    .await
    .unwrap();

    let args = captured
        .lock()
        .unwrap()
        .clone()
        .expect("the create_room round trip must have been called");
    assert_eq!(args["createdBy"]["roomId"], json!("r1"));
    assert_eq!(
        args["createdBy"]["harnessId"],
        json!(""),
        "harnessId must be an empty string, never null"
    );
}

#[tokio::test]
async fn create_room_resolve_and_create_payloads_use_null_for_omitted_optionals() {
    // Pins the wire contract for a minimal `{task}`-only call: every
    // `Option` field `create_room` didn't get goes to the frontend as
    // JSON `null`, via `serde_json::json!`, never as an absent key.
    // The frontend's `parseResolveArgs`/`parseCreateArgs` must treat
    // that `null` the same as "omitted" — a real agent call hit this
    // exact shape and was rejected with "kind must be a string" before
    // that fix (#330 follow-up).
    let f = fixture();
    let (_tmp, caller) = git_room(&f);
    let state = agent_api_state(&f);

    let resolve_args = std::sync::Arc::new(std::sync::Mutex::new(None::<Value>));
    let create_args = std::sync::Arc::new(std::sync::Mutex::new(None::<Value>));
    let resolve_clone = std::sync::Arc::clone(&resolve_args);
    let create_clone = std::sync::Arc::clone(&create_args);
    state.set_test_frontend(move |kind, args| match kind {
        "create_room.resolve" => {
            *resolve_clone.lock().unwrap() = Some(args.clone());
            Ok(json!({ "kind": "claude", "agent": null }))
        }
        "create_room" => {
            *create_clone.lock().unwrap() = Some(args.clone());
            Ok(json!({
                "roomId": "new-room",
                "name": "task name",
                "harnessId": "new-harness",
                "kind": "claude",
            }))
        }
        other => Err(format!("unexpected request_frontend kind {other:?}")),
    });

    verbs::create_room(
        &state,
        &caller,
        &create_room_args("do the thing"),
        &MailContext::permissive(),
        true,
    )
    .await
    .unwrap();

    let resolve = resolve_args
        .lock()
        .unwrap()
        .clone()
        .expect("resolve called");
    assert_eq!(resolve["kind"], Value::Null, "omitted kind must be null");
    assert_eq!(resolve["agent"], Value::Null, "omitted agent must be null");

    let create = create_args.lock().unwrap().clone().expect("create called");
    assert_eq!(create["branch"], Value::Null, "omitted branch must be null");
    assert_eq!(
        create["baseBranch"],
        Value::Null,
        "omitted baseBranch must be null"
    );
    assert_eq!(
        create["agent"],
        Value::Null,
        "the resolved agent (still None here) must be null"
    );
    assert_eq!(
        create["promptFirstLine"],
        Value::Null,
        "omitted prompt must leave promptFirstLine null, never absent"
    );
}

#[tokio::test]
async fn create_room_happy_path_without_a_prompt() {
    let f = fixture();
    let (_tmp, caller) = git_room(&f);
    let state = agent_api_state(&f);
    state.set_test_frontend(|kind, _args| match kind {
        "create_room.resolve" => Ok(json!({ "kind": "claude", "agent": null })),
        "create_room" => Ok(json!({
            "roomId": "new-room",
            "name": "task name",
            "cwd": "/some/wt",
            "repo": "repo",
            "branch": "agent/task",
            "harnessId": "new-harness",
            "kind": "claude",
            "agent": null,
            "sessionId": "sess-1",
        })),
        other => Err(format!("unexpected request_frontend kind {other:?}")),
    });
    let out = verbs::create_room(
        &state,
        &caller,
        &create_room_args("do the thing"),
        &MailContext::permissive(),
        true,
    )
    .await
    .unwrap();
    assert_eq!(out.room_id, "new-room");
    assert_eq!(out.harness_id, "new-harness");
    assert_eq!(out.kind, "claude");
    assert_eq!(out.session_id.as_deref(), Some("sess-1"));
    assert_eq!(out.message_id, None, "no prompt was given");
    assert_eq!(
        f.db.all_harness_messages("new-room", "new-harness")
            .unwrap(),
        Vec::new()
    );
}

#[tokio::test]
async fn create_room_forwards_base_behind_upstream_when_the_frontend_reports_it() {
    let f = fixture();
    let (_tmp, caller) = git_room(&f);
    let state = agent_api_state(&f);
    state.set_test_frontend(|kind, _args| match kind {
        "create_room.resolve" => Ok(json!({ "kind": "claude", "agent": null })),
        "create_room" => Ok(json!({
            "roomId": "new-room",
            "name": "task name",
            "cwd": "/some/wt",
            "repo": "repo",
            "branch": "agent/task",
            "harnessId": "new-harness",
            "kind": "claude",
            "agent": null,
            "sessionId": "sess-1",
            "baseBehindUpstream": 3,
        })),
        other => Err(format!("unexpected request_frontend kind {other:?}")),
    });
    let out = verbs::create_room(
        &state,
        &caller,
        &create_room_args("do the thing"),
        &MailContext::permissive(),
        true,
    )
    .await
    .unwrap();
    assert_eq!(out.base_behind_upstream, Some(3));
}

#[tokio::test]
async fn create_room_omits_base_behind_upstream_when_the_frontend_does_not_report_it() {
    let f = fixture();
    let (_tmp, caller) = git_room(&f);
    let state = agent_api_state(&f);
    state.set_test_frontend(|kind, _args| match kind {
        "create_room.resolve" => Ok(json!({ "kind": "claude", "agent": null })),
        "create_room" => Ok(json!({
            "roomId": "new-room",
            "name": "task name",
            "cwd": "/some/wt",
            "repo": "repo",
            "branch": "agent/task",
            "harnessId": "new-harness",
            "kind": "claude",
            "agent": null,
            "sessionId": "sess-1",
        })),
        other => Err(format!("unexpected request_frontend kind {other:?}")),
    });
    let out = verbs::create_room(
        &state,
        &caller,
        &create_room_args("do the thing"),
        &MailContext::permissive(),
        true,
    )
    .await
    .unwrap();
    assert_eq!(out.base_behind_upstream, None);
}

#[tokio::test]
async fn create_room_happy_path_with_a_prompt_queues_the_first_message() {
    let f = fixture();
    let (_tmp, caller) = git_room(&f);
    let state = agent_api_state(&f);
    let create_args = std::sync::Arc::new(std::sync::Mutex::new(None::<Value>));
    let create_clone = std::sync::Arc::clone(&create_args);
    state.set_test_frontend(move |kind, args| match kind {
        "create_room.resolve" => Ok(json!({ "kind": "claude", "agent": null })),
        "create_room" => {
            *create_clone.lock().unwrap() = Some(args.clone());
            Ok(json!({
                "roomId": "new-room", "name": "task name",
                "harnessId": "new-harness", "kind": "claude",
            }))
        }
        other => Err(format!("unexpected request_frontend kind {other:?}")),
    });
    let args = verbs::CreateRoomArgs {
        prompt: Some("start working on the thing".to_owned()),
        ..create_room_args("do the thing")
    };
    let out = verbs::create_room(&state, &caller, &args, &MailContext::permissive(), true)
        .await
        .unwrap();
    let message_id = out
        .message_id
        .clone()
        .expect("a prompt should queue a message");

    let messages =
        f.db.all_harness_messages("new-room", "new-harness")
            .unwrap();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].id, message_id);
    assert_eq!(messages[0].body, "start working on the thing");
    assert_eq!(messages[0].from_room_id, caller.room_id);
    assert!(messages[0].read_ms.is_none());

    // #356: `promptFirstLine` is a TOP-LEVEL field of the "create_room"
    // request, not nested under `createdBy` — `parseCreateArgs`
    // (`app/src/agentRequests.ts`) reads it there.
    let sent = create_args.lock().unwrap().clone().expect("create called");
    assert_eq!(sent["promptFirstLine"], json!("start working on the thing"));
    assert_eq!(
        sent["createdBy"].get("promptFirstLine"),
        None,
        "must not also be nested under createdBy"
    );
}
