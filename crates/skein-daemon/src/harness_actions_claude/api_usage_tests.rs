//! `api_usage` extraction (#341): one row per API response.

use super::api_usage::api_usage_action;
use super::*;
use crate::db::{Database, NewHarnessAction};
use serde_json::{Value, json};
use skein_harness::claude::AssistantRow;
use tempfile::TempDir;

fn chunk(uuid: &str, ts: &str, msg: &str, req: &str, stop: &str, output: u64) -> Value {
    json!({
        "type": "assistant", "uuid": uuid, "timestamp": ts, "requestId": req,
        "message": {
            "id": msg, "model": "claude-opus-4-1", "stop_reason": stop,
            "content": [{"type": "text", "text": "x"}],
            "usage": {"input_tokens": 3, "cache_creation_input_tokens": 100,
                      "cache_read_input_tokens": 50, "output_tokens": output}
        }
    })
}

fn tool_chunk(uuid: &str, ts: &str, output: u64) -> Value {
    let mut v = chunk(uuid, ts, "msg_1", "req_1", "tool_use", output);
    v["message"]["content"] = json!([
        {"type": "tool_use", "id": "toolu_1", "name": "Bash", "input": {"command": "ls"}}
    ]);
    v
}

fn tool_result() -> Value {
    json!({
        "type": "user", "timestamp": "2026-05-15T21:00:05.000Z",
        "toolUseResult": {"stdout": "x"},
        "message": {"content": [
            {"type": "tool_result", "tool_use_id": "toolu_1", "content": "x", "is_error": false}
        ]}
    })
}

fn turn_duration() -> Value {
    json!({"type": "system", "subtype": "turn_duration",
           "timestamp": "2026-05-15T21:00:09.000Z", "durationMs": 9000})
}

/// The first response: three streamed chunks, output 6 -> 6 -> 185.
fn first_response() -> Vec<Value> {
    vec![
        tool_chunk("a1", "2026-05-15T21:00:01.000Z", 6),
        tool_chunk("a2", "2026-05-15T21:00:02.000Z", 6),
        tool_chunk("a3", "2026-05-15T21:00:03.000Z", 185),
        tool_result(),
    ]
}

/// The second: two chunks ending the turn, then the `turn_duration` row.
fn second_response() -> Vec<Value> {
    vec![
        chunk(
            "b1",
            "2026-05-15T21:00:06.000Z",
            "msg_2",
            "req_2",
            "end_turn",
            10,
        ),
        chunk(
            "b2",
            "2026-05-15T21:00:07.000Z",
            "msg_2",
            "req_2",
            "end_turn",
            42,
        ),
        turn_duration(),
    ]
}

fn ingest_all(ex: &mut ActionExtractor, rows: &[Value]) -> Vec<ExtractedAction> {
    rows.iter().flat_map(|r| ex.ingest(r)).collect()
}

fn usage_rows(actions: &[ExtractedAction]) -> Vec<&ExtractedAction> {
    actions
        .iter()
        .filter(|a| a.kind == action_kind::API_USAGE)
        .collect()
}

fn persist(db: &Database, actions: Vec<ExtractedAction>) {
    let rows: Vec<NewHarnessAction> = actions
        .into_iter()
        .map(|a| NewHarnessAction {
            timestamp_ms: a.timestamp_ms,
            kind: a.kind,
            payload: a.payload,
            source: a.source,
        })
        .collect();
    db.record_harness_actions("h1", "r1", Some("claude"), &rows)
        .unwrap();
}

fn stored_usage(db: &Database) -> Vec<crate::db::HarnessAction> {
    db.recent_harness_actions_by_room_and_kind("r1", action_kind::API_USAGE, -1, 100)
        .unwrap()
}

#[test]
fn streamed_responses_yield_one_api_usage_row_each_with_final_usage() {
    let mut ex = ActionExtractor::new();
    let rows: Vec<Value> = [first_response(), second_response()].concat();
    let actions = ingest_all(&mut ex, &rows);
    let usage = usage_rows(&actions);
    // The end_turn response is still pending until the next row arrives
    // (turn_duration closed it above), so both are out.
    assert_eq!(usage.len(), 2);

    let p0: Value = serde_json::from_str(&usage[0].payload).unwrap();
    assert_eq!(p0["usage"]["output"], 185);
    assert_eq!(p0["usage"]["input"], 3);
    assert_eq!(p0["usage"]["cache_creation"], 100);
    assert_eq!(p0["usage"]["cache_read"], 50);
    assert_eq!(p0["stop_reason"], "tool_use");
    assert_eq!(p0["message_id"], "msg_1");
    assert_eq!(p0["request_id"], "req_1");
    assert_eq!(p0["model"], "claude-opus-4-1");
    assert_eq!(usage[0].source.as_deref(), Some("msg_1"));

    let p1: Value = serde_json::from_str(&usage[1].payload).unwrap();
    assert_eq!(p1["usage"]["output"], 42);
    assert_eq!(p1["stop_reason"], "end_turn");
    assert_eq!(usage[1].source.as_deref(), Some("msg_2"));
}

// Pre-existing behaviour, unchanged here: every end_turn chunk repeats the
// terminal stop_reason, so turn_cost fires per chunk (the frontend dedupes by request_id).
#[test]
fn turn_cost_still_fires_per_terminal_chunk_and_never_for_tool_use() {
    let mut ex = ActionExtractor::new();
    let rows: Vec<Value> = [first_response(), second_response()].concat();
    let actions = ingest_all(&mut ex, &rows);
    // Two end_turn chunks are two terminal rows, as before this change.
    let turn_costs = actions
        .iter()
        .filter(|a| a.kind == action_kind::TURN_COST)
        .count();
    assert_eq!(turn_costs, 2);
    assert!(
        actions
            .iter()
            .all(|a| a.kind != action_kind::TURN_COST || a.source.as_deref() != Some("a1")),
        "a tool_use chunk is not a turn_cost"
    );
}

#[test]
fn replaying_the_file_through_extractor_and_db_twice_leaves_two_rows() {
    let dir = TempDir::new().unwrap();
    let db = Database::open(&dir.path().join("t.db")).unwrap();
    let rows: Vec<Value> = [first_response(), second_response()].concat();
    for _ in 0..2 {
        let mut ex = ActionExtractor::new();
        persist(&db, ingest_all(&mut ex, &rows));
    }
    let stored = stored_usage(&db);
    assert_eq!(stored.len(), 2);
    let outputs: Vec<i64> = stored
        .iter()
        .map(|r| {
            serde_json::from_str::<Value>(&r.payload).unwrap()["usage"]["output"]
                .as_i64()
                .unwrap()
        })
        .collect();
    assert!(outputs.contains(&185) && outputs.contains(&42));
}

#[test]
fn a_response_closed_only_in_a_later_tick_is_recorded_once() {
    let dir = TempDir::new().unwrap();
    let db = Database::open(&dir.path().join("t.db")).unwrap();
    let mut ex = ActionExtractor::new();
    // Backfill ends on the response's chunks; nothing may be flushed.
    let mut rows = first_response();
    rows.extend(second_response());
    let (backfill, live) = rows.split_at(5); // ends after b1
    let early = ingest_all(&mut ex, backfill);
    assert_eq!(usage_rows(&early).len(), 1, "msg_2 is still streaming");
    persist(&db, early);
    // The same extractor carries on into the live tail.
    persist(&db, ingest_all(&mut ex, live));
    let stored = stored_usage(&db);
    assert_eq!(stored.len(), 2);
    let last: Value = serde_json::from_str(
        &stored
            .iter()
            .find(|r| r.source.as_deref() == Some("msg_2"))
            .unwrap()
            .payload,
    )
    .unwrap();
    assert_eq!(last["usage"]["output"], 42);
}

#[test]
fn builder_adds_agent_fields_and_falls_back_to_the_dedupe_key() {
    let row = AssistantRow {
        request_id: Some("req_9".into()),
        timestamp_ms: Some(77),
        ..Default::default()
    };
    let a = api_usage_action(&row, Some(("ag1", Some("Explore"))));
    assert_eq!(a.source.as_deref(), Some("req_9"));
    assert_eq!(a.timestamp_ms, 77);
    let p: Value = serde_json::from_str(&a.payload).unwrap();
    assert_eq!(p["agent_id"], "ag1");
    assert_eq!(p["agent_type"], "Explore");
    let none = api_usage_action(&AssistantRow::default(), None);
    assert!(none.source.is_none());
}
