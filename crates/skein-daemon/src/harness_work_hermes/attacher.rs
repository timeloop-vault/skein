//! The daemon's own attach of the Hermes work poller (#732). The poller needs
//! a session id, so it is attached where the daemon learns one: at spawn for a
//! harness resumed with a bound session (the runner), and when the
//! `on_session_start` hook reports the session of a fresh one (the hook tap).
//! The phase machine is its only consumer; it stops with the harness.

use std::collections::HashMap;
use std::sync::Arc;

use parking_lot::Mutex;

use super::{HermesWorkManager, hermes_work_attach_impl};
use crate::harness_phase::sinks::HermesWorkPhaseSink;
use crate::harness_phase::{ActivityPhase, PhaseService};
use crate::room_store::RoomStore;
use crate::spawn_env::state::SpawnEnvState;

pub struct HermesWorkAttacher {
    manager: HermesWorkManager,
    spawn_env: Arc<SpawnEnvState>,
    phase: Arc<PhaseService>,
    /// The session each attached poller reads (`attachedFor`).
    sessions: Mutex<HashMap<String, String>>,
}

impl HermesWorkAttacher {
    #[must_use]
    pub fn new(
        manager: HermesWorkManager,
        spawn_env: Arc<SpawnEnvState>,
        phase: Arc<PhaseService>,
    ) -> Self {
        Self {
            manager,
            spawn_env,
            phase,
            sessions: Mutex::new(HashMap::new()),
        }
    }

    /// Start (or replace) the poller of `harness_id` on `session_id`, in the
    /// profile `profile` (`None` = the default). Needs a tokio runtime on the
    /// calling thread.
    ///
    /// # Errors
    /// No runtime, no Hermes home, or an invalid profile name.
    pub fn attach(
        &self,
        harness_id: &str,
        session_id: &str,
        profile: Option<&str>,
    ) -> Result<(), String> {
        let runtime = tokio::runtime::Handle::try_current()
            .map_err(|_| "no async runtime for the hermes work poller".to_owned())?;
        let _entered = runtime.enter();
        let mut sessions = self.sessions.lock();
        hermes_work_attach_impl(
            &self.manager,
            &self.spawn_env.snapshot(),
            harness_id.to_owned(),
            session_id.to_owned(),
            profile,
            Box::new(HermesWorkPhaseSink {
                service: Arc::clone(&self.phase),
                harness_id: harness_id.to_owned(),
            }),
        )?;
        sessions.insert(harness_id.to_owned(), session_id.to_owned());
        Ok(())
    }

    /// The `on_session_start` hook named `session_id`: attach unless that
    /// session is already being polled. The profile comes from the record.
    pub fn session_started(
        &self,
        rooms: &RoomStore,
        room_id: &str,
        harness_id: &str,
        session_id: &str,
    ) {
        // A hook can arrive after the process is gone (or the harness was
        // forgotten); a poller attached now would have nothing to stop it.
        if !self.has_live_record(harness_id) {
            tracing::debug!(
                harness = harness_id,
                "hermes work attach skipped: no live process"
            );
            return;
        }
        if self
            .sessions
            .lock()
            .get(harness_id)
            .is_some_and(|s| s == session_id)
        {
            return;
        }
        let profile = rooms
            .harness_record(room_id, harness_id)
            .and_then(|h| h.hermes_profile)
            .map(|p| p.trim().to_owned())
            .filter(|p| !p.is_empty());
        if let Err(e) = self.attach(harness_id, session_id, profile.as_deref()) {
            tracing::warn!(harness = harness_id, error = %e, "hermes work attach failed");
            return;
        }
        self.detach_if_gone(harness_id);
    }

    /// The phase record exists and the process has not exited.
    fn has_live_record(&self, harness_id: &str) -> bool {
        self.phase
            .get(harness_id)
            .is_some_and(|a| a.phase != ActivityPhase::Exited)
    }

    /// The re-check after an attach: a stop that ran between the guard and
    /// the attach has already detached, so the poller just attached would be
    /// orphaned. Detach it again if the record is gone or exited.
    pub(crate) fn detach_if_gone(&self, harness_id: &str) {
        if !self.has_live_record(harness_id) {
            tracing::debug!(
                harness = harness_id,
                "hermes work poller detached: the harness stopped during the attach"
            );
            self.detach(harness_id);
        }
    }

    /// Stop the poller. Idempotent.
    pub fn detach(&self, harness_id: &str) {
        self.manager.detach(harness_id);
        self.sessions.lock().remove(harness_id);
    }

    /// Completed poll passes of the harness's poller (tests wait on the first).
    #[cfg(test)]
    pub(crate) fn passes(&self, harness_id: &str) -> usize {
        self.manager.passes(harness_id)
    }

    #[must_use]
    pub fn is_attached(&self, harness_id: &str) -> bool {
        self.manager.is_attached(harness_id)
    }
}
