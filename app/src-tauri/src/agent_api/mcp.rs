//! The verbs, as MCP.
//!
//! A thin JSON-RPC layer over [`super::verbs`] — `initialize`,
//! `tools/list`, `tools/call`, `ping`, and notifications. Deliberately
//! stateless: no `Mcp-Session-Id`, no SSE, no server-initiated
//! requests. A POST carrying a request gets a single JSON object back,
//! which the Streamable HTTP transport explicitly permits, and every
//! client has to support.
//!
//! # Tool groups (#534)
//!
//! Every tool belongs to a [`ToolGroup`] the user switches on or off
//! globally in Settings. A group that is off is hidden from `tools/list`
//! and its instructions paragraph, *and* refused in [`call_tool`] (and on
//! the `/api` mirror) — hidden-and-refused, which supersedes #176's
//! list-and-refuse for the old per-feature kill switches. `skein_info`
//! is in no group and is always available.
//!
//! # The tool list is the contract
//!
//! Several things are absent from [`tool_specs`] *and*
//! refused by name in [`call_tool`]: `resolve`, because an agent that
//! can close its own comments removes the review's only gate; `approve`,
//! because one that can sign off its own work removes it a level higher;
//! and the ways to *destroy* a room outright (`remove_worktree`,
//! `delete_room`) — destroying stays the user's decision, the same way
//! Skein itself performs no git mutations (see `CLAUDE.md`). `#411`
//! narrows that third rule one level: `close_room` (archive only,
//! creator-only, only once the target is signed off) is a real tool now,
//! and `archive_room` is refused by name as its alias, pointing callers
//! at the real one rather than at "no". All these refusals are spelled
//! out rather than left to the tool list being short — a model told a
//! tool is merely missing goes looking for another way in.
//!
//! The descriptions here are the agent's documentation; they are the
//! only thing it reads before deciding what to call, so they say what
//! each verb is *for*, not merely what it does. `create_room`'s in
//! particular is load-bearing (#330): it says plainly that this opens a
//! real room and spawns a real agent process, that `path` may be any
//! checkout on the machine, and that a `prompt` starts that agent
//! working unattended in the background.
//!
//! # Why `tools/call` is async
//!
//! Most verbs are plain, synchronous functions over a
//! [`crate::db::Database`] — [`call_tool`] only needed to be async once
//! `create_room` landed, since opening a room means round-tripping to
//! the webview (`AgentApiState::request_frontend`). #356's `list_rooms`,
//! `get_room` and `list_harnesses` are async for the same reason: each
//! makes its own such round trip (`"harness_phases"`) to read a live
//! harness's activity phase, capped at a few seconds rather than
//! `create_room`'s whole minute — a read-only listing call must never
//! hang on a busy or absent webview.

mod design_specs;
mod instructions;
mod specs;

pub use specs::tool_specs;

use serde::Deserialize;
use serde_json::{Value, json};

use super::auth::Caller;
use super::groups::{ToolGroup, ToolGroups, group_of};
use super::state::AgentApiState;
use super::verbs::{self, MailContext, VerbError};

/// What we advertise. Older clients negotiate down by sending their own
/// version in `initialize`; we echo anything we know rather than
/// forcing an upgrade.
pub const PROTOCOL_VERSION: &str = "2025-11-25";

/// Versions whose wire shape this server is compatible with.
pub const SUPPORTED_VERSIONS: &[&str] = &["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"];

/// Server name, echoed in `initialize`'s `serverInfo.name`. It is not
/// the prefix a client puts on a tool call — that comes from the
/// server *key* in the client's own MCP config (`api` for the Claude
/// Code plugin's `.mcp.json`, `skein` for opencode's `opencode.json`),
/// not from this constant.
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

/// Names that mean "approve this myself" (#214). Refused for the same
/// reason as [`RESOLVE_ALIASES`], one level up: an agent that can sign
/// off its own work removes the only gate the review has. `review_status`
/// is how it *reads* the sign-off, and there is no way to write one.
const SIGNOFF_ALIASES: &[&str] = &[
    "approve",
    "sign_off",
    "signoff",
    "approve_review",
    "set_review_status",
    "set_signoff",
    "mark_approved",
];

/// Names that would destroy a room `create_room` (#330) just gained the
/// power to open, refused for the same reason as [`RESOLVE_ALIASES`] and
/// [`SIGNOFF_ALIASES`]: Skein performs no git mutations and destroys
/// nothing on its own — that stays the user's decision (`CLAUDE.md`) —
/// and an agent told these tools are simply missing would go looking for
/// another way to do it (a raw `rm -rf` on the worktree, say).
///
/// `archive_room` sits here too, but for a narrower reason since #411:
/// `close_room` is the real, guarded verb for that action now (archive
/// only, creator-only, only once signed off), so `archive_room` is
/// refused as its alias — the answer names `close_room` instead of
/// "no", which is why [`call_tool`] gives it its own message rather than
/// sharing `remove_worktree`'s/`delete_room`'s.
const DESTROY_ALIASES: &[&str] = &["archive_room", "remove_worktree", "delete_room"];

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

/// The tools a client may see with these groups on: every spec whose
/// group is enabled, plus `skein_info`, which is in no group.
pub fn tool_specs_for(groups: ToolGroups) -> Vec<Value> {
    tool_specs()
        .into_iter()
        .filter(|spec| {
            spec["name"]
                .as_str()
                .and_then(group_of)
                .is_none_or(|g| groups.allows(g))
        })
        .collect()
}

/// Handle one JSON-RPC message.
pub async fn handle(
    state: &AgentApiState,
    caller: &Caller,
    body: &str,
    mail: &MailContext,
) -> Outcome {
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
        "initialize" => Outcome::Json(Box::new(ok(
            &id,
            &initialize_result(&params, state.tool_groups()),
        ))),
        "ping" => Outcome::Json(Box::new(ok(&id, &json!({})))),
        "tools/list" => Outcome::Json(Box::new(ok(
            &id,
            &json!({ "tools": tool_specs_for(state.tool_groups()) }),
        ))),
        "tools/call" => Outcome::Json(Box::new(
            tools_call(state, caller, &id, &params, mail).await,
        )),
        other => Outcome::Json(Box::new(err(
            &id,
            -32601,
            &format!("unknown method: {other}"),
        ))),
    }
}

/// Build the `initialize` result: protocol version, capabilities and instructions.
///
/// `listChanged` stays `false`: the server is stateless (no SSE, nothing
/// to push a notification down), so a changed group setting shows up in
/// `tools/list` the next time the client lists, in practice on a harness
/// restart. The refusal in [`call_tool`] applies immediately.
fn initialize_result(params: &Value, groups: ToolGroups) -> Value {
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
            "title": "Skein agent API",
            "version": crate::build_info::VERSION,
        },
        "instructions": instructions::instructions(groups),
    })
}

async fn tools_call(
    state: &AgentApiState,
    caller: &Caller,
    id: &Value,
    params: &Value,
    mail: &MailContext,
) -> Value {
    let Some(name) = params.get("name").and_then(Value::as_str) else {
        return err(id, -32602, "tools/call needs a name");
    };
    let args = params.get("arguments").cloned().unwrap_or(json!({}));
    match call_tool(state, caller, name, &args, mail).await {
        Ok(value) if is_screenshot(name) => ok(id, &image_content(&value)),
        Ok(value) => ok(id, &tool_content(&value, false)),
        // A tool that ran and refused is *not* a protocol error: the
        // model has to see the reason, and a JSON-RPC error is
        // surfaced to the client's plumbing rather than to the model.
        Err(e) => ok(id, &tool_content(&json!({ "error": e.message() }), true)),
    }
}

/// Dispatch one tool by name. Public so the resolve prohibition can be
/// tested without a JSON-RPC envelope around it.
///
/// Async only because `create_room` is: every other branch awaits
/// nothing, and dispatch happens before any of them run.
pub async fn call_tool(
    state: &AgentApiState,
    caller: &Caller,
    name: &str,
    args: &Value,
    mail: &MailContext,
) -> Result<Value, VerbError> {
    let db = &state.db;
    // Claude Code sends the bare name; be tolerant of a client that
    // sends its own namespaced form back to us.
    let name = bare_name(name);
    if RESOLVE_ALIASES.contains(&name) {
        return Err(VerbError::Refused(
            "resolving a thread is the reviewer's decision, not yours. \
             Reply on the thread and call mark_addressed; the reviewer \
             closes it after reading."
                .into(),
        ));
    }
    if SIGNOFF_ALIASES.contains(&name) {
        return Err(VerbError::Refused(
            "signing off is the reviewer's decision, not yours. An agent \
             that could approve its own work would remove the only gate \
             this review has. Call review_status to see whether they \
             have."
                .into(),
        ));
    }
    if DESTROY_ALIASES.contains(&name) {
        let message = if name == "archive_room" {
            "archiving is close_room, with its rules: only the room that \
             opened this room with create_room may archive it, and only \
             once it is signed off on its current HEAD. Call close_room \
             instead."
        } else {
            "destroying a room is the user's decision, not yours. Skein \
             performs no git mutations and closes nothing on its own — \
             say what you'd like removed and why, and let them do it."
        };
        return Err(VerbError::Refused(message.to_owned()));
    }
    // A group that is off is hidden from `tools/list` and refused here,
    // so a client that cached the list (or guesses a name) learns why.
    if let Some(g) = group_of(name)
        && !state.tool_groups().allows(g)
    {
        return Err(group_refusal(g, name));
    }
    match name {
        "list_comments" => to_value(verbs::list_comments(db, caller, &parse(args)?)?),
        "get_comment" => to_value(verbs::get_comment(db, caller, &parse(args)?)?),
        "get_diff" => to_value(verbs::get_diff(db, caller, &parse(args)?)?),
        "reply" => to_value(verbs::reply(db, caller, &parse(args)?)?),
        "mark_addressed" => to_value(verbs::mark_addressed(db, caller, &parse(args)?)?),
        "review_status" => to_value(verbs::review_status(db, caller)?),
        "skein_info" => to_value(verbs::skein_info(state)),
        "send_message" => to_value(verbs::send_message(
            db,
            caller,
            &parse(args)?,
            mail.policy,
            mail.agent_sees_mcp,
            mail.app.as_ref(),
        )?),
        "create_room" => to_value(
            verbs::create_room(
                state,
                caller,
                &parse(args)?,
                mail,
                state.tool_groups().allows(ToolGroup::Rooms),
            )
            .await?,
        ),
        "close_room" => to_value(
            verbs::close_room(
                state,
                caller,
                &parse(args)?,
                state.tool_groups().allows(ToolGroup::Rooms),
            )
            .await?,
        ),
        "open_harness" => to_value(
            verbs::open_harness(
                state,
                caller,
                &parse(args)?,
                mail,
                state.tool_groups().allows(ToolGroup::Rooms),
            )
            .await?,
        ),
        "close_harness" => to_value(
            verbs::close_harness(
                state,
                caller,
                &parse(args)?,
                state.tool_groups().allows(ToolGroup::Rooms),
            )
            .await?,
        ),
        "list_design_harnesses" => to_value(verbs::list_design_harnesses(state, caller).await?),
        "get_design_state" => {
            to_value(verbs::get_design_state(state, caller, &parse(args)?).await?)
        }
        "open_design_entry" => to_value(
            verbs::open_design_entry(
                state,
                caller,
                &parse(args)?,
                state.tool_groups().allows(ToolGroup::Design),
            )
            .await?,
        ),
        "set_design_device" => to_value(
            verbs::set_design_device(
                state,
                caller,
                &parse(args)?,
                state.tool_groups().allows(ToolGroup::Design),
            )
            .await?,
        ),
        "show_element" => to_value(
            verbs::show_element(
                state,
                caller,
                &parse(args)?,
                state.tool_groups().allows(ToolGroup::Design),
            )
            .await?,
        ),
        "show_changes" => to_value(
            verbs::show_changes(
                state,
                caller,
                &parse(args)?,
                state.tool_groups().allows(ToolGroup::Design),
            )
            .await?,
        ),
        "get_design_screenshot" => to_value(
            verbs::get_design_screenshot(
                state,
                caller,
                &parse(args)?,
                state.tool_groups().allows(ToolGroup::Design),
            )
            .await?,
        ),
        "show_design_pane" => to_value(
            verbs::show_design_pane(
                state,
                caller,
                &parse(args)?,
                state.tool_groups().allows(ToolGroup::Design),
            )
            .await?,
        ),
        "invoke_element" => to_value(
            verbs::invoke_element(
                state,
                caller,
                &parse(args)?,
                state.tool_groups().allows(ToolGroup::Design),
            )
            .await?,
        ),
        "read_messages" => to_value(verbs::read_messages(
            db,
            caller,
            &parse(args)?,
            mail.policy,
        )?),
        // #364: unlike read_messages, never marks anything read.
        "message_history" => to_value(verbs::message_history(
            db,
            caller,
            &parse(args)?,
            mail.policy,
        )?),
        // Not scoped to `caller`'s room — see the doc comment on
        // `find_rooms_for_path` for why this verb alone answers across
        // every room.
        "find_rooms_for_path" => to_value(verbs::find_rooms_for_path(db, &parse(args)?)?),
        // #356: the same considered cross-room exception, extended to a
        // director listing and inspecting the rooms it opened.
        "list_rooms" => to_value(verbs::list_rooms(state, caller, &parse(args)?, mail).await?),
        "get_room" => to_value(verbs::get_room(state, &parse(args)?).await?),
        "list_harnesses" => to_value(verbs::list_harnesses(state, &parse(args)?).await?),
        other => Err(VerbError::NotFound(format!("no such tool: {other}"))),
    }
}

/// The refusal for a tool whose group is off in Settings (#534). Shared
/// with the `/api` mirror so both surfaces say the same thing.
pub(crate) fn group_refusal(group: ToolGroup, what: &str) -> VerbError {
    tracing::info!(
        group = group.key(),
        tool = what,
        "agent api: refused, group is off"
    );
    VerbError::Refused(format!(
        "the {} tools ({what}) are turned off in Skein's Settings for every \
         harness. Ask the user if you need them.",
        group.label()
    ))
}

/// Whether a tool call mutates the review — the HTTP layer uses it to
/// decide whether the pane needs telling.
pub fn is_write(name: &str) -> bool {
    matches!(bare_name(name), "reply" | "mark_addressed")
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

/// The tool name without any client namespace prefix (`mcp__x__name`).
fn bare_name(name: &str) -> &str {
    name.rsplit("__").next().unwrap_or(name)
}

fn is_screenshot(name: &str) -> bool {
    bare_name(name) == "get_design_screenshot"
}

/// A result carrying an image (#552): the metadata as pretty JSON text,
/// then the base64 `png` field lifted out into an MCP image block.
/// Without a `png` field it is the ordinary text-only result.
pub(crate) fn image_content(value: &Value) -> Value {
    let mut meta = value.clone();
    let png = meta
        .as_object_mut()
        .and_then(|o| o.remove("png"))
        .and_then(|p| p.as_str().map(str::to_owned));
    let mut result = tool_content(&meta, false);
    if let (Some(png), Some(content)) = (png, result["content"].as_array_mut()) {
        content.push(json!({ "type": "image", "data": png, "mimeType": "image/png" }));
    }
    result
}

fn ok(id: &Value, result: &Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

fn err(id: &Value, code: i32, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}
