//! Hermes' `state.db` message rows for ONE session, turned into the
//! action rows Skein's Live Context feed already renders (#625), plus
//! the session's model and cost.
//!
//! Pure and read-only: the caller owns the connection (see
//! [`super::open`] for the flags) and the cursor; nothing here writes.
//! Shapes were recorded on Hermes v0.21.7+184 (fixtures under
//! `src/hermes/fixtures/statedb-*`).
//!
//! # What a message row looks like
//!
//! - `user` — the typed prompt. Hermes also writes synthetic user rows
//!   (an async delegation's result carries
//!   `display_kind = 'async_delegation_complete'`; a compaction summary
//!   has `_compressed_summary = 1`); neither is a human prompt.
//! - `assistant` with `finish_reason = 'tool_calls'` — empty content and
//!   `tool_calls`, a JSON array of
//!   `{id, call_id, function: {name, arguments}}` where `arguments` is
//!   itself a JSON STRING.
//! - `tool` — one row per call, `tool_call_id` pointing back, `content`
//!   usually a JSON object string.
//! - `assistant` without tool calls — the model's text. The feed has no
//!   row kind for it, so nothing is emitted.
//!
//! `messages.timestamp` is epoch SECONDS (REAL), unlike the ms the feed
//! uses; [`secs_to_ms`] converts.
//!
//! # Cursor and pairing
//!
//! The cursor is `messages.id` (AUTOINCREMENT, monotonic across all
//! sessions, so a per-session cursor is simply the last id consumed).
//! A tool call and its result are ONE feed row, emitted when the result
//! has landed. A call at the tail of the session whose result is missing
//! is HELD: the cursor stays in front of its assistant row and the next
//! call re-reads it, so nothing is ever emitted twice. A call whose
//! result is missing but which has later user/assistant rows after it
//! (an interrupted turn: Ctrl+C writes no result) is emitted with
//! `result: null` instead, or it would block the feed for good.
//!
//! # Skipped rows
//!
//! `active = 0` rows (rewound) are skipped. `compacted = 1` rows are
//! NOT: they were real actions the agent took, and compaction only
//! decides what the model still sees.

use rusqlite::{Connection, OptionalExtension, params};
use serde_json::{Map, Value, json};

use super::Result;

/// One feed row. `message_id` is the cursor key of the message the row
/// was built from (the assistant row for a tool call).
#[derive(Debug, Clone, PartialEq)]
pub struct HermesActionRow {
    pub message_id: i64,
    /// Epoch ms.
    pub timestamp_ms: i64,
    /// A `harness_actions.kind` the frontend renders: `user_prompt`,
    /// `tool_call` or `patch`.
    pub kind: String,
    pub payload: Value,
}

/// A session's model and spend.
#[derive(Debug, Clone, PartialEq)]
pub struct HermesCost {
    pub model: Option<String>,
    pub estimated_cost_usd: Option<f64>,
    pub actual_cost_usd: Option<f64>,
    /// Hermes' own label, e.g. `estimated`.
    pub cost_status: Option<String>,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_read_tokens: i64,
    pub cache_write_tokens: i64,
    pub reasoning_tokens: i64,
    pub api_call_count: i64,
    /// Spend on side tasks (`approval`, `title_generation`, …) that
    /// Hermes keeps out of the session's own totals.
    pub auxiliary_cost_usd: f64,
}

/// Seconds (REAL) to epoch ms.
#[allow(clippy::cast_possible_truncation)]
pub fn secs_to_ms(secs: f64) -> i64 {
    (secs * 1000.0).round() as i64
}

struct Msg {
    id: i64,
    role: String,
    content: Option<String>,
    tool_call_id: Option<String>,
    tool_calls: Option<String>,
    timestamp_ms: i64,
    display_kind: Option<String>,
    compressed_summary: bool,
}

fn load(conn: &Connection, session_id: &str, after: i64) -> Result<Vec<Msg>> {
    let mut stmt = conn.prepare(
        "SELECT id, role, content, tool_call_id, tool_calls, timestamp, display_kind, \
         COALESCE(_compressed_summary, 0) FROM messages \
         WHERE session_id = ?1 AND id > ?2 AND active = 1 ORDER BY id",
    )?;
    let rows = stmt.query_map(params![session_id, after], |r| {
        Ok(Msg {
            id: r.get(0)?,
            role: r.get(1)?,
            content: r.get(2)?,
            tool_call_id: r.get(3)?,
            tool_calls: r.get(4)?,
            timestamp_ms: secs_to_ms(r.get(5)?),
            display_kind: r.get(6)?,
            compressed_summary: r.get::<_, i64>(7)? != 0,
        })
    })?;
    Ok(rows.collect::<std::result::Result<_, _>>()?)
}

/// Feed rows for messages strictly after `after`, in order, and the new
/// cursor (pass it back as `after`). See the module docs for what holds
/// the cursor back.
pub fn actions_after(
    conn: &Connection,
    session_id: &str,
    after: i64,
) -> Result<(Vec<HermesActionRow>, i64)> {
    let msgs = load(conn, session_id, after)?;
    let mut out = Vec::new();
    let mut cursor = after;
    for (i, m) in msgs.iter().enumerate() {
        match m.role.as_str() {
            "user" => {
                if let Some(row) = user_prompt(m) {
                    out.push(row);
                }
            }
            "assistant" => {
                let calls = parse_calls(m.tool_calls.as_deref());
                let later = &msgs[i + 1..];
                let at_tail = later.iter().all(|l| l.role == "tool");
                let all_landed = calls.iter().all(|c| {
                    later
                        .iter()
                        .any(|l| l.tool_call_id.as_deref() == Some(&c.id))
                });
                if !calls.is_empty() && !all_landed && at_tail {
                    break;
                }
                for c in &calls {
                    let result = later
                        .iter()
                        .find(|l| l.role == "tool" && l.tool_call_id.as_deref() == Some(&c.id));
                    out.push(call_row(m, c, result));
                }
            }
            // Tool rows are consumed by the call that owns them.
            _ => {}
        }
        cursor = m.id;
    }
    Ok((out, cursor))
}

fn user_prompt(m: &Msg) -> Option<HermesActionRow> {
    let text = m.content.as_deref().filter(|t| !t.trim().is_empty())?;
    if m.compressed_summary || m.display_kind.as_deref().is_some_and(|k| !k.is_empty()) {
        return None;
    }
    Some(HermesActionRow {
        message_id: m.id,
        timestamp_ms: m.timestamp_ms,
        kind: "user_prompt".into(),
        payload: json!({ "prompt": text }),
    })
}

struct Call {
    id: String,
    name: String,
    input: Value,
}

fn parse_calls(raw: Option<&str>) -> Vec<Call> {
    let Some(Value::Array(items)) = raw.and_then(|r| serde_json::from_str(r).ok()) else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|it| {
            let f = it.get("function")?;
            let name = f.get("name")?.as_str()?.to_owned();
            let id = it
                .get("id")
                .or_else(|| it.get("call_id"))?
                .as_str()?
                .to_owned();
            let args = f.get("arguments").and_then(Value::as_str).unwrap_or("");
            let input = match serde_json::from_str::<Value>(args) {
                Ok(v @ Value::Object(_)) => v,
                _ if args.is_empty() => json!({}),
                _ => json!({ "raw": args }),
            };
            Some(Call { id, name, input })
        })
        .collect()
}

/// Hermes' tool names to the lower-case names the feed rows key on.
fn feed_tool(name: &str) -> &str {
    match name {
        "terminal" => "bash",
        "read_file" => "read",
        "search_files" => "grep",
        "write_file" => "write",
        "patch" => "edit",
        other => other,
    }
}

/// Why a tool result is a failure, if it is one. A denied approval
/// carries a non-null `error` (and `exit_code: -1`); a command that ran
/// and failed has a non-zero `exit_code` with `error: null`.
fn failure(result: &Value) -> Option<String> {
    let obj = result.as_object()?;
    if let Some(e) = obj
        .get("error")
        .and_then(Value::as_str)
        .filter(|e| !e.is_empty())
    {
        return Some(e.to_owned());
    }
    if obj.get("success").and_then(Value::as_bool) == Some(false) {
        return Some("failed".into());
    }
    if let Some(s) = obj.get("status").and_then(Value::as_str)
        && matches!(s, "blocked" | "error")
    {
        return Some(s.to_owned());
    }
    match obj.get("exit_code").and_then(Value::as_i64) {
        Some(c) if c != 0 => Some(format!("exit code {c}")),
        _ => None,
    }
}

fn call_row(assistant: &Msg, call: &Call, result_msg: Option<&Msg>) -> HermesActionRow {
    let tool = feed_tool(&call.name);
    let parsed = result_msg.map(|r| {
        let text = r.content.as_deref().unwrap_or("");
        serde_json::from_str::<Value>(text).unwrap_or_else(|_| Value::String(text.to_owned()))
    });
    let err = parsed.as_ref().and_then(failure);

    let mut input = call.input.clone();
    // The feed's read row looks for `file_path`; Hermes says `path`.
    if tool == "read"
        && let Some(o) = input.as_object_mut()
        && let Some(p) = o.get("path").cloned()
    {
        o.entry("file_path").or_insert(p);
    }
    // The bash row reads `{stdout}`.
    let result = match (tool, &parsed) {
        ("bash", Some(Value::Object(o))) => {
            let mut r = Map::new();
            r.insert(
                "stdout".into(),
                o.get("output")
                    .cloned()
                    .unwrap_or(Value::String(String::new())),
            );
            if let Some(c) = o.get("exit_code") {
                r.insert("exit_code".into(), c.clone());
            }
            Value::Object(r)
        }
        (_, Some(v)) => v.clone(),
        (_, None) => Value::Null,
    };

    let started = assistant.timestamp_ms;
    let mut payload = json!({
        "tool": tool,
        "input": input,
        "result": result,
        "is_error": err.is_some(),
        "started_at_ms": started,
    });
    let obj = payload.as_object_mut().expect("object literal");
    if let Some(r) = result_msg {
        let ended = r.timestamp_ms;
        obj.insert("ended_at_ms".into(), json!(ended));
        obj.insert("duration_ms".into(), json!((ended - started).max(0)));
    }
    if let Some(e) = err {
        obj.insert("error".into(), Value::String(e));
    }

    let kind = if matches!(tool, "write" | "edit") {
        add_patch_fields(obj, &call.input, parsed.as_ref());
        "patch"
    } else {
        "tool_call"
    };
    HermesActionRow {
        message_id: assistant.id,
        timestamp_ms: started,
        kind: kind.into(),
        payload,
    }
}

/// `files` (the file comes from the tool ARGUMENTS) and, when the
/// result carries a unified diff, `patch_info` with it.
fn add_patch_fields(obj: &mut Map<String, Value>, input: &Value, result: Option<&Value>) {
    let file = input
        .get("path")
        .and_then(Value::as_str)
        .filter(|p| !p.is_empty())
        .map(|p| p.strip_prefix("./").unwrap_or(p).to_owned());
    let files: Vec<Value> = file.iter().map(|f| json!(f)).collect();
    obj.insert("files".into(), Value::Array(files));
    let diff = result
        .and_then(|r| r.get("diff"))
        .and_then(Value::as_str)
        .filter(|d| !d.is_empty());
    if let Some(diff) = diff {
        // No +/- counts: Hermes joins the last hunk line of a file with
        // no trailing newline onto one line (`-hi+bye`, recorded), so
        // counting lines would show wrong numbers. The row omits them
        // and the feed omits the delta.
        obj.insert("patch_info".into(), json!({ "diff": diff, "file": file }));
    }
}

/// The session's model and cost, or `None` for an unknown session.
///
/// Source: the `sessions` row. Measured against `session_model_usage`:
/// the row equals that table's `task = ''` entry (the agent loop) and
/// EXCLUDES the side tasks (`approval`, `title_generation`), which only
/// the usage table records — they are summed into `auxiliary_cost_usd`
/// so the caller can show either number. When `sessions` has no
/// estimate at all (the row is written after the first API call) the
/// main-task usage rows stand in.
pub fn session_cost(conn: &Connection, session_id: &str) -> Result<Option<HermesCost>> {
    let main = conn
        .query_row(
            "SELECT model, estimated_cost_usd, actual_cost_usd, cost_status, \
             COALESCE(input_tokens,0), COALESCE(output_tokens,0), \
             COALESCE(cache_read_tokens,0), COALESCE(cache_write_tokens,0), \
             COALESCE(reasoning_tokens,0), COALESCE(api_call_count,0) \
             FROM sessions WHERE id = ?1",
            params![session_id],
            |r| {
                Ok(HermesCost {
                    model: r.get(0)?,
                    estimated_cost_usd: r.get(1)?,
                    actual_cost_usd: r.get(2)?,
                    cost_status: r.get(3)?,
                    input_tokens: r.get(4)?,
                    output_tokens: r.get(5)?,
                    cache_read_tokens: r.get(6)?,
                    cache_write_tokens: r.get(7)?,
                    reasoning_tokens: r.get(8)?,
                    api_call_count: r.get(9)?,
                    auxiliary_cost_usd: 0.0,
                })
            },
        )
        .optional()?;
    let Some(mut cost) = main else {
        return Ok(None);
    };
    let (aux, task_est, task_act, task_tok): (f64, f64, f64, i64) = conn.query_row(
        "SELECT \
         COALESCE(SUM(CASE WHEN task <> '' THEN MAX(estimated_cost_usd, actual_cost_usd) END), 0), \
         COALESCE(SUM(CASE WHEN task = '' THEN estimated_cost_usd END), 0), \
         COALESCE(SUM(CASE WHEN task = '' THEN actual_cost_usd END), 0), \
         COALESCE(SUM(CASE WHEN task = '' THEN input_tokens + output_tokens END), 0) \
         FROM session_model_usage WHERE session_id = ?1",
        params![session_id],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
    )?;
    cost.auxiliary_cost_usd = aux;
    if cost.estimated_cost_usd.is_none() && cost.actual_cost_usd.is_none() && task_tok > 0 {
        cost.estimated_cost_usd = Some(task_est);
        cost.actual_cost_usd = (task_act > 0.0).then_some(task_act);
    }
    Ok(Some(cost))
}

#[cfg(test)]
mod tests;
