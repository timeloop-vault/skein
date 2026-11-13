//! The acceptance test of #731: a daemon with no client starts a harness
//! from its record and the Claude adapter writes `harness_actions` rows.

use std::io::Write;

use super::*;

const SESSION: &str = "7f3c0a52-0000-4000-8000-000000000731";

fn append(path: &Path, lines: &[&str]) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .unwrap();
    let mut block = String::new();
    for l in lines {
        block.push_str(l);
        block.push('\n');
    }
    f.write_all(block.as_bytes()).unwrap();
    f.sync_all().unwrap();
}

#[test]
fn a_daemon_with_no_client_runs_a_harness_and_records_its_actions() {
    let fx = Fx::new();
    fx.create(room(
        "r1",
        &fx.work,
        vec![in_shell(harness(
            "h1",
            "claude",
            Some(shell_cmd()),
            Some(SESSION),
        ))],
    ));

    // No sink, no view, no frontend: the daemon starts it from the record.
    let out = fx.ensure(false, None).expect("ensure_running");
    assert!(out.started);
    assert!(out.exit.is_none());
    let pty_id = fx
        .runner()
        .pty_id("h1")
        .expect("the runner has the process");
    assert!(fx.daemon.pty.is_alive(&pty_id));

    // The transcript the adapter tails lives under the redirected home.
    let work = fx.work.to_string_lossy().into_owned();
    let transcript = skein_harness::claude::session_jsonl_path(&fx.home, &work, SESSION);
    assert!(transcript.starts_with(&fx.home));
    append(
        &transcript,
        &[
            r#"{"type":"assistant","uuid":"a1","timestamp":"2026-05-15T21:16:22.572Z","message":{"content":[{"type":"tool_use","id":"toolu_1","name":"Bash","input":{"command":"echo hi"}}]}}"#,
            r#"{"type":"user","timestamp":"2026-05-15T21:16:23.000Z","toolUseResult":{"stdout":"hi"},"message":{"content":[{"type":"tool_result","tool_use_id":"toolu_1","content":"hi","is_error":false}]}}"#,
        ],
    );

    let rows = || {
        fx.daemon
            .db
            .recent_harness_actions_by_harness("h1", -1, 100)
            .unwrap()
    };
    assert!(
        wait_until(|| !rows().is_empty()),
        "no harness_actions row appeared for the harness"
    );
    let rows = rows();
    assert_eq!(rows[0].kind, crate::db::action_kind::TOOL_CALL);
    assert_eq!(rows[0].harness_id, "h1");
    assert_eq!(rows[0].room_id, "r1");
    let payload: Value = serde_json::from_str(&rows[0].payload).unwrap();
    assert_eq!(payload["tool"], "Bash");
}
