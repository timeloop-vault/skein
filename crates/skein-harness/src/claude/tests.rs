use super::*;
use serde_json::json;
use std::io::Write;

#[test]
fn encode_cwd_matches_claude_scheme() {
    // Verified against real project dirs (chapter-5 recon §3, #145).
    assert_eq!(encode_cwd("/Users/foo/bar"), "-Users-foo-bar");
    assert_eq!(encode_cwd("/foo-bar/baz"), "-foo-bar-baz");
    assert_eq!(encode_cwd("C:\\git\\skein"), "C--git-skein");
    assert_eq!(encode_cwd("C:\\Users\\stefa"), "C--Users-stefa");
    assert_eq!(encode_cwd("D:\\"), "D--");
}

// #259: the vectors below come from running Claude Code 2.1.270's
// own encoder, lifted from its binary, over the same inputs.

#[test]
fn encode_cwd_replaces_every_non_alphanumeric() {
    assert_eq!(
        encode_cwd("D:\\gaming_pc_d\\git\\amber-skola"),
        "D--gaming-pc-d-git-amber-skola"
    );
    assert_eq!(encode_cwd("/home/u/my.proj dir"), "-home-u-my-proj-dir");
    assert_eq!(encode_cwd("C:\\Users\\u\\.buzz"), "C--Users-u--buzz");
}

#[test]
fn encode_cwd_counts_utf16_units_like_claude() {
    assert_eq!(encode_cwd("C:\\git\\skåne"), "C--git-sk-ne");
    assert_eq!(encode_cwd("/tmp/a😀b"), "-tmp-a--b");
}

#[test]
fn encode_cwd_truncates_long_names_with_claudes_hash() {
    let at_limit = format!("/{}", "a".repeat(199));
    assert_eq!(encode_cwd(&at_limit), format!("-{}", "a".repeat(199)));

    let over = format!("/{}", "a".repeat(200));
    assert_eq!(encode_cwd(&over), format!("-{}-b6ymvl", "a".repeat(199)));

    let nested = format!(
        "/home/user/projects/{}end",
        "some_long.directory name/".repeat(8)
    );
    let encoded = encode_cwd(&nested);
    assert_eq!(encoded.len(), 207);
    assert!(encoded.ends_with("-some--124j3r"), "{encoded}");
}

#[test]
fn long_name_hash_matches_javascript() {
    assert_eq!(
        js_string_hash("D:\\gaming_pc_d\\git\\amber-skola"),
        1_073_734_462
    );
    assert_eq!(js_string_hash("/tmp/a😀b"), -1_782_071_675);
    assert_eq!(base36(0), "0");
    // |i32::MIN|, which `Math.abs` also yields as a positive number.
    assert_eq!(base36(i32::MIN.unsigned_abs()), "zik0zk");
}

#[test]
fn session_path_and_subagents_dir_sit_side_by_side() {
    let home = Path::new("/home/u");
    let p = session_jsonl_path(home, "/home/u/proj", "abc");
    assert_eq!(
        p,
        Path::new("/home/u/.claude/projects/-home-u-proj/abc.jsonl")
    );
    assert_eq!(
        subagents_dir(&p).unwrap(),
        Path::new("/home/u/.claude/projects/-home-u-proj/abc/subagents")
    );
}

#[test]
fn session_exists_scans_every_project_dir() {
    let home = tempfile::TempDir::new().unwrap();
    let dir = projects_dir(home.path()).join("C--git-x");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("s1.jsonl"), "").unwrap();
    assert!(session_exists(home.path(), "s1"));
    assert!(!session_exists(home.path(), "s2"));
    assert_eq!(
        find_session_jsonl(home.path(), "s1"),
        Some(dir.join("s1.jsonl"))
    );
    assert_eq!(find_session_jsonl(home.path(), "s2"), None);
    assert!(!session_exists(Path::new("/definitely/not/here"), "s1"));
}

#[test]
fn list_sessions_finds_transcripts_and_their_subagents() {
    let home = tempfile::TempDir::new().unwrap();
    let dir = projects_dir(home.path()).join("C--git-x");
    let sub = dir.join("s1").join("subagents");
    fs::create_dir_all(&sub).unwrap();
    fs::write(dir.join("s1.jsonl"), "").unwrap();
    fs::write(dir.join("notes.txt"), "").unwrap();
    fs::write(sub.join("agent-b.jsonl"), "").unwrap();
    fs::write(sub.join("agent-a.jsonl"), "").unwrap();
    fs::write(sub.join("agent-a.meta.json"), "{}").unwrap();

    let sessions = list_sessions(home.path());
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].session_id, "s1");
    let subs = sessions[0].subagent_files();
    let names: Vec<_> = subs
        .iter()
        .map(|p| p.file_name().unwrap().to_str().unwrap().to_owned())
        .collect();
    assert_eq!(names, ["agent-a.jsonl", "agent-b.jsonl"]);
}

#[test]
fn subagent_id_from_path_accepts_only_agent_jsonl() {
    assert_eq!(
        subagent_id_from_path(Path::new("agent-01ABC.jsonl")),
        Some("01ABC".to_owned())
    );
    assert_eq!(subagent_id_from_path(Path::new("notagent-x.jsonl")), None);
    assert_eq!(subagent_id_from_path(Path::new("agent-x.meta.json")), None);
    assert_eq!(subagent_id_from_path(Path::new("agent-.jsonl")), None);
}

#[test]
fn subagent_meta_path_swaps_extension_for_meta_json() {
    assert_eq!(
        subagent_meta_path(Path::new("/a/subagents/agent-01ABC.jsonl")),
        Path::new("/a/subagents/agent-01ABC.meta.json")
    );
}

#[test]
fn read_subagent_meta_parses_a_real_sidecar() {
    let home = tempfile::TempDir::new().unwrap();
    let dir = home.path().join("subagents");
    fs::create_dir_all(&dir).unwrap();
    let jsonl = dir.join("agent-01ABC.jsonl");
    fs::write(&jsonl, "").unwrap();
    fs::write(
            dir.join("agent-01ABC.meta.json"),
            r#"{"agentType":"explore","description":"Map Claude JSONL tailer","toolUseId":"toolu_01NQaF9vThE1Z2brYyXv4anm","spawnDepth":1,"requestShape":"background","requestNonInteractive":true}"#,
        )
        .unwrap();

    let meta = read_subagent_meta(&jsonl).expect("sidecar should parse");
    assert_eq!(meta.agent_type.as_deref(), Some("explore"));
    assert_eq!(meta.description.as_deref(), Some("Map Claude JSONL tailer"));
    assert_eq!(
        meta.tool_use_id.as_deref(),
        Some("toolu_01NQaF9vThE1Z2brYyXv4anm")
    );
    assert_eq!(meta.spawn_depth, Some(1));
    assert_eq!(meta.request_shape.as_deref(), Some("background"));
    assert_eq!(meta.request_non_interactive, Some(true));
}

#[test]
fn read_subagent_meta_ignores_unknown_keys() {
    let home = tempfile::TempDir::new().unwrap();
    let dir = home.path().join("subagents");
    fs::create_dir_all(&dir).unwrap();
    let jsonl = dir.join("agent-01ABC.jsonl");
    fs::write(&jsonl, "").unwrap();
    fs::write(
        dir.join("agent-01ABC.meta.json"),
        r#"{"agentType":"explore","fromTheFuture":true}"#,
    )
    .unwrap();

    let meta = read_subagent_meta(&jsonl).expect("sidecar should parse");
    assert_eq!(meta.agent_type.as_deref(), Some("explore"));
}

#[test]
fn read_subagent_meta_missing_or_malformed_is_none() {
    let home = tempfile::TempDir::new().unwrap();
    let dir = home.path().join("subagents");
    fs::create_dir_all(&dir).unwrap();

    let no_sidecar = dir.join("agent-missing.jsonl");
    fs::write(&no_sidecar, "").unwrap();
    assert_eq!(read_subagent_meta(&no_sidecar), None);

    let malformed = dir.join("agent-bad.jsonl");
    fs::write(&malformed, "").unwrap();
    fs::write(dir.join("agent-bad.meta.json"), "not json").unwrap();
    assert_eq!(read_subagent_meta(&malformed), None);
}

#[test]
fn subagent_row_is_terminal_matches_the_terminal_stop_reasons() {
    for reason in ["end_turn", "stop_sequence", "max_tokens"] {
        let row = json!({"type": "assistant", "message": {"stop_reason": reason}});
        assert!(
            subagent_row_is_terminal(&row),
            "{reason} should be terminal"
        );
    }
    let tool_use = json!({"type": "assistant", "message": {"stop_reason": "tool_use"}});
    assert!(!subagent_row_is_terminal(&tool_use));

    let null_reason = json!({"type": "assistant", "message": {"stop_reason": null}});
    assert!(!subagent_row_is_terminal(&null_reason));

    let user_row = json!({"type": "user", "message": {"stop_reason": "end_turn"}});
    assert!(!subagent_row_is_terminal(&user_row));
}

fn handback_use() -> Value {
    json!({"parentUuid":"uuid-1","isSidechain":true,"agentId":"a1",
            "message":{"id":"msg_X","type":"message","role":"assistant",
            "content":[{"type":"tool_use","id":"toolu_H","name":"SubagentHandback",
            "input":{"message":"..."},"caller":{"type":"direct"}}],
            "stop_reason":null,"stop_sequence":null},
            "type":"assistant","uuid":"uuid-2",
            "timestamp":"2026-01-01T00:00:00.000Z","version":"2.1.285"})
}

fn handback_result(ends_turn: bool) -> Value {
    let mut row = json!({"parentUuid":"uuid-2","isSidechain":true,"agentId":"a1",
            "type":"user","message":{"role":"user","content":[{"tool_use_id":"toolu_H",
            "type":"tool_result","content":[{"type":"text",
            "text":"{\"success\":true,\"message\":\"Report delivered to your caller.\"}"}]}]},
            "uuid":"uuid-4","timestamp":"2026-01-01T00:00:01.100Z",
            "toolUseResult":{"success":true,"message":"Report delivered to your caller."},
            "sourceToolAssistantUUID":"uuid-2","version":"2.1.285"});
    if ends_turn {
        row["toolEndsTurn"] = json!(true);
    }
    row
}

fn end_turn_row() -> Value {
    json!({"type":"assistant","isSidechain":true,"agentId":"a1",
            "message":{"id":"msg_Y","role":"assistant",
            "content":[{"type":"text","text":"..."}],"stop_reason":"end_turn"},
            "uuid":"uuid-5","timestamp":"2026-01-01T00:00:02.000Z","version":"2.1.278"})
}

fn thinking_row() -> Value {
    json!({"type":"assistant","isSidechain":true,"agentId":"a1",
            "message":{"id":"msg_Y","role":"assistant",
            "content":[{"type":"thinking","thinking":"..."}],"stop_reason":null}})
}

fn bash_use(stop_reason: &Value) -> Value {
    json!({"type":"assistant","isSidechain":true,"agentId":"a1",
            "message":{"id":"msg_Z","role":"assistant",
            "content":[{"type":"tool_use","id":"toolu_Y","name":"Bash","input":{}}],
            "stop_reason":stop_reason}})
}

fn bash_result() -> Value {
    json!({"type":"user","isSidechain":true,"agentId":"a1",
            "message":{"role":"user","content":[{"tool_use_id":"toolu_Y",
            "type":"tool_result","content":"ok"}]}})
}

fn resume_row() -> Value {
    json!({"type":"user","isSidechain":true,"agentId":"a1",
            "message":{"role":"user","content":"please continue"}})
}

fn attachment_row() -> Value {
    json!({"type":"attachment","isSidechain":true,"agentId":"a1","attachment":{}})
}

fn transcript(rows: &[Value]) -> String {
    rows.iter()
        .map(Value::to_string)
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn lifecycle_handback_finishes_on_the_result_not_the_tool_use() {
    let mut lc = SubagentLifecycle::default();
    assert_eq!(lc.observe(&handback_use()), None);
    assert!(!lc.is_finished());
    assert_eq!(
        lc.observe(&handback_result(true)),
        Some(SubagentTransition::Finished)
    );
    assert!(lc.is_finished());
    assert_eq!(lc.observe(&handback_result(true)), None);
}

#[test]
fn lifecycle_result_matching_handback_id_finishes_without_the_flag() {
    let mut lc = SubagentLifecycle::default();
    lc.observe(&handback_use());
    assert_eq!(
        lc.observe(&handback_result(false)),
        Some(SubagentTransition::Finished)
    );
}

#[test]
fn lifecycle_tool_ends_turn_alone_finishes_when_the_tool_use_was_not_seen() {
    let mut lc = SubagentLifecycle::default();
    assert_eq!(
        lc.observe(&handback_result(true)),
        Some(SubagentTransition::Finished)
    );
    assert!(lc.is_finished());
}

#[test]
fn lifecycle_old_end_turn_ending_still_finishes() {
    let mut lc = SubagentLifecycle::default();
    assert_eq!(
        lc.observe(&end_turn_row()),
        Some(SubagentTransition::Finished)
    );
    assert!(lc.is_finished());
}

#[test]
fn lifecycle_mid_work_tool_use_keeps_working() {
    for stop in &[json!("tool_use"), json!(null)] {
        let mut lc = SubagentLifecycle::default();
        assert_eq!(lc.observe(&bash_use(stop)), None);
        assert!(!lc.is_finished());
        assert_eq!(lc.observe(&bash_result()), None);
        assert!(!lc.is_finished());
    }
}

#[test]
fn lifecycle_trailing_rows_after_a_27x_handback_never_reopen() {
    let mut lc = SubagentLifecycle::default();
    let transitions: Vec<_> = [
        handback_use(),
        handback_result(false),
        thinking_row(),
        end_turn_row(),
    ]
    .iter()
    .filter_map(|r| lc.observe(r))
    .collect();
    assert_eq!(transitions, vec![SubagentTransition::Finished]);
    assert!(lc.is_finished());
}

#[test]
fn lifecycle_resume_reopens_and_finishes_again() {
    let mut lc = SubagentLifecycle::default();
    lc.observe(&handback_use());
    lc.observe(&handback_result(true));
    assert!(lc.is_finished());
    assert_eq!(
        lc.observe(&resume_row()),
        Some(SubagentTransition::Reopened)
    );
    assert!(!lc.is_finished());
    assert_eq!(lc.observe(&bash_use(&json!("tool_use"))), None);
    assert_eq!(lc.observe(&bash_result()), None);
    assert!(!lc.is_finished());
    lc.observe(&handback_use());
    assert_eq!(
        lc.observe(&handback_result(true)),
        Some(SubagentTransition::Finished)
    );
}

#[test]
fn lifecycle_queue_transcript_only_row_does_not_reopen() {
    let mut lc = SubagentLifecycle::default();
    lc.observe(&end_turn_row());
    let mut row = resume_row();
    row["queueTranscriptOnly"] = json!(true);
    assert_eq!(lc.observe(&row), None);
    assert!(lc.is_finished());
    assert!(is_queue_transcript_only(&row));
    assert!(!is_queue_transcript_only(&resume_row()));
}

#[test]
fn lifecycle_queue_transcript_only_after_a_handback_does_not_reopen() {
    let mut lc = SubagentLifecycle::default();
    lc.observe(&handback_use());
    lc.observe(&handback_result(true));
    let mut row = resume_row();
    row["queueTranscriptOnly"] = json!(true);
    assert_eq!(lc.observe(&row), None);
    assert!(lc.is_finished());
}

#[test]
fn lifecycle_is_meta_row_still_reopens() {
    let mut lc = SubagentLifecycle::default();
    lc.observe(&end_turn_row());
    let mut row = resume_row();
    row["isMeta"] = json!(true);
    assert_eq!(lc.observe(&row), Some(SubagentTransition::Reopened));
}

#[test]
fn lifecycle_trailing_attachment_row_does_not_reopen() {
    let mut lc = SubagentLifecycle::default();
    lc.observe(&handback_use());
    lc.observe(&handback_result(true));
    assert_eq!(lc.observe(&attachment_row()), None);
    assert!(lc.is_finished());
}

#[test]
fn subagent_transcript_is_finished_scans_the_whole_transcript() {
    assert!(subagent_transcript_is_finished(&transcript(&[
        bash_use(&json!("tool_use")),
        bash_result(),
        handback_use(),
        handback_result(true),
    ])));
    assert!(subagent_transcript_is_finished(&transcript(&[
        bash_use(&json!("tool_use")),
        bash_result(),
        end_turn_row(),
    ])));
    assert!(!subagent_transcript_is_finished(&transcript(&[
        bash_use(&json!("tool_use")),
        bash_result(),
    ])));
    assert!(!subagent_transcript_is_finished(&transcript(&[
        bash_result(),
        handback_use(),
    ])));
    assert!(subagent_transcript_is_finished(&transcript(&[
        handback_use(),
        handback_result(true),
        attachment_row(),
    ])));
    let with_junk = format!("{}\n\nnot json\n{}", handback_use(), handback_result(true));
    assert!(subagent_transcript_is_finished(&with_junk));
}

/// Shape taken from a real 2.1.263 assistant row.
fn assistant_row() -> Value {
    json!({
        "parentUuid": "p1",
        "isSidechain": false,
        "message": {
            "model": "claude-fable-5-1",
            "id": "msg_01",
            "role": "assistant",
            "content": [],
            "stop_reason": "tool_use",
            "usage": {
                "input_tokens": 2,
                "cache_creation_input_tokens": 14654,
                "cache_read_input_tokens": 31919,
                "output_tokens": 400,
                "output_tokens_details": {"thinking_tokens": 135},
                "cache_creation": {
                    "ephemeral_1h_input_tokens": 14654,
                    "ephemeral_5m_input_tokens": 0
                }
            }
        },
        "requestId": "req_01",
        "type": "assistant",
        "uuid": "u1",
        "timestamp": "2026-09-08T11:16:21.559Z",
        "cwd": "C:\\git\\agents",
        "sessionId": "s1"
    })
}

#[test]
fn assistant_row_is_typed_with_usage() {
    let Some(Row::Assistant(a)) = row_from_value(&assistant_row()) else {
        panic!("expected assistant row");
    };
    assert_eq!(a.model.as_deref(), Some("claude-fable-5-1"));
    assert_eq!(a.request_id.as_deref(), Some("req_01"));
    assert_eq!(a.message_id.as_deref(), Some("msg_01"));
    assert!(!a.is_terminal());
    assert!(!a.is_sidechain);
    assert_eq!(a.usage.cache_read, 31919);
    assert_eq!(a.usage.cache_creation_1h, 14654);
    assert_eq!(a.usage.thinking, 135);
    assert_eq!(a.usage.total_input(), 2 + 14654 + 31919);
    assert_eq!(a.dedupe_key().as_deref(), Some("msg_01:req_01"));
    assert!(a.timestamp_ms.is_some());
}

#[test]
fn user_row_carries_spawned_agent() {
    let v = json!({
        "type": "user",
        "isSidechain": false,
        "uuid": "u2",
        "timestamp": "2026-07-12T17:09:56.953Z",
        "message": {"role": "user", "content": []},
        "toolUseResult": {
            "isAsync": true,
            "status": "async_launched",
            "agentId": "a837d4fac8622afd9",
            "description": "Audit first editor open log",
            "resolvedModel": "claude-fable-5"
        }
    });
    let Some(Row::User(u)) = row_from_value(&v) else {
        panic!("expected user row");
    };
    assert!(u.is_tool_result);
    let spawned = u.spawned_agent.unwrap();
    assert_eq!(spawned.agent_id, "a837d4fac8622afd9");
    assert_eq!(spawned.resolved_model.as_deref(), Some("claude-fable-5"));
}

#[test]
fn plain_prompt_is_not_a_tool_result() {
    let v = json!({"type": "user", "message": {"role": "user", "content": "hi"}});
    let Some(Row::User(u)) = row_from_value(&v) else {
        panic!("expected user row");
    };
    assert!(!u.is_tool_result);
    assert!(u.spawned_agent.is_none());
    assert_eq!(u.timestamp_ms, None);
}

/// Shape taken verbatim from a real 2.1.263 `cost-state` row.
#[test]
fn cost_state_row_is_typed() {
    let v = json!({
        "type": "cost-state",
        "sessionId": "9210e4bf",
        "totalCostUSD": 12.05013775,
        "totalAPIDuration": 181766,
        "totalAPIDurationWithoutRetries": 181728,
        "totalToolDuration": 1197212,
        "totalLinesAdded": 12,
        "totalLinesRemoved": 1,
        "totalDuration": 11859757,
        "startTime": 1788723920352u64,
        "modelUsage": {
            "claude-fable-5-1": {
                "inputTokens": 838,
                "outputTokens": 10945,
                "thinkingTokens": 2722,
                "cacheReadInputTokens": 5129711,
                "cacheCreationInputTokens": 510604,
                "webSearchRequests": 0,
                "costUSD": 12.05013775
            }
        },
        "hasUnknownModelCost": false
    });
    let Some(Row::CostState(c)) = row_from_value(&v) else {
        panic!("expected cost-state row");
    };
    assert_eq!(c.session_id, "9210e4bf");
    assert!((c.total_cost_usd - 12.050_137_75).abs() < 1e-9);
    assert_eq!(c.total_duration_ms, 11_859_757);
    assert_eq!(c.total_lines_added, 12);
    let m = &c.model_usage["claude-fable-5-1"];
    assert_eq!(m.cache_read_input_tokens, 5_129_711);
    assert!((m.cost_usd - 12.050_137_75).abs() < 1e-9);
}

#[test]
fn cost_state_tolerates_a_field_less_row() {
    let v = json!({"type": "cost-state", "sessionId": "s"});
    let Some(Row::CostState(c)) = row_from_value(&v) else {
        panic!("expected cost-state row");
    };
    assert_eq!(c.total_cost_usd, 0.0);
    assert!(c.model_usage.is_empty());
}

/// Shape taken verbatim from a real 2.1.263 transcript started with
/// `claude --agent scribe`.
#[test]
fn agent_setting_row_names_the_agent() {
    let v = json!({"type": "agent-setting", "agentSetting": "scribe", "sessionId": "s1"});
    assert_eq!(
        row_from_value(&v),
        Some(Row::AgentSetting("scribe".to_owned()))
    );
    // Field-less row: still typed, so a caller can count it.
    assert_eq!(
        row_from_value(&json!({"type": "agent-setting"})),
        Some(Row::AgentSetting(String::new()))
    );
}

#[test]
fn summarize_reports_the_agent_setting_or_none() {
    let plain = summarize(vec![row_from_value(&assistant_row()).unwrap()]);
    assert_eq!(plain.agent_setting, None);

    let named = summarize(vec![
        row_from_value(&json!({"type": "agent-setting", "agentSetting": "explore"})).unwrap(),
        row_from_value(&assistant_row()).unwrap(),
        row_from_value(&json!({"type": "agent-setting", "agentSetting": "orchestrator"})).unwrap(),
    ]);
    assert_eq!(named.agent_setting.as_deref(), Some("orchestrator"));
}

#[test]
fn other_rows_keep_their_type() {
    let v = json!({"type": "file-history-snapshot", "timestamp": "2026-05-15T21:16:22Z"});
    assert!(matches!(
        row_from_value(&v),
        Some(Row::Other { ty, timestamp_ms: Some(_) }) if ty == "file-history-snapshot"
    ));
    assert!(row_from_value(&json!({"foo": 1})).is_none());
    assert!(parse_row("").is_none());
    assert!(parse_row("{not json").is_none());
}

#[test]
fn summarize_dedupes_streamed_chunks_and_keeps_newest_cost_state() {
    let mut chunk2 = assistant_row();
    chunk2["uuid"] = json!("u1b");
    chunk2["message"]["stop_reason"] = json!("end_turn");
    let mut other = assistant_row();
    other["message"]["id"] = json!("msg_02");
    other["requestId"] = json!("req_02");
    other["message"]["model"] = json!("claude-haiku-4-5");
    let rows = vec![
        row_from_value(&assistant_row()).unwrap(),
        row_from_value(&chunk2).unwrap(),
        row_from_value(&other).unwrap(),
        row_from_value(&json!({"type": "cost-state", "sessionId": "s", "totalCostUSD": 1.0}))
            .unwrap(),
        row_from_value(&json!({"type": "cost-state", "sessionId": "s", "totalCostUSD": 2.5}))
            .unwrap(),
    ];
    let s = summarize(rows);
    assert_eq!(s.responses_by_model["claude-fable-5-1"], 1);
    assert_eq!(s.responses_by_model["claude-haiku-4-5"], 1);
    assert_eq!(s.usage_by_model["claude-fable-5-1"].cache_read, 31919);
    assert!((s.cost_state.unwrap().total_cost_usd - 2.5).abs() < 1e-9);
    assert_eq!(s.cwd.as_deref(), Some("C:\\git\\agents"));
}

#[test]
fn rows_skips_bad_lines_and_reads_the_rest() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("s.jsonl");
    let mut f = fs::File::create(&path).unwrap();
    writeln!(f, "{}", assistant_row()).unwrap();
    writeln!(f).unwrap();
    writeln!(f, "{{\"type\":\"cost-state\",\"sessionId\":\"s\"}}").unwrap();
    f.write_all(b"\xff\xfe not utf8\n").unwrap();
    write!(f, "{{\"type\":\"assistant\",\"truncated").unwrap();
    drop(f);
    let got: Vec<Row> = rows(&path).unwrap().collect();
    assert_eq!(got.len(), 2);
    assert!(matches!(got[0], Row::Assistant(_)));
    assert!(matches!(got[1], Row::CostState(_)));
}
