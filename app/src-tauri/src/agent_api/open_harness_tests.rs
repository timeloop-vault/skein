//! `open_harness` (#411): guards and happy paths.

use std::sync::atomic::{AtomicBool, Ordering};

use serde_json::{Value, json};

use super::close_room_tests::room_created_by;
use super::tests::{agent_api_state, caller_for, fixture, harness, room, save};
use super::verbs::{self, MailContext, VerbError};
use crate::db::Harness;

// ── opening a harness (issue #411) ──────────────────────────────────────
//
// `open_harness` is exercised as a plain function (`verbs::open_harness`),
// the same way `create_room`'s and `close_room`'s own guards are —
// dispatch through MCP is already covered above
// (`open_harness_is_dispatched_through_mcp_not_refused_by_name`).

pub(super) fn open_harness_args(room: &str) -> verbs::OpenHarnessArgs {
    verbs::OpenHarnessArgs {
        room: room.to_owned(),
        kind: None,
        agent: None,
        prompt: None,
    }
}

#[tokio::test]
async fn open_harness_is_refused_when_the_kill_switch_is_off() {
    let f = fixture();
    save(&f.db, &[room("r1", vec![harness("h1", "claude", "main")])]);
    let caller = caller_for(&f.db, "r1", Some("h1"));
    let state = agent_api_state(&f);
    let err = verbs::open_harness(
        &state,
        &caller,
        &open_harness_args("r1"),
        &MailContext::permissive(),
        false,
    )
    .await
    .unwrap_err();
    assert!(
        matches!(&err, VerbError::Refused(m) if m.starts_with("disabled:")),
        "{err:?}"
    );
}

#[tokio::test]
async fn open_harness_is_rate_limited_to_ten_attempts_a_minute() {
    let f = fixture();
    save(&f.db, &[room("r1", vec![harness("h1", "claude", "main")])]);
    let caller = caller_for(&f.db, "r1", Some("h1"));
    let state = agent_api_state(&f);
    // No target room exists at all — proof the rate counter is charged
    // before the target is even looked up, not after.
    for n in 0..10 {
        let err = verbs::open_harness(
            &state,
            &caller,
            &open_harness_args("missing"),
            &MailContext::permissive(),
            true,
        )
        .await
        .unwrap_err();
        assert!(
            matches!(&err, VerbError::NotFound(_)),
            "attempt {n}: expected the guards to pass through to NotFound, got {err:?}"
        );
    }
    let err = verbs::open_harness(
        &state,
        &caller,
        &open_harness_args("missing"),
        &MailContext::permissive(),
        true,
    )
    .await
    .unwrap_err();
    assert!(
        matches!(&err, VerbError::Refused(m) if m.starts_with("rate_limited:")),
        "{err:?}"
    );
}

#[tokio::test]
async fn open_harness_unknown_room_is_not_found() {
    let f = fixture();
    save(&f.db, &[room("r1", vec![harness("h1", "claude", "main")])]);
    let caller = caller_for(&f.db, "r1", Some("h1"));
    let state = agent_api_state(&f);
    let err = verbs::open_harness(
        &state,
        &caller,
        &open_harness_args("nope"),
        &MailContext::permissive(),
        true,
    )
    .await
    .unwrap_err();
    assert!(
        matches!(&err, VerbError::NotFound(m) if m.contains("nope")),
        "{err:?}"
    );
}

#[tokio::test]
async fn open_harness_refuses_a_room_neither_its_own_nor_one_it_created() {
    let f = fixture();
    save(
        &f.db,
        &[
            room("r1", vec![harness("h1", "claude", "main")]),
            room_created_by("r2", vec![], "r-someone-else"),
        ],
    );
    let caller = caller_for(&f.db, "r1", Some("h1"));
    let state = agent_api_state(&f);
    let err = verbs::open_harness(
        &state,
        &caller,
        &open_harness_args("r2"),
        &MailContext::permissive(),
        true,
    )
    .await
    .unwrap_err();
    assert!(
        matches!(&err, VerbError::Refused(m) if m.starts_with("not_in_scope:")),
        "{err:?}"
    );
}

#[tokio::test]
async fn open_harness_refuses_an_already_archived_room() {
    let f = fixture();
    let mut target = room_created_by("r2", vec![], "r1");
    target.archived = Some(1);
    save(
        &f.db,
        &[room("r1", vec![harness("h1", "claude", "main")]), target],
    );
    let caller = caller_for(&f.db, "r1", Some("h1"));
    let state = agent_api_state(&f);
    let err = verbs::open_harness(
        &state,
        &caller,
        &open_harness_args("r2"),
        &MailContext::permissive(),
        true,
    )
    .await
    .unwrap_err();
    assert!(
        matches!(&err, VerbError::Refused(m) if m.starts_with("archived:")),
        "{err:?}"
    );
}

#[tokio::test]
async fn open_harness_refuses_a_room_whose_folder_holds_a_different_repo() {
    let f = fixture();
    // A plain folder that exists but is not a repo reads as a mismatch.
    let plain = tempfile::TempDir::new().unwrap();
    let mut r1 = room("r1", vec![harness("h1", "claude", "main")]);
    r1.cwd = Some(plain.path().to_string_lossy().into_owned());
    r1.repo_identity = Some(crate::db::RepoIdentity {
        root_commits: vec!["0".repeat(40)],
        origin_url: None,
    });
    save(&f.db, &[r1]);
    let caller = caller_for(&f.db, "r1", Some("h1"));
    let state = agent_api_state(&f);
    let err = verbs::open_harness(
        &state,
        &caller,
        &open_harness_args("r1"),
        &MailContext::permissive(),
        true,
    )
    .await
    .unwrap_err();
    assert!(
        matches!(&err, VerbError::Refused(m) if m.starts_with("repo_mismatch:")),
        "{err:?}"
    );
}

#[tokio::test]
async fn open_harness_refuses_once_the_harness_ceiling_is_hit() {
    let f = fixture();
    let harnesses: Vec<Harness> = (0..8)
        .map(|i| harness(&format!("h{i}"), "claude", "main"))
        .collect();
    save(&f.db, &[room("r1", harnesses)]);
    let caller = caller_for(&f.db, "r1", Some("h0"));
    let state = agent_api_state(&f);
    let err = verbs::open_harness(
        &state,
        &caller,
        &open_harness_args("r1"),
        &MailContext::permissive(),
        true,
    )
    .await
    .unwrap_err();
    assert!(
        matches!(&err, VerbError::Refused(m) if m.starts_with("room_full:")),
        "{err:?}"
    );
}

#[tokio::test]
async fn open_harness_maps_a_missing_webview_to_unavailable() {
    let f = fixture();
    save(&f.db, &[room("r1", vec![harness("h1", "claude", "main")])]);
    let caller = caller_for(&f.db, "r1", Some("h1"));
    let state = agent_api_state(&f); // no test_frontend hook, no AppHandle
    let err = verbs::open_harness(
        &state,
        &caller,
        &open_harness_args("r1"),
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
async fn open_harness_maps_a_frontend_resolve_refusal_to_refused() {
    let f = fixture();
    save(&f.db, &[room("r1", vec![harness("h1", "claude", "main")])]);
    let caller = caller_for(&f.db, "r1", Some("h1"));
    let state = agent_api_state(&f);
    state.set_test_frontend(|kind, _args| match kind {
        "open_harness.resolve" => Err("unknown kind \"bogus\"".to_owned()),
        other => Err(format!("unexpected request_frontend kind {other:?}")),
    });
    let err = verbs::open_harness(
        &state,
        &caller,
        &open_harness_args("r1"),
        &MailContext::permissive(),
        true,
    )
    .await
    .unwrap_err();
    assert!(
        matches!(&err, VerbError::Refused(m) if m.contains("bogus")),
        "{err:?}"
    );
}

#[tokio::test]
async fn open_harness_with_a_prompt_refuses_an_unreachable_resolved_kind_before_opening_anything() {
    // Same shape as `create_room`'s own version of this test: the
    // resolve round trip has to run to learn the kind is unreachable,
    // but the second (opening) round trip must never be asked for.
    let f = fixture();
    save(&f.db, &[room("r1", vec![harness("h1", "claude", "main")])]);
    let caller = caller_for(&f.db, "r1", Some("h1"));
    let state = agent_api_state(&f);
    let round2_called = std::sync::Arc::new(AtomicBool::new(false));
    let round2_flag = std::sync::Arc::clone(&round2_called);
    state.set_test_frontend(move |kind, _args| match kind {
        "open_harness.resolve" => Ok(json!({ "kind": "copilot", "agent": null })),
        "open_harness" => {
            round2_flag.store(true, Ordering::SeqCst);
            Ok(json!({}))
        }
        other => Err(format!("unexpected request_frontend kind {other:?}")),
    });
    let args = verbs::OpenHarnessArgs {
        prompt: Some("go".to_owned()),
        ..open_harness_args("r1")
    };
    let err = verbs::open_harness(&state, &caller, &args, &MailContext::permissive(), true)
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
async fn open_harness_happy_path_in_its_own_room() {
    let f = fixture();
    save(&f.db, &[room("r1", vec![harness("h1", "claude", "main")])]);
    let caller = caller_for(&f.db, "r1", Some("h1"));
    let state = agent_api_state(&f);
    let opened_args = std::sync::Arc::new(std::sync::Mutex::new(None::<Value>));
    let opened_clone = std::sync::Arc::clone(&opened_args);
    state.set_test_frontend(move |kind, args| match kind {
        "open_harness.resolve" => Ok(json!({ "kind": "opencode", "agent": null })),
        "open_harness" => {
            *opened_clone.lock().unwrap() = Some(args.clone());
            Ok(json!({
                "harnessId": "h2",
                "kind": "opencode",
                "agent": null,
                "name": "opencode",
            }))
        }
        other => Err(format!("unexpected request_frontend kind {other:?}")),
    });
    let out = verbs::open_harness(
        &state,
        &caller,
        &open_harness_args("r1"),
        &MailContext::permissive(),
        true,
    )
    .await
    .unwrap();
    assert_eq!(out.room_id, "r1");
    assert_eq!(out.harness_id, "h2");
    assert_eq!(out.kind, "opencode");
    assert_eq!(out.agent, None);
    assert_eq!(out.name, "opencode");
    assert_eq!(out.message_id, None, "no prompt was given");

    let sent = opened_args
        .lock()
        .unwrap()
        .clone()
        .expect("open_harness requested");
    assert_eq!(sent["roomId"], json!("r1"));
    assert_eq!(sent["createdBy"]["roomId"], json!("r1"));
    assert_eq!(sent["createdBy"]["harnessId"], json!("h1"));
}

#[tokio::test]
async fn open_harness_happy_path_in_a_room_it_created() {
    let f = fixture();
    save(
        &f.db,
        &[
            room("r1", vec![harness("h1", "claude", "main")]),
            room_created_by("r2", vec![harness("h2a", "claude", "main")], "r1"),
        ],
    );
    let caller = caller_for(&f.db, "r1", Some("h1"));
    let state = agent_api_state(&f);
    state.set_test_frontend(|kind, _args| match kind {
        "open_harness.resolve" => Ok(json!({ "kind": "claude", "agent": null })),
        "open_harness" => Ok(json!({
            "harnessId": "h2b",
            "kind": "claude",
            "agent": null,
            "name": "claude 2",
        })),
        other => Err(format!("unexpected request_frontend kind {other:?}")),
    });
    let out = verbs::open_harness(
        &state,
        &caller,
        &open_harness_args("r2"),
        &MailContext::permissive(),
        true,
    )
    .await
    .unwrap();
    assert_eq!(out.room_id, "r2");
    assert_eq!(out.harness_id, "h2b");
}

#[tokio::test]
async fn open_harness_happy_path_with_a_prompt_queues_the_first_message() {
    let f = fixture();
    save(&f.db, &[room("r1", vec![harness("h1", "claude", "main")])]);
    let caller = caller_for(&f.db, "r1", Some("h1"));
    let state = agent_api_state(&f);
    state.set_test_frontend(|kind, _args| match kind {
        "open_harness.resolve" => Ok(json!({ "kind": "claude", "agent": null })),
        "open_harness" => Ok(json!({
            "harnessId": "h2",
            "kind": "claude",
            "agent": null,
            "name": "claude 2",
        })),
        other => Err(format!("unexpected request_frontend kind {other:?}")),
    });
    let args = verbs::OpenHarnessArgs {
        prompt: Some("start working on the thing".to_owned()),
        ..open_harness_args("r1")
    };
    let out = verbs::open_harness(&state, &caller, &args, &MailContext::permissive(), true)
        .await
        .unwrap();
    let message_id = out
        .message_id
        .clone()
        .expect("a prompt should queue a message");

    let messages = f.db.all_harness_messages("r1", "h2").unwrap();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].id, message_id);
    assert_eq!(messages[0].body, "start working on the thing");
    assert_eq!(messages[0].from_room_id, caller.room_id);
    assert!(messages[0].read_ms.is_none());
}
