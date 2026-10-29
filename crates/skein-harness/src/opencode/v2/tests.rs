use super::*;
use serde_json::json;

const FIXTURE: &str = include_str!("fixtures/turn-tool-question-permission.sse");
const SID: &str = "ses_e7dd29bc7ffe6SxdD4hNEuFY1a";

fn frame(ty: &str, data: &Value) -> String {
    json!({"id": "evt_1", "created": 1, "type": ty, "data": data}).to_string()
}

fn sid() -> Value {
    json!({ "sessionID": SID })
}

fn with_sid(extra: &Value) -> Value {
    let mut v = sid();
    for (k, val) in extra.as_object().unwrap() {
        v[k] = val.clone();
    }
    v
}

fn fixture_payloads() -> Vec<String> {
    let mut buf = FIXTURE.as_bytes().to_vec();
    let out = drain_sse_data(&mut buf);
    assert!(buf.is_empty(), "fixture ends on a block boundary");
    out
}

fn s(x: &str) -> String {
    x.to_owned()
}

#[test]
fn per_variant_table() {
    use V2Event as E;
    let cases: Vec<(&str, Value, V2Event)> = vec![
        ("server.connected", json!({}), E::ServerConnected),
        (
            "session.created",
            sid(),
            E::SessionCreated {
                session_id: s(SID),
                parent_id: None,
            },
        ),
        // schema-derived, not captured
        (
            "session.created",
            with_sid(&json!({"parentID": "ses_parent"})),
            E::SessionCreated {
                session_id: s(SID),
                parent_id: Some(s("ses_parent")),
            },
        ),
        (
            "session.execution.started",
            sid(),
            E::ExecutionStarted { session_id: s(SID) },
        ),
        (
            "session.execution.succeeded",
            sid(),
            E::ExecutionEnded {
                session_id: s(SID),
                outcome: ExecutionOutcome::Succeeded,
            },
        ),
        // schema-derived, not captured
        (
            "session.execution.failed",
            sid(),
            E::ExecutionEnded {
                session_id: s(SID),
                outcome: ExecutionOutcome::Failed,
            },
        ),
        // schema-derived, not captured
        (
            "session.execution.interrupted",
            sid(),
            E::ExecutionEnded {
                session_id: s(SID),
                outcome: ExecutionOutcome::Interrupted,
            },
        ),
        // schema-derived, not captured
        (
            "session.status",
            with_sid(&json!({"status": {"type": "busy"}})),
            E::SessionStatus {
                session_id: s(SID),
                status: SessionStatus::Busy,
            },
        ),
        // schema-derived, not captured
        (
            "session.status",
            with_sid(&json!({"status": "idle"})),
            E::SessionStatus {
                session_id: s(SID),
                status: SessionStatus::Idle,
            },
        ),
        // schema-derived, not captured
        (
            "session.status",
            with_sid(
                &json!({"status": {"type": "retry", "attempt": 1, "message": "m", "next": 5}}),
            ),
            E::SessionStatus {
                session_id: s(SID),
                status: SessionStatus::Retry,
            },
        ),
        (
            "session.inbox.enqueued",
            with_sid(&json!({"inboxID": "msg_1", "item": {"type": "user"}})),
            E::UserPrompt {
                session_id: s(SID),
                inbox_id: s("msg_1"),
            },
        ),
        (
            "session.text.delta",
            with_sid(&json!({"delta": "x"})),
            E::Streaming { session_id: s(SID) },
        ),
        (
            "session.reasoning.delta",
            with_sid(&json!({"delta": "x"})),
            E::Streaming { session_id: s(SID) },
        ),
        (
            "session.tool.input.started",
            with_sid(&json!({"id": "call_1", "name": "execute"})),
            E::ToolInputStarted {
                session_id: s(SID),
                call_id: s("call_1"),
                name: s("execute"),
            },
        ),
        (
            "session.tool.called",
            with_sid(&json!({"id": "call_1", "input": {"code": "1"}, "executed": false})),
            E::ToolCalled {
                session_id: s(SID),
                call_id: s("call_1"),
                input: json!({"code": "1"}),
            },
        ),
        (
            "session.tool.success",
            with_sid(&json!({"id": "call_1", "metadata": {"error": true}})),
            E::ToolEnded {
                session_id: s(SID),
                call_id: s("call_1"),
                error: true,
            },
        ),
        (
            "session.tool.success",
            with_sid(&json!({"id": "call_1", "metadata": {"error": false}})),
            E::ToolEnded {
                session_id: s(SID),
                call_id: s("call_1"),
                error: false,
            },
        ),
        // schema-derived, not captured
        (
            "session.tool.failed",
            with_sid(&json!({"id": "call_1"})),
            E::ToolEnded {
                session_id: s(SID),
                call_id: s("call_1"),
                error: true,
            },
        ),
        (
            "permission.asked",
            with_sid(&json!({"id": "per_1", "action": "read"})),
            E::PermissionAsked {
                request_id: s("per_1"),
                session_id: s(SID),
            },
        ),
        // schema-derived, not captured
        (
            "permission.replied",
            with_sid(&json!({"requestID": "per_1"})),
            E::PermissionReplied {
                request_id: s("per_1"),
                session_id: s(SID),
            },
        ),
        (
            "form.created",
            json!({"form": {"id": "frm_1", "sessionID": SID, "metadata": {"kind": "question"}}}),
            E::FormCreated {
                form_id: s("frm_1"),
                session_id: Some(s(SID)),
                kind: Some(s("question")),
            },
        ),
        // schema-derived, not captured: MCP elicitation has no session
        (
            "form.created",
            json!({"form": {"id": "frm_2"}}),
            E::FormCreated {
                form_id: s("frm_2"),
                session_id: None,
                kind: None,
            },
        ),
        (
            "form.replied",
            with_sid(&json!({"id": "frm_1", "answer": {"q0": "Blue"}})),
            E::FormResolved {
                form_id: s("frm_1"),
                session_id: Some(s(SID)),
                outcome: FormOutcome::Replied,
            },
        ),
        // schema-derived, not captured
        (
            "form.cancelled",
            json!({"id": "frm_1"}),
            E::FormResolved {
                form_id: s("frm_1"),
                session_id: None,
                outcome: FormOutcome::Cancelled,
            },
        ),
        // schema-derived, not captured
        (
            "session.agent.selected",
            with_sid(&json!({"agent": "plan"})),
            E::AgentSelected {
                session_id: s(SID),
                agent: s("plan"),
            },
        ),
        // schema-derived, not captured
        (
            "session.agent.selected",
            with_sid(&json!({"agent": {"name": "build"}})),
            E::AgentSelected {
                session_id: s(SID),
                agent: s("build"),
            },
        ),
    ];
    for (ty, data, want) in cases {
        assert_eq!(parse_event(&frame(ty, &data)), Some(want), "{ty} {data}");
    }
}

#[test]
fn silent_cases() {
    assert_eq!(parse_event("not json"), None);
    assert_eq!(parse_event(""), None);
    assert_eq!(parse_event("{}"), None);
    assert_eq!(
        parse_event(&frame("project.updated", &json!({"id": "x"}))),
        None
    );
    assert_eq!(parse_event(&frame("session.step.started", &sid())), None);
    assert_eq!(parse_event(&frame("brand.new.type", &sid())), None);
    // a non-user inbox item is not a prompt
    let agent_item = with_sid(&json!({"inboxID": "m", "item": {"type": "synthetic"}}));
    assert_eq!(
        parse_event(&frame("session.inbox.enqueued", &agent_item)),
        None
    );
    // unknown status
    let odd = with_sid(&json!({"status": {"type": "weird"}}));
    assert_eq!(parse_event(&frame("session.status", &odd)), None);
}

#[test]
fn session_scoped_frames_need_a_session_id() {
    let types = [
        "session.created",
        "session.execution.started",
        "session.execution.succeeded",
        "session.status",
        "session.inbox.enqueued",
        "session.text.delta",
        "session.reasoning.delta",
        "session.tool.input.started",
        "session.tool.called",
        "session.tool.success",
        "session.tool.failed",
        "permission.asked",
        "permission.replied",
        "session.agent.selected",
    ];
    let rest = json!({
        "id": "x", "name": "n", "requestID": "r", "inboxID": "m", "agent": "a",
        "status": "busy", "item": {"type": "user"}, "metadata": {},
    });
    for ty in types {
        assert_eq!(
            parse_event(&frame(ty, &rest)),
            None,
            "{ty} without sessionID"
        );
        let empty = with_sid(&json!({"sessionID": ""}));
        let mut d = rest.clone();
        d["sessionID"] = empty["sessionID"].clone();
        assert_eq!(
            parse_event(&frame(ty, &d)),
            None,
            "{ty} with empty sessionID"
        );
    }
}

#[test]
fn framing_crlf_multiline_comments_and_partials() {
    let mut buf = b": heartbeat\r\n\r\nevent: x\r\nid: 7\r\ndata: {\"a\":1}\r\n\r\ndata: one\ndata: two\n\ndata:no-space\n\ndata: part".to_vec();
    let out = drain_sse_data(&mut buf);
    assert_eq!(out, vec![s("{\"a\":1}"), s("one\ntwo"), s("no-space")]);
    assert_eq!(buf, b"data: part");
    buf.extend_from_slice(b"ial\n\n");
    assert_eq!(drain_sse_data(&mut buf), vec![s("partial")]);
    assert_eq!(buf.len(), 0);
}

#[test]
fn framing_survives_a_split_inside_a_utf8_sequence() {
    let whole = "data: {\"t\":\"h\u{e5}\u{1f600}\"}\n\n".as_bytes();
    for cut in 1..whole.len() {
        let mut buf = whole[..cut].to_vec();
        let mut got = drain_sse_data(&mut buf);
        buf.extend_from_slice(&whole[cut..]);
        got.extend(drain_sse_data(&mut buf));
        assert_eq!(got, vec![s("{\"t\":\"h\u{e5}\u{1f600}\"}")], "cut at {cut}");
    }
}

#[test]
fn fixture_framing_is_chunk_size_independent() {
    let whole = fixture_payloads();
    assert_eq!(
        whole.len(),
        73,
        "one payload per data block; heartbeats skipped"
    );
    for size in 1..=64 {
        let mut buf = Vec::new();
        let mut got = Vec::new();
        for chunk in FIXTURE.as_bytes().chunks(size) {
            buf.extend_from_slice(chunk);
            got.extend(drain_sse_data(&mut buf));
        }
        assert!(buf.is_empty(), "chunk size {size}");
        assert_eq!(got, whole, "chunk size {size}");
    }
}

fn tag(e: &V2Event) -> &'static str {
    match e {
        V2Event::ServerConnected => "connected",
        V2Event::SessionCreated { .. } => "created",
        V2Event::ExecutionStarted { .. } => "exec-start",
        V2Event::ExecutionEnded { .. } => "exec-end",
        V2Event::SessionStatus { .. } => "status",
        V2Event::UserPrompt { .. } => "prompt",
        V2Event::Streaming { .. } => "stream",
        V2Event::ToolInputStarted { .. } => "tool-in",
        V2Event::ToolCalled { .. } => "tool-called",
        V2Event::ToolEnded { .. } => "tool-end",
        V2Event::PermissionAsked { .. } => "perm-asked",
        V2Event::PermissionReplied { .. } => "perm-replied",
        V2Event::FormCreated { .. } => "form-created",
        V2Event::FormResolved { .. } => "form-resolved",
        V2Event::AgentSelected { .. } => "agent",
    }
}

#[test]
fn fixture_replays_to_the_pinned_sequence() {
    let events: Vec<V2Event> = fixture_payloads()
        .iter()
        .filter_map(|p| parse_event(p))
        .collect();
    let seq: Vec<&str> = events.iter().map(tag).collect();
    let expected = vec![
        "connected",
        "created",
        "prompt",
        "exec-start",
        "tool-in",
        "tool-called",
        "tool-end",
        "stream",
        "stream",
        "stream",
        "stream",
        "stream",
        "tool-in",
        "tool-called",
        "tool-end",
        "stream",
        "exec-end",
        "prompt",
        "exec-start",
        "tool-in",
        "tool-called",
        "form-created",
        "form-resolved",
        "tool-end",
        "stream",
        "exec-end",
        "prompt",
        "exec-start",
        "stream",
        "stream",
        "tool-in",
        "tool-called",
        "perm-asked",
    ];
    assert_eq!(seq, expected);
}

#[test]
fn fixture_details() {
    let events: Vec<V2Event> = fixture_payloads()
        .iter()
        .filter_map(|p| parse_event(p))
        .collect();
    assert!(events.contains(&V2Event::ToolInputStarted {
        session_id: s(SID),
        call_id: s("call_00_ET_UTJBD9vMiVeeuWu3XMPd8933"),
        name: s("execute"),
    }));
    // the first tool failed inside the executor: success frame, metadata.error
    assert!(events.contains(&V2Event::ToolEnded {
        session_id: s(SID),
        call_id: s("call_00_ET_UTJBD9vMiVeeuWu3XMPd8933"),
        error: true,
    }));
    assert!(events.contains(&V2Event::FormResolved {
        form_id: s("frm_1822e1b1b001Dbj9AOoUvUQsyS"),
        session_id: Some(s(SID)),
        outcome: FormOutcome::Replied,
    }));
}
