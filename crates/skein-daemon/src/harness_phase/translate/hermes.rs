//! #625/#628: the binding between Hermes shell hooks and the phase machine
//! (L2c-3). Port of the machine-driving parts of
//! `app/src/harnessEventsHermes.ts` and `app/src/hermesWork.ts`. The
//! decisions live in [`super::hermes_hook`]; this applies its effects, and
//! maps the disk stream's [`HermesWorkEvent`]s onto the subagent and
//! background-task registries.
//!
//! One [`HermesTranslator`] per harness. Pure: time is injected, nothing is
//! read from the screen or the clock. The 500 ms launch poll's screen read
//! stays with the client; it feeds the result in through
//! [`HermesTranslator::poll_launch`] (or [`HermesTranslator::launch_screen_ready`]).
//!
//! Outstanding delegated work (#628) comes from two sources feeding the same
//! registries the Claude adapter does, so the existing end-of-turn deferral
//! (#277/#446) holds a Hermes turn at "delegating" instead of "waiting":
//! the `subagent_start` / `subagent_stop` hooks (live), and the disk stream
//! (an attach snapshot with `initial: true`, then live diffs), covering
//! background `delegate_task` children AND background terminal processes,
//! which have no hook at all. Both may report the same delegation; `record`
//! and `finish` are idempotent (finish of an unknown id is a no-op), and the
//! live-start notes run only the first time an id is seen live, so
//! double-reporting is harmless.

use std::collections::HashMap;

use skein_harness::claude::background::BackgroundKind;

use super::super::PhaseMachine;
use super::super::background_tasks::BackgroundTaskInput;
use super::super::subagents::SubagentInput;
use super::super::types::{ActivityPhase, TransitionSource};
use super::hermes_hook::{HermesAdapterState, HermesEffect, reduce_hermes_hook};
use crate::agent_api::hermes_event::HarnessHermesHook;
use crate::harness_work_hermes::HermesWorkEvent;

/// The registry has no tombstone: a hook's start+stop followed by the disk
/// poller's first sighting of the same child (its row still unended for a
/// moment) would re-add a finished child. A live start this soon after its
/// end is that echo, so it is ignored.
pub const RECENTLY_ENDED_MS: i64 = 3_000;

/// Consecutive `ready` screen readings that stand in for a launch signal.
const LAUNCH_READY_READS: u32 = 2;

/// What the client still has to do after a hook (the effects with no
/// machine call).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HermesHookOutcome {
    /// `on_session_start` for the main session: the hook's session id and
    /// whether it differs from the harness's current one. The client claims
    /// the id (`markClaimed` / `releaseHermesCapture`), persists it when
    /// `changed`, and (re)attaches the actions/work streams for it.
    pub session_started: Option<(String, bool)>,
    /// `on_session_end` of the main session: call `hermes_actions_turn_end`.
    pub turn_ended: bool,
}

#[derive(Debug, Default)]
pub struct HermesTranslator {
    state: HermesAdapterState,
    /// The harness's session id as Skein has it (`getSessionId`).
    current_session_id: Option<String>,
    /// Session whose work stream is attached (`attachedFor`).
    attached_for: Option<String>,
    /// Ends seen, for [`RECENTLY_ENDED_MS`] (TS `endedAt`, per harness).
    ended_at: HashMap<String, i64>,
    /// Consecutive `ready` launch-poll readings.
    ready_reads: u32,
    disposed: bool,
}

impl HermesTranslator {
    /// `bound_session_id`: the session bound at spawn (a resume), if any.
    #[must_use]
    pub fn new(bound_session_id: Option<String>) -> Self {
        Self {
            current_session_id: bound_session_id.clone(),
            attached_for: bound_session_id,
            ..Self::default()
        }
    }

    /// The harness's live session id changed outside a hook (hermesCapture).
    pub fn set_current_session_id(&mut self, id: Option<String>) {
        self.current_session_id = id;
    }

    /// The session whose actions/work streams are attached.
    #[must_use]
    pub fn attached_for(&self) -> Option<&str> {
        self.attached_for.as_deref()
    }

    /// `createHermesBinding`'s start: this source is authoritative. Resets
    /// the reducer state, as a fresh binding does.
    pub fn attached(&mut self, m: &mut PhaseMachine, id: &str, _now_ms: i64) {
        self.state = HermesAdapterState::default();
        self.ended_at.clear();
        self.ready_reads = 0;
        self.disposed = false;
        m.attach_authoritative_source(id);
    }

    /// `binding.dispose`: forget delegated work, release a dialog nothing
    /// can answer any more, drop authority. Idempotent.
    pub fn detached(&mut self, m: &mut PhaseMachine, id: &str, now_ms: i64) {
        if self.disposed {
            return;
        }
        self.disposed = true;
        self.clear_work(m, id);
        m.release_permission(id, TransitionSource::AdapterDetached, now_ms);
        m.detach_authoritative_source(id, now_ms);
    }

    /// `binding.handle`: one Hermes hook.
    pub fn apply_hook(
        &mut self,
        m: &mut PhaseMachine,
        id: &str,
        hook: &HarnessHermesHook,
        now_ms: i64,
    ) -> HermesHookOutcome {
        let mut out = HermesHookOutcome::default();
        if self.disposed {
            return out;
        }
        m.adapter_delivered(id, true, now_ms);
        let effects = reduce_hermes_hook(&mut self.state, hook, self.current_session_id.as_deref());
        for e in effects {
            self.apply_effect(m, id, e, &mut out, now_ms);
        }
        out
    }

    fn apply_effect(
        &mut self,
        m: &mut PhaseMachine,
        id: &str,
        e: HermesEffect,
        out: &mut HermesHookOutcome,
        now_ms: i64,
    ) {
        match e {
            HermesEffect::SessionStarted {
                session_id,
                changed,
            } => {
                m.note_launch_signal(id, TransitionSource::L2c3HermesSessionStart, now_ms);
                if let Some(sid) = session_id {
                    // The hook's id is authoritative: the client claims it
                    // and stops the capture poll so no other pending
                    // harness (or a late tick) can take or overwrite it.
                    if changed {
                        self.current_session_id = Some(sid.clone());
                    }
                    self.note_attached_session(m, id, &sid);
                    out.session_started = Some((sid, changed));
                }
            }
            HermesEffect::Running => {
                m.set_running_from_adapter(id, TransitionSource::L2c3HermesRunning, false, now_ms);
            }
            HermesEffect::Waiting { interrupted } => {
                if interrupted {
                    m.set_waiting_from_adapter(id, TransitionSource::L2c3HermesInterrupted, now_ms);
                } else {
                    // #628: defers while delegated work is outstanding.
                    m.awaiting_prompt_from_adapter(id, TransitionSource::L2c3HermesEndTurn, now_ms);
                }
            }
            HermesEffect::SubagentStart {
                agent_id,
                description,
            } => self.start_subagent(m, id, &agent_id, description, false, now_ms),
            HermesEffect::SubagentEnd { agent_id } => self.end_subagent(m, id, &agent_id, now_ms),
            HermesEffect::PermissionOpen { tool_name, .. } => m.set_permission_from_adapter(
                id,
                TransitionSource::L2c3HermesPermission,
                tool_name.as_deref(),
                None,
                None,
                now_ms,
            ),
            HermesEffect::PermissionAnswered => {
                m.clear_permission(
                    id,
                    TransitionSource::L2c3HermesPermissionAnswered,
                    None,
                    now_ms,
                );
            }
            HermesEffect::PermissionAnsweredIdle => {
                // Main turn already over: nothing more will arrive, so
                // settle to waiting (from `permission`, hence not notifiable).
                m.set_waiting_from_adapter(
                    id,
                    TransitionSource::L2c3HermesPermissionAnsweredIdle,
                    now_ms,
                );
            }
            HermesEffect::TurnEnded => out.turn_ended = true,
        }
    }

    /// `attachActions`' machine-relevant half: a new session id drops the
    /// old session's delegations, which can no longer report their ends.
    fn note_attached_session(&mut self, m: &mut PhaseMachine, id: &str, session_id: &str) {
        if self.attached_for.as_deref() == Some(session_id) {
            return;
        }
        if self.attached_for.is_some() {
            self.clear_work(m, id);
        }
        self.attached_for = Some(session_id.to_owned());
    }

    /// `applyHermesWork`: one event of the disk stream. The client must only
    /// forward events of the currently attached session (TS drops the rest).
    pub fn apply_work(
        &mut self,
        m: &mut PhaseMachine,
        id: &str,
        event: &HermesWorkEvent,
        now_ms: i64,
    ) {
        if self.disposed {
            return;
        }
        match event {
            HermesWorkEvent::SubagentStart {
                agent_id,
                description,
                initial,
            } => self.start_subagent(m, id, agent_id, description.clone(), *initial, now_ms),
            HermesWorkEvent::SubagentEnd { agent_id } => {
                self.end_subagent(m, id, agent_id, now_ms);
            }
            HermesWorkEvent::BackgroundStart {
                task_id,
                command,
                initial,
            } => {
                // "bash": no deadline (not a Monitor), so it gets the plain
                // BACKGROUND_TASK_CEILING_MS rule. Hermes reports no timeout.
                m.background_tasks_mut().record(
                    id,
                    BackgroundTaskInput {
                        task_id: task_id.clone(),
                        kind: BackgroundKind::Bash,
                        description: None,
                        command: command.clone(),
                        timeout_ms: None,
                        persistent: false,
                        agent_id: None,
                    },
                    *initial,
                    now_ms,
                );
                if !initial {
                    m.reclaim_delegation_from_adapter(id, now_ms);
                }
            }
            HermesWorkEvent::BackgroundEnd { task_id } => {
                m.background_tasks_mut().finish(id, task_id);
            }
            HermesWorkEvent::DelegationsIdle => {
                for s in m.subagents().live(id) {
                    self.end_subagent(m, id, &s.agent_id, now_ms);
                }
            }
        }
    }

    /// A subagent began (or was found running). Only a LIVE start the
    /// registry has not already counted as live feeds `delegated_count` and
    /// proof-of-life, like the Claude `subagent_start` arm.
    fn start_subagent(
        &mut self,
        m: &mut PhaseMachine,
        id: &str,
        agent_id: &str,
        description: Option<String>,
        initial: bool,
        now_ms: i64,
    ) {
        if !initial && self.recently_ended(agent_id, now_ms) {
            return;
        }
        let already_live = m
            .subagents()
            .live(id)
            .iter()
            .any(|s| s.agent_id == agent_id && !s.from_attach);
        m.subagents_mut().record(
            id,
            SubagentInput {
                agent_id: agent_id.to_owned(),
                agent_type: None,
                description,
            },
            initial,
            now_ms,
        );
        if initial || already_live {
            return;
        }
        m.note_subagent_started(id, now_ms);
        m.reclaim_delegation_from_adapter(id, now_ms);
    }

    fn recently_ended(&mut self, agent_id: &str, now_ms: i64) -> bool {
        self.ended_at.retain(|_, t| now_ms - *t < RECENTLY_ENDED_MS);
        self.ended_at.contains_key(agent_id)
    }

    fn end_subagent(&mut self, m: &mut PhaseMachine, id: &str, agent_id: &str, now_ms: i64) {
        self.ended_at.insert(agent_id.to_owned(), now_ms);
        m.subagents_mut().finish(id, agent_id);
        m.note_subagent_activity(id, now_ms);
    }

    /// `clearHermesWork`: drop every delegated-work entry of this harness
    /// (session change or teardown). The deferral needs no nudge: the next
    /// tick sees the empty working set and runs its ordinary settle window.
    fn clear_work(&mut self, m: &mut PhaseMachine, id: &str) {
        self.ended_at.clear();
        m.subagents_mut().forget(id);
        m.background_tasks_mut().forget(id);
    }

    /// Hermes fires no hook at launch (the first is at the first prompt), so
    /// the 15 s launch-silent watchdog would degrade every idle-at-launch
    /// Hermes. While `spawning`, the rendered `ready` status line (two reads
    /// in a row, like the screen-phase tick) stands in as the launch signal.
    /// The client reads the screen every 500 ms and calls this with whether
    /// it read `waiting`; returns whether the poll should keep running.
    pub fn poll_launch(
        &mut self,
        m: &mut PhaseMachine,
        id: &str,
        screen_waiting: bool,
        now_ms: i64,
    ) -> bool {
        if self.disposed || m.get(id).map(|a| a.phase) != Some(ActivityPhase::Spawning) {
            return false;
        }
        self.ready_reads = if screen_waiting {
            self.ready_reads + 1
        } else {
            0
        };
        if self.ready_reads >= LAUNCH_READY_READS {
            self.launch_screen_ready(m, id, now_ms);
            return false;
        }
        true
    }

    /// The launch stand-in fired: `noteLaunchSignal` with `l2c3-hermes-ready`.
    #[allow(clippy::unused_self)]
    pub fn launch_screen_ready(&mut self, m: &mut PhaseMachine, id: &str, now_ms: i64) {
        m.note_launch_signal(id, TransitionSource::L2c3HermesReady, now_ms);
    }
}
