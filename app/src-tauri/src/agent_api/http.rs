//! The routes, and nothing else.
//!
//! Two surfaces over one set of verbs:
//!
//! * `POST /mcp` — what the harnesses talk to. Claude Code
//!   (`type: "http"`) and opencode (`type: "remote"`) both speak MCP
//!   over HTTP with custom headers, which is the whole reason there is
//!   no separate bridge process here.
//! * `/api/*` — the same verbs as ordinary JSON, for the `skein` CLI
//!   D10 defers. It costs one route each and keeps the MCP layer from
//!   becoming the only way in.
//!
//! The listener binds `127.0.0.1` only, and every route validates
//! `Origin`. Both are required of a local MCP server: without them a
//! page in the user's browser can reach this port by DNS rebinding.

//! # Where things are
//!
//! | File | The question it answers |
//! |---|---|
//! | `http/review.rs` | What the reviewer said, what the diff is, has it been signed off — and the two routes that only refuse. |
//! | `http/info.rs` | Which Skein build the agent runs under (#535). |
//! | `http/mail.rs` | How a harness sends, reads and pages back through mail over plain JSON (#327, #364). |
//! | `http/rooms.rs` | How rooms are opened, closed, found, listed and read (#330, #354, #356, #411). |
//! | `http/design.rs` | How the design pane is read and driven over plain JSON (#512). |
//! | `http/harnesses.rs` | How harnesses are opened, closed and listed (#356, #411). |
//! | `http/hooks.rs` | What the injected plugin's `PermissionRequest` and `SessionStart` hooks post (#86, #273). |
//!
//! This file keeps the router, the MCP endpoint and the plumbing every
//! route shares (authentication, error bodies, `with_caller`).

mod design;
mod harnesses;
mod hooks;
mod info;
mod mail;
mod review;
mod rooms;

use std::sync::Arc;

use axum::Router;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use serde_json::{Value, json};

use super::auth::{self, AuthError, Caller, HARNESS_HEADER};
use super::mcp;
use super::state::AgentApiState;
use super::verbs::{MailContext, MailPolicy, VerbError};
use design::{api_design_entry, api_design_harnesses, api_design_show, api_design_state};
use harnesses::{api_close_harness, api_list_harnesses, api_open_harness};
use hooks::{api_harness_permission, api_harness_session_end, api_harness_session_start};
use info::api_info;
use mail::{api_message_history, api_read_messages, api_send_message};
use review::{
    api_addressed, api_diff, api_get, api_list, api_reply, api_resolve, api_signoff, api_status,
};
use rooms::{
    api_close_room, api_create_room, api_find_rooms_for_path, api_get_room, api_list_rooms,
};

/// Header the MCP spec has clients send on every request after
/// `initialize`.
const PROTOCOL_HEADER: &str = "mcp-protocol-version";

/// The largest body we will read. An MCP request is a few hundred
/// bytes; a reply is a comment. Anything past this is a mistake or an
/// attack, and either way is better refused than buffered.
const MAX_BODY: usize = 1024 * 1024;

pub fn router(state: Arc<AgentApiState>) -> Router {
    Router::new()
        .route("/mcp", post(mcp_post).get(no_stream).delete(no_stream))
        .route("/api/health", get(health))
        .route("/api/comments", get(api_list))
        .route("/api/comments/{thread_id}", get(api_get))
        .route("/api/comments/{thread_id}/reply", post(api_reply))
        .route("/api/comments/{thread_id}/addressed", post(api_addressed))
        .route("/api/comments/{thread_id}/resolve", post(api_resolve))
        .route("/api/diff", get(api_diff))
        // POST is present and always refuses — see `api_signoff`.
        .route("/api/status", get(api_status).post(api_signoff))
        .route("/api/info", get(api_info))
        .route(
            "/api/messages",
            post(api_send_message).get(api_read_messages),
        )
        .route("/api/messages/history", get(api_message_history))
        .route("/api/rooms", get(api_list_rooms).post(api_create_room))
        .route("/api/rooms/find", get(api_find_rooms_for_path))
        .route("/api/rooms/{room_id}", get(api_get_room))
        .route("/api/rooms/{room_id}/close", post(api_close_room))
        .route(
            "/api/harnesses",
            get(api_list_harnesses).post(api_open_harness),
        )
        .route("/api/harnesses/{harness_id}/close", post(api_close_harness))
        .route("/api/design/harnesses", get(api_design_harnesses))
        .route("/api/design/state", get(api_design_state))
        .route("/api/design/entry", post(api_design_entry))
        .route("/api/design/show", post(api_design_show))
        .route("/api/harness/permission", post(api_harness_permission))
        .route(
            "/api/harness/session-start",
            post(api_harness_session_start),
        )
        .route("/api/harness/session-end", post(api_harness_session_end))
        .layer(axum::extract::DefaultBodyLimit::max(MAX_BODY))
        .with_state(state)
}

/// Serve until the process ends. Skein's PTYs die with it too, so
/// there is nothing to drain.
pub async fn serve(listener: tokio::net::TcpListener, state: Arc<AgentApiState>) {
    if let Err(e) = axum::serve(listener, router(state)).await {
        tracing::error!(error = %e, "agent api: server stopped");
    }
}

// ── the MCP endpoint ──────────────────────────────────────────────

async fn mcp_post(
    State(state): State<Arc<AgentApiState>>,
    headers: HeaderMap,
    body: String,
) -> Response {
    if !protocol_ok(&headers) {
        return error_body(
            StatusCode::BAD_REQUEST,
            "unsupported MCP-Protocol-Version — this server speaks 2024-11-05 \
             through 2025-06-18",
        );
    }
    let caller = match authenticate(&state, &headers) {
        Ok(c) => c,
        Err(e) => return refuse(&e),
    };
    // Peek at the tool name before dispatch so a write can nudge the
    // pane afterwards. Cheap, and it keeps `mcp::handle` free of any
    // notion of a UI.
    let tool = serde_json::from_str::<Value>(&body).ok().and_then(|v| {
        v.get("params")
            .and_then(|p| p.get("name"))
            .and_then(Value::as_str)
            .map(str::to_owned)
    });
    let mail = mail_context(&state);
    let outcome = mcp::handle(&state, &caller, &body, &mail).await;
    // Only a write that actually landed. A refused `reply` changes
    // nothing, and a pane that flickers on every failed call teaches
    // the user to distrust the ones that mean something.
    if tool.as_deref().is_some_and(mcp::is_write) && succeeded(&outcome) {
        state.notify_review_changed(&caller.room_id);
    }
    notify_mail_tools(&state, &caller, tool.as_deref(), &outcome);
    match outcome {
        mcp::Outcome::Json(v) => (StatusCode::OK, axum::Json(*v)).into_response(),
        mcp::Outcome::Accepted => StatusCode::ACCEPTED.into_response(),
        mcp::Outcome::BadRequest(msg) => error_body(StatusCode::BAD_REQUEST, &msg),
    }
}

/// A `tools/call` result that is neither a JSON-RPC error nor an
/// `isError` tool result.
fn succeeded(outcome: &mcp::Outcome) -> bool {
    match outcome {
        mcp::Outcome::Json(v) => {
            v.get("error").is_none() && v["result"]["isError"] != Value::Bool(true)
        }
        _ => false,
    }
}

/// The mailbox policy (#327), read fresh from the live spawn settings
/// on every request — see `AgentApiState::spawn_settings`.
fn mail_context(state: &AgentApiState) -> MailContext {
    let settings = state.spawn_settings();
    MailContext {
        policy: MailPolicy {
            messaging_enabled: settings.allow_agent_messaging,
            claude_injected: settings.inject_claude_plugin,
            opencode_injected: settings.inject_opencode_config,
        },
        agent_sees_mcp: crate::agents::agent_sees_mcp,
        app: state.app.clone(),
    }
}

/// `send_message`/`read_messages` notify a *different* inbox than the
/// generic `is_write` path does (that one always tells the caller's own
/// room; a send's target is almost always someone else's). Rather than
/// re-run the verb, this re-parses the tool result MCP already wrapped
/// into text — the same JSON `to_room_id`/`to_harness_id` /
/// `newly_marked_read` fields the plain `/api/messages` handlers below
/// read directly off the typed struct.
fn notify_mail_tools(
    state: &AgentApiState,
    caller: &Caller,
    tool: Option<&str>,
    outcome: &mcp::Outcome,
) {
    if !succeeded(outcome) {
        return;
    }
    let Some(name) = tool.map(|n| n.rsplit("__").next().unwrap_or(n)) else {
        return;
    };
    let Some(result) = tool_result_value(outcome) else {
        return;
    };
    match name {
        "send_message" => {
            if let (Some(room_id), Some(harness_id)) =
                (result["toRoomId"].as_str(), result["toHarnessId"].as_str())
            {
                state.notify_mail_changed(room_id, harness_id);
            }
        }
        "read_messages" if result["newlyMarkedRead"].as_u64().is_some_and(|n| n > 0) => {
            if let Some(harness_id) = caller.harness_id.as_deref() {
                state.notify_mail_changed(&caller.room_id, harness_id);
            }
        }
        _ => {}
    }
}

/// Pull the structured tool result back out of an MCP `Outcome` — the
/// same value `call_tool` produced before `tool_content` wrapped it as
/// pretty-printed text inside the JSON-RPC envelope.
fn tool_result_value(outcome: &mcp::Outcome) -> Option<Value> {
    let mcp::Outcome::Json(v) = outcome else {
        return None;
    };
    let text = v["result"]["content"][0]["text"].as_str()?;
    serde_json::from_str(text).ok()
}

/// The spec allows a server to decline the server-to-client SSE stream
/// and to decline session termination. We have neither.
async fn no_stream() -> Response {
    error_body(
        StatusCode::METHOD_NOT_ALLOWED,
        "this endpoint is request/response only — POST a JSON-RPC message",
    )
}

// ── the plain JSON surface ────────────────────────────────────────

async fn health() -> Response {
    (
        StatusCode::OK,
        axum::Json(json!({
            "name": mcp::SERVER_NAME,
            "version": crate::build_info::VERSION,
            "protocolVersion": mcp::PROTOCOL_VERSION,
            "mcp": "/mcp",
        })),
    )
        .into_response()
}

// ── shared plumbing ───────────────────────────────────────────────

fn protocol_ok(headers: &HeaderMap) -> bool {
    match header_str(headers, PROTOCOL_HEADER) {
        None => true,
        Some(v) => mcp::SUPPORTED_VERSIONS.contains(&v),
    }
}

fn authenticate(state: &AgentApiState, headers: &HeaderMap) -> Result<Caller, AuthError> {
    auth::check_origin(header_str(headers, header::ORIGIN.as_str()))?;
    let token = auth::bearer(header_str(headers, header::AUTHORIZATION.as_str()));
    let caller = auth::authenticate(&state.db, token, header_str(headers, HARNESS_HEADER));
    if let Err(e) = &caller {
        tracing::warn!(
            token = token.map(auth::token_prefix).unwrap_or_default(),
            status = e.status(),
            "agent api: refused"
        );
    }
    caller
}

fn header_str<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers.get(name).and_then(|v| v.to_str().ok())
}

/// Whether a route's success should tell the pane to re-read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Notify {
    Never,
    /// Only when the verb actually landed — a refused reply changes
    /// nothing, and a pane that flickers on failures teaches the user
    /// to ignore the refreshes that mean something.
    OnSuccess,
}

fn with_caller(
    state: &Arc<AgentApiState>,
    headers: &HeaderMap,
    notify: Notify,
    run: impl FnOnce(&Caller) -> Result<Value, VerbError>,
) -> Response {
    let caller = match authenticate(state, headers) {
        Ok(c) => c,
        Err(e) => return refuse(&e),
    };
    match run(&caller) {
        Ok(v) => {
            if notify == Notify::OnSuccess {
                state.notify_review_changed(&caller.room_id);
            }
            (StatusCode::OK, axum::Json(v)).into_response()
        }
        Err(e) => error_body(
            StatusCode::from_u16(e.status()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
            e.message(),
        ),
    }
}

fn json_of<T: serde::Serialize>(value: T) -> Result<Value, VerbError> {
    serde_json::to_value(value).map_err(|e| VerbError::Internal(e.to_string()))
}

fn refuse(e: &AuthError) -> Response {
    error_body(
        StatusCode::from_u16(e.status()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
        &e.message(),
    )
}

fn error_body(status: StatusCode, message: &str) -> Response {
    (status, axum::Json(json!({ "error": message }))).into_response()
}
