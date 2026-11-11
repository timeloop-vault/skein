//! The harness hook routes over real HTTP: permission (#86) and session-start (#273).

use std::sync::Arc;

use serde_json::json;

use super::auth::{self};
use super::http_route_tests::serve_fixture;
use super::tests::{fixture, harness, room, save};

// ── the permission hook route (#86) ──────────────────────────────

#[tokio::test]
async fn the_permission_route_needs_a_token() {
    let f = fixture();
    save(&f.db, &[room("r1", vec![harness("h1", "claude", "main")])]);
    let base = serve_fixture(Arc::clone(&f.db)).await;
    let http = reqwest::Client::new();

    let anon = http
        .post(format!("{base}/api/harness/permission"))
        .header(auth::HARNESS_HEADER, "h1")
        .json(&json!({ "tool_name": "Bash" }))
        .send()
        .await
        .unwrap();
    assert_eq!(anon.status(), 401);
}

#[tokio::test]
async fn the_permission_route_needs_a_harness_the_room_actually_contains() {
    let f = fixture();
    save(&f.db, &[room("r1", vec![harness("h1", "claude", "main")])]);
    let token = f.db.ensure_room_token("r1", 1).unwrap();
    let base = serve_fixture(Arc::clone(&f.db)).await;
    let http = reqwest::Client::new();

    // No X-Skein-Harness at all.
    let no_header = http
        .post(format!("{base}/api/harness/permission"))
        .bearer_auth(&token)
        .json(&json!({}))
        .send()
        .await
        .unwrap();
    assert_eq!(no_header.status(), 400);

    // A harness id the room does not contain — attribution would keep
    // it, but this route needs more than attribution.
    let ghost = http
        .post(format!("{base}/api/harness/permission"))
        .bearer_auth(&token)
        .header(auth::HARNESS_HEADER, "h-ghost")
        .json(&json!({}))
        .send()
        .await
        .unwrap();
    assert_eq!(ghost.status(), 400);
}

#[tokio::test]
async fn a_valid_permission_ping_answers_no_content() {
    let f = fixture();
    save(&f.db, &[room("r1", vec![harness("h1", "claude", "main")])]);
    let token = f.db.ensure_room_token("r1", 1).unwrap();
    let base = serve_fixture(Arc::clone(&f.db)).await;
    let http = reqwest::Client::new();

    let ok = http
        .post(format!("{base}/api/harness/permission"))
        .bearer_auth(&token)
        .header(auth::HARNESS_HEADER, "h1")
        .json(&json!({ "tool_name": "Bash", "agent_type": "general" }))
        .send()
        .await
        .unwrap();
    assert_eq!(ok.status(), 204);
}

#[tokio::test]
async fn a_permission_ping_with_agent_id_answers_no_content() {
    // Subagent case (#276): `agent_id` is present only inside a
    // subagent call, per Claude Code's own hook docs. There is no
    // `AppHandle` in `fixture()` (see `AgentApiState::for_test`), so
    // the emitted event payload isn't observable here — this only
    // proves the route accepts and threads the field without erroring.
    let f = fixture();
    save(&f.db, &[room("r1", vec![harness("h1", "claude", "main")])]);
    let token = f.db.ensure_room_token("r1", 1).unwrap();
    let base = serve_fixture(Arc::clone(&f.db)).await;
    let http = reqwest::Client::new();

    let ok = http
        .post(format!("{base}/api/harness/permission"))
        .bearer_auth(&token)
        .header(auth::HARNESS_HEADER, "h1")
        .json(&json!({ "tool_name": "Bash", "agent_type": "general", "agent_id": "sub-1" }))
        .send()
        .await
        .unwrap();
    assert_eq!(ok.status(), 204);
}

#[tokio::test]
async fn a_permission_ping_with_no_agent_id_answers_no_content() {
    // Main-session case: no `agent_id` in the payload at all.
    let f = fixture();
    save(&f.db, &[room("r1", vec![harness("h1", "claude", "main")])]);
    let token = f.db.ensure_room_token("r1", 1).unwrap();
    let base = serve_fixture(Arc::clone(&f.db)).await;
    let http = reqwest::Client::new();

    let ok = http
        .post(format!("{base}/api/harness/permission"))
        .bearer_auth(&token)
        .header(auth::HARNESS_HEADER, "h1")
        .json(&json!({ "tool_name": "Bash", "agent_type": "general" }))
        .send()
        .await
        .unwrap();
    assert_eq!(ok.status(), 204);
}

#[tokio::test]
async fn a_body_that_is_not_json_still_answers_no_content() {
    // The hook's payload shape is Claude Code's own, not ours to
    // police — a hook whose payload changes shape must not start
    // failing the harness's turn.
    let f = fixture();
    save(&f.db, &[room("r1", vec![harness("h1", "claude", "main")])]);
    let token = f.db.ensure_room_token("r1", 1).unwrap();
    let base = serve_fixture(Arc::clone(&f.db)).await;
    let http = reqwest::Client::new();

    let ok = http
        .post(format!("{base}/api/harness/permission"))
        .bearer_auth(&token)
        .header(auth::HARNESS_HEADER, "h1")
        .header("Content-Type", "application/json")
        .body("not json at all")
        .send()
        .await
        .unwrap();
    assert_eq!(ok.status(), 204);
}

// ── the session-start hook route (#273) ──────────────────────────

#[tokio::test]
async fn the_session_start_route_needs_a_token() {
    let f = fixture();
    save(&f.db, &[room("r1", vec![harness("h1", "claude", "main")])]);
    let base = serve_fixture(Arc::clone(&f.db)).await;
    let http = reqwest::Client::new();

    let anon = http
        .post(format!("{base}/api/harness/session-start"))
        .header(auth::HARNESS_HEADER, "h1")
        .json(&json!({ "source": "startup" }))
        .send()
        .await
        .unwrap();
    assert_eq!(anon.status(), 401);
}

#[tokio::test]
async fn the_session_start_route_needs_a_harness_the_room_actually_contains() {
    let f = fixture();
    save(&f.db, &[room("r1", vec![harness("h1", "claude", "main")])]);
    let token = f.db.ensure_room_token("r1", 1).unwrap();
    let base = serve_fixture(Arc::clone(&f.db)).await;
    let http = reqwest::Client::new();

    // No X-Skein-Harness at all.
    let no_header = http
        .post(format!("{base}/api/harness/session-start"))
        .bearer_auth(&token)
        .json(&json!({}))
        .send()
        .await
        .unwrap();
    assert_eq!(no_header.status(), 400);

    // A harness id the room does not contain — attribution would keep
    // it, but this route needs more than attribution.
    let ghost = http
        .post(format!("{base}/api/harness/session-start"))
        .bearer_auth(&token)
        .header(auth::HARNESS_HEADER, "h-ghost")
        .json(&json!({}))
        .send()
        .await
        .unwrap();
    assert_eq!(ghost.status(), 400);
}

#[tokio::test]
async fn a_valid_session_start_ping_answers_no_content() {
    let f = fixture();
    save(&f.db, &[room("r1", vec![harness("h1", "claude", "main")])]);
    let token = f.db.ensure_room_token("r1", 1).unwrap();
    let base = serve_fixture(Arc::clone(&f.db)).await;
    let http = reqwest::Client::new();

    let ok = http
        .post(format!("{base}/api/harness/session-start"))
        .bearer_auth(&token)
        .header(auth::HARNESS_HEADER, "h1")
        .json(&json!({
            "session_id": "sid-1",
            "source": "startup",
            "hook_event_name": "SessionStart",
            "cwd": "/tmp/whatever"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(ok.status(), 204);
}

#[tokio::test]
async fn a_session_start_body_that_is_not_json_still_answers_no_content() {
    // The hook's payload shape is Claude Code's own, not ours to
    // police — a hook whose payload changes shape must not start
    // failing the harness's launch.
    let f = fixture();
    save(&f.db, &[room("r1", vec![harness("h1", "claude", "main")])]);
    let token = f.db.ensure_room_token("r1", 1).unwrap();
    let base = serve_fixture(Arc::clone(&f.db)).await;
    let http = reqwest::Client::new();

    let ok = http
        .post(format!("{base}/api/harness/session-start"))
        .bearer_auth(&token)
        .header(auth::HARNESS_HEADER, "h1")
        .header("Content-Type", "application/json")
        .body("not json at all")
        .send()
        .await
        .unwrap();
    assert_eq!(ok.status(), 204);
}
