//! When does the next [`PhaseMachine::tick`] need to run (#761)?
//!
//! The tick used to be polled once a second whatever the state. Everything it
//! does is a function of the stored state and the clock, so the instant at
//! which it will next change anything can be computed, and a driver can sleep
//! until then. This file is that computation; it mirrors `tick` (`machine.rs`),
//! `tick_work` (`work.rs`) and the screen rule (`screen.rs`) branch for branch,
//! and `tests/deadline.rs` holds the two to each other.
//!
//! The screen rule counts samples, one per `TICK_INTERVAL_MS`, not time. To
//! keep that rule the same when `tick` runs at arbitrary instants the tick
//! steps it at most once per interval ([`PhaseMachine::tick_screen_gated`])
//! and the forecast here wakes the driver when the next step would matter.

use super::background_tasks::deadline_of;
use super::constants::{
    ADAPTER_SILENT_AFTER_MS, BACKGROUND_TASK_CEILING_MS, DELEGATION_CEILING_MS,
    DELEGATION_SETTLE_MS, IDLE_AFTER_MS, LAUNCH_SILENT_AFTER_MS, PATTERN_WAITING_AFTER_MS,
    TICK_INTERVAL_MS,
};
use super::machine::PhaseMachine;
use super::patterns::matches_waiting_prompt;
use super::screen::{Debounce, ScreenReading};
use super::types::{ActivityPhase, HarnessActivity};

/// The earliest of a set of instants.
#[derive(Default)]
struct Earliest(Option<i64>);

impl Earliest {
    fn add(&mut self, at: i64) {
        self.0 = Some(self.0.map_or(at, |cur| cur.min(at)));
    }
}

/// What one screen step would do right now (a pure mirror of
/// `tick_screen_phase`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ScreenForecast {
    /// The step's return value: the screen owns this harness, skip L2a/L2b.
    claims: bool,
    /// The step would write something: a transition, a debounce entry
    /// created, advanced, reset or removed, an unknown-tick count moved, or
    /// ownership released.
    changes: bool,
}

impl PhaseMachine {
    /// The earliest epoch-ms instant at which [`Self::tick`] would change any
    /// state, assuming no input arrives before then; `None` when nothing is
    /// pending. The minimum of [`Self::harness_deadline`] over every harness.
    ///
    /// Contract:
    ///
    /// * (S) Soundness: if this returns `None` or a value greater than `t`,
    ///   `tick(t)` for any `now_ms <= t` mutates nothing observable (no
    ///   transition, no registry change, no stamp).
    /// * (L) Liveness: if a tick at time `d` would change something and no
    ///   input arrives, this returns a value `<= d`.
    ///
    /// A returned value `<= now_ms` means "tick now". Any input (an event,
    /// output, a keystroke, a stored screen reading) can move the answer
    /// earlier, so the driver asks again after each one.
    #[must_use]
    pub fn next_deadline(&self, now_ms: i64) -> Option<i64> {
        self.order
            .iter()
            .filter_map(|id| self.harness_deadline(id, now_ms))
            .min()
    }

    /// [`Self::next_deadline`] for one harness; the same contract.
    ///
    /// Branch order is `tick`'s: the Monitor expiry, then the armed
    /// deferral (which owns the harness), then the #441 watchdog, then for an
    /// authoritative harness the two silent-adapter watchdogs, otherwise the
    /// screen rule and L2b/L2a.
    #[must_use]
    pub(super) fn harness_deadline(&self, id: &str, now_ms: i64) -> Option<i64> {
        let a = self.store.get(id)?;
        let mut d = Earliest::default();
        // `expire_due`: a Monitor ends at its own deadline.
        for e in self.background_tasks.iter(id) {
            if let Some(at) = deadline_of(e) {
                d.add(at);
            }
        }
        if let Some(armed) = a.delegation_deferred_at {
            self.deferral_deadline(id, a, armed, now_ms, &mut d);
            return d.0;
        }
        // `tick_stale_work` (#441).
        let outstanding = self.outstanding_work(id);
        if a.phase == ActivityPhase::Running && a.authoritative && outstanding > 0 {
            let newest_task = self
                .background_tasks
                .iter(id)
                .filter(|e| !e.from_attach && !e.overdue)
                .map(|e| e.started_at)
                .fold(0, i64::max);
            let last_work_at = a
                .last_turn_signal
                .map_or(a.spawned_at, |s| s.at)
                .max(a.delegation_activity_at)
                .max(newest_task);
            d.add(last_work_at.saturating_add(DELEGATION_CEILING_MS));
        }
        if a.authoritative {
            if !a.adapter_heard {
                if let Some(at) = a.prompt_submitted_at {
                    d.add(at.saturating_add(ADAPTER_SILENT_AFTER_MS));
                }
                if a.launch_signal_at.is_none() && a.phase == ActivityPhase::Spawning {
                    d.add(a.spawned_at.saturating_add(LAUNCH_SILENT_AFTER_MS));
                }
            }
            return d.0;
        }
        self.screen_and_idle_deadline(id, a, now_ms, &mut d);
        d.0
    }

    /// `tick_deferred`'s rules for an armed deferral.
    fn deferral_deadline(
        &self,
        id: &str,
        a: &HarnessActivity,
        armed: i64,
        now_ms: i64,
        d: &mut Earliest,
    ) {
        // #86: permission is the harder stop; the tick leaves it alone.
        if a.phase == ActivityPhase::Permission {
            return;
        }
        for e in self.background_tasks.iter(id) {
            if !e.from_attach && !e.overdue && deadline_of(e).is_none() {
                d.add(
                    armed
                        .max(e.started_at)
                        .saturating_add(BACKGROUND_TASK_CEILING_MS),
                );
            }
        }
        if self.subagents.working_count(id) > 0 {
            d.add(
                a.delegation_activity_at
                    .saturating_add(DELEGATION_CEILING_MS),
            );
        }
        if self.outstanding_work(id) > 0 {
            // The tick clears a stale settle mark, so a later settle always
            // starts a fresh window.
            if a.delegation_emptied_at.is_some() {
                d.add(now_ms);
            }
            return;
        }
        match a.delegation_emptied_at {
            // The tick stamps the mark, which starts the settle window.
            None => d.add(now_ms),
            Some(emptied) => d.add(emptied.saturating_add(DELEGATION_SETTLE_MS)),
        }
    }

    /// The non-authoritative tail of `tick`: the screen rule, then L2b/L2a.
    fn screen_and_idle_deadline(
        &self,
        id: &str,
        a: &HarnessActivity,
        now_ms: i64,
        d: &mut Earliest,
    ) {
        let last = self.screen.last_step.get(id).copied();
        let forecast = self.forecast_screen(id, a);
        // Between steps the tick reuses the last claim (none yet = false).
        let reused = last.is_some_and(|(_, claimed)| claimed);
        if forecast.changes || forecast.claims != reused {
            d.add(last.map_or(now_ms, |(at, _)| at.saturating_add(TICK_INTERVAL_MS)));
        }
        // While the pending step would claim the harness, a gated tick skips
        // L2a/L2b and the step itself (reported above) then claims it: no L2
        // deadline until the verdict changes.
        if reused || forecast.claims {
            return;
        }
        let Some(last_output_at) = a.last_output_at else {
            return;
        };
        if a.phase != ActivityPhase::Running {
            return;
        }
        let after = if matches_waiting_prompt(&a.tail) {
            PATTERN_WAITING_AFTER_MS
        } else {
            IDLE_AFTER_MS
        };
        d.add(last_output_at.saturating_add(after));
    }

    /// A pure mirror of `tick_screen_phase`: what a step taken now returns
    /// and whether it writes anything.
    fn forecast_screen(&self, id: &str, a: &HarnessActivity) -> ScreenForecast {
        let owns = self.is_screen_waiting(id);
        let debounce = self.screen.debounce.get(id);
        if a.phase != ActivityPhase::Running && !owns {
            return ScreenForecast {
                claims: false,
                changes: debounce.is_some(),
            };
        }
        let reading = self
            .screen
            .readings
            .get(id)
            .copied()
            .unwrap_or(ScreenReading::Unknown);
        if reading == ScreenReading::Unknown {
            if !owns {
                return ScreenForecast {
                    claims: false,
                    changes: debounce.is_some(),
                };
            }
            // Either the unknown-tick count advances or ownership is released
            // (`SCREEN_UNKNOWN_RELEASE_TICKS`); both write.
            return ScreenForecast {
                claims: true,
                changes: true,
            };
        }
        let next = Debounce::next(debounce, reading, a.phase);
        let mut changes = self.screen.unknown_ticks.contains_key(id) || debounce != Some(&next);
        if next.confirmed() {
            changes |= (a.phase == ActivityPhase::Running && reading == ScreenReading::Waiting)
                || (owns && reading == ScreenReading::Running);
        }
        ScreenForecast {
            claims: true,
            changes,
        }
    }

    /// The screen step of one tick, at most once per `TICK_INTERVAL_MS`: the
    /// rule counts samples, so a tick taken sooner than that reuses the last
    /// step's verdict instead of counting again. Returns the verdict
    /// `tick_screen_phase` gives: true = skip L2a/L2b.
    pub(super) fn tick_screen_gated(&mut self, id: &str, now_ms: i64) -> bool {
        if let Some(&(at, claimed)) = self.screen.last_step.get(id)
            && now_ms - at < TICK_INTERVAL_MS
        {
            // A reading that arrived since the last step may make the
            // pending step claim the harness: it would at the next
            // interval, before L2a/L2b could act in the old 1 Hz tick, so
            // they must not run now. (The step itself is not taken early.)
            return claimed
                || self
                    .store
                    .get(id)
                    .is_some_and(|a| self.forecast_screen(id, a).claims);
        }
        let claimed = self.tick_screen_phase(id, now_ms);
        self.screen
            .last_step
            .insert(id.to_owned(), (now_ms, claimed));
        claimed
    }
}
