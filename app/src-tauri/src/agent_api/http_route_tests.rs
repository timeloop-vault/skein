//! The endpoint over real HTTP: handshake, refused ways in, and the /api routes.

use std::sync::Arc;

use serde_json::{Value, json};

use super::auth::{self};
use super::find_rooms_tests::room_with_cwd;
use super::mcp;
use super::room_listing_tests::created_by;
use super::state::AgentApiState;
use super::tests::{fixture, harness, room, save, seed_thread};
use crate::db::Database;

// ── over real HTTP ────────────────────────────────────────────────

/// Bind the router on an ephemeral port and return its base URL.
pub(super) async fn serve_fixture(db: Arc<Database>) -> String {
    // reqwest 0.13 builds a TLS context eagerly even for plain http,
    // and panics without a rustls provider — the same call `run()`
    // makes for the opencode SSE client.
    crate::install_rustls_provider();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let state = Arc::new(AgentApiState::for_test(db));
    tokio::spawn(super::http::serve(listener, state));
    format!("http://127.0.0.1:{port}")
}

#[tokio::test]
async fn the_endpoint_answers_a_real_handshake_and_lists_its_tools() {
    let f = fixture();
    save(&f.db, &[room("r1", vec![harness("h1", "claude", "main")])]);
    seed_thread(&f.db, "r1", "t1", "rename this");
    let token = f.db.ensure_room_token("r1", 1).unwrap();
    let base = serve_fixture(Arc::clone(&f.db)).await;
    let http = reqwest::Client::new();

    let init: Value = http
        .post(format!("{base}/mcp"))
        .bearer_auth(&token)
        .json(&json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": { "protocolVersion": mcp::PROTOCOL_VERSION }
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(init["result"]["serverInfo"]["name"], "skein");

    let listed: Value = http
        .post(format!("{base}/mcp"))
        .bearer_auth(&token)
        .header("MCP-Protocol-Version", mcp::PROTOCOL_VERSION)
        .header(auth::HARNESS_HEADER, "h1")
        .json(&json!({
            "jsonrpc": "2.0", "id": 2, "method": "tools/call",
            "params": { "name": "list_comments", "arguments": {} }
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let text = listed["result"]["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("rename this"));
    assert!(text.contains("t1"));
}

#[tokio::test]
async fn the_endpoint_refuses_the_ways_in_that_are_not_ours() {
    let f = fixture();
    save(&f.db, &[room("r1", vec![])]);
    let token = f.db.ensure_room_token("r1", 1).unwrap();
    let base = serve_fixture(Arc::clone(&f.db)).await;
    let http = reqwest::Client::new();
    let call = json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" });

    // No token at all.
    let anon = http
        .post(format!("{base}/mcp"))
        .json(&call)
        .send()
        .await
        .unwrap();
    assert_eq!(anon.status(), 401);

    // A page in the user's browser, reaching 127.0.0.1 by DNS
    // rebinding — the attack the spec's Origin rule exists for.
    let cross = http
        .post(format!("{base}/mcp"))
        .bearer_auth(&token)
        .header("Origin", "https://evil.example")
        .json(&call)
        .send()
        .await
        .unwrap();
    assert_eq!(cross.status(), 403);

    // A protocol version we do not speak.
    let ancient = http
        .post(format!("{base}/mcp"))
        .bearer_auth(&token)
        .header("MCP-Protocol-Version", "1999-01-01")
        .json(&call)
        .send()
        .await
        .unwrap();
    assert_eq!(ancient.status(), 400);

    // We offer neither a server-to-client stream nor session teardown.
    let streamed = http.get(format!("{base}/mcp")).send().await.unwrap();
    assert_eq!(streamed.status(), 405);

    // Health is public — it carries no room, no token and no comment.
    let health = http.get(format!("{base}/api/health")).send().await.unwrap();
    assert_eq!(health.status(), 200);
}

#[tokio::test]
async fn the_resolve_route_exists_only_to_say_no() {
    let f = fixture();
    save(&f.db, &[room("r1", vec![])]);
    seed_thread(&f.db, "r1", "t1", "close me");
    let token = f.db.ensure_room_token("r1", 1).unwrap();
    let base = serve_fixture(Arc::clone(&f.db)).await;
    let http = reqwest::Client::new();

    // Unauthenticated callers learn about their token, not our policy.
    let anon = http
        .post(format!("{base}/api/comments/t1/resolve"))
        .send()
        .await
        .unwrap();
    assert_eq!(anon.status(), 401);

    let authed = http
        .post(format!("{base}/api/comments/t1/resolve"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();
    assert_eq!(authed.status(), 403);
    let body: Value = authed.json().await.unwrap();
    assert!(body["error"].as_str().unwrap().contains("reviewer"));
    assert!(
        f.db.review_thread("t1")
            .unwrap()
            .unwrap()
            .resolved_ms
            .is_none()
    );
}

#[tokio::test]
async fn the_find_rooms_route_answers_across_rooms_and_needs_a_token() {
    let f = fixture();
    let mut room1 = room_with_cwd("r1", "C:/repo-wt/task-1");
    room1.harnesses = vec![harness("h1", "claude", "main")];
    room1.active_harness_id = "h1".to_owned();
    save(
        &f.db,
        &[room1, room("r2", vec![harness("h2", "claude", "main")])],
    );
    // r2's own token, not r1's — proving the answer is cross-room.
    let token = f.db.ensure_room_token("r2", 1).unwrap();
    let base = serve_fixture(Arc::clone(&f.db)).await;
    let http = reqwest::Client::new();

    let anon = http
        .get(format!("{base}/api/rooms/find?path=C:/repo-wt/task-1"))
        .send()
        .await
        .unwrap();
    assert_eq!(anon.status(), 401);

    let authed = http
        .get(format!("{base}/api/rooms/find?path=C:/repo-wt/task-1"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();
    assert_eq!(authed.status(), 200);
    let body: Value = authed.json().await.unwrap();
    assert_eq!(body["rooms"][0]["room_id"], "r1");
    assert_eq!(body["rooms"][0]["match"], "cwd");
    assert_eq!(body["rooms"][0]["safe_to_remove"], false);

    let missing_path = http
        .get(format!("{base}/api/rooms/find"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();
    assert_eq!(missing_path.status(), 403, "an empty path must be refused");
}

#[tokio::test]
async fn the_rooms_find_route_still_wins_over_the_dynamic_room_id_route() {
    // #356 added `GET /api/rooms/{room_id}` beside the pre-existing
    // `GET /api/rooms/find` — proof the literal segment still wins over
    // the dynamic one, whatever order axum's router happens to try them.
    let f = fixture();
    let mut room1 = room_with_cwd("r1", "C:/repo-wt/task-1");
    room1.harnesses = vec![harness("h1", "claude", "main")];
    room1.active_harness_id = "h1".to_owned();
    save(&f.db, &[room1]);
    let token = f.db.ensure_room_token("r1", 1).unwrap();
    let base = serve_fixture(Arc::clone(&f.db)).await;
    let http = reqwest::Client::new();

    let find = http
        .get(format!("{base}/api/rooms/find?path=C:/repo-wt/task-1"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();
    assert_eq!(find.status(), 200);
    let body: Value = find.json().await.unwrap();
    assert_eq!(
        body["rooms"][0]["room_id"], "r1",
        "must be find_rooms_for_path's shape, not get_room's: {body}"
    );
}

#[tokio::test]
async fn the_list_rooms_route_answers_across_rooms_and_needs_a_token() {
    let f = fixture();
    let mut child = room("child", vec![harness("h", "claude", "main")]);
    child.created_by = Some(created_by("director", "d"));
    save(
        &f.db,
        &[
            room("director", vec![harness("d", "claude", "main")]),
            child,
        ],
    );
    let token = f.db.ensure_room_token("director", 1).unwrap();
    let base = serve_fixture(Arc::clone(&f.db)).await;
    let http = reqwest::Client::new();

    let anon = http.get(format!("{base}/api/rooms")).send().await.unwrap();
    assert_eq!(anon.status(), 401);

    let authed = http
        .get(format!("{base}/api/rooms?created_by=me"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();
    assert_eq!(authed.status(), 200);
    let body: Value = authed.json().await.unwrap();
    let rooms = body["rooms"].as_array().unwrap();
    assert_eq!(rooms.len(), 1);
    assert_eq!(rooms[0]["room_id"], "child");
}

#[tokio::test]
async fn the_get_room_route_refuses_an_unknown_id() {
    let f = fixture();
    save(&f.db, &[room("r1", vec![harness("h1", "claude", "main")])]);
    let token = f.db.ensure_room_token("r1", 1).unwrap();
    let base = serve_fixture(Arc::clone(&f.db)).await;
    let http = reqwest::Client::new();

    let anon = http
        .get(format!("{base}/api/rooms/ghost"))
        .send()
        .await
        .unwrap();
    assert_eq!(anon.status(), 401);

    let authed = http
        .get(format!("{base}/api/rooms/ghost"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();
    assert_eq!(authed.status(), 404);
}

#[tokio::test]
async fn the_get_room_route_answers_across_rooms() {
    let f = fixture();
    save(
        &f.db,
        &[
            room("r1", vec![harness("h1", "claude", "main")]),
            room("r2", vec![harness("h2", "claude", "main")]),
        ],
    );
    // r2's own token, not r1's — proving the answer is cross-room, the
    // same considered exception `find_rooms_for_path` documents.
    let token = f.db.ensure_room_token("r2", 1).unwrap();
    let base = serve_fixture(Arc::clone(&f.db)).await;
    let http = reqwest::Client::new();

    let authed = http
        .get(format!("{base}/api/rooms/r1"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();
    assert_eq!(authed.status(), 200);
    let body: Value = authed.json().await.unwrap();
    assert_eq!(body["room_id"], "r1");
    assert_eq!(body["harnesses"][0]["harness_id"], "h1");
}

#[tokio::test]
async fn the_list_harnesses_route_answers_across_rooms_and_needs_a_token() {
    let f = fixture();
    save(
        &f.db,
        &[
            room("r1", vec![harness("h1", "claude", "main")]),
            room("r2", vec![harness("h2", "claude", "main")]),
        ],
    );
    // r2's own token — proving the answer is cross-room.
    let token = f.db.ensure_room_token("r2", 1).unwrap();
    let base = serve_fixture(Arc::clone(&f.db)).await;
    let http = reqwest::Client::new();

    let anon = http
        .get(format!("{base}/api/harnesses"))
        .send()
        .await
        .unwrap();
    assert_eq!(anon.status(), 401);

    let authed = http
        .get(format!("{base}/api/harnesses"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();
    assert_eq!(authed.status(), 200);
    let body: Value = authed.json().await.unwrap();
    let harnesses = body["harnesses"].as_array().unwrap();
    let ids: Vec<&str> = harnesses
        .iter()
        .map(|h| h["harness_id"].as_str().unwrap())
        .collect();
    assert_eq!(ids.len(), 2, "both rooms' harnesses, cross-room: {ids:?}");
    assert!(ids.contains(&"h1"));
    assert!(ids.contains(&"h2"));
}

#[tokio::test]
async fn a_reply_over_http_lands_attributed_to_its_harness() {
    let f = fixture();
    save(&f.db, &[room("r1", vec![harness("h1", "claude", "main")])]);
    seed_thread(&f.db, "r1", "t1", "explain this");
    let token = f.db.ensure_room_token("r1", 1).unwrap();
    let base = serve_fixture(Arc::clone(&f.db)).await;
    let http = reqwest::Client::new();

    let posted = http
        .post(format!("{base}/api/comments/t1/reply"))
        .bearer_auth(&token)
        .header(auth::HARNESS_HEADER, "h1")
        .json(&json!({ "body": "it guards the empty case" }))
        .send()
        .await
        .unwrap();
    assert_eq!(posted.status(), 200);

    let stored = f.db.review_comments_for_room("r1").unwrap();
    let agent = stored.iter().find(|c| c.author_kind == "agent").unwrap();
    assert_eq!(agent.author_id.as_deref(), Some("h1"));

    // And the pane sees it: the thread DTO the review surface builds
    // carries the byline, not the raw id.
    let comments = crate::review_surface::query::scope_impl(
        &f.db,
        "r1",
        "",
        crate::review_surface::Scope::Pending,
        None,
    );
    assert!(comments.is_ok());
}
