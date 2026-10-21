//! The Hermes hook routes over real HTTP (#625): the recorded payloads
//! in `crates/skein-harness` posted the way the hook script posts them,
//! with the emitted events captured by `AgentApiState`'s test sink.

use std::sync::Arc;

use serde_json::{Value, json};

use super::auth;
use super::hermes_event::HarnessHermesHook;
use super::state::AgentApiState;
use super::tests::{fixture, harness, room, save};
use crate::db::Database;

const HOOKS_B: &str =
    include_str!("../../../../crates/skein-harness/src/hermes/fixtures/hooks-B.jsonl");
const HOOKS_C: &str =
    include_str!("../../../../crates/skein-harness/src/hermes/fixtures/hooks-C.jsonl");

async fn serve(db: Arc<Database>) -> (String, Arc<AgentApiState>) {
    crate::install_rustls_provider();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let state = Arc::new(AgentApiState::for_test(db));
    tokio::spawn(super::http::serve(listener, Arc::clone(&state)));
    (format!("http://127.0.0.1:{port}"), state)
}

/// The URL the hook script uses for this event.
fn route_for(name: &str) -> &'static str {
    match name {
        "pre_approval_request" | "post_approval_response" => "/api/harness/permission",
        "on_session_start" => "/api/harness/session-start",
        _ => "/api/harness/hermes-turn",
    }
}

async fn post_lines(base: &str, token: &str, jsonl: &str) {
    let http = reqwest::Client::new();
    for line in jsonl.lines().filter(|l| !l.trim().is_empty()) {
        let v: Value = serde_json::from_str(line).unwrap();
        let name = v["hook_event_name"].as_str().unwrap();
        let res = http
            .post(format!("{base}{}", route_for(name)))
            .bearer_auth(token)
            .header(auth::HARNESS_HEADER, "h1")
            .body(line.to_owned())
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), 204, "{name}");
    }
}

fn events(state: &AgentApiState) -> Vec<HarnessHermesHook> {
    state.test_hermes_events.lock().clone()
}

#[tokio::test]
async fn an_approved_dialog_emits_a_smart_precheck_then_a_request_response_pair() {
    let f = fixture();
    save(&f.db, &[room("r1", vec![harness("h1", "hermes", "main")])]);
    let token = f.db.ensure_room_token("r1", 1).unwrap();
    let (base, state) = serve(Arc::clone(&f.db)).await;
    // The first session of hooks-B: approved with `once`.
    let first_session: String = HOOKS_B.lines().take(9).collect::<Vec<_>>().join("\n");
    post_lines(&base, &token, &first_session).await;

    let evs = events(&state);
    assert_eq!(evs.len(), 9);
    assert!(
        evs.iter()
            .all(|e| e.room_id == "r1" && e.harness_id == "h1")
    );
    let names: Vec<&str> = evs.iter().map(|e| e.event.as_str()).collect();
    assert_eq!(
        &names[..9],
        [
            "on_session_start",
            "pre_llm_call",
            "pre_tool_call",
            "pre_approval_request",
            "pre_approval_request",
            "post_approval_response",
            "post_tool_call",
            "post_llm_call",
            "on_session_end"
        ]
    );
    let smart = &evs[3];
    let dialog = &evs[4];
    let answer = &evs[5];
    assert_eq!(smart.surface.as_deref(), Some("smart"));
    assert_eq!(dialog.surface.as_deref(), Some("cli"));
    assert_eq!(answer.choice.as_deref(), Some("once"));
    assert!(smart.tool_call_id.is_some());
    assert_eq!(smart.tool_call_id, dialog.tool_call_id);
    assert_eq!(dialog.tool_call_id, answer.tool_call_id);
    assert_eq!(dialog.turn_id, answer.turn_id);
    assert!(dialog.description.is_some());
    assert_eq!(evs[8].completed, Some(true));
    assert_eq!(evs[8].interrupted, Some(false));
    assert_eq!(evs[0].session_id.as_deref(), Some("20261021_112625_cf3d58"));
}

#[tokio::test]
async fn a_denied_dialog_carries_the_choice() {
    let f = fixture();
    save(&f.db, &[room("r1", vec![harness("h1", "hermes", "main")])]);
    let token = f.db.ensure_room_token("r1", 1).unwrap();
    let (base, state) = serve(Arc::clone(&f.db)).await;
    let second: String = HOOKS_B
        .lines()
        .skip(10)
        .take(10)
        .collect::<Vec<_>>()
        .join("\n");
    post_lines(&base, &token, &second).await;
    let denied: Vec<_> = events(&state)
        .into_iter()
        .filter(|e| e.event == "post_approval_response")
        .collect();
    assert_eq!(denied.len(), 1);
    assert_eq!(denied[0].choice.as_deref(), Some("deny"));
}

#[tokio::test]
async fn an_interrupted_turn_end_is_reported_and_finalize_passes_through() {
    let f = fixture();
    save(&f.db, &[room("r1", vec![harness("h1", "hermes", "main")])]);
    let token = f.db.ensure_room_token("r1", 1).unwrap();
    let (base, state) = serve(Arc::clone(&f.db)).await;
    post_lines(&base, &token, HOOKS_C).await;
    let evs = events(&state);
    let names: Vec<&str> = evs.iter().map(|e| e.event.as_str()).collect();
    assert_eq!(
        names,
        [
            "on_session_start",
            "pre_llm_call",
            "on_session_end",
            "on_session_finalize"
        ]
    );
    assert_eq!(evs[2].interrupted, Some(true));
    assert_eq!(evs[2].completed, Some(false));
    assert!(evs[2].turn_id.is_some());
}

#[tokio::test]
async fn a_claude_permission_body_does_not_emit_a_hermes_event() {
    let f = fixture();
    save(&f.db, &[room("r1", vec![harness("h1", "claude", "main")])]);
    let token = f.db.ensure_room_token("r1", 1).unwrap();
    let (base, state) = serve(Arc::clone(&f.db)).await;
    let http = reqwest::Client::new();
    for (route, name) in [
        ("permission", "PermissionRequest"),
        ("session-start", "SessionStart"),
    ] {
        let res = http
            .post(format!("{base}/api/harness/{route}"))
            .bearer_auth(&token)
            .header(auth::HARNESS_HEADER, "h1")
            .json(&json!({ "hook_event_name": name, "tool_name": "Bash", "session_id": "s-1" }))
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), 204);
    }
    assert_eq!(events(&state).len(), 0);
}

#[tokio::test]
async fn the_hermes_turn_route_ignores_a_body_that_is_not_a_hermes_hook() {
    let f = fixture();
    save(&f.db, &[room("r1", vec![harness("h1", "hermes", "main")])]);
    let token = f.db.ensure_room_token("r1", 1).unwrap();
    let (base, state) = serve(Arc::clone(&f.db)).await;
    let http = reqwest::Client::new();
    for body in ["not json", "{}", r#"{"hook_event_name":"PreToolUse"}"#] {
        let res = http
            .post(format!("{base}/api/harness/hermes-turn"))
            .bearer_auth(&token)
            .header(auth::HARNESS_HEADER, "h1")
            .body(body)
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), 204);
    }
    assert_eq!(events(&state).len(), 0);
}

#[tokio::test]
async fn the_hermes_routes_need_a_token_and_a_real_harness() {
    let f = fixture();
    save(&f.db, &[room("r1", vec![harness("h1", "hermes", "main")])]);
    let token = f.db.ensure_room_token("r1", 1).unwrap();
    let (base, state) = serve(Arc::clone(&f.db)).await;
    let http = reqwest::Client::new();
    let body = json!({ "hook_event_name": "pre_llm_call", "session_id": "s1" });
    for route in ["hermes-turn", "permission", "session-start"] {
        let url = format!("{base}/api/harness/{route}");
        let anon = http
            .post(&url)
            .header(auth::HARNESS_HEADER, "h1")
            .json(&body)
            .send()
            .await
            .unwrap();
        assert_eq!(anon.status(), 401, "{route}");
        let none = http
            .post(&url)
            .bearer_auth(&token)
            .json(&body)
            .send()
            .await
            .unwrap();
        assert_eq!(none.status(), 400, "{route}");
        let ghost = http
            .post(&url)
            .bearer_auth(&token)
            .header(auth::HARNESS_HEADER, "h-ghost")
            .json(&body)
            .send()
            .await
            .unwrap();
        assert_eq!(ghost.status(), 400, "{route}");
    }
    assert_eq!(events(&state).len(), 0);
}

#[tokio::test]
async fn a_multi_megabyte_pre_llm_call_is_accepted_and_its_fields_are_capped() {
    let f = fixture();
    save(&f.db, &[room("r1", vec![harness("h1", "hermes", "main")])]);
    let token = f.db.ensure_room_token("r1", 1).unwrap();
    let (base, state) = serve(Arc::clone(&f.db)).await;
    let history = "x".repeat(3 * 1024 * 1024);
    let body = json!({
        "hook_event_name": "pre_llm_call",
        "session_id": "20261021_000000_abcdef",
        "extra": {
            "turn_id": "t".repeat(500),
            "description": "d".repeat(2000),
            "child_session_id": "../escape",
            "history": history
        }
    });
    let res = reqwest::Client::new()
        .post(format!("{base}/api/harness/hermes-turn"))
        .bearer_auth(&token)
        .header(auth::HARNESS_HEADER, "h1")
        .body(body.to_string())
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 204);
    let evs = events(&state);
    assert_eq!(evs.len(), 1);
    assert_eq!(evs[0].turn_id.as_ref().unwrap().len(), 128);
    assert_eq!(evs[0].description.as_ref().unwrap().len(), 500);
    assert_eq!(evs[0].child_session_id, None);
}

#[test]
fn the_event_serialises_camel_case_with_nulls() {
    let v = serde_json::to_value(HarnessHermesHook {
        room_id: "r".into(),
        harness_id: "h".into(),
        event: "pre_llm_call".into(),
        ..Default::default()
    })
    .unwrap();
    assert_eq!(v["roomId"], "r");
    assert_eq!(v["turnExitReason"], Value::Null);
    assert_eq!(v["childSessionId"], Value::Null);
}

#[tokio::test]
async fn an_unauthenticated_oversized_hook_body_is_refused_with_the_auth_status() {
    let f = fixture();
    let (base, state) = serve(Arc::clone(&f.db)).await;
    let big = "x".repeat(4 * 1024 * 1024);
    let http = reqwest::Client::new();
    for path in [
        "/api/harness/permission",
        "/api/harness/session-start",
        "/api/harness/hermes-turn",
    ] {
        for token in [None, Some("not-a-real-token")] {
            let mut req = http.post(format!("{base}{path}")).body(big.clone());
            if let Some(t) = token {
                req = req.bearer_auth(t);
            }
            // The server answers before reading the body, so the client
            // may see the 401 or its upload cut off mid-send; either way
            // nothing was accepted.
            match req.send().await {
                Ok(res) => assert_eq!(res.status(), 401, "{path} {token:?}"),
                Err(e) => assert!(e.is_request(), "{path} {token:?}: {e}"),
            }
        }
    }
    assert_eq!(events(&state).len(), 0);
}

#[tokio::test]
async fn a_non_hook_route_keeps_the_one_mib_body_limit() {
    let f = fixture();
    save(&f.db, &[room("r1", vec![harness("h1", "hermes", "main")])]);
    let token = f.db.ensure_room_token("r1", 1).unwrap();
    let (base, _state) = serve(Arc::clone(&f.db)).await;
    let res = reqwest::Client::new()
        .post(format!("{base}/api/messages"))
        .bearer_auth(&token)
        .header(auth::HARNESS_HEADER, "h1")
        .header("content-type", "application/json")
        .body(format!(
            r#"{{"to":"r1","body":"{}"}}"#,
            "x".repeat(2 * 1024 * 1024)
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 413);
}
