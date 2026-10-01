//! A reappearing, shrinking or recreated transcript is never replayed as live (#425).

use super::adapter::{ActionPersistence, SubagentTail, TailState};
use super::background_tasks::BackgroundState;
use super::resync::{FINGERPRINT_LEN, extend_fingerprint, fingerprint_of};
use super::tests::{drain, drain_brief, retry_fs_op, test_manager};
use super::tick::tick;
use super::*;
use crate::harness_actions_claude::ActionExtractor;
use parking_lot::Mutex;
use skein_harness::claude::local_command::LocalCommandTracker;
use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc;
use tempfile::TempDir;

/// `attach_at` must not create either directory Claude owns — not
/// the project dir the transcript lives in, and not the
/// `subagents` dir next to it. Once Claude (simulated here)
/// creates the project dir and writes the first row, a
/// `supervise_once` pass must still find and tail it — nothing
/// about not pre-creating the dir should cost a fresh spawn its
/// first events.
#[test]
fn attach_does_not_create_claude_owned_dirs() {
    let dir = TempDir::new().unwrap();
    let parent = dir.path().join("projects").join("proj-x");
    let path = parent.join("session.jsonl");

    let (tx, rx) = mpsc::channel();
    let manager = test_manager();
    manager
        .attach_at(
            "h1",
            path.clone(),
            move |e| {
                let _ = tx.send(e);
            },
            None,
        )
        .unwrap();

    assert!(
        !parent.exists(),
        "attach must not create the project dir Claude owns"
    );
    let sub_dir = skein_harness::claude::subagents_dir(&path).unwrap();
    assert!(
        !sub_dir.exists(),
        "attach must not create the subagents dir Claude owns"
    );

    // Claude "arrives": creates its own project dir and writes the
    // first row.
    fs::create_dir_all(&parent).unwrap();
    let mut f = fs::File::create(&path).unwrap();
    writeln!(
        f,
        r#"{{"type":"assistant","sessionId":"x","message":{{"stop_reason":"end_turn","content":[]}}}}"#
    )
    .unwrap();
    f.sync_all().unwrap();

    manager.supervise_once();
    let events = drain(&rx);
    assert!(
        events
            .iter()
            .any(|e| matches!(e, ClaudeEvent::AwaitingPrompt)),
        "expected AwaitingPrompt once the project dir and file appeared, got {events:?}"
    );
}

/// The #362 repro itself: a watched directory disappears out from
/// under the live watch and comes back — on Windows,
/// `ReadDirectoryChangesW` on a deleted directory just stops
/// delivering, silently, so nothing short of a supervisor pass
/// noticing the identity changed will ever tail it again. Exercises
/// both the main transcript's parent and the subagents dir.
#[test]
fn tail_survives_watched_dir_deleted_and_recreated() {
    let dir = TempDir::new().unwrap();
    let parent = dir.path().join("proj");
    fs::create_dir_all(&parent).unwrap();
    let path = parent.join("session.jsonl");

    let (tx, rx) = mpsc::channel();
    let manager = test_manager();
    manager
        .attach_at(
            "h1",
            path.clone(),
            move |e| {
                let _ = tx.send(e);
            },
            None,
        )
        .unwrap();

    retry_fs_op(
        || fs::remove_dir_all(&parent),
        "remove_dir_all(main parent)",
    );
    retry_fs_op(
        || fs::create_dir_all(&parent),
        "create_dir_all(main parent)",
    );
    let mut f = fs::File::create(&path).unwrap();
    writeln!(
        f,
        r#"{{"type":"assistant","sessionId":"x","message":{{"stop_reason":"end_turn","content":[]}}}}"#
    )
    .unwrap();
    f.sync_all().unwrap();

    manager.supervise_once();
    let events = drain(&rx);
    assert!(
        events
            .iter()
            .any(|e| matches!(e, ClaudeEvent::AwaitingPrompt)),
        "tail did not survive the main parent dir being deleted and recreated, got {events:?}"
    );

    // Same story, one level down: the subagents dir.
    let sub_dir = skein_harness::claude::subagents_dir(&path).unwrap();
    fs::create_dir_all(&sub_dir).unwrap();
    manager.supervise_once();
    drain_brief(&rx); // nothing expected yet — just let the arm settle

    retry_fs_op(
        || fs::remove_dir_all(&sub_dir),
        "remove_dir_all(subagents dir)",
    );
    retry_fs_op(
        || fs::create_dir_all(&sub_dir),
        "create_dir_all(subagents dir)",
    );
    let mut sf = fs::File::create(sub_dir.join("agent-x.jsonl")).unwrap();
    writeln!(
        sf,
        r#"{{"type":"assistant","isSidechain":true,"agentId":"x","message":{{"stop_reason":"tool_use","content":[]}}}}"#
    )
    .unwrap();
    sf.sync_all().unwrap();

    manager.supervise_once();
    let events = drain(&rx);
    assert!(
        events
            .iter()
            .any(|e| matches!(e, ClaudeEvent::SubagentStart { agent_id, .. } if agent_id == "x")),
        "subagent tail did not survive its dir being deleted and recreated, got {events:?}"
    );
}

// ── #425: a reappearing transcript is never replayed as live ────

pub(super) const T425_PROMPT: &str =
    r#"{"type":"user","timestamp":"2026-05-15T21:16:21.000Z","message":{"content":"go"}}"#;

pub(super) const T425_TOOL: &str = r#"{"type":"assistant","uuid":"a1","timestamp":"2026-05-15T21:16:22.572Z","message":{"content":[{"type":"tool_use","id":"toolu_1","name":"Bash","input":{"command":"ls"}}]}}"#;

pub(super) const T425_RESULT: &str = r#"{"type":"user","timestamp":"2026-05-15T21:16:23.000Z","toolUseResult":{"stdout":"x"},"message":{"content":[{"type":"tool_result","tool_use_id":"toolu_1","content":"x","is_error":false}]}}"#;

pub(super) const T425_END: &str = r#"{"type":"assistant","timestamp":"2026-05-15T21:16:24.000Z","message":{"stop_reason":"end_turn","content":[{"type":"text","text":"done-A"}]}}"#;

const T425_NEW: &str =
    r#"{"type":"user","timestamp":"2026-05-15T21:16:30.000Z","message":{"content":"more"}}"#;

pub(super) fn t425_jsonl(rows: &[&str]) -> String {
    let mut out = rows.join("\n");
    out.push('\n');
    out
}

/// A never-attached state on `path` (file may or may not exist yet),
/// driven directly through `tick`.
pub(super) fn t425_state(path: &Path, actions: Option<ActionPersistence>) -> Arc<Mutex<TailState>> {
    Arc::new(Mutex::new(TailState {
        harness_id: "h1".into(),
        path: path.to_path_buf(),
        last_pos: 0,
        partial: String::new(),
        attached: false,
        ever_attached: false,
        fingerprint: Vec::new(),
        in_assistant_turn: false,
        local_command: LocalCommandTracker::default(),
        actions,
        subagents_dir: None,
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
    }))
}

pub(super) fn t425_tick(state: &Arc<Mutex<TailState>>) -> Vec<ClaudeEvent> {
    let out = std::sync::Mutex::new(Vec::new());
    tick(state, &|e| out.lock().unwrap().push(e));
    out.into_inner().unwrap()
}

pub(super) fn t425_persistence(db_path: &Path) -> (Arc<crate::db::Database>, ActionPersistence) {
    let db = Arc::new(crate::db::Database::open(db_path).unwrap());
    let ap = ActionPersistence {
        extractor: ActionExtractor::new(),
        db: Arc::clone(&db),
        harness_id: "h1".into(),
        room_id: "r1".into(),
        cwd: String::new(),
        app: None,
    };
    (db, ap)
}

/// Attach live on the base transcript, then make it vanish
/// (`SessionEnd`), leaving the state ready for a reappearance.
pub(super) fn t425_vanished(
    dir: &TempDir,
    with_db: bool,
) -> (
    Arc<Mutex<TailState>>,
    PathBuf,
    Option<Arc<crate::db::Database>>,
) {
    let path = dir.path().join("session.jsonl");
    let (db, ap) = if with_db {
        let (db, ap) = t425_persistence(&dir.path().join("t.db"));
        (Some(db), Some(ap))
    } else {
        (None, None)
    };
    let state = t425_state(&path, ap);
    fs::write(
        &path,
        t425_jsonl(&[T425_PROMPT, T425_TOOL, T425_RESULT, T425_END]),
    )
    .unwrap();
    let first = t425_tick(&state);
    assert_eq!(first.len(), 4, "first appearance reads live, got {first:?}");
    fs::remove_file(&path).unwrap();
    let ended = t425_tick(&state);
    assert!(matches!(ended.as_slice(), [ClaudeEvent::SessionEnd]));
    assert!(!state.lock().attached);
    (state, path, db)
}

#[test]
fn reappear_same_reads_only_the_new_rows() {
    let dir = TempDir::new().unwrap();
    let (state, path, db) = t425_vanished(&dir, true);
    let db = db.unwrap();
    let rows_before = db.recent_harness_actions_by_room("r1", -1, 100).unwrap();
    assert!(!rows_before.is_empty());

    fs::write(
        &path,
        t425_jsonl(&[T425_PROMPT, T425_TOOL, T425_RESULT, T425_END, T425_NEW]),
    )
    .unwrap();
    let events = t425_tick(&state);
    assert!(
        matches!(events.as_slice(), [ClaudeEvent::UserPrompt { .. }]),
        "only the appended row may be read live, got {events:?}"
    );
    let rows_after = db.recent_harness_actions_by_room("r1", -1, 100).unwrap();
    assert_eq!(
        rows_after.len(),
        rows_before.len(),
        "no duplicated harness_actions rows"
    );
    let len = fs::metadata(&path).unwrap().len();
    assert_eq!(state.lock().last_pos, len);
}

#[test]
fn reappear_truncated_resyncs_as_backfill() {
    let dir = TempDir::new().unwrap();
    let (state, path, db) = t425_vanished(&dir, true);
    let db = db.unwrap();
    let rows_before = db.recent_harness_actions_by_room("r1", -1, 100).unwrap();

    fs::write(&path, t425_jsonl(&[T425_PROMPT, T425_END])).unwrap();
    let events = t425_tick(&state);
    assert!(
        matches!(events.as_slice(), [ClaudeEvent::AwaitingPrompt]),
        "expected only the derived initial phase, got {events:?}"
    );
    let len = fs::metadata(&path).unwrap().len();
    {
        let s = state.lock();
        assert_eq!(s.last_pos, len);
        assert!(s.partial.is_empty());
    }
    assert_eq!(
        db.recent_harness_actions_by_room("r1", -1, 100)
            .unwrap()
            .len(),
        rows_before.len()
    );
    // Tailing continues normally from the resynced position.
    let mut f = fs::OpenOptions::new().append(true).open(&path).unwrap();
    writeln!(f, "{T425_NEW}").unwrap();
    f.sync_all().unwrap();
    let events = t425_tick(&state);
    assert!(matches!(
        events.as_slice(),
        [ClaudeEvent::UserPrompt { .. }]
    ));
}

#[test]
fn reappear_different_content_resyncs_as_backfill() {
    let dir = TempDir::new().unwrap();
    let (state, path, _db) = t425_vanished(&dir, true);
    let old_len = state.lock().last_pos;

    // Longer than before, but not the same bytes.
    let other_end = T425_END.replace("done-A", "done-B");
    let content = t425_jsonl(&[
        T425_PROMPT,
        T425_TOOL,
        T425_RESULT,
        T425_NEW,
        T425_PROMPT,
        &other_end,
    ]);
    assert!(content.len() as u64 > old_len);
    fs::write(&path, &content).unwrap();
    let events = t425_tick(&state);
    assert!(
        matches!(events.as_slice(), [ClaudeEvent::AwaitingPrompt]),
        "expected only the derived initial phase, got {events:?}"
    );
    assert_eq!(state.lock().last_pos, content.len() as u64);
}

/// #428: the stat-skip (`len == last_pos`) sits after the reappear
/// classification, so a recreated file of exactly the same length
/// but different bytes is still resynced, never skipped.
#[test]
fn reappear_same_length_different_content_is_not_stat_skipped() {
    let dir = TempDir::new().unwrap();
    let (state, path, _db) = t425_vanished(&dir, true);
    let old_len = state.lock().last_pos;

    let other_end = T425_END.replace("done-A", "done-B");
    let content = t425_jsonl(&[T425_PROMPT, T425_TOOL, T425_RESULT, &other_end]);
    assert_eq!(content.len() as u64, old_len, "fixture must be same length");
    fs::write(&path, &content).unwrap();
    let events = t425_tick(&state);
    assert!(
        matches!(events.as_slice(), [ClaudeEvent::AwaitingPrompt]),
        "expected only the derived initial phase, got {events:?}"
    );
    let s = state.lock();
    assert!(s.attached);
    assert_eq!(s.last_pos, content.len() as u64);
    let mut expected = Vec::new();
    extend_fingerprint(&mut expected, content.as_bytes());
    assert_eq!(s.fingerprint, expected);
}

/// A partial line carried across the vanish is completed by the
/// bytes appended to the recreated (same) file: read live, once.
#[test]
fn reappear_same_completes_a_carried_partial_line() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("session.jsonl");
    let state = t425_state(&path, None);
    let (head, tail) = T425_END.split_at(40);
    let before = format!(
        "{T425_PROMPT}
{head}"
    );
    fs::write(&path, &before).unwrap();
    let first = t425_tick(&state);
    assert!(matches!(first.as_slice(), [ClaudeEvent::UserPrompt { .. }]));
    assert!(!state.lock().partial.is_empty());
    fs::remove_file(&path).unwrap();
    assert!(matches!(
        t425_tick(&state).as_slice(),
        [ClaudeEvent::SessionEnd]
    ));

    fs::write(
        &path,
        format!(
            "{before}{tail}
"
        ),
    )
    .unwrap();
    let events = t425_tick(&state);
    assert!(
        matches!(events.as_slice(), [ClaudeEvent::AwaitingPrompt]),
        "the completed row is new and read live once, got {events:?}"
    );
}

/// A file that exists at attach but isn't valid UTF-8 yet must not
/// be replayed from 0 as live once it becomes readable.
#[test]
fn unreadable_at_attach_does_not_replay_once_readable() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("session.jsonl");
    let mut bad = t425_jsonl(&[T425_PROMPT, T425_TOOL]).into_bytes();
    bad.extend_from_slice(&[0xff, 0xfe]);
    fs::write(&path, &bad).unwrap();

    let (tx, rx) = mpsc::channel();
    let manager = test_manager();
    manager
        .attach_at(
            "h1",
            path.clone(),
            move |e| {
                let _ = tx.send(e);
            },
            None,
        )
        .unwrap();
    drain_brief(&rx);

    fs::write(
        &path,
        t425_jsonl(&[T425_PROMPT, T425_TOOL, T425_RESULT, T425_END]),
    )
    .unwrap();
    let events = drain(&rx);
    assert!(
        events
            .iter()
            .all(|e| matches!(e, ClaudeEvent::AwaitingPrompt)),
        "history must not replay as live events, got {events:?}"
    );
}

#[test]
fn fingerprint_keeps_the_last_256_bytes_across_short_reads() {
    let mut fp = fingerprint_of(&[1u8; 300]);
    assert_eq!(fp.len(), FINGERPRINT_LEN);
    extend_fingerprint(&mut fp, &[2u8; 10]);
    assert_eq!(fp.len(), FINGERPRINT_LEN);
    assert_eq!(&fp[FINGERPRINT_LEN - 10..], &[2u8; 10]);
    assert_eq!(fp[0], 1);
    let mut small = Vec::new();
    extend_fingerprint(&mut small, b"ab");
    extend_fingerprint(&mut small, b"cd");
    assert_eq!(small, b"abcd");
}

/// While attached, a shrinking transcript is resynced, not replayed.
#[test]
fn attached_shrink_resyncs_without_replay() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("session.jsonl");
    let state = t425_state(&path, None);
    fs::write(
        &path,
        t425_jsonl(&[T425_PROMPT, T425_TOOL, T425_RESULT, T425_END]),
    )
    .unwrap();
    assert_eq!(t425_tick(&state).len(), 4);
    fs::write(&path, t425_jsonl(&[T425_PROMPT, T425_END])).unwrap();
    let events = t425_tick(&state);
    assert!(matches!(events.as_slice(), [ClaudeEvent::AwaitingPrompt]));
    assert_eq!(state.lock().last_pos, fs::metadata(&path).unwrap().len());
}

/// A subagent transcript that shrinks is re-seeded like at attach:
/// nothing emitted, `last_pos` at the new EOF.
#[test]
fn subagent_shrink_reseeds_without_replay() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("session.jsonl");
    fs::write(&path, "").unwrap();
    let sub_dir = dir.path().join("subagents");
    fs::create_dir_all(&sub_dir).unwrap();
    let sub_path = sub_dir.join("agent-x.jsonl");
    let content = concat!(
        r#"{"type":"assistant","isSidechain":true,"agentId":"x","message":{"stop_reason":"end_turn","content":[]}}"#,
        "\n"
    );
    fs::write(&sub_path, content).unwrap();

    let state = t425_state(&path, None);
    {
        let mut s = state.lock();
        s.attached = true;
        s.ever_attached = true;
        s.subagents_dir = Some(sub_dir);
        s.subagents.insert(
            "x".into(),
            SubagentTail {
                path: sub_path,
                last_pos: 10_000,
                partial: "junk".into(),
                lifecycle: skein_harness::claude::SubagentLifecycle::default(),
                agent_type: None,
                description: None,
                started_ms: None,
                started_ms_resolved: true,
                last_ts_ms: 0,
                open_failure_logged: false,
            },
        );
    }
    let events = t425_tick(&state);
    assert!(events.is_empty(), "no replay, got {events:?}");
    let s = state.lock();
    let t = s.subagents.get("x").unwrap();
    assert_eq!(t.last_pos, content.len() as u64);
    assert!(t.partial.is_empty());
    assert_eq!(
        t.lifecycle.is_finished(),
        skein_harness::claude::subagent_transcript_is_finished(content)
    );
}
