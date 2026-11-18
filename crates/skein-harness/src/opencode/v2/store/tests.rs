//! Pinned on the opencode 2.3.2 captures in `../fixtures/store/`.

use rusqlite::Connection;
use serde_json::Value;

use super::*;

pub(crate) const MSGS_A: &str = include_str!("../fixtures/store/messages_A.jsonl");
pub(crate) const MSGS_B: &str = include_str!("../fixtures/store/messages_B.jsonl");
const MSGS_C: &str = include_str!("../fixtures/store/messages_C.jsonl");
const MSGS_D: &str = include_str!("../fixtures/store/messages_D.jsonl");
const MSGS_E: &str = include_str!("../fixtures/store/messages_E.jsonl");
const MSGS_F: &str = include_str!("../fixtures/store/messages_F.jsonl");
const MSGS_F_CHILD: &str = include_str!("../fixtures/store/messages_F_child.jsonl");
const SCHEMA: &str = include_str!("../fixtures/store/schema.sql");

/// Parses every row of a captured session the way the reader does.
pub(crate) fn rows(raw: &str) -> Vec<(Value, StoredMessage)> {
    raw.lines()
        .map(|l| {
            let row: Value = serde_json::from_str(l).expect("fixture line is JSON");
            let kind = parse_message(row["type"].as_str().unwrap(), &row["data"]);
            let msg = StoredMessage {
                id: row["id"].as_str().unwrap().to_owned(),
                seq: row["seq"].as_i64().unwrap(),
                time_created: row["time_created"].as_i64().unwrap(),
                kind,
            };
            (row, msg)
        })
        .collect()
}

pub(crate) fn steps(raw: &str) -> Vec<(String, AssistantStep)> {
    rows(raw)
        .into_iter()
        .filter_map(|(_, m)| match m.kind {
            MessageKind::Assistant(s) => Some((m.id, s)),
            _ => None,
        })
        .collect()
}

pub(crate) fn tools(step: &AssistantStep) -> Vec<&ToolItem> {
    step.content
        .iter()
        .filter_map(|c| match c {
            ContentItem::Tool(t) => Some(t),
            _ => None,
        })
        .collect()
}

#[test]
fn no_known_type_reads_as_other() {
    for raw in [MSGS_A, MSGS_B, MSGS_C, MSGS_D, MSGS_E, MSGS_F, MSGS_F_CHILD] {
        for (row, m) in rows(raw) {
            assert!(
                !matches!(m.kind, MessageKind::Other { .. }),
                "row {} of type {} fell through",
                m.id,
                row["type"]
            );
        }
    }
}

#[test]
fn user_text_and_idle_outcome() {
    let r = rows(MSGS_B);
    assert_eq!(r[0].1.seq, 4);
    assert_eq!(r[0].1.time_created, 1_795_006_713_861);
    let MessageKind::User { text } = &r[0].1.kind else {
        panic!("first row is the user prompt");
    };
    assert!(text.starts_with("Read the file notes.txt"));
    assert_eq!(
        r.last().unwrap().1.kind,
        MessageKind::Idle {
            outcome: Some("succeeded".into())
        }
    );
}

#[test]
fn step_tokens_cost_finish_and_model() {
    let s = steps(MSGS_B);
    assert_eq!(s.len(), 2);
    let first = &s[0].1;
    assert_eq!(first.finish.as_deref(), Some("tool-calls"));
    assert_eq!(first.raw_finish.as_deref(), Some("tool_calls"));
    assert_eq!(
        first.tokens,
        Tokens {
            input: 5431,
            output: 79,
            reasoning: 0,
            cache_read: 3328,
            cache_write: 0
        }
    );
    assert!(first.cost.abs() < f64::EPSILON);
    assert_eq!(first.model_id.as_deref(), Some("deepseek-v4.1-flash-free"));
    assert_eq!(first.provider_id.as_deref(), Some("opencode"));
    assert_eq!(first.agent.as_deref(), Some("build"));
    assert_eq!(first.created, Some(1_795_006_713_872));
    assert_eq!(first.completed, Some(1_795_006_718_756));
    assert_eq!(s[1].1.finish.as_deref(), Some("stop"));
}

#[test]
fn read_and_shell_tools() {
    let s = steps(MSGS_B);
    let t = tools(&s[0].1);
    assert_eq!(t.len(), 2);
    assert_eq!(t[0].name, "read");
    assert_eq!(t[0].id, "call_7ded820d7bc045a990a3922d");
    assert_eq!(t[0].status, ToolStatus::Completed);
    assert_eq!(t[0].input["path"], "notes.txt");
    assert_eq!(
        t[0].result.as_deref(),
        Some("Read file notes.txt, lines 1-3\n1: line1\n2: line2\n3: line3")
    );
    assert_eq!(t[1].name, "shell");
    assert_eq!(t[1].input["command"], "echo done-b");
    assert_eq!(t[1].result.as_deref(), Some("done-b\r\n"));
    assert_eq!(t[1].metadata["exit"], 0);
    assert_eq!(t[1].ran, Some(1_795_006_716_176));
    assert_eq!(
        s[0].1.content[0],
        ContentItem::Text("I'll read the file and run the command in parallel.".into())
    );
}

#[test]
fn edit_patch_is_reachable_and_write_has_content() {
    let s = steps(MSGS_C);
    let all: Vec<&ToolItem> = s.iter().flat_map(|(_, st)| tools(st)).collect();
    let edit = all.iter().find(|t| t.name == "edit").expect("edit");
    let patch = edit.metadata["files"][0]["patch"].as_str().unwrap();
    assert!(patch.contains("-hello world\n+hello skein"));
    assert_eq!(edit.metadata["files"][0]["file"], "a.txt");
    assert_eq!(edit.input["newString"], "hello skein");
    let write = all.iter().find(|t| t.name == "write").expect("write");
    assert_eq!(write.input["path"], "new.txt");
    assert_eq!(write.input["content"], "alpha\nbeta\n");
    assert_eq!(
        write.result.as_deref(),
        Some("Created file successfully: new.txt")
    );
}

#[test]
fn reasoning_item() {
    let s = steps(MSGS_C);
    let r = s[0]
        .1
        .content
        .iter()
        .find_map(|c| match c {
            ContentItem::Reasoning {
                text,
                created,
                completed,
            } => Some((text.clone(), *created, *completed)),
            _ => None,
        })
        .expect("reasoning item");
    assert_eq!(
        r,
        (
            "Let me check a.txt exists.".to_owned(),
            Some(1_795_006_725_657),
            Some(1_795_006_725_909)
        )
    );
    // Scenario D is reasoning plus text only.
    let d = steps(MSGS_D);
    assert!(d.iter().any(|(_, st)| {
        st.content
            .iter()
            .any(|c| matches!(c, ContentItem::Reasoning { .. }))
    }));
    assert!(d.iter().all(|(_, st)| tools(st).is_empty()));
}

#[test]
fn failing_tool_has_error_status_and_message() {
    let s = steps(MSGS_E);
    let t = tools(&s[0].1);
    assert_eq!(t.len(), 1);
    assert_eq!(t[0].status, ToolStatus::Error);
    assert_eq!(
        t[0].error.as_deref(),
        Some("File not found: does-not-exist.txt")
    );
    assert_eq!(t[0].result, None);
    assert_eq!(t[0].input["path"], "does-not-exist.txt");
}

#[test]
fn subagent_hand_back_is_synthetic() {
    let r = rows(MSGS_F);
    let syn = r
        .iter()
        .find_map(|(_, m)| match &m.kind {
            MessageKind::Synthetic(s) => Some((m.seq, s)),
            _ => None,
        })
        .expect("synthetic row");
    assert_eq!(syn.0, 24);
    assert_eq!(
        syn.1.child_id.as_deref(),
        Some("ses_e11422fd4ffeQ2X4Ze2OS4FGCk")
    );
    assert_eq!(
        syn.1.description.as_deref(),
        Some("List files and count notes.txt lines")
    );
    assert!(
        syn.1
            .text
            .as_deref()
            .unwrap()
            .contains("Exact line count: 3")
    );
}

#[test]
fn unknown_type_and_sparse_data_are_lenient() {
    let v: Value = serde_json::json!({"surprise": 1});
    assert_eq!(
        parse_message("future", &v),
        MessageKind::Other {
            kind: "future".into()
        }
    );
    let MessageKind::Assistant(s) = parse_message("assistant", &v) else {
        panic!("assistant")
    };
    assert_eq!(s, AssistantStep::default());
}

fn db_with(session_rows: &[(&str, &str)]) -> Connection {
    let start = SCHEMA.find("CREATE TABLE `session_message`").unwrap();
    let end = start + SCHEMA[start..].find(");").unwrap() + 2;
    let conn = Connection::open_in_memory().unwrap();
    // The captured FK points at session_v2, which this test does not create.
    conn.execute_batch("PRAGMA foreign_keys = OFF;").unwrap();
    conn.execute_batch(&SCHEMA[start..end]).unwrap();
    for (_, raw) in session_rows {
        for l in raw.lines() {
            let r: Value = serde_json::from_str(l).unwrap();
            conn.execute(
                "INSERT INTO session_message (id, session_id, type, seq, time_created, time_updated, data) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                rusqlite::params![
                    r["id"].as_str().unwrap(),
                    r["session_id"].as_str().unwrap(),
                    r["type"].as_str().unwrap(),
                    r["seq"].as_i64().unwrap(),
                    r["time_created"].as_i64().unwrap(),
                    r["time_updated"].as_i64().unwrap(),
                    r["data"].to_string(),
                ],
            )
            .unwrap();
        }
    }
    conn
}

#[test]
fn reads_back_in_seq_order_and_matches_direct_parse() {
    // Insert in reverse so ORDER BY is what puts them right.
    let reversed: String = MSGS_B.lines().rev().collect::<Vec<_>>().join("\n");
    let conn = db_with(&[("B", &reversed), ("C", MSGS_C)]);
    let sid = "ses_e114330feffek27Wru67RSzCAQ";
    let got = session_messages(&conn, sid).unwrap();
    let want: Vec<StoredMessage> = rows(MSGS_B).into_iter().map(|(_, m)| m).collect();
    assert_eq!(got, want);
    assert_eq!(
        got.iter().map(|m| m.seq).collect::<Vec<_>>(),
        [4, 5, 19, 25]
    );
    let after = session_messages_after(&conn, sid, 5).unwrap();
    assert_eq!(after.iter().map(|m| m.seq).collect::<Vec<_>>(), [19, 25]);
    assert_eq!(session_messages(&conn, "ses_nope").unwrap(), Vec::new());
}

#[test]
fn missing_table_is_empty_not_an_error() {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch("CREATE TABLE message (id text);")
        .unwrap();
    assert_eq!(session_messages(&conn, "ses_x").unwrap(), Vec::new());
}
