//! A subagent transcript writes `api_usage` rows for the delegating harness (#341).

use super::resync_tests::{t425_jsonl, t425_persistence, t425_state, t425_tick};
use parking_lot::Mutex;
use std::fs;
use std::sync::Arc;
use tempfile::TempDir;

use super::adapter::TailState;

fn chunk(msg: &str, out: u64, stop: &str, ts: &str) -> String {
    format!(
        r#"{{"type":"assistant","isSidechain":true,"agentId":"a1","requestId":"req_{msg}","timestamp":"{ts}","message":{{"id":"{msg}","model":"claude-x","stop_reason":"{stop}","content":[],"usage":{{"input_tokens":10,"output_tokens":{out},"cache_read_input_tokens":3}}}}}}"#
    )
}

const TOOL_RESULT: &str = r#"{"type":"user","isSidechain":true,"agentId":"a1","timestamp":"2026-05-15T21:16:22.000Z","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t1","content":"ok"}]}}"#;

fn usage_rows(db: &crate::db::Database) -> Vec<serde_json::Value> {
    db.recent_harness_actions_by_harness("h1", -1, 100)
        .unwrap()
        .iter()
        .filter(|a| a.kind == crate::db::action_kind::API_USAGE)
        .map(|a| serde_json::from_str(&a.payload).unwrap())
        .collect()
}

fn armed_state(
    dir: &TempDir,
) -> (
    Arc<Mutex<TailState>>,
    Arc<crate::db::Database>,
    std::path::PathBuf,
) {
    let path = dir.path().join("session.jsonl");
    fs::write(&path, "").unwrap();
    let sub_dir = skein_harness::claude::subagents_dir(&path).unwrap();
    fs::create_dir_all(&sub_dir).unwrap();
    let (db, ap) = t425_persistence(&dir.path().join("t.db"));
    let state = t425_state(&path, Some(ap));
    {
        let mut s = state.lock();
        s.attached = true;
        s.ever_attached = true;
        s.subagents_dir = Some(sub_dir.clone());
    }
    (state, db, sub_dir)
}

#[test]
fn subagent_responses_become_api_usage_rows_for_the_parent_harness() {
    let dir = TempDir::new().unwrap();
    let (state, db, sub_dir) = armed_state(&dir);
    fs::write(
        sub_dir.join("agent-a1.meta.json"),
        r#"{"agentType":"explore","description":"Map the tailer"}"#,
    )
    .unwrap();
    let rows = [
        chunk("msg_1", 6, "tool_use", "2026-05-15T21:16:20.000Z"),
        chunk("msg_1", 6, "tool_use", "2026-05-15T21:16:20.100Z"),
        chunk("msg_1", 185, "tool_use", "2026-05-15T21:16:21.000Z"),
        TOOL_RESULT.to_owned(),
        chunk("msg_2", 40, "end_turn", "2026-05-15T21:16:25.000Z"),
    ];
    let refs: Vec<&str> = rows.iter().map(String::as_str).collect();
    let sub_path = sub_dir.join("agent-a1.jsonl");
    fs::write(&sub_path, t425_jsonl(&refs)).unwrap();

    t425_tick(&state);
    assert_eq!(
        usage_rows(&db).len(),
        1,
        "trailing response waits for a quiet tick"
    );
    t425_tick(&state);
    let got = usage_rows(&db);
    assert_eq!(got.len(), 2, "{got:?}");
    for p in &got {
        assert_eq!(p["agent_id"], "a1");
        assert_eq!(p["agent_type"], "explore");
    }
    let outputs: Vec<i64> = got
        .iter()
        .map(|p| p["usage"]["output"].as_i64().unwrap())
        .collect();
    assert!(outputs.contains(&185), "{got:?}");
    assert!(
        outputs.contains(&40),
        "trailing response via finish: {got:?}"
    );
    let rows_all = db.recent_harness_actions_by_harness("h1", -1, 100).unwrap();
    assert!(
        rows_all
            .iter()
            .filter(|a| a.kind == crate::db::action_kind::API_USAGE)
            .all(|a| a.harness_id == "h1")
    );

    // Ticking again, and re-reading the file from the start, adds nothing.
    t425_tick(&state);
    state.lock().subagents.get_mut("a1").unwrap().last_pos = 0;
    t425_tick(&state);
    assert_eq!(usage_rows(&db).len(), 2);
}

/// The last response's chunks straddle two ticks: it is closed only on the
/// quiet tick after, so it is counted once with the final output.
#[test]
fn trailing_response_split_across_ticks_is_one_row_with_final_output() {
    let dir = TempDir::new().unwrap();
    let (state, db, sub_dir) = armed_state(&dir);
    let sub_path = sub_dir.join("agent-a1.jsonl");
    fs::write(
        &sub_path,
        t425_jsonl(&[&chunk("msg_9", 6, "end_turn", "2026-05-15T21:16:20.000Z")]),
    )
    .unwrap();
    t425_tick(&state);
    assert_eq!(
        usage_rows(&db).len(),
        0,
        "a tick that read rows closes nothing"
    );
    let mut f = fs::OpenOptions::new().append(true).open(&sub_path).unwrap();
    std::io::Write::write_all(
        &mut f,
        format!(
            "{}
",
            chunk("msg_9", 40, "end_turn", "2026-05-15T21:16:21.000Z")
        )
        .as_bytes(),
    )
    .unwrap();
    drop(f);
    t425_tick(&state);
    t425_tick(&state);
    let got = usage_rows(&db);
    assert_eq!(got.len(), 1, "{got:?}");
    assert_eq!(got[0]["usage"]["output"], 40);
}
