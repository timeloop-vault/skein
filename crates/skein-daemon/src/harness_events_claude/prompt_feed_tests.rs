//! #723 end to end: backfill, a restart, and the live tail each keep
//! one `user_prompt` per real prompt and one `ai_title` per change.

use super::persist_tests::make_persisting_adapter;
use super::tests::{append_block, wait_for};
use crate::db::{Database, action_kind};
use serde_json::{Value, json};
use tempfile::TempDir;

fn per_request(text: &str, ts: &str, t: &str) -> Vec<Value> {
    vec![
        json!({"type": "api-request"}),
        json!({"type": "attachment", "timestamp": ts, "attachment": {"type": "prompt_snapshot"}}),
        json!({"type": "last-prompt", "lastPrompt": text, "leafUuid": "leaf", "sessionId": "s1"}),
        json!({"type": "ai-title", "aiTitle": t, "sessionId": "s1"}),
    ]
}

fn prompt_rows(n: u32, uuid: &str, ts: &str, text: &str, t: &str) -> Vec<String> {
    let tool = format!("toolu_{n}");
    let mut rows = vec![json!({
        "type": "user", "uuid": uuid, "promptId": "p", "timestamp": ts,
        "origin": {"kind": "human"},
        "message": {"role": "user", "content": text}
    })];
    rows.extend(per_request(text, ts, t));
    rows.push(json!({
        "type": "assistant", "uuid": format!("a{n}"), "timestamp": ts,
        "requestId": format!("req_{n}"),
        "message": {
            "id": format!("msg_{n}"), "model": "claude-opus-4-1", "stop_reason": "tool_use",
            "content": [{"type": "tool_use", "id": tool, "name": "Bash",
                         "input": {"command": "ls"}}],
            "usage": {"input_tokens": 3, "output_tokens": 5}
        }
    }));
    rows.push(json!({
        "type": "user", "timestamp": ts, "promptId": "p",
        "toolUseResult": {"stdout": "x"},
        "message": {"content": [
            {"type": "tool_result", "tool_use_id": tool, "content": "x", "is_error": false}
        ]}
    }));
    rows.extend(per_request(text, ts, t));
    rows.push(json!({
        "type": "assistant", "uuid": format!("e{n}"), "timestamp": ts,
        "requestId": format!("req_e{n}"),
        "message": {
            "id": format!("msg_e{n}"), "model": "claude-opus-4-1", "stop_reason": "end_turn",
            "content": [{"type": "text", "text": "done"}],
            "usage": {"input_tokens": 3, "output_tokens": 7}
        }
    }));
    rows.push(
        json!({"type": "system", "subtype": "turn_duration", "timestamp": ts,
                     "durationMs": 900}),
    );
    rows.extend(per_request(text, ts, t));
    rows.into_iter().map(|r| r.to_string()).collect()
}

fn count(db: &Database, kind: &str) -> Vec<Value> {
    db.recent_harness_actions_by_room("r1", -1, 100)
        .unwrap()
        .into_iter()
        .filter(|a| a.kind == kind)
        .map(|a| serde_json::from_str(&a.payload).unwrap())
        .collect()
}

#[test]
fn later_prompt_after_restart_is_persisted_once() {
    let dir = TempDir::new().unwrap();
    let jsonl = dir.path().join("session.jsonl");
    let db_path = dir.path().join("test.db");

    let first = prompt_rows(
        1,
        "u1",
        "2026-05-15T21:00:00.000Z",
        "first prompt",
        "Title A",
    );
    let refs: Vec<&str> = first.iter().map(String::as_str).collect();
    append_block(&jsonl, &refs);

    // Backfill.
    drop(make_persisting_adapter(jsonl.clone(), &db_path, "h1", "r1"));
    let db = Database::open(&db_path).unwrap();
    assert_eq!(count(&db, action_kind::USER_PROMPT).len(), 1);
    assert_eq!(count(&db, action_kind::AI_TITLE).len(), 1);
    let total = db
        .recent_harness_actions_by_room("r1", -1, 100)
        .unwrap()
        .len();

    // Restart: nothing new to add.
    let manager = make_persisting_adapter(jsonl.clone(), &db_path, "h1", "r1");
    assert_eq!(
        db.recent_harness_actions_by_room("r1", -1, 100)
            .unwrap()
            .len(),
        total,
        "re-attach must add no rows"
    );

    // Prompt 2 arrives on the live tail.
    let second = prompt_rows(
        2,
        "u2",
        "2026-05-15T21:10:00.000Z",
        "second prompt",
        "Title B",
    );
    let refs: Vec<&str> = second.iter().map(String::as_str).collect();
    append_block(&jsonl, &refs);
    wait_for(
        || count(&db, action_kind::USER_PROMPT).len() >= 2,
        "the live tail to persist prompt 2",
    );
    wait_for(
        || count(&db, action_kind::AI_TITLE).len() >= 2,
        "the live tail to persist title B",
    );
    drop(manager);

    let prompts = count(&db, action_kind::USER_PROMPT);
    assert_eq!(prompts.len(), 2);
    assert!(prompts.iter().any(|p| p["prompt"] == "second prompt"));
    assert_eq!(count(&db, action_kind::AI_TITLE).len(), 2);
}
