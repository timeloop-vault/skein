//! The action data of opencode v2 SSE frames (#678): what the Live Context
//! rows are built from. Separate from [`super::parse_event`], which keeps the
//! phase-only view; the same frame can feed both.
//!
//! Frames are `{id, created, type, data}` with `data.sessionID` and (for
//! step/tool/reasoning frames) `data.assistantMessageID`. The tool name rides
//! ONLY on `session.tool.input.started`.

use serde_json::Value;

use super::store::{parse_tokens, result_text};
use crate::opencode::Tokens;

/// What one action frame says.
#[derive(Debug, Clone, PartialEq)]
pub enum V2ActionKind {
    StepEnded {
        finish: Option<String>,
        raw_finish: Option<String>,
        tokens: Tokens,
        cost: f64,
    },
    ToolInputStarted {
        call_id: String,
        name: String,
    },
    ToolCalled {
        call_id: String,
        input: Value,
    },
    ToolSuccess {
        call_id: String,
        result: Option<String>,
        metadata: Value,
    },
    ToolFailed {
        call_id: String,
        error: Option<String>,
    },
    ReasoningEnded {
        ordinal: u64,
        text: String,
    },
    /// A user prompt entered the inbox (`item.type == "user"` only).
    InboxEnqueued {
        inbox_id: String,
        text: String,
    },
}

/// One action frame with its common fields.
#[derive(Debug, Clone, PartialEq)]
pub struct V2Action {
    pub session_id: String,
    /// Absent on `session.inbox.enqueued`.
    pub assistant_message_id: Option<String>,
    /// The frame's `created`, epoch ms.
    pub created: i64,
    pub kind: V2ActionKind,
}

fn string(v: &Value, key: &str) -> Option<String> {
    v.get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

/// Parses one frame's JSON payload. `None` for malformed JSON, frames that
/// carry no action data and ones missing their `sessionID` or key ids.
#[must_use]
pub fn parse_action_event(payload: &str) -> Option<V2Action> {
    let frame: Value = serde_json::from_str(payload).ok()?;
    let ty = frame.get("type")?.as_str()?;
    let data = frame.get("data")?;
    let created = frame.get("created")?.as_i64()?;
    let call_id = || string(data, "id");
    let kind = match ty {
        "session.step.ended" => V2ActionKind::StepEnded {
            finish: string(data, "finish"),
            raw_finish: string(data, "rawFinish"),
            tokens: data.get("tokens").map(parse_tokens).unwrap_or_default(),
            cost: data.get("cost").and_then(Value::as_f64).unwrap_or(0.0),
        },
        "session.tool.input.started" => V2ActionKind::ToolInputStarted {
            call_id: call_id()?,
            name: string(data, "name")?,
        },
        "session.tool.called" => V2ActionKind::ToolCalled {
            call_id: call_id()?,
            input: data.get("input").cloned().unwrap_or(Value::Null),
        },
        "session.tool.success" => V2ActionKind::ToolSuccess {
            call_id: call_id()?,
            result: result_text(data.get("content")),
            metadata: data.get("metadata").cloned().unwrap_or(Value::Null),
        },
        "session.tool.failed" => V2ActionKind::ToolFailed {
            call_id: call_id()?,
            error: data
                .pointer("/error/message")
                .and_then(Value::as_str)
                .map(str::to_owned),
        },
        "session.reasoning.ended" => V2ActionKind::ReasoningEnded {
            ordinal: data.get("ordinal").and_then(Value::as_u64).unwrap_or(0),
            text: string(data, "text").unwrap_or_default(),
        },
        "session.inbox.enqueued" => {
            if data.pointer("/item/type").and_then(Value::as_str) != Some("user") {
                return None;
            }
            V2ActionKind::InboxEnqueued {
                inbox_id: string(data, "inboxID")?,
                text: data
                    .pointer("/item/payload/text")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
            }
        }
        _ => return None,
    };
    Some(V2Action {
        session_id: string(data, "sessionID")?,
        assistant_message_id: string(data, "assistantMessageID"),
        created,
        kind,
    })
}
