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
//! - `user_prompt` — user-role messages.
//! - `ai_title` — `session.updated` SSE events with a title change.
//!
//! opencode-specific kinds not present in Claude today:
//! - `compaction` — context-window compaction events.
//! - `reasoning` — the model's reasoning blocks (text + opaque blob).

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
    } else if kind == action_kind::PLAN_CHANGE {
        if let Some(obj) = payload.as_object_mut() {
            if let Some(pi) = extract_plan_item(state) {
                obj.insert("plan_item".into(), pi);
            }
        }
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

/// Read the opencode `SQLite` DB and extract all actions for a session,
/// persisting any with `timestamp_ms > max_ts` into Skein's DB.
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
    let Ok(conn) =
        rusqlite::Connection::open_with_flags(&db_path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
    else {
        return 0;
    };

    let mut count = 0;

    // Parts — the main source of tool calls, patches, step-finish.
    let Ok(mut stmt) = conn.prepare(
        "SELECT data, time_created FROM part \
         WHERE session_id = ?1 \
         ORDER BY time_created, id",
    ) else {
        return 0;
    };
    let Ok(rows) = stmt.query_map(rusqlite::params![session_id], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
    }) else {
        return 0;
    };
    for row in rows {
        let Ok((data, ts_created)) = row else {
            continue;
        };
        let Ok(value) = serde_json::from_str::<Value>(&data) else {
            continue;
        };
        let mut actions = Vec::new();
        extract_from_part(&value, &mut actions);
        for mut action in actions {
            if action.timestamp_ms == 0 {
                action.timestamp_ms = ts_created;
            }
            if action.timestamp_ms <= max_ts {
                continue;
            }
            if let Err(e) = skein_db.record_harness_action(
                harness_id,
                room_id,
                action.timestamp_ms,
                action.kind,
                &action.payload,
                action.source.as_deref(),
            ) {
                tracing::trace!(error = %e, "opencode backfill: record failed");
            } else {
                count += 1;
            }
        }
    }

    // User-role messages as user_prompt.
    if let Ok(mut msg_stmt) = conn.prepare(
        "SELECT data, time_created FROM message \
         WHERE session_id = ?1 \
         ORDER BY time_created, id",
    ) {
        if let Ok(msg_rows) = msg_stmt.query_map(rusqlite::params![session_id], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        }) {
            for row in msg_rows {
                let Ok((data, ts_created)) = row else {
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
                let payload = json!({
                    "prompt": null,
                    "summary_diffs": summary_text,
                });
                if let Err(e) = skein_db.record_harness_action(
                    harness_id,
                    room_id,
                    ts_created,
                    action_kind::USER_PROMPT,
                    &payload.to_string(),
                    None,
                ) {
                    tracing::trace!(error = %e, "opencode backfill: user_prompt record failed");
                } else {
                    count += 1;
                }
            }
        }
    }

    count
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // ── SSE extraction ───────────────────────────────────────────

    #[test]
    fn completed_bash_tool_emits_tool_call() {
        let payload = json!({
            "type": "message.part.updated",
            "properties": {"part": {
                "type": "tool", "tool": "bash",
                "callID": "toolu_1",
                "state": {
                    "status": "completed",
                    "input": {"command": "ls", "description": "list files"},
                    "output": "file1\nfile2\n",
                    "metadata": {"output": "file1\nfile2\n", "exit": 0, "truncated": false},
                    "title": "list files",
                    "time": {"start": 1000, "end": 1050},
                }
            }}
        });
        let actions = extract_from_sse(&payload);
        assert_eq!(actions.len(), 1);
        let a = &actions[0];
        assert_eq!(a.kind, action_kind::TOOL_CALL);
        assert_eq!(a.timestamp_ms, 1000);
        assert_eq!(a.source.as_deref(), Some("toolu_1"));
        let p: Value = serde_json::from_str(&a.payload).unwrap();
        assert_eq!(p["tool"], "bash");
        assert_eq!(p["duration_ms"], 50);
        assert_eq!(p["title"], "list files");
    }

    #[test]
    fn edit_tool_classified_as_patch() {
        let payload = json!({
            "type": "message.part.updated",
            "properties": {"part": {
                "type": "tool", "tool": "edit",
                "callID": "toolu_2",
                "state": {
                    "status": "completed",
                    "input": {"filePath": "/foo.rs", "oldString": "a", "newString": "b"},
                    "output": "Edit applied successfully.",
                    "metadata": {
                        "filediff": {"file": "/foo.rs", "patch": "@@ ...", "additions": 1, "deletions": 1},
                        "truncated": false,
                    },
                    "title": "foo.rs",
                    "time": {"start": 2000, "end": 2001},
                }
            }}
        });
        let actions = extract_from_sse(&payload);
        assert_eq!(actions.len(), 1);
        let a = &actions[0];
        assert_eq!(a.kind, action_kind::PATCH);
        let p: Value = serde_json::from_str(&a.payload).unwrap();
        assert_eq!(p["files"], json!(["/foo.rs"]));
        assert_eq!(p["patch_info"]["additions"], 1);
        assert_eq!(p["patch_info"]["deletions"], 1);
    }

    #[test]
    fn todowrite_classified_as_plan_change() {
        let payload = json!({
            "type": "message.part.updated",
            "properties": {"part": {
                "type": "tool", "tool": "todowrite",
                "callID": "toolu_3",
                "state": {
                    "status": "completed",
                    "input": {"todos": [
                        {"content": "Do thing A", "status": "pending", "priority": "high"},
                        {"content": "Do thing B", "status": "completed", "priority": "high"},
                    ]},
                    "output": "[...]",
                    "metadata": {"todos": [
                        {"content": "Do thing A", "status": "pending", "priority": "high"},
                        {"content": "Do thing B", "status": "completed", "priority": "high"},
                    ], "truncated": false},
                    "title": "2 todos",
                    "time": {"start": 3000, "end": 3001},
                }
            }}
        });
        let actions = extract_from_sse(&payload);
        assert_eq!(actions.len(), 1);
        let a = &actions[0];
        assert_eq!(a.kind, action_kind::PLAN_CHANGE);
        let p: Value = serde_json::from_str(&a.payload).unwrap();
        assert_eq!(p["plan_item"]["count"], 2);
    }

    #[test]
    fn error_tool_emits_with_error_flag() {
        let payload = json!({
            "type": "message.part.updated",
            "properties": {"part": {
                "type": "tool", "tool": "edit",
                "callID": "toolu_err",
                "state": {
                    "status": "error",
                    "input": {},
                    "error": "Found multiple matches",
                    "metadata": {"interrupted": false},
                    "time": {"start": 4000, "end": 4001},
                }
            }}
        });
        let actions = extract_from_sse(&payload);
        assert_eq!(actions.len(), 1);
        let p: Value = serde_json::from_str(&actions[0].payload).unwrap();
        assert_eq!(p["is_error"], true);
        assert_eq!(p["error"], "Found multiple matches");
    }

    #[test]
    fn pending_or_running_tool_does_not_emit() {
        for status in ["pending", "running"] {
            let payload = json!({
                "type": "message.part.updated",
                "properties": {"part": {
                    "type": "tool", "tool": "bash",
                    "callID": "toolu_x",
                    "state": {
                        "status": status,
                        "input": {"command": "ls"},
                        "time": {"start": 5000},
                    }
                }}
            });
            let actions = extract_from_sse(&payload);
            assert!(actions.is_empty(), "{status} should not emit");
        }
    }

    #[test]
    fn step_finish_emits_turn_cost() {
        let payload = json!({
            "type": "message.part.updated",
            "properties": {"part": {
                "type": "step-finish",
                "reason": "tool-calls",
                "snapshot": "abc123",
                "tokens": {"total": 1000, "input": 800, "output": 200, "reasoning": 0,
                           "cache": {"write": 0, "read": 700}},
                "cost": 0.005,
            }}
        });
        let actions = extract_from_sse(&payload);
        assert_eq!(actions.len(), 1);
        let a = &actions[0];
        assert_eq!(a.kind, action_kind::TURN_COST);
        let p: Value = serde_json::from_str(&a.payload).unwrap();
        assert_eq!(p["reason"], "tool-calls");
        assert_eq!(p["tokens"]["total"], 1000);
    }

    #[test]
    fn patch_part_emits_patch_action() {
        let payload = json!({
            "type": "message.part.updated",
            "properties": {"part": {
                "type": "patch",
                "hash": "abc123",
                "files": ["/foo.rs", "/bar.rs"],
            }}
        });
        let actions = extract_from_sse(&payload);
        assert_eq!(actions.len(), 1);
        let a = &actions[0];
        assert_eq!(a.kind, action_kind::PATCH);
        let p: Value = serde_json::from_str(&a.payload).unwrap();
        assert_eq!(p["files"], json!(["/foo.rs", "/bar.rs"]));
    }

    #[test]
    fn session_updated_with_title_emits_ai_title() {
        let payload = json!({
            "type": "session.updated",
            "properties": {
                "title": "Refactoring the parser",
                "time": {"created": 1000, "updated": 2000},
            }
        });
        let actions = extract_from_sse(&payload);
        assert_eq!(actions.len(), 1);
        assert_eq!(actions[0].kind, action_kind::AI_TITLE);
        let p: Value = serde_json::from_str(&actions[0].payload).unwrap();
        assert_eq!(p["ai_title"], "Refactoring the parser");
    }

    #[test]
    fn irrelevant_sse_events_emit_nothing() {
        for ty in [
            "server.heartbeat",
            "server.connected",
            "session.status",
            "session.idle",
            "session.created",
            "session.diff",
            "mcp.tools.changed",
            "message.part.delta",
        ] {
            let payload = json!({"type": ty, "properties": {}});
            assert!(
                extract_from_sse(&payload).is_empty(),
                "{ty} should not emit"
            );
        }
    }

    #[test]
    fn narration_and_step_start_parts_do_not_emit() {
        for part_type in ["text", "step-start", "file"] {
            let payload = json!({
                "type": "message.part.updated",
                "properties": {"part": {"type": part_type, "text": "hello"}}
            });
            assert!(
                extract_from_sse(&payload).is_empty(),
                "{part_type} part should not emit"
            );
        }
    }

    #[test]
    fn compaction_part_emits_compaction_action() {
        let payload = json!({
            "type": "message.part.updated",
            "properties": {"part": {"type": "compaction", "auto": true}}
        });
        let actions = extract_from_sse(&payload);
        assert_eq!(actions.len(), 1);
        let a = &actions[0];
        assert_eq!(a.kind, action_kind::COMPACTION);
        let p: Value = serde_json::from_str(&a.payload).unwrap();
        assert_eq!(p["auto"], true);
    }

    #[test]
    fn reasoning_part_emits_reasoning_action() {
        let payload = json!({
            "type": "message.part.updated",
            "properties": {"part": {
                "type": "reasoning",
                "text": "Update summary with progress.",
                "time": {"start": 1_000, "end": 1_500},
                "metadata": {"copilot": {"reasoningOpaque": "OPAQUE_BLOB"}},
            }}
        });
        let actions = extract_from_sse(&payload);
        assert_eq!(actions.len(), 1);
        let a = &actions[0];
        assert_eq!(a.kind, action_kind::REASONING);
        assert_eq!(a.timestamp_ms, 1_000);
        let p: Value = serde_json::from_str(&a.payload).unwrap();
        assert_eq!(p["text"], "Update summary with progress.");
        assert_eq!(p["duration_ms"], 500);
        assert_eq!(p["reasoning_opaque"], "OPAQUE_BLOB");
    }

    #[test]
    fn reasoning_part_without_opaque_blob_still_emits() {
        // Some providers don't supply the opaque blob — we still
        // capture the text.
        let payload = json!({
            "type": "message.part.updated",
            "properties": {"part": {
                "type": "reasoning",
                "text": "Thinking through the problem.",
                "time": {"start": 2_000, "end": 2_200},
            }}
        });
        let actions = extract_from_sse(&payload);
        assert_eq!(actions.len(), 1);
        let p: Value = serde_json::from_str(&actions[0].payload).unwrap();
        assert_eq!(p["text"], "Thinking through the problem.");
        assert!(p["reasoning_opaque"].is_null());
    }

    #[test]
    fn question_tool_classified_as_tool_call() {
        let payload = json!({
            "type": "message.part.updated",
            "properties": {"part": {
                "type": "tool", "tool": "question",
                "callID": "toolu_q",
                "state": {
                    "status": "completed",
                    "input": {"questions": [{"question": "Which?"}]},
                    "output": "User answered",
                    "metadata": {"answers": [["Option A"]], "truncated": false},
                    "title": "Asked 1 question",
                    "time": {"start": 6000, "end": 6500},
                }
            }}
        });
        let actions = extract_from_sse(&payload);
        assert_eq!(actions.len(), 1);
        assert_eq!(actions[0].kind, action_kind::TOOL_CALL);
        let p: Value = serde_json::from_str(&actions[0].payload).unwrap();
        assert_eq!(p["tool"], "question");
        assert_eq!(p["title"], "Asked 1 question");
    }
}
