//! The runner's gate on the phase inputs a client sends (#732). Two of them
//! name a harness the client may be wrong about (`StartFailed`,
//! `RestartRequested`); the runner knows the stored records and whether a
//! process is alive, so it checks both before the phase service acts.

use super::{HarnessRunner, lookup};
use crate::harness_phase::ClientPhaseInput;

impl HarnessRunner {
    /// One client-observed phase input. `StartFailed` and `RestartRequested`
    /// are dropped (debug log) unless `harness_id` is a stored harness, and
    /// run under the STORED room id, never one the client names;
    /// `StartFailed` is also dropped while the harness has a live process or
    /// a spawn is in progress. Everything else passes straight through.
    pub fn client_phase_input(&self, harness_id: &str, input: ClientPhaseInput) {
        match input {
            ClientPhaseInput::StartFailed => {
                let Some(room_id) = self.stored_room_of(harness_id) else {
                    return not_stored(harness_id, "startFailed");
                };
                // Not blocking: a held gate is a spawn in progress, which
                // makes the report stale anyway.
                let gate = self.spawn_gate(harness_id);
                if let Some(_one_at_a_time) = gate.try_lock() {
                    if self.has_live_process(harness_id) {
                        tracing::debug!(
                            target: "skein::phase",
                            harness = %harness_id,
                            "startFailed dropped: the harness has a live process"
                        );
                    } else {
                        self.phase.start_failed(harness_id, &room_id);
                    }
                } else {
                    tracing::debug!(
                        target: "skein::phase",
                        harness = %harness_id,
                        "startFailed dropped: a spawn is in progress"
                    );
                }
                self.prune_gate(harness_id, &gate);
            }
            ClientPhaseInput::RestartRequested { source } => {
                match self.stored_room_of(harness_id) {
                    Some(room_id) => self.phase.restart_requested(harness_id, &room_id, source),
                    None => not_stored(harness_id, "restartRequested"),
                }
            }
            other => self.phase.client_input(harness_id, other),
        }
    }

    /// The id of the room whose stored record holds `harness_id`, if the
    /// spawn path would accept it too: not an archived room, a PTY kind.
    fn stored_room_of(&self, harness_id: &str) -> Option<String> {
        let rooms = self.store.rooms();
        let room_id = rooms
            .iter()
            .find(|r| r.harnesses.iter().any(|h| h.id == harness_id))
            .map(|r| r.id.clone())?;
        lookup(&rooms, &room_id, harness_id).ok()?;
        Some(room_id)
    }

    /// The runner has a process for the harness and it has not exited.
    fn has_live_process(&self, harness_id: &str) -> bool {
        self.pty_id(harness_id)
            .is_some_and(|pty_id| self.pty.is_alive(&pty_id))
    }
}

fn not_stored(harness_id: &str, kind: &str) {
    tracing::debug!(
        target: "skein::phase",
        harness = %harness_id,
        "{kind} dropped: not a stored harness"
    );
}
