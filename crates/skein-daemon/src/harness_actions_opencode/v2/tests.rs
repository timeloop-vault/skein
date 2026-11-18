//! The 2.3.2 captures of 2026-11-18 (skein-harness `fixtures/`) are the
//! contract: the live frames and the stored rows of one session must give
//! the same rows.

use serde_json::Value;
use skein_harness::opencode::v2::store::{StoredMessage, parse_message};
use skein_harness::opencode::v2::{V2Action, drain_sse_data, parse_action_event};

use super::*;

const DIR: &str = "../../../../skein-harness/src/opencode/v2/fixtures";
const SSE_B: &str = include_str!("../../../../skein-harness/src/opencode/v2/fixtures/store-B.sse");
const SSE_C: &str = include_str!("../../../../skein-harness/src/opencode/v2/fixtures/store-C.sse");
const SSE_D: &str = include_str!("../../../../skein-harness/src/opencode/v2/fixtures/store-D.sse");
const SSE_E: &str = include_str!("../../../../skein-harness/src/opencode/v2/fixtures/store-E.sse");
const SSE_F: &str = include_str!("../../../../skein-harness/src/opencode/v2/fixtures/store-F.sse");
const MSG_B: &str =
    include_str!("../../../../skein-harness/src/opencode/v2/fixtures/store/messages_B.jsonl");
const MSG_C: &str =
    include_str!("../../../../skein-harness/src/opencode/v2/fixtures/store/messages_C.jsonl");
const MSG_D: &str =
    include_str!("../../../../skein-harness/src/opencode/v2/fixtures/store/messages_D.jsonl");
const MSG_E: &str =
    include_str!("../../../../skein-harness/src/opencode/v2/fixtures/store/messages_E.jsonl");
const MSG_F: &str =
    include_str!("../../../../skein-harness/src/opencode/v2/fixtures/store/messages_F.jsonl");

fn frames(raw: &str) -> Vec<V2Action> {
    let mut buf = raw.as_bytes().to_vec();
    drain_sse_data(&mut buf)
        .iter()
        .filter_map(|p| parse_action_event(p))
        .collect()
}

fn live_rows(raw: &str) -> Vec<ExtractedAction> {
    let mut t = V2ActionTracker::default();
    frames(raw).iter().flat_map(|a| t.observe(a)).collect()
}

fn stored(raw: &str) -> Vec<StoredMessage> {
    raw.lines()
        .map(|l| {
            let v: Value = serde_json::from_str(l).unwrap();
            StoredMessage {
                id: v["id"].as_str().unwrap().to_owned(),
                seq: v["seq"].as_i64().unwrap(),
                time_created: v["time_created"].as_i64().unwrap(),
                kind: parse_message(v["type"].as_str().unwrap(), &v["data"]),
            }
        })
        .collect()
}

fn stored_rows(raw: &str) -> Vec<ExtractedAction> {
    rows_from_stored(&stored(raw))
}

fn payload(r: &ExtractedAction) -> Value {
    serde_json::from_str(&r.payload).unwrap()
}

fn by_source<'a>(rows: &'a [ExtractedAction], source: &str) -> &'a ExtractedAction {
    rows.iter()
        .find(|r| r.source.as_deref() == Some(source))
        .unwrap_or_else(|| panic!("no row {source}"))
}

fn with_tool<'a>(rows: &'a [ExtractedAction], tool: &str) -> &'a ExtractedAction {
    rows.iter()
        .find(|r| payload(r)["tool"] == tool)
        .unwrap_or_else(|| panic!("no {tool} row"))
}

/// (kind, source, payload), sorted by source. A set, not a sequence: live is
/// chronological (a tool result can land after the step's reasoning of the
/// next ordinal), stored groups a step's content in item order and puts the
/// step's `turn_cost` last; the identity is the source, not the position.
fn identity(rows: &[ExtractedAction]) -> Vec<(String, String, Value)> {
    let mut v: Vec<_> = rows
        .iter()
        .map(|r| {
            (
                r.kind.to_owned(),
                r.source.clone().expect("every row has a source"),
                payload(r),
            )
        })
        .collect();
    v.sort_by(|a, b| a.1.cmp(&b.1));
    v
}

fn assert_paths_agree(sse: &str, msgs: &str) {
    let live = live_rows(sse);
    let stored = stored_rows(msgs);
    assert!(!live.is_empty());
    assert_eq!(identity(&live), identity(&stored));
    let sources: std::collections::HashSet<_> = live.iter().map(|r| &r.source).collect();
    assert_eq!(sources.len(), live.len(), "sources are unique");
}

#[test]
fn live_and_stored_agree_b() {
    assert_paths_agree(SSE_B, MSG_B);
}

#[test]
fn live_and_stored_agree_c() {
    assert_paths_agree(SSE_C, MSG_C);
}

#[test]
fn live_and_stored_agree_d() {
    assert_paths_agree(SSE_D, MSG_D);
}

#[test]
fn live_and_stored_agree_e() {
    assert_paths_agree(SSE_E, MSG_E);
}

#[test]
fn live_and_stored_timestamps_are_reported() {
    // The two paths time the same row from different clocks (the frame's
    // `created` vs the stored item's own time). Print the deltas so the
    // finding is visible with `--nocapture`, and bound them.
    let mut worst = 0;
    for (sse, msgs) in [
        (SSE_B, MSG_B),
        (SSE_C, MSG_C),
        (SSE_D, MSG_D),
        (SSE_E, MSG_E),
    ] {
        let live = live_rows(sse);
        for s in stored_rows(msgs) {
            let l = by_source(&live, s.source.as_deref().unwrap());
            let d = l.timestamp_ms - s.timestamp_ms;
            println!("{} {} live-stored = {d} ms", s.kind, s.source.unwrap());
            worst = worst.max(d.abs());
        }
    }
    println!("worst |live - stored| = {worst} ms");
    assert!(worst < 1000);
}

#[test]
fn b_shell_and_read_rows() {
    let rows = live_rows(SSE_B);
    let shell = with_tool(&rows, "shell");
    assert_eq!(shell.kind, action_kind::TOOL_CALL);
    let p = payload(shell);
    assert!(p["input"]["command"].as_str().unwrap().contains("echo"));
    assert!(p["result"].is_string());
    assert_eq!(p["is_error"], false);
    assert!(p["metadata"].is_object() || p["metadata"].is_null());
    assert!(shell.source.as_deref().unwrap().starts_with("call_"));
    let read = with_tool(&rows, "read");
    assert_eq!(read.kind, action_kind::TOOL_CALL);
    assert_eq!(payload(read)["input"]["path"], "notes.txt");
}

#[test]
fn c_edit_and_write_rows() {
    for rows in [live_rows(SSE_C), stored_rows(MSG_C)] {
        let edit = with_tool(&rows, "edit");
        assert_eq!(edit.kind, action_kind::PATCH);
        let p = payload(edit);
        assert_eq!(p["files"], serde_json::json!(["a.txt"]));
        assert_eq!(p["patch_info"]["additions"], 1);
        assert_eq!(p["patch_info"]["deletions"], 1);
        assert!(
            p["patch_info"]["diff"]
                .as_str()
                .unwrap()
                .contains("Index: a.txt")
        );
        let write = with_tool(&rows, "write");
        assert_eq!(write.kind, action_kind::PATCH);
        let w = payload(write);
        assert_eq!(w["files"], serde_json::json!(["new.txt"]));
        assert!(w.get("patch_info").is_none(), "no diff for a write, as v1");
    }
}

#[test]
fn c_reasoning_row() {
    let rows = live_rows(SSE_C);
    let r = rows
        .iter()
        .find(|r| r.kind == action_kind::REASONING)
        .unwrap();
    assert_eq!(payload(r)["text"], "Let me check a.txt exists.");
    assert!(r.source.as_deref().unwrap().ends_with(":r0"));
    assert!(
        stored_rows(MSG_D)
            .iter()
            .any(|r| r.kind == action_kind::REASONING)
    );
}

#[test]
fn e_failed_row() {
    for rows in [live_rows(SSE_E), stored_rows(MSG_E)] {
        let failed: Vec<_> = rows
            .iter()
            .filter(|r| payload(r)["is_error"] == true)
            .collect();
        assert_eq!(failed.len(), 1);
        let p = payload(failed[0]);
        assert!(p["error"].as_str().is_some_and(|e| !e.is_empty()));
        assert_ne!(p["tool"], "unknown");
    }
}

#[test]
fn turn_costs_carry_reason_and_token_keys() {
    let rows = live_rows(SSE_B);
    let costs: Vec<Value> = rows
        .iter()
        .filter(|r| r.kind == action_kind::TURN_COST)
        .map(payload)
        .collect();
    assert_eq!(costs.len(), 2);
    assert_eq!(costs[0]["reason"], "tool-calls");
    assert_eq!(costs[1]["reason"], "stop");
    for c in &costs {
        let t = &c["tokens"];
        for k in ["input", "output", "reasoning"] {
            assert!(t[k].is_u64(), "tokens.{k}");
        }
        assert!(t["cache"]["read"].is_u64() && t["cache"]["write"].is_u64());
        assert!(c["cost"].is_number());
    }
}

#[test]
fn user_prompt_row() {
    let live = live_rows(SSE_B);
    let u = live
        .iter()
        .find(|r| r.kind == action_kind::USER_PROMPT)
        .unwrap();
    assert!(
        payload(u)["prompt"]
            .as_str()
            .unwrap()
            .starts_with("Read the file notes.txt")
    );
    assert!(u.source.as_deref().unwrap().starts_with("msg_"));
}

#[test]
fn f_subagent_row_and_synthetic_gives_nothing() {
    let live = live_rows(SSE_F);
    let sub = with_tool(&live, "subagent");
    assert_eq!(sub.kind, action_kind::TOOL_CALL);
    let msgs = stored(MSG_F);
    assert!(
        msgs.iter()
            .any(|m| matches!(m.kind, MessageKind::Synthetic(_)))
    );
    let synthetic_only: Vec<_> = msgs
        .into_iter()
        .filter(|m| matches!(m.kind, MessageKind::Synthetic(_) | MessageKind::Idle { .. }))
        .collect();
    assert!(rows_from_stored(&synthetic_only).is_empty());
    assert_eq!(
        stored_rows(MSG_F)
            .iter()
            .filter(|r| payload(r)["tool"] == "subagent")
            .count(),
        1
    );
}

#[test]
fn f_frames_come_from_two_sessions() {
    // The tracker takes no session filter; the caller reads `session_id`.
    let sessions: std::collections::HashSet<_> =
        frames(SSE_F).into_iter().map(|a| a.session_id).collect();
    assert_eq!(sessions.len(), 2, "root and child, from {DIR}");
}

fn action(created: i64, kind: V2ActionKind) -> V2Action {
    V2Action {
        session_id: "s".into(),
        assistant_message_id: Some("m".into()),
        created,
        kind,
    }
}

fn started(id: &str) -> V2Action {
    action(
        1,
        V2ActionKind::ToolInputStarted {
            call_id: id.into(),
            name: "read".into(),
        },
    )
}

fn failed(id: &str) -> V2Action {
    action(
        5,
        V2ActionKind::ToolFailed {
            call_id: id.into(),
            error: None,
        },
    )
}

#[test]
fn result_for_a_call_never_seen_start_gives_no_row() {
    let mut t = V2ActionTracker::default();
    assert!(t.observe(&failed("call_x")).is_empty());
    assert!(t.pending.is_empty() && t.order.is_empty());
}

#[test]
fn named_call_without_input_still_gives_a_row() {
    let mut t = V2ActionTracker::default();
    t.observe(&started("call_y"));
    let rows = t.observe(&failed("call_y"));
    assert_eq!(rows.len(), 1);
    let p = payload(&rows[0]);
    assert_eq!(p["tool"], "read");
    assert!(p["input"].is_null());
    assert_eq!(rows[0].source.as_deref(), Some("call_y"));
}

#[test]
fn pending_cap_evicts_only_the_oldest() {
    let mut t = V2ActionTracker::default();
    for i in 0..PENDING_CAP + 10 {
        t.observe(&started(&format!("c{i}")));
    }
    assert_eq!(t.pending.len(), PENDING_CAP);
    assert_eq!(t.order.len(), PENDING_CAP);
    let newest = format!("c{}", PENDING_CAP + 9);
    let rows = t.observe(&failed(&newest));
    assert_eq!(rows.len(), 1);
    assert_eq!(payload(&rows[0])["tool"], "read");
    // The oldest were evicted: no name, no row.
    assert!(t.observe(&failed("c0")).is_empty());
    assert!(t.pending.len() < PENDING_CAP);
}

#[test]
fn in_flight_step_gives_its_tool_but_no_turn_cost() {
    let data = serde_json::json!({
        "time": {"created": 10},
        "content": [{
            "type": "tool", "id": "call_h", "name": "read", "executed": false,
            "state": {"status": "completed", "input": {"path": "a"},
                "content": [{"type": "text", "text": "x"}], "metadata": {}},
            "time": {"created": 11, "completed": 12}
        }, {"type": "reasoning", "text": "half", "time": {"created": 13}}]
    });
    let msgs = vec![StoredMessage {
        id: "m".into(),
        seq: 1,
        time_created: 10,
        kind: parse_message("assistant", &data),
    }];
    let rows = rows_from_stored(&msgs);
    assert_eq!(rows.len(), 1, "only the finished tool");
    assert_eq!(rows[0].source.as_deref(), Some("call_h"));
}
