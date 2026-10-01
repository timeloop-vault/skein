//! Subagent start, end and handback on the sidecar tail.

use super::persist_tests::make_persisting_adapter;
use super::tests::{append_lines, drain, test_manager};
use super::*;
use std::io::Write;
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::Duration;
use std::{fs, thread};
use tempfile::TempDir;

// ── subagent tailing (#276) ────────────────────────────────────

/// Like `make_adapter`, but the main transcript already has one
/// row on disk before `attach_at` runs. Subagent-tailing tests
/// want `attached == true` from the start — otherwise `tick`
/// returns before it ever reaches `tick_subagents` (see the early
/// `if !s.attached` return), and a subagents-dir-only change
/// would silently be missed until the main file also gets its
/// first write. Production sessions always write the main file
/// before delegating, so this mirrors reality, not just the test.
pub(super) fn make_adapter_with_existing_main(
    dir: &TempDir,
) -> (ClaudeEventsManager, PathBuf, mpsc::Receiver<ClaudeEvent>) {
    let path = dir.path().join("session.jsonl");
    {
        let mut f = fs::File::create(&path).unwrap();
        writeln!(
            f,
            r#"{{"type":"user","sessionId":"x","message":{{"content":[{{"type":"text","text":"hi"}}]}}}}"#
        )
        .unwrap();
        f.sync_all().unwrap();
    }
    let (tx, rx) = mpsc::channel();
    let manager = test_manager();
    manager
        .attach_at(
            "harness-1",
            path.clone(),
            move |event| {
                tx.send(event).unwrap();
            },
            None,
        )
        .unwrap();
    (manager, path, rx)
}

#[test]
fn subagent_start_emitted_with_meta_from_sidecar() {
    let dir = TempDir::new().unwrap();
    let (mgr, path, rx) = make_adapter_with_existing_main(&dir);
    // Drain the main-transcript bootstrap event before we care
    // about subagent ones.
    drain(&rx);

    let sub_dir = skein_harness::claude::subagents_dir(&path).unwrap();
    fs::create_dir_all(&sub_dir).unwrap();
    // #362: the subagents dir didn't exist at attach time, so
    // nothing is watching it yet — `supervise_once` stands in for
    // the background thread the production manager runs every
    // `SUPERVISE_INTERVAL`, arming the watch now that the dir is
    // here.
    mgr.supervise_once();
    fs::write(
        sub_dir.join("agent-a1.meta.json"),
        r#"{"agentType":"explore","description":"Map the tailer"}"#,
    )
    .unwrap();
    let mut f = fs::File::create(sub_dir.join("agent-a1.jsonl")).unwrap();
    writeln!(
        f,
        r#"{{"type":"assistant","isSidechain":true,"agentId":"a1","message":{{"stop_reason":"tool_use","content":[]}}}}"#
    )
    .unwrap();
    f.sync_all().unwrap();

    let events = drain(&rx);
    assert!(
        events.iter().any(|e| matches!(
            e,
            ClaudeEvent::SubagentStart { agent_id, agent_type, description, initial }
                if agent_id == "a1"
                    && agent_type.as_deref() == Some("explore")
                    && description.as_deref() == Some("Map the tailer")
                    && !initial
        )),
        "expected a live (non-initial) SubagentStart with sidecar meta, got {events:?}"
    );
}

#[test]
fn subagent_start_emitted_without_sidecar() {
    let dir = TempDir::new().unwrap();
    let (mgr, path, rx) = make_adapter_with_existing_main(&dir);
    drain(&rx);

    let sub_dir = skein_harness::claude::subagents_dir(&path).unwrap();
    fs::create_dir_all(&sub_dir).unwrap();
    // #362: arm the watch on the just-created dir — see the sibling
    // test above for why this is needed.
    mgr.supervise_once();
    // No .meta.json sidecar written for this one.
    let mut f = fs::File::create(sub_dir.join("agent-a2.jsonl")).unwrap();
    writeln!(
        f,
        r#"{{"type":"assistant","isSidechain":true,"agentId":"a2","message":{{"stop_reason":"tool_use","content":[]}}}}"#
    )
    .unwrap();
    f.sync_all().unwrap();

    let events = drain(&rx);
    assert!(
        events.iter().any(|e| matches!(
            e,
            ClaudeEvent::SubagentStart { agent_id, agent_type, description, initial }
                if agent_id == "a2" && agent_type.is_none() && description.is_none() && !initial
        )),
        "expected a live (non-initial) SubagentStart with no metadata, got {events:?}"
    );
}

#[test]
fn subagent_end_emitted_once_not_per_tick() {
    let dir = TempDir::new().unwrap();
    let (mgr, path, rx) = make_adapter_with_existing_main(&dir);
    drain(&rx);

    let sub_dir = skein_harness::claude::subagents_dir(&path).unwrap();
    fs::create_dir_all(&sub_dir).unwrap();
    // #362: arm the watch on the just-created dir.
    mgr.supervise_once();
    let mut f = fs::File::create(sub_dir.join("agent-a3.jsonl")).unwrap();
    writeln!(
        f,
        r#"{{"type":"assistant","isSidechain":true,"agentId":"a3","message":{{"stop_reason":"end_turn","content":[]}}}}"#
    )
    .unwrap();
    f.sync_all().unwrap();

    let events = drain(&rx);
    let end_count = events
        .iter()
        .filter(|e| matches!(e, ClaudeEvent::SubagentEnd { agent_id, .. } if agent_id == "a3"))
        .count();
    assert_eq!(
        end_count, 1,
        "expected exactly one SubagentEnd, got {events:?}"
    );

    // Force another tick that has nothing new to say about the
    // subagent — append to the main transcript, which shares the
    // same debouncer/callback as the subagents dir.
    {
        let mut mf = fs::OpenOptions::new().append(true).open(&path).unwrap();
        writeln!(
            mf,
            r#"{{"type":"assistant","sessionId":"x","message":{{"stop_reason":"end_turn","content":[]}}}}"#
        )
        .unwrap();
        mf.sync_all().unwrap();
    }

    let more = drain(&rx);
    assert!(
        !more
            .iter()
            .any(|e| matches!(e, ClaudeEvent::SubagentEnd { agent_id, .. } if agent_id == "a3")),
        "SubagentEnd re-fired on a later tick, got {more:?}"
    );
}

pub(super) const HB_TOOL_USE: &str = r#"{"type":"assistant","isSidechain":true,"agentId":"a1","uuid":"u2","timestamp":"2026-01-01T00:00:00.000Z","message":{"id":"msg_X","role":"assistant","content":[{"type":"tool_use","id":"toolu_H","name":"SubagentHandback","input":{"message":"..."}}],"stop_reason":null}}"#;

pub(super) const HB_RESULT: &str = r#"{"type":"user","isSidechain":true,"agentId":"a1","uuid":"u4","timestamp":"2026-01-01T00:00:01.100Z","message":{"role":"user","content":[{"tool_use_id":"toolu_H","type":"tool_result","content":[{"type":"text","text":"{\"success\":true,\"message\":\"Report delivered to your caller.\"}"}]}]},"toolUseResult":{"success":true,"message":"Report delivered to your caller."},"toolEndsTurn":true,"sourceToolAssistantUUID":"u2"}"#;

pub(super) const HB_RESULT_NO_FLAG: &str = r#"{"type":"user","isSidechain":true,"agentId":"a1","uuid":"u4","timestamp":"2026-01-01T00:00:01.100Z","message":{"role":"user","content":[{"tool_use_id":"toolu_H","type":"tool_result","content":"delivered"}]}}"#;

pub(super) const BASH_USE: &str = r#"{"type":"assistant","isSidechain":true,"agentId":"a1","uuid":"u0","timestamp":"2026-01-01T00:00:00.000Z","message":{"role":"assistant","content":[{"type":"tool_use","id":"toolu_B","name":"Bash","input":{"command":"ls"}}],"stop_reason":"tool_use"}}"#;

pub(super) const BASH_RESULT: &str = r#"{"type":"user","isSidechain":true,"agentId":"a1","uuid":"u1","timestamp":"2026-01-01T00:00:00.500Z","message":{"role":"user","content":[{"tool_use_id":"toolu_B","type":"tool_result","content":"ok"}]}}"#;

pub(super) fn count_ends(events: &[ClaudeEvent]) -> usize {
    events
        .iter()
        .filter(|e| matches!(e, ClaudeEvent::SubagentEnd { agent_id, .. } if agent_id == "a1"))
        .count()
}

/// #440: a `SubagentHandback` `tool_use` alone is not an exit; its
/// `tool_result` is — one `SubagentEnd` and one `subagent_end` row.
#[test]
fn subagent_handback_result_ends_subagent_once_with_action_row() {
    let dir = TempDir::new().unwrap();
    let jsonl = dir.path().join("session.jsonl");
    let db_path = dir.path().join("test.db");
    let _manager = make_persisting_adapter(jsonl.clone(), &db_path, "h1", "r1");
    append_lines(
        &jsonl,
        &[
            r#"{"type":"assistant","sessionId":"x","message":{"stop_reason":"tool_use","content":[]}}"#,
        ],
    );
    let sub_dir = skein_harness::claude::subagents_dir(&jsonl).unwrap();
    fs::create_dir_all(&sub_dir).unwrap();
    let sub_path = sub_dir.join("agent-a1.jsonl");
    append_lines(&sub_path, &[BASH_USE, BASH_RESULT]);
    thread::sleep(Duration::from_secs(1));
    append_lines(&sub_path, &[HB_TOOL_USE]);
    thread::sleep(Duration::from_secs(1));

    let db = crate::db::Database::open(&db_path).unwrap();
    let ends = |db: &crate::db::Database| {
        db.recent_harness_actions_by_room("r1", -1, 100)
            .unwrap()
            .iter()
            .filter(|a| a.kind == crate::db::action_kind::SUBAGENT_END)
            .count()
    };
    assert_eq!(ends(&db), 0, "tool_use alone must not end the subagent");

    append_lines(&sub_path, &[HB_RESULT]);
    thread::sleep(Duration::from_secs(2));
    assert_eq!(ends(&db), 1, "the handback result ends the subagent once");
}

#[test]
fn subagent_handback_tool_use_then_result_emits_one_end_event() {
    let dir = TempDir::new().unwrap();
    let (mgr, path, rx) = make_adapter_with_existing_main(&dir);
    drain(&rx);
    let sub_dir = skein_harness::claude::subagents_dir(&path).unwrap();
    fs::create_dir_all(&sub_dir).unwrap();
    mgr.supervise_once();
    let sub_path = sub_dir.join("agent-a1.jsonl");
    append_lines(&sub_path, &[BASH_USE, BASH_RESULT, HB_TOOL_USE]);
    let events = drain(&rx);
    assert_eq!(count_ends(&events), 0, "tool_use alone, got {events:?}");
    append_lines(&sub_path, &[HB_RESULT]);
    let events = drain(&rx);
    assert_eq!(count_ends(&events), 1, "got {events:?}");
}

/// The pre-handback ending (terminal `stop_reason`) still exits.
#[test]
fn subagent_end_turn_still_ends_subagent() {
    let dir = TempDir::new().unwrap();
    let (mgr, path, rx) = make_adapter_with_existing_main(&dir);
    drain(&rx);
    let sub_dir = skein_harness::claude::subagents_dir(&path).unwrap();
    fs::create_dir_all(&sub_dir).unwrap();
    mgr.supervise_once();
    append_lines(
        &sub_dir.join("agent-a1.jsonl"),
        &[
            BASH_USE,
            r#"{"type":"assistant","isSidechain":true,"agentId":"a1","message":{"stop_reason":"end_turn","content":[]}}"#,
        ],
    );
    let events = drain(&rx);
    assert_eq!(count_ends(&events), 1, "got {events:?}");
}

/// Trailing 2.1.27x rows after a handback exit do not re-open; a
/// real user prompt does.
#[test]
fn subagent_trailing_rows_after_handback_do_not_reopen_but_prompt_does() {
    let dir = TempDir::new().unwrap();
    let (mgr, path, rx) = make_adapter_with_existing_main(&dir);
    drain(&rx);
    let sub_dir = skein_harness::claude::subagents_dir(&path).unwrap();
    fs::create_dir_all(&sub_dir).unwrap();
    mgr.supervise_once();
    let sub_path = sub_dir.join("agent-a1.jsonl");
    append_lines(&sub_path, &[BASH_USE, BASH_RESULT, HB_TOOL_USE, HB_RESULT]);
    let events = drain(&rx);
    assert_eq!(count_ends(&events), 1, "got {events:?}");

    append_lines(
        &sub_path,
        &[
            r#"{"type":"assistant","isSidechain":true,"agentId":"a1","message":{"role":"assistant","content":[{"type":"text","text":"done"}],"stop_reason":"end_turn"}}"#,
            r#"{"type":"attachment","isSidechain":true,"agentId":"a1","attachment":{"type":"x"}}"#,
        ],
    );
    let events = drain(&rx);
    assert_eq!(count_ends(&events), 0, "no second end, got {events:?}");
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, ClaudeEvent::SubagentStart { .. })),
        "trailing rows must not re-open, got {events:?}"
    );

    append_lines(
        &sub_path,
        &[
            r#"{"type":"user","isSidechain":true,"agentId":"a1","message":{"role":"user","content":"carry on"}}"#,
        ],
    );
    let events = drain(&rx);
    assert!(
        events.iter().any(|e| matches!(
            e,
            ClaudeEvent::SubagentStart { agent_id, initial, .. }
                if agent_id == "a1" && !*initial
        )),
        "a user prompt re-opens, got {events:?}"
    );
}
