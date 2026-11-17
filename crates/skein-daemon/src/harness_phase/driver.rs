//! The two threads that run [`PhaseService`] in the background (#761).
//!
//! * `skein-phase-deadline` sleeps until the machine's earliest deadline
//!   ([`PhaseMachine::next_deadline`]) and ticks then. Nothing ticks on a
//!   timer: with no deadline pending it waits for an input.
//! * `skein-phase-supervisor` runs the supervisor every
//!   [`SUPERVISE_INTERVAL_MS`], with the facts only the caller can gather.
//!
//! Both hold only a `Weak<PhaseService>` and wait on [`Wake`], which the
//! service owns a second `Arc` of. Dropping the service closes it, so both
//! threads end at once; neither polls to find out.
//!
//! The wake rule: the driver records, in [`Inner::driver_armed`] and under the
//! service lock, the instant it sleeps until. An input that makes an earlier
//! deadline pokes it (see [`PhaseService::with_for`]). A deadline that moves
//! later needs no wake: the driver wakes at the stale instant, finds nothing
//! due and arms again.
//!
//! [`SUPERVISE_INTERVAL_MS`]: super::constants::SUPERVISE_INTERVAL_MS
//! [`PhaseMachine::next_deadline`]: super::PhaseMachine::next_deadline

use std::panic::AssertUnwindSafe;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError, Weak};
use std::thread::JoinHandle;
use std::time::Duration;

use super::SupervisorFacts;
use super::constants::SUPERVISE_INTERVAL_MS;
use super::service::{Inner, PhaseService};

/// [`Inner::driver_armed`] while the driver is not asleep: no wake needed,
/// because it recomputes before it sleeps again.
pub(super) const NOT_SLEEPING: i64 = i64::MIN;
/// [`Inner::driver_armed`] while the driver sleeps with no deadline pending:
/// any deadline is earlier.
pub(super) const SLEEPING_INDEFINITELY: i64 = i64::MAX;

/// A sleep towards a deadline is cut to this, then the driver recomputes (no
/// tick unless something is due). Monotonic clocks may not count time the
/// machine spent suspended, so a long sleep could otherwise run that late.
const MAX_SLEEP_MS: i64 = 30_000;
/// Consecutive due passes allowed before a deadline that never clears is
/// treated as a bug.
const MAX_BACK_TO_BACK: u32 = 3;
/// How long the driver backs off for after that, and after a panic.
const BACKOFF_MS: i64 = 1_000;

#[derive(Default)]
struct WakeState {
    poked: bool,
    closed: bool,
}

/// What the threads sleep on. Locked after the service lock, never before.
#[derive(Default)]
pub(super) struct Wake {
    state: Mutex<WakeState>,
    driver: Condvar,
    supervisor: Condvar,
}

impl Wake {
    fn lock(&self) -> MutexGuard<'_, WakeState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub(super) fn poke(&self) {
        self.lock().poked = true;
        self.driver.notify_one();
    }

    fn clear_poke(&self) {
        self.lock().poked = false;
    }

    /// The service is gone: end both threads.
    pub(super) fn close(&self) {
        self.lock().closed = true;
        self.driver.notify_all();
        self.supervisor.notify_all();
    }

    /// Wait for a poke, the close or `timeout` (`None` = no timeout).
    /// Returns whether the service is closed.
    fn wait_driver(&self, timeout: Option<Duration>) -> bool {
        let g = self.lock();
        let g = match timeout {
            None => self
                .driver
                .wait_while(g, |s| !s.poked && !s.closed)
                .unwrap_or_else(PoisonError::into_inner),
            Some(t) => {
                self.driver
                    .wait_timeout_while(g, t, |s| !s.poked && !s.closed)
                    .unwrap_or_else(PoisonError::into_inner)
                    .0
            }
        };
        g.closed
    }

    /// Wait `timeout`, or until the close. Returns whether it is closed.
    fn wait_supervisor(&self, timeout: Duration) -> bool {
        let g = self.lock();
        self.supervisor
            .wait_timeout_while(g, timeout, |s| !s.closed)
            .unwrap_or_else(PoisonError::into_inner)
            .0
            .closed
    }
}

/// What one driver pass did.
enum Step {
    /// Ticked; plan again at once.
    Ran,
    /// Nothing is due: wait this long (`None` = for a poke), then plan again.
    Sleep(Option<Duration>),
}

/// The driver's guard against a deadline that never clears.
#[derive(Default)]
struct Spin {
    due_in_a_row: u32,
    logged: bool,
}

fn millis(ms: i64) -> Duration {
    Duration::from_millis(u64::try_from(ms).unwrap_or(1).max(1))
}

impl PhaseService {
    /// The earliest deadline of the whole machine, counted for the tests that
    /// pin where a scan happens.
    #[cfg_attr(not(test), allow(clippy::unused_self))] // the counter is test-only
    pub(super) fn scan_deadline(&self, g: &Inner, now: i64) -> Option<i64> {
        #[cfg(test)]
        self.deadline_scans
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        g.machine.next_deadline(now)
    }

    /// Whether the driver is asleep, for tests that must not race its start.
    #[cfg(test)]
    pub(super) fn driver_is_asleep(&self) -> bool {
        self.lock().driver_armed != NOT_SLEEPING
    }

    /// Poke the driver when `deadline` is earlier than the instant it sleeps
    /// until. Under the service lock.
    pub(super) fn wake_if_earlier(&self, g: &mut Inner, deadline: Option<i64>) {
        if let Some(d) = deadline
            && d < g.driver_armed
        {
            g.driver_armed = d;
            self.wake.poke();
        }
    }

    /// Spawn `skein-phase-deadline`. A std thread, not a task: the PTYs
    /// already use them and the daemon must not need a tokio runtime for this.
    /// `None` if the OS refused the thread (logged; the machine then never
    /// ticks).
    #[must_use]
    pub fn start_deadline_driver(self: &Arc<Self>) -> Option<JoinHandle<()>> {
        let weak: Weak<Self> = Arc::downgrade(self);
        let wake = Arc::clone(&self.wake);
        std::thread::Builder::new()
            .name("skein-phase-deadline".to_owned())
            .spawn(move || {
                let mut spin = Spin::default();
                loop {
                    let Some(svc) = weak.upgrade() else {
                        break;
                    };
                    // A panic must not end phase tracking for good: log it,
                    // back off so it cannot spin, and plan again.
                    let step = std::panic::catch_unwind(AssertUnwindSafe(|| svc.drive(&mut spin)));
                    // Never wait holding the service alive.
                    drop(svc);
                    let sleep = match step {
                        Ok(Step::Ran) => continue,
                        Ok(Step::Sleep(t)) => t,
                        Err(_) => {
                            tracing::error!(target: "skein::phase", "phase tick panicked; continuing");
                            Some(millis(BACKOFF_MS))
                        }
                    };
                    if wake.wait_driver(sleep) {
                        break;
                    }
                }
            })
            .map_err(|e| tracing::error!("phase deadline driver: cannot start thread: {e}"))
            .ok()
    }

    /// One planning pass: under the lock, find the earliest deadline. Due:
    /// run the tick. Otherwise record when the driver sleeps until.
    fn drive(&self, spin: &mut Spin) -> Step {
        let now = (self.clock)();
        {
            let mut g = self.lock();
            g.driver_armed = NOT_SLEEPING;
            self.wake.clear_poke();
            let due = match self.scan_deadline(&g, now) {
                None => {
                    g.driver_armed = SLEEPING_INDEFINITELY;
                    spin.due_in_a_row = 0;
                    return Step::Sleep(None);
                }
                Some(d) if d > now => {
                    g.driver_armed = d;
                    spin.due_in_a_row = 0;
                    return Step::Sleep(Some(millis((d - now).min(MAX_SLEEP_MS))));
                }
                Some(d) => d,
            };
            spin.due_in_a_row += 1;
            if spin.due_in_a_row > MAX_BACK_TO_BACK {
                if !spin.logged {
                    spin.logged = true;
                    tracing::error!(
                        target: "skein::phase",
                        due,
                        now,
                        "a phase tick left its deadline due; backing off"
                    );
                }
                spin.due_in_a_row = 0;
                g.driver_armed = now.saturating_add(BACKOFF_MS);
                return Step::Sleep(Some(millis(BACKOFF_MS)));
            }
        }
        #[cfg(test)]
        self.driver_ticks
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        // Whole-machine pass on a wake, so every harness is examined for the
        // stream (a registry change the tick made included).
        self.with(|s, now| {
            s.sweep = true;
            s.machine.tick(now);
        });
        Step::Ran
    }

    /// Spawn `skein-phase-supervisor`, every [`SUPERVISE_INTERVAL_MS`].
    /// `facts` is gathered outside the lock.
    #[must_use]
    pub fn start_supervisor(
        self: &Arc<Self>,
        facts: Arc<dyn Fn() -> SupervisorFacts + Send + Sync>,
    ) -> Option<JoinHandle<()>> {
        let ms = u64::try_from(SUPERVISE_INTERVAL_MS).unwrap_or(5_000);
        self.start_supervisor_every(Duration::from_millis(ms), facts)
    }

    pub(super) fn start_supervisor_every(
        self: &Arc<Self>,
        every: Duration,
        facts: Arc<dyn Fn() -> SupervisorFacts + Send + Sync>,
    ) -> Option<JoinHandle<()>> {
        let weak: Weak<Self> = Arc::downgrade(self);
        let wake = Arc::clone(&self.wake);
        std::thread::Builder::new()
            .name("skein-phase-supervisor".to_owned())
            .spawn(move || {
                while !wake.wait_supervisor(every) {
                    let Some(svc) = weak.upgrade() else {
                        break;
                    };
                    // A panic (a bad facts closure) must not end the pass
                    // for good: log it and keep going.
                    let step = std::panic::catch_unwind(AssertUnwindSafe(|| {
                        let f = facts();
                        svc.supervise_once(&f);
                    }));
                    if step.is_err() {
                        tracing::error!(target: "skein::phase", "phase supervisor panicked; continuing");
                    }
                }
            })
            .map_err(|e| tracing::error!("phase supervisor: cannot start thread: {e}"))
            .ok()
    }

    /// [`Self::start_supervisor_every`] under the name the earlier tests use.
    #[cfg(test)]
    pub(super) fn start_ticker_every(
        self: &Arc<Self>,
        every: Duration,
        facts: Arc<dyn Fn() -> SupervisorFacts + Send + Sync>,
    ) -> Option<JoinHandle<()>> {
        self.start_supervisor_every(every, facts)
    }

    /// The supervisor pass: evaluate the invariants and apply the two
    /// repairs. A repair can create a deadline; `with` wakes the driver for it.
    pub fn supervise_once(&self, facts: &SupervisorFacts) {
        self.with(|s, now| {
            // The repairs write records of any harness: look at all of them.
            s.sweep = true;
            for v in s.machine.supervise(facts, now) {
                tracing::debug!(target: "skein::supervisor", violation = ?v, "supervisor");
            }
        });
    }
}

impl Drop for PhaseService {
    fn drop(&mut self) {
        self.wake.close();
    }
}
