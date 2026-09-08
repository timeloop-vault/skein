//! The verbs, as MCP.
//!
//! A thin JSON-RPC layer over [`super::verbs`] — `initialize`,
//! `tools/list`, `tools/call`, `ping`, and notifications. Deliberately
//! stateless: no `Mcp-Session-Id`, no SSE, no server-initiated
//! requests. A POST carrying a request gets a single JSON object back,
//! which the Streamable HTTP transport explicitly permits, and every
//! client has to support.
//!
//! # The tool list is the contract
//!
//! Five tools, and no sixth. `resolve` is absent from [`tool_specs`]
//! and refused by name in [`call_tool`] — an agent that can close its
//! own comments removes the review's only gate, so the refusal is
//! spelled out rather than left to the tool list being short.
//!
//! The descriptions here are the agent's documentation; they are the
//! only thing it reads before deciding what to call, so they say what
//! each verb is *for*, not merely what it does.

use serde::Deserialize;
use serde_json::{Value, json};

use super::auth::Caller;
use super::verbs::{self, VerbError};
use crate::db::Database;

/// What we advertise. Older clients negotiate down by sending their own
/// version in `initialize`; we echo anything we know rather than
/// forcing an upgrade.
pub const PROTOCOL_VERSION: &str = "2025-06-18";

/// Versions whose wire shape this server is compatible with.
pub const SUPPORTED_VERSIONS: &[&str] = &["2025-06-18", "2025-03-26", "2024-11-05"];

/// Server name, and the prefix a Claude Code tool call carries
/// (`mcp__skein__list_comments`).
pub const SERVER_NAME: &str = "skein";

/// Names that mean "close this thread". Refused explicitly so the
/// answer is a reason rather than "unknown tool" (#176) — an agent that
/// is told a tool is missing will look for another way to do it.
const RESOLVE_ALIASES: &[&str] = &[
    "resolve",
    "resolve_thread",
    "resolve_comment",
    "close_thread",
    "close_comment",
    "mark_resolved",
];

/// What the HTTP layer should do with a parsed message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// A JSON-RPC response body, HTTP 200.
    Json(Box<Value>),
    /// A notification or response we accepted — HTTP 202, no body.
    Accepted,
    /// Malformed beyond having an id to answer to — HTTP 400.
    BadRequest(String),
}

/// The MCP-level description a client sees at `initialize`.
fn instructions() -> &'static str {
    "Skein's review surface for the room this token belongs to. \
     Use list_comments to see the reviewer's outstanding comments, \
     get_comment to read one together with the code it is about, and \
     get_diff to read the change under review. Answer with reply, and \
     use mark_addressed once you have made the change (give the commit \
     sha when you have one). You cannot resolve threads: the reviewer \
     closes them after reading your reply."
}

/// Handle one JSON-RPC message.
pub fn handle(db: &Database, caller: &Caller, body: &str) -> Outcome {
    let parsed: Value = match serde_json::from_str(body) {
        Ok(v) => v,
        Err(e) => return Outcome::BadRequest(format!("not JSON: {e}")),
    };
    // Batches were removed in 2025-06-18 and we never needed them.
    if parsed.is_array() {
        return Outcome::BadRequest("JSON-RPC batches are not supported".into());
    }
    let Some(method) = parsed.get("method").and_then(Value::as_str) else {
        // A response to a request we never made. Nothing to do with it,
        // but it is a legal thing for a client to POST.
        return Outcome::Accepted;
    };
    let id = parsed.get("id").cloned();
    let params = parsed.get("params").cloned().unwrap_or(Value::Null);

    // No id = a notification: acknowledge, answer nothing.
    let Some(id) = id.filter(|v| !v.is_null()) else {
        return Outcome::Accepted;
    };

    match method {
        "initialize" => Outcome::Json(Box::new(ok(&id, &initialize_result(&params)))),
        "ping" => Outcome::Json(Box::new(ok(&id, &json!({})))),
        "tools/list" => Outcome::Json(Box::new(ok(&id, &json!({ "tools": tool_specs() })))),
        "tools/call" => Outcome::Json(Box::new(tools_call(db, caller, &id, &params))),
        other => Outcome::Json(Box::new(err(
            &id,
            -32601,
            &format!("unknown method: {other}"),
        ))),
    }
}

fn initialize_result(params: &Value) -> Value {
    // Echo the client's version when we know it; otherwise state ours
    // and let the client decide whether it can live with that.
    let asked = params.get("protocolVersion").and_then(Value::as_str);
    let version = asked
        .filter(|v| SUPPORTED_VERSIONS.contains(v))
        .unwrap_or(PROTOCOL_VERSION);
    json!({
        "protocolVersion": version,
        "capabilities": { "tools": { "listChanged": false } },
        "serverInfo": {
            "name": SERVER_NAME,
            "title": "Skein review",
            "version": env!("CARGO_PKG_VERSION"),
        },
        "instructions": instructions(),
    })
}

fn tools_call(db: &Database, caller: &Caller, id: &Value, params: &Value) -> Value {
    let Some(name) = params.get("name").and_then(Value::as_str) else {
        return err(id, -32602, "tools/call needs a name");
    };
    let args = params.get("arguments").cloned().unwrap_or(json!({}));
    match call_tool(db, caller, name, &args) {
        Ok(value) => ok(id, &tool_content(&value, false)),
        // A tool that ran and refused is *not* a protocol error: the
        // model has to see the reason, and a JSON-RPC error is
        // surfaced to the client's plumbing rather than to the model.
        Err(e) => ok(id, &tool_content(&json!({ "error": e.message() }), true)),
    }
}

/// Dispatch one tool by name. Public so the resolve prohibition can be
/// tested without a JSON-RPC envelope around it.
pub fn call_tool(
    db: &Database,
    caller: &Caller,
    name: &str,
    args: &Value,
) -> Result<Value, VerbError> {
    // Claude Code sends the bare name; be tolerant of a client that
    // sends its own namespaced form back to us.
    let name = name.rsplit("__").next().unwrap_or(name);
    if RESOLVE_ALIASES.contains(&name) {
        return Err(VerbError::Refused(
            "resolving a thread is the reviewer's decision, not yours. \
             Reply on the thread and call mark_addressed; the reviewer \
             closes it after reading."
                .into(),
        ));
    }
    match name {
        "list_comments" => to_value(verbs::list_comments(db, caller, &parse(args)?)?),
        "get_comment" => to_value(verbs::get_comment(db, caller, &parse(args)?)?),
        "get_diff" => to_value(verbs::get_diff(db, caller, &parse(args)?)?),
        "reply" => to_value(verbs::reply(db, caller, &parse(args)?)?),
        "mark_addressed" => to_value(verbs::mark_addressed(db, caller, &parse(args)?)?),
        other => Err(VerbError::NotFound(format!("no such tool: {other}"))),
    }
}

/// Whether a tool call mutates the review — the HTTP layer uses it to
/// decide whether the pane needs telling.
pub fn is_write(name: &str) -> bool {
    matches!(
        name.rsplit("__").next().unwrap_or(name),
        "reply" | "mark_addressed"
    )
}

fn parse<T: for<'de> Deserialize<'de>>(args: &Value) -> Result<T, VerbError> {
    serde_json::from_value(args.clone())
        .map_err(|e| VerbError::Refused(format!("bad arguments: {e}")))
}

fn to_value<T: serde::Serialize>(v: T) -> Result<Value, VerbError> {
    serde_json::to_value(v).map_err(|e| VerbError::Internal(e.to_string()))
}

/// MCP tool results are a content array. We send pretty JSON as text:
/// every client renders it, and the model reads it without a schema.
fn tool_content(value: &Value, is_error: bool) -> Value {
    let text = serde_json::to_string_pretty(value).unwrap_or_else(|e| e.to_string());
    json!({
        "content": [{ "type": "text", "text": text }],
        "isError": is_error,
    })
}

fn ok(id: &Value, result: &Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

fn err(id: &Value, code: i32, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

/// The five tools, in the order an agent would use them.
pub fn tool_specs() -> Vec<Value> {
    vec![
        json!({
            "name": "list_comments",
            "title": "List review comments",
            "description":
                "The reviewer's comments on this room's work. Defaults to the \
                 unresolved ones — the list of what still needs doing. Each \
                 thread gives its file and current line range, whether its \
                 anchor has gone outdated, whether you have already marked it \
                 addressed, and the full conversation on it.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "status": {
                        "type": "string",
                        "enum": ["unresolved", "all"],
                        "description": "Default unresolved.",
                    },
                    "file": {
                        "type": "string",
                        "description": "Worktree-relative path to filter by.",
                    },
                },
                "additionalProperties": false,
            },
            "annotations": { "readOnlyHint": true },
        }),
        json!({
            "name": "get_comment",
            "title": "Read one comment with its code",
            "description":
                "One thread together with the code it is about: the lines it was \
                 written against, those lines as they stand today, and the diff \
                 hunk it falls in. Read this before changing anything — a \
                 comment without its code is not actionable, and an outdated \
                 thread only makes sense against its original lines.",
            "inputSchema": {
                "type": "object",
                "properties": { "thread_id": { "type": "string" } },
                "required": ["thread_id"],
                "additionalProperties": false,
            },
            "annotations": { "readOnlyHint": true },
        }),
        json!({
            "name": "get_diff",
            "title": "Read the change under review",
            "description":
                "The review's diff as unified patch text. Scope 'branch' \
                 (default) is everything this branch does against its base, \
                 committed and uncommitted; 'pending' is only what is not yet \
                 reviewed; 'commit' needs a commit_sha. Pass a file to keep the \
                 answer small — a whole-branch diff is truncated at 256 KB.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "file": { "type": "string" },
                    "scope": {
                        "type": "string",
                        "enum": ["branch", "pending", "commit"],
                    },
                    "commit_sha": { "type": "string" },
                },
                "additionalProperties": false,
            },
            "annotations": { "readOnlyHint": true },
        }),
        json!({
            "name": "reply",
            "title": "Reply to a review comment",
            "description":
                "Post a reply on a thread, attributed to you. Use it to say what \
                 you changed, to disagree with a reason, or to ask the reviewer \
                 something. This does not close the thread.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "thread_id": { "type": "string" },
                    "body": { "type": "string" },
                },
                "required": ["thread_id", "body"],
                "additionalProperties": false,
            },
        }),
        json!({
            "name": "mark_addressed",
            "title": "Mark a comment addressed",
            "description":
                "Record that you have handled a comment, with the commit that did \
                 it when there is one. This is a claim, not a resolution: the \
                 reviewer still reads it and decides whether the thread closes. \
                 A later reply from the reviewer clears the mark.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "thread_id": { "type": "string" },
                    "commit_sha": {
                        "type": "string",
                        "description": "The commit that addressed it, if committed.",
                    },
                    "note": { "type": "string" },
                },
                "required": ["thread_id"],
                "additionalProperties": false,
            },
        }),
    ]
}
