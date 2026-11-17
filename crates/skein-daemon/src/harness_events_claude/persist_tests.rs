//! Action persistence and backfill on attach and on the live tail (#80).

use super::adapter::ActionPersistence;
use super::tests::wait_for;
use super::*;
use crate::harness_actions_claude::ActionExtractor;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tempfile::TempDir;

// ── action persistence + backfill (issue #80) ────────────────

/// Build a manager wired to a real on-disk Database in `dir`, and
/// return the path used so the caller can re-open it. Action sink
/// is enabled — call sites populate JSONL via `attach_at` with a
/// `Some(ActionPersistence)`.
pub(super) fn make_persisting_adapter(
    jsonl: PathBuf,
    db_path: &Path,
    harness_id: &str,
    room_id: &str,
) -> ClaudeEventsManager {
    make_persisting_adapter_with_events(jsonl, db_path, harness_id, room_id).0
}

/// Like `make_persisting_adapter`, also handing back the event
/// stream so a test can wait on events instead of sleeping.
pub(super) fn make_persisting_adapter_with_events(
    jsonl: PathBuf,
    db_path: &Path,
    harness_id: &str,
    room_id: &str,
) -> (ClaudeEventsManager, std::sync::mpsc::Receiver<ClaudeEvent>) {
    let (tx, rx) = std::sync::mpsc::channel();
    let db = Arc::new(crate::db::Database::open(db_path).unwrap());
    let manager = ClaudeEventsManager::new_for_test(Arc::clone(&db));
    let persistence = Some(ActionPersistence {
        extractor: ActionExtractor::new(),
        db,
        harness_id: harness_id.into(),
        room_id: room_id.into(),
        // Phase/action tests only; note_patch no-ops on an empty cwd.
        cwd: String::new(),
        events: None,
    });
    manager
        .attach_at(
            harness_id,
            jsonl,
            move |event| {
                let _ = tx.send(event);
            },
            persistence,
        )
        .unwrap();
    (manager, rx)
}

/// `Tool_use` + `tool_result` rows already in the file at attach
/// time are backfilled into `harness_actions` on first attach.
#[test]
fn backfill_persists_existing_tool_calls() {
    let dir = TempDir::new().unwrap();
    let jsonl = dir.path().join("session.jsonl");
    let db_path = dir.path().join("test.db");

    // Pre-seed the JSONL with a tool_use + result pair.
    {
        let mut f = std::fs::File::create(&jsonl).unwrap();
        writeln!(
            f,
            r#"{{"type":"assistant","uuid":"a1","timestamp":"2026-05-15T21:16:22.572Z","message":{{"content":[{{"type":"tool_use","id":"toolu_1","name":"Bash","input":{{"command":"ls"}}}}]}}}}"#
        ).unwrap();
        writeln!(
            f,
            r#"{{"type":"user","timestamp":"2026-05-15T21:16:23.000Z","toolUseResult":{{"stdout":"x"}},"message":{{"content":[{{"type":"tool_result","tool_use_id":"toolu_1","content":"x","is_error":false}}]}}}}"#
        ).unwrap();
        f.sync_all().unwrap();
    }

    let _manager = make_persisting_adapter(jsonl, &db_path, "h1", "r1");

    let db = crate::db::Database::open(&db_path).unwrap();
    let actions = db.recent_harness_actions_by_room("r1", -1, 100).unwrap();
    assert_eq!(actions.len(), 1, "expected 1 backfilled action");
    let a = &actions[0];
    assert_eq!(a.kind, crate::db::action_kind::TOOL_CALL);
    assert_eq!(a.harness_id, "h1");
    let payload: serde_json::Value = serde_json::from_str(&a.payload).unwrap();
    assert_eq!(payload["tool"], "Bash");
}

/// Re-attaching to the same session (Skein restart) does not
/// re-insert rows already in `harness_actions`. Only rows with
/// a strictly newer `timestamp_ms` are persisted.
#[test]
fn second_attach_skips_already_persisted_rows() {
    let dir = TempDir::new().unwrap();
    let jsonl = dir.path().join("session.jsonl");
    let db_path = dir.path().join("test.db");

    {
        let mut f = std::fs::File::create(&jsonl).unwrap();
        writeln!(
            f,
            r#"{{"type":"assistant","uuid":"a1","timestamp":"2026-05-15T21:16:22.572Z","message":{{"content":[{{"type":"tool_use","id":"toolu_1","name":"Bash","input":{{"command":"ls"}}}}]}}}}"#
        ).unwrap();
        writeln!(
            f,
            r#"{{"type":"user","timestamp":"2026-05-15T21:16:23.000Z","toolUseResult":{{"stdout":"x"}},"message":{{"content":[{{"type":"tool_result","tool_use_id":"toolu_1","content":"x","is_error":false}}]}}}}"#
        ).unwrap();
        f.sync_all().unwrap();
    }

    // First attach: backfill = 1 row.
    let manager1 = make_persisting_adapter(jsonl.clone(), &db_path, "h1", "r1");
    drop(manager1);

    // Re-attach to same JSONL + same DB. Should be a no-op for actions.
    let _manager2 = make_persisting_adapter(jsonl, &db_path, "h1", "r1");

    let db = crate::db::Database::open(&db_path).unwrap();
    let actions = db.recent_harness_actions_by_room("r1", -1, 100).unwrap();
    assert_eq!(actions.len(), 1, "expected no duplicate on re-attach");
}

/// Rows that appear in the file after the first backfill (Skein
/// closed, Claude wrote more, Skein re-opens) ARE persisted.
#[test]
fn second_attach_picks_up_rows_added_while_skein_was_down() {
    let dir = TempDir::new().unwrap();
    let jsonl = dir.path().join("session.jsonl");
    let db_path = dir.path().join("test.db");

    // First batch (before Skein "closes").
    {
        let mut f = std::fs::File::create(&jsonl).unwrap();
        writeln!(
            f,
            r#"{{"type":"assistant","uuid":"a1","timestamp":"2026-05-15T21:16:22.572Z","message":{{"content":[{{"type":"tool_use","id":"toolu_1","name":"Read","input":{{"file_path":"/x"}}}}]}}}}"#
        ).unwrap();
        writeln!(
            f,
            r#"{{"type":"user","timestamp":"2026-05-15T21:16:23.000Z","toolUseResult":{{"file":"x","type":"file"}},"message":{{"content":[{{"type":"tool_result","tool_use_id":"toolu_1","content":"x","is_error":false}}]}}}}"#
        ).unwrap();
        f.sync_all().unwrap();
    }
    let manager1 = make_persisting_adapter(jsonl.clone(), &db_path, "h1", "r1");
    drop(manager1);

    // Append a second tool call (Skein was down, Claude kept working).
    {
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(&jsonl)
            .unwrap();
        writeln!(
            f,
            r#"{{"type":"assistant","uuid":"a2","timestamp":"2026-05-15T21:17:00.000Z","message":{{"content":[{{"type":"tool_use","id":"toolu_2","name":"Bash","input":{{"command":"pwd"}}}}]}}}}"#
        ).unwrap();
        writeln!(
            f,
            r#"{{"type":"user","timestamp":"2026-05-15T21:17:01.000Z","toolUseResult":{{"stdout":"/foo"}},"message":{{"content":[{{"type":"tool_result","tool_use_id":"toolu_2","content":"/foo","is_error":false}}]}}}}"#
        ).unwrap();
        f.sync_all().unwrap();
    }

    let _manager2 = make_persisting_adapter(jsonl, &db_path, "h1", "r1");
    let db = crate::db::Database::open(&db_path).unwrap();
    let actions = db.recent_harness_actions_by_room("r1", -1, 100).unwrap();
    assert_eq!(
        actions.len(),
        2,
        "expected backfill to pick up only the new row"
    );
    // Newest first.
    let p0: serde_json::Value = serde_json::from_str(&actions[0].payload).unwrap();
    let p1: serde_json::Value = serde_json::from_str(&actions[1].payload).unwrap();
    assert_eq!(p0["tool"], "Bash");
    assert_eq!(p1["tool"], "Read");
}

/// Each row that arrives on the live tail (after attach) ALSO
/// gets persisted. This is the steady-state path: Claude writes
/// a new row, notify fires, tick reads + extracts + persists.
#[test]
fn live_tail_persists_rows_appended_after_attach() {
    let dir = TempDir::new().unwrap();
    let jsonl = dir.path().join("session.jsonl");
    let db_path = dir.path().join("test.db");

    // File doesn't exist yet — adapter will attach via parent-dir
    // watcher and start at byte 0 on create.
    let _manager = make_persisting_adapter(jsonl.clone(), &db_path, "h1", "r1");

    // Create + append a tool_use/result pair.
    {
        let mut f = std::fs::File::create(&jsonl).unwrap();
        writeln!(
            f,
            r#"{{"type":"assistant","uuid":"a1","timestamp":"2026-05-15T21:16:22.572Z","message":{{"content":[{{"type":"tool_use","id":"toolu_1","name":"Bash","input":{{"command":"echo hi"}}}}]}}}}"#
        ).unwrap();
        writeln!(
            f,
            r#"{{"type":"user","timestamp":"2026-05-15T21:16:23.000Z","toolUseResult":{{"stdout":"hi"}},"message":{{"content":[{{"type":"tool_result","tool_use_id":"toolu_1","content":"hi","is_error":false}}]}}}}"#
        ).unwrap();
        f.sync_all().unwrap();
    }

    // FSEvents can take up to ~1-2 s on macOS to deliver, and longer
    // under CPU load: wait on the row, not on a clock.
    let db = crate::db::Database::open(&db_path).unwrap();
    wait_for(
        || {
            !db.recent_harness_actions_by_room("r1", -1, 100)
                .unwrap()
                .is_empty()
        },
        "the live tail to persist the action",
    );
    let actions = db.recent_harness_actions_by_room("r1", -1, 100).unwrap();
    assert_eq!(actions[0].kind, crate::db::action_kind::TOOL_CALL);
}

// ── api_usage idempotency (#341) ──────────────────────────────

fn usage(ts: i64, source: &str) -> crate::harness_actions_claude::ExtractedAction {
    crate::harness_actions_claude::ExtractedAction {
        kind: crate::db::action_kind::API_USAGE,
        timestamp_ms: ts,
        payload: "{}".into(),
        source: Some(source.into()),
    }
}

fn persistence(db: Arc<crate::db::Database>) -> ActionPersistence {
    ActionPersistence {
        extractor: ActionExtractor::new(),
        db,
        harness_id: "h1".into(),
        room_id: "r1".into(),
        cwd: String::new(),
        events: None,
    }
}

/// A response older than the watermark lands once, and replaying the
/// same batch (a re-attach or resync) adds nothing.
#[test]
fn api_usage_older_than_watermark_lands_once_and_replays_add_nothing() {
    let dir = TempDir::new().unwrap();
    let db = Arc::new(crate::db::Database::open(&dir.path().join("t.db")).unwrap());
    db.record_harness_action("h1", "r1", 1_000, "tool_call", "{}", None, None)
        .unwrap();
    assert_eq!(super::persist::max_persisted_ts_ms(&db, "h1"), 1_000);
    let ap = persistence(Arc::clone(&db));
    for _ in 0..2 {
        super::persist::persist_extracted_batch(
            &ap,
            vec![usage(500, "msg_a"), usage(600, "msg_b")],
        );
    }
    let rows = db
        .recent_harness_actions_by_room_and_kind("r1", crate::db::action_kind::API_USAGE, 0, 10)
        .unwrap();
    assert_eq!(rows.len(), 2);
}

/// An `api_usage` row newer than the last real row must not raise the
/// watermark, or older unpersisted main-transcript rows never backfill.
#[test]
fn api_usage_row_does_not_raise_the_backfill_watermark() {
    let dir = TempDir::new().unwrap();
    let db = Arc::new(crate::db::Database::open(&dir.path().join("t.db")).unwrap());
    db.record_harness_action("h1", "r1", 1_000, "tool_call", "{}", None, None)
        .unwrap();
    let ap = persistence(Arc::clone(&db));
    super::persist::persist_extracted_batch(&ap, vec![usage(5_000, "msg_sub")]);
    assert_eq!(super::persist::max_persisted_ts_ms(&db, "h1"), 1_000);
    let older_main = crate::harness_actions_claude::ExtractedAction {
        kind: "tool_call",
        timestamp_ms: 2_000,
        payload: "{}".into(),
        source: None,
    };
    super::persist::persist_extracted_batch(&ap, vec![older_main]);
    assert_eq!(super::persist::max_persisted_ts_ms(&db, "h1"), 2_000);
}

/// The live path reports nothing new for an ignored duplicate.
#[test]
fn live_persist_ignores_duplicate_api_usage() {
    let dir = TempDir::new().unwrap();
    let db = Arc::new(crate::db::Database::open(&dir.path().join("t.db")).unwrap());
    let ap = persistence(Arc::clone(&db));
    super::persist::persist_extracted(&ap, vec![usage(10, "msg_a"), usage(10, "msg_a")], true);
    let rows = db.recent_harness_actions_by_harness("h1", -1, 10).unwrap();
    assert_eq!(rows.len(), 1);
}
