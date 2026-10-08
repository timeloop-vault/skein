use super::*;
use std::sync::mpsc;

fn test_db() -> (tempfile::TempDir, crate::db::Database) {
    let dir = tempfile::TempDir::new().unwrap();
    let db = crate::db::Database::open(&dir.path().join("t.db")).unwrap();
    (dir, db)
}

/// `process_buffer` is the unit-testable core. We feed it canned
/// SSE bytes and assert the emitted events. Reconnect / reqwest
/// concerns are covered by integration dogfood.
fn drain_events<F>(input: &[&str], frame_each: F) -> Vec<OpencodeEvent>
where
    F: Fn(usize, &str) -> Vec<u8>,
{
    let (tx, rx) = mpsc::channel();
    let cb = move |e: OpencodeEvent| {
        tx.send(e).unwrap();
    };
    let (_dir, db) = test_db();
    let cache: Mutex<HashMap<String, bool>> = Mutex::new(HashMap::new());
    let mut buf = Vec::new();
    for (i, chunk) in input.iter().enumerate() {
        buf.extend_from_slice(&frame_each(i, chunk));
        process_buffer(
            &mut buf,
            &cb,
            &db,
            None,
            "h-test",
            "r-test",
            "",
            &cache,
            &Mutex::default(),
        );
    }
    let mut out = Vec::new();
    while let Ok(e) = rx.try_recv() {
        out.push(e);
    }
    out
}

fn frame(s: &str) -> Vec<u8> {
    // SSE frame: `data: {payload}\n\n`.
    let mut v = b"data: ".to_vec();
    v.extend_from_slice(s.as_bytes());
    v.extend_from_slice(b"\n\n");
    v
}

#[test]
fn prompt_is_dropped_only_for_a_known_child_session() {
    assert!(should_emit_prompt(Some(true)));
    assert!(should_emit_prompt(None));
    assert!(!should_emit_prompt(Some(false)));
}

#[test]
fn session_status_busy_and_idle_map_correctly() {
    let events = drain_events(
        &[
            r#"{"type":"session.status","properties":{"sessionID":"s","status":{"type":"busy"}}}"#,
            r#"{"type":"session.status","properties":{"sessionID":"s","status":{"type":"idle"}}}"#,
        ],
        |_, p| frame(p),
    );
    assert!(matches!(events.first(), Some(OpencodeEvent::SessionBusy)));
    assert!(matches!(events.get(1), Some(OpencodeEvent::SessionIdle)));
    assert_eq!(events.len(), 2);
}

#[test]
fn session_created_captures_session_id() {
    let payload = r#"{"type":"session.created","properties":{"sessionID":"ses_abc123","info":{"id":"ses_abc123"}}}"#;
    let events = drain_events(&[payload], |_, p| frame(p));
    assert!(
        matches!(events.first(), Some(OpencodeEvent::SessionCreated { session_id, .. }) if session_id == "ses_abc123"),
        "expected SessionCreated(ses_abc123), got {events:?}"
    );
}

#[test]
fn session_created_reports_root_when_no_parent_id() {
    let payload = r#"{"type":"session.created","properties":{"sessionID":"ses_root","info":{"id":"ses_root"}}}"#;
    let events = drain_events(&[payload], |_, p| frame(p));
    assert!(
        matches!(events.first(), Some(OpencodeEvent::SessionCreated { session_id, parent_id })
            if session_id == "ses_root" && parent_id.is_none()),
        "expected root SessionCreated, got {events:?}"
    );
}

#[test]
fn session_created_reports_child_when_parent_id_present() {
    // A subagent (task-tool) child session always carries its
    // parent's id — verified upstream, not a guessed symmetry
    // (#116).
    let payload = r#"{"type":"session.created","properties":{"sessionID":"ses_child","info":{"id":"ses_child","parentID":"ses_root"}}}"#;
    let events = drain_events(&[payload], |_, p| frame(p));
    assert!(
        matches!(events.first(), Some(OpencodeEvent::SessionCreated { session_id, parent_id })
            if session_id == "ses_child" && parent_id.as_deref() == Some("ses_root")),
        "expected child SessionCreated, got {events:?}"
    );
}

#[test]
fn user_message_in_root_session_emits_root_session_prompted() {
    // opencode's `/sessions` picker publishes nothing to `/event`
    // itself (sst/opencode#5409) — a user-role message in a known
    // root session is the first observable sign of a switch.
    let events = drain_events(
        &[
            r#"{"type":"session.created","properties":{"sessionID":"ses_root","info":{"id":"ses_root"}}}"#,
            r#"{"type":"message.updated","properties":{"info":{"role":"user","sessionID":"ses_root"}}}"#,
        ],
        |_, p| frame(p),
    );
    assert!(
        events.iter().any(|e| matches!(e, OpencodeEvent::RootSessionPrompted { session_id } if session_id == "ses_root")),
        "expected RootSessionPrompted for a known root session, got {events:?}"
    );
}

#[test]
fn user_message_in_child_session_never_emits_root_session_prompted() {
    let events = drain_events(
        &[
            r#"{"type":"session.created","properties":{"sessionID":"ses_child","info":{"id":"ses_child","parentID":"ses_root"}}}"#,
            r#"{"type":"message.updated","properties":{"info":{"role":"user","sessionID":"ses_child"}}}"#,
        ],
        |_, p| frame(p),
    );
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, OpencodeEvent::RootSessionPrompted { .. })),
        "a subagent's own child session must never be followed, got {events:?}"
    );
}

#[test]
fn session_updated_alone_populates_the_root_cache() {
    // `session.updated` never surfaces its own `OpencodeEvent`, but
    // it's the only signal available for a session already alive
    // when the adapter attaches (no `session.created` to see).
    let events = drain_events(
        &[
            r#"{"type":"session.updated","properties":{"sessionID":"ses_root","info":{"id":"ses_root"}}}"#,
            r#"{"type":"message.updated","properties":{"info":{"role":"user","sessionID":"ses_root"}}}"#,
        ],
        |_, p| frame(p),
    );
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, OpencodeEvent::SessionCreated { .. })),
        "session.updated must never surface its own event, got {events:?}"
    );
    assert!(
        events.iter().any(|e| matches!(e, OpencodeEvent::RootSessionPrompted { session_id } if session_id == "ses_root")),
        "session.updated should have cached root status, got {events:?}"
    );
}

#[test]
fn user_message_in_unknown_session_is_returned_for_lookup_not_guessed() {
    let (_dir, db) = test_db();
    let (tx, rx) = mpsc::channel();
    let cb = move |e: OpencodeEvent| {
        tx.send(e).unwrap();
    };
    let cache: Mutex<HashMap<String, bool>> = Mutex::new(HashMap::new());
    let mut buf = frame(
        r#"{"type":"message.updated","properties":{"info":{"role":"user","sessionID":"ses_unknown"}}}"#,
    );
    let unresolved = process_buffer(
        &mut buf,
        &cb,
        &db,
        None,
        "h-test",
        "r-test",
        "",
        &cache,
        &Mutex::default(),
    );
    assert_eq!(unresolved, vec!["ses_unknown".to_owned()]);
    assert!(
        rx.try_recv().is_err(),
        "an unknown session must never emit before the lookup resolves it"
    );
}

#[test]
fn decide_root_session_prompted_only_fires_for_a_confirmed_root() {
    assert!(matches!(
        decide_root_session_prompted("ses_1", Some(true)),
        Some(OpencodeEvent::RootSessionPrompted { session_id }) if session_id == "ses_1"
    ));
    assert!(decide_root_session_prompted("ses_1", Some(false)).is_none());
    assert!(
        decide_root_session_prompted("ses_1", None).is_none(),
        "a failed or ambiguous lookup must never be followed on a guess"
    );
}

#[test]
fn session_id_validation_rejects_anything_not_alnum_or_underscore() {
    assert!(is_valid_session_id("ses_abc123"));
    assert!(!is_valid_session_id(""));
    assert!(!is_valid_session_id("ses/abc"));
    assert!(!is_valid_session_id("ses abc"));
    assert!(!is_valid_session_id("../../etc"));
}

#[test]
fn seed_own_session_root_accepts_a_valid_id_as_root() {
    assert_eq!(
        seed_own_session_root(Some("ses_abc123")),
        Some(("ses_abc123".to_owned(), true))
    );
}

#[test]
fn seed_own_session_root_rejects_absent_or_malformed_ids() {
    assert_eq!(
        seed_own_session_root(None),
        None,
        "fresh spawn, no session yet"
    );
    assert_eq!(seed_own_session_root(Some("")), None);
    assert_eq!(seed_own_session_root(Some("ses/../abc")), None);
}

#[test]
fn session_idle_redundant_event_is_suppressed() {
    // opencode emits both `session.status` idle AND a separate
    // `session.idle` event. We surface only the former so the
    // policy layer doesn't see double transitions.
    let events = drain_events(
        &[r#"{"type":"session.idle","properties":{"sessionID":"s"}}"#],
        |_, p| frame(p),
    );
    assert!(
        events.is_empty(),
        "session.idle should be suppressed, got {events:?}"
    );
}

#[test]
fn metadata_rows_do_not_emit() {
    let events = drain_events(
        &[
            r#"{"type":"server.heartbeat","properties":{}}"#,
            r#"{"type":"server.connected","properties":{}}"#,
            r#"{"type":"session.updated","properties":{"sessionID":"s"}}"#,
            r#"{"type":"session.diff","properties":{"sessionID":"s"}}"#,
            r#"{"type":"mcp.tools.changed","properties":{"server":"github"}}"#,
        ],
        |_, p| frame(p),
    );
    assert!(
        events.is_empty(),
        "metadata rows should be silent, got {events:?}"
    );
}

#[test]
fn permission_asked_and_replied_use_different_id_fields() {
    // `id` on asked, `requestID` on replied — verified against a
    // live opencode 1.18.30 binary, not a guessed symmetry.
    let events = drain_events(
        &[
            r#"{"type":"permission.asked","properties":{"id":"perm_1","sessionID":"ses_1","permission":"bash","patterns":["rm -rf"]}}"#,
            r#"{"type":"permission.replied","properties":{"sessionID":"ses_1","requestID":"perm_1","reply":"once"}}"#,
        ],
        |_, p| frame(p),
    );
    assert!(
        matches!(events.as_slice(), [
            OpencodeEvent::PermissionAsked { request_id, session_id },
            OpencodeEvent::PermissionReplied { request_id: r2, session_id: s2 },
        ] if request_id == "perm_1" && session_id == "ses_1" && r2 == "perm_1" && s2 == "ses_1"),
        "got {events:?}"
    );
}

#[test]
fn question_asked_replied_and_rejected_all_parse() {
    let events = drain_events(
        &[
            r#"{"type":"question.asked","properties":{"id":"q_1","sessionID":"ses_1","questions":[]}}"#,
            r#"{"type":"question.replied","properties":{"sessionID":"ses_1","requestID":"q_1","answers":[]}}"#,
            r#"{"type":"question.asked","properties":{"id":"q_2","sessionID":"ses_1","questions":[]}}"#,
            r#"{"type":"question.rejected","properties":{"sessionID":"ses_1","requestID":"q_2"}}"#,
        ],
        |_, p| frame(p),
    );
    assert!(
        matches!(events.as_slice(), [
            OpencodeEvent::QuestionAsked { request_id: r1, .. },
            OpencodeEvent::QuestionResolved { request_id: r2, .. },
            OpencodeEvent::QuestionAsked { request_id: r3, .. },
            OpencodeEvent::QuestionResolved { request_id: r4, .. },
        ] if r1 == "q_1" && r2 == "q_1" && r3 == "q_2" && r4 == "q_2"),
        "question.replied and question.rejected must both resolve: {events:?}"
    );
}

/// A subagent's child session blocks the same turn just as much as
/// the harness's own — these events are never filtered by session,
/// unlike `UserMessageAgent`.
#[test]
fn permission_and_question_events_are_not_filtered_by_session() {
    let events = drain_events(
        &[
            r#"{"type":"permission.asked","properties":{"id":"p1","sessionID":"ses_child_subagent"}}"#,
        ],
        |_, p| frame(p),
    );
    assert!(
        matches!(events.first(), Some(OpencodeEvent::PermissionAsked { session_id, .. }) if session_id == "ses_child_subagent"),
        "got {events:?}"
    );
}

#[test]
fn tool_use_part_emits_tool_use_start() {
    let payload = r#"{"type":"message.part.updated","properties":{"sessionID":"s","part":{"type":"tool","name":"Edit","id":"t1"}}}"#;
    let events = drain_events(&[payload], |_, p| frame(p));
    assert!(
        matches!(events.first(), Some(OpencodeEvent::ToolUseStart { name }) if name == "Edit"),
        "expected ToolUseStart(Edit), got {events:?}"
    );
}

#[test]
fn user_message_reports_the_agent_it_was_sent_to() {
    // Verbatim from a live `opencode serve` 1.18.30 (#248).
    let payload = r#"{"type":"message.updated","properties":{"sessionID":"ses_1","info":{"id":"msg_1","role":"user","sessionID":"ses_1","time":{"created":1789240141274},"agent":"plan","model":{"providerID":"p","modelID":"m"}}}}"#;
    let events = drain_events(&[payload], |_, p| frame(p));
    assert!(
        matches!(events.as_slice(), [OpencodeEvent::UserMessageAgent { session_id, agent }]
            if session_id == "ses_1" && agent == "plan"),
        "expected UserMessageAgent(ses_1, plan), got {events:?}"
    );
}

#[test]
fn older_mode_field_still_names_the_agent() {
    let payload = r#"{"type":"message.updated","properties":{"info":{"role":"user","sessionID":"ses_1","mode":"build"}}}"#;
    let events = drain_events(&[payload], |_, p| frame(p));
    assert!(
        matches!(events.as_slice(), [OpencodeEvent::UserMessageAgent { agent, .. }] if agent == "build"),
        "got {events:?}"
    );
}

#[test]
fn assistant_and_agentless_messages_do_not_report_an_agent() {
    // A compaction turn's assistant message says `compaction`, which
    // is not an agent the user picked — the reason this is user-only.
    let events = drain_events(
        &[
            r#"{"type":"message.updated","properties":{"info":{"role":"assistant","sessionID":"s","agent":"compaction","mode":"compaction"}}}"#,
            r#"{"type":"message.updated","properties":{"info":{"role":"user","sessionID":"s"}}}"#,
            r#"{"type":"message.updated","properties":{"info":{"role":"user","sessionID":"s","agent":""}}}"#,
        ],
        |_, p| frame(p),
    );
    assert!(events.is_empty(), "got {events:?}");
}

#[test]
fn malformed_json_skipped_other_events_still_emit() {
    let events = drain_events(
        &[
            "this-is-not-json",
            r#"{"type":"session.status","properties":{"sessionID":"s","status":{"type":"idle"}}}"#,
        ],
        |_, p| frame(p),
    );
    assert!(
        matches!(events.first(), Some(OpencodeEvent::SessionIdle)),
        "valid event after garbage should emit, got {events:?}"
    );
}

#[test]
fn partial_chunk_split_across_writes() {
    let (_tdir, tdb) = test_db();
    let (tx, rx) = mpsc::channel();
    let cb = move |e: OpencodeEvent| {
        tx.send(e).unwrap();
    };
    let cache: Mutex<HashMap<String, bool>> = Mutex::new(HashMap::new());
    let mut buf = Vec::new();
    buf.extend_from_slice(
        br#"data: {"type":"session.status","properties":{"sessionID":"s","status":{"type":"#,
    );
    process_buffer(
        &mut buf,
        &cb,
        &tdb,
        None,
        "h-test",
        "r-test",
        "",
        &cache,
        &Mutex::default(),
    );
    assert!(rx.try_recv().is_err(), "partial frame must not emit");
    buf.extend_from_slice(br#""idle"}}}"#);
    buf.extend_from_slice(b"\n\n");
    process_buffer(
        &mut buf,
        &cb,
        &tdb,
        None,
        "h-test",
        "r-test",
        "",
        &cache,
        &Mutex::default(),
    );
    let mut out: Vec<OpencodeEvent> = Vec::new();
    while let Ok(e) = rx.try_recv() {
        out.push(e);
    }
    assert!(
        matches!(out.first(), Some(OpencodeEvent::SessionIdle)),
        "completed frame should emit, got {out:?}"
    );
}

#[test]
fn multiple_frames_in_one_chunk_all_emit() {
    let (_tdir, tdb) = test_db();
    let mut buf = Vec::new();
    for payload in [
        r#"{"type":"session.status","properties":{"sessionID":"s","status":{"type":"busy"}}}"#,
        r#"{"type":"message.part.delta","properties":{"sessionID":"s","delta":"a"}}"#,
        r#"{"type":"session.status","properties":{"sessionID":"s","status":{"type":"idle"}}}"#,
    ] {
        buf.extend_from_slice(&frame(payload));
    }
    let (tx, rx) = mpsc::channel();
    let cb = move |e: OpencodeEvent| {
        tx.send(e).unwrap();
    };
    let cache: Mutex<HashMap<String, bool>> = Mutex::new(HashMap::new());
    process_buffer(
        &mut buf,
        &cb,
        &tdb,
        None,
        "h-test",
        "r-test",
        "",
        &cache,
        &Mutex::default(),
    );
    let mut out = Vec::new();
    while let Ok(e) = rx.try_recv() {
        out.push(e);
    }
    assert!(matches!(out.first(), Some(OpencodeEvent::SessionBusy)));
    assert!(matches!(out.get(1), Some(OpencodeEvent::MessageDelta)));
    assert!(matches!(out.get(2), Some(OpencodeEvent::SessionIdle)));
    assert_eq!(out.len(), 3);
}

#[test]
fn data_field_with_no_space_after_colon_still_parsed() {
    let (_tdir, tdb) = test_db();
    let mut buf = Vec::new();
    buf.extend_from_slice(
        br#"data:{"type":"session.status","properties":{"sessionID":"s","status":{"type":"idle"}}}"#,
    );
    buf.extend_from_slice(b"\n\n");
    let (tx, rx) = mpsc::channel();
    let cb = move |e: OpencodeEvent| {
        tx.send(e).unwrap();
    };
    let cache: Mutex<HashMap<String, bool>> = Mutex::new(HashMap::new());
    process_buffer(
        &mut buf,
        &cb,
        &tdb,
        None,
        "h-test",
        "r-test",
        "",
        &cache,
        &Mutex::default(),
    );
    let mut out: Vec<OpencodeEvent> = Vec::new();
    while let Ok(e) = rx.try_recv() {
        out.push(e);
    }
    assert!(
        matches!(out.first(), Some(OpencodeEvent::SessionIdle)),
        "should handle no-space `data:` form, got {out:?}"
    );
}

/// A server that accepts the connection but never answers `/event` must
/// fail the attempt (so the reconnect loop retries) rather than hang it.
#[tokio::test]
async fn unanswered_event_request_times_out() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    // Accept and hold the socket open, never writing a byte.
    let holder = std::thread::spawn(move || listener.accept().map(|(s, _)| s));
    let (_dir, db) = test_db();
    crate::setup::install_rustls_provider();
    let client = reqwest::Client::new();
    let cancel = Notify::new();
    let connected = AtomicBool::new(false);
    let emitted = Mutex::new(Vec::new());
    let on_event = |e: OpencodeEvent| emitted.lock().push(e);
    let root_cache = Mutex::new(HashMap::new());
    let prompts = Mutex::new(harness_actions_opencode::UserPromptTracker::default());
    let result = tokio::time::timeout(
        Duration::from_secs(5),
        stream_events(
            &client,
            &format!("http://127.0.0.1:{port}/event"),
            port,
            Duration::from_millis(200),
            &cancel,
            &on_event,
            &connected,
            &db,
            None,
            "h1",
            "r1",
            "/tmp",
            &root_cache,
            &prompts,
        ),
    )
    .await
    .expect("stream_events hung past its response timeout");
    assert!(matches!(result, Err(StreamError::NoResponse(_))));
    assert!(!connected.load(Ordering::Acquire));
    assert!(emitted.lock().is_empty());
    drop(holder.join());
}
