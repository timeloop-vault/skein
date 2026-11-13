//! #625: the pure half of the Hermes hook adapter (L2c-3). Port of
//! `app/src/hermesHookAdapter.ts`. Hermes' shell hooks reach the daemon as
//! [`HarnessHermesHook`]s (one per hook firing, see
//! `agent_api/http/hermes_hooks.rs`); this reducer turns each into the phase
//! effects [`super::hermes::HermesTranslator`] applies to the machine. No
//! store, no I/O: state in, state + effects out.
//!
//! What the recorded hooks (CLI and TUI, fixtures in crates/skein-harness)
//! establish, and what the rules below lean on:
//! - `on_session_start` fires at the FIRST PROMPT, not at launch, and
//!   `on_session_end` fires at the end of EVERY turn (not only at exit).
//! - A dangerous command emits `pre_approval_request` twice: once with
//!   surface `smart` (Hermes' own LLM pre-check, never answered, no dialog)
//!   and once with the real dialog (`cli`, or `gateway` in the TUI). Only
//!   the real one is paired with a `post_approval_response`.
//! - A background `delegate_task` brings a child session whose own
//!   start/llm/end hooks arrive carrying the CHILD's session id and platform
//!   `subagent`; tool and approval events carry no platform at all, so the
//!   child set is tracked from `subagent_start`.
//!
//! Approval correlation policy (the shell payload has no request id):
//! 1. Key = `toolCallId`; without one, `(turnId, description)`. Open
//!    approvals are a multiset: two identical keys are two dialogs.
//! 2. A response carrying a `toolCallId` removes the open approval with that
//!    id, else nothing (a stray response never clears someone else's
//!    dialog). One without a `toolCallId` removes the oldest open approval
//!    with the same (turnId, description), else the oldest of the same turn
//!    (null turn ids match each other), else nothing.
//! 3. A main-session turn end clears every open approval; the decisive
//!    keystroke clear in the machine still applies on top, because the
//!    answer hook can be lost.
//!
//! Approvals from a child session DO open permission: the harness is
//! blocked on them. Delegation deferral (#628): `subagent_start` /
//! `subagent_stop` become `SubagentStart` / `SubagentEnd` effects the
//! translator records in the subagents registry, so the end of the parent's
//! turn defers while a child runs (background processes arrive from the
//! disk stream, see `hermes.rs`).
//! When the last approval is answered the phase returns to `running` only if
//! the main turn is still open (`main_turn_open`); a child's approval
//! answered after the parent's turn ended has no further hooks coming, so it
//! settles to `waiting` instead of stranding on `running`.
//!
//! Known gap: `child_sessions` lives only in memory. After a restart in the
//! middle of a delegation it is empty, so a child's `pre_tool_call` (which
//! carries no platform) can read as main work and reopen the turn.

use crate::agent_api::hermes_event::HarnessHermesHook;

/// A dialog believed open.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenApproval {
    pub key: String,
    pub turn_id: Option<String>,
    pub tool_name: Option<String>,
    pub description: Option<String>,
}

/// The reducer's state (`HermesAdapterState`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HermesAdapterState {
    /// Session ids of delegated children, learned from `subagent_start`.
    pub child_sessions: Vec<String>,
    /// Dialogs believed open, oldest first.
    pub open_approvals: Vec<OpenApproval>,
    /// A main-session turn is believed in progress (start seen, no end yet).
    pub main_turn_open: bool,
}

/// What a hook asks the machine to do (`HermesEffect`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HermesEffect {
    SessionStarted {
        session_id: Option<String>,
        changed: bool,
    },
    Running,
    Waiting {
        interrupted: bool,
    },
    PermissionOpen {
        tool_name: Option<String>,
        description: Option<String>,
    },
    PermissionAnswered,
    PermissionAnsweredIdle,
    TurnEnded,
    SubagentStart {
        agent_id: String,
        description: Option<String>,
    },
    SubagentEnd {
        agent_id: String,
    },
}

#[must_use]
pub fn approval_key(ev: &HarnessHermesHook) -> String {
    match &ev.tool_call_id {
        Some(id) => format!("call:{id}"),
        None => format!(
            "turn:{}|{}",
            ev.turn_id.as_deref().unwrap_or(""),
            ev.description.as_deref().unwrap_or("")
        ),
    }
}

fn is_child(s: &HermesAdapterState, ev: &HarnessHermesHook) -> bool {
    ev.platform.as_deref() == Some("subagent")
        || ev
            .session_id
            .as_ref()
            .is_some_and(|id| s.child_sessions.contains(id))
}

/// Index of the open approval a response answers, if any.
#[must_use]
pub fn match_approval(open: &[OpenApproval], ev: &HarnessHermesHook) -> Option<usize> {
    if ev.tool_call_id.is_some() {
        let key = approval_key(ev);
        return open.iter().position(|a| a.key == key);
    }
    open.iter()
        .position(|a| a.turn_id == ev.turn_id && a.description == ev.description)
        .or_else(|| open.iter().position(|a| a.turn_id == ev.turn_id))
}

fn reduce_approval_request(
    s: &mut HermesAdapterState,
    ev: &HarnessHermesHook,
) -> Vec<HermesEffect> {
    if ev.surface.as_deref() == Some("smart") {
        return Vec::new();
    }
    s.open_approvals.push(OpenApproval {
        key: approval_key(ev),
        turn_id: ev.turn_id.clone(),
        tool_name: ev.tool_name.clone(),
        description: ev.description.clone(),
    });
    vec![HermesEffect::PermissionOpen {
        tool_name: ev.tool_name.clone(),
        description: ev.description.clone(),
    }]
}

fn reduce_approval_response(
    s: &mut HermesAdapterState,
    ev: &HarnessHermesHook,
) -> Vec<HermesEffect> {
    if ev.surface.as_deref() == Some("smart") {
        return Vec::new();
    }
    let Some(at) = match_approval(&s.open_approvals, ev) else {
        return Vec::new();
    };
    s.open_approvals.remove(at);
    vec![match s.open_approvals.last() {
        Some(last) => HermesEffect::PermissionOpen {
            tool_name: last.tool_name.clone(),
            description: last.description.clone(),
        },
        None if s.main_turn_open => HermesEffect::PermissionAnswered,
        None => HermesEffect::PermissionAnsweredIdle,
    }]
}

/// `reduceHermesHook`, mutating `s` in place. `current_session_id` is the
/// harness's session id as Skein currently has it.
pub fn reduce_hermes_hook(
    s: &mut HermesAdapterState,
    ev: &HarnessHermesHook,
    current_session_id: Option<&str>,
) -> Vec<HermesEffect> {
    match ev.event.as_str() {
        "pre_approval_request" => reduce_approval_request(s, ev),
        "post_approval_response" => reduce_approval_response(s, ev),
        "subagent_start" => {
            let Some(child) = &ev.child_session_id else {
                return Vec::new();
            };
            // The goal is not forwarded by the Rust hook route today.
            if !s.child_sessions.contains(child) {
                s.child_sessions.push(child.clone());
            }
            vec![HermesEffect::SubagentStart {
                agent_id: child.clone(),
                description: None,
            }]
        }
        // child_sessions keeps the id: the child's late hooks stay filtered.
        "subagent_stop" => ev
            .child_session_id
            .as_ref()
            .map(|id| HermesEffect::SubagentEnd {
                agent_id: id.clone(),
            })
            .into_iter()
            .collect(),
        // A child's own first hook can beat `subagent_start` here: remember it.
        "on_session_start" => {
            if is_child(s, ev) {
                if let Some(id) = &ev.session_id
                    && !s.child_sessions.contains(id)
                {
                    s.child_sessions.push(id.clone());
                }
                return Vec::new();
            }
            s.main_turn_open = true;
            vec![
                HermesEffect::SessionStarted {
                    session_id: ev.session_id.clone(),
                    changed: ev.session_id.is_some()
                        && ev.session_id.as_deref() != current_session_id,
                },
                HermesEffect::Running,
            ]
        }
        "pre_llm_call" | "pre_tool_call" => {
            if is_child(s, ev) {
                return Vec::new();
            }
            s.main_turn_open = true;
            vec![HermesEffect::Running]
        }
        "on_session_end" => {
            if is_child(s, ev) {
                return Vec::new();
            }
            s.open_approvals.clear();
            s.main_turn_open = false;
            vec![
                HermesEffect::Waiting {
                    interrupted: ev.interrupted == Some(true),
                },
                HermesEffect::TurnEnded,
            ]
        }
        // post_* and anything unknown change no phase.
        _ => Vec::new(),
    }
}
