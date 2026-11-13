//! The driver around the pure [`PhaseMachine`] (#732): [`PhaseService`].
//!
//! The machine takes the time as an argument and queues its transitions in an
//! outbox; this is the part that owns the clock, the per-harness translators,
//! the tick thread and the shadow comparison. It is shared by reference
//! (`Arc<PhaseService>`), every method takes `&self`, and all state sits
//! behind one `std::sync::Mutex` that is never held across a callback or any
//! I/O: the clock is read and the facts are gathered before the lock is
//! taken, and nothing is logged to a database from inside it.
//!
//! Shadow mode: the webview's machine still decides everything. This one runs
//! on the same inputs and [`Inner::after`] hands each transition it makes to
//! the [`ShadowComparator`], which logs the ones the client's machine did not
//! also make.

use std::collections::{BTreeMap, HashMap};
use std::panic::AssertUnwindSafe;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};
use std::thread::JoinHandle;
use std::time::Duration;

use super::client_input::ClientPhaseInput;
use super::constants::TICK_INTERVAL_MS;
use super::machine::PhaseMachine;
use super::shadow::{ShadowComparator, ShadowStats};
use super::translate::claude::{ClaudeTranslator, apply_hook_permission};
use super::translate::hermes::{HermesHookOutcome, HermesTranslator};
use super::translate::opencode::OpencodeTranslator;
use super::types::{ActivityPhase, HarnessActivity, TransitionSource};
use super::{SupervisorFacts, WorkCounts};
use crate::agent_api::hermes_event::HarnessHermesHook;
use crate::harness_events_claude::ClaudeEvent;
use crate::harness_events_opencode::OpencodeEvent;
use crate::harness_work_hermes::HermesWorkEvent;

/// Epoch ms. Injected so tests drive time.
pub type Clock = Arc<dyn Fn() -> i64 + Send + Sync>;

/// What the service knows about a harness beyond the machine's record.
#[derive(Debug, Clone)]
pub(super) struct HarnessMeta {
    // FLIP(#732): the second PR reads this for the `harness_events` write and
    // the phase stream; until then nothing does.
    #[allow(dead_code)]
    pub(super) room_id: String,
}

#[derive(Default)]
pub(super) struct Inner {
    pub(super) machine: PhaseMachine,
    pub(super) claude: HashMap<String, ClaudeTranslator>,
    pub(super) opencode: HashMap<String, OpencodeTranslator>,
    pub(super) hermes: HashMap<String, HermesTranslator>,
    pub(super) meta: HashMap<String, HarnessMeta>,
    pub(super) shadow: ShadowComparator,
}

impl Inner {
    /// Runs after every mutating call and every tick, still under the lock.
    ///
    /// FLIP(#732): this is where the next PR writes the `harness_events` rows
    /// and emits the phase stream from the transitions, and acts on the
    /// delegation-armed ids (#381). In shadow mode the Rust side only feeds
    /// the comparator, and the changed/armed outboxes are drained and dropped
    /// so they cannot grow.
    fn after(&mut self, now_ms: i64) {
        for t in self.machine.drain_transitions() {
            // A transition of an id the service has forgotten (or never
            // knew) must not make the comparator learn it.
            if self.meta.contains_key(&t.harness_id) {
                // The comparator logs what it finds; nothing here needs it.
                let _ = self.shadow.rust_transition(&t, now_ms);
            }
        }
        drop(self.machine.drain_changed());
        drop(self.machine.drain_delegation_armed());
    }

    /// A fresh spawn replaces the record, so adapter state tied to the old
    /// process is dead; the runner attaches the adapters again.
    /// What the webview's `forgetViewState` does on every unmount
    /// (`terminalEnsureApply.ts`): the record, both registries, the mute
    /// window, the screen state and reading, and the translators. Unlike
    /// [`PhaseService::forget`] it keeps `meta` and the comparator's state.
    fn forget_machine(&mut self, id: &str) {
        self.machine.forget(id);
        self.machine.subagents_mut().forget(id);
        self.machine.background_tasks_mut().forget(id);
        self.drop_translators(id);
    }

    fn drop_translators(&mut self, id: &str) {
        self.claude.remove(id);
        self.opencode.remove(id);
        self.hermes.remove(id);
    }
}

/// See the module docs.
pub struct PhaseService {
    inner: Mutex<Inner>,
    clock: Clock,
}

fn system_now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|d| i64::try_from(d.as_millis()).ok())
        .unwrap_or(0)
}

impl PhaseService {
    /// With the system clock.
    #[must_use]
    pub fn new() -> Arc<Self> {
        Self::with_clock(Arc::new(system_now_ms))
    }

    #[must_use]
    pub fn with_clock(clock: Clock) -> Arc<Self> {
        Arc::new(Self {
            inner: Mutex::new(Inner::default()),
            clock,
        })
    }

    /// Run `f` on the state at one instant: read the clock first, then lock,
    /// mutate, run [`Inner::after`], unlock. A panic in another caller must
    /// not stop phase tracking for good, so a poisoned lock is taken over.
    pub(super) fn with<R>(&self, f: impl FnOnce(&mut Inner, i64) -> R) -> R {
        let now = (self.clock)();
        let mut g = self.lock();
        let r = f(&mut g, now);
        g.after(now);
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
        self.with(|s, now| {
            s.forget_machine(id);
            s.machine.spawned(id, now);
            s.machine.set_injected(id, injected);
            s.meta.insert(
                id.to_owned(),
                HarnessMeta {
                    room_id: room_id.to_owned(),
                },
            );
            s.shadow.register(id);
            // A real spawn voids any open view-reset resync.
            s.shadow.end_resync(id, now);
        });
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
    /// whatever the comparator still holds for it.
    pub fn forget(&self, id: &str) {
        self.with(|s, _| {
            s.forget_machine(id);
            s.meta.remove(id);
            s.shadow.forget(id);
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

    /// The outcome is what the client still has to do (claim the session id,
    /// end the turn's actions); the daemon will own that in the flip.
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
            ClientPhaseInput::Reattached => s.machine.reattached(id, now),
            // The webview rebuilt its own record: the machine stays as it
            // is, only the comparison needs to resync.
            ClientPhaseInput::ViewReset => s.shadow.view_reset(id, now),
        });
    }

    // ---- shadow ---------------------------------------------------------

    /// One transition the webview's machine made (a `harness_events` row as
    /// the TS side wrote it), to be paired with the Rust side's own.
    pub fn shadow_client_transition(
        &self,
        id: &str,
        from: &str,
        to: &str,
        source: Option<&str>,
        at_ms: i64,
    ) {
        let now = (self.clock)();
        // The comparator logs the divergences it finds.
        let _ = self
            .lock()
            .shadow
            .client_transition(id, from, to, source, at_ms, now);
    }

    #[must_use]
    pub fn shadow_stats(&self) -> ShadowStats {
        self.lock().shadow.stats()
    }

    // ---- tick -----------------------------------------------------------

    /// One pass of the background scan: the machine's tick, the supervisor,
    /// then the comparator's expiry sweep. `facts` is what only the caller
    /// knows (live attaches, mail, transcript samples); gather it before
    /// calling, it is not read under the lock.
    pub fn tick_once(&self, facts: &SupervisorFacts) {
        self.with(|s, now| {
            s.machine.tick(now);
            for v in s.machine.supervise(facts, now) {
                tracing::debug!(target: "skein::phase_shadow", violation = ?v, "supervisor");
            }
            let _ = s.shadow.sweep(now);
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
