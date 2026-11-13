//! The activity tick's screen-phase rule (#623). Port of
//! `app/src/screenPhaseTick.ts`.
//!
//! A TUI that never goes byte-quiet (Hermes redraws a clock and a spinner
//! constantly) can't reach L2a idle or an L2b tail match, so its running <->
//! waiting comes from the rendered screen.
//!
//! In TS the tick calls an injected reader that reads the xterm buffer in the
//! webview. The daemon has no terminal emulator (#584), so the reading is a
//! client-to-daemon input instead: [`PhaseMachine::set_screen_reading`] stores
//! the client's latest reading per harness and the tick uses the stored value
//! exactly where TS calls the reader (one "read" per tick, the value is not
//! consumed). `None` is "no client is reporting".
//!
//! The 500 ms Hermes launch poll (`harnessEventsHermes.ts`) does not call into
//! `screenPhaseTick.ts` (it calls `readScreenPhaseNow` on the input registry
//! directly), so it is not part of this port.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use super::machine::PhaseMachine;
use super::types::{ActivityPhase, TransitionSource};

/// Consecutive identical readings needed before a transition: a torn
/// `ConPTY` frame must not flip the phase.
const CONFIRM_READS: u32 = 2;

/// A screen-owned waiting whose screen then stops being recognised (an
/// overlay, a picker, a resize) for this many ticks in a row is released to
/// `running`, so the ordinary L2a/L2b rules can take over. Without it the
/// phase would stay stuck at waiting while the agent works.
pub const SCREEN_UNKNOWN_RELEASE_TICKS: u32 = 5;

/// What the rendered screen says about the harness's turn (`ScreenPhase` in
/// `screenPhase.ts`): `waiting` for the input prompt, `running` while a turn
/// is in flight, `unknown` when the screen was not positively recognised and
/// the caller falls back to the byte-stream heuristics.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ScreenReading {
    Waiting,
    Running,
    Unknown,
}

#[derive(Debug, Clone, Copy)]
struct Debounce {
    reading: ScreenReading,
    count: u32,
    /// The activity phase this count was taken in; a change resets it.
    phase: ActivityPhase,
}

/// The module-level maps of `screenPhaseTick.ts` plus the stored readings.
#[derive(Debug, Default)]
pub(super) struct ScreenState {
    /// The client's latest reading per harness (the injected reader's value).
    readings: HashMap<String, ScreenReading>,
    debounce: HashMap<String, Debounce>,
    /// `phase_since` of the waiting the screen rule set, per harness.
    waiting_since: HashMap<String, i64>,
    /// Consecutive unrecognised ticks while the screen owns a waiting.
    unknown_ticks: HashMap<String, u32>,
}

impl ScreenState {
    /// `forgetScreenPhase`'s extra duty here: the stored reading goes too.
    pub(super) fn forget_reading(&mut self, id: &str) {
        self.readings.remove(id);
    }
}

impl PhaseMachine {
    /// Store the client's latest screen reading for `id`; `None` withdraws it
    /// (no client is reporting). Read by the next [`Self::tick`] only; it
    /// changes no phase by itself.
    pub fn set_screen_reading(&mut self, id: &str, reading: Option<ScreenReading>) {
        match reading {
            Some(r) => {
                self.screen.readings.insert(id.to_owned(), r);
            }
            None => self.screen.forget_reading(id),
        }
    }

    /// Is `id` in a `waiting` that the screen rule set (and still is that
    /// same waiting)? Output must not undo it; only the tick may.
    pub(super) fn is_screen_waiting(&self, id: &str) -> bool {
        self.get(id).is_some_and(|a| {
            a.phase == ActivityPhase::Waiting
                && self.screen.waiting_since.get(id) == Some(&a.phase_since)
        })
    }

    /// Drop the screen's claim on `id`'s waiting and its debounce state
    /// (`clearScreenOwnership`). Also what `forget` calls (TS
    /// `forgetScreenPhase` is the same function); `forget` additionally drops
    /// the stored reading, which TS keeps in the registry.
    pub(super) fn clear_screen_ownership(&mut self, id: &str) {
        self.screen.debounce.remove(id);
        self.screen.waiting_since.remove(id);
        self.screen.unknown_ticks.remove(id);
    }

    #[cfg(test)]
    pub(super) fn has_screen_state(&self, id: &str) -> bool {
        self.screen.readings.contains_key(id)
            || self.screen.debounce.contains_key(id)
            || self.screen.waiting_since.contains_key(id)
            || self.screen.unknown_ticks.contains_key(id)
    }

    /// `forgetScreenPhase`: ownership plus the stored reading.
    pub(super) fn forget_screen_phase(&mut self, id: &str) {
        self.clear_screen_ownership(id);
        self.screen.forget_reading(id);
    }

    /// One non-authoritative harness's tick (`tickScreenPhase`). Returns true
    /// when the screen gave a positive reading and the caller should skip
    /// L2a/L2b for this tick.
    ///
    /// TS returns false at once when no reader is registered, but the app
    /// registers one at module load and it answers `unknown` for anything it
    /// cannot read, so "no client reporting" (`None`) is handled as that
    /// `unknown` reading.
    pub(super) fn tick_screen_phase(&mut self, id: &str, now_ms: i64) -> bool {
        let Some(a_phase) = self.get(id).map(|a| a.phase) else {
            return false;
        };
        let screen_owns_waiting = self.is_screen_waiting(id);
        if a_phase != ActivityPhase::Running && !screen_owns_waiting {
            self.screen.debounce.remove(id);
            return false;
        }
        let reading = self
            .screen
            .readings
            .get(id)
            .copied()
            .unwrap_or(ScreenReading::Unknown);
        if reading == ScreenReading::Unknown {
            self.screen.debounce.remove(id);
            if !screen_owns_waiting {
                return false;
            }
            let n = self.screen.unknown_ticks.get(id).copied().unwrap_or(0) + 1;
            if n < SCREEN_UNKNOWN_RELEASE_TICKS {
                self.screen.unknown_ticks.insert(id.to_owned(), n);
                return true;
            }
            self.clear_screen_ownership(id);
            self.set_phase(
                id,
                ActivityPhase::Running,
                TransitionSource::L2bScreenCleared,
                now_ms,
            );
            return true;
        }
        self.screen.unknown_ticks.remove(id);
        let count = match self.screen.debounce.get(id) {
            Some(prev) if prev.reading == reading && prev.phase == a_phase => prev.count + 1,
            _ => 1,
        };
        self.screen.debounce.insert(
            id.to_owned(),
            Debounce {
                reading,
                count,
                phase: a_phase,
            },
        );
        if count < CONFIRM_READS {
            return true;
        }
        if a_phase == ActivityPhase::Running && reading == ScreenReading::Waiting {
            self.set_phase(
                id,
                ActivityPhase::Waiting,
                TransitionSource::L2bScreen,
                now_ms,
            );
            // `set_phase` replaces the record in TS, so re-read it for the
            // new phase_since.
            if let Some(next) = self.get(id).filter(|n| n.phase == ActivityPhase::Waiting) {
                let since = next.phase_since;
                self.screen.waiting_since.insert(id.to_owned(), since);
            }
            self.screen.debounce.remove(id);
        } else if screen_owns_waiting && reading == ScreenReading::Running {
            self.set_phase(
                id,
                ActivityPhase::Running,
                TransitionSource::L2bScreenCleared,
                now_ms,
            );
            self.clear_screen_ownership(id);
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn two_confirming_reads_move_running_to_waiting() {
        let mut m = PhaseMachine::new();
        m.spawned("h", 0);
        m.record_output("h", Some("hi"), 100);
        assert_eq!(m.get("h").unwrap().phase, ActivityPhase::Running);
        m.set_screen_reading("h", Some(ScreenReading::Waiting));
        m.tick(1_000);
        assert_eq!(m.get("h").unwrap().phase, ActivityPhase::Running);
        m.tick(2_000);
        assert_eq!(m.get("h").unwrap().phase, ActivityPhase::Waiting);
        let last = m.drain_transitions().pop().unwrap();
        assert_eq!(last.source, TransitionSource::L2bScreen);
        assert!(m.is_screen_waiting("h"));
        m.forget("h");
        assert!(!m.is_screen_waiting("h"));
    }
}
