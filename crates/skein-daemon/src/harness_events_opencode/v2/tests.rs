use skein_harness::opencode::v2::{
    ExecutionOutcome, FormOutcome, SessionStatus, V2Event, drain_sse_data, parse_event,
};

use super::super::OpencodeEvent as E;
use super::connection::is_root_from_body;
use super::translate::{Translation, V2Translator};

const ROOT: &str = "ses_root";
const CHILD: &str = "ses_child";

/// A translator that knows `ses_root` (own) and `ses_child` (created below it).
fn tr() -> V2Translator {
    let mut t = V2Translator::new(Some(ROOT));
    t.translate(V2Event::SessionCreated {
        session_id: CHILD.into(),
        parent_id: Some(ROOT.into()),
    });
    t
}

fn out(t: &mut V2Translator, e: V2Event) -> Vec<E> {
    match t.translate(e) {
        Translation::Emit(v) => v,
        Translation::NeedsLookup(s) => panic!("unexpected lookup for {s}"),
    }
}

fn s(x: &str) -> String {
    x.to_owned()
}

fn status(sid: &str, status: SessionStatus) -> V2Event {
    V2Event::SessionStatus {
        session_id: s(sid),
        status,
    }
}

#[test]
fn execution_started_is_busy_for_root_and_delta_for_child() {
    let mut t = tr();
    let started = |sid: &str| V2Event::ExecutionStarted { session_id: s(sid) };
    assert!(matches!(out(&mut t, started(ROOT))[..], [E::SessionBusy]));
    assert!(matches!(out(&mut t, started(CHILD))[..], [E::MessageDelta]));
}

#[test]
fn execution_ended_is_idle_for_root_for_any_outcome_and_silent_for_child() {
    let mut t = tr();
    for outcome in [
        ExecutionOutcome::Succeeded,
        ExecutionOutcome::Failed,
        ExecutionOutcome::Interrupted,
    ] {
        let ended = |sid: &str| V2Event::ExecutionEnded {
            session_id: s(sid),
            outcome,
        };
        assert!(matches!(out(&mut t, ended(ROOT))[..], [E::SessionIdle]));
        assert!(out(&mut t, ended(CHILD)).is_empty());
    }
}

#[test]
fn session_status_maps_by_root_and_child() {
    let mut t = tr();
    for st in [SessionStatus::Busy, SessionStatus::Retry] {
        assert!(matches!(
            out(&mut t, status(ROOT, st))[..],
            [E::SessionBusy]
        ));
        assert!(matches!(
            out(&mut t, status(CHILD, st))[..],
            [E::MessageDelta]
        ));
    }
    assert!(matches!(
        out(&mut t, status(ROOT, SessionStatus::Idle))[..],
        [E::SessionIdle]
    ));
    assert!(out(&mut t, status(CHILD, SessionStatus::Idle)).is_empty());
}

#[test]
fn user_prompt_follows_only_a_root() {
    let mut t = tr();
    let prompt = |sid: &str| V2Event::UserPrompt {
        session_id: s(sid),
        inbox_id: s("msg_1"),
    };
    assert!(
        matches!(&out(&mut t, prompt(ROOT))[..], [E::RootSessionPrompted { session_id }] if session_id == ROOT)
    );
    assert!(out(&mut t, prompt(CHILD)).is_empty());
}

#[test]
fn session_created_is_emitted_and_classifies_the_session() {
    let mut t = V2Translator::new(None);
    let created = |sid: &str, parent: Option<&str>| V2Event::SessionCreated {
        session_id: s(sid),
        parent_id: parent.map(s),
    };
    assert!(matches!(
        &out(&mut t, created("ses_a", None))[..],
        [E::SessionCreated {
            parent_id: None,
            ..
        }]
    ));
    assert!(
        matches!(&out(&mut t, created("ses_b", Some("ses_a")))[..], [E::SessionCreated { parent_id: Some(p), .. }] if p == "ses_a")
    );
    let ended = |sid: &str| V2Event::ExecutionEnded {
        session_id: s(sid),
        outcome: ExecutionOutcome::Succeeded,
    };
    assert!(matches!(out(&mut t, ended("ses_a"))[..], [E::SessionIdle]));
    assert!(out(&mut t, ended("ses_b")).is_empty());
}

#[test]
fn unknown_session_asks_for_a_lookup_only_on_root_sensitive_events() {
    let mut t = V2Translator::new(Some(ROOT));
    let unknown = "ses_unknown";
    for ev in [
        V2Event::ExecutionStarted {
            session_id: s(unknown),
        },
        V2Event::ExecutionEnded {
            session_id: s(unknown),
            outcome: ExecutionOutcome::Failed,
        },
        status(unknown, SessionStatus::Idle),
        V2Event::UserPrompt {
            session_id: s(unknown),
            inbox_id: s("m"),
        },
    ] {
        assert!(matches!(t.translate(ev), Translation::NeedsLookup(sid) if sid == unknown));
    }
    // The others never need one.
    assert!(matches!(
        out(
            &mut t,
            V2Event::Streaming {
                session_id: s(unknown)
            }
        )[..],
        [E::MessageDelta]
    ));
    assert!(matches!(
        &out(&mut t, V2Event::ToolInputStarted {
            session_id: s(unknown),
            call_id: s("c"),
            name: s("bash")
        })[..],
        [E::ToolUseStart { name }] if name == "bash"
    ));
}

#[test]
fn a_recorded_lookup_answer_is_cached_and_replayed() {
    let mut t = V2Translator::new(None);
    let ev = || V2Event::ExecutionEnded {
        session_id: s("ses_x"),
        outcome: ExecutionOutcome::Succeeded,
    };
    assert!(matches!(t.translate(ev()), Translation::NeedsLookup(_)));
    t.record("ses_x", true);
    assert!(matches!(out(&mut t, ev())[..], [E::SessionIdle]));
}

#[test]
fn failed_lookup_reads_as_child_and_is_not_cached() {
    let mut t = V2Translator::new(None);
    let ev = || V2Event::ExecutionEnded {
        session_id: s("ses_x"),
        outcome: ExecutionOutcome::Succeeded,
    };
    assert!(matches!(t.translate(ev()), Translation::NeedsLookup(_)));
    assert!(t.translate_as_child(ev()).is_empty());
    assert!(matches!(t.translate(ev()), Translation::NeedsLookup(_)));
    let started = V2Event::ExecutionStarted {
        session_id: s("ses_x"),
    };
    assert!(matches!(
        t.translate_as_child(started)[..],
        [E::MessageDelta]
    ));
}

#[test]
fn lookup_body_root_iff_parent_id_absent_or_null() {
    let j = |v: &str| serde_json::from_str::<serde_json::Value>(v).unwrap();
    assert_eq!(
        is_root_from_body(&j(r#"{"data":{"id":"ses_a"}}"#)),
        Some(true)
    );
    assert_eq!(
        is_root_from_body(&j(r#"{"data":{"parentID":null}}"#)),
        Some(true)
    );
    assert_eq!(
        is_root_from_body(&j(r#"{"data":{"parentID":"ses_p"}}"#)),
        Some(false)
    );
    assert_eq!(is_root_from_body(&j(r#"{"id":"ses_a"}"#)), None);
    assert_eq!(is_root_from_body(&j("[]")), None);
}

fn form_created(session: Option<&str>, kind: Option<&str>) -> V2Event {
    V2Event::FormCreated {
        form_id: s("frm_1"),
        session_id: session.map(s),
        kind: kind.map(s),
    }
}

#[test]
fn only_question_forms_with_a_session_become_questions() {
    let mut t = tr();
    assert!(matches!(
        &out(&mut t, form_created(Some(ROOT), Some("question")))[..],
        [E::QuestionAsked { request_id, session_id }] if request_id == "frm_1" && session_id == ROOT
    ));
    assert!(out(&mut t, form_created(Some(ROOT), Some("other"))).is_empty());
    assert!(out(&mut t, form_created(Some(ROOT), None)).is_empty());
    assert!(out(&mut t, form_created(None, Some("question"))).is_empty());
}

#[test]
fn replied_and_cancelled_forms_both_resolve_the_question() {
    let mut t = tr();
    for outcome in [FormOutcome::Replied, FormOutcome::Cancelled] {
        let ev = V2Event::FormResolved {
            form_id: s("frm_1"),
            session_id: Some(s(CHILD)),
            outcome,
        };
        assert!(matches!(
            &out(&mut t, ev)[..],
            [E::QuestionResolved { request_id, session_id }] if request_id == "frm_1" && session_id == CHILD
        ));
    }
    let none = V2Event::FormResolved {
        form_id: s("frm_1"),
        session_id: None,
        outcome: FormOutcome::Cancelled,
    };
    assert!(out(&mut t, none).is_empty());
}

#[test]
fn permission_agent_and_inert_events() {
    let mut t = tr();
    assert!(matches!(
        &out(&mut t, V2Event::PermissionAsked { request_id: s("p1"), session_id: s(CHILD) })[..],
        [E::PermissionAsked { request_id, session_id }] if request_id == "p1" && session_id == CHILD
    ));
    assert!(matches!(
        &out(
            &mut t,
            V2Event::PermissionReplied {
                request_id: s("p1"),
                session_id: s(CHILD)
            }
        )[..],
        [E::PermissionReplied { .. }]
    ));
    assert!(matches!(
        &out(&mut t, V2Event::AgentSelected { session_id: s(ROOT), agent: s("plan") })[..],
        [E::UserMessageAgent { session_id, agent }] if session_id == ROOT && agent == "plan"
    ));
    assert!(out(&mut t, V2Event::ServerConnected).is_empty());
    let called = V2Event::ToolCalled {
        session_id: s(ROOT),
        call_id: s("c"),
        input: serde_json::Value::Null,
    };
    assert!(out(&mut t, called).is_empty());
    let ended = V2Event::ToolEnded {
        session_id: s(ROOT),
        call_id: s("c"),
        error: false,
    };
    assert!(out(&mut t, ended).is_empty());
}

/// Compact names so the pinned sequence reads on one line.
fn tag(e: &E) -> String {
    match e {
        E::SessionCreated { .. } => s("created"),
        E::RootSessionPrompted { .. } => s("prompted"),
        E::SessionBusy => s("busy"),
        E::SessionIdle => s("idle"),
        E::MessageDelta => s("delta"),
        E::ToolUseStart { name } => format!("tool:{name}"),
        E::UserMessageAgent { agent, .. } => format!("agent:{agent}"),
        E::PermissionAsked { .. } => s("perm_asked"),
        E::PermissionReplied { .. } => s("perm_replied"),
        E::QuestionAsked { .. } => s("q_asked"),
        E::QuestionResolved { .. } => s("q_resolved"),
        other => format!("{other:?}"),
    }
}

#[test]
fn replaying_the_captured_fixture_pins_the_emitted_sequence() {
    let raw = include_str!(
        "../../../../../crates/skein-harness/src/opencode/v2/fixtures/turn-tool-question-permission.sse"
    );
    let mut buf = raw.as_bytes().to_vec();
    let mut t = V2Translator::new(Some("ses_e7dd29bc7ffe6SxdD4hNEuFY1a"));
    let mut seq: Vec<String> = Vec::new();
    for payload in drain_sse_data(&mut buf) {
        let Some(ev) = parse_event(&payload) else {
            continue;
        };
        match t.translate(ev) {
            Translation::Emit(v) => seq.extend(v.iter().map(tag)),
            Translation::NeedsLookup(sid) => panic!("fixture needed a lookup for {sid}"),
        }
    }
    // Run-length compress consecutive repeats: `delta*N`.
    let mut compact: Vec<String> = Vec::new();
    let mut i = 0;
    while i < seq.len() {
        let n = seq[i..].iter().take_while(|x| **x == seq[i]).count();
        compact.push(if n > 1 {
            format!("{}*{n}", seq[i])
        } else {
            seq[i].clone()
        });
        i += n;
    }
    assert_eq!(
        compact.join(" "),
        concat!(
            "created prompted busy tool:execute delta*5 tool:glob delta idle ",
            "prompted busy tool:question q_asked q_resolved delta idle ",
            "prompted busy delta*2 tool:read perm_asked"
        )
    );
}
