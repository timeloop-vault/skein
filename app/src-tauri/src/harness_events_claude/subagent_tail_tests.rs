//! Subagent tail details: attach seeding, persistence and main-transcript isolation.

use super::adapter::TailState;
use super::background_tasks::BackgroundState;
use super::persist_tests::make_persisting_adapter;
use super::subagent_tests::{
    BASH_RESULT, BASH_USE, HB_RESULT, HB_RESULT_NO_FLAG, HB_TOOL_USE, count_ends,
    make_adapter_with_existing_main,
};
use super::tests::{append_lines, drain, drain_brief, make_adapter, test_manager};
use super::tick::tick;
use super::*;
use parking_lot::Mutex;
use skein_harness::claude::local_command::LocalCommandTracker;
use std::collections::HashMap;
use std::io::Write;
use std::sync::Arc;
use std::sync::mpsc;
use std::time::Duration;
use std::{fs, thread};
use tempfile::TempDir;

/// At attach, a handback-finished transcript seeds nothing; a
/// mid-work one seeds an initial start.
#[test]
fn attach_seeds_start_only_for_subagent_not_handed_back() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("session.jsonl");
    append_lines(
        &path,
        &[
            r#"{"type":"assistant","sessionId":"x","message":{"stop_reason":"tool_use","content":[]}}"#,
        ],
    );
    let sub_dir = skein_harness::claude::subagents_dir(&path).unwrap();
    fs::create_dir_all(&sub_dir).unwrap();
    append_lines(
        &sub_dir.join("agent-done.jsonl"),
        &[BASH_USE, BASH_RESULT, HB_TOOL_USE, HB_RESULT],
    );
    append_lines(&sub_dir.join("agent-busy.jsonl"), &[BASH_USE]);

    let (tx, rx) = mpsc::channel();
    let manager = test_manager();
    manager
        .attach_at("h1", path, move |e| tx.send(e).unwrap(), None)
        .unwrap();
    let events = drain_brief(&rx);
    assert!(
        events.iter().any(|e| matches!(
            e,
            ClaudeEvent::SubagentStart { agent_id, initial, .. }
                if agent_id == "busy" && *initial
        )),
        "got {events:?}"
    );
    assert!(
        !events.iter().any(
            |e| matches!(e, ClaudeEvent::SubagentStart { agent_id, .. } if agent_id == "done")
        ),
        "handed-back subagent must not be seeded, got {events:?}"
    );
}

/// The handback id seen at attach carries into the live tail: a
/// result without `toolEndsTurn` still ends the subagent.
#[test]
fn attach_handback_id_carries_into_live_tail() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("session.jsonl");
    append_lines(
        &path,
        &[
            r#"{"type":"assistant","sessionId":"x","message":{"stop_reason":"tool_use","content":[]}}"#,
        ],
    );
    let sub_dir = skein_harness::claude::subagents_dir(&path).unwrap();
    fs::create_dir_all(&sub_dir).unwrap();
    let sub_path = sub_dir.join("agent-a1.jsonl");
    append_lines(&sub_path, &[BASH_USE, BASH_RESULT, HB_TOOL_USE]);

    let (tx, rx) = mpsc::channel();
    let manager = test_manager();
    manager
        .attach_at("h1", path, move |e| tx.send(e).unwrap(), None)
        .unwrap();
    let events = drain_brief(&rx);
    assert!(
        events.iter().any(|e| matches!(
            e,
            ClaudeEvent::SubagentStart { agent_id, initial, .. }
                if agent_id == "a1" && *initial
        )),
        "got {events:?}"
    );
    append_lines(&sub_path, &[HB_RESULT_NO_FLAG]);
    let events = drain(&rx);
    assert_eq!(count_ends(&events), 1, "got {events:?}");
}

/// `tick_subagents` stats a subagent file before opening it and
/// skips the open/seek/read entirely when nothing has grown since
/// `last_pos` — the skip must never wedge the tail. A no-growth
/// tick produces no duplicate events, and a later tick where the
/// file DOES grow is still read correctly.
#[test]
fn subagent_tail_skip_on_no_growth_does_not_wedge_later_reads() {
    let dir = TempDir::new().unwrap();
    let (mgr, path, rx) = make_adapter_with_existing_main(&dir);
    drain(&rx);

    let sub_dir = skein_harness::claude::subagents_dir(&path).unwrap();
    fs::create_dir_all(&sub_dir).unwrap();
    // #362: arm the watch on the just-created dir.
    mgr.supervise_once();
    let sub_path = sub_dir.join("agent-a6.jsonl");
    {
        let mut f = fs::File::create(&sub_path).unwrap();
        writeln!(
            f,
            r#"{{"type":"assistant","isSidechain":true,"agentId":"a6","message":{{"stop_reason":"tool_use","content":[]}}}}"#
        )
        .unwrap();
        f.sync_all().unwrap();
    }

    let events = drain(&rx);
    assert!(
        events
            .iter()
            .any(|e| matches!(e, ClaudeEvent::SubagentStart { agent_id, .. } if agent_id == "a6")),
        "expected SubagentStart for a6, got {events:?}"
    );

    // A tick with nothing new for the subagent — append only to
    // the main transcript, which shares the same debouncer/
    // callback and still drives `tick_subagents` once per tick.
    {
        let mut mf = fs::OpenOptions::new().append(true).open(&path).unwrap();
        writeln!(
            mf,
            r#"{{"type":"assistant","sessionId":"x","message":{{"stop_reason":"end_turn","content":[]}}}}"#
        )
        .unwrap();
        mf.sync_all().unwrap();
    }
    let quiet = drain(&rx);
    assert!(
        !quiet
            .iter()
            .any(|e| matches!(e, ClaudeEvent::SubagentStart { agent_id, .. } if agent_id == "a6")),
        "SubagentStart re-fired on a no-growth tick, got {quiet:?}"
    );

    // The subagent file DOES grow now — a stat-skip on the
    // previous tick must not have wedged `last_pos` such that this
    // new row goes unread.
    {
        let mut f = fs::OpenOptions::new().append(true).open(&sub_path).unwrap();
        writeln!(
            f,
            r#"{{"type":"assistant","isSidechain":true,"agentId":"a6","message":{{"stop_reason":"end_turn","content":[]}}}}"#
        )
        .unwrap();
        f.sync_all().unwrap();
    }
    let after_growth = drain(&rx);
    let end_count = after_growth
        .iter()
        .filter(|e| matches!(e, ClaudeEvent::SubagentEnd { agent_id, .. } if agent_id == "a6"))
        .count();
    assert_eq!(
        end_count, 1,
        "expected exactly one SubagentEnd after the file grew again, got {after_growth:?}"
    );
}

/// A live subagent reaching a terminal row writes exactly one
/// `subagent_end` `harness_actions` row, with the expected payload
/// fields including a `duration_ms` computed from the first row's
/// timestamp on this transcript.
#[test]
fn subagent_end_persists_one_action_with_duration() {
    let dir = TempDir::new().unwrap();
    let jsonl = dir.path().join("session.jsonl");
    let db_path = dir.path().join("test.db");

    // `tick()` only reaches `tick_subagents` once the main
    // transcript exists and the initial attach probe has seen it
    // (see `!s.attached` in `tick`) — a session with no main file
    // yet never ticks at all, subagents included. Give it one row
    // so the watcher has something to attach to, mirroring
    // `main_transcript_events_unaffected_by_a_subagents_dir`.
    let _manager = make_persisting_adapter(jsonl.clone(), &db_path, "h1", "r1");
    {
        let mut f = fs::File::create(&jsonl).unwrap();
        writeln!(
            f,
            r#"{{"type":"assistant","sessionId":"x","timestamp":"2026-05-15T21:16:19.000Z","message":{{"stop_reason":"tool_use","content":[]}}}}"#
        )
        .unwrap();
        f.sync_all().unwrap();
    }

    let sub_dir = skein_harness::claude::subagents_dir(&jsonl).unwrap();
    fs::create_dir_all(&sub_dir).unwrap();
    fs::write(
        sub_dir.join("agent-a1.meta.json"),
        r#"{"agentType":"explore","description":"Map the tailer"}"#,
    )
    .unwrap();
    let mut f = fs::File::create(sub_dir.join("agent-a1.jsonl")).unwrap();
    writeln!(
        f,
        r#"{{"type":"assistant","isSidechain":true,"agentId":"a1","timestamp":"2026-05-15T21:16:20.000Z","message":{{"stop_reason":"tool_use","content":[]}}}}"#
    )
    .unwrap();
    writeln!(
        f,
        r#"{{"type":"assistant","isSidechain":true,"agentId":"a1","timestamp":"2026-05-15T21:16:25.000Z","message":{{"stop_reason":"end_turn","content":[]}}}}"#
    )
    .unwrap();
    f.sync_all().unwrap();

    // Give the debouncer a moment to tick and persist.
    thread::sleep(Duration::from_secs(2));

    let db = crate::db::Database::open(&db_path).unwrap();
    let actions = db.recent_harness_actions_by_room("r1", -1, 100).unwrap();
    let ends: Vec<_> = actions
        .iter()
        .filter(|a| a.kind == crate::db::action_kind::SUBAGENT_END)
        .collect();
    assert_eq!(
        ends.len(),
        1,
        "expected exactly one subagent_end row, got {actions:?}"
    );
    let payload: serde_json::Value = serde_json::from_str(&ends[0].payload).unwrap();
    assert_eq!(payload["agent_id"], "a1");
    assert_eq!(payload["agent_type"], "explore");
    assert_eq!(payload["description"], "Map the tailer");
    assert_eq!(payload["duration_ms"], 5000);
}

/// A subagent already finished on disk at attach time must not get
/// a synthetic `subagent_end` action — mirrors
/// `attach_emits_start_only_for_the_unfinished_subagent`'s phase
/// assertion, but for the persisted action row.
#[test]
fn attach_does_not_persist_subagent_end_for_already_finished_subagent() {
    let dir = TempDir::new().unwrap();
    let jsonl = dir.path().join("session.jsonl");
    let db_path = dir.path().join("test.db");
    {
        let mut f = fs::File::create(&jsonl).unwrap();
        writeln!(
            f,
            r#"{{"type":"assistant","sessionId":"x","message":{{"stop_reason":"tool_use","content":[]}}}}"#
        )
        .unwrap();
        f.sync_all().unwrap();
    }
    let sub_dir = skein_harness::claude::subagents_dir(&jsonl).unwrap();
    fs::create_dir_all(&sub_dir).unwrap();
    // Finished before attach — last row has a terminal stop_reason.
    fs::write(
        sub_dir.join("agent-fin.jsonl"),
        "{\"type\":\"assistant\",\"isSidechain\":true,\"agentId\":\"fin\",\"timestamp\":\"2026-05-15T21:16:20.000Z\",\"message\":{\"stop_reason\":\"end_turn\",\"content\":[]}}\n",
    )
    .unwrap();

    let _manager = make_persisting_adapter(jsonl, &db_path, "h1", "r1");
    thread::sleep(Duration::from_secs(2));

    let db = crate::db::Database::open(&db_path).unwrap();
    let actions = db.recent_harness_actions_by_room("r1", -1, 100).unwrap();
    assert!(
        !actions
            .iter()
            .any(|a| a.kind == crate::db::action_kind::SUBAGENT_END),
        "attach must not synthesize a subagent_end action for an already-finished subagent, got {actions:?}"
    );
}

#[test]
fn subagent_tool_result_emitted_for_tool_result_row() {
    let dir = TempDir::new().unwrap();
    let (mgr, path, rx) = make_adapter_with_existing_main(&dir);
    drain(&rx);

    let sub_dir = skein_harness::claude::subagents_dir(&path).unwrap();
    fs::create_dir_all(&sub_dir).unwrap();
    // #362: arm the watch on the just-created dir.
    mgr.supervise_once();
    let mut f = fs::File::create(sub_dir.join("agent-a4.jsonl")).unwrap();
    writeln!(
        f,
        r#"{{"type":"user","isSidechain":true,"agentId":"a4","message":{{"content":[{{"type":"tool_result","tool_use_id":"t1","content":"ok"}}]}}}}"#
    )
    .unwrap();
    f.sync_all().unwrap();

    let events = drain(&rx);
    assert!(
        events
            .iter()
            .any(|e| matches!(e, ClaudeEvent::SubagentToolResult { agent_id } if agent_id == "a4")),
        "expected SubagentToolResult, got {events:?}"
    );
}

#[test]
fn attach_emits_start_only_for_the_unfinished_subagent() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("session.jsonl");
    {
        let mut f = fs::File::create(&path).unwrap();
        writeln!(
            f,
            r#"{{"type":"assistant","sessionId":"x","message":{{"stop_reason":"tool_use","content":[]}}}}"#
        )
        .unwrap();
        f.sync_all().unwrap();
    }
    let sub_dir = skein_harness::claude::subagents_dir(&path).unwrap();
    fs::create_dir_all(&sub_dir).unwrap();
    // Finished before attach — last row has a terminal stop_reason.
    fs::write(
        sub_dir.join("agent-fin.jsonl"),
        "{\"type\":\"assistant\",\"isSidechain\":true,\"agentId\":\"fin\",\"message\":{\"stop_reason\":\"end_turn\",\"content\":[]}}\n",
    )
    .unwrap();
    // Still running — last row has no terminal stop_reason.
    fs::write(
        sub_dir.join("agent-live.jsonl"),
        "{\"type\":\"assistant\",\"isSidechain\":true,\"agentId\":\"live\",\"message\":{\"stop_reason\":\"tool_use\",\"content\":[]}}\n",
    )
    .unwrap();

    let (tx, rx) = mpsc::channel();
    let manager = test_manager();
    manager
        .attach_at("h1", path, move |e| tx.send(e).unwrap(), None)
        .unwrap();

    let events = drain_brief(&rx);
    assert!(
        events.iter().any(|e| matches!(
            e,
            ClaudeEvent::SubagentStart { agent_id, initial, .. }
                if agent_id == "live" && *initial
        )),
        "expected an initial SubagentStart for the unfinished subagent, got {events:?}"
    );
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, ClaudeEvent::SubagentStart { agent_id, .. } if agent_id == "fin")),
        "the already-finished subagent must not get a synthetic SubagentStart, got {events:?}"
    );
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, ClaudeEvent::SubagentEnd { .. })),
        "attach should not synthesize a SubagentEnd for a subagent that was already finished, got {events:?}"
    );
}

#[test]
fn subagent_line_split_across_two_ticks_is_reassembled() {
    let dir = TempDir::new().unwrap();
    let (mgr, path, rx) = make_adapter_with_existing_main(&dir);
    drain(&rx);

    let sub_dir = skein_harness::claude::subagents_dir(&path).unwrap();
    fs::create_dir_all(&sub_dir).unwrap();
    // #362: arm the watch on the just-created dir.
    mgr.supervise_once();
    let sub_path = sub_dir.join("agent-a5.jsonl");
    {
        let mut f = fs::File::create(&sub_path).unwrap();
        f.write_all(
            br#"{"type":"assistant","isSidechain":true,"agentId":"a5","message":{"stop_reason":"#,
        )
        .unwrap();
        f.sync_all().unwrap();
    }
    thread::sleep(Duration::from_millis(150));
    {
        let mut f = fs::OpenOptions::new().append(true).open(&sub_path).unwrap();
        f.write_all(b"\"end_turn\",\"content\":[]}}\n").unwrap();
        f.sync_all().unwrap();
    }

    let events = drain(&rx);
    let end_count = events
        .iter()
        .filter(|e| matches!(e, ClaudeEvent::SubagentEnd { agent_id, .. } if agent_id == "a5"))
        .count();
    assert_eq!(
        end_count, 1,
        "expected the reassembled line to parse into exactly one SubagentEnd, got {events:?}"
    );
}

#[test]
fn main_transcript_events_unaffected_by_a_subagents_dir() {
    let dir = TempDir::new().unwrap();
    let (_mgr, path, rx) = make_adapter(&dir);

    // A session that has already delegated: subagents dir exists
    // with a live transcript in it before the main file gets its
    // first (and, here, only) row.
    let sub_dir = skein_harness::claude::subagents_dir(&path).unwrap();
    fs::create_dir_all(&sub_dir).unwrap();
    fs::write(
        sub_dir.join("agent-a9.jsonl"),
        "{\"type\":\"assistant\",\"isSidechain\":true,\"agentId\":\"a9\",\"message\":{\"stop_reason\":\"tool_use\",\"content\":[]}}\n",
    )
    .unwrap();

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
        "main-transcript AwaitingPrompt must still fire with a subagents dir present, got {events:?}"
    );
}

// ── #362: no eager creation, and re-arming a deleted+recreated
//    watched dir ─────────────────────────────────────────────────

/// A subagents dir that simply doesn't exist yet — the common case
/// now that `attach_at` no longer pre-creates it, since most
/// sessions never delegate at all — must not trip the one-shot
/// "could not read subagents dir" warn guard. That guard is for a
/// REAL failure (permissions, say), not "this session hasn't
/// delegated"; tripping it here would warn once per harness that
/// never even uses subagents.
#[test]
fn missing_subagents_dir_does_not_trip_the_read_failure_guard() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("session.jsonl");
    fs::write(&path, "").unwrap();
    // Deliberately never created.
    let sub_dir = dir.path().join("session").join("subagents");
    let state = Arc::new(Mutex::new(TailState {
        harness_id: "h".into(),
        path,
        last_pos: 0,
        partial: String::new(),
        attached: true,
        ever_attached: true,
        fingerprint: Vec::new(),
        in_assistant_turn: false,
        local_command: LocalCommandTracker::default(),
        actions: None,
        subagents_dir: Some(sub_dir),
        subagents: HashMap::new(),
        background: BackgroundState::default(),
        subagents_dir_read_failure_logged: false,
        first_read_logged: false,
        events_sent: 0,
        send_errors: 0,
        last_heartbeat: None,
        utf8_stall_at: None,
        utf8_stall_count: 0,
        utf8_stall_warned: false,
        last_cli_version: None,
    }));

    tick(&state, &|_| {});
    assert!(
        !state.lock().subagents_dir_read_failure_logged,
        "a merely-nonexistent subagents dir must not trip the read-failure guard"
    );
}
