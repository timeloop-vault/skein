//! The MCP envelope: initialize, notifications, refused tools and unknown methods.

use serde_json::json;

use super::mcp;
use super::tests::{agent_api_state, caller_for, fixture, handled, room, save, seed_thread};
use super::verbs::MailContext;

// ── the MCP envelope ──────────────────────────────────────────────

#[tokio::test]
async fn initialize_echoes_a_version_it_knows_and_states_its_own_otherwise() {
    let f = fixture();
    save(&f.db, &[room("r1", vec![])]);
    let caller = caller_for(&f.db, "r1", None);
    let state = agent_api_state(&f);

    let old = handled(
        &state,
        &caller,
        &json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": { "protocolVersion": "2024-11-05" }
        }),
    )
    .await;
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
        &state,
        &caller,
        &json!({
            "jsonrpc": "2.0", "id": 2, "method": "initialize",
            "params": { "protocolVersion": "2099-01-01" }
        }),
    )
    .await;
    assert_eq!(future["result"]["protocolVersion"], mcp::PROTOCOL_VERSION);
}

#[tokio::test]
async fn a_notification_is_accepted_with_no_body_and_no_answer() {
    let f = fixture();
    save(&f.db, &[room("r1", vec![])]);
    let caller = caller_for(&f.db, "r1", None);
    let state = agent_api_state(&f);
    let out = mcp::handle(
        &state,
        &caller,
        &json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }).to_string(),
        &MailContext::permissive(),
    )
    .await;
    assert_eq!(out, mcp::Outcome::Accepted);
}

#[tokio::test]
async fn a_refused_tool_answers_the_model_rather_than_the_plumbing() {
    // isError inside a *successful* JSON-RPC result: a protocol-level
    // error is handled by the client and never shown to the model, and
    // a model that cannot see the reason will simply try again.
    let f = fixture();
    save(&f.db, &[room("r1", vec![])]);
    seed_thread(&f.db, "r1", "t1", "hi");
    let caller = caller_for(&f.db, "r1", None);
    let state = agent_api_state(&f);
    let out = handled(
        &state,
        &caller,
        &json!({
            "jsonrpc": "2.0", "id": 3, "method": "tools/call",
            "params": { "name": "resolve", "arguments": { "thread_id": "t1" } }
        }),
    )
    .await;
    assert!(out.get("error").is_none());
    assert_eq!(out["result"]["isError"], true);
    let text = out["result"]["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("reviewer"));
}

#[tokio::test]
async fn an_unknown_method_is_a_json_rpc_error_and_junk_is_a_bad_request() {
    let f = fixture();
    save(&f.db, &[room("r1", vec![])]);
    let caller = caller_for(&f.db, "r1", None);
    let state = agent_api_state(&f);
    let out = handled(
        &state,
        &caller,
        &json!({
            "jsonrpc": "2.0", "id": 4, "method": "resources/list"
        }),
    )
    .await;
    assert_eq!(out["error"]["code"], -32601);

    assert!(matches!(
        mcp::handle(&state, &caller, "{not json", &MailContext::permissive()).await,
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
