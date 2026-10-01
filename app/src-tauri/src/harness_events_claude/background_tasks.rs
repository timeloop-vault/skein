//! Background-task state fed from every transcript row, and its reconciliation into events.

use super::ClaudeEvent;
use super::adapter::SubagentTail;
use crate::harness_actions_claude::ExtractedAction;
use skein_harness::claude::background;
use skein_harness::claude::background::{
    BackgroundKind, BackgroundTask, BackgroundTasks, BackgroundTransition, Outcome, OutcomeStatus,
    OutputTrailer,
};
use std::collections::{HashMap, HashSet};

/// `harness_actions.kind` of the row a background task's end writes
/// (#445). Local rather than in `db::action_kind`, which a sibling change
/// is editing; the feed reads the kind off the row either way.
pub(super) const BACKGROUND_END: &str = "background_end";

/// How long past a Monitor's `started + timeout` the adapter waits
/// before it ends the task itself as `Expired` (#445). Claude Code
/// normally writes a `[Monitor expired after …` notice at the deadline,
/// which says more than the sweep can (its summary), so the sweep must
/// lose that race; it exists for the Monitors that pass their deadline
/// with no notice at all (recon §Q2).
pub(super) const MONITOR_EXPIRY_GRACE_MS: i64 = 60_000;

/// The background-task tracker plus which subagent launched which task
/// (#445). A task absent from `owner` belongs to the main session.
#[derive(Default)]
pub(super) struct BackgroundState {
    pub(super) tasks: BackgroundTasks,
    /// task id → agent id of the subagent whose transcript started it.
    pub(super) owner: HashMap<String, String>,
    /// The adapter's promise to the frontend: every task id whose
    /// `BackgroundStart` was emitted (initial or live) and whose
    /// `BackgroundEnd` has not been, mapped to its kind so an end can be
    /// emitted even when the tracker no longer knows the task. Every
    /// announced start gets exactly one end, and an end is only ever
    /// emitted (event or `background_end` row) for an id in here: an end
    /// for a task nobody was told about just updates the tracker.
    pub(super) announced: HashMap<String, BackgroundKind>,
}

/// Where a live feed's results go: the tick's event list and the rows
/// `tick` persists afterwards. History feeds pass no sink at all.
pub(super) struct BackgroundOut<'a> {
    pub(super) harness_id: &'a str,
    pub(super) events: &'a mut Vec<ClaudeEvent>,
    pub(super) rows: &'a mut Vec<ExtractedAction>,
}

pub(super) fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

pub(super) fn background_start_event(
    task: &BackgroundTask,
    agent_id: Option<String>,
    initial: bool,
) -> ClaudeEvent {
    ClaudeEvent::BackgroundStart {
        task_id: task.task_id.clone(),
        tool_use_id: task.tool_use_id.clone(),
        task_kind: task.kind,
        description: task.description.clone(),
        command: task.command.clone(),
        timeout_ms: task.timeout_ms,
        persistent: task.persistent,
        auto_backgrounded: task.auto_backgrounded,
        agent_id,
        initial,
    }
}

impl BackgroundState {
    /// Feeds one transcript row. `agent_id` is the subagent whose file it
    /// came from, `None` for the main file. The tracker is
    /// order-independent (an early terminal is remembered), so the main
    /// file and the subagent files can be fed in any order. With a sink
    /// (live) every transition becomes an event, and an end also a
    /// `background_end` row; without one (history) they are dropped, but
    /// ownership is still recorded.
    pub(super) fn feed(
        &mut self,
        row: &serde_json::Value,
        agent_id: Option<&str>,
        mut out: Option<&mut BackgroundOut<'_>>,
    ) {
        let row_ts = skein_harness::claude::timestamp_ms(row);
        for transition in self.tasks.observe(row) {
            match transition {
                BackgroundTransition::Started(task) => {
                    if let Some(agent) = agent_id {
                        self.owner.insert(task.task_id.clone(), agent.to_owned());
                    }
                    if let Some(out) = out.as_deref_mut() {
                        self.announced.insert(task.task_id.clone(), task.kind);
                        tracing::info!(
                            harness_id = %out.harness_id,
                            task_id = %task.task_id,
                            task_kind = ?task.kind,
                            initial = false,
                            "claude_events: background task started"
                        );
                        out.events.push(background_start_event(
                            &task,
                            agent_id.map(str::to_owned),
                            false,
                        ));
                    }
                }
                BackgroundTransition::Ended { task, outcome } => {
                    if let Some(out) = out.as_deref_mut() {
                        self.announce_end(&task, &outcome, row_ts, true, out);
                    }
                }
                // Monitor output lines: no event, no row. S4 (#447) decides.
                BackgroundTransition::MonitorEvent { .. } => {}
            }
        }
    }

    /// The end of one task: the event, plus (for an end Skein actually
    /// saw) the feed row. `ts` is the ending row's timestamp, falling
    /// back to now. Silent unless the task's start was announced, which
    /// this then retires.
    pub(super) fn announce_end(
        &mut self,
        task: &BackgroundTask,
        outcome: &Outcome,
        ts: Option<i64>,
        with_row: bool,
        out: &mut BackgroundOut<'_>,
    ) {
        if self.announced.remove(&task.task_id).is_none() {
            return;
        }
        let agent_id = self.owner.get(&task.task_id).cloned();
        tracing::info!(
            harness_id = %out.harness_id,
            task_id = %task.task_id,
            task_kind = ?task.kind,
            status = ?outcome.status,
            "claude_events: background task ended"
        );
        out.events.push(ClaudeEvent::BackgroundEnd {
            task_id: task.task_id.clone(),
            task_kind: task.kind,
            agent_id: agent_id.clone(),
            status: outcome.status,
            exit_code: outcome.exit_code,
        });
        if !with_row {
            return;
        }
        let timestamp_ms = ts.unwrap_or_else(now_ms);
        let mut payload = serde_json::json!({
            "task_id": task.task_id,
            "task_kind": task.kind,
            "description": task.description,
            "command": task.command,
            "status": outcome.status,
            "exit_code": outcome.exit_code,
            "summary": outcome.summary,
            "agent_id": agent_id,
        });
        if let Some(start) = task.started_ms
            && let Some(obj) = payload.as_object_mut()
        {
            obj.insert(
                "duration_ms".into(),
                serde_json::json!(timestamp_ms.saturating_sub(start)),
            );
        }
        out.rows.push(ExtractedAction {
            kind: BACKGROUND_END,
            timestamp_ms,
            payload: payload.to_string(),
            source: None,
        });
    }

    /// Ends every still-outstanding task `agent_id` launched, as
    /// `SubagentEnded`: event only, no row (a subagent's exit doesn't say
    /// its tasks finished). Called when that subagent's transcript ends.
    pub(super) fn end_owned_by(&mut self, agent_id: &str, events: &mut Vec<ClaudeEvent>) {
        let owned: Vec<String> = self
            .owner
            .iter()
            .filter(|(_, owner)| owner.as_str() == agent_id)
            .map(|(task_id, _)| task_id.clone())
            .collect();
        for task_id in owned {
            if let Some(BackgroundTransition::Ended { task, outcome }) = self
                .tasks
                .end(&task_id, Outcome::new(OutcomeStatus::SubagentEnded))
                && self.announced.remove(&task_id).is_some()
            {
                tracing::info!(
                    task_id = %task.task_id,
                    task_kind = ?task.kind,
                    status = ?outcome.status,
                    "claude_events: background task ended"
                );
                events.push(ClaudeEvent::BackgroundEnd {
                    task_id: task.task_id,
                    task_kind: task.kind,
                    agent_id: Some(agent_id.to_owned()),
                    status: outcome.status,
                    exit_code: None,
                });
            }
        }
    }

    /// Ends Monitors past their deadline plus [`MONITOR_EXPIRY_GRACE_MS`]
    /// as `Expired`, event and row. `now_ms` is the caller's clock so a
    /// test can drive it.
    pub(super) fn sweep(&mut self, now_ms: i64, out: &mut BackgroundOut<'_>) {
        for transition in self.tasks.expire_overdue(now_ms, MONITOR_EXPIRY_GRACE_MS) {
            if let BackgroundTransition::Ended { task, outcome } = transition {
                self.announce_end(&task, &outcome, Some(now_ms), true, out);
            }
        }
    }
}

/// Re-derives the live background set from a freshly fed tracker and
/// reconciles it with what was announced (#445). At attach, once main and
/// subagent history are both fed, nothing is announced yet; after a
/// resync the old tracker's `announced` set was carried over.
///
/// First, everything the transcripts can't vouch for ends silently: a
/// Monitor past its deadline (so the sweep doesn't later end what no one
/// was told about), a task whose `.output` file already ends in an
/// `[exited …]` / `[killed]` trailer, and a task whose launching subagent
/// is finished. A missing owner counts as finished: its file was skipped
/// or unreadable, and ending silently is the safe direction. Then:
/// every announced id that is no longer outstanding gets one
/// `BackgroundEnd` (`unknown`, or `subagent_ended` for an owned task), no
/// row, since Skein can't know how it ended; and every outstanding id not
/// yet announced gets `BackgroundStart { initial: true }` (liveness
/// unknown, like the attach batch). Ends come first.
///
/// Accepted loss: on a resync or subagent re-seed, a live-announced
/// main-session task that the trailer or the deadline shows as done is
/// closed here with no row, and its later real notification is then
/// dropped as unannounced, so that task's `background_end` row is lost.
/// It happens only on a rare shrink or truncation, and it errs toward
/// closing rather than leaking.
pub(super) fn reconcile_background(
    background: &mut BackgroundState,
    subagents: &HashMap<String, SubagentTail>,
    now_ms: i64,
) -> Vec<ClaudeEvent> {
    // Discarded: history is not replayed, and these are not announced.
    background.tasks.expire_overdue(now_ms, 0);
    let outstanding: Vec<BackgroundTask> = background
        .tasks
        .outstanding(now_ms)
        .into_iter()
        .cloned()
        .collect();
    let mut live = HashSet::new();
    let mut starts = Vec::new();
    for task in outstanding {
        let trailer = background
            .tasks
            .output_path(&task.task_id)
            .map_or(OutputTrailer::Unreadable, |p| {
                background::read_output_trailer(&p)
            });
        if matches!(trailer, OutputTrailer::Exited(_) | OutputTrailer::Killed) {
            background
                .tasks
                .end(&task.task_id, Outcome::new(OutcomeStatus::Completed));
            continue;
        }
        let owner = background.owner.get(&task.task_id).cloned();
        if let Some(agent) = &owner
            && subagents
                .get(agent)
                .is_none_or(|t| t.lifecycle.is_finished())
        {
            background
                .tasks
                .end(&task.task_id, Outcome::new(OutcomeStatus::SubagentEnded));
            continue;
        }
        live.insert(task.task_id.clone());
        if !background.announced.contains_key(&task.task_id) {
            background.announced.insert(task.task_id.clone(), task.kind);
            starts.push(background_start_event(&task, owner, true));
        }
    }
    let gone: Vec<(String, BackgroundKind)> = background
        .announced
        .iter()
        .filter(|(id, _)| !live.contains(*id))
        .map(|(id, kind)| (id.clone(), *kind))
        .collect();
    let mut events = Vec::new();
    for (task_id, task_kind) in gone {
        background.announced.remove(&task_id);
        let agent_id = background.owner.get(&task_id).cloned();
        events.push(ClaudeEvent::BackgroundEnd {
            task_id,
            task_kind,
            status: if agent_id.is_some() {
                OutcomeStatus::SubagentEnded
            } else {
                OutcomeStatus::Unknown
            },
            agent_id,
            exit_code: None,
        });
    }
    events.extend(starts);
    events
}
