//! The runner's side of the client's event channels: the phase sinks the app
//! tees beside its own, and what happens to the adapters when a client's
//! channel goes away.

use super::HarnessRunner;
use super::gates::lookup;
use crate::harness_phase::sinks::{ClaudePhaseSink, HermesWorkPhaseSink, OpencodePhaseSink};

impl HarnessRunner {
    /// The sink to put beside the client's own in `Tee::new(client, this)`
    /// when the client attaches its event channel: the new attach replaces
    /// the daemon's adapter and its sink with it (#732).
    #[must_use]
    pub fn claude_phase_sink(&self, harness_id: &str) -> ClaudePhaseSink {
        ClaudePhaseSink {
            service: std::sync::Arc::clone(&self.phase),
            harness_id: harness_id.to_owned(),
        }
    }

    /// [`claude_phase_sink`](Self::claude_phase_sink) for opencode.
    #[must_use]
    pub fn opencode_phase_sink(&self, harness_id: &str) -> OpencodePhaseSink {
        OpencodePhaseSink {
            service: std::sync::Arc::clone(&self.phase),
            harness_id: harness_id.to_owned(),
        }
    }

    /// [`claude_phase_sink`](Self::claude_phase_sink) for the Hermes work
    /// poller, which only the client attaches.
    #[must_use]
    pub fn hermes_work_phase_sink(&self, harness_id: &str) -> HermesWorkPhaseSink {
        HermesWorkPhaseSink {
            service: std::sync::Arc::clone(&self.phase),
            harness_id: harness_id.to_owned(),
        }
    }

    /// A client that attached its own event channel through
    /// `claude_events_attach` / `opencode_events_attach` is going away. If the
    /// runner still has the harness its own no-op adapter replaces the
    /// client's, so action rows keep being written; otherwise the adapters
    /// are detached. Blocking, like `ensure_running`.
    pub fn release_events(&self, harness_id: &str) {
        // Under the gate, so a stop sent with the client's detach cannot
        // land between the entry check and the attach.
        let gate = self.spawn_gate(harness_id);
        {
            let _one_at_a_time = gate.lock();
            self.release_events_locked(harness_id);
        }
        self.prune_gate(harness_id, &gate);
    }

    fn release_events_locked(&self, harness_id: &str) {
        let Some(run) = self.entries.lock().get(harness_id).cloned() else {
            self.adapters.detach_for_good(harness_id);
            return;
        };
        // An exited process has nothing to tail.
        if !self.pty.is_alive(&run.pty_id) {
            self.adapters.detach_for_good(harness_id);
            return;
        }
        self.pause_point("release_events");
        let rooms = self.store.rooms();
        let session = lookup(&rooms, &run.room_id, harness_id)
            .ok()
            .and_then(|(_, h, _)| h.session_id.clone());
        // Known limit: a v1 opencode uses the port of the spawn-time argv. If
        // the client re-pointed its adapter after an opencode restarted from
        // the post-exit shell (#517), the runner never learns the new port and
        // this re-attach tails the old one; the client re-points again on its
        // next attach.
        self.attach_adapters(harness_id, &run, session.as_deref(), false);
        // `reconcile` takes no gate and may have stopped it meanwhile.
        if !self.entries.lock().contains_key(harness_id) {
            self.adapters.detach_for_good(harness_id);
        }
    }
}
