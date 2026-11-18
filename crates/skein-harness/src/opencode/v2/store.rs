//! opencode v2's stored messages: the `session_message` table of
//! `opencode.db` (#678).
//!
//! One row per message, `data` is JSON, `seq` is the session's event
//! sequence (with gaps). Types seen in the 2.3.2 captures: `user`,
//! `assistant` (one row per model step, tool calls inside `content`), `idle`
//! (end-of-turn marker) and `synthetic` (a subagent hand-back). The shape
//! belongs to opencode, not Skein: every field is optional, unknown keys are
//! ignored and an unknown `type` reads as [`MessageKind::Other`].

use rusqlite::{Connection, params};
use serde_json::Value;

use crate::opencode::{Result, Tokens};

#[cfg(test)]
pub(super) mod tests;

/// How a tool call ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolStatus {
    Completed,
    Error,
    /// Pending/running or a status this reader has not seen.
    Other,
}

/// One tool call inside an assistant step.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolItem {
    /// `call_…`, the id the SSE tool frames carry.
    pub id: String,
    pub name: String,
    pub status: ToolStatus,
    /// `state.input`, `Null` when absent.
    pub input: Value,
    /// The result's `text` content items joined with `\n`.
    pub result: Option<String>,
    /// `state.metadata`, `Null` when absent (e.g. `files[].patch` for edit).
    pub metadata: Value,
    /// `state.error.message` for an errored call.
    pub error: Option<String>,
    pub created: Option<i64>,
    pub ran: Option<i64>,
    pub completed: Option<i64>,
}

/// One entry of an assistant step's `content`.
#[derive(Debug, Clone, PartialEq)]
pub enum ContentItem {
    Text(String),
    Reasoning {
        text: String,
        created: Option<i64>,
        completed: Option<i64>,
    },
    Tool(ToolItem),
    Other,
}

/// An `assistant` row: one model step.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AssistantStep {
    /// `"stop"` or `"tool-calls"`.
    pub finish: Option<String>,
    pub raw_finish: Option<String>,
    pub cost: f64,
    pub tokens: Tokens,
    pub model_id: Option<String>,
    pub provider_id: Option<String>,
    pub agent: Option<String>,
    pub created: Option<i64>,
    pub first_token: Option<i64>,
    pub streamed: Option<i64>,
    pub completed: Option<i64>,
    pub content: Vec<ContentItem>,
}

/// A `synthetic` row, minimal: a subagent hand-back.
#[derive(Debug, Clone, PartialEq)]
pub struct Synthetic {
    pub text: Option<String>,
    pub description: Option<String>,
    /// `metadata.childID`.
    pub child_id: Option<String>,
}

/// The parsed `data` of one row.
#[derive(Debug, Clone, PartialEq)]
pub enum MessageKind {
    User { text: String },
    Assistant(AssistantStep),
    Idle { outcome: Option<String> },
    Synthetic(Synthetic),
    Other { kind: String },
}

/// One `session_message` row.
#[derive(Debug, Clone, PartialEq)]
pub struct StoredMessage {
    pub id: String,
    pub seq: i64,
    /// `time_created`, epoch ms.
    pub time_created: i64,
    pub kind: MessageKind,
}

fn string(v: &Value, key: &str) -> Option<String> {
    v.get(key).and_then(Value::as_str).map(str::to_owned)
}

fn ptr_string(v: &Value, ptr: &str) -> Option<String> {
    v.pointer(ptr).and_then(Value::as_str).map(str::to_owned)
}

fn num(v: &Value, ptr: &str) -> Option<i64> {
    v.pointer(ptr).and_then(Value::as_i64)
}

/// `{input, output, reasoning, cache{read, write}}`; missing parts are 0.
pub(super) fn parse_tokens(v: &Value) -> Tokens {
    let n = |ptr: &str| v.pointer(ptr).and_then(Value::as_u64).unwrap_or(0);
    Tokens {
        input: n("/input"),
        output: n("/output"),
        reasoning: n("/reasoning"),
        cache_read: n("/cache/read"),
        cache_write: n("/cache/write"),
    }
}

/// The `text` items of a tool result's `content`, joined with `\n`.
pub(super) fn result_text(content: Option<&Value>) -> Option<String> {
    let parts: Vec<&str> = content?
        .as_array()?
        .iter()
        .filter(|c| c.get("type").and_then(Value::as_str) == Some("text"))
        .filter_map(|c| c.get("text").and_then(Value::as_str))
        .collect();
    (!parts.is_empty()).then(|| parts.join("\n"))
}

fn parse_tool(item: &Value) -> ToolItem {
    let state = item.get("state").unwrap_or(&Value::Null);
    let status = match state.get("status").and_then(Value::as_str) {
        Some("completed") => ToolStatus::Completed,
        Some("error") => ToolStatus::Error,
        _ => ToolStatus::Other,
    };
    ToolItem {
        id: string(item, "id").unwrap_or_default(),
        name: string(item, "name").unwrap_or_default(),
        status,
        input: state.get("input").cloned().unwrap_or(Value::Null),
        result: result_text(state.get("content")),
        metadata: state.get("metadata").cloned().unwrap_or(Value::Null),
        error: ptr_string(state, "/error/message"),
        created: num(item, "/time/created"),
        ran: num(item, "/time/ran"),
        completed: num(item, "/time/completed"),
    }
}

fn parse_content(item: &Value) -> ContentItem {
    match item.get("type").and_then(Value::as_str) {
        Some("text") => ContentItem::Text(string(item, "text").unwrap_or_default()),
        Some("reasoning") => ContentItem::Reasoning {
            text: string(item, "text").unwrap_or_default(),
            created: num(item, "/time/created"),
            completed: num(item, "/time/completed"),
        },
        Some("tool") => ContentItem::Tool(parse_tool(item)),
        _ => ContentItem::Other,
    }
}

fn parse_assistant(data: &Value) -> AssistantStep {
    AssistantStep {
        finish: string(data, "finish"),
        raw_finish: string(data, "rawFinish"),
        cost: data.get("cost").and_then(Value::as_f64).unwrap_or(0.0),
        tokens: data.get("tokens").map(parse_tokens).unwrap_or_default(),
        model_id: ptr_string(data, "/model/id"),
        provider_id: ptr_string(data, "/model/providerID"),
        agent: string(data, "agent"),
        created: num(data, "/time/created"),
        first_token: num(data, "/time/firstToken"),
        streamed: num(data, "/time/streamed"),
        completed: num(data, "/time/completed"),
        content: data
            .get("content")
            .and_then(Value::as_array)
            .map(|a| a.iter().map(parse_content).collect())
            .unwrap_or_default(),
    }
}

/// Parses one row's `type` and `data`. Never fails: an unknown type or a
/// shape that does not fit reads as sparse fields or [`MessageKind::Other`].
#[must_use]
pub fn parse_message(kind: &str, data: &Value) -> MessageKind {
    match kind {
        "user" => MessageKind::User {
            text: string(data, "text").unwrap_or_default(),
        },
        "assistant" => MessageKind::Assistant(parse_assistant(data)),
        "idle" => MessageKind::Idle {
            outcome: string(data, "outcome"),
        },
        "synthetic" => MessageKind::Synthetic(Synthetic {
            text: string(data, "text"),
            description: string(data, "description"),
            child_id: ptr_string(data, "/metadata/childID"),
        }),
        other => MessageKind::Other {
            kind: other.to_owned(),
        },
    }
}

fn table_exists(conn: &Connection) -> Result<bool> {
    let mut stmt = conn
        .prepare("SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'session_message'")?;
    Ok(stmt.exists([])?)
}

/// The session's messages in `seq` order. A database with no
/// `session_message` table (v1 only) reads as empty, never as an error; a row
/// whose `data` is not JSON reads as [`MessageKind::Other`].
///
/// # Errors
/// A sqlite failure other than a missing table.
pub fn session_messages(conn: &Connection, session_id: &str) -> Result<Vec<StoredMessage>> {
    session_messages_after(conn, session_id, -1)
}

/// Like [`session_messages`], only rows with `seq` greater than `after_seq`
/// (pass `-1` for all; `seq` starts at 0 or above).
///
/// # Errors
/// A sqlite failure other than a missing table.
pub fn session_messages_after(
    conn: &Connection,
    session_id: &str,
    after_seq: i64,
) -> Result<Vec<StoredMessage>> {
    if !table_exists(conn)? {
        return Ok(Vec::new());
    }
    let mut stmt = conn.prepare(
        "SELECT id, type, seq, time_created, data FROM session_message \
         WHERE session_id = ?1 AND seq > ?2 ORDER BY seq",
    )?;
    let rows = stmt.query_map(params![session_id, after_seq], |r| {
        let kind: String = r.get(1)?;
        let data: String = r.get(4)?;
        let kind = match serde_json::from_str::<Value>(&data) {
            Ok(v) => parse_message(&kind, &v),
            Err(_) => MessageKind::Other { kind },
        };
        Ok(StoredMessage {
            id: r.get(0)?,
            seq: r.get(2)?,
            time_created: r.get(3)?,
            kind,
        })
    })?;
    Ok(rows.collect::<std::result::Result<_, _>>()?)
}
