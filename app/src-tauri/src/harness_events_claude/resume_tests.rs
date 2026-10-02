//! #336: a transcript that ends mid-turn, replayed on attach. A fresh
//! process (right after `pty_spawn`) cannot be mid-turn, so the replay
//! must read `AwaitingPrompt`; any other attach keeps the transcript's
//! own verdict, since a live process may be mid long tool call.

use super::tests::{drain, drain_brief, test_manager};
use super::*;
use std::fs;
use std::io::Write;
use std::path::Path;
use std::sync::mpsc;
use tempfile::TempDir;

const USER_PROMPT: &str = r#"{"type":"user","sessionId":"x","message":{"role":"user","content":[{"type":"text","text":"list files"}]}}"#;
const TOOL_USE: &str = r#"{"type":"assistant","sessionId":"x","message":{"role":"assistant","stop_reason":"tool_use","content":[{"type":"tool_use","id":"toolu_1","name":"Bash","input":{"command":"ls"}}]}}"#;
const TOOL_RESULT: &str = r#"{"type":"user","sessionId":"x","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_1","content":"a.txt"}]}}"#;
const END_TURN: &str = r#"{"type":"assistant","sessionId":"x","message":{"role":"assistant","stop_reason":"end_turn","content":[{"type":"text","text":"done"}]}}"#;

fn write_rows(path: &Path, rows: &[&str]) {
    let mut f = fs::File::create(path).unwrap();
    for row in rows {
        writeln!(f, "{row}").unwrap();
    }
    f.sync_all().unwrap();
}

fn append_row(path: &Path, row: &str) {
    let mut f = fs::OpenOptions::new().append(true).open(path).unwrap();
    writeln!(f, "{row}").unwrap();
    f.sync_all().unwrap();
}

/// Attach to a transcript holding `rows`, returning what the attach emitted.
fn attach(
    rows: &[&str],
    fresh_process: bool,
) -> (
    TempDir,
    std::path::PathBuf,
    ClaudeEventsManager,
    mpsc::Receiver<ClaudeEvent>,
) {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("session.jsonl");
    if !rows.is_empty() {
        write_rows(&path, rows);
    }
    let (tx, rx) = mpsc::channel();
    let manager = test_manager();
    manager
        .attach_at_with(
            "harness-1",
            path.clone(),
            move |event| {
                let _ = tx.send(event);
            },
            None,
            fresh_process,
        )
        .unwrap();
    (dir, path, manager, rx)
}

fn has(events: &[ClaudeEvent], f: impl Fn(&ClaudeEvent) -> bool) -> bool {
    events.iter().any(f)
}

#[test]
fn fresh_attach_on_tool_use_tail_is_awaiting_prompt() {
    let (_d, _p, _m, rx) = attach(&[USER_PROMPT, TOOL_USE], true);
    let events = drain(&rx);
    assert!(has(&events, |e| matches!(e, ClaudeEvent::AwaitingPrompt)));
    assert!(!has(&events, |e| matches!(e, ClaudeEvent::AssistantTurn)));
}

#[test]
fn fresh_attach_on_tool_result_tail_is_awaiting_prompt() {
    let (_d, _p, _m, rx) = attach(&[USER_PROMPT, TOOL_USE, TOOL_RESULT], true);
    let events = drain(&rx);
    assert!(has(&events, |e| matches!(e, ClaudeEvent::AwaitingPrompt)));
    assert!(!has(&events, |e| matches!(
        e,
        ClaudeEvent::ToolUseResult | ClaudeEvent::AssistantTurn
    )));
}

#[test]
fn fresh_attach_on_user_prompt_tail_is_awaiting_prompt() {
    let (_d, _p, _m, rx) = attach(&[USER_PROMPT], true);
    let events = drain(&rx);
    assert!(has(&events, |e| matches!(e, ClaudeEvent::AwaitingPrompt)));
    assert!(!has(&events, |e| matches!(
        e,
        ClaudeEvent::UserPrompt { .. }
    )));
}

/// The hard constraint: an attach to a process that is not fresh keeps
/// the transcript's verdict, so a live long tool call stays running.
#[test]
fn non_fresh_attach_on_tool_use_tail_stays_assistant_turn() {
    let (_d, _p, _m, rx) = attach(&[USER_PROMPT, TOOL_USE], false);
    let events = drain(&rx);
    assert!(has(&events, |e| matches!(e, ClaudeEvent::AssistantTurn)));
    assert!(!has(&events, |e| matches!(e, ClaudeEvent::AwaitingPrompt)));
}

#[test]
fn live_rows_after_a_fresh_attach_are_not_rewritten() {
    let (_d, path, _m, rx) = attach(&[USER_PROMPT, TOOL_USE], true);
    let first = drain(&rx);
    assert!(has(&first, |e| matches!(e, ClaudeEvent::AwaitingPrompt)));

    append_row(&path, TOOL_USE);
    let live = drain(&rx);
    assert!(
        has(&live, |e| matches!(e, ClaudeEvent::AssistantTurn)),
        "a live tool_use row must still read as a turn, got {live:?}"
    );
}

/// A manual reattach of a fresh-attached adapter re-runs the history scan
/// but must not apply the fresh-process rule: the process has been alive.
#[test]
fn reattach_of_a_fresh_attached_adapter_keeps_the_transcript_verdict() {
    let (_d, path, mgr, rx) = attach(&[USER_PROMPT, TOOL_USE], true);
    drain(&rx);

    mgr.simulate_dead_watch_for_test("harness-1");
    append_row(&path, TOOL_RESULT);

    let outcome = mgr.reattach("harness-1").unwrap();
    assert_eq!(outcome, ReattachOutcome::Reattached);
    let events = drain_brief(&rx);
    assert!(
        !has(&events, |e| matches!(e, ClaudeEvent::AwaitingPrompt)),
        "reattach must not apply the #336 rule, got {events:?}"
    );
    assert!(
        has(&events, |e| matches!(
            e,
            ClaudeEvent::UserPrompt { .. }
                | ClaudeEvent::ToolUseResult
                | ClaudeEvent::AssistantTurn
        )),
        "reattach replays the transcript's own non-terminal verdict, got {events:?}"
    );
}

#[test]
fn fresh_attach_on_clean_end_turn_tail_is_awaiting_prompt() {
    let (_d, _p, _m, rx) = attach(&[USER_PROMPT, END_TURN], true);
    let events = drain(&rx);
    assert!(has(&events, |e| matches!(e, ClaudeEvent::AwaitingPrompt)));
}

#[test]
fn fresh_attach_on_a_missing_transcript_emits_no_initial_event() {
    let (_d, _p, _m, rx) = attach(&[], true);
    let events = drain_brief(&rx);
    assert!(events.is_empty(), "got {events:?}");
}
