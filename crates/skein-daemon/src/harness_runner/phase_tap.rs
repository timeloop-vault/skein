//! The runner's tap into the phase machine (#732): the observer one spawned
//! process reports to.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::harness_phase::PhaseService;
use crate::pty::PtyObserver;

/// Feeds one process of one harness into the [`PhaseService`].
///
/// Bound to a single spawn: once the runner stops that process (or replaces
/// it) it [`retire`](Self::retire)s the observer, so the old process's late
/// output and its exit, which arrive after the new record exists, can never
/// land on the new one. A natural exit is not a retirement: the harness keeps
/// its entry and its phase reads `exited`.
///
/// Lock-free on the retire check, so it is safe from the PTY threads whatever
/// the runner is doing; the service has its own mutex and calls back into
/// nothing.
pub(super) struct RunnerObserver {
    phase: Arc<PhaseService>,
    harness_id: String,
    room_id: String,
    retired: AtomicBool,
}

impl RunnerObserver {
    pub(super) fn new(phase: Arc<PhaseService>, harness_id: &str, room_id: &str) -> Arc<Self> {
        Arc::new(Self {
            phase,
            harness_id: harness_id.to_owned(),
            room_id: room_id.to_owned(),
            retired: AtomicBool::new(false),
        })
    }

    /// Stop forwarding. Idempotent.
    pub(super) fn retire(&self) {
        self.retired.store(true, Ordering::SeqCst);
    }

    fn live(&self) -> bool {
        !self.retired.load(Ordering::SeqCst)
    }
}

impl PtyObserver for RunnerObserver {
    fn started(&self, injected: bool) {
        if self.live() {
            self.phase
                .spawned(&self.harness_id, &self.room_id, injected);
        }
    }

    fn output(&self, chunk: &str) {
        if self.live() {
            self.phase.record_output(&self.harness_id, chunk);
        }
    }

    fn exited(&self, code: Option<u32>) {
        if self.live() {
            // Windows codes such as 0xC0000005 do not fit an i32; the
            // wrapped value stays non-zero (the machine only stores it).
            let code = code.map(|c| i32::from_ne_bytes(c.to_ne_bytes()));
            self.phase.exited(&self.harness_id, code);
        }
    }
}
