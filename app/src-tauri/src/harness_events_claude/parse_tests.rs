//! Row to event translation and the initial-state read of an existing transcript.

use super::background_tasks::BackgroundState;
use super::parse::{determine_initial_state, parse_value};
use super::persist::scan_history;
use super::resync_tests::{
    T425_END, T425_PROMPT, T425_RESULT, T425_TOOL, t425_jsonl, t425_tick, t425_vanished,
};
use super::tests::{drain, drain_brief, make_adapter};
use super::*;
use skein_harness::claude::local_command::LocalCommandTracker;
use std::fs;
use std::io::Write;
use tempfile::TempDir;

#[test]
fn awaiting_prompt_emitted_for_assistant_end_turn() {
    let dir = TempDir::new().unwrap();
    let (_mgr, path, rx) = make_adapter(&dir);

    let mut f = fs::File::create(&path).unwrap();
    writeln!(
        f,
        r#"{{"type":"assistant","sessionId":"x","message":{{"stop_reason":"end_turn","content":[{{"type":"text","text":"done"}}]}}}}"#
    )
    .unwrap();
    f.sync_all().unwrap();

    let events = drain(&rx);
    assert!(
        events
            .iter()
            .any(|e| matches!(e, ClaudeEvent::AwaitingPrompt)),
        "expected AwaitingPrompt, got {events:?}"
    );
}

#[test]
fn awaiting_prompt_emitted_for_assistant_stop_sequence() {
    let dir = TempDir::new().unwrap();
    let (_mgr, path, rx) = make_adapter(&dir);

    let mut f = fs::File::create(&path).unwrap();
    writeln!(
        f,
        r#"{{"type":"assistant","sessionId":"x","message":{{"stop_reason":"stop_sequence","content":[]}}}}"#
    )
    .unwrap();
    f.sync_all().unwrap();

    let events = drain(&rx);
    assert!(
        events
            .iter()
            .any(|e| matches!(e, ClaudeEvent::AwaitingPrompt)),
        "expected AwaitingPrompt, got {events:?}"
    );
}

#[test]
fn last_prompt_row_does_not_emit_awaiting_prompt() {
    // `last-prompt` fires when Claude captures the user's new
    // prompt at the START of a turn — opposite of awaiting.
    // The first L2c-1 cut treated it as "done" which kept the
    // dot green until L2a's 8s idle timer fired.
    let dir = TempDir::new().unwrap();
    let (_mgr, path, rx) = make_adapter(&dir);

    let mut f = fs::File::create(&path).unwrap();
    writeln!(
        f,
        r#"{{"type":"last-prompt","sessionId":"x","lastPrompt":"hi","leafUuid":"u"}}"#
    )
    .unwrap();
    f.sync_all().unwrap();

    let events = drain_brief(&rx);
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, ClaudeEvent::AwaitingPrompt)),
        "last-prompt must not emit AwaitingPrompt, got {events:?}"
    );
}

#[test]
fn streamed_assistant_rows_coalesce_to_one_turn_event() {
    let dir = TempDir::new().unwrap();
    let (_mgr, path, rx) = make_adapter(&dir);

    let mut f = fs::File::create(&path).unwrap();
    // Two assistant rows with non-terminal stop_reason (tool_use)
    // followed by a terminal end_turn row. The first two should
    // coalesce to a single AssistantTurn; the third triggers
    // AwaitingPrompt.
    for _ in 0..2 {
        writeln!(
            f,
            r#"{{"type":"assistant","sessionId":"x","message":{{"stop_reason":"tool_use","content":[{{"type":"text","text":"hi"}}]}}}}"#
        )
        .unwrap();
    }
    writeln!(
        f,
        r#"{{"type":"assistant","sessionId":"x","message":{{"stop_reason":"end_turn","content":[{{"type":"text","text":"done"}}]}}}}"#
    )
    .unwrap();
    f.sync_all().unwrap();

    let events = drain(&rx);
    let turn_count = events
        .iter()
        .filter(|e| matches!(e, ClaudeEvent::AssistantTurn))
        .count();
    assert_eq!(turn_count, 1, "should coalesce, got {events:?}");
    assert!(
        events
            .iter()
            .any(|e| matches!(e, ClaudeEvent::AwaitingPrompt)),
        "expected AwaitingPrompt, got {events:?}"
    );
}

#[test]
fn tool_use_block_emits_tool_use_start() {
    let dir = TempDir::new().unwrap();
    let (_mgr, path, rx) = make_adapter(&dir);

    let mut f = fs::File::create(&path).unwrap();
    // First non-terminal assistant row → AssistantTurn.
    writeln!(
        f,
        r#"{{"type":"assistant","sessionId":"x","message":{{"stop_reason":"tool_use","content":[{{"type":"text","text":"thinking"}}]}}}}"#
    )
    .unwrap();
    // Mid-turn assistant row carrying the actual tool_use block.
    // AssistantTurn is already emitted, so this one surfaces
    // ToolUseStart with the tool name.
    writeln!(
        f,
        r#"{{"type":"assistant","sessionId":"x","message":{{"stop_reason":"tool_use","content":[{{"type":"tool_use","name":"Edit","id":"t1","input":{{}}}}]}}}}"#
    )
    .unwrap();
    writeln!(
        f,
        r#"{{"type":"user","sessionId":"x","toolUseResult":"ok","message":{{"content":[]}}}}"#
    )
    .unwrap();
    f.sync_all().unwrap();

    let events = drain(&rx);
    assert!(
        events
            .iter()
            .any(|e| matches!(e, ClaudeEvent::ToolUseStart { name } if name == "Edit")),
        "expected ToolUseStart(Edit), got {events:?}"
    );
    assert!(
        events
            .iter()
            .any(|e| matches!(e, ClaudeEvent::ToolUseResult)),
        "expected ToolUseResult, got {events:?}"
    );
}

#[test]
fn sidechain_rows_are_ignored() {
    let dir = TempDir::new().unwrap();
    let (_mgr, path, rx) = make_adapter(&dir);

    let mut f = fs::File::create(&path).unwrap();
    writeln!(
        f,
        r#"{{"type":"assistant","isSidechain":true,"sessionId":"x","message":{{"content":[]}}}}"#
    )
    .unwrap();
    writeln!(
        f,
        r#"{{"type":"last-prompt","isSidechain":true,"sessionId":"x"}}"#
    )
    .unwrap();
    f.sync_all().unwrap();

    let events = drain_brief(&rx);
    assert!(
        events.is_empty(),
        "sidechain rows should be ignored, got {events:?}"
    );
}

/// `parse_value` on one JSON row, starting outside an assistant turn.
fn parse_one(row: &str) -> Option<ClaudeEvent> {
    let value: serde_json::Value = serde_json::from_str(row).unwrap();
    parse_value(&value, &mut false, &mut LocalCommandTracker::default())
}

// #260 — the row shapes below are from a real Claude Code 2.1.270
// transcript whose last turn was interrupted during a tool call.

#[test]
fn interrupt_rows_end_the_turn() {
    for row in [
        r#"{"type":"user","sessionId":"x","message":{"role":"user","content":[{"type":"text","text":"[Request interrupted by user for tool use]"}]}}"#,
        r#"{"type":"user","sessionId":"x","message":{"role":"user","content":[{"type":"text","text":"[Request interrupted by user]"}]}}"#,
        r#"{"type":"user","sessionId":"x","message":{"role":"user","content":"[Request interrupted by user]"}}"#,
    ] {
        assert!(
            matches!(parse_one(row), Some(ClaudeEvent::AwaitingPrompt)),
            "{row}"
        );
    }
}

#[test]
fn interrupt_closes_an_open_assistant_turn() {
    let value: serde_json::Value = serde_json::from_str(
        r#"{"type":"user","sessionId":"x","message":{"content":[{"type":"text","text":"[Request interrupted by user for tool use]"}]}}"#,
    )
    .unwrap();
    let mut in_turn = true;
    assert!(matches!(
        parse_value(&value, &mut in_turn, &mut LocalCommandTracker::default()),
        Some(ClaudeEvent::AwaitingPrompt)
    ));
    assert!(!in_turn);
}

#[test]
fn a_prompt_that_only_mentions_the_interrupt_text_is_still_a_prompt() {
    let row = r#"{"type":"user","sessionId":"x","message":{"content":[{"type":"text","text":"why did I see [Request interrupted by user]?"}]}}"#;
    assert!(matches!(
        parse_one(row),
        Some(ClaudeEvent::UserPrompt { .. })
    ));
    // A tool result never ends the turn, whatever its text says.
    let result = r#"{"type":"user","sessionId":"x","toolUseResult":"x","message":{"content":[{"type":"text","text":"[Request interrupted by user]"}]}}"#;
    assert!(matches!(
        parse_one(result),
        Some(ClaudeEvent::ToolUseResult)
    ));
}

const QUEUED_NOTIFICATION: &str = r#"{"type":"user","message":{"role":"user","content":"<task-notification>done</task-notification>"},"origin":{"kind":"task-notification"},"promptSource":"system","queueTranscriptOnly":true,"uuid":"u9","timestamp":"2026-01-01T00:00:00.030Z"}"#;

#[test]
fn queue_transcript_only_user_row_is_not_a_prompt() {
    let value: serde_json::Value = serde_json::from_str(QUEUED_NOTIFICATION).unwrap();
    let mut in_turn = true;
    assert!(parse_value(&value, &mut in_turn, &mut LocalCommandTracker::default()).is_none());
    assert!(in_turn, "the skip must leave the turn flag alone");
    // Same row without the flag starts a turn.
    let unflagged = QUEUED_NOTIFICATION.replace(r#""queueTranscriptOnly":true,"#, "");
    assert!(matches!(
        parse_one(&unflagged),
        Some(ClaudeEvent::UserPrompt { .. })
    ));
}

#[test]
fn queue_transcript_only_row_after_end_turn_keeps_awaiting_prompt() {
    let end = r#"{"type":"assistant","message":{"stop_reason":"end_turn","content":[]}}"#;
    let log = format!("{end}\n{QUEUED_NOTIFICATION}\n");
    assert!(matches!(
        determine_initial_state(&log),
        Some(ClaudeEvent::AwaitingPrompt)
    ));
}

// #463 — local slash command rows (shapes from Claude Code 2.1.285,
// contents anonymised).
const LC_END: &str = r#"{"type":"assistant","message":{"stop_reason":"end_turn","content":[]},"uuid":"a1","timestamp":"2026-01-01T00:00:00.000Z"}"#;

const LC_NAME: &str = r#"{"type":"system","subtype":"local_command","content":"<command-name>/context</command-name>\n<command-message>context</command-message>\n<command-args></command-args>","level":"info","isMeta":false,"uuid":"s1","timestamp":"2026-01-01T00:00:01.000Z"}"#;

const LC_STDOUT: &str = r#"{"type":"system","subtype":"local_command","content":"<local-command-stdout>ctx output</local-command-stdout>","level":"info","isMeta":false,"commandRun":{"command":"context"},"uuid":"s2","timestamp":"2026-01-01T00:00:01.100Z"}"#;

const LC_OUTPUT: &str = r#"{"type":"user","isMeta":true,"promptId":"p1","message":{"role":"user","content":"ctx output"},"uuid":"u1","timestamp":"2026-01-01T00:00:01.200Z"}"#;

const LC_PEER: &str = r#"{"type":"user","isMeta":true,"promptId":"p2","origin":{"kind":"peer","from":"x","handback":true},"promptSource":"system","turnOrigin":"peer","message":{"role":"user","content":"Another Claude session sent a message: hi"},"uuid":"u2","timestamp":"2026-01-01T00:00:02.000Z"}"#;

const LC_CUSTOM: &str = r#"{"type":"user","promptId":"p3","origin":{"kind":"human"},"message":{"role":"user","content":"<command-message>my-cmd</command-message>\n<command-name>/my-cmd</command-name>"},"uuid":"u3","timestamp":"2026-01-01T00:00:03.000Z"}"#;

fn lc_feed(
    rows: &[&str],
    in_turn: &mut bool,
    tracker: &mut LocalCommandTracker,
) -> Vec<ClaudeEvent> {
    rows.iter()
        .filter_map(|r| {
            let v: serde_json::Value = serde_json::from_str(r).unwrap();
            parse_value(&v, in_turn, tracker)
        })
        .collect()
}

#[test]
fn local_command_rows_after_end_turn_produce_no_events_live() {
    let mut in_turn = false;
    let mut tracker = LocalCommandTracker::default();
    let first = lc_feed(&[LC_END], &mut in_turn, &mut tracker);
    assert!(matches!(first.as_slice(), [ClaudeEvent::AwaitingPrompt]));
    let events = lc_feed(&[LC_NAME, LC_STDOUT, LC_OUTPUT], &mut in_turn, &mut tracker);
    assert!(events.is_empty(), "got {events:?}");
}

#[test]
fn local_command_rows_appended_to_a_tailed_file_produce_no_events() {
    let dir = TempDir::new().unwrap();
    let (state, path, _db) = t425_vanished(&dir, false);
    fs::write(
        &path,
        t425_jsonl(&[
            T425_PROMPT,
            T425_TOOL,
            T425_RESULT,
            T425_END,
            LC_NAME,
            LC_STDOUT,
            LC_OUTPUT,
        ]),
    )
    .unwrap();
    let events = t425_tick(&state);
    assert!(events.is_empty(), "got {events:?}");
}

#[test]
fn local_command_rows_after_end_turn_keep_awaiting_prompt_on_attach() {
    let log = format!("{LC_END}\n{LC_NAME}\n{LC_STDOUT}\n{LC_OUTPUT}\n");
    assert!(matches!(
        determine_initial_state(&log),
        Some(ClaudeEvent::AwaitingPrompt)
    ));
}

#[test]
fn custom_slash_command_still_starts_a_turn() {
    let mut in_turn = false;
    let mut tracker = LocalCommandTracker::default();
    let events = lc_feed(
        &[LC_END, LC_NAME, LC_STDOUT, LC_OUTPUT, LC_CUSTOM],
        &mut in_turn,
        &mut tracker,
    );
    assert!(
        matches!(
            events.as_slice(),
            [ClaudeEvent::AwaitingPrompt, ClaudeEvent::UserPrompt { .. }]
        ),
        "got {events:?}"
    );
}

#[test]
fn peer_message_right_after_a_local_command_still_starts_a_turn() {
    let mut in_turn = false;
    let mut tracker = LocalCommandTracker::default();
    let events = lc_feed(
        &[LC_END, LC_NAME, LC_STDOUT, LC_OUTPUT, LC_PEER],
        &mut in_turn,
        &mut tracker,
    );
    assert!(
        matches!(
            events.as_slice(),
            [ClaudeEvent::AwaitingPrompt, ClaudeEvent::UserPrompt { .. }]
        ),
        "got {events:?}"
    );
}

#[test]
fn local_command_burst_split_across_attach_and_live_is_recognised() {
    let attach = format!("{LC_END}\n{LC_NAME}\n{LC_STDOUT}\n");
    let mut tracker = LocalCommandTracker::default();
    let (init, _) = scan_history(&attach, None, &mut BackgroundState::default(), &mut tracker);
    assert!(matches!(init, Some(ClaudeEvent::AwaitingPrompt)));
    // The live tail is seeded with the attach walk's tracker.
    let mut in_turn = false;
    let events = lc_feed(&[LC_OUTPUT], &mut in_turn, &mut tracker);
    assert!(events.is_empty(), "got {events:?}");
}

#[test]
fn turn_duration_ends_the_turn_and_other_system_rows_do_not() {
    let duration = r#"{"type":"system","subtype":"turn_duration","durationMs":3158,"messageCount":44,"sessionId":"x"}"#;
    assert!(matches!(
        parse_one(duration),
        Some(ClaudeEvent::AwaitingPrompt)
    ));
    let other = r#"{"type":"system","subtype":"compact_boundary","sessionId":"x"}"#;
    assert!(parse_one(other).is_none());
}

#[test]
fn determine_initial_state_resumes_an_interrupted_turn_as_waiting() {
    let log = concat!(
        r#"{"type":"user","sessionId":"x","message":{"content":"try again"}}"#,
        "\n",
        r#"{"type":"assistant","sessionId":"x","message":{"stop_reason":"tool_use","content":[{"type":"text","text":"ok"}]}}"#,
        "\n",
        r#"{"type":"assistant","sessionId":"x","message":{"stop_reason":"tool_use","content":[{"type":"tool_use","name":"Bash","id":"t1","input":{}}]}}"#,
        "\n",
        r#"{"type":"user","sessionId":"x","toolUseResult":"Error","message":{"content":[{"type":"tool_result","tool_use_id":"t1"}]}}"#,
        "\n",
        r#"{"type":"user","sessionId":"x","message":{"content":[{"type":"text","text":"[Request interrupted by user for tool use]"}]}}"#,
        "\n",
        r#"{"type":"system","subtype":"turn_duration","durationMs":3158,"sessionId":"x"}"#,
        "\n",
        r#"{"type":"cost-state","sessionId":"x","totalCostUSD":0.9}"#,
        "\n",
        r#"{"type":"last-prompt","lastPrompt":"try again","sessionId":"x"}"#,
        "\n"
    );
    let result = determine_initial_state(log);
    assert!(
        matches!(result, Some(ClaudeEvent::AwaitingPrompt)),
        "an interrupted session must not resume into running, got {result:?}"
    );
}

#[test]
fn determine_initial_state_handles_empty_and_metadata_only() {
    assert!(determine_initial_state("").is_none());
    assert!(determine_initial_state("\n\n").is_none());
    let metadata_only = concat!(
        r#"{"type":"permission-mode","sessionId":"x"}"#,
        "\n",
        r#"{"type":"ai-title","sessionId":"x"}"#,
        "\n"
    );
    assert!(determine_initial_state(metadata_only).is_none());
}

#[test]
fn determine_initial_state_skips_sidechain_rows_when_picking_final() {
    // Real layout: main session ends in end_turn, then sub-agent
    // writes more rows. The sub-agent rows must not override the
    // main session's terminal state.
    let log = concat!(
        r#"{"type":"assistant","sessionId":"x","message":{"stop_reason":"end_turn","content":[]}}"#,
        "\n",
        r#"{"type":"assistant","isSidechain":true,"sessionId":"x","message":{"stop_reason":"tool_use","content":[]}}"#,
        "\n",
        r#"{"type":"user","isSidechain":true,"sessionId":"x","message":{"content":[]}}"#,
        "\n"
    );
    let result = determine_initial_state(log);
    assert!(
        matches!(result, Some(ClaudeEvent::AwaitingPrompt)),
        "main session's end_turn must win over sub-agent rows, got {result:?}"
    );
}
