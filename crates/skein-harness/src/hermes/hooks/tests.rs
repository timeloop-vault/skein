use super::*;
use serde_json::json;

fn load(raw: &str) -> Vec<HermesHook> {
    raw.lines()
        .map(|l| parse_hook(&serde_json::from_str(l).unwrap()).expect("every line parses"))
        .collect()
}

fn a() -> Vec<HermesHook> {
    load(include_str!("../fixtures/hooks-A.jsonl"))
}
fn b() -> Vec<HermesHook> {
    load(include_str!("../fixtures/hooks-B.jsonl"))
}
fn c() -> Vec<HermesHook> {
    load(include_str!("../fixtures/hooks-C.jsonl"))
}
fn d() -> Vec<HermesHook> {
    load(include_str!("../fixtures/hooks-D.jsonl"))
}
fn e() -> Vec<HermesHook> {
    load(include_str!("../fixtures/hooks-E.jsonl"))
}

fn other(s: &str) -> HermesHookEvent {
    HermesHookEvent::Other(s.into())
}

fn empty() -> HermesHook {
    HermesHook {
        event: HermesHookEvent::SessionStart,
        session_id: None,
        turn_id: None,
        tool_call_id: None,
        tool_name: None,
        platform: None,
        surface: None,
        description: None,
        choice: None,
        completed: None,
        interrupted: None,
        failed: None,
        turn_exit_reason: None,
        model: None,
        child_session_id: None,
        schema_version: None,
    }
}

#[test]
fn plain_turn_sequence() {
    let h = a();
    let events: Vec<_> = h.iter().map(|x| x.event.clone()).collect();
    assert_eq!(
        events,
        [
            HermesHookEvent::SessionStart,
            HermesHookEvent::PreLlmCall,
            other("post_llm_call"),
            HermesHookEvent::SessionEnd,
            other("on_session_finalize"),
        ]
    );
    assert!(
        h.iter()
            .all(|x| x.schema_version.as_deref() == Some("hermes.observer.v1"))
    );
    assert!(h[0].turn_id.is_none(), "no turn before the first prompt");
    assert!(h[1].turn_id.is_some());
    assert_eq!(h[1].turn_id, h[3].turn_id);
    assert_eq!(h[3].completed, Some(true));
    assert_eq!(h[3].interrupted, Some(false));
    assert!(!h[3].ended_interrupted());
    assert!(h[3].model.is_some());
    assert!(!h[3].is_subagent());
}

#[test]
fn approvals_pair_and_smart_precheck_is_filtered() {
    let h = b();
    let requests = h
        .iter()
        .filter(|x| x.event == HermesHookEvent::ApprovalRequest)
        .count();
    assert_eq!(requests, 4);
    let posts: Vec<&HermesHook> = h
        .iter()
        .filter(|x| x.event == HermesHookEvent::ApprovalResponse)
        .collect();
    assert_eq!(posts.len(), 2);
    for post in &posts {
        assert!(post.approval_key().is_some());
        let real: Vec<_> = h
            .iter()
            .filter(|x| x.event == HermesHookEvent::ApprovalRequest && !x.is_smart_precheck())
            .filter(|x| x.approval_key() == post.approval_key())
            .collect();
        assert_eq!(real.len(), 1, "one dialog per answered approval");
        assert_eq!(real[0].surface.as_deref(), Some("cli"));
    }
    let smart = h.iter().filter(|x| x.is_smart_precheck()).count();
    assert_eq!(smart, 2);
    assert_eq!(posts[0].choice.as_deref(), Some("once"));
    assert_eq!(posts[1].choice.as_deref(), Some("deny"));
    assert_ne!(posts[0].approval_key(), posts[1].approval_key());
    assert_eq!(posts[0].description.as_deref(), Some("recursive delete"));
}

#[test]
fn interrupt_ends_the_turn_as_interrupted() {
    let h = c();
    let end = h
        .iter()
        .find(|x| x.event == HermesHookEvent::SessionEnd)
        .unwrap();
    assert_eq!(end.interrupted, Some(true));
    assert_eq!(end.completed, Some(false));
    assert_eq!(
        end.turn_exit_reason.as_deref(),
        Some("interrupted_during_api_call")
    );
    assert!(end.ended_interrupted());
}

#[test]
fn background_delegation_separates_child_from_parent() {
    let h = d();
    let start = h
        .iter()
        .find(|x| x.event == HermesHookEvent::SubagentStart)
        .unwrap();
    let stop = h
        .iter()
        .find(|x| x.event == HermesHookEvent::SubagentStop)
        .unwrap();
    let child = start.child_session_id.clone().expect("child id");
    assert_ne!(start.session_id.as_ref(), Some(&child));
    assert_eq!(stop.child_session_id.as_ref(), Some(&child));
    assert_eq!(start.session_id, stop.session_id);
    assert!(start.turn_id.is_none() && stop.turn_id.is_none());
    assert!(!start.is_subagent());

    let child_ends: Vec<_> = h
        .iter()
        .filter(|x| x.event == HermesHookEvent::SessionEnd && x.is_subagent())
        .collect();
    assert_eq!(child_ends.len(), 1);
    assert_eq!(child_ends[0].session_id.as_ref(), Some(&child));
    let parent_ends = h
        .iter()
        .filter(|x| x.event == HermesHookEvent::SessionEnd && !x.is_subagent())
        .count();
    assert_eq!(parent_ends, 2);
}

#[test]
fn file_edit_tools_carry_name_and_call_id() {
    let h = e();
    let tools: Vec<_> = h
        .iter()
        .filter(|x| x.event == HermesHookEvent::PreToolCall)
        .collect();
    assert_eq!(tools.len(), 2);
    assert!(
        tools
            .iter()
            .all(|t| t.tool_name.is_some() && t.tool_call_id.is_some())
    );
}

#[test]
fn malformed_input_table() {
    for v in [
        json!(null),
        json!("on_session_start"),
        json!([1]),
        json!({}),
        json!({"hook_event_name": 7}),
        json!({"session_id": "s"}),
    ] {
        assert!(parse_hook(&v).is_none(), "{v}");
    }
}

#[test]
fn wrong_types_read_as_absent() {
    let h = parse_hook(&json!({
        "hook_event_name": "on_session_end",
        "session_id": 5,
        "tool_name": ["x"],
        "extra": {"turn_id": 1, "completed": "yes", "interrupted": null, "model": {}, "unknown": 1}
    }))
    .unwrap();
    assert_eq!(
        h,
        HermesHook {
            event: HermesHookEvent::SessionEnd,
            ..empty()
        }
    );
}

#[test]
fn non_object_extra_and_unknown_event() {
    let h = parse_hook(&json!({"hook_event_name": "brand_new", "extra": "oops"})).unwrap();
    assert_eq!(
        h,
        HermesHook {
            event: other("brand_new"),
            ..empty()
        }
    );
}

#[test]
fn approval_key_prefers_call_id_then_turn_and_description() {
    let mut h = HermesHook {
        event: HermesHookEvent::ApprovalRequest,
        ..empty()
    };
    assert_eq!(h.approval_key(), None);
    h.turn_id = Some("t".into());
    assert_eq!(h.approval_key(), None);
    h.description = Some("d".into());
    assert_eq!(
        h.approval_key(),
        Some(ApprovalKey::TurnDescription("t".into(), "d".into()))
    );
    h.tool_call_id = Some("c".into());
    assert_eq!(h.approval_key(), Some(ApprovalKey::ToolCall("c".into())));
}

#[test]
fn tui_dialog_surface_is_gateway_and_pairs_with_the_post() {
    let h = load(include_str!("../fixtures/tui-hooks-B.jsonl"));
    let reals: Vec<_> = h
        .iter()
        .filter(|x| x.event == HermesHookEvent::ApprovalRequest && !x.is_smart_precheck())
        .collect();
    let posts: Vec<_> = h
        .iter()
        .filter(|x| x.event == HermesHookEvent::ApprovalResponse)
        .collect();
    assert_eq!(reals.len(), 2);
    assert_eq!(posts.len(), 2);
    for (real, post) in reals.iter().zip(&posts) {
        assert_eq!(real.surface.as_deref(), Some("gateway"));
        assert_eq!(post.surface.as_deref(), Some("gateway"));
        assert!(real.approval_key().is_some());
        assert_eq!(real.approval_key(), post.approval_key());
    }
    assert_eq!(h.iter().filter(|x| x.is_smart_precheck()).count(), 2);
}
