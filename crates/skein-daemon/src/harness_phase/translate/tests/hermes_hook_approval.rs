//! Port of `app/src/hermesHookAdapter.test.ts`, second half: the approval
//! bookkeeping, turn ends, session and subagent effects. The helpers and the
//! first half (fixture replays, child events) are in `hermes_hook_tests`.

// `assert!(x.is_empty())` reads better than a typed empty literal here.
#![allow(clippy::assert_is_empty)]

use super::hermes_hook::{ev, in_turn, keys, kind, req, resp, run, run0, s};
use crate::agent_api::hermes_event::HarnessHermesHook;
use crate::harness_phase::translate::hermes_hook::{
    HermesAdapterState, HermesEffect, reduce_hermes_hook,
};

#[test]
fn two_same_description_approvals_without_tool_call_id_are_a_multiset() {
    let two = run(&[req(), req()], &in_turn());
    assert_eq!(two.state.open_approvals.len(), 2);
    let one = run(&[resp()], &two.state);
    assert_eq!(one.state.open_approvals.len(), 1);
    assert_eq!(kind(&one.steps[0].1[0]), "permissionOpen");
    let none = run(&[resp()], &one.state);
    assert_eq!(none.kinds, ["permissionAnswered"]);
    assert!(none.state.open_approvals.is_empty());
}

#[test]
fn with_tool_call_id_the_matching_call_is_removed_not_the_oldest() {
    let open = run0(&[
        HarnessHermesHook {
            tool_call_id: s("a"),
            ..req()
        },
        HarnessHermesHook {
            tool_call_id: s("b"),
            ..req()
        },
    ])
    .state;
    let out = run(
        &[HarnessHermesHook {
            tool_call_id: s("b"),
            ..resp()
        }],
        &open,
    );
    assert_eq!(keys(&out.state), ["call:a"]);
}

#[test]
fn a_post_whose_tool_call_id_matches_nothing_clears_nothing() {
    let open = run0(&[
        HarnessHermesHook {
            tool_call_id: s("a"),
            ..req()
        },
        HarnessHermesHook {
            tool_call_id: s("b"),
            ..req()
        },
    ])
    .state;
    let out = run(
        &[HarnessHermesHook {
            tool_call_id: s("zzz"),
            ..resp()
        }],
        &open,
    );
    assert_eq!(out.state, open);
    assert!(out.kinds.is_empty());
}

#[test]
fn without_a_tool_call_id_turn_id_description_wins_over_the_oldest_of_the_turn() {
    let open = run0(&[
        HarnessHermesHook {
            tool_call_id: s("a"),
            description: s("ls"),
            ..req()
        },
        HarnessHermesHook {
            tool_call_id: s("b"),
            description: s("rm"),
            ..req()
        },
    ])
    .state;
    let out = run(
        &[HarnessHermesHook {
            description: s("rm"),
            ..resp()
        }],
        &open,
    );
    assert_eq!(keys(&out.state), ["call:a"]);
}

#[test]
fn without_a_tool_call_id_or_a_description_match_the_oldest_of_the_turn_goes() {
    let open = run0(&[
        HarnessHermesHook {
            tool_call_id: s("a"),
            description: s("ls"),
            ..req()
        },
        HarnessHermesHook {
            tool_call_id: s("b"),
            description: s("cat"),
            ..req()
        },
    ])
    .state;
    let out = run(
        &[HarnessHermesHook {
            description: s("other"),
            ..resp()
        }],
        &open,
    );
    assert_eq!(keys(&out.state), ["call:b"]);
}

#[test]
fn a_post_in_another_turn_or_with_nothing_open_does_nothing() {
    let open = run0(&[HarnessHermesHook {
        tool_call_id: s("a"),
        ..req()
    }])
    .state;
    let other = HarnessHermesHook {
        turn_id: s("OTHER"),
        ..resp()
    };
    assert_eq!(run(&[other], &open).state, open);
    let other_call = HarnessHermesHook {
        tool_call_id: s("z"),
        turn_id: s("OTHER"),
        ..resp()
    };
    assert_eq!(run(&[other_call], &open).state, open);
    assert!(run0(&[resp()]).kinds.is_empty());
}

#[test]
fn null_turn_ids_on_both_sides_match_each_other() {
    let open = run0(&[HarnessHermesHook {
        turn_id: None,
        description: None,
        ..req()
    }])
    .state;
    let answer = HarnessHermesHook {
        turn_id: None,
        description: None,
        ..resp()
    };
    assert!(
        run(std::slice::from_ref(&answer), &open)
            .state
            .open_approvals
            .is_empty()
    );
    let loose = run0(&[HarnessHermesHook {
        turn_id: None,
        description: s("x"),
        ..req()
    }])
    .state;
    assert!(
        run(std::slice::from_ref(&answer), &loose)
            .state
            .open_approvals
            .is_empty()
    );
}

#[test]
fn turn_end_clears_every_open_approval() {
    let open = run0(&[
        req(),
        HarnessHermesHook {
            tool_call_id: s("x"),
            ..req()
        },
    ])
    .state;
    let out = run(&[ev("on_session_end")], &open);
    assert!(out.state.open_approvals.is_empty());
    assert_eq!(out.kinds, ["waiting", "turnEnded"]);
}

#[test]
fn session_start_reports_whether_the_id_changed() {
    let e = HarnessHermesHook {
        session_id: s("S2"),
        ..ev("on_session_start")
    };
    let mut st = HermesAdapterState::default();
    let same = reduce_hermes_hook(&mut st, &e, Some("S2"));
    assert_eq!(
        same[0],
        HermesEffect::SessionStarted {
            session_id: s("S2"),
            changed: false
        }
    );
    let mut st = HermesAdapterState::default();
    let diff = reduce_hermes_hook(&mut st, &e, Some("S1"));
    assert_eq!(
        diff[0],
        HermesEffect::SessionStarted {
            session_id: s("S2"),
            changed: true
        }
    );
}

#[test]
fn subagent_start_subagent_stop_carry_the_child_id() {
    let r = run0(&[
        HarnessHermesHook {
            child_session_id: s("C"),
            ..ev("subagent_start")
        },
        HarnessHermesHook {
            child_session_id: s("C"),
            ..ev("subagent_stop")
        },
    ]);
    assert_eq!(r.kinds, ["subagentStart", "subagentEnd"]);
    assert_eq!(
        r.steps[0].1[0],
        HermesEffect::SubagentStart {
            agent_id: "C".to_owned(),
            description: None
        }
    );
    assert_eq!(
        r.steps[1].1[0],
        HermesEffect::SubagentEnd {
            agent_id: "C".to_owned()
        }
    );
}

#[test]
fn no_child_id_no_effect_a_childs_own_hooks_yield_no_subagent_effects() {
    assert!(
        run0(&[ev("subagent_start"), ev("subagent_stop")])
            .kinds
            .is_empty()
    );
    let child = |event: &str| HarnessHermesHook {
        session_id: s("C"),
        platform: s("subagent"),
        ..ev(event)
    };
    let r = run0(&[
        HarnessHermesHook {
            child_session_id: s("C"),
            ..ev("subagent_start")
        },
        child("on_session_start"),
        child("pre_llm_call"),
        child("on_session_end"),
    ]);
    assert_eq!(r.kinds, ["subagentStart"]);
}

#[test]
fn a_child_whose_own_start_beat_subagent_start_still_produces_the_effect() {
    let r = run0(&[
        HarnessHermesHook {
            session_id: s("C"),
            platform: s("subagent"),
            ..ev("on_session_start")
        },
        HarnessHermesHook {
            child_session_id: s("C"),
            ..ev("subagent_start")
        },
    ]);
    assert_eq!(r.kinds, ["subagentStart"]);
}
