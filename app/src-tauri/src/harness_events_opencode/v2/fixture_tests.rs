//! The 2.2.0 captures of 2026-10-29 replayed through the translator, with the
//! fixture's root session seeded as the harness's own session.

use skein_harness::opencode::v2::{drain_sse_data, parse_event};

use super::super::OpencodeEvent as E;
use super::connection::is_root_from_body;
use super::translate::{Translation, V2Translator};

const ROOT_JSON: &str = include_str!(
    "../../../../../crates/skein-harness/src/opencode/v2/fixtures/session-get-root.json"
);
const CHILD_JSON: &str = include_str!(
    "../../../../../crates/skein-harness/src/opencode/v2/fixtures/session-get-child.json"
);

macro_rules! fixture {
    ($name:literal) => {
        include_str!(concat!(
            "../../../../../crates/skein-harness/src/opencode/v2/fixtures/",
            $name
        ))
    };
}

fn translate_all(t: &mut V2Translator, raw: &str) -> Vec<E> {
    let mut buf = raw.as_bytes().to_vec();
    let mut seq = Vec::new();
    for payload in drain_sse_data(&mut buf) {
        let Some(ev) = parse_event(&payload) else {
            continue;
        };
        match t.translate(ev) {
            Translation::Emit(v) => seq.extend(v),
            Translation::NeedsLookup(sid) => panic!("fixture needed a lookup for {sid}"),
        }
    }
    seq
}

/// Short, id-free names; a session is shown as its last 4 characters.
fn tag(e: &E) -> String {
    let tail = |s: &str| s[s.len() - 4..].to_owned();
    match e {
        E::SessionCreated {
            session_id,
            parent_id,
        } => match parent_id {
            Some(p) => format!("created:{}<-{}", tail(session_id), tail(p)),
            None => format!("created:{}", tail(session_id)),
        },
        E::RootSessionPrompted { .. } => "prompted".into(),
        E::SessionBusy => "busy".into(),
        E::SessionIdle => "idle".into(),
        E::MessageDelta => "delta".into(),
        E::ToolUseStart { name } => format!("tool:{name}"),
        E::UserMessageAgent { agent, session_id } => format!("agent:{agent}@{}", tail(session_id)),
        E::PermissionAsked { session_id, .. } => format!("perm_asked@{}", tail(session_id)),
        E::PermissionReplied { session_id, .. } => format!("perm_replied@{}", tail(session_id)),
        E::QuestionAsked { session_id, .. } => format!("q_asked@{}", tail(session_id)),
        E::QuestionResolved { session_id, .. } => format!("q_resolved@{}", tail(session_id)),
        other => format!("{other:?}"),
    }
}

/// Run-length compressed, space-separated: `delta*5`.
fn compact(seq: &[E]) -> String {
    let tags: Vec<String> = seq.iter().map(tag).collect();
    let mut out: Vec<String> = Vec::new();
    let mut i = 0;
    while i < tags.len() {
        let n = tags[i..].iter().take_while(|x| **x == tags[i]).count();
        out.push(if n > 1 {
            format!("{}*{n}", tags[i])
        } else {
            tags[i].clone()
        });
        i += n;
    }
    out.join(" ")
}

fn replay(root: &str, raw: &str) -> String {
    compact(&translate_all(&mut V2Translator::new(Some(root)), raw))
}

#[test]
fn permission_asked_then_replied_then_idle() {
    assert_eq!(
        replay(
            "ses_e77d5e587ffeylOS3IgWhX1xrR",
            fixture!("permission-reply.sse")
        ),
        "created:1xrR prompted busy delta*8 tool:shell perm_asked@1xrR perm_replied@1xrR delta*9 idle"
    );
}

#[test]
fn question_replied_then_idle() {
    assert_eq!(
        replay(
            "ses_e77d58794ffeObfYpP7mdzpMC0",
            fixture!("question-reply.sse")
        ),
        "created:pMC0 prompted busy tool:execute delta*3 tool:execute delta*13 tool:question q_asked@pMC0 q_resolved@pMC0 delta idle"
    );
}

#[test]
fn question_cancelled_resolves_and_the_interrupt_ends_the_turn() {
    assert_eq!(
        replay(
            "ses_e77d55b37ffeQVaTaXT6P7L7ks",
            fixture!("question-cancel.sse")
        ),
        "created:L7ks prompted busy delta*7 tool:question q_asked@L7ks q_resolved@L7ks idle"
    );
}

#[test]
fn subagent_turn_goes_idle_exactly_once_at_the_end() {
    let seq = translate_all(
        &mut V2Translator::new(Some("ses_e77d504baffeXLjjKh7xflE3Bk")),
        fixture!("subagent.sse"),
    );
    assert_eq!(
        seq.iter().filter(|e| matches!(e, E::SessionIdle)).count(),
        1
    );
    assert!(matches!(seq.last(), Some(E::SessionIdle)));
    assert_eq!(
        compact(&seq),
        "created:E3Bk prompted busy delta*11 tool:subagent created:apFP<-E3Bk delta tool:read perm_asked@apFP perm_replied@apFP delta*2 tool:shell perm_asked@apFP perm_replied@apFP delta*6 idle"
    );
}

#[test]
fn interrupt_ends_the_turn_with_idle() {
    let seq = translate_all(
        &mut V2Translator::new(Some("ses_e77d48ac6ffepeEaFg961h1aSO")),
        fixture!("interrupt.sse"),
    );
    assert!(matches!(seq.last(), Some(E::SessionIdle)));
    assert_eq!(compact(&seq), "created:1aSO prompted busy delta*30 idle");
}

#[test]
fn agent_switch_reports_the_new_agent() {
    assert_eq!(
        replay(
            "ses_e77d45f10ffe7tN9eKUWVyDeVL",
            fixture!("agent-switch.sse")
        ),
        "created:DeVL agent:plan@DeVL"
    );
}

#[test]
fn reconnect_after_replays_into_a_fresh_translator_to_work_then_idle() {
    let seq = translate_all(
        &mut V2Translator::new(Some("ses_e77d42725ffeuLGdD79r08PKXi")),
        fixture!("reconnect-after.sse"),
    );
    assert!(seq.iter().any(|e| matches!(e, E::MessageDelta)));
    assert!(matches!(seq.last(), Some(E::SessionIdle)));
    assert_eq!(
        seq.iter().filter(|e| matches!(e, E::SessionIdle)).count(),
        1
    );
    assert_eq!(compact(&seq), "delta*248 idle");
}

#[test]
fn reconnect_before_is_work_with_no_idle() {
    let seq = translate_all(
        &mut V2Translator::new(Some("ses_e77d42725ffeuLGdD79r08PKXi")),
        fixture!("reconnect-before.sse"),
    );
    assert!(!seq.iter().any(|e| matches!(e, E::SessionIdle)));
    assert_eq!(compact(&seq), "created:PKXi prompted busy delta*10");
}

#[test]
fn lookup_extractor_reads_the_real_get_session_bodies() {
    let j = |v: &str| serde_json::from_str::<serde_json::Value>(v).unwrap();
    assert_eq!(is_root_from_body(&j(ROOT_JSON)), Some(true));
    assert_eq!(is_root_from_body(&j(CHILD_JSON)), Some(false));
}
