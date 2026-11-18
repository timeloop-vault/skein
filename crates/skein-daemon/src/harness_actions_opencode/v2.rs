//! opencode v2 → `harness_actions` rows (#678). Pure: no I/O, no DB, no
//! wiring. Two entry points that must agree row for row:
//!
//! - live: [`V2ActionTracker::observe`] folds the action frames of
//!   `GET /api/event` (parsed by `skein_harness::opencode::v2`) into rows;
//! - stored: [`rows_from_stored`] reads the `session_message` rows of the
//!   same session (attach/backfill).
//!
//! Kinds and payload keys match v1 (`super`) wherever a v1 kind exists, so
//! the frontend renders v2 rows unchanged. Identity is `source`, stable
//! across both paths: tool = call id; `turn_cost` = assistant message id;
//! `user_prompt` = inbox id (= the stored user row id); `reasoning` =
//! `<assistant message id>:r<n>`, `n` counting text and reasoning parts
//! together (the live `ordinal`, the stored content index). Timestamps are
//! the live frame's `created` and the stored item's own completion time;
//! `source` is the identity, not the timestamp.
//!
//! Child sessions: the tracker takes no session filter. Every frame carries
//! `V2Action::session_id`, which the caller has to look at (v1 records
//! tool/cost rows of any session, drops only a child's user prompt).

use std::collections::{HashMap, VecDeque};

use serde_json::{Value, json};
use skein_harness::opencode::Tokens;
use skein_harness::opencode::v2::store::{
    AssistantStep, ContentItem, MessageKind, StoredMessage, ToolItem, ToolStatus,
};
use skein_harness::opencode::v2::{V2Action, V2ActionKind};

use super::{ExtractedAction, classify_tool};
use crate::db::action_kind;

/// Bound on remembered in-flight calls; a call lives from its first frame to
/// its result, so this only guards a stream that never delivers results.
const PENDING_CAP: usize = 512;

/// Name for a stored tool item that carries none (the same word v1 uses).
const UNKNOWN_TOOL: &str = "unknown";

/// How a tool call ended; a failure may lack its message.
#[derive(Clone, Copy)]
enum Outcome<'a> {
    Done,
    Failed(Option<&'a str>),
}

/// What the result frame lacks for one call.
#[derive(Debug, Default)]
struct Pending {
    name: Option<String>,
    input: Option<Value>,
}

/// Remembers, per call id, what the result frame lacks: the tool name (only
/// on `input.started`) and the input (only on `called`). Bounded: over
/// [`PENDING_CAP`] the oldest calls are forgotten, never all of them.
#[derive(Debug, Default)]
pub struct V2ActionTracker {
    pending: HashMap<String, Pending>,
    /// Call ids in insertion order, for eviction.
    order: VecDeque<String>,
}

impl V2ActionTracker {
    /// Rows for one action frame (0 or 1). A result whose call id has no
    /// remembered name (it started before this tracker) yields no row: the
    /// next attach's backfill supplies the complete one.
    pub fn observe(&mut self, action: &V2Action) -> Vec<ExtractedAction> {
        match &action.kind {
            V2ActionKind::ToolInputStarted { call_id, name } => {
                self.entry(call_id).name = Some(name.clone());
                Vec::new()
            }
            V2ActionKind::ToolCalled { call_id, input } => {
                self.entry(call_id).input = Some(input.clone());
                Vec::new()
            }
            V2ActionKind::ToolSuccess {
                call_id,
                result,
                metadata,
            } => {
                let Some((name, input)) = self.take(call_id) else {
                    return Vec::new();
                };
                vec![tool_row(
                    call_id,
                    &name,
                    &input,
                    result.as_deref(),
                    metadata,
                    Outcome::Done,
                    action.created,
                )]
            }
            V2ActionKind::ToolFailed { call_id, error } => {
                let Some((name, input)) = self.take(call_id) else {
                    return Vec::new();
                };
                vec![tool_row(
                    call_id,
                    &name,
                    &input,
                    None,
                    &Value::Null,
                    Outcome::Failed(error.as_deref()),
                    action.created,
                )]
            }
            V2ActionKind::StepEnded {
                finish,
                tokens,
                cost,
                ..
            } => {
                let Some(msg) = &action.assistant_message_id else {
                    return Vec::new();
                };
                vec![turn_cost_row(
                    msg,
                    finish.as_deref(),
                    tokens,
                    *cost,
                    action.created,
                )]
            }
            V2ActionKind::ReasoningEnded { ordinal, text } => {
                let Some(msg) = &action.assistant_message_id else {
                    return Vec::new();
                };
                vec![reasoning_row(msg, *ordinal, text, action.created)]
            }
            V2ActionKind::InboxEnqueued { inbox_id, text } => {
                vec![user_prompt_row(inbox_id, text, action.created)]
            }
        }
    }

    fn entry(&mut self, call_id: &str) -> &mut Pending {
        if !self.pending.contains_key(call_id) {
            while self.pending.len() >= PENDING_CAP {
                let Some(oldest) = self.order.pop_front() else {
                    break;
                };
                self.pending.remove(&oldest);
            }
            self.order.push_back(call_id.to_owned());
        }
        self.pending.entry(call_id.to_owned()).or_default()
    }

    /// Name and input of a finished call; `None` when the name was never seen.
    fn take(&mut self, call_id: &str) -> Option<(String, Value)> {
        let p = self.pending.remove(call_id);
        self.order.retain(|id| id != call_id);
        let Some(name) = p.and_then(|p| p.name.map(|n| (n, p.input))) else {
            tracing::debug!(
                call_id,
                "opencode v2: result for a call never seen start; no row"
            );
            return None;
        };
        Some((name.0, name.1.unwrap_or(Value::Null)))
    }
}

/// Rows for a session's stored messages, in message order: a user row gives
/// a `user_prompt`; an assistant step gives its reasoning, its finished tool
/// calls (`completed`/`error`; others are skipped) and one `turn_cost`.
/// `idle`, `synthetic` and unknown rows give nothing.
#[must_use]
pub fn rows_from_stored(messages: &[StoredMessage]) -> Vec<ExtractedAction> {
    let mut out = Vec::new();
    for m in messages {
        match &m.kind {
            MessageKind::User { text } => out.push(user_prompt_row(&m.id, text, m.time_created)),
            MessageKind::Assistant(step) => step_rows(m, step, &mut out),
            _ => {}
        }
    }
    out
}

fn step_rows(m: &StoredMessage, step: &AssistantStep, out: &mut Vec<ExtractedAction>) {
    let step_ts = step.completed.or(step.created).unwrap_or(m.time_created);
    // Text and reasoning parts share one counter (the live `ordinal`).
    let mut ordinal = 0_u64;
    for item in &step.content {
        match item {
            ContentItem::Text(_) => ordinal += 1,
            ContentItem::Reasoning {
                text,
                created,
                completed,
            } => {
                if completed.is_none() {
                    continue;
                }
                let ts = completed.or(*created).unwrap_or(step_ts);
                out.push(reasoning_row(&m.id, ordinal, text, ts));
                ordinal += 1;
            }
            ContentItem::Tool(t) => {
                if let Some(row) = stored_tool_row(t, step_ts) {
                    out.push(row);
                }
            }
            ContentItem::Other => {}
        }
    }
    // A step still in flight has no finish/completed yet; its live
    // `session.step.ended` will supply the row.
    if step.finish.is_some() && step.completed.is_some() {
        out.push(turn_cost_row(
            &m.id,
            step.finish.as_deref(),
            &step.tokens,
            step.cost,
            step_ts,
        ));
    }
}

fn stored_tool_row(t: &ToolItem, fallback_ts: i64) -> Option<ExtractedAction> {
    let error = match t.status {
        ToolStatus::Completed => Outcome::Done,
        ToolStatus::Error => Outcome::Failed(t.error.as_deref()),
        ToolStatus::Other => return None,
    };
    let name = if t.name.is_empty() {
        UNKNOWN_TOOL
    } else {
        &t.name
    };
    // A failed live row has no metadata (the failed frame carries none), so
    // neither does the stored one.
    let metadata = if matches!(error, Outcome::Failed(_)) {
        &Value::Null
    } else {
        &t.metadata
    };
    Some(tool_row(
        &t.id,
        name,
        &t.input,
        t.result.as_deref(),
        metadata,
        error,
        t.completed.or(t.created).unwrap_or(fallback_ts),
    ))
}

fn tool_row(
    call_id: &str,
    name: &str,
    input: &Value,
    result: Option<&str>,
    metadata: &Value,
    outcome: Outcome,
    timestamp_ms: i64,
) -> ExtractedAction {
    let kind = classify_tool(name);
    let mut payload = json!({
        "tool": name,
        "input": input,
        "result": result,
        "is_error": matches!(outcome, Outcome::Failed(_)),
        "metadata": metadata,
    });
    if let Some(obj) = payload.as_object_mut() {
        if let Outcome::Failed(msg) = outcome {
            obj.insert("error".into(), json!(msg));
        }
        if kind == action_kind::PATCH {
            obj.insert("files".into(), files_touched(input, metadata));
            if let Some(pi) = patch_info(metadata) {
                obj.insert("patch_info".into(), pi);
            }
        } else if kind == action_kind::PLAN_CHANGE
            && let Some(todos) = metadata.get("todos").and_then(Value::as_array)
        {
            obj.insert(
                "plan_item".into(),
                json!({"op": "write", "count": todos.len(), "items": todos}),
            );
        }
    }
    ExtractedAction {
        kind,
        timestamp_ms,
        payload: payload.to_string(),
        source: Some(call_id.to_owned()),
    }
}

/// `metadata.files[].file`, else the input's `path` (or v1's `filePath`).
fn files_touched(input: &Value, metadata: &Value) -> Value {
    let from_meta: Vec<&str> = metadata_files(metadata)
        .filter_map(|f| f.get("file").and_then(Value::as_str))
        .collect();
    if !from_meta.is_empty() {
        return json!(from_meta);
    }
    ["path", "filePath"]
        .iter()
        .find_map(|k| input.get(k).and_then(Value::as_str))
        .map_or(json!([]), |p| json!([p]))
}

fn metadata_files(metadata: &Value) -> impl Iterator<Item = &Value> {
    metadata
        .get("files")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
}

/// v1's `patch_info` shape; omitted (as in v1) when there is no per-file
/// diff data, which is the case for `write`.
fn patch_info(metadata: &Value) -> Option<Value> {
    let files: Vec<&Value> = metadata_files(metadata).collect();
    let first = files.first()?;
    let sum = |k: &str| -> u64 {
        files
            .iter()
            .filter_map(|f| f.get(k).and_then(Value::as_u64))
            .sum()
    };
    let diff: Vec<&str> = files
        .iter()
        .filter_map(|f| f.get("patch").and_then(Value::as_str))
        .collect();
    Some(json!({
        "additions": sum("additions"),
        "deletions": sum("deletions"),
        "diff": diff.join("\n"),
        "file": first.get("file").cloned().unwrap_or(Value::Null),
    }))
}

fn turn_cost_row(
    message_id: &str,
    finish: Option<&str>,
    t: &Tokens,
    cost: f64,
    timestamp_ms: i64,
) -> ExtractedAction {
    let payload = json!({
        "reason": finish.unwrap_or(""),
        "tokens": {
            "input": t.input,
            "output": t.output,
            "reasoning": t.reasoning,
            "cache": {"read": t.cache_read, "write": t.cache_write},
        },
        "cost": cost,
    });
    ExtractedAction {
        kind: action_kind::TURN_COST,
        timestamp_ms,
        payload: payload.to_string(),
        source: Some(message_id.to_owned()),
    }
}

fn reasoning_row(message_id: &str, ordinal: u64, text: &str, timestamp_ms: i64) -> ExtractedAction {
    ExtractedAction {
        kind: action_kind::REASONING,
        timestamp_ms,
        payload: json!({"text": text}).to_string(),
        source: Some(format!("{message_id}:r{ordinal}")),
    }
}

fn user_prompt_row(inbox_id: &str, text: &str, timestamp_ms: i64) -> ExtractedAction {
    ExtractedAction {
        kind: action_kind::USER_PROMPT,
        timestamp_ms,
        payload: json!({"prompt": text}).to_string(),
        source: Some(inbox_id.to_owned()),
    }
}

mod backfill;
pub use backfill::backfill_v2_from_db;

#[cfg(test)]
mod backfill_tests;
#[cfg(test)]
mod tests;
