//! One `user_prompt` per real prompt, one `ai_title` per title change (#723).

use super::*;
use crate::db::action_kind;
use serde_json::{Value, json};

fn human(uuid: &str, ts: &str, text: &str) -> Value {
    json!({
        "type": "user", "uuid": uuid, "promptId": "p", "timestamp": ts,
        "origin": {"kind": "human"},
        "message": {"role": "user", "content": text}
    })
}

fn snapshot(ts: &str) -> Value {
    json!({"type": "attachment", "timestamp": ts, "attachment": {"type": "prompt_snapshot"}})
}

fn last_prompt(text: &str) -> Value {
    json!({"type": "last-prompt", "lastPrompt": text, "leafUuid": "leaf", "sessionId": "s1"})
}

fn title(t: &str) -> Value {
    json!({"type": "ai-title", "aiTitle": t, "sessionId": "s1"})
}

fn tool_chunk(uuid: &str, ts: &str, id: &str) -> Value {
    json!({
        "type": "assistant", "uuid": uuid, "timestamp": ts, "requestId": format!("req_{id}"),
        "message": {
            "id": format!("msg_{id}"), "model": "claude-opus-4-1", "stop_reason": "tool_use",
            "content": [{"type": "tool_use", "id": id, "name": "Bash", "input": {"command": "ls"}}],
            "usage": {"input_tokens": 3, "output_tokens": 5}
        }
    })
}

fn tool_result(ts: &str, id: &str) -> Value {
    json!({
        "type": "user", "timestamp": ts, "promptId": "p",
        "toolUseResult": {"stdout": "x"},
        "message": {"content": [
            {"type": "tool_result", "tool_use_id": id, "content": "x", "is_error": false}
        ]}
    })
}

fn end_turn(uuid: &str, ts: &str, id: &str) -> Value {
    json!({
        "type": "assistant", "uuid": uuid, "timestamp": ts, "requestId": format!("req_{id}"),
        "message": {
            "id": format!("msg_{id}"), "model": "claude-opus-4-1", "stop_reason": "end_turn",
            "content": [{"type": "text", "text": "done"}],
            "usage": {"input_tokens": 3, "output_tokens": 7}
        }
    })
}

fn turn_duration(ts: &str) -> Value {
    json!({"type": "system", "subtype": "turn_duration", "timestamp": ts, "durationMs": 900})
}

/// The rows Claude writes around each API response. `modern` adds the
/// 2.1.322+ `api-request` and `prompt_snapshot` rows.
fn per_request(modern: bool, text: &str, ts: &str, t: &str) -> Vec<Value> {
    let mut v = Vec::new();
    if modern {
        v.push(json!({"type": "api-request"}));
        v.push(snapshot(ts));
    }
    v.push(last_prompt(text));
    v.push(title(t));
    v
}

fn transcript(modern: bool) -> Vec<Value> {
    let (t1, t2) = ("2026-05-15T21:00:00.000Z", "2026-05-15T21:10:00.000Z");
    let mut rows = vec![human("u1", t1, "first prompt")];
    rows.extend(per_request(modern, "first prompt", t1, "Title A"));
    rows.push(tool_chunk("a1", "2026-05-15T21:00:01.000Z", "t1"));
    rows.push(tool_result("2026-05-15T21:00:02.000Z", "t1"));
    rows.extend(per_request(modern, "first prompt", t1, "Title A"));
    rows.push(end_turn("a2", "2026-05-15T21:00:03.000Z", "1"));
    rows.push(turn_duration("2026-05-15T21:00:04.000Z"));
    rows.extend(per_request(modern, "first prompt", t1, "Title A"));
    rows.push(human("u2", t2, "second prompt"));
    rows.extend(per_request(modern, "second prompt", t2, "Title B"));
    rows.push(tool_chunk("b1", "2026-05-15T21:10:01.000Z", "t2"));
    rows.push(tool_result("2026-05-15T21:10:02.000Z", "t2"));
    rows.extend(per_request(modern, "second prompt", t2, "Title B"));
    rows.push(end_turn("b2", "2026-05-15T21:10:03.000Z", "2"));
    rows.push(turn_duration("2026-05-15T21:10:04.000Z"));
    rows.extend(per_request(modern, "second prompt", t2, "Title B"));
    rows
}

fn run(rows: &[Value]) -> Vec<ExtractedAction> {
    let mut ex = ActionExtractor::new();
    rows.iter().flat_map(|r| ex.ingest(r)).collect()
}

fn of_kind<'a>(a: &'a [ExtractedAction], kind: &str) -> Vec<&'a ExtractedAction> {
    a.iter().filter(|x| x.kind == kind).collect()
}

fn payload(a: &ExtractedAction) -> Value {
    serde_json::from_str(&a.payload).unwrap()
}

fn assert_two_prompts(modern: bool) {
    let actions = run(&transcript(modern));
    let prompts = of_kind(&actions, action_kind::USER_PROMPT);
    assert_eq!(prompts.len(), 2);
    assert_eq!(payload(prompts[0])["prompt"], "first prompt");
    assert_eq!(payload(prompts[1])["prompt"], "second prompt");
    assert_eq!(prompts[0].source.as_deref(), Some("u1"));
    assert_eq!(prompts[1].source.as_deref(), Some("u2"));
    assert_eq!(
        prompts[0].timestamp_ms,
        parse_timestamp_ms(&human("u1", "2026-05-15T21:00:00.000Z", ""))
    );
    assert_eq!(
        prompts[1].timestamp_ms,
        parse_timestamp_ms(&human("u2", "2026-05-15T21:10:00.000Z", ""))
    );
    let titles = of_kind(&actions, action_kind::AI_TITLE);
    assert_eq!(titles.len(), 2);
    assert_eq!(payload(titles[0])["ai_title"], "Title A");
    assert_eq!(payload(titles[1])["ai_title"], "Title B");
    assert_eq!(of_kind(&actions, action_kind::TOOL_CALL).len(), 2);
}

#[test]
fn modern_shape_yields_one_prompt_and_one_title_per_change() {
    assert_two_prompts(true);
}

#[test]
fn older_shape_yields_one_prompt_and_one_title_per_change() {
    assert_two_prompts(false);
}

#[test]
fn only_real_prompts_count() {
    let ts = "2026-05-15T21:00:00.000Z";
    let rows = vec![
        json!({"type": "user", "uuid": "m1", "timestamp": ts, "isMeta": true,
               "message": {"role": "user", "content": "<system-reminder>x</system-reminder>"}}),
        json!({"type": "user", "uuid": "n1", "timestamp": ts,
               "origin": {"kind": "task-notification"},
               "message": {"role": "user", "content": "<task-notification>done</task-notification>"}}),
        json!({"type": "user", "uuid": "pe1", "timestamp": ts,
               "origin": {"kind": "peer"},
               "message": {"role": "user", "content": "peer hand-back"}}),
        json!({"type": "user", "uuid": "l1", "timestamp": ts,
               "message": {"role": "user",
                           "content": "<local-command-stdout>ok</local-command-stdout>"}}),
    ];
    assert!(of_kind(&run(&rows), action_kind::USER_PROMPT).is_empty());

    let cross = json!({"type": "user", "uuid": "x1", "timestamp": ts,
        "message": {"role": "user",
                    "content": "Another Claude session sent a message: hello"}});
    let actions = run(&[cross]);
    assert_eq!(of_kind(&actions, action_kind::USER_PROMPT).len(), 1);
}

#[test]
fn null_title_between_equal_titles_emits_nothing_extra() {
    let rows = vec![
        title("Title A"),
        json!({"type": "ai-title", "aiTitle": null, "sessionId": "s1"}),
        title("Title A"),
    ];
    let actions = run(&rows);
    let titles = of_kind(&actions, action_kind::AI_TITLE);
    assert_eq!(titles.len(), 1);
    assert_eq!(payload(titles[0])["ai_title"], "Title A");
}
