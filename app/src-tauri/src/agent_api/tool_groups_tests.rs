//! Tool groups (#534): a group that is off is hidden and refused, on
//! `/mcp` and on the `/api` mirror.

use std::sync::Arc;

use serde_json::{Value, json};

use super::groups::{ToolGroup, ToolGroups, group_of};
use super::mcp;
use super::state::AgentApiState;
use super::tests::{caller_for, fixture, handled, harness, room, save};
use super::verbs::MailContext;

fn only_off(g: ToolGroup) -> ToolGroups {
    ToolGroups {
        review: g != ToolGroup::Review,
        mail: g != ToolGroup::Mail,
        rooms: g != ToolGroup::Rooms,
        directory: g != ToolGroup::Directory,
        design: g != ToolGroup::Design,
    }
}

fn names(list: &Value) -> Vec<String> {
    list["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap().to_owned())
        .collect()
}

fn state_with(db: Arc<crate::db::Database>, groups: ToolGroups) -> AgentApiState {
    let state = AgentApiState::for_test(db);
    *state.test_tool_groups.lock() = Some(groups);
    state
}

async fn list(state: &AgentApiState, caller: &super::auth::Caller) -> Value {
    handled(
        state,
        caller,
        &json!({"jsonrpc":"2.0","id":1,"method":"tools/list"}),
    )
    .await
}

async fn instructions_for(
    db: &Arc<crate::db::Database>,
    caller: &super::auth::Caller,
    groups: ToolGroups,
) -> String {
    let state = state_with(Arc::clone(db), groups);
    handled(
        &state,
        caller,
        &json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}),
    )
    .await["result"]["instructions"]
        .as_str()
        .unwrap()
        .to_owned()
}

#[tokio::test]
async fn all_groups_on_lists_every_tool() {
    let f = fixture();
    save(&f.db, &[room("r1", vec![harness("h1", "claude", "main")])]);
    let caller = caller_for(&f.db, "r1", Some("h1"));
    let state = state_with(Arc::clone(&f.db), ToolGroups::default());
    assert_eq!(
        names(&list(&state, &caller).await).len(),
        mcp::tool_specs().len()
    );
}

#[tokio::test]
async fn an_off_group_is_hidden_and_the_rest_stay() {
    let f = fixture();
    save(&f.db, &[room("r1", vec![harness("h1", "claude", "main")])]);
    let caller = caller_for(&f.db, "r1", Some("h1"));
    for g in ToolGroup::ALL {
        let state = state_with(Arc::clone(&f.db), only_off(g));
        let listed = names(&list(&state, &caller).await);
        assert!(listed.contains(&"skein_info".to_owned()), "{}", g.key());
        for spec in mcp::tool_specs() {
            let name = spec["name"].as_str().unwrap();
            let expect = group_of(name) != Some(g);
            assert_eq!(
                listed.iter().any(|n| n == name),
                expect,
                "{name} / {}",
                g.key()
            );
        }
    }
}

#[tokio::test]
async fn instructions_omit_a_disabled_group() {
    let f = fixture();
    save(&f.db, &[room("r1", vec![harness("h1", "claude", "main")])]);
    let caller = caller_for(&f.db, "r1", Some("h1"));
    let probes = [
        (ToolGroup::Review, "review_status"),
        (ToolGroup::Mail, "send_message"),
        (ToolGroup::Rooms, "create_room"),
        (ToolGroup::Directory, "find_rooms_for_path"),
        (ToolGroup::Design, "show_design_pane"),
    ];
    for (g, probe) in probes {
        let on = instructions_for(&f.db, &caller, ToolGroups::default()).await;
        assert!(on.contains(probe), "{probe}");
        let off = instructions_for(&f.db, &caller, only_off(g)).await;
        assert!(!off.contains(probe), "{probe} leaked");
    }
}

#[tokio::test]
async fn calling_an_off_group_tool_is_refused_even_namespaced() {
    let f = fixture();
    save(&f.db, &[room("r1", vec![harness("h1", "claude", "main")])]);
    let caller = caller_for(&f.db, "r1", Some("h1"));
    let state = state_with(Arc::clone(&f.db), only_off(ToolGroup::Review));
    for name in ["review_status", "mcp__plugin_skein_api__review_status"] {
        let err = mcp::call_tool(
            &state,
            &caller,
            name,
            &json!({}),
            &MailContext::permissive(),
        )
        .await
        .unwrap_err();
        assert_eq!(err.status(), 403);
        assert!(err.message().contains("turned off in Skein's Settings"));
        assert!(err.message().contains("for every harness."));
        assert!(err.message().contains("Review"));
    }
    // skein_info is in no group and always works.
    mcp::call_tool(
        &state,
        &caller,
        "skein_info",
        &json!({}),
        &MailContext::permissive(),
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn alias_refusals_keep_precedence_over_the_group_refusal() {
    let f = fixture();
    save(&f.db, &[room("r1", vec![harness("h1", "claude", "main")])]);
    let caller = caller_for(&f.db, "r1", Some("h1"));
    let state = state_with(
        Arc::clone(&f.db),
        ToolGroups {
            review: false,
            rooms: false,
            ..ToolGroups::default()
        },
    );
    for (name, needle) in [
        ("resolve", "reviewer"),
        ("approve", "reviewer"),
        ("archive_room", "close_room"),
    ] {
        let err = mcp::call_tool(
            &state,
            &caller,
            name,
            &json!({}),
            &MailContext::permissive(),
        )
        .await
        .unwrap_err();
        assert!(err.message().contains(needle), "{name}: {}", err.message());
        assert!(!err.message().contains("turned off"), "{name}");
    }
}

async fn serve_with(db: &Arc<crate::db::Database>, groups: ToolGroups) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let state = Arc::new(state_with(Arc::clone(db), groups));
    tokio::spawn(super::http::serve(listener, state));
    format!("http://127.0.0.1:{port}")
}

#[tokio::test]
async fn api_routes_refuse_when_their_group_is_off() {
    crate::install_rustls_provider();
    let f = fixture();
    save(&f.db, &[room("r1", vec![harness("h1", "claude", "main")])]);
    let token = f.db.ensure_room_token("r1", 1).unwrap();
    let http = reqwest::Client::new();
    let routes = [
        (ToolGroup::Review, "GET", "/api/comments"),
        (ToolGroup::Mail, "GET", "/api/messages"),
        (ToolGroup::Rooms, "POST", "/api/rooms"),
        (ToolGroup::Directory, "GET", "/api/rooms"),
        (ToolGroup::Design, "GET", "/api/design/harnesses"),
    ];
    for (g, method, path) in routes {
        let base = serve_with(&f.db, only_off(g)).await;
        let url = format!("{base}{path}");
        let req = if method == "POST" {
            http.post(&url).json(&json!({}))
        } else {
            http.get(&url)
        };
        let resp = req.bearer_auth(&token).send().await.unwrap();
        assert_eq!(resp.status(), 403, "{path}");
        let body: Value = resp.json().await.unwrap();
        assert!(
            body["error"].as_str().unwrap().contains("turned off"),
            "{path}: {body}"
        );
    }
}

#[tokio::test]
async fn a_shared_path_is_gated_per_method_and_info_is_never_gated() {
    crate::install_rustls_provider();
    let f = fixture();
    save(&f.db, &[room("r1", vec![harness("h1", "claude", "main")])]);
    let token = f.db.ensure_room_token("r1", 1).unwrap();
    let off = ToolGroups {
        review: false,
        mail: false,
        rooms: false,
        directory: true,
        design: false,
    };
    let base = serve_with(&f.db, off).await;
    let http = reqwest::Client::new();
    // GET /api/rooms is Directory (on); POST is Rooms (off).
    let got = http
        .get(format!("{base}/api/rooms"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();
    assert_ne!(got.status(), 403);
    let posted = http
        .post(format!("{base}/api/rooms"))
        .bearer_auth(&token)
        .json(&json!({}))
        .send()
        .await
        .unwrap();
    assert_eq!(posted.status(), 403);
    let info = http
        .get(format!("{base}/api/info"))
        .bearer_auth(&token)
        .send()
        .await
        .unwrap();
    assert_eq!(info.status(), 200);
}
