//! The six 2.2.0 captures of 2026-10-29 (see `fixtures/README.md`), replayed
//! through the framer and parser.

use super::*;

const PERM: &str = include_str!("fixtures/permission-reply.sse");
const QREPLY: &str = include_str!("fixtures/question-reply.sse");
const QCANCEL: &str = include_str!("fixtures/question-cancel.sse");
const SUB: &str = include_str!("fixtures/subagent.sse");
const INTERRUPT: &str = include_str!("fixtures/interrupt.sse");
const AGENT: &str = include_str!("fixtures/agent-switch.sse");
const BEFORE: &str = include_str!("fixtures/reconnect-before.sse");
const AFTER: &str = include_str!("fixtures/reconnect-after.sse");

fn replay(raw: &str) -> Vec<V2Event> {
    let mut buf = raw.as_bytes().to_vec();
    let out = drain_sse_data(&mut buf);
    assert!(buf.is_empty(), "fixture ends on a block boundary");
    out.iter().filter_map(|p| parse_event(p)).collect()
}

/// Everything but the high-volume streaming/tool noise.
fn spine(events: &[V2Event]) -> Vec<&V2Event> {
    events
        .iter()
        .filter(|e| {
            !matches!(
                e,
                V2Event::Streaming { .. }
                    | V2Event::ToolInputStarted { .. }
                    | V2Event::ToolCalled { .. }
                    | V2Event::ToolEnded { .. }
            )
        })
        .collect()
}

fn s(x: &str) -> String {
    x.to_owned()
}

#[test]
fn permission_reply_pairs_by_request_id_and_the_turn_succeeds() {
    let ev = replay(PERM);
    let sid = "ses_e77d5e587ffeylOS3IgWhX1xrR";
    let rid = "per_1882a2665001Kog56Z2g7goL4g";
    assert_eq!(
        spine(&ev),
        vec![
            &V2Event::ServerConnected,
            &V2Event::SessionCreated {
                session_id: s(sid),
                parent_id: None
            },
            &V2Event::UserPrompt {
                session_id: s(sid),
                inbox_id: s("msg_1882a1ad1001bNqu4Gf8JacJY4")
            },
            &V2Event::ExecutionStarted { session_id: s(sid) },
            &V2Event::PermissionAsked {
                request_id: s(rid),
                session_id: s(sid)
            },
            &V2Event::PermissionReplied {
                request_id: s(rid),
                session_id: s(sid)
            },
            &V2Event::ExecutionEnded {
                session_id: s(sid),
                outcome: ExecutionOutcome::Succeeded
            },
        ]
    );
    assert!(
        ev.iter().all(|e| !matches!(
            e,
            V2Event::SessionCreated { session_id, .. } if session_id != sid
        )),
        "trimmed to the one session"
    );
}

#[test]
fn question_reply_resolves_the_created_form() {
    let ev = replay(QREPLY);
    let sid = "ses_e77d58794ffeObfYpP7mdzpMC0";
    let fid = "frm_1882a9299001A7Vff3fT6LKx6G";
    let sp = spine(&ev);
    let created = V2Event::FormCreated {
        form_id: s(fid),
        session_id: Some(s(sid)),
        kind: Some(s("question")),
    };
    let resolved = V2Event::FormResolved {
        form_id: s(fid),
        session_id: Some(s(sid)),
        outcome: FormOutcome::Replied,
    };
    let at = |e: &V2Event| sp.iter().position(|x| *x == e).unwrap();
    assert!(at(&created) < at(&resolved));
    assert_eq!(
        sp.last(),
        Some(&&V2Event::ExecutionEnded {
            session_id: s(sid),
            outcome: ExecutionOutcome::Succeeded
        })
    );
}

#[test]
fn question_cancel_resolves_cancelled_and_the_turn_is_interrupted() {
    let ev = replay(QCANCEL);
    let sid = "ses_e77d55b37ffeQVaTaXT6P7L7ks";
    let fid = "frm_1882ab56b001QoQ9pQLCWPj7kI";
    let sp = spine(&ev);
    assert_eq!(
        sp[sp.len() - 3..],
        [
            &V2Event::FormCreated {
                form_id: s(fid),
                session_id: Some(s(sid)),
                kind: Some(s("question"))
            },
            &V2Event::FormResolved {
                form_id: s(fid),
                session_id: Some(s(sid)),
                outcome: FormOutcome::Cancelled
            },
            &V2Event::ExecutionEnded {
                session_id: s(sid),
                outcome: ExecutionOutcome::Interrupted
            },
        ]
    );
}

#[test]
fn subagent_child_is_created_under_the_root_and_runs_its_own_turn() {
    let ev = replay(SUB);
    let root = "ses_e77d504baffeXLjjKh7xflE3Bk";
    let child = "ses_e77d4f854ffeQFm1p4AMZAapFP";
    assert!(ev.contains(&V2Event::SessionCreated {
        session_id: s(root),
        parent_id: None
    }));
    assert!(ev.contains(&V2Event::SessionCreated {
        session_id: s(child),
        parent_id: Some(s(root))
    }));
    let ended = |sid: &str| V2Event::ExecutionEnded {
        session_id: s(sid),
        outcome: ExecutionOutcome::Succeeded,
    };
    let pos = |e: &V2Event| ev.iter().position(|x| x == e).unwrap();
    assert!(pos(&ended(child)) < pos(&ended(root)));
    assert_eq!(ev.last(), Some(&ended(root)));
    // the child's own two permission round trips
    let asked: Vec<&str> = ev
        .iter()
        .filter_map(|e| match e {
            V2Event::PermissionAsked {
                request_id,
                session_id,
            } if session_id == child => Some(request_id.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(
        asked,
        [
            "per_1882b0f6d001hcmMyCEY5qT6a7",
            "per_1882b1aae001EZ6Sbvl6UkZvqf"
        ]
    );
    for rid in asked {
        assert!(ev.contains(&V2Event::PermissionReplied {
            request_id: s(rid),
            session_id: s(child)
        }));
    }
}

#[test]
fn interrupt_ends_the_turn_as_interrupted() {
    let ev = replay(INTERRUPT);
    assert_eq!(
        ev.last(),
        Some(&V2Event::ExecutionEnded {
            session_id: s("ses_e77d48ac6ffepeEaFg961h1aSO"),
            outcome: ExecutionOutcome::Interrupted
        })
    );
}

#[test]
fn agent_switch_is_a_selection_with_no_turn() {
    let ev = replay(AGENT);
    let sid = "ses_e77d45f10ffe7tN9eKUWVyDeVL";
    assert_eq!(
        ev,
        vec![
            V2Event::ServerConnected,
            V2Event::SessionCreated {
                session_id: s(sid),
                parent_id: None
            },
            V2Event::AgentSelected {
                session_id: s(sid),
                agent: s("plan")
            },
        ]
    );
}

#[test]
fn reconnect_streams_split_one_turn_and_the_second_opens_with_the_banner() {
    let sid = "ses_e77d42725ffeuLGdD79r08PKXi";
    let before = replay(BEFORE);
    let after = replay(AFTER);
    assert_eq!(before.first(), Some(&V2Event::ServerConnected));
    assert!(before.contains(&V2Event::ExecutionStarted { session_id: s(sid) }));
    assert!(
        !before
            .iter()
            .any(|e| matches!(e, V2Event::ExecutionEnded { .. })),
        "killed mid-turn"
    );
    // the new stream: banner first, then live frames only, no replay of
    // session.created / the prompt / execution.started
    assert_eq!(after.first(), Some(&V2Event::ServerConnected));
    assert!(after[1..].iter().all(|e| !matches!(
        e,
        V2Event::SessionCreated { .. }
            | V2Event::UserPrompt { .. }
            | V2Event::ExecutionStarted { .. }
    )));
    assert!(
        after[1..]
            .iter()
            .any(|e| matches!(e, V2Event::Streaming { .. }))
    );
    assert_eq!(
        after.last(),
        Some(&V2Event::ExecutionEnded {
            session_id: s(sid),
            outcome: ExecutionOutcome::Succeeded
        })
    );
}
