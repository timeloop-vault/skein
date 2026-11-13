//! The driver around the pure [`PhaseMachine`] (#732): [`PhaseService`].
//!
//! The machine takes the time as an argument and queues its transitions in an
//! outbox; this is the part that owns the clock, the per-harness translators,
//! and the tick thread. It is shared by reference
//! (`Arc<PhaseService>`), every method takes `&self`, and all state sits
//! behind one `std::sync::Mutex` that is never held across a callback or any
//! I/O: the clock is read and the facts are gathered before the lock is
//! taken, and nothing is logged to a database from inside it.
//!
//! Authority: the daemon owns the phase. [`Inner::after`] turns what the
//! machine queued into [`PhaseUpdate`]s (see `stream.rs`) and pushes them onto
//! a channel under the lock; the `skein-phase-out` thread writes the
//! `harness_events` rows and emits the phase stream from there, so no
//! database write or emit happens under the mutex.

use std::collections::{BTreeMap, HashMap};
use std::panic::AssertUnwindSafe;
use std::sync::mpsc::{Sender, channel};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};
use std::thread::JoinHandle;
use std::time::Duration;

use super::client_input::{ClientPhaseInput, RestartSource};
use super::constants::TICK_INTERVAL_MS;
use super::machine::PhaseMachine;
use super::stream::{EventRow, OutMsg, PhaseUpdate, spawn_output_thread};
use super::translate::claude::{ClaudeTranslator, apply_hook_permission};
use super::translate::hermes::{HermesHookOutcome, HermesTranslator};
use super::translate::opencode::OpencodeTranslator;
use super::types::{ActivityPhase, HarnessActivity, Transition, TransitionSource};
use super::{SupervisorFacts, WorkCounts};
use crate::agent_api::hermes_event::HarnessHermesHook;
use crate::db::Database;
use crate::event_sink::SharedEventSink;
use crate::harness_events_claude::ClaudeEvent;
use crate::harness_events_opencode::OpencodeEvent;
use crate::harness_work_hermes::HermesWorkEvent;

/// Epoch ms. Injected so tests drive time.
pub type Clock = Arc<dyn Fn() -> i64 + Send + Sync>;

/// What the service knows about a harness beyond the machine's record.
#[derive(Debug, Clone)]
pub(super) struct HarnessMeta {
    pub(super) room_id: String,
}

pub(super) struct Inner {
    pub(super) machine: PhaseMachine,
    pub(super) claude: HashMap<String, ClaudeTranslator>,
    pub(super) opencode: HashMap<String, OpencodeTranslator>,
    pub(super) hermes: HashMap<String, HermesTranslator>,
    pub(super) meta: HashMap<String, HarnessMeta>,
    /// To the `skein-phase-out` thread; sent to only under the lock, so the
    /// order of rows and events is the lock order.
    pub(super) out: Sender<OutMsg>,
    /// The process-wide stream counter.
    pub(super) seq: u64,
    /// The last update emitted per harness (without transitions), for the
    /// `lastOutputAt` throttle. Dropped on forget.
    pub(super) last_emitted: HashMap<String, PhaseUpdate>,
    /// `(id, room id)` of harnesses forgotten since the last `after`.
    pub(super) pending_forgets: Vec<(String, String)>,
    /// Set by the tick: the next `after` also considers unmarked harnesses.
    pub(super) sweep: bool,
    /// A send to the output thread has failed (and was logged, once).
    pub(super) send_failed: bool,
}

impl Inner {
    fn new(out: Sender<OutMsg>) -> Self {
        Self {
            machine: PhaseMachine::default(),
            claude: HashMap::new(),
            opencode: HashMap::new(),
            hermes: HashMap::new(),
            meta: HashMap::new(),
            out,
            seq: 0,
            last_emitted: HashMap::new(),
            pending_forgets: Vec::new(),
            sweep: false,
            send_failed: false,
        }
    }

    /// Queue a message for the output thread. The receiver is gone only if
    /// that thread died or never started; say so once, then keep going (the
    /// machine still runs).
    pub(super) fn send(&mut self, msg: OutMsg) {
        if self.out.send(msg).is_err() && !self.send_failed {
            self.send_failed = true;
            tracing::error!(
                target: "skein::phase",
                "phase output thread is gone; phase rows and events are being lost"
            );
        }
    }

    /// Runs after every mutating call and every tick, still under the lock:
    /// queues the phase updates for the output thread (see `stream.rs`).
    fn after(&mut self) {
        let mut transitions: BTreeMap<String, Vec<Transition>> = BTreeMap::new();
        for t in self.machine.drain_transitions() {
            // A transition of an id the service has forgotten (or never
            // knew) writes no row and makes no stream update.
            if self.meta.contains_key(&t.harness_id) {
                transitions.entry(t.harness_id.clone()).or_default().push(t);
            }
        }
        let changed = self.machine.drain_changed();
        let armed = self.machine.drain_delegation_armed();
        self.publish(transitions, changed, armed);
    }

    /// What the webview's `forgetViewState` did on every unmount
    /// (`terminalEnsureApply.ts`): the record, both registries, the mute
    /// window, the screen state and reading, and the translators. A fresh
    /// spawn replaces the record, so adapter state tied to the old process
    /// is dead and the runner attaches the adapters again. Unlike
    /// [`PhaseService::forget`] it keeps `meta`.
    fn forget_machine(&mut self, id: &str) {
        self.machine.forget(id);
        self.machine.subagents_mut().forget(id);
        self.machine.background_tasks_mut().forget(id);
        self.drop_translators(id);
    }

    /// The body of [`PhaseService::spawned`].
    fn spawn_record(&mut self, id: &str, room_id: &str, injected: bool, now: i64) {
        self.forget_machine(id);
        self.machine.spawned(id, now);
        self.machine.set_injected(id, injected);
        self.meta.insert(
            id.to_owned(),
            HarnessMeta {
                room_id: room_id.to_owned(),
            },
        );
    }

    /// `StartFailed`: a spawn that produced no process, so `spawned` then
    /// `exited` at one instant. A record already `exited` is left alone.
    fn start_failed(&mut self, id: &str, room_id: &str, now: i64) {
        let exited = self
            .machine
            .get(id)
            .is_some_and(|a| a.phase == ActivityPhase::Exited);
        if exited && self.meta.contains_key(id) {
            return;
        }
        self.spawn_record(id, room_id, false, now);
        self.machine.exited(id, None, now);
    }

    /// `RestartRequested`: one row, nothing else (no machine change, no
    /// stream update).
    fn restart_requested(&mut self, id: &str, room_id: &str, source: RestartSource, now: i64) {
        let rec = self.machine.get(id);
        let row = EventRow {
            harness_id: id.to_owned(),
            room_id: room_id.to_owned(),
            from: rec.map_or("unknown", |a| a.phase.as_str()).to_owned(),
            to: ActivityPhase::Spawning.as_str(),
            at_ms: now,
            has_user_input: rec.is_some_and(|a| a.has_user_input),
            source: source.as_str(),
        };
        self.send(OutMsg::Row(row));
    }

    fn drop_translators(&mut self, id: &str) {
        self.claude.remove(id);
        self.opencode.remove(id);
        self.hermes.remove(id);
    }
}

/// See the module docs.
pub struct PhaseService {
    pub(super) inner: Mutex<Inner>,
    clock: Clock,
    /// A test service's database and the directory it lives in.
    #[cfg(test)]
    pub(super) test_state: Option<(tempfile::TempDir, Arc<Database>)>,
}

fn system_now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|d| i64::try_from(d.as_millis()).ok())
        .unwrap_or(0)
}

impl PhaseService {
    /// With the system clock. `db` receives the `harness_events` rows and
    /// `events` the phase stream.
    #[must_use]
    pub fn new(db: Arc<Database>, events: SharedEventSink) -> Arc<Self> {
        Self::with_clock(Arc::new(system_now_ms), db, events)
    }

    #[must_use]
    pub fn with_clock(clock: Clock, db: Arc<Database>, events: SharedEventSink) -> Arc<Self> {
        let (tx, rx) = channel();
        // `None` (logged): updates are then dropped, the machine still runs.
        drop(spawn_output_thread(rx, db, events));
        Arc::new(Self {
            inner: Mutex::new(Inner::new(tx)),
            clock,
            #[cfg(test)]
            test_state: None,
        })
    }

    /// Run `f` on the state at one instant: read the clock first, then lock,
    /// mutate, run [`Inner::after`], unlock. A panic in another caller must
    /// not stop phase tracking for good, so a poisoned lock is taken over.
    pub(super) fn with<R>(&self, f: impl FnOnce(&mut Inner, i64) -> R) -> R {
        let now = (self.clock)();
        let mut g = self.lock();
        let r = f(&mut g, now);
        g.after();
        r
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    // ---- lifecycle ------------------------------------------------------

    /// A spawn resolved. A respawn of a known id starts clean, as in TS where
    /// every unmount runs `forgetViewState` before the next `spawned`: the
    /// old process's record, registries, mute window, screen state and
    /// adapter state are all dropped first.
    pub fn spawned(&self, id: &str, room_id: &str, injected: bool) {
        self.with(|s, now| s.spawn_record(id, room_id, injected, now));
    }

    /// Ignored for an id the service does not know, so a late exit cannot
    /// resurrect a forgotten harness. (The pure machine still records one, as
    /// TS does.)
    pub fn exited(&self, id: &str, code: Option<i32>) {
        self.with(|s, now| {
            if s.meta.contains_key(id) {
                s.machine.exited(id, code, now);
            }
        });
    }

    /// Drop every trace of a harness: record, registries, translators and
    /// its `meta`.
    pub fn forget(&self, id: &str) {
        self.with(|s, _| {
            s.forget_machine(id);
            if let Some(m) = s.meta.remove(id) {
                s.pending_forgets.push((id.to_owned(), m.room_id));
            }
        });
    }

    // ---- PTY ------------------------------------------------------------

    pub fn record_output(&self, id: &str, chunk: &str) {
        self.with(|s, now| s.machine.record_output(id, Some(chunk), now));
    }

    // ---- adapters -------------------------------------------------------

    pub fn claude_attached(&self, id: &str) {
        self.with(|s, now| {
            s.claude
                .entry(id.to_owned())
                .or_default()
                .attached(&mut s.machine, id, now);
        });
    }

    pub fn claude_event(&self, id: &str, event: &ClaudeEvent) {
        // No attached translator: the event is dropped, never creates one.
        self.with(|s, now| {
            if let Some(t) = s.claude.get_mut(id) {
                t.apply(&mut s.machine, id, event, now);
            }
        });
    }

    pub fn claude_detached(&self, id: &str) {
        self.with(|s, now| {
            if let Some(t) = s.claude.get_mut(id) {
                t.detached(&mut s.machine, id, now);
            }
        });
    }

    pub fn opencode_attached(&self, id: &str) {
        self.with(|s, now| {
            s.opencode
                .entry(id.to_owned())
                .or_default()
                .attached(&mut s.machine, id, now);
        });
    }

    pub fn opencode_event(&self, id: &str, event: &OpencodeEvent) {
        self.with(|s, now| {
            if let Some(t) = s.opencode.get_mut(id) {
                t.apply(&mut s.machine, id, event, now);
            }
        });
    }

    pub fn opencode_detached(&self, id: &str) {
        self.with(|s, now| {
            if let Some(t) = s.opencode.get_mut(id) {
                t.detached(&mut s.machine, id, now);
            }
        });
    }

    /// A fresh Hermes binding replaces the old translator, as TS's
    /// `createHermesBinding` does: the old binding is disposed first
    /// (permission released, authority detached, work cleared).
    pub fn hermes_attached(&self, id: &str, bound_session_id: Option<String>) {
        self.with(|s, now| {
            if let Some(old) = s.hermes.get_mut(id) {
                old.detached(&mut s.machine, id, now);
            }
            let mut t = HermesTranslator::new(bound_session_id);
            t.attached(&mut s.machine, id, now);
            s.hermes.insert(id.to_owned(), t);
        });
    }

    /// The outcome is what the client still has to do: it claims the session
    /// id and ends the turn's action rows from the hook event it receives.
    /// Dropped (default outcome) when no binding is attached, as TS's
    /// `bindings.get(id)?.handle`.
    pub fn hermes_hook(&self, id: &str, hook: &HarnessHermesHook) -> HermesHookOutcome {
        self.with(|s, now| {
            s.hermes
                .get_mut(id)
                .map(|t| t.apply_hook(&mut s.machine, id, hook, now))
                .unwrap_or_default()
        })
    }

    pub fn hermes_work(&self, id: &str, event: &HermesWorkEvent) {
        self.with(|s, now| {
            if let Some(t) = s.hermes.get_mut(id) {
                t.apply_work(&mut s.machine, id, event, now);
            }
        });
    }

    pub fn hermes_detached(&self, id: &str) {
        self.with(|s, now| {
            if let Some(t) = s.hermes.get_mut(id) {
                t.detached(&mut s.machine, id, now);
            }
        });
    }

    /// Claude's `PermissionRequest` command hook (#86). `agent_type` and
    /// `agent_id` name the subagent that opened the dialog (#298).
    pub fn claude_permission_hook(
        &self,
        id: &str,
        tool_name: Option<&str>,
        agent_type: Option<&str>,
        agent_id: Option<&str>,
    ) {
        self.with(|s, now| {
            apply_hook_permission(&mut s.machine, id, tool_name, agent_type, agent_id, now);
        });
    }

    // ---- client inputs --------------------------------------------------

    /// One input only the webview can observe. See [`ClientPhaseInput`].
    pub fn client_input(&self, id: &str, input: ClientPhaseInput) {
        self.with(|s, now| match input {
            ClientPhaseInput::Input {
                data,
                empty_composer,
            } => s.machine.record_input(id, &data, empty_composer, now),
            ClientPhaseInput::PromptSubmitted => s.machine.note_prompt_submitted(id, now),
            ClientPhaseInput::MuteInducedOutput => s.machine.mute_induced_output(id, now),
            ClientPhaseInput::ScreenReading { reading } => {
                s.machine.set_screen_reading(id, reading);
            }
            ClientPhaseInput::HermesLaunchPoll { screen_waiting } => {
                if let Some(t) = s.hermes.get_mut(id) {
                    t.poll_launch(&mut s.machine, id, screen_waiting, now);
                }
            }
            ClientPhaseInput::LaunchSignal => {
                s.machine
                    .note_launch_signal(id, TransitionSource::L2c1ClaudeSessionStart, now);
            }
            ClientPhaseInput::SessionSwitched { switch } => {
                s.machine.session_switched(id, switch, now);
            }
            ClientPhaseInput::RecoverUnheardSilence => {
                s.machine.recover_unheard_silence(id, now);
            }
            // These two need the stored room and a liveness check, which only
            // the runner has (`HarnessRunner::client_phase_input`).
            ClientPhaseInput::StartFailed | ClientPhaseInput::RestartRequested { .. } => {
                tracing::debug!(
                    target: "skein::phase",
                    harness = %id,
                    "start/restart input dropped: it must come through the runner"
                );
            }
        });
    }

    /// `StartFailed` for a harness the caller has verified is stored in
    /// `room_id` and has no live process.
    pub(crate) fn start_failed(&self, id: &str, room_id: &str) {
        self.with(|s, now| s.start_failed(id, room_id, now));
    }

    /// `RestartRequested` for a harness the caller has verified is stored in
    /// `room_id`.
    pub(crate) fn restart_requested(&self, id: &str, room_id: &str, source: RestartSource) {
        self.with(|s, now| s.restart_requested(id, room_id, source, now));
    }

    // ---- tick -----------------------------------------------------------

    /// One pass of the background scan: the machine's tick, then the supervisor.
    /// `facts` is what only the caller knows (live attaches, mail, transcript samples); gather it before
    /// calling, it is not read under the lock.
    pub fn tick_once(&self, facts: &SupervisorFacts) {
        self.with(|s, now| {
            s.sweep = true;
            s.machine.tick(now);
            for v in s.machine.supervise(facts, now) {
                tracing::debug!(target: "skein::supervisor", violation = ?v, "supervisor");
            }
        });
    }

    /// Spawn the one tick thread (every [`TICK_INTERVAL_MS`]). It holds only a
    /// `Weak`, so it ends within one interval of the last `Arc` being
    /// dropped. A std thread, not a task: the PTYs already use them and the
    /// daemon must not need a tokio runtime for this. `None` if the OS
    /// refused the thread (logged; the machine then just never ticks).
    #[must_use]
    pub fn start_ticker(
        self: &Arc<Self>,
        facts: Arc<dyn Fn() -> SupervisorFacts + Send + Sync>,
    ) -> Option<JoinHandle<()>> {
        let ms = u64::try_from(TICK_INTERVAL_MS).unwrap_or(1_000);
        self.start_ticker_every(Duration::from_millis(ms), facts)
    }

    pub(super) fn start_ticker_every(
        self: &Arc<Self>,
        every: Duration,
        facts: Arc<dyn Fn() -> SupervisorFacts + Send + Sync>,
    ) -> Option<JoinHandle<()>> {
        let weak: Weak<Self> = Arc::downgrade(self);
        std::thread::Builder::new()
            .name("skein-phase-tick".to_owned())
            .spawn(move || {
                loop {
                    std::thread::sleep(every);
                    let Some(svc) = weak.upgrade() else {
                        break;
                    };
                    // A panic (a bad facts closure) must not end phase
                    // tracking for good: log it and keep ticking.
                    let step = std::panic::catch_unwind(AssertUnwindSafe(|| {
                        let f = facts();
                        svc.tick_once(&f);
                    }));
                    if step.is_err() {
                        tracing::error!(target: "skein::phase", "phase tick panicked; continuing");
                    }
                }
            })
            .map_err(|e| tracing::error!("phase ticker: cannot start thread: {e}"))
            .ok()
    }

    // ---- reads ----------------------------------------------------------

    #[must_use]
    pub fn phase_snapshot(&self) -> BTreeMap<String, ActivityPhase> {
        self.lock().machine.phase_snapshot()
    }

    /// Every harness id to its record's `spawned_at`: one lock round trip.
    #[must_use]
    pub fn spawned_at_snapshot(&self) -> BTreeMap<String, i64> {
        self.lock().machine.spawned_at_snapshot()
    }

    /// One update per known harness, current state, no transitions, all at
    /// the current stream seq: a client that listens first and then reads
    /// this drops every stream update with `seq <=` the snapshot's.
    #[must_use]
    pub fn stream_snapshot(&self) -> Vec<PhaseUpdate> {
        self.lock().snapshot()
    }

    #[must_use]
    pub fn get(&self, id: &str) -> Option<HarnessActivity> {
        self.lock().machine.get(id).cloned()
    }

    #[must_use]
    pub fn work_snapshot<'a>(
        &self,
        ids: impl IntoIterator<Item = &'a str>,
    ) -> BTreeMap<String, WorkCounts> {
        self.lock().machine.work_snapshot(ids)
    }
}
