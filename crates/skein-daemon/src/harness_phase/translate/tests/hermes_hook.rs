//! Port of `app/src/hermesHookAdapter.test.ts`: the reducer table, one test
//! per vitest case, run over the recorded CLI and TUI hook fixtures. The
//! second half is `hermes_hook_approval_tests`.

// `assert!(x.is_empty())` reads better than a typed empty literal here.
#![allow(clippy::assert_is_empty)]

use serde_json::Value;

use crate::agent_api::hermes_event::HarnessHermesHook;
use crate::harness_phase::translate::hermes_hook::{
    HermesAdapterState, HermesEffect, reduce_hermes_hook,
};

const CLI_A: &str = include_str!("../../../../../skein-harness/src/hermes/fixtures/hooks-A.jsonl");
const CLI_B: &str = include_str!("../../../../../skein-harness/src/hermes/fixtures/hooks-B.jsonl");
const CLI_C: &str = include_str!("../../../../../skein-harness/src/hermes/fixtures/hooks-C.jsonl");
const CLI_D: &str = include_str!("../../../../../skein-harness/src/hermes/fixtures/hooks-D.jsonl");
const TUI_A: &str =
    include_str!("../../../../../skein-harness/src/hermes/fixtures/tui-hooks-A.jsonl");
const TUI_B: &str =
    include_str!("../../../../../skein-harness/src/hermes/fixtures/tui-hooks-B.jsonl");
const TUI_C: &str =
    include_str!("../../../../../skein-harness/src/hermes/fixtures/tui-hooks-C.jsonl");

#[allow(clippy::unnecessary_wraps)]
pub(super) fn s(v: &str) -> Option<String> {
    Some(v.to_owned())
}

fn str_of(v: Option<&Value>) -> Option<String> {
    v.and_then(Value::as_str).map(str::to_owned)
}

fn bool_of(v: Option<&Value>) -> Option<bool> {
    v.and_then(Value::as_bool)
}

/// Maps a recorded `snake_case` payload to the event, as `hermes_hooks.rs` does.
fn to_event(raw: &Value) -> HarnessHermesHook {
    let x = raw.get("extra");
    let f = |k: &str| x.and_then(|x| x.get(k));
    HarnessHermesHook {
        room_id: "r".to_owned(),
        harness_id: "h".to_owned(),
        event: str_of(raw.get("hook_event_name")).unwrap_or_default(),
        session_id: str_of(raw.get("session_id")),
        turn_id: str_of(f("turn_id")),
        tool_call_id: str_of(f("tool_call_id")),
        tool_name: str_of(raw.get("tool_name")),
        platform: str_of(f("platform")),
        surface: str_of(f("surface")),
        description: str_of(f("description")),
        choice: str_of(f("choice")),
        completed: bool_of(f("completed")),
        interrupted: bool_of(f("interrupted")),
        failed: bool_of(f("failed")),
        turn_exit_reason: str_of(f("turn_exit_reason")),
        model: str_of(f("model")),
        child_session_id: str_of(f("child_session_id")),
    }
}

fn fixture(text: &str) -> Vec<HarnessHermesHook> {
    text.lines()
        .filter(|l| !l.is_empty())
        .map(|l| to_event(&serde_json::from_str::<Value>(l).expect("fixture line")))
        .collect()
}

pub(super) fn kind(e: &HermesEffect) -> &'static str {
    match e {
        HermesEffect::SessionStarted { .. } => "sessionStarted",
        HermesEffect::Running => "running",
        HermesEffect::Waiting { .. } => "waiting",
        HermesEffect::PermissionOpen { .. } => "permissionOpen",
        HermesEffect::PermissionAnswered => "permissionAnswered",
        HermesEffect::PermissionAnsweredIdle => "permissionAnsweredIdle",
        HermesEffect::TurnEnded => "turnEnded",
        HermesEffect::SubagentStart { .. } => "subagentStart",
        HermesEffect::SubagentEnd { .. } => "subagentEnd",
    }
}

fn kinds_of(effects: &[HermesEffect]) -> Vec<&'static str> {
    effects.iter().map(kind).collect()
}

pub(super) struct Run {
    pub(super) state: HermesAdapterState,
    pub(super) steps: Vec<(HarnessHermesHook, Vec<HermesEffect>)>,
    pub(super) kinds: Vec<&'static str>,
}

/// Feed events; returns per-event effects plus the final state.
pub(super) fn run(events: &[HarnessHermesHook], start: &HermesAdapterState) -> Run {
    let mut state = start.clone();
    let mut steps = Vec::new();
    for e in events {
        let effects = reduce_hermes_hook(&mut state, e, None);
        steps.push((e.clone(), effects));
    }
    let kinds = steps
        .iter()
        .flat_map(|(_, fx)| fx.iter().map(kind))
        .collect();
    Run {
        state,
        steps,
        kinds,
    }
}

pub(super) fn run0(events: &[HarnessHermesHook]) -> Run {
    run(events, &HermesAdapterState::default())
}

pub(super) fn ev(event: &str) -> HarnessHermesHook {
    HarnessHermesHook {
        room_id: "r".to_owned(),
        harness_id: "h".to_owned(),
        event: event.to_owned(),
        session_id: s("S"),
        ..HarnessHermesHook::default()
    }
}

pub(super) fn req() -> HarnessHermesHook {
    HarnessHermesHook {
        surface: s("cli"),
        turn_id: s("T"),
        description: s("rm"),
        ..ev("pre_approval_request")
    }
}

pub(super) fn resp() -> HarnessHermesHook {
    HarnessHermesHook {
        surface: s("cli"),
        turn_id: s("T"),
        description: s("rm"),
        ..ev("post_approval_response")
    }
}

pub(super) fn in_turn() -> HermesAdapterState {
    HermesAdapterState {
        main_turn_open: true,
        ..HermesAdapterState::default()
    }
}

pub(super) fn keys(st: &HermesAdapterState) -> Vec<&str> {
    st.open_approvals.iter().map(|a| a.key.as_str()).collect()
}

fn plain_turn(text: &str) {
    let r = run0(&fixture(text));
    assert_eq!(
        r.kinds,
        [
            "sessionStarted",
            "running",
            "running",
            "waiting",
            "turnEnded"
        ]
    );
    assert!(r.state.open_approvals.is_empty());
}

#[test]
fn plain_turn_a_hooks_a_jsonl_start_running_end_waiting_turn_ended() {
    plain_turn(CLI_A);
}

#[test]
fn plain_turn_a_tui_hooks_a_jsonl_start_running_end_waiting_turn_ended() {
    plain_turn(TUI_A);
}

fn approvals(text: &str) {
    let events = fixture(text);
    let r = run0(&events);
    let real = events
        .iter()
        .filter(|e| e.event == "pre_approval_request" && e.surface.as_deref() != Some("smart"))
        .count();
    let opens = r
        .steps
        .iter()
        .filter(|(_, fx)| fx.iter().any(|e| kind(e) == "permissionOpen"))
        .count();
    assert_eq!(opens, real);
    for (e, fx) in &r.steps {
        if e.surface.as_deref() == Some("smart") {
            assert!(fx.is_empty());
        }
    }
    let answered = r
        .steps
        .iter()
        .filter(|(_, fx)| fx.iter().any(|e| kind(e) == "permissionAnswered"))
        .count();
    assert_eq!(answered, opens);
    assert!(r.state.open_approvals.is_empty());
    let open_at = r.kinds.iter().position(|k| *k == "permissionOpen");
    let answered_at = r.kinds.iter().position(|k| *k == "permissionAnswered");
    assert!(open_at < answered_at);
}

#[test]
fn approvals_b_hooks_b_jsonl_smart_pre_check_ignored_real_dialog_opens_and_closes() {
    approvals(CLI_B);
}

#[test]
fn approvals_b_tui_hooks_b_jsonl_smart_pre_check_ignored_real_dialog_opens_and_closes() {
    approvals(TUI_B);
}

#[test]
fn a_tool_call_that_follows_the_dialog_never_reopens_or_clears_it() {
    let out = run0(&[
        HarnessHermesHook {
            tool_call_id: s("c1"),
            ..req()
        },
        ev("pre_tool_call"),
    ]);
    assert_eq!(out.state.open_approvals.len(), 1);
}

fn interrupted(text: &str) {
    let r = run0(&fixture(text));
    let end = r
        .steps
        .iter()
        .find(|(e, _)| e.event == "on_session_end")
        .expect("session end");
    assert!(end.1.contains(&HermesEffect::Waiting { interrupted: true }));
}

#[test]
fn interrupt_c_hooks_c_jsonl_ends_the_turn_as_interrupted() {
    interrupted(CLI_C);
}

#[test]
fn interrupt_c_tui_hooks_c_jsonl_ends_the_turn_as_interrupted() {
    interrupted(TUI_C);
}

#[test]
fn child_events_do_not_flip_the_parent_the_parent_end_waits() {
    let events = fixture(CLI_D);
    let parent = events[0].session_id.clone();
    let r = run0(&events);
    assert_eq!(r.state.child_sessions.len(), 1);
    for (e, fx) in &r.steps {
        if e.session_id != parent && e.session_id.is_some() {
            assert!(fx.is_empty());
        }
    }
    let stop = r
        .steps
        .iter()
        .find(|(e, _)| e.event == "subagent_stop")
        .expect("subagent_stop");
    assert_eq!(kinds_of(&stop.1), ["subagentEnd"]);
    let ends: Vec<_> = r
        .steps
        .iter()
        .filter(|(e, _)| e.event == "on_session_end" && e.session_id == parent)
        .collect();
    assert!(!ends.is_empty());
    for (_, fx) in ends {
        assert_eq!(kinds_of(fx), ["waiting", "turnEnded"]);
    }
}

#[test]
fn a_childs_first_hook_before_subagent_start_is_still_treated_as_a_child() {
    let out = run0(&[HarnessHermesHook {
        session_id: s("C"),
        platform: s("subagent"),
        ..ev("on_session_start")
    }]);
    assert!(out.kinds.is_empty());
    assert_eq!(out.state.child_sessions, ["C"]);
    let next = run(
        &[HarnessHermesHook {
            session_id: s("C"),
            ..ev("pre_llm_call")
        }],
        &out.state,
    );
    assert!(next.kinds.is_empty());
}

#[test]
fn a_childs_approval_opens_permission() {
    let s0 = run0(&[HarnessHermesHook {
        child_session_id: s("C"),
        ..ev("subagent_start")
    }])
    .state;
    let out = run(
        &[HarnessHermesHook {
            session_id: s("C"),
            tool_call_id: s("k"),
            tool_name: s("terminal"),
            ..req()
        }],
        &s0,
    );
    assert_eq!(
        out.steps[0].1,
        [HermesEffect::PermissionOpen {
            tool_name: s("terminal"),
            description: s("rm"),
        }]
    );
}

#[test]
fn tracks_open_and_end_of_the_main_turn_a_child_never_touches_it() {
    let open = run0(&[ev("on_session_start")]).state;
    assert!(open.main_turn_open);
    let child = run(
        &[HarnessHermesHook {
            session_id: s("C"),
            platform: s("subagent"),
            ..ev("on_session_end")
        }],
        &open,
    )
    .state;
    assert!(child.main_turn_open);
    assert!(!run(&[ev("on_session_end")], &open).state.main_turn_open);
    assert!(run0(&[ev("pre_tool_call")]).state.main_turn_open);
    assert!(run0(&[ev("pre_llm_call")]).state.main_turn_open);
}

#[test]
fn an_answer_after_the_main_turn_ended_settles_to_waiting_not_running() {
    let out = run0(&[
        ev("on_session_start"),
        HarnessHermesHook {
            child_session_id: s("C"),
            ..ev("subagent_start")
        },
        ev("on_session_end"),
        HarnessHermesHook {
            session_id: s("C"),
            tool_call_id: s("k"),
            ..req()
        },
        HarnessHermesHook {
            session_id: s("C"),
            tool_call_id: s("k"),
            ..resp()
        },
    ]);
    assert_eq!(out.steps[4].1, [HermesEffect::PermissionAnsweredIdle]);
}

#[test]
fn an_answer_during_an_open_turn_returns_to_running() {
    let out = run0(&[
        ev("on_session_start"),
        HarnessHermesHook {
            tool_call_id: s("k"),
            ..req()
        },
        HarnessHermesHook {
            tool_call_id: s("k"),
            ..resp()
        },
    ]);
    assert_eq!(out.steps[2].1, [HermesEffect::PermissionAnswered]);
}
