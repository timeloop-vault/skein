//! The authority's output side (#732): what [`PhaseService`] publishes.
//!
//! Under the service lock, [`Inner::publish`] turns what the machine queued
//! into [`PhaseUpdate`]s and pushes them onto an unbounded channel. One
//! thread (`skein-phase-out`) drains it in order, writes the
//! `harness_events` rows and then emits [`HARNESS_PHASE_EVENT`], so neither
//! the database nor the sink is ever touched under the mutex and rows and
//! events keep lock order.
//!
//! [`PhaseService`]: super::PhaseService

use std::collections::{BTreeMap, BTreeSet};
use std::panic::AssertUnwindSafe;
use std::sync::Arc;
use std::sync::mpsc::Receiver;
use std::thread::JoinHandle;

use serde::Serialize;

use super::background_tasks::BackgroundTaskEntry;
use super::service::Inner;
use super::subagents::SubagentEntry;
use super::types::{ActivityPhase, HarnessActivity, Transition, TurnSignal};
use crate::db::Database;
use crate::event_sink::{self, SharedEventSink};

/// The broadcast event carrying one [`PhaseUpdate`].
pub const HARNESS_PHASE_EVENT: &str = "skein://harness-phase";

/// The part of a harness's phase record that goes to clients (#732).
///
/// The machine's own record, [`HarnessActivity`], stays internal: exposing
/// it would freeze its fields into the protocol. This is the wire type, and
/// it holds only what the webview reads. Adding a field here is a deliberate
/// act (a test pins the key set).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
#[allow(clippy::struct_excessive_bools)]
pub struct PhaseView {
    // Display and notifications.
    pub phase: ActivityPhase,
    pub phase_since: i64,
    pub spawned_at: i64,
    pub has_user_input: bool,
    pub permission_tool: Option<String>,
    pub permission_agent_type: Option<String>,
    pub delegated_count: u32,
    pub last_turn_signal: Option<TurnSignal>,
    pub last_submit_at: Option<i64>,
    // The send gate: `canSendPrompt` and mail delivery still decide in the
    // webview. This whole group leaves the wire with #733, which moves those
    // decisions into the daemon.
    pub authoritative: bool,
    pub adapter_heard: bool,
    pub adapter_silent: bool,
    pub launch_signal_at: Option<i64>,
    pub last_output_at: Option<i64>,
    pub question_open: bool,
    pub delegation_deferred_at: Option<i64>,
    pub injected: bool,
}

impl From<&HarnessActivity> for PhaseView {
    fn from(a: &HarnessActivity) -> Self {
        Self {
            phase: a.phase,
            phase_since: a.phase_since,
            spawned_at: a.spawned_at,
            has_user_input: a.has_user_input,
            permission_tool: a.permission_tool.clone(),
            permission_agent_type: a.permission_agent_type.clone(),
            delegated_count: a.delegated_count,
            last_turn_signal: a.last_turn_signal,
            last_submit_at: a.last_submit_at,
            authoritative: a.authoritative,
            adapter_heard: a.adapter_heard,
            adapter_silent: a.adapter_silent,
            launch_signal_at: a.launch_signal_at,
            last_output_at: a.last_output_at,
            question_open: a.question_open,
            delegation_deferred_at: a.delegation_deferred_at,
            injected: a.injected,
        }
    }
}

/// One harness's phase state at one instant, as sent to clients.
///
/// `seq` is one process-wide counter, so a client that listens first and then
/// reads the snapshot can drop every stream update with `seq <=` the
/// snapshot's.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PhaseUpdate {
    pub seq: u64,
    pub harness_id: String,
    pub room_id: String,
    /// `None` (wire `null`): the harness was forgotten.
    pub view: Option<PhaseView>,
    pub subagents: Vec<SubagentEntry>,
    pub background_tasks: Vec<BackgroundTaskEntry>,
    /// The phase changes this update reports, oldest first. Empty in a
    /// snapshot.
    pub transitions: Vec<Transition>,
    /// The #277 deferral armed in this update (#381).
    pub delegation_armed: bool,
}

/// What the service lock hands to the output thread.
pub(super) enum OutMsg {
    Update(Box<PhaseUpdate>),
    /// A `harness_events` row that is no machine transition (a restart
    /// request): written in order with the transition rows, never streamed.
    Row(EventRow),
    /// Test rendezvous: answered once everything queued before it is done.
    #[cfg(test)]
    Flush(std::sync::mpsc::Sender<()>),
}

/// One `harness_events` row that bypasses the machine.
pub(super) struct EventRow {
    pub(super) harness_id: String,
    pub(super) room_id: String,
    pub(super) from: String,
    pub(super) to: &'static str,
    pub(super) at_ms: i64,
    pub(super) has_user_input: bool,
    pub(super) source: &'static str,
}

/// The start of the throttle's comparison: everything but `lastOutputAt`.
fn only_output_differs(a: &PhaseUpdate, b: &PhaseUpdate) -> bool {
    if a.room_id != b.room_id
        || a.subagents != b.subagents
        || a.background_tasks != b.background_tasks
    {
        return false;
    }
    match (&a.view, &b.view) {
        (None, None) => true,
        (Some(x), Some(y)) => {
            let mut x = x.clone();
            x.last_output_at = y.last_output_at;
            x == *y
        }
        _ => false,
    }
}

fn identical(a: &PhaseUpdate, b: &PhaseUpdate) -> bool {
    only_output_differs(a, b)
        && a.view.as_ref().map(|x| x.last_output_at) == b.view.as_ref().map(|x| x.last_output_at)
}

impl Inner {
    /// The current state of `id` as an update (no transitions, not armed).
    pub(super) fn body(&self, id: &str, room_id: &str, seq: u64) -> PhaseUpdate {
        let view = self.machine.get(id).map(PhaseView::from);
        PhaseUpdate {
            seq,
            harness_id: id.to_owned(),
            room_id: room_id.to_owned(),
            view,
            subagents: self.machine.subagents().live(id),
            background_tasks: self.machine.background_tasks().live(id),
            transitions: Vec::new(),
            delegation_armed: false,
        }
    }

    /// One update per known harness at the current seq.
    pub(super) fn snapshot(&self) -> Vec<PhaseUpdate> {
        let mut ids: Vec<&String> = self.meta.keys().collect();
        ids.sort();
        ids.into_iter()
            .map(|id| self.body(id, &self.meta[id].room_id, self.seq))
            .collect()
    }

    /// Runs under the lock after every call. `sweep` (the tick) also looks at
    /// harnesses nothing marked, which is where throttled output times and
    /// registry changes get their update.
    pub(super) fn publish(
        &mut self,
        mut transitions: BTreeMap<String, Vec<Transition>>,
        changed: Vec<String>,
        armed: Vec<String>,
    ) {
        let sweep = std::mem::take(&mut self.sweep);
        let armed: BTreeSet<String> = armed.into_iter().collect();
        let mut ids: BTreeSet<String> = changed.into_iter().collect();
        ids.extend(transitions.keys().cloned());
        ids.extend(armed.iter().cloned());
        if sweep {
            ids.extend(self.meta.keys().cloned());
        }
        for id in ids {
            let Some(room_id) = self.meta.get(&id).map(|m| m.room_id.clone()) else {
                continue;
            };
            let mut update = self.body(&id, &room_id, 0);
            update.transitions = transitions.remove(&id).unwrap_or_default();
            update.delegation_armed = armed.contains(&id);
            let marked = !update.transitions.is_empty() || update.delegation_armed;
            if !marked
                && let Some(last) = self.last_emitted.get(&id)
                && (identical(last, &update) || (!sweep && only_output_differs(last, &update)))
            {
                continue;
            }
            self.seq += 1;
            update.seq = self.seq;
            let mut kept = update.clone();
            kept.transitions.clear();
            kept.delegation_armed = false;
            self.last_emitted.insert(id, kept);
            self.send(OutMsg::Update(Box::new(update)));
        }
        for (id, room_id) in std::mem::take(&mut self.pending_forgets) {
            self.last_emitted.remove(&id);
            self.seq += 1;
            let seq = self.seq;
            self.send(OutMsg::Update(Box::new(PhaseUpdate {
                seq,
                harness_id: id,
                room_id,
                view: None,
                subagents: Vec::new(),
                background_tasks: Vec::new(),
                transitions: Vec::new(),
                delegation_armed: false,
            })));
        }
    }
}

/// The `skein-phase-out` thread. Holds no reference to the service; it ends
/// when the sender (inside the service) is dropped. `None` if the OS refused
/// the thread (logged; updates are then lost).
pub(super) fn spawn_output_thread(
    rx: Receiver<OutMsg>,
    db: Arc<Database>,
    events: SharedEventSink,
) -> Option<JoinHandle<()>> {
    std::thread::Builder::new()
        .name("skein-phase-out".to_owned())
        .spawn(move || {
            for msg in rx {
                // A panic in the sink or the database layer must not end the
                // thread: every later message would go nowhere.
                let (harness, what) = describe(&msg);
                let step = std::panic::catch_unwind(AssertUnwindSafe(|| handle(&db, &events, msg)));
                if step.is_err() {
                    tracing::error!(
                        target: "skein::phase",
                        harness = %harness,
                        "phase output panicked on {what}; continuing"
                    );
                }
            }
        })
        .map_err(|e| tracing::error!("phase output: cannot start thread: {e}"))
        .ok()
}

/// The harness a message concerns and what kind of message it is, for the log
/// line of a panic.
fn describe(msg: &OutMsg) -> (String, &'static str) {
    match msg {
        OutMsg::Update(u) => (u.harness_id.clone(), "an update"),
        OutMsg::Row(r) => (r.harness_id.clone(), "a restart row"),
        #[cfg(test)]
        OutMsg::Flush(_) => (String::new(), "a flush"),
    }
}

fn handle(db: &Database, events: &SharedEventSink, msg: OutMsg) {
    match msg {
        OutMsg::Update(update) => deliver(db, events, &update),
        OutMsg::Row(r) => {
            if let Err(e) = db.record_harness_event(
                &r.harness_id,
                &r.room_id,
                &r.from,
                r.to,
                r.at_ms,
                r.has_user_input,
                Some(r.source),
            ) {
                tracing::warn!(target: "skein::phase", harness = %r.harness_id, "harness_events write failed: {e}");
            }
        }
        #[cfg(test)]
        OutMsg::Flush(done) => {
            let _ = done.send(());
        }
    }
}

fn deliver(db: &Database, events: &SharedEventSink, update: &PhaseUpdate) {
    for t in &update.transitions {
        if let Err(e) = db.record_harness_event(
            &t.harness_id,
            &update.room_id,
            t.from.as_str(),
            t.to.as_str(),
            t.at_ms,
            t.has_user_input,
            Some(t.source.as_str()),
        ) {
            tracing::warn!(target: "skein::phase", harness = %t.harness_id, "harness_events write failed: {e}");
        }
    }
    if let Err(e) = event_sink::emit(events.as_ref(), HARNESS_PHASE_EVENT, update) {
        tracing::warn!(target: "skein::phase", harness = %update.harness_id, "phase event emit failed: {e}");
    }
}
