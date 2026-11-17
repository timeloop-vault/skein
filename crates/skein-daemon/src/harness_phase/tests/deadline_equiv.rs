//! The safety net for the deadline driver (#761): the same scripted scenario
//! run twice on fresh machines, once ticked every 10 ms of simulated time
//! and once ticked only where `next_deadline` says. Both must produce the
//! same transitions, each within 10 ms of the other.
//!
//! Screen inputs sit on whole seconds, the grid the screen rule samples on.

use skein_harness::claude::background::BackgroundKind;

use crate::harness_phase::background_tasks::BackgroundTaskInput;
use crate::harness_phase::subagents::SubagentInput;
use crate::harness_phase::{
    ActivityPhase, PhaseMachine, ScreenReading, Transition, TransitionSource,
};

const H: &str = "h";
const FINE_MS: i64 = 10;

type Input = Box<dyn Fn(&mut PhaseMachine, i64)>;

struct Scenario {
    inputs: Vec<(i64, Input)>,
    end: i64,
}

impl Scenario {
    fn new(end: i64) -> Self {
        Self {
            inputs: Vec::new(),
            end,
        }
    }

    fn at(mut self, t: i64, f: impl Fn(&mut PhaseMachine, i64) + 'static) -> Self {
        self.inputs.push((t, Box::new(f)));
        self.inputs.sort_by_key(|(t, _)| *t);
        self
    }
}

type Seen = Vec<(String, ActivityPhase, TransitionSource, i64)>;

fn collect(m: &mut PhaseMachine, out: &mut Seen) {
    for t in m.drain_transitions() {
        let Transition {
            harness_id,
            to,
            source,
            at_ms,
            ..
        } = t;
        out.push((harness_id, to, source, at_ms));
    }
}

/// Tick every 10 ms from 0, inputs applied (at their own time) before the
/// tick of the grid instant that reaches them.
fn run_fine(s: &Scenario) -> Seen {
    let mut m = PhaseMachine::new();
    let mut out = Seen::new();
    let mut idx = 0;
    let mut now = 0;
    while now <= s.end {
        while idx < s.inputs.len() && s.inputs[idx].0 <= now {
            let (t, f) = &s.inputs[idx];
            f(&mut m, *t);
            idx += 1;
        }
        m.tick(now);
        collect(&mut m, &mut out);
        now += FINE_MS;
    }
    out
}

/// Tick only at `next_deadline` instants (and "now" when it says so).
fn run_deadline(s: &Scenario) -> Seen {
    let mut m = PhaseMachine::new();
    let mut out = Seen::new();
    let mut idx = 0;
    let mut now = 0;
    let mut spins = 0;
    while now <= s.end {
        while idx < s.inputs.len() && s.inputs[idx].0 <= now {
            let (t, f) = &s.inputs[idx];
            f(&mut m, *t);
            idx += 1;
        }
        collect(&mut m, &mut out);
        let next_input = s.inputs.get(idx).map(|(t, _)| *t);
        match m.next_deadline(now) {
            Some(d) if d <= now => {
                spins += 1;
                assert!(
                    spins < 1_000,
                    "a deadline at or before now that never clears"
                );
                m.tick(now);
                collect(&mut m, &mut out);
            }
            Some(d) if next_input.is_none_or(|ni| d < ni) => {
                spins = 0;
                now = d;
                m.tick(now);
                collect(&mut m, &mut out);
            }
            _ => {
                spins = 0;
                match next_input {
                    Some(ni) => now = ni,
                    None => break,
                }
            }
        }
    }
    out
}

fn assert_equivalent(s: &Scenario) {
    let fine = run_fine(s);
    let dl = run_deadline(s);
    let shape = |v: &Seen| -> Vec<_> {
        v.iter()
            .map(|(h, p, src, _)| (h.clone(), *p, *src))
            .collect()
    };
    assert_eq!(shape(&fine), shape(&dl), "fine {fine:?}\ndeadline {dl:?}");
    for (f, d) in fine.iter().zip(&dl) {
        assert!(
            (f.3 - d.3).abs() <= FINE_MS,
            "transition {f:?} vs {d:?} differ by more than {FINE_MS} ms"
        );
    }
    assert!(!fine.is_empty(), "scenario produced no transitions");
}

fn sub(id: &str) -> SubagentInput {
    SubagentInput {
        agent_id: id.to_owned(),
        agent_type: None,
        description: None,
    }
}

fn task(id: &str, kind: BackgroundKind, timeout_ms: Option<i64>) -> BackgroundTaskInput {
    BackgroundTaskInput {
        task_id: id.to_owned(),
        kind,
        description: None,
        command: None,
        timeout_ms,
        persistent: false,
        agent_id: None,
    }
}

/// An authoritative running Claude harness (spawn at 3, work at 500).
fn claude(s: Scenario) -> Scenario {
    s.at(3, |m, t| {
        m.spawned(H, t);
        m.attach_authoritative_source(H);
    })
    .at(500, |m, t| {
        m.set_running_from_adapter(H, TransitionSource::L2c1ClaudeAssistant, false, t);
    })
}

fn end_turn(m: &mut PhaseMachine, t: i64) {
    m.awaiting_prompt_from_adapter(H, TransitionSource::L2c1ClaudeEndTurn, t);
}

#[test]
fn shell_output_then_quiet() {
    let s = Scenario::new(30_000)
        .at(3, |m, t| m.spawned(H, t))
        .at(107, |m, t| m.record_output(H, Some("compiling"), t))
        .at(12_345, |m, t| m.record_output(H, Some("more"), t));
    assert_equivalent(&s);
}

#[test]
fn prompt_looking_tail() {
    let s = Scenario::new(10_000)
        .at(3, |m, t| m.spawned(H, t))
        .at(107, |m, t| {
            m.record_output(H, Some("[sudo] password for bob: "), t);
        });
    assert_equivalent(&s);
}

#[test]
fn end_of_turn_with_a_subagent_that_then_finishes() {
    let s = claude(Scenario::new(200_000))
        .at(600, |m, t| {
            m.subagents_mut().record(H, sub("a"), false, t);
            m.note_subagent_started(H, t);
        })
        .at(1_001, end_turn)
        .at(5_003, |m, _| {
            m.subagents_mut().finish(H, "a");
        });
    assert_equivalent(&s);
}

#[test]
fn subagent_that_never_finishes() {
    let s = claude(Scenario::new(1_000_000))
        .at(600, |m, t| {
            m.subagents_mut().record(H, sub("a"), false, t);
            m.note_subagent_started(H, t);
        })
        .at(1_001, end_turn);
    assert_equivalent(&s);
}

#[test]
fn background_task_ceiling() {
    let s = claude(Scenario::new(1_000_000))
        .at(600, |m, t| {
            m.background_tasks_mut()
                .record(H, task("t", BackgroundKind::Bash, None), false, t);
        })
        .at(1_001, end_turn);
    assert_equivalent(&s);
}

#[test]
fn monitor_with_a_timeout() {
    let s = claude(Scenario::new(100_000))
        .at(600, |m, t| {
            m.background_tasks_mut().record(
                H,
                task("m", BackgroundKind::Monitor, Some(5_000)),
                false,
                t,
            );
        })
        .at(1_001, end_turn);
    assert_equivalent(&s);
}

#[test]
fn work_watchdog_without_a_deferral() {
    let s = claude(Scenario::new(1_000_000)).at(600, |m, t| {
        m.subagents_mut().record(H, sub("a"), false, t);
        m.note_subagent_started(H, t);
    });
    assert_equivalent(&s);
}

#[test]
fn launch_silent() {
    let s = Scenario::new(30_000).at(3, |m, t| {
        m.spawned(H, t);
        m.attach_authoritative_source(H);
    });
    assert_equivalent(&s);
}

#[test]
fn adapter_silent_after_a_submitted_prompt() {
    let s = Scenario::new(40_000)
        .at(3, |m, t| {
            m.spawned(H, t);
            m.attach_authoritative_source(H);
        })
        .at(500, |m, t| {
            m.note_launch_signal(H, TransitionSource::L2c1ClaudeSessionStart, t);
        })
        .at(1_007, |m, t| m.record_input(H, "\r", false, t))
        // The degrade moves no phase; output afterwards shows L2a took over.
        .at(15_000, |m, t| m.record_output(H, Some("redraw"), t));
    assert_equivalent(&s);
}

#[test]
fn hermes_screen_sequence_and_unknown_release() {
    let s = Scenario::new(30_000)
        .at(0, |m, t| {
            m.spawned(H, t);
            m.record_output(H, Some("banner"), t);
            m.set_screen_reading(H, Some(ScreenReading::Running));
        })
        .at(2_000, |m, _| {
            m.set_screen_reading(H, Some(ScreenReading::Waiting));
        })
        .at(9_000, |m, _| {
            m.set_screen_reading(H, Some(ScreenReading::Unknown));
        });
    assert_equivalent(&s);
}

#[test]
fn permission_dialog_opened_during_a_deferral() {
    let s = claude(Scenario::new(1_200_000))
        .at(600, |m, t| {
            m.subagents_mut().record(H, sub("a"), false, t);
            m.note_subagent_started(H, t);
        })
        .at(1_001, end_turn)
        .at(2_000, |m, t| {
            m.set_permission_from_adapter(
                H,
                TransitionSource::L2c1ClaudePermission,
                Some("Bash"),
                None,
                None,
                t,
            );
        })
        // Answered long after the ceiling passed under the dialog.
        .at(1_000_000, |m, t| {
            m.clear_permission(H, TransitionSource::L2c1ClaudeAssistant, None, t);
        });
    assert_equivalent(&s);
}
