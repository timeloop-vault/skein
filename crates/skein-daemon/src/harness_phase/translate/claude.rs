//! Claude half of the L2c translator (epic #50), phase side only. Port of
//! the phase/registry parts of `app/src/harnessEventsClaude.ts` plus the
//! Claude hook half of `app/src/useHarnessHookEvents.ts`.
//!
//! Turns each [`ClaudeEvent`] into calls on [`PhaseMachine`] and its
//! subagent / background-task registries. Pure: no I/O, no clock reads
//! (`now_ms` is injected), no globals. What stays in the webview is
//! deliberately absent: the Claude version store (`cli_version`), session
//! tracking, shell-claim gating, toasts, and the Live Context feed.
//!
//! The parent (main-session) PHASE is deliberately untouched by the
//! subagent and background arms: a subagent starting, finishing or getting
//! a tool result says nothing about whether the main session is running,
//! waiting or idle (#298). The `note_*` bookkeeping only feeds the
//! deferred-end-of-turn timers (#277).

// The doc comments quote TS identifiers verbatim.
#![allow(clippy::doc_markdown)]

use crate::harness_events_claude::ClaudeEvent;
use crate::harness_phase::SessionSwitch;
use crate::harness_phase::background_tasks::BackgroundTaskInput;
use crate::harness_phase::machine::PhaseMachine;
use crate::harness_phase::subagents::SubagentInput;
use crate::harness_phase::types::TransitionSource;

/// Per-harness state of one Claude attach: the TS closure's `closed` flag
/// and `isLive()` token, folded into one `live` bit.
#[derive(Debug, Default)]
pub struct ClaudeTranslator {
    /// True between [`Self::attached`] and [`Self::detached`] /
    /// [`Self::superseded`]. While false, [`Self::apply`] drops events:
    /// #116, a straggler from an OLD tail landing after a `/clear`
    /// re-point must not feed the NEW session's phase or subagent state.
    live: bool,
}

impl ClaudeTranslator {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether events are currently applied.
    #[must_use]
    pub fn is_live(&self) -> bool {
        self.live
    }

    /// The attach starts. Marks the harness authoritative *synchronously*,
    /// before the adapter's first event: by the time the attach runs the
    /// PTY has been streaming for a while, and without this those chunks
    /// call `record_output` -> running and override whatever the adapter's
    /// history probe is about to emit (found in v0.1.9-dev).
    pub fn attached(&mut self, m: &mut PhaseMachine, id: &str, _now_ms: i64) {
        self.live = true;
        m.attach_authoritative_source(id);
    }

    /// The attach ended (unsubscribe, or the attach itself was rejected so
    /// L2a takes over). Only strips authority while this translator is still
    /// the live one (#422): a stale attach must not strip a newer one's.
    pub fn detached(&mut self, m: &mut PhaseMachine, id: &str, now_ms: i64) {
        if self.live {
            self.live = false;
            m.detach_authoritative_source(id, now_ms);
        }
    }

    /// A newer attach of the same harness took over (TS: `isLive()` turned
    /// false). Stop applying events and touch the machine not at all.
    pub fn superseded(&mut self) {
        self.live = false;
    }

    /// One event from the tail. Dropped unless live (#116, #422).
    pub fn apply(&mut self, m: &mut PhaseMachine, id: &str, event: &ClaudeEvent, now_ms: i64) {
        if !self.live {
            return;
        }
        // #259: any event at all proves the tail is on the right file.
        // #422: a live adapter speaking after `session_end` (the transcript
        // reappeared) takes authority back.
        m.adapter_delivered(id, !matches!(event, ClaudeEvent::SessionEnd), now_ms);
        translate(m, id, event, now_ms);
    }
}

fn translate(m: &mut PhaseMachine, id: &str, event: &ClaudeEvent, now_ms: i64) {
    match event {
        // Adapter saw the start of a Claude turn. Persisted with
        // `l2c1-claude-assistant` so the activity feed can tell "started
        // thinking" apart from "tool result returned".
        ClaudeEvent::AssistantTurn => {
            m.set_running_from_adapter(id, TransitionSource::L2c1ClaudeAssistant, false, now_ms);
        }
        ClaudeEvent::ToolUseStart { .. } => {
            m.set_running_from_adapter(id, TransitionSource::L2c1ClaudeToolUse, false, now_ms);
        }
        // #86: a denial writes a tool_result with `is_error` set, same shape
        // as any other result. Either way the permission gate this tool call
        // was waiting on is gone, approved or not.
        ClaudeEvent::ToolUseResult => {
            m.set_running_from_adapter(id, TransitionSource::L2c1ClaudeToolResult, true, now_ms);
        }
        // #86: the user submitted a fresh prompt: any permission dialog is
        // gone by construction. `task_notification` (#445) is deliberately
        // not consulted: the phase is the same either way.
        ClaudeEvent::UserPrompt { .. } => {
            m.set_running_from_adapter(id, TransitionSource::L2c1ClaudeUserPrompt, true, now_ms);
            // #277: a fresh prompt starts a new delegation-counting window.
            m.note_user_prompt(id);
        }
        // An assistant row with a terminal stop_reason, or (#260) an
        // interrupt / turn_duration row. #277: routed through
        // `awaiting_prompt_from_adapter`, not `set_waiting_from_adapter`: the
        // main transcript's end of turn is a lie while delegated work runs.
        ClaudeEvent::AwaitingPrompt => {
            m.awaiting_prompt_from_adapter(id, TransitionSource::L2c1ClaudeEndTurn, now_ms);
        }
        // User-side action between turns: no phase shift, the harness is
        // still effectively waiting until the user submits.
        // `cli_version` is webview-only (claudeVersionStore).
        ClaudeEvent::Attachment | ClaudeEvent::CliVersion { .. } => {}
        // File vanished: fall back to L2a. #86: a dialog open now would
        // otherwise pin `permission` for good. #422: if the file reappears,
        // the next event restores authority (`adapter_delivered`).
        ClaudeEvent::SessionEnd => {
            m.release_permission(id, TransitionSource::AdapterDetached, now_ms);
            m.detach_authoritative_source(id, now_ms);
        }
        ClaudeEvent::SubagentStart {
            agent_id,
            agent_type,
            description,
            initial,
        } => {
            m.subagents_mut().record(
                id,
                SubagentInput {
                    agent_id: agent_id.clone(),
                    agent_type: agent_type.clone(),
                    description: description.clone(),
                },
                *initial,
                now_ms,
            );
            // #277: only a LIVE start counts toward `delegated_count` and as
            // proof of life for the ceiling; an attach-time replay belongs to
            // a PTY that is already dead and must not look like fresh activity.
            if !*initial {
                m.note_subagent_started(id, now_ms);
                m.reclaim_delegation_from_adapter(id, now_ms); // #634
            }
        }
        // #497: carries no work signal.
        ClaudeEvent::SubagentLabel {
            agent_id,
            agent_type,
            description,
        } => {
            m.subagents_mut()
                .relabel(id, agent_id, agent_type.clone(), description.clone());
        }
        ClaudeEvent::SubagentEnd { agent_id, .. } => {
            m.subagents_mut().finish(id, agent_id);
            m.note_subagent_activity(id, now_ms);
        }
        // The one narrow phase effect a subagent's own tool result may have:
        // prove its own permission gate is gone, not a different subagent's
        // (`agent_id` lets `clear_permission` tell them apart).
        ClaudeEvent::SubagentToolResult { agent_id } => {
            m.clear_permission(
                id,
                TransitionSource::L2c1ClaudeSubagentToolResult,
                Some(agent_id),
                now_ms,
            );
            // #277: proof of life either way, phase effect or not.
            m.note_subagent_activity(id, now_ms);
        }
        // #446: a background task holds an end of turn like a subagent. No
        // direct phase write; the deferral reads the registry.
        ClaudeEvent::BackgroundStart {
            task_id,
            task_kind,
            description,
            command,
            timeout_ms,
            persistent,
            agent_id,
            initial,
            ..
        } => {
            m.background_tasks_mut().record(
                id,
                BackgroundTaskInput {
                    task_id: task_id.clone(),
                    kind: *task_kind,
                    description: description.clone(),
                    command: command.clone(),
                    // A value past i64 is unrepresentable; treat as no timeout.
                    timeout_ms: timeout_ms.and_then(|t| i64::try_from(t).ok()),
                    persistent: *persistent,
                    agent_id: agent_id.clone(),
                },
                *initial,
                now_ms,
            );
            // A live subagent's row is proof of life; an attach replay is not.
            if !*initial && agent_id.is_some() {
                m.note_subagent_activity(id, now_ms);
            }
            if !*initial {
                m.reclaim_delegation_from_adapter(id, now_ms); // #634
            }
        }
        ClaudeEvent::BackgroundEnd {
            task_id, agent_id, ..
        } => {
            m.background_tasks_mut().finish(id, task_id);
            if agent_id.is_some() {
                m.note_subagent_activity(id, now_ms);
            }
        }
    }
}

/// #86: Claude's `PermissionRequest` command hook fired
/// (`skein://harness-permission`): a dialog is on screen. Room-agnostic;
/// the machine ignores an unknown id (a stale event racing a closed
/// harness). `agent_type` / `agent_id` name the subagent that opened it
/// (#298), `None` for the main session. The #318 shell-claim gate
/// (`shellClaim.acceptsPermission`) stays with the caller.
pub fn apply_hook_permission(
    m: &mut PhaseMachine,
    id: &str,
    tool_name: Option<&str>,
    agent_type: Option<&str>,
    agent_id: Option<&str>,
    now_ms: i64,
) {
    m.set_permission_from_adapter(
        id,
        TransitionSource::L2c1ClaudePermission,
        tool_name,
        agent_type,
        agent_id,
        now_ms,
    );
}

/// #273 / #116: Claude's `SessionStart` command hook
/// (`skein://harness-session-start`). Always notes the launch signal;
/// `switch` is `Some` when the caller (sessionTracking's `followedSession`,
/// or the #318 shell-claim decision) has decided the harness moved onto a
/// different conversation, in which case old subagent/delegation state is
/// forgotten. The `startup` source has no phase source of its own, so the
/// caller passes `Resume` for it, as the webview does.
pub fn apply_hook_session_start(
    m: &mut PhaseMachine,
    id: &str,
    switch: Option<SessionSwitch>,
    now_ms: i64,
) {
    m.note_launch_signal(id, TransitionSource::L2c1ClaudeSessionStart, now_ms);
    if let Some(s) = switch {
        m.session_switched(id, s, now_ms);
    }
}

/// #539: a verified adoption of a second startup id
/// (`sessionSwitched(id, "startup")` with no launch signal); callers pass
/// [`SessionSwitch::Startup`], which forgets and disarms but moves no phase.
pub fn apply_hook_session_switched(
    m: &mut PhaseMachine,
    id: &str,
    switch: SessionSwitch,
    now_ms: i64,
) {
    m.session_switched(id, switch, now_ms);
}

#[cfg(test)]
#[path = "tests/claude.rs"]
mod tests;

#[cfg(test)]
#[path = "tests/claude_authority.rs"]
mod authority_tests;
#[cfg(test)]
#[path = "tests/claude_local_command.rs"]
mod local_command_tests;
#[cfg(test)]
#[path = "tests/claude_reclaim.rs"]
mod reclaim_tests;
#[cfg(test)]
#[path = "tests/claude_subagent_exit.rs"]
mod subagent_exit_tests;
