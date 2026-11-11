//! `merge_streams` ordering (#634): table tests on the pure merge, plus
//! end-to-end ticks over real files.

use super::background_tests::{t445_append, t445_live};
use super::merge::{Stream, merge_streams};
use super::resync_tests::t425_tick;
use super::subagent_tests::{BASH_USE, count_ends};
use super::*;
use std::fs;
use tempfile::TempDir;

/// One event per row, labelled by `id`, each row stamped with its ts.
fn stream(rows: &[(&str, Option<i64>)]) -> Stream {
    let mut s = Stream::default();
    for (id, ts) in rows {
        s.events.push(ClaudeEvent::SubagentToolResult {
            agent_id: (*id).to_owned(),
        });
        s.end_row(*ts);
    }
    s
}

/// The merged order as labels, with the timestamp each was sorted under.
fn order(streams: Vec<Stream>) -> Vec<(String, i64)> {
    merge_streams(streams)
        .into_iter()
        .map(|(ts, e)| match e {
            ClaudeEvent::SubagentToolResult { agent_id } => (agent_id, ts),
            other => panic!("unexpected {other:?}"),
        })
        .collect()
}

fn ids(streams: Vec<Stream>) -> Vec<String> {
    order(streams).into_iter().map(|(id, _)| id).collect()
}

#[test]
fn merge_interleaves_streams_by_timestamp() {
    let main = stream(&[("m1", Some(10)), ("m2", Some(30))]);
    let sub = stream(&[("s1", Some(20)), ("s2", Some(40))]);
    assert_eq!(ids(vec![main, sub]), ["m1", "s1", "m2", "s2"]);
}

#[test]
fn merge_never_reorders_within_a_stream() {
    // The second row is stamped earlier than the first: still in file order.
    let main = stream(&[("m1", Some(50)), ("m2", Some(10)), ("m3", Some(60))]);
    let sub = stream(&[("s1", Some(30))]);
    let got = ids(vec![main, sub]);
    let pos = |id: &str| got.iter().position(|g| g == id).unwrap();
    assert!(pos("m1") < pos("m2") && pos("m2") < pos("m3"), "{got:?}");
    assert_eq!(got.len(), 4);
}

#[test]
fn merge_tie_goes_to_the_earlier_stream() {
    let main = stream(&[("m1", Some(10))]);
    let sub_a = stream(&[("a1", Some(10))]);
    let sub_b = stream(&[("b1", Some(10))]);
    assert_eq!(ids(vec![main, sub_a, sub_b]), ["m1", "a1", "b1"]);
}

#[test]
fn merge_missing_timestamp_inherits_the_previous_one() {
    let main = stream(&[("m1", Some(10)), ("m2", None), ("m3", Some(50))]);
    let sub = stream(&[("s1", Some(30))]);
    // m2 inherits 10, so it stays ahead of s1 (30) rather than after it.
    assert_eq!(
        order(vec![main, sub]),
        [
            ("m1".to_owned(), 10),
            ("m2".to_owned(), 10),
            ("s1".to_owned(), 30),
            ("m3".to_owned(), 50),
        ]
    );
}

#[test]
fn merge_leading_untimestamped_takes_the_first_known_timestamp() {
    let main = stream(&[("m1", Some(40))]);
    let sub = stream(&[("s1", None), ("s2", Some(20))]);
    // s1 takes 20 (not "last"), so the subagent's pair precedes m1.
    assert_eq!(
        order(vec![main, sub]),
        [
            ("s1".to_owned(), 20),
            ("s2".to_owned(), 20),
            ("m1".to_owned(), 40),
        ]
    );
}

#[test]
fn merge_fully_untimestamped_stream_sorts_last() {
    let main = stream(&[("m1", None), ("m2", None)]);
    let sub = stream(&[("s1", Some(99))]);
    let got = ids(vec![main, sub]);
    assert_eq!(got, ["s1", "m1", "m2"]);
    // Events pushed with no row at all are also unstamped.
    let bare = Stream::unstamped(vec![ClaudeEvent::SubagentToolResult {
        agent_id: "x".into(),
    }]);
    assert_eq!(ids(vec![bare, stream(&[("s1", Some(1))])]), ["s1", "x"]);
}

const MAIN_USER: &str =
    r#"{"type":"user","timestamp":"2026-01-01T00:00:00.000Z","message":{"content":"go"}}"#;
const SUB_END: &str = r#"{"type":"assistant","isSidechain":true,"agentId":"a1","timestamp":"2026-01-01T00:00:01.000Z","message":{"stop_reason":"end_turn","content":[]}}"#;

fn main_end(ts: &str) -> String {
    format!(
        r#"{{"type":"assistant","timestamp":"{ts}","message":{{"stop_reason":"end_turn","content":[]}}}}"#
    )
}

fn sub_reopen(ts: &str) -> String {
    format!(
        r#"{{"type":"user","isSidechain":true,"agentId":"a1","timestamp":"{ts}","message":{{"role":"user","content":"again"}}}}"#
    )
}

fn is_reopen(e: &ClaudeEvent) -> bool {
    matches!(e, ClaudeEvent::SubagentStart { agent_id, initial: false, .. } if agent_id == "a1")
}

/// Drives `tick` directly (no debouncer), so both appended rows are
/// guaranteed to land in ONE tick: a finished subagent is re-opened and
/// the main session ends its turn in that same tick. Returns that tick's
/// events, in delivery order.
fn reopen_tick(reopen_ts: &str, main_end_ts: &str) -> Vec<ClaudeEvent> {
    let dir = TempDir::new().unwrap();
    let (state, path, sub_dir, _db) = t445_live(&dir, &[MAIN_USER.to_owned()]);
    fs::create_dir_all(&sub_dir).unwrap();
    let sub_path = sub_dir.join("agent-a1.jsonl");
    t445_append(&sub_path, &[BASH_USE.to_owned(), SUB_END.to_owned()]);

    let first = t425_tick(&state);
    assert_eq!(
        count_ends(&first),
        1,
        "subagent must have finished: {first:?}"
    );

    t445_append(&sub_path, &[sub_reopen(reopen_ts)]);
    t445_append(&path, &[main_end(main_end_ts)]);
    t425_tick(&state)
}

#[test]
fn reopen_stamped_before_main_end_of_turn_is_delivered_first() {
    let events = reopen_tick("2026-01-01T00:00:05.000Z", "2026-01-01T00:00:06.000Z");
    let start = events.iter().position(is_reopen);
    let end = events
        .iter()
        .position(|e| matches!(e, ClaudeEvent::AwaitingPrompt));
    let (Some(start), Some(end)) = (start, end) else {
        panic!("expected a re-open and an end of turn, got {events:?}");
    };
    assert!(start < end, "re-open must precede end of turn: {events:?}");
}

#[test]
fn reopen_stamped_after_main_end_of_turn_is_delivered_second() {
    let events = reopen_tick("2026-01-01T00:00:06.000Z", "2026-01-01T00:00:05.000Z");
    let start = events.iter().position(is_reopen);
    let end = events
        .iter()
        .position(|e| matches!(e, ClaudeEvent::AwaitingPrompt));
    let (Some(start), Some(end)) = (start, end) else {
        panic!("expected a re-open and an end of turn, got {events:?}");
    };
    assert!(end < start, "order must follow timestamps: {events:?}");
}
