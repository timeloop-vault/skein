//! `skein_info` (#535): the MCP tool and `/api/info` agree with `build_info`.

use std::sync::Arc;

use serde_json::{Value, json};

use super::http_route_tests::serve_fixture;
use super::mcp;
use super::tests::{agent_api_state, caller_for, fixture, harness, room, save};
use super::verbs::MailContext;
use crate::build_info;

fn assert_info(v: &Value) {
    assert_eq!(v["version"], build_info::VERSION);
    assert_eq!(v["profile"], "dev");
    assert_eq!(v["identifier"], "com.timeloop-vault.skein.dev");
    match build_info::commit() {
        Some(c) => assert_eq!(v["commit"], c),
        None => assert!(v["commit"].is_null()),
    }
}

#[tokio::test]
async fn the_mcp_tool_reports_the_running_build() {
    let f = fixture();
    save(&f.db, &[room("r1", vec![harness("h1", "claude", "main")])]);
    let state = agent_api_state(&f);
    let caller = caller_for(&f.db, "r1", Some("h1"));
    let v = mcp::call_tool(
        &state,
        &caller,
        "skein_info",
        &json!({}),
        &MailContext::permissive(),
    )
    .await
    .unwrap();
    assert_info(&v);
}

#[tokio::test]
async fn the_http_route_reports_the_same_and_refuses_a_bad_token() {
    let f = fixture();
    save(&f.db, &[room("r1", vec![])]);
    let token = f.db.ensure_room_token("r1", 1).unwrap();
    let base = serve_fixture(Arc::clone(&f.db)).await;
    let http = reqwest::Client::new();

    let ok = http
        .get(format!("{base}/api/info"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();
    assert_eq!(ok.status(), 200);
    assert_info(&ok.json::<Value>().await.unwrap());

    let anon = http.get(format!("{base}/api/info")).send().await.unwrap();
    assert_eq!(anon.status(), 401);
    let bad = http
        .get(format!("{base}/api/info"))
        .bearer_auth("not-a-token")
        .send()
        .await
        .unwrap();
    assert_eq!(bad.status(), 401);
}
