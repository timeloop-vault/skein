//! Claude JSONL → `harness_actions` extraction. Issue #80 part B.
//!
//! Sibling of `harness_events_claude.rs`. That module reads the JSONL
//! for phase signal (running / waiting / idle); this one extracts the
//! richer set of actions the Live Context cards render. The two work
//! on the same JSONL — calling `parse_line` for phase and
//! `ActionExtractor::ingest` for actions on every row.
//!
//! ## Action kinds emitted
//!
//! Each row contributes 0 or more actions. The classification mirrors
//! `db::action_kind` (with the explicit choice that every tool gets
//! *exactly one* kind, not a `tool_call` *and* a more-specific kind —
//! see `docs/live-context-design-brief.md` and the recon):
//!
//! - `tool_call` — every `tool_use` block whose name isn't claimed below.
//! - `plan_change` — `TaskCreate` / `TaskUpdate` / `TodoWrite` (the
//!   user's plan mutates here; read-only TaskList/Get fall into
//!   `tool_call`). The first two emit incremental deltas, `TodoWrite`
//!   a full snapshot — see `extract_plan_item`.
//! - `patch` — `Edit` / `Write` / `MultiEdit` (file modifications;
//!   Read/Grep/Glob are still `tool_call`).
//! - `pr_link` — row type `pr-link`.
//! - `queue_op` — row type `queue-operation`.
//! - `edited_text_file` — attachment subtype `edited_text_file`.
//! - `slash_command` — system subtype `local_command`.
//! - `away_summary` — system subtype `away_summary`.
//! - `turn_duration` — system subtype `turn_duration`.
//! - `api_error` — system subtype `api_error`.
//! - `turn_cost` — terminal assistant row (`end_turn` / `stop_sequence`
//!   / `max_tokens`); payload carries the row's `message.usage` + model.
//! - `cost_state` — `cost-state` row: the session's cumulative cost /
//!   token / line totals. A snapshot, not an event — newest wins.
//! - `permission_mode` — `permission-mode` row.
//! - `ai_title` — `ai-title` row (Claude's auto-generated session title).
//! - `bridge_status` — `bridge-session` row (Remote Control bridge id).
//! - `user_prompt` — `last-prompt` row (user-typed prompt captured).
//!
//! ## Tool-use / tool-result joining
//!
//! `tool_call` / `plan_change` / `patch` need both halves: the
//! assistant row's `tool_use` block (name + input) AND the next user
//! row's `toolUseResult` (success / output / structured patch). The
//! extractor buffers pending `tool_use` entries keyed by `tool_use.id`
//! and emits the joined action only when the matching result row is
//! ingested. Sub-agents (Claude's `Agent` tool) close out via the
//! `Agent` result returning its own rich block — no special handling
//! needed.
//!
//! ## Stateless vs stateful
//!
//! Every other kind is row-local — one row in, one action out.
//! The only state is the pending-tool-use map.

use std::collections::HashMap;

use serde_json::{Value, json};

use crate::db::action_kind;

/// One action emitted by the extractor, ready to be persisted via
/// `Database::record_harness_action`. `payload` is a serialized JSON
/// string — the DB layer stores it verbatim.
#[derive(Debug, Clone)]
pub struct ExtractedAction {
    pub kind: &'static str,
    pub timestamp_ms: i64,
    pub payload: String,
    pub source: Option<String>,
}

/// Stateful extractor — buffers `tool_use` blocks until their result
/// arrives, and carries the most-recent observed timestamp forward
/// for row types that don't carry their own.
#[derive(Default)]
pub struct ActionExtractor {
    pending: HashMap<String, PendingToolUse>,
    /// Some row types (`last-prompt`, `ai-title`, `permission-mode`,
    /// `bridge-session`) don't carry a `timestamp` field. They're
    /// always interleaved with timestamped rows in the JSONL, so we
    /// stamp them with the prior row's time to keep them ordered
    /// correctly relative to surrounding activity. Starts at 0;
    /// rows that arrive before any timestamped row get 0 (sort to
    /// bottom of newest-first queries — acceptable since this only
    /// happens on a session that opens with those rows, which we
    /// haven't observed).
    last_ts_ms: i64,
}

#[derive(Debug)]
struct PendingToolUse {
    /// Tool name as it appears in the JSONL (e.g. `Edit`, `TaskCreate`).
    name: String,
    /// `tool_use.input` JSON value, preserved verbatim.
    input: Value,
    /// Epoch ms from the assistant row's `timestamp`.
    started_at_ms: i64,
    /// Assistant row uuid — used as the `source` on the joined action
    /// so consumers can link back to the originating row.
    assistant_uuid: Option<String>,
}

impl ActionExtractor {
    pub fn new() -> Self {
        Self::default()
    }

    /// Ingest one parsed JSONL row. Returns 0 or more actions.
    pub fn ingest(&mut self, value: &Value) -> Vec<ExtractedAction> {
        let mut out = Vec::new();
        // Sub-agent rows have their own dedicated session log — the
        // main session's events drive the main-session activity feed
        // only, so isSidechain=true rows are skipped at this layer.
        if skein_harness::claude::is_sidechain(value) {
            return out;
        }
        // Track the most recent real timestamp for timestamp-less
        // rows that follow.
        let row_ts = parse_timestamp_ms(value);
        if row_ts > 0 {
            self.last_ts_ms = row_ts;
        }
        let Some(ty) = value.get("type").and_then(Value::as_str) else {
            return out;
        };
        match ty {
            "assistant" => self.ingest_assistant(value, &mut out),
            "user" => self.ingest_user(value, &mut out),
            "system" => extract_system(value, &mut out),
            "attachment" => extract_attachment(value, &mut out),
            "pr-link" => extract_pr_link(value, &mut out),
            "queue-operation" => extract_queue_op(value, &mut out),
            "permission-mode" => extract_permission_mode(value, self.last_ts_ms, &mut out),
            "ai-title" => extract_ai_title(value, self.last_ts_ms, &mut out),
            "bridge-session" => extract_bridge_session(value, self.last_ts_ms, &mut out),
            "last-prompt" => extract_user_prompt(value, self.last_ts_ms, &mut out),
            "cost-state" => extract_cost_state(value, self.last_ts_ms, &mut out),
            // Row types we deliberately don't surface:
            // - `file-history-snapshot`: Claude's internal undo
            //   bookkeeping (a `trackedFileBackups` map rewritten
            //   every message). No user-facing event.
            // - `summary`: 0 rows observed in any session in recon;
            //   shape unknown — extractor would be writing blind.
            _ => {}
        }
        out
    }

    fn ingest_assistant(&mut self, value: &Value, out: &mut Vec<ExtractedAction>) {
        let ts = parse_timestamp_ms(value);
        let assistant_uuid = value.get("uuid").and_then(Value::as_str).map(str::to_owned);
        let Some(message) = value.get("message") else {
            return;
        };

        // Buffer every tool_use block in this row for later joining
        // with its result. One assistant row can carry multiple
        // tool_use blocks (parallel tool calls).
        if let Some(content) = message.get("content").and_then(Value::as_array) {
            for block in content {
                if block.get("type").and_then(Value::as_str) != Some("tool_use") {
                    continue;
                }
                let Some(id) = block.get("id").and_then(Value::as_str) else {
                    continue;
                };
                let Some(name) = block.get("name").and_then(Value::as_str) else {
                    continue;
                };
                let input = block.get("input").cloned().unwrap_or(Value::Null);
                self.pending.insert(
                    id.to_owned(),
                    PendingToolUse {
                        name: name.to_owned(),
                        input,
                        started_at_ms: ts,
                        assistant_uuid: assistant_uuid.clone(),
                    },
                );
            }
        }

        // Terminal stop_reason rows close the turn — emit turn_cost
        // with the usage telemetry on the row. We don't try to
        // aggregate across the streamed chunks of the turn; Claude
        // writes the cumulative usage on the terminal row already.
        let stop_reason = message.get("stop_reason").and_then(Value::as_str);
        let terminal = matches!(
            stop_reason,
            Some("end_turn" | "stop_sequence" | "max_tokens")
        );
        if terminal {
            let usage = message.get("usage").cloned().unwrap_or(Value::Null);
            let payload = json!({
                "model": message.get("model").cloned().unwrap_or(Value::Null),
                "stop_reason": stop_reason.unwrap_or(""),
                "usage": usage,
                "request_id": value.get("requestId").cloned().unwrap_or(Value::Null),
            });
            out.push(ExtractedAction {
                kind: action_kind::TURN_COST,
                timestamp_ms: ts,
                payload: payload.to_string(),
                source: assistant_uuid,
            });
        }
    }

    fn ingest_user(&mut self, value: &Value, out: &mut Vec<ExtractedAction>) {
        // User rows can carry one or more tool_result blocks in
        // message.content[], each paired by tool_use_id. The parallel
        // structured field `toolUseResult` (singular) carries the
        // richer per-tool payload — for Edit it has structuredPatch,
        // for Bash it has stdout/stderr/exitCode, etc. We pass it
        // through verbatim under "result".
        let Some(content) = value
            .get("message")
            .and_then(|m| m.get("content"))
            .and_then(Value::as_array)
        else {
            return;
        };

        let ts = parse_timestamp_ms(value);
        // Most user rows carry exactly one tool_result block, paired
        // with one toolUseResult field. When multiple tool_results
        // appear in one row (parallel tool calls), only the structured
        // `toolUseResult` field exists for the *primary* — additional
        // results have just the textual `content`. Capture both.
        let primary_result = value.get("toolUseResult").cloned();
        let mut primary_emitted = false;

        for block in content {
            if block.get("type").and_then(Value::as_str) != Some("tool_result") {
                continue;
            }
            let Some(tool_use_id) = block.get("tool_use_id").and_then(Value::as_str) else {
                continue;
            };
            let Some(pending) = self.pending.remove(tool_use_id) else {
                // Result for an unknown tool_use_id — likely from
                // before this extractor attached, or from a row we
                // skipped (sub-agent, malformed). Drop silently;
                // there's nothing useful to emit without the input.
                continue;
            };
            let is_error = block
                .get("is_error")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let result_body = if !primary_emitted && primary_result.is_some() {
                primary_emitted = true;
                primary_result.clone().unwrap_or(Value::Null)
            } else {
                // Fall back to the textual content block for
                // secondary results in parallel-tool-call rows.
                block.get("content").cloned().unwrap_or(Value::Null)
            };
            // On error the message lives in the tool_result `content`
            // block (a `<tool_use_error>` string), NOT in toolUseResult
            // (success-shaped or null). Capture it as `error` so the
            // activity feed's error row has a message — mirrors opencode.
            let error_text = if is_error {
                block.get("content").and_then(Value::as_str)
            } else {
                None
            };
            out.push(build_tool_action(
                &pending,
                &result_body,
                is_error,
                error_text,
                ts,
            ));
        }
    }
}

/// Classify a joined `tool_use` + result into a kind, normalize the
/// payload, and produce the `ExtractedAction`. The payload always
/// carries: tool, `tool_use_id` (best effort), input, result, `is_error`,
/// `started_at_ms`, `ended_at_ms`, `duration_ms`. Patch + `plan_change` rows
/// get an additional normalized field for their card's quick access.
fn build_tool_action(
    pending: &PendingToolUse,
    result: &Value,
    is_error: bool,
    error_text: Option<&str>,
    ended_at_ms: i64,
) -> ExtractedAction {
    let kind = classify_tool(&pending.name);
    let duration_ms = ended_at_ms.saturating_sub(pending.started_at_ms);
    let mut payload = json!({
        "tool": pending.name,
        "input": pending.input,
        "result": result,
        "is_error": is_error,
        "started_at_ms": pending.started_at_ms,
        "ended_at_ms": ended_at_ms,
        "duration_ms": duration_ms,
    });

    // Error message (when present) so the feed's error row isn't blank.
    if let Some(err) = error_text
        && let Some(obj) = payload.as_object_mut()
    {
        obj.insert("error".into(), json!(err));
    }

    // Card-specific normalization on top of the raw shape.
    if kind == action_kind::PATCH {
        if let Some(obj) = payload.as_object_mut() {
            obj.insert("files".into(), files_touched(&pending.name, &pending.input));
            if let Some(patch_info) = extract_patch_info(result) {
                obj.insert("patch_info".into(), patch_info);
            }
        }
    } else if kind == action_kind::PLAN_CHANGE
        && let Some(obj) = payload.as_object_mut()
        && let Some(plan) = extract_plan_item(&pending.name, &pending.input, result)
    {
        obj.insert("plan_item".into(), plan);
    }

    ExtractedAction {
        kind,
        timestamp_ms: pending.started_at_ms,
        payload: payload.to_string(),
        source: pending.assistant_uuid.clone(),
    }
}

fn classify_tool(name: &str) -> &'static str {
    match name {
        "Edit" | "Write" | "MultiEdit" => action_kind::PATCH,
        "TaskCreate" | "TaskUpdate" | "TodoWrite" => action_kind::PLAN_CHANGE,
        _ => action_kind::TOOL_CALL,
    }
}

/// Best-effort list of files touched by a tool. For Edit/Write the
/// path is in `file_path`; for `MultiEdit` it's the same. Returns an
/// empty array when the input doesn't expose a path (Bash etc.) —
/// callers can still rely on the field existing on every patch row.
fn files_touched(name: &str, input: &Value) -> Value {
    match name {
        "Edit" | "Write" | "MultiEdit" => input
            .get("file_path")
            .and_then(Value::as_str)
            .map_or(json!([]), |p| json!([p])),
        _ => json!([]),
    }
}

/// Pull additions/deletions and the structuredPatch out of an Edit /
/// Write result. The `structuredPatch` is git-style hunks; we keep
/// the full structure (the Diff card will render it directly) and
/// also compute the line counts so the Activity card has them
/// without re-walking the hunks.
fn extract_patch_info(result: &Value) -> Option<Value> {
    let patch = result.get("structuredPatch")?;
    let hunks = patch.as_array()?;
    let mut additions: i64 = 0;
    let mut deletions: i64 = 0;
    for hunk in hunks {
        if let Some(lines) = hunk.get("lines").and_then(Value::as_array) {
            for line in lines {
                if let Some(s) = line.as_str()
                    && let Some(first) = s.chars().next()
                {
                    match first {
                        '+' => additions += 1,
                        '-' => deletions += 1,
                        _ => {}
                    }
                }
            }
        }
    }
    Some(json!({
        "additions": additions,
        "deletions": deletions,
        "structured_patch": patch,
        "user_modified": result.get("userModified").cloned().unwrap_or(Value::Null),
    }))
}

/// Pull a normalized plan item out of `TaskCreate` / `TaskUpdate` /
/// `TodoWrite`. Plan card reads this field directly; Activity card uses
/// the parent payload's tool/input/result for full detail.
///
/// Two shapes, because the tools differ: `TaskCreate`/`TaskUpdate` are
/// incremental deltas keyed by task id, `TodoWrite` is a full snapshot
/// of the whole list. `plan.ts` already reduces both (the snapshot path
/// exists for opencode's `todowrite`), so `TodoWrite` reuses that
/// `op: "write"` + `items` shape verbatim rather than inventing a third.
fn extract_plan_item(name: &str, input: &Value, result: &Value) -> Option<Value> {
    match name {
        "TaskCreate" => Some(json!({
            "op": "create",
            "id": result
                .get("task")
                .and_then(|t| t.get("id"))
                .cloned()
                .unwrap_or(Value::Null),
            "subject": input.get("subject").cloned().unwrap_or(Value::Null),
            "description": input.get("description").cloned().unwrap_or(Value::Null),
            "active_form": input.get("activeForm").cloned().unwrap_or(Value::Null),
            "status": "pending",
        })),
        "TaskUpdate" => {
            let task_id = input.get("taskId").cloned().unwrap_or(Value::Null);
            let status_change = result.get("statusChange").cloned().unwrap_or(Value::Null);
            let updated_fields = result.get("updatedFields").cloned().unwrap_or(json!([]));
            Some(json!({
                "op": "update",
                "id": task_id,
                "updated_fields": updated_fields,
                "status_change": status_change,
            }))
        }
        // The result's `newTodos` is the list as actually applied;
        // the input's `todos` is what was asked for. Prefer the
        // former, fall back to the latter (older result shapes, or a
        // row whose tool_result never landed).
        "TodoWrite" => {
            let todos = result
                .get("newTodos")
                .and_then(Value::as_array)
                .or_else(|| input.get("todos").and_then(Value::as_array))?;
            Some(json!({
                "op": "write",
                "count": todos.len(),
                "items": todos,
            }))
        }
        _ => None,
    }
}

fn extract_system(value: &Value, out: &mut Vec<ExtractedAction>) {
    let Some(subtype) = value.get("subtype").and_then(Value::as_str) else {
        return;
    };
    let ts = parse_timestamp_ms(value);
    let source = value.get("uuid").and_then(Value::as_str).map(str::to_owned);
    let kind = match subtype {
        "local_command" => action_kind::SLASH_COMMAND,
        "away_summary" => action_kind::AWAY_SUMMARY,
        "turn_duration" => action_kind::TURN_DURATION,
        "api_error" => action_kind::API_ERROR,
        // Other system subtypes — `informational`, `bridge_status`,
        // future ones — aren't in the v1 vocabulary. Add when needed.
        _ => return,
    };
    let payload = system_payload(subtype, value);
    out.push(ExtractedAction {
        kind,
        timestamp_ms: ts,
        payload: payload.to_string(),
        source,
    });
}

fn system_payload(subtype: &str, value: &Value) -> Value {
    match subtype {
        "local_command" => json!({
            "content": value.get("content").cloned().unwrap_or(Value::Null),
            "level": value.get("level").cloned().unwrap_or(Value::Null),
        }),
        "away_summary" => json!({
            "content": value.get("content").cloned().unwrap_or(Value::Null),
        }),
        "turn_duration" => json!({
            "duration_ms": value.get("durationMs").cloned().unwrap_or(Value::Null),
            "message_count": value.get("messageCount").cloned().unwrap_or(Value::Null),
        }),
        "api_error" => json!({
            "error": value.get("error").cloned().unwrap_or(Value::Null),
            "level": value.get("level").cloned().unwrap_or(Value::Null),
            "retry_attempt": value.get("retryAttempt").cloned().unwrap_or(Value::Null),
            "max_retries": value.get("maxRetries").cloned().unwrap_or(Value::Null),
            "retry_in_ms": value.get("retryInMs").cloned().unwrap_or(Value::Null),
        }),
        _ => Value::Null,
    }
}

fn extract_attachment(value: &Value, out: &mut Vec<ExtractedAction>) {
    let Some(attachment) = value.get("attachment") else {
        return;
    };
    let Some(subtype) = attachment.get("type").and_then(Value::as_str) else {
        return;
    };
    // Only edited_text_file is in the v1 vocabulary. The other
    // attachment subtypes (task_reminder, date_change, etc.) are
    // internal nudges to Claude or visual-only; they don't go in
    // the activity feed.
    if subtype != "edited_text_file" {
        return;
    }
    let ts = parse_timestamp_ms(value);
    let source = value.get("uuid").and_then(Value::as_str).map(str::to_owned);
    let payload = json!({
        "filename": attachment.get("filename").cloned().unwrap_or(Value::Null),
        "snippet": attachment.get("snippet").cloned().unwrap_or(Value::Null),
    });
    out.push(ExtractedAction {
        kind: action_kind::EDITED_TEXT_FILE,
        timestamp_ms: ts,
        payload: payload.to_string(),
        source,
    });
}

fn extract_pr_link(value: &Value, out: &mut Vec<ExtractedAction>) {
    let ts = parse_timestamp_ms(value);
    let payload = json!({
        "pr_number": value.get("prNumber").cloned().unwrap_or(Value::Null),
        "pr_url": value.get("prUrl").cloned().unwrap_or(Value::Null),
        "pr_repository": value.get("prRepository").cloned().unwrap_or(Value::Null),
    });
    out.push(ExtractedAction {
        kind: action_kind::PR_LINK,
        timestamp_ms: ts,
        payload: payload.to_string(),
        // pr-link rows don't carry a uuid — leave source null.
        source: None,
    });
}

fn extract_queue_op(value: &Value, out: &mut Vec<ExtractedAction>) {
    let ts = parse_timestamp_ms(value);
    let payload = json!({
        "operation": value.get("operation").cloned().unwrap_or(Value::Null),
        "content": value.get("content").cloned().unwrap_or(Value::Null),
    });
    out.push(ExtractedAction {
        kind: action_kind::QUEUE_OP,
        timestamp_ms: ts,
        payload: payload.to_string(),
        source: None,
    });
}

/// Permission-mode rows are timestamp-less in the JSONL; we use the
/// carry-forward `last_ts_ms` so they sort next to surrounding
/// activity. Same pattern for `ai-title`, `bridge-session`, and
/// `last-prompt` below.
fn extract_permission_mode(value: &Value, last_ts_ms: i64, out: &mut Vec<ExtractedAction>) {
    let payload = json!({
        "permission_mode": value.get("permissionMode").cloned().unwrap_or(Value::Null),
    });
    out.push(ExtractedAction {
        kind: action_kind::PERMISSION_MODE,
        timestamp_ms: last_ts_ms,
        payload: payload.to_string(),
        source: None,
    });
}

fn extract_ai_title(value: &Value, last_ts_ms: i64, out: &mut Vec<ExtractedAction>) {
    let payload = json!({
        "ai_title": value.get("aiTitle").cloned().unwrap_or(Value::Null),
    });
    out.push(ExtractedAction {
        kind: action_kind::AI_TITLE,
        timestamp_ms: last_ts_ms,
        payload: payload.to_string(),
        source: None,
    });
}

/// Claude's cumulative session roll-up (`cost-state`, new in 2.1.246).
///
/// Not a feed event — it is a *snapshot*, rewritten a handful of times
/// per session, each row superseding the last. Consumers take the
/// newest and ignore the rest; summing them would multiply the total.
///
/// This is the only place Claude tells us what a session actually cost.
/// The per-turn `turn_cost` rows carry token usage and a model name but
/// no money (#91), so a Claude room's session cost has read `$0.00`
/// since the card shipped — the alternative was a hand-maintained price
/// table that would rot on every pricing change. `totalCostUSD` and the
/// per-model `costUSD` breakdown come straight from upstream instead.
///
/// Like the other metadata rows, `cost-state` carries no `timestamp`,
/// so it inherits the last one seen.
fn extract_cost_state(value: &Value, last_ts_ms: i64, out: &mut Vec<ExtractedAction>) {
    let payload = json!({
        "total_cost_usd": value.get("totalCostUSD").cloned().unwrap_or(Value::Null),
        "model_usage": value.get("modelUsage").cloned().unwrap_or(Value::Null),
        "total_lines_added": value.get("totalLinesAdded").cloned().unwrap_or(Value::Null),
        "total_lines_removed": value.get("totalLinesRemoved").cloned().unwrap_or(Value::Null),
        "total_duration_ms": value.get("totalDuration").cloned().unwrap_or(Value::Null),
        "total_api_duration_ms": value.get("totalAPIDuration").cloned().unwrap_or(Value::Null),
        "total_tool_duration_ms": value.get("totalToolDuration").cloned().unwrap_or(Value::Null),
        "start_time_ms": value.get("startTime").cloned().unwrap_or(Value::Null),
    });
    out.push(ExtractedAction {
        kind: action_kind::COST_STATE,
        timestamp_ms: last_ts_ms,
        payload: payload.to_string(),
        // No uuid on these rows.
        source: None,
    });
}

fn extract_bridge_session(value: &Value, last_ts_ms: i64, out: &mut Vec<ExtractedAction>) {
    let payload = json!({
        "bridge_session_id": value.get("bridgeSessionId").cloned().unwrap_or(Value::Null),
        "last_sequence_num": value.get("lastSequenceNum").cloned().unwrap_or(Value::Null),
    });
    out.push(ExtractedAction {
        kind: action_kind::BRIDGE_STATUS,
        timestamp_ms: last_ts_ms,
        payload: payload.to_string(),
        source: None,
    });
}

/// `last-prompt` rows are written when Claude captures a new
/// user-typed prompt. The full prompt text is on `lastPrompt`
/// (truncated to ~200 chars by Claude); `leafUuid` identifies the
/// row in Claude's conversation DAG.
fn extract_user_prompt(value: &Value, last_ts_ms: i64, out: &mut Vec<ExtractedAction>) {
    let leaf = value
        .get("leafUuid")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let payload = json!({
        "prompt": value.get("lastPrompt").cloned().unwrap_or(Value::Null),
        "leaf_uuid": value.get("leafUuid").cloned().unwrap_or(Value::Null),
    });
    out.push(ExtractedAction {
        kind: action_kind::USER_PROMPT,
        timestamp_ms: last_ts_ms,
        payload: payload.to_string(),
        source: leaf,
    });
}

/// The row's `timestamp` (ISO 8601) as epoch ms, via `skein-harness`.
/// Falls back to 0 when missing / malformed — the row still gets
/// persisted, just without a real time, so it'll sort to the bottom
/// of newest-first queries (where the user can see it and notice
/// something's off).
fn parse_timestamp_ms(value: &Value) -> i64 {
    skein_harness::claude::timestamp_ms(value).unwrap_or(0)
}

#[cfg(test)]
mod tests;
