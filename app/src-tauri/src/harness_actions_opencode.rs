//! opencode → `harness_actions` extraction. Issue #80 part C.
//!
//! Sibling of `harness_actions_claude.rs`. opencode's data lives in
//! two places: the SSE `/event` stream (live) and the `SQLite` DB at
//! `~/.local/share/opencode/opencode.db` (backfill). This module
//! extracts actions from both:
//!
//! - **SSE live path**: `extract_from_sse` takes a parsed SSE payload
//!   and returns actions for `message.part.updated` events whose tool
//!   part is in a terminal state (`completed` / `error`).
//! - **`SQLite` backfill path**: `backfill_from_db` reads the `part`
//!   table for a session and walks it chronologically.
//!
//! ## Action kinds emitted
//!
//! - `tool_call` — every tool part whose name isn't claimed below.
//! - `plan_change` — `todowrite` tool parts.
//! - `patch` — `edit` / `write` / `multiedit` tool parts, plus
//!   dedicated `patch` part-type rows (file-list snapshots).
//! - `turn_cost` — `step-finish` parts (carry tokens + cost).
//! - `user_prompt` — the human's prompt: live, from a user-role
//!   `message.updated` plus its non-synthetic text part (see
//!   `UserPromptTracker`); backfill, from the same text parts in the DB.
//! - `ai_title` — `session.updated` SSE events with a title change.
//!
//! opencode-specific kinds not present in Claude today:
//! - `compaction` — context-window compaction events.
//! - `reasoning` — the model's COMPLETED reasoning blocks (text + opaque
//!   blob); in-progress updates are skipped.

use std::collections::{HashMap, HashSet};

use serde_json::{Value, json};

use crate::db::action_kind;

#[derive(Debug, Clone)]
pub struct ExtractedAction {
    pub kind: &'static str,
    pub timestamp_ms: i64,
    pub payload: String,
    pub source: Option<String>,
}

/// Extract actions from a single SSE `data:` payload. Returns 0 or
/// more actions. Only emits for terminal tool-part states
/// (`completed` / `error`) so the same `callID` doesn't produce
/// duplicates as it transitions through `pending → running →
/// completed`.
pub fn extract_from_sse(payload: &Value) -> Vec<ExtractedAction> {
    let mut out = Vec::new();
    let Some(ty) = payload.get("type").and_then(Value::as_str) else {
        return out;
    };
    let props = payload.get("properties");
    match ty {
        "message.part.updated" => {
            if let Some(part) = props.and_then(|p| p.get("part")) {
                extract_from_part(part, &mut out);
            }
        }
        "session.updated" => {
            if let Some(title) = props.and_then(|p| p.get("title")).and_then(Value::as_str) {
                let ts = props
                    .and_then(|p| p.get("time"))
                    .and_then(|t| t.get("updated"))
                    .and_then(Value::as_i64)
                    .unwrap_or(0);
                out.push(ExtractedAction {
                    kind: action_kind::AI_TITLE,
                    timestamp_ms: ts,
                    payload: json!({"ai_title": title}).to_string(),
                    source: None,
                });
            }
        }
        _ => {}
    }
    out
}

/// Extract actions from a single opencode `part.data` JSON (the
/// shape stored in the `part` `SQLite` table). Used for both SSE
/// live-path and `SQLite` backfill.
fn extract_from_part(part: &Value, out: &mut Vec<ExtractedAction>) {
    let Some(part_type) = part.get("type").and_then(Value::as_str) else {
        return;
    };
    match part_type {
        "tool" => extract_tool_part(part, out),
        "step-finish" => extract_step_finish(part, out),
        "patch" => extract_patch_part(part, out),
        "compaction" => extract_compaction(part, out),
        "reasoning" => extract_reasoning(part, out),
        _ => {}
    }
}

fn extract_tool_part(part: &Value, out: &mut Vec<ExtractedAction>) {
    let Some(state) = part.get("state") else {
        return;
    };
    let status = state.get("status").and_then(Value::as_str).unwrap_or("");
    if status != "completed" && status != "error" {
        return;
    }
    let tool_name = part
        .get("tool")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let kind = classify_tool(tool_name);
    let call_id = part
        .get("callID")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let input = state.get("input").cloned().unwrap_or(Value::Null);
    let output = state.get("output").cloned().unwrap_or(Value::Null);
    let title = state.get("title").and_then(Value::as_str);
    let is_error = status == "error";
    let error_msg = state.get("error").cloned().unwrap_or(Value::Null);
    let interrupted = state
        .get("metadata")
        .and_then(|m| m.get("interrupted"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let started_at = state
        .get("time")
        .and_then(|t| t.get("start"))
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let ended_at = state
        .get("time")
        .and_then(|t| t.get("end"))
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let duration_ms = ended_at.saturating_sub(started_at);

    let mut payload = json!({
        "tool": tool_name,
        "input": input,
        "result": output,
        "is_error": is_error,
        "interrupted": interrupted,
        "started_at_ms": started_at,
        "ended_at_ms": ended_at,
        "duration_ms": duration_ms,
    });
    if let Some(t) = title {
        payload
            .as_object_mut()
            .unwrap()
            .insert("title".into(), json!(t));
    }
    if is_error {
        payload
            .as_object_mut()
            .unwrap()
            .insert("error".into(), error_msg);
    }

    if kind == action_kind::PATCH {
        if let Some(obj) = payload.as_object_mut() {
            obj.insert("files".into(), files_touched(tool_name, &input));
            if let Some(pi) = extract_patch_info(state) {
                obj.insert("patch_info".into(), pi);
            }
        }
    } else if kind == action_kind::PLAN_CHANGE
        && let Some(obj) = payload.as_object_mut()
        && let Some(pi) = extract_plan_item(state)
    {
        obj.insert("plan_item".into(), pi);
    }

    out.push(ExtractedAction {
        kind,
        timestamp_ms: started_at,
        payload: payload.to_string(),
        source: call_id,
    });
}

fn classify_tool(name: &str) -> &'static str {
    match name {
        "edit" | "write" | "multiedit" => action_kind::PATCH,
        "todowrite" => action_kind::PLAN_CHANGE,
        _ => action_kind::TOOL_CALL,
    }
}

fn files_touched(name: &str, input: &Value) -> Value {
    match name {
        "edit" | "write" | "multiedit" => input
            .get("filePath")
            .and_then(Value::as_str)
            .map_or(json!([]), |p| json!([p])),
        _ => json!([]),
    }
}

fn extract_patch_info(state: &Value) -> Option<Value> {
    let metadata = state.get("metadata")?;
    let filediff = metadata.get("filediff")?;
    Some(json!({
        "additions": filediff.get("additions").cloned().unwrap_or(Value::Null),
        "deletions": filediff.get("deletions").cloned().unwrap_or(Value::Null),
        "diff": filediff.get("patch").cloned().unwrap_or(Value::Null),
        "file": filediff.get("file").cloned().unwrap_or(Value::Null),
    }))
}

fn extract_plan_item(state: &Value) -> Option<Value> {
    let metadata = state.get("metadata")?;
    let todos = metadata.get("todos").and_then(Value::as_array)?;
    Some(json!({
        "op": "write",
        "count": todos.len(),
        "items": todos,
    }))
}

fn extract_step_finish(part: &Value, out: &mut Vec<ExtractedAction>) {
    let tokens = part.get("tokens").cloned().unwrap_or(Value::Null);
    let cost = part.get("cost").cloned().unwrap_or(Value::Null);
    let reason = part.get("reason").and_then(Value::as_str).unwrap_or("");
    let snapshot = part.get("snapshot").cloned().unwrap_or(Value::Null);
    let payload = json!({
        "reason": reason,
        "tokens": tokens,
        "cost": cost,
        "snapshot": snapshot,
    });
    out.push(ExtractedAction {
        kind: action_kind::TURN_COST,
        timestamp_ms: 0,
        payload: payload.to_string(),
        source: None,
    });
}

fn extract_patch_part(part: &Value, out: &mut Vec<ExtractedAction>) {
    let files = part.get("files").cloned().unwrap_or(json!([]));
    let hash = part.get("hash").cloned().unwrap_or(Value::Null);
    let payload = json!({
        "files": files,
        "hash": hash,
    });
    out.push(ExtractedAction {
        kind: action_kind::PATCH,
        timestamp_ms: 0,
        payload: payload.to_string(),
        source: None,
    });
}

/// Context-window compaction event. opencode auto-compacts when the
/// session grows beyond a threshold (and the user can trigger it
/// manually). Payload carries `auto: bool`; opencode 1.14 doesn't
/// expose the before/after token counts on this part, so we just
/// surface the event itself — the surrounding `step-finish` parts
/// before/after give the token deltas.
fn extract_compaction(part: &Value, out: &mut Vec<ExtractedAction>) {
    let payload = json!({
        "auto": part.get("auto").cloned().unwrap_or(Value::Null),
    });
    out.push(ExtractedAction {
        kind: action_kind::COMPACTION,
        timestamp_ms: 0,
        payload: payload.to_string(),
        source: None,
    });
}

/// Model reasoning block. Carries `text` (plain-text summary opencode
/// keeps client-side) and `metadata.copilot.reasoningOpaque` (the
/// provider's encrypted reasoning blob). We persist both — the UI
/// decides what to show; the opaque blob is what re-hydrates
/// reasoning on resume so future tooling may want it.
fn extract_reasoning(part: &Value, out: &mut Vec<ExtractedAction>) {
    let text = part.get("text").cloned().unwrap_or(Value::Null);
    let started_at = part
        .get("time")
        .and_then(|t| t.get("start"))
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let ended_at = part
        .get("time")
        .and_then(|t| t.get("end"))
        .and_then(Value::as_i64)
        .unwrap_or(0);
    if ended_at == 0 {
        // Still streaming: the completed update carries the text, and
        // emitting both would give every block two rows.
        return;
    }
    let duration_ms = ended_at.saturating_sub(started_at);
    let opaque = part
        .get("metadata")
        .and_then(|m| m.get("copilot"))
        .and_then(|c| c.get("reasoningOpaque"))
        .cloned()
        .unwrap_or(Value::Null);
    let payload = json!({
        "text": text,
        "started_at_ms": started_at,
        "ended_at_ms": ended_at,
        "duration_ms": duration_ms,
        "reasoning_opaque": opaque,
    });
    out.push(ExtractedAction {
        kind: action_kind::REASONING,
        timestamp_ms: started_at,
        payload: payload.to_string(),
        source: None,
    });
}

/// Bound on remembered ids; a session's user messages are few, so this
/// only guards against unbounded growth over a very long-lived adapter.
const TRACKER_CAP: usize = 512;

/// A human prompt found on the live stream, with the session it was
/// written in so the caller can decide whether that session is root.
#[derive(Debug, Clone)]
pub struct ObservedPrompt {
    pub session_id: String,
    pub action: ExtractedAction,
}

/// Stateful observer turning opencode's two-event user prompt (a
/// user-role `message.updated`, then a text `message.part.updated`)
/// into one `user_prompt` action. The text part carries no role, so the
/// message ids seen with `role == "user"` are remembered. A text part
/// that arrives before its message is ignored (opencode sends the
/// message first); each part id emits at most once.
#[derive(Debug, Default)]
pub struct UserPromptTracker {
    /// user message id → (session id, `time.created`)
    user_messages: HashMap<String, (String, i64)>,
    emitted_parts: HashSet<String>,
}

impl UserPromptTracker {
    pub fn observe(&mut self, payload: &Value) -> Option<ObservedPrompt> {
        let ty = payload.get("type").and_then(Value::as_str)?;
        let props = payload.get("properties")?;
        match ty {
            "message.updated" => {
                let info = props.get("info")?;
                if info.get("role").and_then(Value::as_str) != Some("user") {
                    return None;
                }
                let id = info.get("id").and_then(Value::as_str)?;
                let session = info
                    .get("sessionID")
                    .or_else(|| props.get("sessionID"))
                    .and_then(Value::as_str)?;
                let created = info
                    .get("time")
                    .and_then(|t| t.get("created"))
                    .and_then(Value::as_i64)
                    .unwrap_or(0);
                if self.user_messages.len() >= TRACKER_CAP {
                    self.user_messages.clear();
                    self.emitted_parts.clear();
                }
                self.user_messages
                    .insert(id.to_owned(), (session.to_owned(), created));
                None
            }
            "message.part.updated" => {
                let part = props.get("part")?;
                let text = user_text_of(part)?;
                let part_id = part.get("id").and_then(Value::as_str)?;
                let message_id = part.get("messageID").and_then(Value::as_str)?;
                let (session, created) = self.user_messages.get(message_id)?.clone();
                if self.emitted_parts.contains(part_id) {
                    return None;
                }
                if self.emitted_parts.len() >= TRACKER_CAP {
                    self.user_messages.clear();
                    self.emitted_parts.clear();
                }
                self.emitted_parts.insert(part_id.to_owned());
                let timestamp_ms = if created > 0 {
                    created
                } else {
                    props.get("time").and_then(Value::as_i64).unwrap_or(0)
                };
                Some(ObservedPrompt {
                    session_id: session,
                    action: ExtractedAction {
                        kind: action_kind::USER_PROMPT,
                        timestamp_ms,
                        payload: json!({"prompt": text}).to_string(),
                        source: Some(part_id.to_owned()),
                    },
                })
            }
            _ => None,
        }
    }
}

/// The text of a non-synthetic, non-empty `text` part.
fn user_text_of(part: &Value) -> Option<&str> {
    if part.get("type").and_then(Value::as_str) != Some("text")
        || part.get("synthetic").and_then(Value::as_bool) == Some(true)
    {
        return None;
    }
    part.get("text")
        .and_then(Value::as_str)
        .filter(|t| !t.is_empty())
}

/// Join the non-synthetic text parts of one user message (`part.data`
/// values, in order) into the prompt; `None` when there is no text.
///
/// Live emits one row per text part, backfill one joined row per
/// message. opencode sends a single text part per typed prompt, so the
/// two agree in practice.
fn join_prompt_text(parts: &[Value]) -> Option<String> {
    let texts: Vec<&str> = parts.iter().filter_map(user_text_of).collect();
    (!texts.is_empty()).then(|| texts.join("\n"))
}

/// Read the opencode `SQLite` DB and extract all actions for a session,
/// persisting any with `timestamp_ms > max_ts` into Skein's DB in one
/// batch insert (#171e — this used to be one `INSERT` per action).
/// Returns the number of actions inserted. Opens the opencode DB
/// read-only so we never contend with opencode's writer lock.
pub fn backfill_from_db(
    session_id: &str,
    harness_id: &str,
    room_id: &str,
    max_ts: i64,
    skein_db: &crate::db::Database,
) -> usize {
    let Some(db_path) = crate::resume::opencode_db_path() else {
        return 0;
    };
    if !db_path.exists() {
        return 0;
    }
    let Ok(conn) = skein_harness::opencode::open_read_only(&db_path) else {
        return 0;
    };

    let mut rows_to_insert: Vec<crate::db::NewHarnessAction> = Vec::new();
    // Text parts per message id, in part order, for the user prompts below.
    let mut texts_by_message: HashMap<String, Vec<Value>> = HashMap::new();

    // Parts — the main source of tool calls, patches, step-finish.
    let Ok(mut stmt) = conn.prepare(
        "SELECT data, time_created, message_id FROM part \
         WHERE session_id = ?1 \
         ORDER BY time_created, id",
    ) else {
        return 0;
    };
    let Ok(rows) = stmt.query_map(rusqlite::params![session_id], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, i64>(1)?,
            row.get::<_, String>(2)?,
        ))
    }) else {
        return 0;
    };
    for row in rows {
        let Ok((data, ts_created, message_id)) = row else {
            continue;
        };
        let Ok(value) = serde_json::from_str::<Value>(&data) else {
            continue;
        };
        if value.get("type").and_then(Value::as_str) == Some("text") {
            texts_by_message
                .entry(message_id)
                .or_default()
                .push(value.clone());
        }
        let mut actions = Vec::new();
        extract_from_part(&value, &mut actions);
        for mut action in actions {
            if action.timestamp_ms == 0 {
                action.timestamp_ms = ts_created;
            }
            if action.timestamp_ms <= max_ts {
                continue;
            }
            rows_to_insert.push(crate::db::NewHarnessAction {
                timestamp_ms: action.timestamp_ms,
                kind: action.kind,
                payload: action.payload,
                source: action.source,
            });
        }
    }

    // User-role messages as user_prompt.
    if let Ok(mut msg_stmt) = conn.prepare(
        "SELECT data, time_created, id FROM message \
         WHERE session_id = ?1 \
         ORDER BY time_created, id",
    ) && let Ok(msg_rows) = msg_stmt.query_map(rusqlite::params![session_id], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, i64>(1)?,
            row.get::<_, String>(2)?,
        ))
    }) {
        for row in msg_rows {
            let Ok((data, ts_created, message_id)) = row else {
                continue;
            };
            let Ok(value) = serde_json::from_str::<Value>(&data) else {
                continue;
            };
            let role = value.get("role").and_then(Value::as_str).unwrap_or("");
            if role != "user" {
                continue;
            }
            if ts_created <= max_ts {
                continue;
            }
            let summary_text = value
                .get("summary")
                .and_then(|s| s.get("diffs"))
                .cloned()
                .unwrap_or(Value::Null);
            let prompt = texts_by_message
                .get(&message_id)
                .and_then(|parts| join_prompt_text(parts));
            let payload = json!({
                "prompt": prompt,
                "summary_diffs": summary_text,
            });
            rows_to_insert.push(crate::db::NewHarnessAction {
                timestamp_ms: ts_created,
                kind: action_kind::USER_PROMPT,
                payload: payload.to_string(),
                source: None,
            });
        }
    }

    match skein_db.record_harness_actions(harness_id, room_id, &rows_to_insert) {
        Ok(inserted) => inserted,
        Err(e) => {
            // max_ts didn't advance, so the next attach retries this same batch.
            tracing::warn!(harness_id = %harness_id, rows = rows_to_insert.len(), error = %e,
                "opencode backfill: batch record failed");
            0
        }
    }
}

#[cfg(test)]
mod tests;
