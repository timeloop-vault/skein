//! Port of `app/src/supervisor/supervisorState.test.ts` (#423): the grace
//! clock, the recovery rate limit and cap, the history and the log lines.
//!
//! The TS tests drive `evaluate` with one switchable invariant; here that is
//! `evaluate_with` over an invariant whose `check` reads a thread-local flag
//! (each test runs on its own thread, so the flags never collide).

use std::cell::Cell;

use super::invariants::snap;
use crate::harness_phase::invariants::{
    Effect, HISTORY_CAP, HarnessSupervisorState, Invariant, LogLevel, RECOVERY_CAP,
    RECOVERY_MIN_INTERVAL_MS, RECOVERY_WINDOW_MS, Recovery, Severity, SupervisorCode,
    SupervisorSnapshot, Verdict, ViolationRecord, format_effect, format_recovery_outcome,
};

thread_local! {
    static ON: Cell<bool> = const { Cell::new(false) };
}

fn on(v: bool) {
    ON.set(v);
}

fn check(_: &SupervisorSnapshot, _: i64) -> Verdict {
    if ON.get() {
        Verdict::Violated {
            evidence: "k=v".to_owned(),
        }
    } else {
        Verdict::Ok
    }
}

/// `makeInv`: defaults `graceMs: 10_000`, `recovery: "set_waiting"`.
fn make_inv(grace_ms: i64, recovery: Option<Recovery>) -> Invariant {
    on(false);
    Invariant {
        code: SupervisorCode::EndedTurnNotWaiting,
        grace_ms,
        severity: Severity::Warn,
        recovery,
        check,
    }
}

fn run(state: &mut HarnessSupervisorState, inv: &Invariant, now: i64) -> Vec<Effect> {
    state.evaluate_with(std::slice::from_ref(inv), &snap(), now)
}

fn types(e: &[Effect]) -> Vec<&'static str> {
    e.iter()
        .map(|x| match x {
            Effect::Violation { .. } => "violation",
            Effect::Cleared { .. } => "cleared",
            Effect::Recover { .. } => "recover",
            Effect::RecoveryExhausted { .. } => "recovery_exhausted",
        })
        .collect()
}

const CODE: SupervisorCode = SupervisorCode::EndedTurnNotWaiting;

fn violation_effect() -> Effect {
    Effect::Violation {
        code: CODE,
        severity: Severity::Warn,
        evidence: "k=v".to_owned(),
    }
}

#[test]
fn evaluate_respects_the_grace_period_and_reports_once_per_onset() {
    let inv = make_inv(10_000, None);
    on(true);
    let mut state = HarnessSupervisorState::default();
    assert_eq!(run(&mut state, &inv, 0), vec![]);
    assert_eq!(run(&mut state, &inv, 9_999), vec![]);
    assert_eq!(run(&mut state, &inv, 10_000), vec![violation_effect()]);
    assert_eq!(run(&mut state, &inv, 20_000), vec![]);
}

#[test]
fn evaluate_emits_cleared_and_closes_the_history_entry() {
    let inv = make_inv(0, None);
    on(true);
    let mut state = HarnessSupervisorState::default();
    run(&mut state, &inv, 100);
    on(false);
    let effects = run(&mut state, &inv, 600);
    assert_eq!(
        effects,
        vec![Effect::Cleared {
            code: CODE,
            after_ms: 500
        }]
    );
    assert_eq!(
        state.history,
        vec![ViolationRecord {
            code: CODE,
            severity: Severity::Warn,
            since: 100,
            evidence: "k=v".to_owned(),
            cleared_at: Some(600),
        }]
    );
}

#[test]
fn evaluate_a_clear_before_the_grace_ends_emits_nothing_and_restarts_the_clock() {
    let inv = make_inv(10_000, None);
    on(true);
    let mut state = HarnessSupervisorState::default();
    run(&mut state, &inv, 0);
    on(false);
    assert_eq!(run(&mut state, &inv, 5_000), vec![]);
    on(true);
    run(&mut state, &inv, 6_000);
    assert_eq!(run(&mut state, &inv, 15_999), vec![]);
    let effects = run(&mut state, &inv, 16_000);
    assert_eq!(types(&effects), vec!["violation"]);
}

#[test]
fn evaluate_rate_limits_recovery_and_caps_it_with_exactly_one_exhausted() {
    let inv = make_inv(0, Some(Recovery::SetWaiting));
    on(true);
    let mut all = Vec::new();
    let mut state = HarnessSupervisorState::default();
    let mut now = 0;
    for _ in 0..12 {
        all.extend(run(&mut state, &inv, now));
        now += RECOVERY_MIN_INTERVAL_MS / 2;
    }
    assert_eq!(
        types(&all),
        vec![
            "violation",
            "recover",
            "recover",
            "recover",
            "recovery_exhausted"
        ]
    );
    assert_eq!(
        all.last().unwrap(),
        &Effect::RecoveryExhausted {
            code: CODE,
            attempts: RECOVERY_CAP
        }
    );
    assert_eq!(state.codes[&CODE].attempts, vec![0, 30_000, 60_000]);
}

#[test]
fn evaluate_attempts_persist_across_a_clear_and_re_onset() {
    let inv = make_inv(0, Some(Recovery::SetWaiting));
    let mut state = HarnessSupervisorState::default();
    let mut all = Vec::new();
    for i in 0..5 {
        on(true);
        all.extend(run(&mut state, &inv, i * 100_000));
        on(false);
        all.extend(run(&mut state, &inv, i * 100_000 + 1_000));
    }
    let count = |t: &str| types(&all).iter().filter(|x| **x == t).count();
    assert_eq!(count("recover"), RECOVERY_CAP);
    assert_eq!(count("recovery_exhausted"), 1);
    assert_eq!(count("violation"), 5);
}

#[test]
fn evaluate_forgets_attempts_after_the_window_so_a_later_recurrence_recovers_again() {
    let inv = make_inv(0, Some(Recovery::SetWaiting));
    on(true);
    let mut state = HarnessSupervisorState::default();
    let mut all = Vec::new();
    for i in 0..6 {
        all.extend(run(&mut state, &inv, i * RECOVERY_MIN_INTERVAL_MS));
    }
    assert_eq!(
        types(&all)
            .iter()
            .filter(|x| **x == "recovery_exhausted")
            .count(),
        1
    );
    let effects = run(
        &mut state,
        &inv,
        6 * RECOVERY_MIN_INTERVAL_MS + RECOVERY_WINDOW_MS,
    );
    assert_eq!(types(&effects), vec!["recover"]);
    assert!(!state.codes[&CODE].exhausted);
}

#[test]
fn evaluate_caps_history_at_50_dropping_the_oldest() {
    let inv = make_inv(0, None);
    let mut state = HarnessSupervisorState::default();
    for i in 0..i64::try_from(HISTORY_CAP + 10).unwrap() {
        on(true);
        run(&mut state, &inv, i * 10);
        on(false);
        run(&mut state, &inv, i * 10 + 5);
    }
    assert_eq!(state.history.len(), HISTORY_CAP);
    assert_eq!(state.history[0].since, 10 * 10);
}

// NOT PORTED: evaluate_does_not_mutate_its_input — `evaluate` in TS returns a
// new state and the test checks the old one is untouched (`structuredClone`
// before, `toEqual` after). The Rust `evaluate(&mut self, ..)` advances the
// state in place by design, so there is no separate input left to compare;
// the property is the signature.

#[test]
fn formatting_format_effect() {
    let rows = [
        (
            Effect::Violation {
                code: SupervisorCode::MailHeld,
                severity: Severity::Warn,
                evidence: "unread=1".to_owned(),
            },
            LogLevel::Warn,
            "[skein] supervisor harness=h1 code=mail_held violation evidence=unread=1",
        ),
        (
            Effect::Violation {
                code: SupervisorCode::AdapterWithoutAuthority,
                severity: Severity::Error,
                evidence: "x=1".to_owned(),
            },
            LogLevel::Error,
            "[skein] supervisor harness=h1 code=adapter_without_authority violation evidence=x=1",
        ),
        (
            Effect::Cleared {
                code: SupervisorCode::MailHeld,
                after_ms: 5,
            },
            LogLevel::Info,
            "[skein] supervisor harness=h1 code=mail_held cleared afterMs=5",
        ),
        (
            Effect::Recover {
                code: SupervisorCode::EndedTurnNotWaiting,
                recovery: Recovery::SetWaiting,
            },
            LogLevel::Info,
            "[skein] supervisor harness=h1 code=ended_turn_not_waiting recover recovery=set_waiting",
        ),
        (
            Effect::RecoveryExhausted {
                code: SupervisorCode::EndedTurnNotWaiting,
                attempts: 3,
            },
            LogLevel::Error,
            "[skein] supervisor harness=h1 code=ended_turn_not_waiting recovery_exhausted attempts=3",
        ),
    ];
    for (i, (effect, level, message)) in rows.into_iter().enumerate() {
        assert_eq!(
            format_effect("h1", &effect),
            (level, message.to_owned()),
            "formatEffect {i}"
        );
    }
}

#[test]
fn formatting_format_recovery_outcome() {
    let applied =
        format_recovery_outcome("h1", SupervisorCode::MailHeld, Recovery::SetWaiting, true);
    assert!(applied.1.contains("applied=true"));
    let refused =
        format_recovery_outcome("h1", SupervisorCode::MailHeld, Recovery::SetWaiting, false);
    assert_eq!(refused.0, LogLevel::Warn);
}
