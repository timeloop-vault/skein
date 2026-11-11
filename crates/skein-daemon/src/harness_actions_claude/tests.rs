use super::*;
use serde_json::json;

#[allow(clippy::needless_pass_by_value)]
fn ingest_one(extractor: &mut ActionExtractor, value: Value) -> Vec<ExtractedAction> {
    extractor.ingest(&value)
}

// ISO-8601 parsing itself is tested in skein-harness (#209); the
// extractor tests only need one known epoch value.

fn ts_572() -> i64 {
    // 2026-05-15T21:16:22.572Z = 20588 days since 1970-01-01
    //   20588 * 86_400_000 = 1_778_803_200_000
    // + 21*3600000 + 16*60000 + 22*1000 + 572 = 76_582_572
    //   total = 1_778_879_782_572 ms
    1_778_879_782_572
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

/// `TodoWrite` is the full-snapshot plan tool. It must land in the
/// same `op: "write"` + `items` shape opencode's `todowrite` uses,
/// because `plan.ts` picks its reducer off that discriminator.
#[test]
fn todo_write_emits_full_snapshot_plan_item() {
    let mut x = ActionExtractor::new();
    ingest_one(
        &mut x,
        json!({
            "type": "assistant",
            "uuid": "a1",
            "timestamp": "2026-05-15T21:16:22.572Z",
            "message": {"content": [{
                "type": "tool_use", "id": "toolu_tw", "name": "TodoWrite",
                "input": {"todos": [
                    {"content": "First", "status": "completed",
                     "activeForm": "Doing first"},
                    {"content": "Second", "status": "in_progress",
                     "activeForm": "Doing second"},
                ]},
            }]},
        }),
    );
    let actions = ingest_one(
        &mut x,
        json!({
            "type": "user",
            "timestamp": "2026-05-15T21:16:23.000Z",
            "toolUseResult": {
                "oldTodos": [],
                "newTodos": [
                    {"content": "First", "status": "completed",
                     "activeForm": "Doing first"},
                    {"content": "Second", "status": "in_progress",
                     "activeForm": "Doing second"},
                    {"content": "Third", "status": "pending",
                     "activeForm": "Doing third"},
                ],
            },
            "message": {"content": [{
                "type": "tool_result", "tool_use_id": "toolu_tw",
                "content": "ok", "is_error": false
            }]},
        }),
    );
    assert_eq!(actions.len(), 1);
    let a = &actions[0];
    assert_eq!(a.kind, action_kind::PLAN_CHANGE);
    let payload: Value = serde_json::from_str(&a.payload).unwrap();
    assert_eq!(payload["plan_item"]["op"], "write");
    // `newTodos` (as applied) wins over the input's `todos`.
    assert_eq!(payload["plan_item"]["count"], 3);
    assert_eq!(payload["plan_item"]["items"][2]["content"], "Third");
    assert_eq!(payload["plan_item"]["items"][1]["status"], "in_progress");
}

/// Result shapes without `newTodos` (and rows whose `tool_result`
/// never landed a list) still produce a usable snapshot from the
/// tool input.
#[test]
fn todo_write_falls_back_to_input_todos() {
    let mut x = ActionExtractor::new();
    ingest_one(
        &mut x,
        json!({
            "type": "assistant",
            "uuid": "a1",
            "timestamp": "2026-05-15T21:16:22.572Z",
            "message": {"content": [{
                "type": "tool_use", "id": "toolu_tw", "name": "TodoWrite",
                "input": {"todos": [
                    {"content": "Only", "status": "pending",
                     "activeForm": "Doing only"},
                ]},
            }]},
        }),
    );
    let actions = ingest_one(
        &mut x,
        json!({
            "type": "user",
            "timestamp": "2026-05-15T21:16:23.000Z",
            "toolUseResult": {"ok": true},
            "message": {"content": [{
                "type": "tool_result", "tool_use_id": "toolu_tw",
                "content": "ok", "is_error": false
            }]},
        }),
    );
    assert_eq!(actions.len(), 1);
    let payload: Value = serde_json::from_str(&actions[0].payload).unwrap();
    assert_eq!(payload["plan_item"]["op"], "write");
    assert_eq!(payload["plan_item"]["count"], 1);
    assert_eq!(payload["plan_item"]["items"][0]["content"], "Only");
}

/// Read-only task tools stay `tool_call` — they mutate nothing, so
/// letting them through would put empty rows in the Plan card.
#[test]
fn read_only_task_tools_are_not_plan_changes() {
    for name in ["TaskList", "TaskGet", "TaskOutput", "TaskStop"] {
        assert_eq!(
            classify_tool(name),
            action_kind::TOOL_CALL,
            "{name} should classify as tool_call"
        );
    }
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
    // The error message comes from the tool_result content block (not
    // toolUseResult) so the activity feed's error row isn't blank.
    assert_eq!(payload["error"], "Exit code 1\n");
}

#[test]
fn successful_result_has_no_error_field() {
    let mut x = ActionExtractor::new();
    ingest_one(
        &mut x,
        json!({
            "type": "assistant",
            "uuid": "a1",
            "timestamp": "2026-05-15T21:16:22.572Z",
            "message": {"content": [{
                "type": "tool_use", "id": "toolu_1", "name": "Bash",
                "input": {"command": "ls"},
            }]},
        }),
    );
    let actions = ingest_one(
        &mut x,
        json!({
            "type": "user",
            "timestamp": "2026-05-15T21:16:23.000Z",
            "toolUseResult": {"stdout": "ok", "stderr": "", "interrupted": false},
            "message": {"content": [{
                "type": "tool_result", "tool_use_id": "toolu_1",
                "content": "ok", "is_error": false
            }]},
        }),
    );
    let payload: Value = serde_json::from_str(&actions[0].payload).unwrap();
    assert!(payload.get("error").is_none(), "no error field on success");
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

// ── timestamp-less rows ──────────────────────────────────────

#[test]
fn permission_mode_emits_action_with_carry_forward_ts() {
    // Permission-mode rows don't carry a `timestamp` field; the
    // extractor stamps them with the previous row's time.
    let mut x = ActionExtractor::new();
    // Prime a known timestamp from a real row first.
    ingest_one(
        &mut x,
        json!({
            "type": "system", "subtype": "turn_duration",
            "uuid": "u1",
            "timestamp": "2026-05-15T21:16:22.572Z",
            "durationMs": 100, "messageCount": 1,
        }),
    );
    let actions = ingest_one(
        &mut x,
        json!({
            "type": "permission-mode",
            "permissionMode": "acceptEdits",
            "sessionId": "s1",
        }),
    );
    assert_eq!(actions.len(), 1);
    let a = &actions[0];
    assert_eq!(a.kind, action_kind::PERMISSION_MODE);
    assert_eq!(a.timestamp_ms, ts_572());
    let payload: Value = serde_json::from_str(&a.payload).unwrap();
    assert_eq!(payload["permission_mode"], "acceptEdits");
}

#[test]
fn ai_title_emits_action() {
    let mut x = ActionExtractor::new();
    ingest_one(
        &mut x,
        json!({
            "type": "system", "subtype": "turn_duration",
            "uuid": "u1",
            "timestamp": "2026-05-15T21:16:22.572Z",
            "durationMs": 100, "messageCount": 1,
        }),
    );
    let actions = ingest_one(
        &mut x,
        json!({
            "type": "ai-title",
            "aiTitle": "Refactoring the activity feed",
            "sessionId": "s1",
        }),
    );
    assert_eq!(actions.len(), 1);
    let a = &actions[0];
    assert_eq!(a.kind, action_kind::AI_TITLE);
    assert_eq!(a.timestamp_ms, ts_572());
    let payload: Value = serde_json::from_str(&a.payload).unwrap();
    assert_eq!(payload["ai_title"], "Refactoring the activity feed");
}

#[test]
fn bridge_session_emits_bridge_status_action() {
    let mut x = ActionExtractor::new();
    ingest_one(
        &mut x,
        json!({
            "type": "system", "subtype": "turn_duration",
            "uuid": "u1",
            "timestamp": "2026-05-15T21:16:22.572Z",
            "durationMs": 100, "messageCount": 1,
        }),
    );
    let actions = ingest_one(
        &mut x,
        json!({
            "type": "bridge-session",
            "sessionId": "s1",
            "bridgeSessionId": "cse_01ABC",
            "lastSequenceNum": 0,
        }),
    );
    assert_eq!(actions.len(), 1);
    let a = &actions[0];
    assert_eq!(a.kind, action_kind::BRIDGE_STATUS);
    let payload: Value = serde_json::from_str(&a.payload).unwrap();
    assert_eq!(payload["bridge_session_id"], "cse_01ABC");
}

/// Shape taken verbatim from a real 2.1.263 `cost-state` row.
#[test]
fn cost_state_emits_session_totals_with_carry_forward_ts() {
    let mut x = ActionExtractor::new();
    ingest_one(
        &mut x,
        json!({
            "type": "system", "subtype": "turn_duration",
            "uuid": "u1",
            "timestamp": "2026-05-15T21:16:22.572Z",
            "durationMs": 100, "messageCount": 1,
        }),
    );
    let actions = ingest_one(
        &mut x,
        json!({
            "type": "cost-state",
            "sessionId": "s1",
            "totalCostUSD": 154.266_434_75,
            "totalAPIDuration": 20_196_313,
            "totalToolDuration": 1_920_542,
            "totalLinesAdded": 1273,
            "totalLinesRemoved": 41,
            "totalDuration": 190_980_216,
            "startTime": 1_788_371_932_507i64,
            "modelUsage": {
                "claude-fable-5-1": {
                    "inputTokens": 74395, "outputTokens": 1_517_947,
                    "costUSD": 150.483_92,
                },
            },
        }),
    );
    assert_eq!(actions.len(), 1);
    let a = &actions[0];
    assert_eq!(a.kind, action_kind::COST_STATE);
    // No uuid on the row, and no timestamp — it inherits the last.
    assert_eq!(a.source, None);
    assert_eq!(a.timestamp_ms, 1_778_879_782_572);
    let payload: Value = serde_json::from_str(&a.payload).unwrap();
    assert_eq!(payload["total_cost_usd"], 154.266_434_75);
    assert_eq!(payload["total_lines_added"], 1273);
    assert_eq!(payload["total_lines_removed"], 41);
    assert_eq!(payload["total_duration_ms"], 190_980_216);
    assert_eq!(payload["start_time_ms"], 1_788_371_932_507i64);
    assert_eq!(
        payload["model_usage"]["claude-fable-5-1"]["costUSD"],
        150.483_92
    );
}

/// Snapshots supersede rather than accumulate, so a later row must
/// stand on its own — a consumer that took the newest and a
/// consumer that summed would otherwise disagree.
#[test]
fn each_cost_state_row_carries_the_running_total() {
    let mut x = ActionExtractor::new();
    let first = ingest_one(
        &mut x,
        json!({"type": "cost-state", "sessionId": "s1", "totalCostUSD": 1.5}),
    );
    let second = ingest_one(
        &mut x,
        json!({"type": "cost-state", "sessionId": "s1", "totalCostUSD": 4.25}),
    );
    let usd = |a: &[ExtractedAction]| -> f64 {
        serde_json::from_str::<Value>(&a[0].payload).unwrap()["total_cost_usd"]
            .as_f64()
            .unwrap()
    };
    assert!((usd(&first) - 1.5).abs() < f64::EPSILON);
    assert!((usd(&second) - 4.25).abs() < f64::EPSILON);
}

/// A row missing every optional field must still produce a row
/// rather than panicking or being dropped — the frontend already
/// treats each field as optional.
#[test]
fn cost_state_tolerates_a_field_less_row() {
    let mut x = ActionExtractor::new();
    let actions = ingest_one(&mut x, json!({"type": "cost-state", "sessionId": "s1"}));
    assert_eq!(actions.len(), 1);
    assert_eq!(actions[0].kind, action_kind::COST_STATE);
    let payload: Value = serde_json::from_str(&actions[0].payload).unwrap();
    assert!(payload["total_cost_usd"].is_null());
    assert!(payload["model_usage"].is_null());
}

#[test]
fn last_prompt_emits_user_prompt_with_leaf_uuid_as_source() {
    let mut x = ActionExtractor::new();
    ingest_one(
        &mut x,
        json!({
            "type": "system", "subtype": "turn_duration",
            "uuid": "u1",
            "timestamp": "2026-05-15T21:16:22.572Z",
            "durationMs": 100, "messageCount": 1,
        }),
    );
    let actions = ingest_one(
        &mut x,
        json!({
            "type": "last-prompt",
            "lastPrompt": "please continue",
            "leafUuid": "leaf-abc",
            "sessionId": "s1",
        }),
    );
    assert_eq!(actions.len(), 1);
    let a = &actions[0];
    assert_eq!(a.kind, action_kind::USER_PROMPT);
    assert_eq!(a.source.as_deref(), Some("leaf-abc"));
    let payload: Value = serde_json::from_str(&a.payload).unwrap();
    assert_eq!(payload["prompt"], "please continue");
}

#[test]
fn timestamp_less_row_before_any_timestamped_row_gets_ts_zero() {
    // Edge case: session opens with a timestamp-less row. We
    // have no prior timestamp to carry forward; the row gets 0
    // and sorts to the bottom of newest-first queries. Not great
    // but bounded and not observed in real sessions.
    let mut x = ActionExtractor::new();
    let actions = ingest_one(
        &mut x,
        json!({
            "type": "permission-mode",
            "permissionMode": "default",
            "sessionId": "s1",
        }),
    );
    assert_eq!(actions.len(), 1);
    assert_eq!(actions[0].timestamp_ms, 0);
}

// ── row types we deliberately skip ───────────────────────────

#[test]
fn file_history_snapshot_and_summary_rows_emit_nothing() {
    // Documented exclusions: file-history-snapshot is Claude's
    // internal undo bookkeeping (no user-facing event); summary
    // wasn't observed in any recon session (shape unknown).
    let mut x = ActionExtractor::new();
    for ty in [
        "file-history-snapshot",
        "summary",
        "completely-new-row-type",
    ] {
        let row = json!({"type": ty, "timestamp": "2026-05-15T21:16:22.572Z"});
        assert!(ingest_one(&mut x, row).is_empty(), "{ty} should not emit");
    }
}
