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

// ── user prompts (#525) ──────────────────────────────────────

fn user_message(id: &str, session: &str, created: i64) -> Value {
    json!({
        "id": "evt_1",
        "type": "message.updated",
        "properties": {"sessionID": session, "info": {
            "role": "user", "id": id, "sessionID": session,
            "time": {"created": created}, "agent": "build",
            "summary": {"diffs": []},
        }}
    })
}

fn assistant_message(id: &str, session: &str) -> Value {
    json!({
        "id": "evt_2",
        "type": "message.updated",
        "properties": {"sessionID": session, "info": {
            "role": "assistant", "id": id, "sessionID": session,
            "time": {"created": 1_791_068_013_000_i64},
        }}
    })
}

fn text_part(part_id: &str, message_id: &str, text: &str) -> Value {
    json!({
        "id": "evt_3",
        "type": "message.part.updated",
        "properties": {"sessionID": "ses_A", "part": {
            "id": part_id, "messageID": message_id, "sessionID": "ses_A",
            "type": "text", "text": text,
        }}
    })
}

#[test]
fn user_message_then_text_part_emits_one_user_prompt() {
    let mut t = UserPromptTracker::default();
    assert!(
        t.observe(&user_message("msg_U", "ses_A", 1_791_068_012_813))
            .is_none()
    );
    let got = t
        .observe(&text_part(
            "prt_P",
            "msg_U",
            "Read the file /work/hello.txt",
        ))
        .expect("prompt");
    assert_eq!(got.session_id, "ses_A");
    assert_eq!(got.action.kind, action_kind::USER_PROMPT);
    assert_eq!(got.action.timestamp_ms, 1_791_068_012_813);
    assert_eq!(got.action.source.as_deref(), Some("prt_P"));
    let p: Value = serde_json::from_str(&got.action.payload).unwrap();
    assert_eq!(p["prompt"], "Read the file /work/hello.txt");
}

#[test]
fn repeated_text_part_and_message_updates_emit_once() {
    let mut t = UserPromptTracker::default();
    t.observe(&user_message("msg_U", "ses_A", 100));
    assert!(t.observe(&text_part("prt_P", "msg_U", "hi")).is_some());
    t.observe(&user_message("msg_U", "ses_A", 100));
    assert!(t.observe(&text_part("prt_P", "msg_U", "hi")).is_none());
}

#[test]
fn assistant_text_part_is_not_a_prompt() {
    let mut t = UserPromptTracker::default();
    t.observe(&user_message("msg_U", "ses_A", 100));
    t.observe(&assistant_message("msg_A1", "ses_A"));
    assert!(
        t.observe(&text_part(
            "prt_A",
            "msg_A1",
            "The secret word is pineapple."
        ))
        .is_none()
    );
}

#[test]
fn synthetic_and_empty_text_parts_are_skipped() {
    let mut t = UserPromptTracker::default();
    t.observe(&user_message("msg_U", "ses_A", 100));
    let mut synthetic = text_part("prt_S", "msg_U", "Called the Read tool");
    synthetic["properties"]["part"]["synthetic"] = json!(true);
    assert!(t.observe(&synthetic).is_none());
    assert!(t.observe(&text_part("prt_E", "msg_U", "")).is_none());
}

#[test]
fn text_part_before_its_message_is_ignored() {
    let mut t = UserPromptTracker::default();
    assert!(t.observe(&text_part("prt_P", "msg_U", "hi")).is_none());
}

#[test]
fn timestamp_falls_back_to_event_time_then_zero() {
    let mut t = UserPromptTracker::default();
    t.observe(&user_message("msg_U", "ses_A", 0));
    let mut part = text_part("prt_P", "msg_U", "hi");
    part["properties"]["time"] = json!(555);
    assert_eq!(t.observe(&part).unwrap().action.timestamp_ms, 555);
    let got = t.observe(&text_part("prt_Q", "msg_U", "again")).unwrap();
    assert_eq!(got.action.timestamp_ms, 0);
}

#[test]
fn tracker_memory_is_bounded() {
    let mut t = UserPromptTracker::default();
    for i in 0..(TRACKER_CAP * 3) {
        t.observe(&user_message(&format!("msg_{i}"), "ses_A", 1));
        t.observe(&text_part(&format!("prt_{i}"), &format!("msg_{i}"), "x"));
    }
    assert!(t.user_messages.len() <= TRACKER_CAP);
    assert!(t.emitted_parts.len() <= TRACKER_CAP);
}

#[test]
fn join_prompt_text_joins_non_synthetic_text_parts() {
    let parts = vec![
        json!({"type": "text", "text": "first"}),
        json!({"type": "text", "text": "ctx", "synthetic": true}),
        json!({"type": "text", "text": "second"}),
    ];
    assert_eq!(join_prompt_text(&parts).as_deref(), Some("first\nsecond"));
    assert_eq!(join_prompt_text(&[]), None);
    assert_eq!(
        join_prompt_text(&[json!({"type": "text", "text": "x", "synthetic": true})]),
        None
    );
}

#[test]
fn in_progress_reasoning_does_not_emit_completed_does() {
    let mk = |time: Value| {
        json!({
            "type": "message.part.updated",
            "properties": {"part": {"type": "reasoning", "text": "", "time": time}}
        })
    };
    assert!(extract_from_sse(&mk(json!({"start": 1_000}))).is_empty());
    let done = extract_from_sse(&mk(json!({"start": 1_000, "end": 1_400})));
    assert_eq!(done.len(), 1);
    let p: Value = serde_json::from_str(&done[0].payload).unwrap();
    assert_eq!(p["duration_ms"], 400);
}
