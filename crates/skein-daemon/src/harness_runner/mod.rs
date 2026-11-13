//! The daemon starts and stops harnesses itself (#731, skeind P2a slice 3).
//!
//! `ensure_running` spawns a harness from its stored record, with no client
//! involved, and attaches the L2c adapters. The argv is DERIVED from the
//! record at spawn time (`harness_argv::spawn_argv`, #752: launch mode, kind,
//! session id and a session-existence probe), never spawned as the stored
//! `cmd` text (in shell launch mode the stored shell argv is what is spawned);
//! the session id the derivation ends with is written back. A view going
//! away (`detach_view`) never kills it; only `stop_harness`, a record change
//! (`reconcile`) or `restart` does. When the process exits on its own the
//! runner does nothing: the entry stays and reads as exited, and nothing is
//! started in its place.
//!
//! Locks: `spawns` holds one gate per harness; `ensure_running`, the public
//! `stop_harness` and `release_events` take it, so a stop cannot slip in
//! between a spawn and its entry being recorded. `entries` is held only for
//! map reads and writes, never across a kill, a spawn or a store call.
//! `reconcile` runs from the room store's observer after its lock is released
//! and takes only `entries`, never a gate: a spawn's own record write-back
//! (which re-enters the observer on the gate-holding thread) would deadlock
//! on it, and the observer must not wait for a seconds-long spawn either.
//! What it may miss (an `ensure_running` that has not recorded its entry yet)
//! `ensure_running` repairs itself by re-checking the records afterwards.
//! `is_busy` may be called under the store lock: it takes `entries` and the
//! gate map only briefly and only try-locks a gate; no runner code holds
//! `entries` or the gate map while calling the store.

mod adapters;
mod client_input;
mod events;
mod gates;
mod phase_tap;
mod record;

pub use self::record::{SESSION_REPORT_ORIGIN, SessionReport};

use std::collections::HashMap;
use std::sync::Arc;

use parking_lot::Mutex;
use skein_proto::rooms::{Harness, LaunchMode, Room};

use self::adapters::{Adapters, AttachSpec};
pub use self::gates::{EnsureError, Refusal};
use self::gates::{SpawnPlan, lookup, wanted};
use self::phase_tap::RunnerObserver;
use crate::agent_endpoint::AgentApiEndpoint;
use crate::boot_resume::{opencode_port_from_cmd, probe_session};
use crate::db::Database;
use crate::event_sink::{self, SharedEventSink};
use crate::harness_actions_hermes::HermesActionsManager;
use crate::harness_config::HarnessConfig;
use crate::harness_events_claude::ClaudeEventsManager;
use crate::harness_events_opencode::OpencodeEventsManager;
use crate::harness_kind::HarnessKind;
use crate::harness_phase::PhaseService;
use crate::harness_work_hermes::HermesWorkAttacher;
use crate::opencode_sidecar::start::{EXIT_EVENT, SidecarExitPayload};
use crate::opencode_sidecar::{OnExit, OpencodeSidecarManager};
use crate::pty::command::{PtySpawnArgs, PtySpawnDeps, PtySpawnResult, pty_spawn_with_sink};
use crate::pty::{AttachOutcome, ExitRecord, PtyEvent, PtyManager, PtyObserver};
use crate::room_store::RoomStore;
use crate::spawn_env::state::SpawnEnvState;
use crate::stream_sink::StreamSink;

#[cfg(test)]
type PauseHook = Arc<dyn Fn() + Send + Sync>;

/// The `origin` of the record writes the runner makes.
pub const RUNNER_ORIGIN: &str = "daemon-runner";

/// Rows and columns used when a request has none (zero).
const DEFAULT_ROWS: u16 = 24;
const DEFAULT_COLS: u16 = 80;

/// What the runner is built from. Managers are clones that share the managed
/// state's registries.
pub struct RunnerDeps {
    pub store: Arc<RoomStore>,
    pub db: Arc<Database>,
    pub pty: PtyManager,
    pub sidecars: OpencodeSidecarManager,
    pub claude: ClaudeEventsManager,
    pub opencode: OpencodeEventsManager,
    pub hermes_actions: HermesActionsManager,
    pub hermes_work: Arc<HermesWorkAttacher>,
    pub spawn_env: Arc<SpawnEnvState>,
    pub endpoint: AgentApiEndpoint,
    pub harness_config: HarnessConfig,
    pub events: SharedEventSink,
    /// The phase machine (#732); the runner feeds it.
    pub phase: Arc<PhaseService>,
    /// The session-existence probe; `None` = the real one over the tools'
    /// own stores ([`probe_session`]). Tests inject.
    pub session_probe: Option<SessionProbe>,
}

#[derive(Debug, Clone)]
pub struct EnsureRequest {
    pub room_id: String,
    pub harness_id: String,
    pub rows: u16,
    pub cols: u16,
    /// Beats the record's own `envProfile` when set.
    pub env_profile: Option<String>,
    /// Stop a running process first and spawn a new one.
    pub restart: bool,
    /// The argv the caller believes the record holds. When set and different,
    /// the request is refused as `stale_record` before anything is touched.
    pub expect_cmd: Option<Vec<String>>,
    /// The launch mode the caller believes the record holds (absent in the
    /// record reads as `Tool`). When set and different, the request is
    /// refused as `stale_record`, like `expect_cmd`.
    pub expect_launch: Option<LaunchMode>,
}

/// "Does this harness's stored session still exist?" The runner asks it at
/// every spawn to choose resume over fresh (#752). An error keeps the id.
pub type SessionProbe = Arc<dyn Fn(&Harness) -> Result<bool, String> + Send + Sync>;

pub struct EnsureOutcome {
    /// False when an existing process was reused.
    pub started: bool,
    /// The result of the spawn that made the process (the stored one when
    /// reused).
    pub spawn: PtySpawnResult,
    /// Set once the process has exited.
    pub exit: Option<ExitRecord>,
    /// The id of the sink attached by this call (`None` without a sink);
    /// pass it to [`HarnessRunner::detach_view_if`].
    pub attach_id: Option<u64>,
}

/// One harness the runner spawned.
#[derive(Clone)]
struct Running {
    pty_id: String,
    room_id: String,
    kind: HarnessKind,
    spawn: PtySpawnResult,
    cwd: String,
    /// argv[0] as spawned.
    program: Option<String>,
    hermes_profile: Option<String>,
    opencode_port: Option<u16>,
    /// Retired when this process is stopped or replaced (#732).
    observer: Arc<RunnerObserver>,
}

pub struct HarnessRunner {
    store: Arc<RoomStore>,
    db: Arc<Database>,
    pty: PtyManager,
    sidecars: OpencodeSidecarManager,
    spawn_env: Arc<SpawnEnvState>,
    endpoint: AgentApiEndpoint,
    harness_config: HarnessConfig,
    sidecar_exit: OnExit,
    adapters: Adapters,
    phase: Arc<PhaseService>,
    session_probe: Mutex<SessionProbe>,
    entries: Mutex<HashMap<String, Running>>,
    spawns: Mutex<HashMap<String, Arc<Mutex<()>>>>,
    /// Test-only pause points: name -> a closure run when the code reaches it.
    #[cfg(test)]
    pause_points: Mutex<HashMap<&'static str, PauseHook>>,
    /// Test-only: every argv handed to the PTY layer, oldest first.
    #[cfg(test)]
    spawned_argv: Mutex<Vec<Vec<String>>>,
    /// Test-only: when set, this argv is spawned in place of the derived one
    /// (which is still recorded), so a test can derive `claude ...` without
    /// the CLI being installed.
    #[cfg(test)]
    stand_in: Mutex<Option<Vec<String>>>,
}

/// What happens when an opencode sidecar exits on its own: tell the clients,
/// then end the bound TUI, which would otherwise reconnect forever.
fn sidecar_exit_hook(pty: PtyManager, events: SharedEventSink) -> OnExit {
    Arc::new(move |notice| {
        tracing::info!(
            harness = %notice.harness_id,
            generation = notice.generation,
            code = ?notice.code,
            "opencode sidecar exited"
        );
        let (harness_id, generation) = (notice.harness_id.clone(), notice.generation);
        // Event first, so a view knows why the TUI died when the PTY's own
        // Exit arrives.
        if let Err(e) = event_sink::emit(
            events.as_ref(),
            EXIT_EVENT,
            &SidecarExitPayload::from(notice),
        ) {
            tracing::warn!(error = %e, "harness runner: sidecar exit event failed");
        }
        let ended = pty.terminate_bound(&harness_id, generation);
        if ended > 0 {
            tracing::info!(harness = %harness_id, generation, ended, "terminated TUI of a dead sidecar");
        }
    })
}

impl HarnessRunner {
    #[must_use]
    pub fn new(deps: RunnerDeps) -> Self {
        let RunnerDeps {
            store,
            db,
            pty,
            sidecars,
            claude,
            opencode,
            hermes_actions,
            hermes_work,
            spawn_env,
            endpoint,
            harness_config,
            events,
            phase,
            session_probe,
        } = deps;
        let session_probe = session_probe.unwrap_or_else(|| {
            let env = Arc::clone(&spawn_env);
            Arc::new(move |h: &Harness| probe_session(Some(&env), h))
        });
        Self {
            session_probe: Mutex::new(session_probe),
            store,
            db,
            sidecar_exit: sidecar_exit_hook(pty.clone(), events),
            pty,
            sidecars: sidecars.clone(),
            adapters: Adapters {
                claude,
                opencode,
                hermes: hermes_actions,
                hermes_work,
                sidecars,
                spawn_env: Arc::clone(&spawn_env),
                phase: Arc::clone(&phase),
            },
            phase,
            spawn_env,
            endpoint,
            harness_config,
            entries: Mutex::new(HashMap::new()),
            spawns: Mutex::new(HashMap::new()),
            #[cfg(test)]
            pause_points: Mutex::new(HashMap::new()),
            #[cfg(test)]
            spawned_argv: Mutex::new(Vec::new()),
            #[cfg(test)]
            stand_in: Mutex::new(None),
        }
    }

    /// The PTY id of the harness's current process, if the runner has one.
    #[must_use]
    pub fn pty_id(&self, harness_id: &str) -> Option<String> {
        self.entries
            .lock()
            .get(harness_id)
            .map(|r| r.pty_id.clone())
    }

    /// Is the harness up or being started right now? True when the runner
    /// holds an entry for it, or when its spawn gate is held (a spawn or stop
    /// in flight: the record it read must not be rewritten under it). Never
    /// blocks and creates no gate.
    #[must_use]
    pub fn is_busy(&self, harness_id: &str) -> bool {
        if self.entries.lock().contains_key(harness_id) {
            return true;
        }
        let gate = self.spawns.lock().get(harness_id).cloned();
        gate.is_some_and(|g| g.try_lock().is_none())
    }

    /// Record a client's session observation field-level (#755); see
    /// [`SessionReport`]. `Ok(true)` when the record changed.
    ///
    /// # Errors
    /// The store refused the edit.
    pub fn report_session(
        &self,
        room_id: &str,
        harness_id: &str,
        report: SessionReport,
    ) -> Result<bool, String> {
        record::report_session(&self.store, room_id, harness_id, report)
    }

    /// Make sure the harness has a process, spawning one from its record when
    /// it has none. Blocking (the spawn probes the CLI, may start a sidecar
    /// and the Claude attach reads the transcript): run it off the executor,
    /// inside the tokio runtime.
    ///
    /// With a process already known and `restart` false nothing is spawned:
    /// `sink` is attached to that process (an exited one delivers its `Exit`
    /// through the attach) and the stored spawn result comes back with
    /// `started: false`. Folder, kind, agent and session id come from the
    /// record, never from the caller; the argv is derived from it at spawn
    /// time (resume versus fresh via the session probe, the shell argv in
    /// shell launch mode). A session id minted or dropped by the derivation
    /// is written back to the record after the spawn.
    ///
    /// # Errors
    /// A [`Refusal`] (see [`Refusal::code`]), or the spawn failed.
    pub fn ensure_running(
        &self,
        req: &EnsureRequest,
        sink: Option<Box<dyn StreamSink<PtyEvent>>>,
    ) -> Result<EnsureOutcome, EnsureError> {
        let gate = self.spawn_gate(&req.harness_id);
        let _one_at_a_time = gate.lock();

        let rooms = self.store.rooms();
        let (room, harness, kind) =
            lookup(&rooms, &req.room_id, &req.harness_id).map_err(EnsureError::Refused)?;

        if let Some(expect) = &req.expect_cmd
            && harness.cmd.as_ref() != Some(expect)
        {
            return Err(EnsureError::Refused(Refusal::StaleRecord));
        }
        if let Some(expect) = &req.expect_launch
            && harness.launch.clone().unwrap_or(LaunchMode::Tool) != *expect
        {
            return Err(EnsureError::Refused(Refusal::StaleRecord));
        }

        let existing = self.entries.lock().get(&req.harness_id).cloned();
        let mut sink = sink;
        if let Some(run) = existing {
            if !req.restart && self.pty_known(&run.pty_id) {
                let mut attach_id = None;
                let exit = match sink.take() {
                    Some(s) => {
                        let (outcome, id) = self
                            .pty
                            .attach_tracked(&run.pty_id, s)
                            .map_err(|e| EnsureError::SpawnFailed(e.to_string()))?;
                        attach_id = Some(id);
                        match outcome {
                            AttachOutcome::Live => None,
                            AttachOutcome::Exited { code } => Some(ExitRecord { code }),
                        }
                    }
                    None => self.pty.exit_code(&run.pty_id),
                };
                return Ok(EnsureOutcome {
                    started: false,
                    spawn: run.spawn,
                    exit,
                    attach_id,
                });
            }
            // A restart, or a process someone killed behind our back. The
            // gate is already ours. The phase record is kept for the
            // respawn to replace (it keeps its place in the tick order).
            self.stop_process(&req.harness_id, false);
        }

        let probe = Arc::clone(&self.session_probe.lock());
        let plan = match SpawnPlan::from_record(
            room,
            harness,
            kind,
            req.env_profile.as_deref(),
            &*probe,
        ) {
            Ok(plan) => plan,
            Err(refusal) => {
                // The old process (if any) is already stopped, and its phase
                // record was kept for a respawn that is not coming.
                self.phase.forget(&req.harness_id);
                return Err(EnsureError::Refused(refusal));
            }
        };
        let (room_id, session_before, version_before) = (
            room.id.clone(),
            harness.session_id.clone(),
            harness.opencode_version.clone(),
        );
        let has_sink = sink.is_some();
        self.pause_point("before_spawn");
        // `spawned` is called from inside the spawn, before the child's
        // threads start (see `RunnerObserver`).
        let observer = RunnerObserver::new(Arc::clone(&self.phase), &req.harness_id, &room_id);
        let spawn = match self.spawn(req, &room_id, &plan, sink, &observer) {
            Ok(spawn) => spawn,
            Err(e) => {
                // A restart that failed leaves no process for the record.
                self.phase.forget(&req.harness_id);
                return Err(e);
            }
        };

        let program = plan.cmd.first().cloned();
        let run = Running {
            pty_id: spawn.id.clone(),
            room_id: room_id.clone(),
            kind,
            spawn: spawn.clone(),
            cwd: plan.cwd.clone(),
            program,
            hermes_profile: plan.hermes_profile.clone(),
            opencode_port: opencode_port_from_cmd(&plan.cmd),
            observer,
        };
        self.entries
            .lock()
            .insert(req.harness_id.clone(), run.clone());

        // The derived id (minted, or the stale one dropped); opencode v2 may
        // have created or confirmed a different session, and that wins.
        let session_now = spawn.session_id.clone().or(plan.session_id.clone());
        self.attach_adapters(&req.harness_id, &run, session_now.as_deref(), true);
        record::write_back(
            &self.store,
            &room_id,
            &req.harness_id,
            kind,
            &spawn,
            session_now.as_deref(),
            session_before.as_deref(),
            version_before.as_ref(),
        );

        // The records may have changed while the spawn ran (the room was
        // archived, the harness removed); `reconcile` ran then, but found no
        // entry yet.
        if let Err(refusal) = lookup(&self.store.rooms(), &room_id, &req.harness_id) {
            self.stop_unlocked(&req.harness_id);
            return Err(EnsureError::Refused(refusal));
        }
        Ok(EnsureOutcome {
            started: true,
            attach_id: if has_sink {
                self.pty.attach_id(&spawn.id)
            } else {
                None
            },
            exit: self.pty.exit_code(&spawn.id),
            spawn,
        })
    }

    /// Kill the process (and the sidecar bound to it), detach the three
    /// adapters and forget the harness. Idempotent. Returns whether the runner
    /// had it. Waits for an `ensure_running` of the same harness that is
    /// mid-spawn, then stops what it made.
    pub fn stop_harness(&self, harness_id: &str) -> bool {
        let gate = self.spawn_gate(harness_id);
        let removed = {
            let _one_at_a_time = gate.lock();
            self.stop_unlocked(harness_id)
        };
        self.prune_gate(harness_id, &gate);
        removed
    }

    /// [`stop_harness`](Self::stop_harness) for callers that hold the gate or
    /// must not wait for it (`reconcile`).
    fn stop_unlocked(&self, harness_id: &str) -> bool {
        self.stop_process(harness_id, true)
    }

    /// Kill and detach; `forget_phase` also drops the harness's phase record.
    /// False only on the restart path, where the respawn replaces it.
    fn stop_process(&self, harness_id: &str, forget_phase: bool) -> bool {
        let removed = self.entries.lock().remove(harness_id);
        if let Some(run) = &removed {
            // Before the kill: the dying process's exit must not reach the
            // phase record (a restart is about to replace it).
            run.observer.retire();
            if let Some((sidecar_harness, generation)) = self.pty.kill(&run.pty_id) {
                self.sidecars
                    .stop_if_generation(&sidecar_harness, generation);
            }
        }
        if forget_phase {
            // First, so a Hermes session-start hook landing mid-stop sees no
            // record and attaches nothing; `adapters.detach` never reads the
            // phase record. Takes the translators with it, so no separate
            // `*_detached`.
            self.phase.forget(harness_id);
        }
        self.adapters.detach(harness_id);
        removed.is_some()
    }

    /// The phase service this runner feeds (#732).
    #[must_use]
    pub fn phase(&self) -> &Arc<PhaseService> {
        &self.phase
    }

    /// A view went away: clear the PTY's sink and nothing else. Never kills.
    /// Returns whether a sink was attached.
    pub fn detach_view(&self, harness_id: &str) -> bool {
        let Some(pty_id) = self.pty_id(harness_id) else {
            return false;
        };
        self.pty.detach(&pty_id)
    }

    /// [`detach_view`](Self::detach_view), but only if `attach_id` (from
    /// [`EnsureOutcome::attach_id`]) is still the current attachment: a late
    /// detach from an old view must not blank a newer one.
    pub fn detach_view_if(&self, harness_id: &str, attach_id: u64) -> bool {
        let Some(pty_id) = self.pty_id(harness_id) else {
            return false;
        };
        self.pty.detach_if(&pty_id, attach_id)
    }

    /// Stop every harness the records no longer want: removed, or in an
    /// archived room. Called by the room store's observer after each applied
    /// batch. Kills happen after `entries` is released.
    ///
    /// Two concurrent applies can run their observers in either order, so the
    /// batch's own snapshot may be older than the store: the current records
    /// are read here and `_batch` is ignored.
    pub fn reconcile(&self, _batch: &[Room]) {
        let rooms = self.store.rooms();
        let gone: Vec<String> = self
            .entries
            .lock()
            .keys()
            .filter(|id| !wanted(&rooms, id))
            .cloned()
            .collect();
        for id in gone {
            tracing::info!(harness = %id, "harness runner: records no longer want it; stopping");
            self.stop_unlocked(&id);
        }
    }

    /// Hand the runner to the store so it hears of every applied batch. The
    /// observer holds a weak reference: the store must not keep the runner
    /// alive, nor the runner (which holds the store) the other way round.
    pub fn observe_store(self: &Arc<Self>) {
        let weak = Arc::downgrade(self);
        self.store.observe(move |rooms| {
            if let Some(runner) = weak.upgrade() {
                runner.reconcile(rooms);
            }
        });
    }

    fn spawn(
        &self,
        req: &EnsureRequest,
        room_id: &str,
        plan: &SpawnPlan,
        sink: Option<Box<dyn StreamSink<PtyEvent>>>,
        observer: &Arc<RunnerObserver>,
    ) -> Result<PtySpawnResult, EnsureError> {
        let sidecars = self.sidecars.clone();
        let cmd = plan.cmd.clone();
        #[cfg(test)]
        let cmd = {
            self.spawned_argv.lock().push(cmd.clone());
            self.stand_in.lock().clone().unwrap_or(cmd)
        };
        pty_spawn_with_sink(
            PtySpawnDeps {
                manager: &self.pty,
                spawn_env: &self.spawn_env,
                db: &self.db,
                endpoint: &self.endpoint,
                harness_config: &self.harness_config,
                sidecars: &self.sidecars,
            },
            PtySpawnArgs {
                cmd,
                cwd: plan.cwd.clone(),
                rows: if req.rows == 0 {
                    DEFAULT_ROWS
                } else {
                    req.rows
                },
                cols: if req.cols == 0 {
                    DEFAULT_COLS
                } else {
                    req.cols
                },
                room_id: room_id.to_owned(),
                harness_id: req.harness_id.clone(),
                kind: plan.kind,
                env_profile: plan.env_profile.clone(),
                agent: plan.agent.clone(),
                session_id: plan.session_id.clone(),
            },
            sink,
            move |(harness_id, generation)| {
                sidecars.stop_if_generation(harness_id, *generation);
            },
            &self.sidecar_exit,
            Some(Arc::clone(observer) as Arc<dyn PtyObserver>),
        )
        .map_err(EnsureError::SpawnFailed)
    }

    /// The adapters for a running harness: v1 opencode by the stored `--port`,
    /// v2 by the sidecar generation the spawn reported.
    fn attach_adapters(
        &self,
        harness_id: &str,
        run: &Running,
        session_id: Option<&str>,
        fresh_process: bool,
    ) {
        self.adapters.attach(&AttachSpec {
            harness_id,
            room_id: &run.room_id,
            kind: run.kind,
            cwd: &run.cwd,
            session_id,
            opencode_port: if run.spawn.sidecar_generation.is_some() {
                None
            } else {
                run.opencode_port
            },
            sidecar_generation: run.spawn.sidecar_generation,
            program: run.program.as_deref(),
            hermes_profile: run.hermes_profile.as_deref(),
            hermes_hooks: run.spawn.hermes_hooks,
            fresh_process,
        });
    }

    /// The process exists in the PTY manager, running or exited.
    fn pty_known(&self, pty_id: &str) -> bool {
        self.pty.is_alive(pty_id) || self.pty.exit_code(pty_id).is_some()
    }

    fn spawn_gate(&self, harness_id: &str) -> Arc<Mutex<()>> {
        Arc::clone(self.spawns.lock().entry(harness_id.to_owned()).or_default())
    }

    /// Forget a gate nobody else holds. `gate` is the caller's clone (its
    /// guard already dropped); a count above map + caller means another
    /// thread is queued on it or about to be, so it stays.
    fn prune_gate(&self, harness_id: &str, gate: &Arc<Mutex<()>>) {
        let mut spawns = self.spawns.lock();
        if spawns
            .get(harness_id)
            .is_some_and(|g| Arc::ptr_eq(g, gate) && Arc::strong_count(g) == 2)
        {
            spawns.remove(harness_id);
        }
    }

    /// Test-only: replace the session probe.
    #[cfg(test)]
    fn set_session_probe(&self, probe: SessionProbe) {
        *self.session_probe.lock() = probe;
    }

    /// Test-only: spawn `argv` in place of whatever is derived (which is
    /// still recorded by [`spawned_argv`](Self::spawned_argv)).
    #[cfg(test)]
    fn set_stand_in(&self, argv: Vec<String>) {
        *self.stand_in.lock() = Some(argv);
    }

    /// Test-only: every derived argv handed to the PTY layer, oldest first.
    #[cfg(test)]
    fn spawned_argv(&self) -> Vec<Vec<String>> {
        self.spawned_argv.lock().clone()
    }

    #[cfg(test)]
    fn pause_point(&self, name: &str) {
        let hook = self.pause_points.lock().get(name).cloned();
        if let Some(hook) = hook {
            hook();
        }
    }

    #[cfg(not(test))]
    #[allow(clippy::unused_self)]
    fn pause_point(&self, _name: &str) {}
}

#[cfg(test)]
mod tests;
