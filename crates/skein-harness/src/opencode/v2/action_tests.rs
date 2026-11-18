//! The 2.3.2 captures of 2026-11-18 (see `fixtures/README.md`) through
//! `parse_action_event`, and against the stored rows of the same session.

use super::store::tests::{MSGS_A, MSGS_B, steps, tools};
use super::store::{ContentItem, ToolStatus};
use super::*;

const SSE_A: &str = include_str!("fixtures/store-A.sse");
const SSE_B: &str = include_str!("fixtures/store-B.sse");
const SSE_C: &str = include_str!("fixtures/store-C.sse");
const SSE_D: &str = include_str!("fixtures/store-D.sse");
const SSE_E: &str = include_str!("fixtures/store-E.sse");
const SSE_F: &str = include_str!("fixtures/store-F.sse");

fn actions(raw: &str) -> Vec<V2Action> {
    let mut buf = raw.as_bytes().to_vec();
    let out = drain_sse_data(&mut buf);
    assert!(buf.is_empty(), "fixture ends on a block boundary");
    out.iter().filter_map(|p| parse_action_event(p)).collect()
}

fn count(a: &[V2Action], f: impl Fn(&V2ActionKind) -> bool) -> usize {
    a.iter().filter(|x| f(&x.kind)).count()
}

#[test]
fn scenario_a_is_one_prompt_and_one_step() {
    let a = actions(SSE_A);
    assert_eq!(a.len(), 2);
    let V2ActionKind::InboxEnqueued { inbox_id, text } = &a[0].kind else {
        panic!("inbox first, got {:?}", a[0]);
    };
    assert!(inbox_id.starts_with("msg_"));
    assert_ne!(text, "");
    assert_eq!(a[0].assistant_message_id, None);
    let V2ActionKind::StepEnded { finish, .. } = &a[1].kind else {
        panic!("step");
    };
    assert_eq!(finish.as_deref(), Some("stop"));
}

#[test]
fn scenario_b_counts_and_exact_values() {
    let a = actions(SSE_B);
    assert_eq!(
        count(&a, |k| matches!(k, V2ActionKind::StepEnded { .. })),
        2
    );
    assert_eq!(
        count(&a, |k| matches!(k, V2ActionKind::ToolInputStarted { .. })),
        2
    );
    assert_eq!(
        count(&a, |k| matches!(k, V2ActionKind::ToolCalled { .. })),
        2
    );
    assert_eq!(
        count(&a, |k| matches!(k, V2ActionKind::ToolSuccess { .. })),
        2
    );
    let started = a
        .iter()
        .find(|x| matches!(x.kind, V2ActionKind::ToolInputStarted { .. }))
        .unwrap();
    assert_eq!(started.created, 1_795_006_715_717);
    assert_eq!(started.session_id, "ses_e114330feffek27Wru67RSzCAQ");
    assert_eq!(
        started.assistant_message_id.as_deref(),
        Some("msg_1eebcd00a001Yj7N6nR0QckTBI")
    );
    assert_eq!(
        started.kind,
        V2ActionKind::ToolInputStarted {
            call_id: "call_7ded820d7bc045a990a3922d".into(),
            name: "read".into()
        }
    );
}

#[test]
fn scenario_c_reasoning_and_edit_metadata() {
    let a = actions(SSE_C);
    let r = a
        .iter()
        .find_map(|x| match &x.kind {
            V2ActionKind::ReasoningEnded { ordinal, text } => Some((*ordinal, text.clone())),
            _ => None,
        })
        .expect("reasoning.ended");
    assert_eq!(r, (0, "Let me check a.txt exists.".to_owned()));
    let patched = a.iter().any(|x| match &x.kind {
        V2ActionKind::ToolSuccess { metadata, .. } => metadata["files"][0]["patch"]
            .as_str()
            .is_some_and(|p| p.contains("+hello skein")),
        _ => false,
    });
    assert!(patched, "edit success frame carries the patch");
}

#[test]
fn scenario_d_has_reasoning_and_no_tools() {
    let a = actions(SSE_D);
    assert!(count(&a, |k| matches!(k, V2ActionKind::ReasoningEnded { .. })) > 0);
    assert_eq!(
        count(&a, |k| matches!(k, V2ActionKind::ToolCalled { .. })),
        0
    );
}

#[test]
fn scenario_e_failure_carries_the_message() {
    let a = actions(SSE_E);
    let err: Vec<_> = a
        .iter()
        .filter_map(|x| match &x.kind {
            V2ActionKind::ToolFailed { call_id, error } => {
                Some((call_id.as_str(), error.as_deref()))
            }
            _ => None,
        })
        .collect();
    assert_eq!(
        err,
        [(
            "call_1b65ca0744e546f0a864720d",
            Some("File not found: does-not-exist.txt")
        )]
    );
}

#[test]
fn scenario_f_has_two_sessions() {
    let a = actions(SSE_F);
    let mut sessions: Vec<&str> = a.iter().map(|x| x.session_id.as_str()).collect();
    sessions.sort_unstable();
    sessions.dedup();
    assert_eq!(sessions.len(), 2, "root and child frames");
}

#[test]
fn non_action_and_malformed_frames_are_none() {
    assert!(parse_action_event("not json").is_none());
    assert!(
        parse_action_event(r#"{"type":"session.text.delta","created":1,"data":{"sessionID":"s"}}"#)
            .is_none()
    );
    // A non-user inbox item is not a prompt.
    assert!(
        parse_action_event(
            r#"{"type":"session.inbox.enqueued","created":1,"data":{"inboxID":"i","sessionID":"s","item":{"type":"other"}}}"#
        )
        .is_none()
    );
}

#[test]
fn sse_step_and_tool_data_equal_the_stored_rows_of_b() {
    let a = actions(SSE_B);
    let stored = steps(MSGS_B);
    assert_eq!(stored.len(), 2);
    for (id, step) in &stored {
        // Step data.
        let ended = a
            .iter()
            .find(|x| {
                x.assistant_message_id.as_deref() == Some(id)
                    && matches!(x.kind, V2ActionKind::StepEnded { .. })
            })
            .expect("step.ended for stored assistant row");
        let V2ActionKind::StepEnded {
            finish,
            raw_finish,
            tokens,
            cost,
        } = &ended.kind
        else {
            unreachable!()
        };
        assert_eq!(finish, &step.finish);
        assert_eq!(raw_finish, &step.raw_finish);
        assert_eq!(tokens, &step.tokens);
        assert!((cost - step.cost).abs() < f64::EPSILON);
        // Tool data, by call id.
        for t in tools(step) {
            assert_eq!(t.status, ToolStatus::Completed);
            let mine = |x: &&V2Action| x.assistant_message_id.as_deref() == Some(id);
            let name = a.iter().filter(mine).find_map(|x| match &x.kind {
                V2ActionKind::ToolInputStarted { call_id, name } if *call_id == t.id => Some(name),
                _ => None,
            });
            assert_eq!(name, Some(&t.name));
            let input = a.iter().filter(mine).find_map(|x| match &x.kind {
                V2ActionKind::ToolCalled { call_id, input } if *call_id == t.id => Some(input),
                _ => None,
            });
            assert_eq!(input, Some(&t.input));
            let res = a.iter().filter(mine).find_map(|x| match &x.kind {
                V2ActionKind::ToolSuccess {
                    call_id,
                    result,
                    metadata,
                } if *call_id == t.id => Some((result, metadata)),
                _ => None,
            });
            assert_eq!(res, Some((&t.result, &t.metadata)));
        }
    }
    // Plain text step of A matches too (no tools, no tool frames).
    let a_steps = steps(MSGS_A);
    assert!(
        a_steps[0]
            .1
            .content
            .iter()
            .all(|c| matches!(c, ContentItem::Text(_)))
    );
}
