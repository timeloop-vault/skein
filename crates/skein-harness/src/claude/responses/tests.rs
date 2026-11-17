use super::*;
use crate::claude::{parse_row, summarize};
use serde_json::json;
use std::io::Write;

fn asst(mid: &str, rid: &str, stop: Option<&str>, out: u64, ts: &str) -> Row {
    let v = json!({
        "type": "assistant", "uuid": format!("u-{out}"), "sessionId": "s1",
        "requestId": rid, "timestamp": ts,
        "message": {"id": mid, "model": "claude-opus", "stop_reason": stop, "usage": {
            "input_tokens": 3, "cache_creation_input_tokens": 100,
            "cache_read_input_tokens": 50, "output_tokens": out}}
    });
    parse_row(&v.to_string()).unwrap()
}

fn keyless(out: u64) -> Row {
    let v = json!({"type": "assistant", "message": {"model": "m",
        "usage": {"output_tokens": out}}});
    parse_row(&v.to_string()).unwrap()
}

fn other(ty: &str) -> Row {
    parse_row(&json!({"type": ty}).to_string()).unwrap()
}

fn tool_result() -> Row {
    parse_row(&json!({"type": "user", "toolUseResult": {"x": 1}}).to_string()).unwrap()
}

const T: &str = "2026-01-01T00:00:00.000Z";

#[test]
fn growing_output_chunks_fold_to_the_final_count() {
    let rows = vec![
        asst("m1", "r1", Some("end_turn"), 6, T),
        asst("m1", "r1", Some("end_turn"), 6, T),
        asst("m1", "r1", Some("end_turn"), 185, T),
    ];
    let out = fold_responses(rows);
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].usage.output, 185);
    assert_eq!(out[0].usage.cache_creation, 100);
    assert_eq!(out[0].usage.input, 3);
}

#[test]
fn repeated_final_output_stays_put() {
    let rows = (0..3).map(|_| asst("m1", "r1", Some("end_turn"), 185, T));
    let out = fold_responses(rows);
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].usage.output, 185);
}

#[test]
fn tool_use_then_end_turn_make_two_responses() {
    let rows = vec![
        asst("m1", "r1", Some("tool_use"), 10, T),
        asst("m1", "r1", Some("tool_use"), 10, T),
        tool_result(),
        asst("m2", "r2", Some("end_turn"), 20, T),
        other("system"),
    ];
    let out = fold_responses(rows);
    assert_eq!(out.len(), 2);
    assert_eq!(out[0].stop_reason.as_deref(), Some("tool_use"));
    assert_eq!(out[1].stop_reason.as_deref(), Some("end_turn"));
}

#[test]
fn push_returns_a_response_exactly_when_the_next_row_arrives() {
    let mut fold = ResponseFold::default();
    assert!(
        fold.push(&asst("m1", "r1", Some("tool_use"), 5, T))
            .is_none()
    );
    assert!(
        fold.push(&asst("m1", "r1", Some("tool_use"), 9, T))
            .is_none()
    );
    let closed = fold.push(&tool_result()).expect("closed by tool_result");
    assert_eq!(closed.usage.output, 9);
    assert!(fold.push(&other("system")).is_none());
    assert!(fold.finish().is_none());
}

#[test]
fn finish_returns_a_trailing_response_and_fold_stays_reusable() {
    let mut fold = ResponseFold::default();
    assert!(
        fold.push(&asst("m1", "r1", Some("end_turn"), 5, T))
            .is_none()
    );
    assert_eq!(fold.finish().unwrap().usage.output, 5);
    // A straggler of the emitted key is still dropped.
    assert!(
        fold.push(&asst("m1", "r1", Some("end_turn"), 5, T))
            .is_none()
    );
    assert!(fold.finish().is_none());
    assert!(fold.push(&asst("m2", "r2", None, 7, T)).is_none());
    assert_eq!(fold.finish().unwrap().usage.output, 7);
}

#[test]
fn non_adjacent_straggler_is_dropped() {
    let rows = vec![
        asst("m1", "r1", Some("tool_use"), 10, T),
        tool_result(),
        asst("m2", "r2", Some("end_turn"), 20, T),
        asst("m1", "r1", Some("tool_use"), 10, T),
    ];
    let out = fold_responses(rows);
    assert_eq!(out.len(), 2);
}

#[test]
fn keyless_rows_each_count() {
    let out = fold_responses(vec![keyless(1), keyless(2), keyless(3)]);
    assert_eq!(out.len(), 3);
}

#[test]
fn merge_keeps_first_identity_last_stop_and_latest_timestamp() {
    let first = asst("m1", "r1", None, 1, "2026-01-01T00:00:00.000Z");
    let last = asst("m1", "r1", Some("end_turn"), 2, "2026-01-01T00:00:05.000Z");
    let (Row::Assistant(a), Row::Assistant(b)) = (&first, &last) else {
        unreachable!()
    };
    let out = fold_responses(vec![first.clone(), last.clone()]);
    assert_eq!(out[0].uuid, a.uuid);
    assert_eq!(out[0].stop_reason.as_deref(), Some("end_turn"));
    assert_eq!(out[0].timestamp_ms, b.timestamp_ms);
}

#[test]
fn summarize_reports_final_output() {
    let s = summarize(vec![
        asst("m1", "r1", Some("end_turn"), 6, T),
        asst("m1", "r1", Some("end_turn"), 6, T),
        asst("m1", "r1", Some("end_turn"), 185, T),
        other("system"),
    ]);
    assert_eq!(s.usage_by_model["claude-opus"].output, 185);
    assert_eq!(s.usage_by_model["claude-opus"].cache_creation, 100);
    assert_eq!(s.responses_by_model["claude-opus"], 1);
}

#[test]
fn responses_reads_a_file_with_a_trailing_response() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("t.jsonl");
    let mut f = std::fs::File::create(&path).unwrap();
    for v in [
        json!({"type": "assistant", "requestId": "r1", "message": {"id": "m1",
            "usage": {"output_tokens": 6}}}),
        json!({"type": "assistant", "requestId": "r1", "message": {"id": "m1",
            "usage": {"output_tokens": 185}}}),
    ] {
        writeln!(f, "{v}").unwrap();
    }
    drop(f);
    let out: Vec<_> = responses(&path).unwrap().collect();
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].usage.output, 185);
}
