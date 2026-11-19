//! The observer's screen reading (#809): Hermes only, deduplicated, withdrawn
//! on exit.

use std::sync::Arc;

use super::*;

const READY: &str = " ─ ready │ haiku 4.5 │ 14.4k/200k │ [█░░░░░░░░░] 7% │ 3m 22s ";
const BUSY: &str = " ─ (¬_¬) pondering… · 35s │ haiku 4.5 │ 13.9k/200k │ [█░░░░░░░░░] 7% ";

fn rows(status: &str) -> Vec<String> {
    let mut v = vec![String::new(); 23];
    v.push(status.to_owned());
    v
}

fn observer(kind: HarnessKind) -> (Arc<PhaseService>, Arc<RunnerObserver>) {
    let phase = PhaseService::test_new();
    let obs = RunnerObserver::new(Arc::clone(&phase), Arc::default(), "h1", "r1", kind, "tool");
    (phase, obs)
}

#[test]
fn the_exit_notice_names_the_program_and_the_code() {
    let (_, obs) = observer(HarnessKind::Claude);
    let notice = obs.exit_notice(Some(2)).expect("a notice");
    let text = String::from_utf8(notice).unwrap();
    assert!(text.contains("[skein] tool exited (2)"), "{text:?}");
    let unknown = String::from_utf8(obs.exit_notice(None).unwrap()).unwrap();
    assert!(unknown.contains("tool exited (?)"), "{unknown:?}");
}

#[test]
fn only_a_live_hermes_observer_wants_the_screen() {
    for kind in [
        HarnessKind::Claude,
        HarnessKind::Opencode,
        HarnessKind::Byoh,
    ] {
        let (phase, obs) = observer(kind);
        assert!(!obs.wants_screen());
        obs.screen(&rows(READY));
        assert_eq!(phase.test_screen_reading("h1"), None);
    }
    let (_, obs) = observer(HarnessKind::Hermes);
    assert!(obs.wants_screen());
    obs.retire();
    assert!(!obs.wants_screen());
}

#[test]
fn a_reading_is_delivered_when_it_changes_and_not_repeated() {
    let (phase, obs) = observer(HarnessKind::Hermes);
    obs.screen(&rows(READY));
    assert_eq!(
        phase.test_screen_reading("h1"),
        Some(ScreenReading::Waiting)
    );

    // Overwrite what the service holds: an unchanged reading must not be
    // re-sent over it.
    phase.client_input(
        "h1",
        ClientPhaseInput::ScreenReading {
            reading: Some(ScreenReading::Running),
        },
    );
    obs.screen(&rows(READY));
    assert_eq!(
        phase.test_screen_reading("h1"),
        Some(ScreenReading::Running)
    );

    obs.screen(&rows(BUSY));
    assert_eq!(
        phase.test_screen_reading("h1"),
        Some(ScreenReading::Running)
    );
    obs.screen(&rows(READY));
    assert_eq!(
        phase.test_screen_reading("h1"),
        Some(ScreenReading::Waiting)
    );
}

#[test]
fn an_exit_withdraws_the_reading_and_no_later_one_lands() {
    let (phase, obs) = observer(HarnessKind::Hermes);
    obs.screen(&rows(READY));
    obs.exited(Some(0));
    assert_eq!(phase.test_screen_reading("h1"), None);
    obs.screen(&rows(BUSY));
    assert_eq!(phase.test_screen_reading("h1"), None);
}

#[test]
fn an_exit_withdraws_nothing_it_never_delivered() {
    // Raw mode: the webview owns the reading, the observer never sent one.
    let (phase, obs) = observer(HarnessKind::Hermes);
    phase.client_input(
        "h1",
        ClientPhaseInput::ScreenReading {
            reading: Some(ScreenReading::Waiting),
        },
    );
    obs.exited(Some(0));
    assert_eq!(
        phase.test_screen_reading("h1"),
        Some(ScreenReading::Waiting)
    );
}

#[test]
fn a_restart_starts_with_no_reading_and_the_old_exit_clears_nothing_new() {
    let (phase, old) = observer(HarnessKind::Hermes);
    old.screen(&rows(READY));
    assert_eq!(
        phase.test_screen_reading("h1"),
        Some(ScreenReading::Waiting)
    );

    // Restart: the runner retires the old observer, then builds the new one.
    old.retire();
    assert_eq!(phase.test_screen_reading("h1"), None);
    let new = RunnerObserver::new(
        Arc::clone(&phase),
        Arc::default(),
        "h1",
        "r1",
        HarnessKind::Hermes,
        "tool",
    );
    assert_eq!(phase.test_screen_reading("h1"), None);

    // The old process's late exit and cut change nothing; the new one's
    // reading lands and survives them.
    new.screen(&rows(BUSY));
    old.exited(Some(0));
    old.screen(&rows(READY));
    assert_eq!(
        phase.test_screen_reading("h1"),
        Some(ScreenReading::Running)
    );
}
