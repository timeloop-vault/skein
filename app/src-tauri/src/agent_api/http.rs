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
use super::verbs::{
    self, AddressedArgs, CreateRoomArgs, DiffArgs, GetCommentArgs, ListArgs, MailContext,
    MailPolicy, ReadMessagesArgs, ReplyArgs, SendMessageArgs, VerbError,
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
        .route(
            "/api/messages",
            post(api_send_message).get(api_read_messages),
        )
        .route("/api/rooms", post(api_create_room))
        .route("/api/harness/permission", post(api_harness_permission))
        .route(
            "/api/harness/session-start",
            post(api_harness_session_start),
        )
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

#[derive(Debug, Deserialize)]
struct SendMessageBody {
    to: String,
    body: String,
}

/// `POST /api/messages` — the plain-JSON mirror of `send_message`.
async fn api_send_message(
    State(state): State<Arc<AgentApiState>>,
    headers: HeaderMap,
    axum::Json(input): axum::Json<SendMessageBody>,
) -> Response {
    let caller = match authenticate(&state, &headers) {
        Ok(c) => c,
        Err(e) => return refuse(&e),
    };
    let mail = mail_context(&state);
    let out = verbs::send_message(
        &state.db,
        &caller,
        &SendMessageArgs {
            to: input.to,
            body: input.body,
        },
        mail.policy,
        mail.agent_sees_mcp,
        mail.app.as_ref(),
    );
    match out {
        Ok(v) => {
            state.notify_mail_changed(&v.to_room_id, &v.to_harness_id);
            match json_of(v) {
                Ok(json) => (StatusCode::OK, axum::Json(json)).into_response(),
                Err(e) => error_body(StatusCode::INTERNAL_SERVER_ERROR, e.message()),
            }
        }
        Err(e) => error_body(
            StatusCode::from_u16(e.status()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
            e.message(),
        ),
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReadMessagesQuery {
    /// `includeRead` on the wire — unlike every other query param on
    /// this API (`commit_sha` and friends stay `snake_case`, matching
    /// the MCP `arguments` shape), because the brief for #327 names
    /// this exact spelling and there is no MCP argument here for it to
    /// mirror.
    #[serde(default)]
    include_read: Option<bool>,
}

/// `GET /api/messages?includeRead=true` — the plain-JSON mirror of
/// `read_messages`.
async fn api_read_messages(
    State(state): State<Arc<AgentApiState>>,
    headers: HeaderMap,
    Query(q): Query<ReadMessagesQuery>,
) -> Response {
    let caller = match authenticate(&state, &headers) {
        Ok(c) => c,
        Err(e) => return refuse(&e),
    };
    let mail = mail_context(&state);
    let out = verbs::read_messages(
        &state.db,
        &caller,
        &ReadMessagesArgs {
            include_read: q.include_read,
        },
        mail.policy,
    );
    match out {
        Ok(v) => {
            if v.newly_marked_read > 0 {
                if let Some(harness_id) = caller.harness_id.as_deref() {
                    state.notify_mail_changed(&caller.room_id, harness_id);
                }
            }
            match json_of(v) {
                Ok(json) => (StatusCode::OK, axum::Json(json)).into_response(),
                Err(e) => error_body(StatusCode::INTERNAL_SERVER_ERROR, e.message()),
            }
        }
        Err(e) => error_body(
            StatusCode::from_u16(e.status()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
            e.message(),
        ),
    }
}

/// `POST /api/rooms` — the plain-JSON mirror of the MCP `create_room`
/// tool (#330). Takes `CreateRoomArgs` directly as the request body: it
/// already carries the camelCase wire shape the tool's own
/// `arguments` object does, so there is no separate body struct to keep
/// in sync the way `SendMessageBody` mirrors `SendMessageArgs`.
async fn api_create_room(
    State(state): State<Arc<AgentApiState>>,
    headers: HeaderMap,
    axum::Json(args): axum::Json<CreateRoomArgs>,
) -> Response {
    let caller = match authenticate(&state, &headers) {
        Ok(c) => c,
        Err(e) => return refuse(&e),
    };
    let mail = mail_context(&state);
    let room_creation_enabled = state.spawn_settings().allow_agent_room_creation;
    match verbs::create_room(&state, &caller, &args, &mail, room_creation_enabled).await {
        Ok(v) => match json_of(v) {
            Ok(json) => (StatusCode::OK, axum::Json(json)).into_response(),
            Err(e) => error_body(StatusCode::INTERNAL_SERVER_ERROR, e.message()),
        },
        Err(e) => error_body(
            StatusCode::from_u16(e.status()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
            e.message(),
        ),
    }
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

/// The injected Claude plugin's `PermissionRequest` hook posts here
/// (#86) — not an agent verb, so it carries no MCP tool and is not in
/// `mcp.rs`. Unlike the review verbs, the harness id here is not mere
/// attribution: it *is* what the event is about, so a header that is
/// absent or names a harness the room doesn't contain is a 400, not a
/// silent no-op.
///
/// The body is Claude Code's own hook payload shape, not ours to
/// police: only `tool_name`, `agent_type` and `agent_id` are read out
/// of it, and a body that fails to parse as JSON at all still succeeds
/// — a hook that changes shape in a future Claude Code release must not
/// start breaking the harness's turn. `tool_input` is deliberately
/// never read; it can hold secrets a permission dialog is about to ask
/// on.
async fn api_harness_permission(
    State(state): State<Arc<AgentApiState>>,
    headers: HeaderMap,
    body: String,
) -> Response {
    let caller = match authenticate(&state, &headers) {
        Ok(c) => c,
        Err(e) => return refuse(&e),
    };
    // `harness_label` is Some only when `X-Skein-Harness` named a
    // harness the room actually contains (see `identify_harness`) — the
    // same signal `harness_id` alone can't give, since an unknown hint
    // is still kept as attribution.
    let (Some(harness_id), Some(_)) = (caller.harness_id.clone(), caller.harness_label.as_ref())
    else {
        return error_body(
            StatusCode::BAD_REQUEST,
            "X-Skein-Harness must name a harness this room actually contains",
        );
    };
    let payload: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
    let tool_name = payload
        .get("tool_name")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let agent_type = payload
        .get("agent_type")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let agent_id = payload
        .get("agent_id")
        .and_then(Value::as_str)
        .map(str::to_owned);
    tracing::info!(
        harness_id = %harness_id,
        room_id = %caller.room_id,
        tool_name = tool_name.as_deref(),
        agent_type = agent_type.as_deref(),
        agent_id = agent_id.as_deref(),
        "agent api: harness permission ping"
    );
    state.notify_harness_permission(
        &caller.room_id,
        &harness_id,
        tool_name,
        agent_type,
        agent_id,
    );
    StatusCode::NO_CONTENT.into_response()
}

/// The injected Claude plugin's `SessionStart` hook posts here (#273)
/// — like the permission route above, this is not an agent verb: it
/// carries no MCP tool and is not in `mcp.rs`.
///
/// The body is Claude Code's own hook payload, not ours to police: a
/// body that fails to parse as JSON at all still succeeds — a future
/// Claude Code payload change must not start breaking a harness's
/// launch. Since #116, `session_id` and `source` are forwarded (see
/// `session_start_fields`) so the frontend can follow a `/clear` or an
/// in-tool `/resume` onto the new conversation id — `source == "clear"`
/// and `source == "resume"` are the only values it acts on.
///
/// Duplicate fires are expected and must stay harmless. Upstream
/// anthropics/claude-code#78455 reports `SessionStart` firing twice
/// within a few hundred ms for the same project — once for a
/// "phantom" session that never materialises, with a payload
/// indistinguishable from the real one — so nothing here or
/// downstream may hang a consume-once side effect on this route:
/// emitting the event twice is fine, since the frontend handler
/// (`noteLaunchSignal`) is idempotent — it always records a
/// timestamp, and only moves the phase when the transcript tail
/// hasn't already spoken for itself (or when recovering a harness its
/// own launch-silent watchdog gave up on), never overriding
/// `permission` or `exited`.
async fn api_harness_session_start(
    State(state): State<Arc<AgentApiState>>,
    headers: HeaderMap,
    body: String,
) -> Response {
    let caller = match authenticate(&state, &headers) {
        Ok(c) => c,
        Err(e) => return refuse(&e),
    };
    let (Some(harness_id), Some(_)) = (caller.harness_id.clone(), caller.harness_label.as_ref())
    else {
        return error_body(
            StatusCode::BAD_REQUEST,
            "X-Skein-Harness must name a harness this room actually contains",
        );
    };
    let (session_id, source) = session_start_fields(&body);
    tracing::info!(
        harness_id = %harness_id,
        room_id = %caller.room_id,
        session_id = session_id.as_deref(),
        source = source.as_deref(),
        "agent api: harness session-start ping"
    );
    state.notify_harness_session_start(&caller.room_id, &harness_id, session_id, source);
    StatusCode::NO_CONTENT.into_response()
}

/// The largest `source` value passed through — Claude's own values
/// (`startup`/`resume`/`clear`/`compact`/`fork`) are all well under
/// this; a longer value is treated the same as absent rather than
/// truncated.
const MAX_SOURCE_LEN: usize = 32;

/// The largest plausible session id. Claude's ids are UUIDs (36
/// chars); this is generous headroom, not a format promise.
const MAX_SESSION_ID_LEN: usize = 128;

/// Pulls `session_id` and `source` out of a `SessionStart` hook body,
/// tolerating a body that isn't JSON at all (returns `(None, None)`).
///
/// `session_id` is validated, not merely extracted: the frontend turns
/// it straight into a transcript file name (`<sid>.jsonl`), so a value
/// containing a path separator or `..` must never get through. Only a
/// non-empty string of at most [`MAX_SESSION_ID_LEN`] ASCII
/// alphanumerics, `-` and `_` is accepted; anything else — including a
/// non-string JSON value — becomes `None`. `source` is passed through
/// verbatim (no trimming) when it is a string of at most
/// [`MAX_SOURCE_LEN`]; longer or non-string values become `None`.
fn session_start_fields(body: &str) -> (Option<String>, Option<String>) {
    let payload: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    let session_id = payload
        .get("session_id")
        .and_then(Value::as_str)
        .filter(|s| {
            !s.is_empty()
                && s.len() <= MAX_SESSION_ID_LEN
                && s.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        })
        .map(str::to_owned);
    let source = payload
        .get("source")
        .and_then(Value::as_str)
        .filter(|s| s.len() <= MAX_SOURCE_LEN)
        .map(str::to_owned);
    (session_id, source)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_start_fields_reads_a_valid_id_and_source() {
        let body = r#"{"session_id":"9c1f2e3a-1111-2222-3333-444455556666","source":"clear"}"#;
        assert_eq!(
            session_start_fields(body),
            (
                Some("9c1f2e3a-1111-2222-3333-444455556666".to_owned()),
                Some("clear".to_owned())
            )
        );
    }

    #[test]
    fn session_start_fields_tolerates_missing_fields() {
        assert_eq!(session_start_fields("{}"), (None, None));
    }

    #[test]
    fn session_start_fields_tolerates_a_non_json_body() {
        assert_eq!(session_start_fields("not json at all"), (None, None));
    }

    #[test]
    fn session_start_fields_rejects_a_traversal_attempt() {
        for bad in ["../x", "a/b", "a\\b"] {
            let body = json!({ "session_id": bad }).to_string();
            let (session_id, _) = session_start_fields(&body);
            assert_eq!(session_id, None, "expected {bad:?} to be rejected");
        }
    }

    #[test]
    fn session_start_fields_rejects_an_overlong_id() {
        let body = json!({ "session_id": "a".repeat(MAX_SESSION_ID_LEN + 1) }).to_string();
        assert_eq!(session_start_fields(&body).0, None);
    }

    #[test]
    fn session_start_fields_rejects_a_non_string_session_id() {
        let body = json!({ "session_id": 12345 }).to_string();
        assert_eq!(session_start_fields(&body).0, None);
    }

    #[test]
    fn session_start_fields_ignores_an_overlong_source() {
        let body = json!({ "source": "x".repeat(MAX_SOURCE_LEN + 1) }).to_string();
        assert_eq!(session_start_fields(&body).1, None);
    }
}
