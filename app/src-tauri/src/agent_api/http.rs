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

use std::sync::Arc;

use axum::Router;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use serde::Deserialize;
use serde_json::{Value, json};

use super::auth::{self, AuthError, Caller, HARNESS_HEADER};
use super::mcp;
use super::state::AgentApiState;
use super::verbs::{self, AddressedArgs, DiffArgs, GetCommentArgs, ListArgs, ReplyArgs, VerbError};

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
    let outcome = mcp::handle(&state.db, &caller, &body);
    // Only a write that actually landed. A refused `reply` changes
    // nothing, and a pane that flickers on every failed call teaches
    // the user to distrust the ones that mean something.
    if tool.as_deref().is_some_and(mcp::is_write) && succeeded(&outcome) {
        state.notify_review_changed(&caller.room_id);
    }
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
            "version": env!("CARGO_PKG_VERSION"),
            "protocolVersion": mcp::PROTOCOL_VERSION,
            "mcp": "/mcp",
        })),
    )
        .into_response()
}

#[derive(Debug, Deserialize)]
struct ListQuery {
    status: Option<String>,
    file: Option<String>,
}

async fn api_list(
    State(state): State<Arc<AgentApiState>>,
    headers: HeaderMap,
    Query(q): Query<ListQuery>,
) -> Response {
    with_caller(&state, &headers, Notify::Never, |caller| {
        verbs::list_comments(
            &state.db,
            caller,
            &ListArgs {
                status: q.status.clone(),
                file: q.file.clone(),
            },
        )
        .and_then(json_of)
    })
}

async fn api_get(
    State(state): State<Arc<AgentApiState>>,
    headers: HeaderMap,
    Path(thread_id): Path<String>,
) -> Response {
    with_caller(&state, &headers, Notify::Never, |caller| {
        verbs::get_comment(
            &state.db,
            caller,
            &GetCommentArgs {
                thread_id: thread_id.clone(),
            },
        )
        .and_then(json_of)
    })
}

async fn api_diff(
    State(state): State<Arc<AgentApiState>>,
    headers: HeaderMap,
    Query(q): Query<DiffQuery>,
) -> Response {
    with_caller(&state, &headers, Notify::Never, |caller| {
        verbs::get_diff(
            &state.db,
            caller,
            &DiffArgs {
                file: q.file.clone(),
                scope: q.scope.clone(),
                commit_sha: q.commit_sha.clone(),
            },
        )
        .and_then(json_of)
    })
}

#[derive(Debug, Deserialize)]
struct DiffQuery {
    file: Option<String>,
    scope: Option<String>,
    commit_sha: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ReplyBody {
    body: String,
}

async fn api_reply(
    State(state): State<Arc<AgentApiState>>,
    headers: HeaderMap,
    Path(thread_id): Path<String>,
    axum::Json(input): axum::Json<ReplyBody>,
) -> Response {
    with_caller(&state, &headers, Notify::OnSuccess, |caller| {
        verbs::reply(
            &state.db,
            caller,
            &ReplyArgs {
                thread_id: thread_id.clone(),
                body: input.body.clone(),
            },
        )
        .and_then(json_of)
    })
}

#[derive(Debug, Deserialize)]
struct AddressedBody {
    commit_sha: Option<String>,
    note: Option<String>,
}

async fn api_addressed(
    State(state): State<Arc<AgentApiState>>,
    headers: HeaderMap,
    Path(thread_id): Path<String>,
    axum::Json(input): axum::Json<AddressedBody>,
) -> Response {
    with_caller(&state, &headers, Notify::OnSuccess, |caller| {
        verbs::mark_addressed(
            &state.db,
            caller,
            &AddressedArgs {
                thread_id: thread_id.clone(),
                commit_sha: input.commit_sha.clone(),
                note: input.note.clone(),
            },
        )
        .and_then(json_of)
    })
}

/// Whether the reviewer has signed off (#214) — the gate to read
/// before landing.
async fn api_status(State(state): State<Arc<AgentApiState>>, headers: HeaderMap) -> Response {
    with_caller(&state, &headers, Notify::Never, |caller| {
        verbs::review_status(&state.db, caller).and_then(json_of)
    })
}

/// Present, and always refuses.
///
/// The write half of the sign-off does not exist for anyone but the
/// reviewer. Same reasoning as `api_resolve` one level up: an agent
/// that could approve its own work would remove the gate the review
/// exists to be.
async fn api_signoff(State(state): State<Arc<AgentApiState>>, headers: HeaderMap) -> Response {
    if let Err(e) = authenticate(&state, &headers) {
        return refuse(&e);
    }
    error_body(
        StatusCode::FORBIDDEN,
        "signing off is the reviewer's decision — GET this endpoint to \
         see whether they have",
    )
}

/// Present, and always refuses.
///
/// A 404 here would read as "not implemented yet" and invite a retry
/// after the next release. The route exists so the answer is the
/// design: resolve belongs to the reviewer (D8).
async fn api_resolve(
    State(state): State<Arc<AgentApiState>>,
    headers: HeaderMap,
    Path(thread_id): Path<String>,
) -> Response {
    // Still authenticate: an unauthenticated caller should learn it has
    // no token, not read our policy.
    let _ = thread_id;
    if let Err(e) = authenticate(&state, &headers) {
        return refuse(&e);
    }
    error_body(
        StatusCode::FORBIDDEN,
        "resolving a thread is the reviewer's decision — reply and call \
         mark_addressed instead",
    )
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
