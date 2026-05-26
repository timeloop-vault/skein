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
//! - `plan_change` — `TaskCreate` / `TaskUpdate` (the user's plan mutates
//!   here; read-only TaskList/Get fall into `tool_call`).
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
/// arrives. One instance per attached session.
#[derive(Default)]
pub struct ActionExtractor {
    pending: HashMap<String, PendingToolUse>,
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
        if value
            .get("isSidechain")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            return out;
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
            // Other row types — `ai-title`, `last-prompt`,
            // `file-history-snapshot`, `permission-mode`,
            // `bridge-session`, `summary` — aren't part of the v1
            // action vocabulary. Adding them later is a one-arm
            // addition here.
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
            out.push(build_tool_action(&pending, &result_body, is_error, ts));
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
        "TaskCreate" | "TaskUpdate" => action_kind::PLAN_CHANGE,
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
                if let Some(s) = line.as_str() {
                    if let Some(first) = s.chars().next() {
                        match first {
                            '+' => additions += 1,
                            '-' => deletions += 1,
                            _ => {}
                        }
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

/// Pull a normalized plan item out of `TaskCreate` / `TaskUpdate`. Plan
/// card reads this field directly; Activity card uses the parent
/// payload's tool/input/result for full detail.
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

/// Pull the row's `timestamp` field (ISO 8601) and convert to epoch
/// ms. Fall back to 0 when missing / malformed — the row still gets
/// persisted, just without a real time, so it'll sort to the bottom
/// of newest-first queries (where the user can see it and notice
/// something's off).
fn parse_timestamp_ms(value: &Value) -> i64 {
    value
        .get("timestamp")
        .and_then(Value::as_str)
        .and_then(parse_iso8601_ms)
        .unwrap_or(0)
}

/// Minimal ISO 8601 → epoch ms. Claude writes timestamps as
/// `"2026-05-15T21:16:22.572Z"`. We avoid pulling chrono just for
/// this — the surface is narrow and stable.
#[allow(clippy::cast_sign_loss, clippy::cast_possible_wrap)]
fn parse_iso8601_ms(s: &str) -> Option<i64> {
    // Format: YYYY-MM-DDTHH:MM:SS[.fff]Z
    let bytes = s.as_bytes();
    if bytes.len() < 20 || bytes[10] != b'T' || *bytes.last().unwrap_or(&b'_') != b'Z' {
        return None;
    }
    let year: i64 = s.get(0..4)?.parse().ok()?;
    let month: i64 = s.get(5..7)?.parse().ok()?;
    let day: i64 = s.get(8..10)?.parse().ok()?;
    let hour: i64 = s.get(11..13)?.parse().ok()?;
    let minute: i64 = s.get(14..16)?.parse().ok()?;
    let second: i64 = s.get(17..19)?.parse().ok()?;
    let millis: i64 = if bytes.get(19) == Some(&b'.') {
        // Up to 3 fractional digits, zero-padded on the right.
        let frac_end = s.len() - 1; // strip trailing Z
        let frac = s.get(20..frac_end)?;
        let mut padded = String::with_capacity(3);
        padded.push_str(frac);
        while padded.len() < 3 {
            padded.push('0');
        }
        padded.get(..3)?.parse().ok()?
    } else {
        0
    };
    Some(
        days_from_civil(year, month, day) * 86_400_000
            + hour * 3_600_000
            + minute * 60_000
            + second * 1_000
            + millis,
    )
}

/// Howard Hinnant's date algorithm — days since 1970-01-01 (Unix
/// epoch). Exact for proleptic Gregorian, valid for any year in the
/// i64 range. We use it instead of pulling chrono.
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[allow(clippy::needless_pass_by_value)]
    fn ingest_one(extractor: &mut ActionExtractor, value: Value) -> Vec<ExtractedAction> {
        extractor.ingest(&value)
    }

    // ── timestamp parser ─────────────────────────────────────────

    #[test]
    fn parses_iso8601_with_millis() {
        assert_eq!(parse_iso8601_ms("2026-05-15T21:16:22.572Z"), Some(ts_572()));
    }

    fn ts_572() -> i64 {
        // 2026-05-15T21:16:22.572Z = 20588 days since 1970-01-01
        //   20588 * 86_400_000 = 1_778_803_200_000
        // + 21*3600000 + 16*60000 + 22*1000 + 572 = 76_582_572
        //   total = 1_778_879_782_572 ms
        1_778_879_782_572
    }

    #[test]
    fn parses_iso8601_without_millis() {
        // Trailing seconds, no fractional component.
        let with = parse_iso8601_ms("2026-05-15T21:16:22.000Z").unwrap();
        let without = parse_iso8601_ms("2026-05-15T21:16:22Z").unwrap();
        assert_eq!(with, without);
    }

    #[test]
    fn rejects_invalid_timestamp_returns_none() {
        assert_eq!(parse_iso8601_ms("not a timestamp"), None);
        assert_eq!(parse_iso8601_ms("2026-05-15"), None);
        assert_eq!(parse_iso8601_ms(""), None);
    }

    // ── tool join (the headline) ─────────────────────────────────

    #[test]
    fn assistant_tool_use_alone_emits_nothing() {
        // The tool_use row is buffered; nothing emitted until the
        // result arrives.
        let mut x = ActionExtractor::new();
        let row = json!({
            "type": "assistant",
            "uuid": "a1",
            "timestamp": "2026-05-15T21:16:22.572Z",
            "message": {
                "content": [
                    {"type": "tool_use", "id": "toolu_1", "name": "Bash",
                     "input": {"command": "ls"}}
                ],
                "stop_reason": "tool_use",
            },
        });
        assert!(ingest_one(&mut x, row).is_empty());
    }

    #[test]
    fn tool_use_then_result_emits_tool_call_action() {
        let mut x = ActionExtractor::new();
        ingest_one(
            &mut x,
            json!({
                "type": "assistant",
                "uuid": "a1",
                "timestamp": "2026-05-15T21:16:22.572Z",
                "message": {
                    "content": [
                        {"type": "tool_use", "id": "toolu_1", "name": "Bash",
                         "input": {"command": "ls -la"}}
                    ],
                },
            }),
        );
        let actions = ingest_one(
            &mut x,
            json!({
                "type": "user",
                "sourceToolUseID": "toolu_1",
                "timestamp": "2026-05-15T21:16:23.000Z",
                "toolUseResult": {"stdout": "file1\n", "stderr": "", "interrupted": false},
                "message": {
                    "content": [
                        {"type": "tool_result", "tool_use_id": "toolu_1",
                         "content": "file1\n", "is_error": false}
                    ],
                },
            }),
        );
        assert_eq!(actions.len(), 1);
        let a = &actions[0];
        assert_eq!(a.kind, action_kind::TOOL_CALL);
        assert_eq!(a.timestamp_ms, ts_572());
        assert_eq!(a.source.as_deref(), Some("a1"));
        let payload: Value = serde_json::from_str(&a.payload).unwrap();
        assert_eq!(payload["tool"], "Bash");
        assert_eq!(payload["input"]["command"], "ls -la");
        assert_eq!(payload["is_error"], false);
        assert_eq!(payload["duration_ms"], 428);
        assert_eq!(payload["result"]["stdout"], "file1\n");
    }

    #[test]
    fn edit_tool_classified_as_patch_with_normalized_info() {
        let mut x = ActionExtractor::new();
        ingest_one(
            &mut x,
            json!({
                "type": "assistant",
                "uuid": "a1",
                "timestamp": "2026-05-15T21:16:22.572Z",
                "message": {"content": [{
                    "type": "tool_use", "id": "toolu_e", "name": "Edit",
                    "input": {"file_path": "/abs/path/foo.rs",
                              "old_string": "old", "new_string": "new"},
                }]},
            }),
        );
        let actions = ingest_one(
            &mut x,
            json!({
                "type": "user",
                "sourceToolAssistantUUID": "a1",
                "timestamp": "2026-05-15T21:16:23.000Z",
                "toolUseResult": {
                    "filePath": "/abs/path/foo.rs",
                    "userModified": false,
                    "structuredPatch": [{
                        "oldStart": 1, "oldLines": 1, "newStart": 1, "newLines": 2,
                        "lines": [" context", "-old", "+new", "+extra"]
                    }],
                },
                "message": {"content": [{
                    "type": "tool_result", "tool_use_id": "toolu_e",
                    "content": "ok", "is_error": false
                }]},
            }),
        );
        assert_eq!(actions.len(), 1);
        let a = &actions[0];
        assert_eq!(a.kind, action_kind::PATCH);
        let payload: Value = serde_json::from_str(&a.payload).unwrap();
        assert_eq!(payload["files"], json!(["/abs/path/foo.rs"]));
        assert_eq!(payload["patch_info"]["additions"], 2);
        assert_eq!(payload["patch_info"]["deletions"], 1);
        assert_eq!(payload["patch_info"]["user_modified"], false);
    }

    #[test]
    fn task_create_classified_as_plan_change() {
        let mut x = ActionExtractor::new();
        ingest_one(
            &mut x,
            json!({
                "type": "assistant",
                "uuid": "a1",
                "timestamp": "2026-05-15T21:16:22.572Z",
                "message": {"content": [{
                    "type": "tool_use", "id": "toolu_tc", "name": "TaskCreate",
                    "input": {"subject": "Do the thing",
                              "description": "details",
                              "activeForm": "Doing the thing"},
                }]},
            }),
        );
        let actions = ingest_one(
            &mut x,
            json!({
                "type": "user",
                "timestamp": "2026-05-15T21:16:23.000Z",
                "toolUseResult": {"task": {"id": "1", "subject": "Do the thing"}},
                "message": {"content": [{
                    "type": "tool_result", "tool_use_id": "toolu_tc",
                    "content": "Task #1 created", "is_error": false
                }]},
            }),
        );
        assert_eq!(actions.len(), 1);
        let a = &actions[0];
        assert_eq!(a.kind, action_kind::PLAN_CHANGE);
        let payload: Value = serde_json::from_str(&a.payload).unwrap();
        assert_eq!(payload["plan_item"]["op"], "create");
        assert_eq!(payload["plan_item"]["id"], "1");
        assert_eq!(payload["plan_item"]["subject"], "Do the thing");
        assert_eq!(payload["plan_item"]["status"], "pending");
    }

    #[test]
    fn task_update_carries_status_change() {
        let mut x = ActionExtractor::new();
        ingest_one(
            &mut x,
            json!({
                "type": "assistant",
                "uuid": "a1",
                "timestamp": "2026-05-15T21:16:22.572Z",
                "message": {"content": [{
                    "type": "tool_use", "id": "toolu_tu", "name": "TaskUpdate",
                    "input": {"taskId": "1", "status": "in_progress"},
                }]},
            }),
        );
        let actions = ingest_one(
            &mut x,
            json!({
                "type": "user",
                "timestamp": "2026-05-15T21:16:23.000Z",
                "toolUseResult": {
                    "success": true, "taskId": "1",
                    "updatedFields": ["status"],
                    "statusChange": {"from": "pending", "to": "in_progress"},
                },
                "message": {"content": [{
                    "type": "tool_result", "tool_use_id": "toolu_tu",
                    "content": "ok", "is_error": false
                }]},
            }),
        );
        assert_eq!(actions.len(), 1);
        let a = &actions[0];
        assert_eq!(a.kind, action_kind::PLAN_CHANGE);
        let payload: Value = serde_json::from_str(&a.payload).unwrap();
        assert_eq!(payload["plan_item"]["op"], "update");
        assert_eq!(payload["plan_item"]["status_change"]["from"], "pending");
        assert_eq!(payload["plan_item"]["status_change"]["to"], "in_progress");
    }

    #[test]
    fn parallel_tool_uses_in_one_row_join_with_results_in_one_row() {
        let mut x = ActionExtractor::new();
        ingest_one(
            &mut x,
            json!({
                "type": "assistant",
                "uuid": "a1",
                "timestamp": "2026-05-15T21:16:22.572Z",
                "message": {"content": [
                    {"type": "tool_use", "id": "toolu_a", "name": "Read",
                     "input": {"file_path": "/x"}},
                    {"type": "tool_use", "id": "toolu_b", "name": "Read",
                     "input": {"file_path": "/y"}},
                ]},
            }),
        );
        let actions = ingest_one(
            &mut x,
            json!({
                "type": "user",
                "timestamp": "2026-05-15T21:16:23.000Z",
                "toolUseResult": {"file": "x contents", "type": "file"},
                "message": {"content": [
                    {"type": "tool_result", "tool_use_id": "toolu_a",
                     "content": "x contents", "is_error": false},
                    {"type": "tool_result", "tool_use_id": "toolu_b",
                     "content": "y contents", "is_error": false},
                ]},
            }),
        );
        assert_eq!(actions.len(), 2);
        let payload_a: Value = serde_json::from_str(&actions[0].payload).unwrap();
        let payload_b: Value = serde_json::from_str(&actions[1].payload).unwrap();
        assert_eq!(payload_a["input"]["file_path"], "/x");
        assert_eq!(payload_b["input"]["file_path"], "/y");
    }

    #[test]
    fn error_result_propagates_is_error_true() {
        let mut x = ActionExtractor::new();
        ingest_one(
            &mut x,
            json!({
                "type": "assistant",
                "uuid": "a1",
                "timestamp": "2026-05-15T21:16:22.572Z",
                "message": {"content": [{
                    "type": "tool_use", "id": "toolu_1", "name": "Bash",
                    "input": {"command": "false"},
                }]},
            }),
        );
        let actions = ingest_one(
            &mut x,
            json!({
                "type": "user",
                "timestamp": "2026-05-15T21:16:23.000Z",
                "toolUseResult": {"stdout": "", "stderr": "", "interrupted": false},
                "message": {"content": [{
                    "type": "tool_result", "tool_use_id": "toolu_1",
                    "content": "Exit code 1\n", "is_error": true
                }]},
            }),
        );
        assert_eq!(actions.len(), 1);
        let payload: Value = serde_json::from_str(&actions[0].payload).unwrap();
        assert_eq!(payload["is_error"], true);
    }

    #[test]
    fn result_for_unknown_tool_use_id_is_dropped_silently() {
        // Adapter attached mid-stream; we see a result for a tool_use
        // we never observed. Don't emit a broken action.
        let mut x = ActionExtractor::new();
        let actions = ingest_one(
            &mut x,
            json!({
                "type": "user",
                "timestamp": "2026-05-15T21:16:23.000Z",
                "toolUseResult": {"stdout": "stale"},
                "message": {"content": [{
                    "type": "tool_result", "tool_use_id": "toolu_orphan",
                    "content": "stale", "is_error": false
                }]},
            }),
        );
        assert!(actions.is_empty());
    }

    // ── stateless extractors ─────────────────────────────────────

    #[test]
    fn pr_link_row_emits_pr_link_action() {
        let mut x = ActionExtractor::new();
        let actions = ingest_one(
            &mut x,
            json!({
                "type": "pr-link",
                "prNumber": 61,
                "prUrl": "https://github.com/foo/bar/pull/61",
                "prRepository": "foo/bar",
                "timestamp": "2026-05-18T07:21:06.089Z",
            }),
        );
        assert_eq!(actions.len(), 1);
        let a = &actions[0];
        assert_eq!(a.kind, action_kind::PR_LINK);
        let payload: Value = serde_json::from_str(&a.payload).unwrap();
        assert_eq!(payload["pr_number"], 61);
        assert_eq!(payload["pr_url"], "https://github.com/foo/bar/pull/61");
    }

    #[test]
    fn queue_operation_emits_queue_op() {
        let mut x = ActionExtractor::new();
        let actions = ingest_one(
            &mut x,
            json!({
                "type": "queue-operation",
                "operation": "enqueue",
                "content": "please continue",
                "timestamp": "2026-05-19T11:54:19.886Z",
            }),
        );
        assert_eq!(actions.len(), 1);
        let payload: Value = serde_json::from_str(&actions[0].payload).unwrap();
        assert_eq!(payload["operation"], "enqueue");
        assert_eq!(payload["content"], "please continue");
    }

    #[test]
    fn edited_text_file_attachment_emits_action() {
        let mut x = ActionExtractor::new();
        let actions = ingest_one(
            &mut x,
            json!({
                "type": "attachment",
                "uuid": "att-1",
                "timestamp": "2026-05-15T21:16:22.572Z",
                "attachment": {
                    "type": "edited_text_file",
                    "filename": "/path/foo.rs",
                    "snippet": "..."
                },
            }),
        );
        assert_eq!(actions.len(), 1);
        let a = &actions[0];
        assert_eq!(a.kind, action_kind::EDITED_TEXT_FILE);
        let payload: Value = serde_json::from_str(&a.payload).unwrap();
        assert_eq!(payload["filename"], "/path/foo.rs");
    }

    #[test]
    fn other_attachment_subtypes_are_skipped() {
        let mut x = ActionExtractor::new();
        for subtype in [
            "task_reminder",
            "date_change",
            "deferred_tools_delta",
            "skill_listing",
            "plan_mode_exit",
            "command_permissions",
        ] {
            let actions = ingest_one(
                &mut x,
                json!({
                    "type": "attachment",
                    "timestamp": "2026-05-15T21:16:22.572Z",
                    "attachment": {"type": subtype},
                }),
            );
            assert!(actions.is_empty(), "{subtype} should not emit");
        }
    }

    #[test]
    fn system_subtypes_map_to_their_kinds() {
        let mut x = ActionExtractor::new();
        for (subtype, expected_kind) in [
            ("local_command", action_kind::SLASH_COMMAND),
            ("away_summary", action_kind::AWAY_SUMMARY),
            ("turn_duration", action_kind::TURN_DURATION),
            ("api_error", action_kind::API_ERROR),
        ] {
            let actions = ingest_one(
                &mut x,
                json!({
                    "type": "system",
                    "subtype": subtype,
                    "uuid": format!("u-{subtype}"),
                    "timestamp": "2026-05-15T21:16:22.572Z",
                    "content": "x",
                    "error": {"status": 529},
                    "durationMs": 1000,
                    "messageCount": 5,
                }),
            );
            assert_eq!(actions.len(), 1, "{subtype} should emit 1");
            assert_eq!(actions[0].kind, expected_kind, "{subtype} kind mismatch");
        }
    }

    #[test]
    fn system_informational_subtype_is_not_in_v1_vocabulary() {
        let mut x = ActionExtractor::new();
        let actions = ingest_one(
            &mut x,
            json!({
                "type": "system",
                "subtype": "informational",
                "timestamp": "2026-05-15T21:16:22.572Z",
                "content": "Remote Control failed",
            }),
        );
        assert!(actions.is_empty());
    }

    #[test]
    fn turn_duration_payload_has_normalized_fields() {
        let mut x = ActionExtractor::new();
        let actions = ingest_one(
            &mut x,
            json!({
                "type": "system",
                "subtype": "turn_duration",
                "timestamp": "2026-05-15T21:16:22.572Z",
                "durationMs": 63075,
                "messageCount": 32,
            }),
        );
        let payload: Value = serde_json::from_str(&actions[0].payload).unwrap();
        assert_eq!(payload["duration_ms"], 63075);
        assert_eq!(payload["message_count"], 32);
    }

    // ── turn_cost ────────────────────────────────────────────────

    #[test]
    fn terminal_assistant_row_emits_turn_cost() {
        let mut x = ActionExtractor::new();
        let actions = ingest_one(
            &mut x,
            json!({
                "type": "assistant",
                "uuid": "a-end",
                "requestId": "req_1",
                "timestamp": "2026-05-15T21:18:31.000Z",
                "message": {
                    "model": "claude-opus-4-7",
                    "stop_reason": "end_turn",
                    "usage": {
                        "input_tokens": 100, "output_tokens": 50,
                        "cache_read_input_tokens": 9000,
                    },
                    "content": [{"type": "text", "text": "done"}],
                },
            }),
        );
        assert_eq!(actions.len(), 1);
        let a = &actions[0];
        assert_eq!(a.kind, action_kind::TURN_COST);
        let payload: Value = serde_json::from_str(&a.payload).unwrap();
        assert_eq!(payload["model"], "claude-opus-4-7");
        assert_eq!(payload["stop_reason"], "end_turn");
        assert_eq!(payload["usage"]["input_tokens"], 100);
        assert_eq!(payload["request_id"], "req_1");
    }

    #[test]
    fn non_terminal_assistant_row_does_not_emit_turn_cost() {
        let mut x = ActionExtractor::new();
        let actions = ingest_one(
            &mut x,
            json!({
                "type": "assistant",
                "uuid": "a-mid",
                "timestamp": "2026-05-15T21:18:00.000Z",
                "message": {
                    "model": "claude-opus-4-7",
                    "stop_reason": "tool_use",
                    "usage": {"input_tokens": 100, "output_tokens": 50},
                    "content": [{"type": "tool_use", "id": "t", "name": "Read", "input": {}}],
                },
            }),
        );
        assert!(actions.is_empty() || actions.iter().all(|a| a.kind != action_kind::TURN_COST));
    }

    // ── sub-agent / sidechain filter ─────────────────────────────

    #[test]
    fn sidechain_rows_are_ignored() {
        let mut x = ActionExtractor::new();
        let actions = ingest_one(
            &mut x,
            json!({
                "type": "assistant",
                "uuid": "a-side",
                "isSidechain": true,
                "timestamp": "2026-05-15T21:16:22.572Z",
                "message": {
                    "content": [{"type": "tool_use", "id": "toolu_s", "name": "Read",
                                 "input": {"file_path": "/x"}}],
                },
            }),
        );
        assert!(actions.is_empty());
        // And the buffered tool_use should not have been recorded —
        // a follow-up result on the main chain should drop silently.
        let actions = ingest_one(
            &mut x,
            json!({
                "type": "user",
                "timestamp": "2026-05-15T21:16:23.000Z",
                "toolUseResult": {"file": "x"},
                "message": {"content": [{
                    "type": "tool_result", "tool_use_id": "toolu_s",
                    "content": "x", "is_error": false
                }]},
            }),
        );
        assert!(actions.is_empty());
    }

    // ── row types not in v1 vocab ────────────────────────────────

    #[test]
    fn unknown_or_metadata_rows_emit_nothing() {
        let mut x = ActionExtractor::new();
        for ty in [
            "ai-title",
            "last-prompt",
            "file-history-snapshot",
            "permission-mode",
            "bridge-session",
            "summary",
            "completely-new-row-type",
        ] {
            let row = json!({"type": ty, "timestamp": "2026-05-15T21:16:22.572Z"});
            assert!(ingest_one(&mut x, row).is_empty(), "{ty} should not emit");
        }
    }
}
