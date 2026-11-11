//! `close_harness` (#411): guards and happy paths.

use serde_json::{Value, json};

use super::close_room_tests::room_created_by;
use super::open_harness_tests::open_harness_args;
use super::tests::{agent_api_state, caller_for, fixture, harness, room, save};
use super::verbs::{self, MailContext, VerbError};

// ── closing a harness (issue #411) ──────────────────────────────────────
//
// `close_harness` is exercised as a plain function (`verbs::close_harness`),
// the same way `close_room`'s and `open_harness`'s own guards are —
// dispatch through MCP is already covered above
// (`close_harness_is_dispatched_through_mcp`). It shares `open_harness`'s
// `harness_control` rate bucket, so the two verbs' rate-limit tests
// together prove the cap is combined rather than per-verb.

fn close_harness_args(harness: &str) -> verbs::CloseHarnessArgs {
    verbs::CloseHarnessArgs {
        harness: harness.to_owned(),
    }
}

#[tokio::test]
async fn close_harness_is_refused_when_the_kill_switch_is_off() {
    let f = fixture();
    save(
        &f.db,
        &[room(
            "r1",
            vec![
                harness("h1", "claude", "main"),
                harness("h2", "claude", "second"),
            ],
        )],
    );
    let caller = caller_for(&f.db, "r1", Some("h1"));
    let state = agent_api_state(&f);
    let err = verbs::close_harness(&state, &caller, &close_harness_args("h2"), false)
        .await
        .unwrap_err();
    assert!(
        matches!(&err, VerbError::Refused(m) if m.starts_with("disabled:")),
        "{err:?}"
    );
}

#[tokio::test]
async fn close_harness_shares_open_harness_rate_bucket() {
    let f = fixture();
    save(
        &f.db,
        &[room(
            "r1",
            vec![
                harness("h1", "claude", "main"),
                harness("h2", "claude", "second"),
            ],
        )],
    );
    let caller = caller_for(&f.db, "r1", Some("h1"));
    let state = agent_api_state(&f);
    // Spend nine of the ten combined attempts through `open_harness`
    // (proof against a missing room, same trick `open_harness`'s own
    // rate test uses), then the tenth through `close_harness` against an
    // unknown harness — both must reach their own NotFound, and the
    // eleventh, on either verb, must be rate-limited.
    for n in 0..9 {
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
            "attempt {n}: expected NotFound, got {err:?}"
        );
    }
    let err = verbs::close_harness(&state, &caller, &close_harness_args("missing"), true)
        .await
        .unwrap_err();
    assert!(
        matches!(&err, VerbError::NotFound(_)),
        "tenth combined attempt: expected NotFound, got {err:?}"
    );
    let err = verbs::close_harness(&state, &caller, &close_harness_args("missing"), true)
        .await
        .unwrap_err();
    assert!(
        matches!(&err, VerbError::Refused(m) if m.starts_with("rate_limited:")),
        "{err:?}"
    );
}

#[tokio::test]
async fn close_harness_unknown_harness_is_not_found() {
    let f = fixture();
    save(&f.db, &[room("r1", vec![harness("h1", "claude", "main")])]);
    let caller = caller_for(&f.db, "r1", Some("h1"));
    let state = agent_api_state(&f);
    let err = verbs::close_harness(&state, &caller, &close_harness_args("nope"), true)
        .await
        .unwrap_err();
    assert!(
        matches!(&err, VerbError::NotFound(m) if m.contains("nope")),
        "{err:?}"
    );
}

#[tokio::test]
async fn close_harness_refuses_a_harness_neither_in_its_own_room_nor_one_it_created() {
    let f = fixture();
    save(
        &f.db,
        &[
            room("r1", vec![harness("h1", "claude", "main")]),
            room_created_by(
                "r2",
                vec![
                    harness("h2a", "claude", "main"),
                    harness("h2b", "claude", "two"),
                ],
                "r-someone-else",
            ),
        ],
    );
    let caller = caller_for(&f.db, "r1", Some("h1"));
    let state = agent_api_state(&f);
    let err = verbs::close_harness(&state, &caller, &close_harness_args("h2a"), true)
        .await
        .unwrap_err();
    assert!(
        matches!(&err, VerbError::Refused(m) if m.starts_with("not_in_scope:")),
        "{err:?}"
    );
}

#[tokio::test]
async fn close_harness_in_its_own_room_refuses_an_unidentified_caller() {
    // No `X-Skein-Harness` at all: there is no way to prove the target
    // isn't the caller itself, so this must refuse rather than guess.
    let f = fixture();
    save(
        &f.db,
        &[room(
            "r1",
            vec![
                harness("h1", "claude", "main"),
                harness("h2", "claude", "second"),
            ],
        )],
    );
    let caller = caller_for(&f.db, "r1", None);
    assert_eq!(caller.harness_id, None, "test setup: no caller identity");
    let state = agent_api_state(&f);
    let err = verbs::close_harness(&state, &caller, &close_harness_args("h2"), true)
        .await
        .unwrap_err();
    assert!(
        matches!(&err, VerbError::Refused(m) if m.starts_with("caller_unknown:")),
        "{err:?}"
    );
}

#[tokio::test]
async fn close_harness_cannot_close_itself() {
    let f = fixture();
    save(
        &f.db,
        &[room(
            "r1",
            vec![
                harness("h1", "claude", "main"),
                harness("h2", "claude", "second"),
            ],
        )],
    );
    let caller = caller_for(&f.db, "r1", Some("h1"));
    let state = agent_api_state(&f);
    let err = verbs::close_harness(&state, &caller, &close_harness_args("h1"), true)
        .await
        .unwrap_err();
    assert!(
        matches!(&err, VerbError::Refused(m) if m.starts_with("self:")),
        "{err:?}"
    );
}

#[tokio::test]
async fn close_harness_refuses_a_harness_in_an_archived_room_it_created() {
    let f = fixture();
    let mut target = room_created_by(
        "r2",
        vec![
            harness("h2a", "claude", "main"),
            harness("h2b", "claude", "two"),
        ],
        "r1",
    );
    target.archived = Some(1);
    save(
        &f.db,
        &[room("r1", vec![harness("h1", "claude", "main")]), target],
    );
    let caller = caller_for(&f.db, "r1", Some("h1"));
    let state = agent_api_state(&f);
    let err = verbs::close_harness(&state, &caller, &close_harness_args("h2a"), true)
        .await
        .unwrap_err();
    assert!(
        matches!(&err, VerbError::Refused(m) if m.starts_with("archived:")),
        "{err:?}"
    );
}

#[tokio::test]
async fn close_harness_refuses_the_last_harness_in_a_room_it_created() {
    // A room's only harness can never be `self` from another room's
    // caller — only `close_harness_cannot_close_itself` above exercises
    // that overlap, where the caller and the room's last harness are the
    // same id. Here the caller is a *different* room, so this is the one
    // path that actually reaches the `last_harness` guard.
    // Unlike the test above, the caller here is a *different* room, so
    // `self` never applies and `last_harness` is the guard that fires.
    let f = fixture();
    save(
        &f.db,
        &[
            room("r1", vec![harness("h1", "claude", "main")]),
            room_created_by("r2", vec![harness("h2", "claude", "main")], "r1"),
        ],
    );
    let caller = caller_for(&f.db, "r1", Some("h1"));
    let state = agent_api_state(&f);
    let err = verbs::close_harness(&state, &caller, &close_harness_args("h2"), true)
        .await
        .unwrap_err();
    assert!(
        matches!(&err, VerbError::Refused(m) if m.starts_with("last_harness:")),
        "{err:?}"
    );
}

#[tokio::test]
async fn close_harness_maps_a_missing_webview_to_unavailable() {
    let f = fixture();
    save(
        &f.db,
        &[room(
            "r1",
            vec![
                harness("h1", "claude", "main"),
                harness("h2", "claude", "second"),
            ],
        )],
    );
    let caller = caller_for(&f.db, "r1", Some("h1"));
    let state = agent_api_state(&f); // no test_frontend hook, no AppHandle
    let err = verbs::close_harness(&state, &caller, &close_harness_args("h2"), true)
        .await
        .unwrap_err();
    assert!(
        matches!(&err, VerbError::Unavailable(m) if m.contains("no webview")),
        "{err:?}"
    );
}

#[tokio::test]
async fn close_harness_frontend_permission_open_refusal_keeps_its_code() {
    let f = fixture();
    save(
        &f.db,
        &[room(
            "r1",
            vec![
                harness("h1", "claude", "main"),
                harness("h2", "claude", "second"),
            ],
        )],
    );
    let caller = caller_for(&f.db, "r1", Some("h1"));
    let state = agent_api_state(&f);
    state.set_test_frontend(|kind, _args| match kind {
        "close_harness" => {
            Err("permission_open: harness \"h2\" has an open permission dialog".to_owned())
        }
        other => Err(format!("unexpected request_frontend kind {other:?}")),
    });
    let err = verbs::close_harness(&state, &caller, &close_harness_args("h2"), true)
        .await
        .unwrap_err();
    assert!(
        matches!(&err, VerbError::Refused(m) if m.starts_with("permission_open:")),
        "the frontend's own code must survive frontend_error unchanged: {err:?}"
    );
}

#[tokio::test]
async fn close_harness_frontend_unsaved_files_refusal_keeps_its_code() {
    let f = fixture();
    save(
        &f.db,
        &[room(
            "r1",
            vec![
                harness("h1", "claude", "main"),
                harness("h2", "claude", "second"),
            ],
        )],
    );
    let caller = caller_for(&f.db, "r1", Some("h1"));
    let state = agent_api_state(&f);
    state.set_test_frontend(|kind, _args| match kind {
        "close_harness" => Err("unsaved_files: notes.md".to_owned()),
        other => Err(format!("unexpected request_frontend kind {other:?}")),
    });
    let err = verbs::close_harness(&state, &caller, &close_harness_args("h2"), true)
        .await
        .unwrap_err();
    assert!(
        matches!(&err, VerbError::Refused(m) if m.starts_with("unsaved_files:")),
        "the frontend's own code must survive frontend_error unchanged: {err:?}"
    );
}

#[tokio::test]
async fn close_harness_happy_path_in_its_own_room() {
    let f = fixture();
    save(
        &f.db,
        &[room(
            "r1",
            vec![
                harness("h1", "claude", "main"),
                harness("h2", "claude", "second"),
            ],
        )],
    );
    let caller = caller_for(&f.db, "r1", Some("h1"));
    let state = agent_api_state(&f);
    let captured = std::sync::Arc::new(std::sync::Mutex::new(None::<Value>));
    let captured_clone = std::sync::Arc::clone(&captured);
    state.set_test_frontend(move |kind, args| match kind {
        "close_harness" => {
            *captured_clone.lock().unwrap() = Some(args.clone());
            Ok(json!({ "harnessId": "h2", "phase": "waiting" }))
        }
        other => Err(format!("unexpected request_frontend kind {other:?}")),
    });

    let out = verbs::close_harness(&state, &caller, &close_harness_args("h2"), true)
        .await
        .unwrap();
    assert_eq!(out.room_id, "r1");
    assert_eq!(out.harness_id, "h2");
    assert_eq!(out.phase, "waiting");
    assert_eq!(out.closed_by.room_id, "r1");
    assert_eq!(out.closed_by.harness_id.as_deref(), Some("h1"));

    let sent = captured
        .lock()
        .unwrap()
        .clone()
        .expect("close_harness requested");
    assert_eq!(sent["roomId"], json!("r1"));
    assert_eq!(sent["harnessId"], json!("h2"));
    assert_eq!(sent["closedBy"]["roomId"], json!("r1"));
    assert_eq!(sent["closedBy"]["harnessId"], json!("h1"));
}

#[tokio::test]
async fn close_harness_happy_path_in_a_room_it_created_reports_the_running_phase() {
    let f = fixture();
    save(
        &f.db,
        &[
            room("r1", vec![harness("h1", "claude", "main")]),
            room_created_by(
                "r2",
                vec![
                    harness("h2a", "claude", "main"),
                    harness("h2b", "claude", "two"),
                ],
                "r1",
            ),
        ],
    );
    let caller = caller_for(&f.db, "r1", Some("h1"));
    let state = agent_api_state(&f);
    state.set_test_frontend(|kind, _args| match kind {
        "close_harness" => Ok(json!({ "harnessId": "h2b", "phase": "running" })),
        other => Err(format!("unexpected request_frontend kind {other:?}")),
    });

    let out = verbs::close_harness(&state, &caller, &close_harness_args("h2b"), true)
        .await
        .unwrap();
    assert_eq!(out.room_id, "r2");
    assert_eq!(out.harness_id, "h2b");
    assert_eq!(out.phase, "running");
}
