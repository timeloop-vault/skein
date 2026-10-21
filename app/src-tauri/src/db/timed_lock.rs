//! A `parking_lot::Mutex` that names its slow users (#599).
//!
//! Every database call funnels through one connection lock, so a stall
//! anywhere shows up as a wait everywhere. `lock()` records who asked
//! (`#[track_caller]`, so the caller is the `db/<file>.rs:<line>` of the
//! `Database` method that locked, not the Tauri command) and how long it
//! waited; dropping the guard records how long it was held. Either over
//! [`SLOW_LOCK_THRESHOLD`] logs a WARN on target `perf`, always after the
//! lock is released; below it nothing is logged and the cost is two
//! `Instant::now()` calls.

use std::ops::{Deref, DerefMut};
use std::panic::Location;
use std::time::{Duration, Instant};

use parking_lot::{Mutex, MutexGuard};

/// A wait or hold at or above this is logged.
pub(super) const SLOW_LOCK_THRESHOLD: Duration = Duration::from_millis(50);

pub(super) fn is_slow(d: Duration, threshold: Duration) -> bool {
    d >= threshold
}

fn millis(d: Duration) -> u64 {
    u64::try_from(d.as_millis()).unwrap_or(u64::MAX)
}

pub(super) struct TimedMutex<T>(Mutex<T>);

impl<T> TimedMutex<T> {
    pub(super) const fn new(value: T) -> Self {
        Self(Mutex::new(value))
    }

    #[track_caller]
    pub(super) fn lock(&self) -> TimedGuard<'_, T> {
        let caller = Location::caller();
        let start = Instant::now();
        let guard = self.0.lock();
        let acquired = Instant::now();
        TimedGuard {
            guard: Some(guard),
            caller,
            acquired,
            waited: acquired - start,
        }
    }
}

pub(super) struct TimedGuard<'a, T> {
    /// `None` only once released in `finish`, i.e. while dropping.
    guard: Option<MutexGuard<'a, T>>,
    caller: &'static Location<'static>,
    acquired: Instant,
    waited: Duration,
}

impl<T> TimedGuard<'_, T> {
    /// Release the lock, then report. Nothing is logged under the lock.
    fn finish(&mut self, threshold: Duration, mut report: impl FnMut(&'static str, Duration)) {
        let Some(guard) = self.guard.take() else {
            return;
        };
        drop(guard);
        let held = self.acquired.elapsed();
        if is_slow(self.waited, threshold) {
            report("db lock wait slow", self.waited);
        }
        if is_slow(held, threshold) {
            report("db lock held long", held);
        }
    }
}

#[cfg(test)]
impl<T> TimedGuard<'_, T> {
    fn caller(&self) -> &'static Location<'static> {
        self.caller
    }

    fn waited(&self) -> Duration {
        self.waited
    }

    fn held_so_far(&self) -> Duration {
        self.acquired.elapsed()
    }
}

impl<T> Deref for TimedGuard<'_, T> {
    type Target = T;
    fn deref(&self) -> &T {
        self.guard.as_ref().expect("guard live until drop")
    }
}

impl<T> DerefMut for TimedGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        self.guard.as_mut().expect("guard live until drop")
    }
}

impl<T> Drop for TimedGuard<'_, T> {
    fn drop(&mut self) {
        let caller = self.caller;
        self.finish(SLOW_LOCK_THRESHOLD, |msg, d| {
            if msg == "db lock wait slow" {
                tracing::warn!(target: "perf", wait_ms = millis(d), caller = %caller, "db lock wait slow");
            } else {
                tracing::warn!(target: "perf", hold_ms = millis(d), caller = %caller, "db lock held long");
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn threshold_is_inclusive() {
        let t = Duration::from_millis(50);
        assert!(!is_slow(Duration::from_millis(49), t));
        assert!(is_slow(t, t));
        assert!(is_slow(Duration::from_millis(500), t));
    }

    #[test]
    fn derefs_and_names_the_call_site() {
        let m = TimedMutex::new(1_u32);
        let mut g = m.lock();
        *g += 1;
        assert_eq!(*g, 2);
        assert!(g.caller().file().ends_with("timed_lock.rs"));
    }

    #[test]
    fn measures_hold() {
        let m = TimedMutex::new(());
        let g = m.lock();
        std::thread::sleep(Duration::from_millis(5));
        assert!(g.held_so_far() >= Duration::from_millis(5));
    }

    #[test]
    fn measures_wait() {
        let m = Arc::new(TimedMutex::new(()));
        let g = m.lock();
        let m2 = Arc::clone(&m);
        let h = std::thread::spawn(move || m2.lock().waited());
        std::thread::sleep(Duration::from_millis(30));
        drop(g);
        assert!(h.join().unwrap() >= Duration::from_millis(20));
    }

    #[test]
    fn reports_only_after_the_lock_is_free() {
        let m = TimedMutex::new(());
        let mut g = m.lock();
        let mut reports = 0;
        g.finish(Duration::ZERO, |_, _| {
            reports += 1;
            assert!(m.0.try_lock().is_some(), "logged while still locked");
        });
        assert_eq!(reports, 2, "one slow wait and one slow hold");
    }
}
