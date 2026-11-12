//! Shared rig for the ported vitest tables: a [`PhaseMachine`] plus a fake
//! clock that behaves like `vi.useFakeTimers()` over the TS store's one
//! `setInterval`.
//!
//! In TS the 1 s tick starts at the first `spawned` and fires at
//! `start + k * TICK_INTERVAL_MS`; `vi.advanceTimersByTime(ms)` fires every
//! tick that falls inside the advance, in order. [`Rig::advance`] does the
//! same, so a ported table reads line for line like its TS original.

use super::PhaseMachine;
use super::constants::TICK_INTERVAL_MS;

/// The epoch the fake clock starts at. Non-zero so a `0` timestamp in a
/// record is distinguishable from "now".
pub const T0: i64 = 1_000_000;

pub struct Rig {
    pub m: PhaseMachine,
    pub now: i64,
    /// When the next tick fires; `None` until the first `spawned`.
    next_tick: Option<i64>,
}

impl Rig {
    pub fn new() -> Self {
        Self {
            m: PhaseMachine::new(),
            now: T0,
            next_tick: None,
        }
    }

    /// `harnessActivity.spawned(id)`: also starts the tick, as TS
    /// `ensureTick` does on the first spawn.
    pub fn spawned(&mut self, id: &str) {
        self.m.spawned(id, self.now);
        self.next_tick.get_or_insert(self.now + TICK_INTERVAL_MS);
    }

    /// `vi.advanceTimersByTime(ms)`: move the clock, firing each due tick
    /// at its own time.
    pub fn advance(&mut self, ms: i64) {
        let end = self.now + ms;
        while let Some(at) = self.next_tick {
            if at > end {
                break;
            }
            self.now = at;
            self.m.tick(at);
            self.next_tick = Some(at + TICK_INTERVAL_MS);
        }
        self.now = end;
    }

    /// [`Self::advance`] with the supervisor running: the TS supervisor is an
    /// `onTick` listener, so `supervise(facts, at)` follows each tick. Returns
    /// every line it reported, in order.
    pub fn advance_supervised(
        &mut self,
        ms: i64,
        facts: &super::SupervisorFacts,
    ) -> Vec<super::Violation> {
        let end = self.now + ms;
        let mut out = Vec::new();
        while let Some(at) = self.next_tick {
            if at > end {
                break;
            }
            self.now = at;
            self.m.tick(at);
            out.extend(self.m.supervise(facts, at));
            self.next_tick = Some(at + TICK_INTERVAL_MS);
        }
        self.now = end;
        out
    }

    /// `vi.setSystemTime(now + ms)`: move the clock without firing ticks.
    pub fn skip(&mut self, ms: i64) {
        self.now += ms;
        if let Some(at) = self.next_tick
            && at <= self.now
        {
            // A real interval would not fire retroactively either.
            let missed = (self.now - at) / TICK_INTERVAL_MS + 1;
            self.next_tick = Some(at + missed * TICK_INTERVAL_MS);
        }
    }
}
