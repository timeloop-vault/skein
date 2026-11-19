//! The runner's tap into the phase machine (#732): the observer one spawned
//! process reports to.

use std::sync::Arc;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};

use parking_lot::Mutex;

use super::notices;
use crate::harness_kind::HarnessKind;
use crate::harness_phase::screen_reader::read_hermes;
use crate::harness_phase::{ClientPhaseInput, PhaseService, ScreenReading};
use crate::pty::PtyObserver;

/// Told every live chunk of a harness's PTY output, on the PTY's coalescer
/// thread (#733): prompt delivery's paste-mode and quiet tracking. Bound
/// once, after the runner exists; it must not block.
pub type OutputTap = Arc<dyn Fn(&str, &str) + Send + Sync>;

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
    tap: Arc<OnceLock<OutputTap>>,
    harness_id: String,
    room_id: String,
    retired: AtomicBool,
    kind: HarnessKind,
    /// The last screen reading handed to the phase service (frames mode,
    /// Hermes); the dedup, and whether an exit has anything to withdraw.
    last_reading: Mutex<Option<ScreenReading>>,
    /// The process has exited: no reading follows.
    exit_seen: AtomicBool,
    /// argv[0] as spawned: the name in the exit banner (#809).
    program: String,
}

impl RunnerObserver {
    pub(super) fn new(
        phase: Arc<PhaseService>,
        tap: Arc<OnceLock<OutputTap>>,
        harness_id: &str,
        room_id: &str,
        kind: HarnessKind,
        program: &str,
    ) -> Arc<Self> {
        Arc::new(Self {
            phase,
            tap,
            harness_id: harness_id.to_owned(),
            room_id: room_id.to_owned(),
            retired: AtomicBool::new(false),
            kind,
            last_reading: Mutex::new(None),
            exit_seen: AtomicBool::new(false),
            program: program.to_owned(),
        })
    }

    /// Stop forwarding. Idempotent. A reading this observer delivered is
    /// withdrawn first, so a respawn of the same harness starts with none (a
    /// retired observer's own exit is no longer forwarded). Retiring is
    /// ordered before the new observer exists and under the reading lock, so
    /// the withdrawal can never clear the new process's reading.
    pub(super) fn retire(&self) {
        let mut last = self.last_reading.lock();
        self.retired.store(true, Ordering::SeqCst);
        if last.take().is_some() {
            self.phase.client_input(
                &self.harness_id,
                ClientPhaseInput::ScreenReading { reading: None },
            );
        }
    }

    fn live(&self) -> bool {
        !self.retired.load(Ordering::SeqCst)
    }

    /// The process is gone: withdraw the reading this observer delivered (the
    /// webview sends `null` for a harness that is gone), so a respawn starts
    /// clean. Nothing was delivered in raw mode, so nothing is sent there.
    fn clear_screen_reading(&self) {
        let mut last = self.last_reading.lock();
        self.exit_seen.store(true, Ordering::SeqCst);
        if last.take().is_some() && self.live() {
            self.phase.client_input(
                &self.harness_id,
                ClientPhaseInput::ScreenReading { reading: None },
            );
        }
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
            if let Some(tap) = self.tap.get() {
                tap(&self.harness_id, chunk);
            }
        }
    }

    fn wants_screen(&self) -> bool {
        self.kind == HarnessKind::Hermes && self.live()
    }

    fn screen(&self, rows: &[String]) {
        if self.kind != HarnessKind::Hermes {
            return;
        }
        let reading = read_hermes(rows);
        // Held across the hand-off so a racing exit cannot be followed by a
        // stale reading; the service never calls back into the observer.
        let mut last = self.last_reading.lock();
        if !self.live() || self.exit_seen.load(Ordering::SeqCst) || *last == Some(reading) {
            return;
        }
        *last = Some(reading);
        self.phase.client_input(
            &self.harness_id,
            ClientPhaseInput::ScreenReading {
                reading: Some(reading),
            },
        );
    }

    fn exit_notice(&self, code: Option<u32>) -> Option<Vec<u8>> {
        Some(notices::exit_banner(&self.program, code))
    }

    fn exited(&self, code: Option<u32>) {
        self.clear_screen_reading();
        if self.live() {
            // Windows codes such as 0xC0000005 do not fit an i32; the
            // wrapped value stays non-zero (the machine only stores it).
            let code = code.map(|c| i32::from_ne_bytes(c.to_ne_bytes()));
            self.phase.exited(&self.harness_id, code);
        }
    }
}

#[cfg(test)]
mod tests;
