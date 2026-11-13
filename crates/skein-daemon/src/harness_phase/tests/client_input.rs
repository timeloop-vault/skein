//! `StartFailed` and `RestartRequested` (#732): the two inputs that make the
//! daemon write what the webview used to write past its own phase store.

use std::sync::Arc;

use crate::harness_phase::client_input::{ClientPhaseInput, RestartSource};
use crate::harness_phase::service::PhaseService;
use crate::harness_phase::test_support::RecordingSink;
use crate::harness_phase::types::ActivityPhase;
use crate::harness_phase::{ScreenReading, SessionSwitch};

const T0: i64 = 1_000_000;

fn rig() -> (Arc<PhaseService>, Arc<RecordingSink>) {
    PhaseService::test_with_clock_and_sink(Arc::new(|| T0))
}

fn rows(svc: &PhaseService, id: &str) -> Vec<(String, String, String, Option<String>, bool, i64)> {
    svc.flush_outputs();
    let mut rows = svc
        .test_db()
        .recent_harness_events_by_harness(id, 0, 100)
        .unwrap();
    rows.reverse();
    rows.into_iter()
        .map(|e| {
            (
                e.room_id,
                e.from_phase,
                e.to_phase,
                e.source,
                e.has_user_input,
                e.timestamp_ms,
            )
        })
        .collect()
}

fn parse(s: &str) -> ClientPhaseInput {
    serde_json::from_str(s).unwrap()
}

#[test]
fn start_failed_without_a_record_leaves_an_exited_one() {
    let (svc, sink) = rig();
    svc.start_failed("h", "r1");
    assert_eq!(svc.phase_snapshot()["h"], ActivityPhase::Exited);
    let got = rows(&svc, "h");
    assert_eq!(got.len(), 1);
    assert_eq!(
        (got[0].0.as_str(), got[0].1.as_str(), got[0].2.as_str()),
        ("r1", "spawning", "exited")
    );
    // One stream update, carrying the transition.
    let events = sink.events();
    assert_eq!(events.len(), 1);
    let v: serde_json::Value = serde_json::from_str(&events[0].1).unwrap();
    assert_eq!(v["roomId"], "r1");
    assert_eq!(v["view"]["phase"], "exited");
    assert_eq!(v["transitions"].as_array().unwrap().len(), 1);
}

#[test]
fn start_failed_replaces_a_running_record() {
    let (svc, _sink) = rig();
    svc.spawned("h", "r1", false);
    svc.record_output("h", "hello");
    assert_eq!(svc.phase_snapshot()["h"], ActivityPhase::Running);
    svc.start_failed("h", "r1");
    assert_eq!(svc.phase_snapshot()["h"], ActivityPhase::Exited);
    let last = rows(&svc, "h").pop().unwrap();
    assert_eq!((last.1.as_str(), last.2.as_str()), ("spawning", "exited"));
}

#[test]
fn restart_requested_with_a_record_logs_its_phase_and_changes_nothing() {
    let (svc, sink) = rig();
    svc.spawned("h", "r1", false);
    svc.record_output("h", "hello");
    svc.flush_outputs();
    let before_events = sink.events().len();
    let before_rows = rows(&svc, "h").len();
    svc.restart_requested("h", "r1", RestartSource::UserRestart);
    assert_eq!(svc.phase_snapshot()["h"], ActivityPhase::Running);
    let got = rows(&svc, "h");
    assert_eq!(got.len(), before_rows + 1);
    assert_eq!(
        got.last().unwrap(),
        &(
            "r1".to_owned(),
            "running".to_owned(),
            "spawning".to_owned(),
            Some("user-restart".to_owned()),
            false,
            T0
        )
    );
    assert_eq!(sink.events().len(), before_events);
}

#[test]
fn restart_requested_without_a_record_reads_unknown() {
    let (svc, sink) = rig();
    svc.restart_requested("h", "r1", RestartSource::VersionAutoRestart);
    assert!(svc.phase_snapshot().is_empty());
    assert_eq!(
        rows(&svc, "h"),
        vec![(
            "r1".to_owned(),
            "unknown".to_owned(),
            "spawning".to_owned(),
            Some("version-auto-restart".to_owned()),
            false,
            T0
        )]
    );
    assert_eq!(sink.events().len(), 0);
}

#[test]
fn both_inputs_deserialize_from_the_wire_shape() {
    assert!(matches!(
        parse(r#"{"kind":"startFailed"}"#),
        ClientPhaseInput::StartFailed
    ));
    // A frontend that still sends `roomId` is not refused: the key is ignored.
    assert!(matches!(
        parse(r#"{"kind":"startFailed","roomId":"r"}"#),
        ClientPhaseInput::StartFailed
    ));
    assert!(matches!(
        parse(r#"{"kind":"restartRequested","roomId":"r","source":"user-restart"}"#),
        ClientPhaseInput::RestartRequested {
            source: RestartSource::UserRestart
        }
    ));
    assert!(matches!(
        parse(r#"{"kind":"restartRequested","source":"version-auto-restart"}"#),
        ClientPhaseInput::RestartRequested {
            source: RestartSource::VersionAutoRestart
        }
    ));
    assert!(
        serde_json::from_str::<ClientPhaseInput>(
            r#"{"kind":"restartRequested","source":"pty-output"}"#
        )
        .is_err()
    );
}

#[test]
fn wire_shape_is_camel_case_tagged() {
    let i = parse(r#"{"kind":"input","data":"a","emptyComposer":true}"#);
    assert!(matches!(
        i,
        ClientPhaseInput::Input {
            empty_composer: true,
            ..
        }
    ));
    let i = parse(r#"{"kind":"sessionSwitched","switch":"fork"}"#);
    assert!(matches!(
        i,
        ClientPhaseInput::SessionSwitched {
            switch: SessionSwitch::Fork
        }
    ));
    let i = parse(r#"{"kind":"screenReading","reading":null}"#);
    assert!(matches!(
        i,
        ClientPhaseInput::ScreenReading { reading: None }
    ));
    let i = parse(r#"{"kind":"hermesLaunchPoll","screenWaiting":true}"#);
    assert!(matches!(
        i,
        ClientPhaseInput::HermesLaunchPoll {
            screen_waiting: true
        }
    ));
}

#[test]
fn every_frontend_shape_deserializes() {
    assert!(matches!(
        parse(r#"{"kind":"promptSubmitted"}"#),
        ClientPhaseInput::PromptSubmitted
    ));
    assert!(matches!(
        parse(r#"{"kind":"muteInducedOutput"}"#),
        ClientPhaseInput::MuteInducedOutput
    ));
    for (s, want) in [
        ("waiting", ScreenReading::Waiting),
        ("running", ScreenReading::Running),
        ("unknown", ScreenReading::Unknown),
    ] {
        let i = parse(&format!(r#"{{"kind":"screenReading","reading":"{s}"}}"#));
        assert!(matches!(i, ClientPhaseInput::ScreenReading { reading: Some(r) } if r == want));
    }
    assert!(matches!(
        parse(r#"{"kind":"launchSignal"}"#),
        ClientPhaseInput::LaunchSignal
    ));
    for (s, want) in [
        ("clear", SessionSwitch::Clear),
        ("resume", SessionSwitch::Resume),
        ("fork", SessionSwitch::Fork),
        ("startup", SessionSwitch::Startup),
    ] {
        let i = parse(&format!(r#"{{"kind":"sessionSwitched","switch":"{s}"}}"#));
        assert!(matches!(i, ClientPhaseInput::SessionSwitched { switch } if switch == want));
    }
    assert!(matches!(
        parse(r#"{"kind":"recoverUnheardSilence"}"#),
        ClientPhaseInput::RecoverUnheardSilence
    ));
}
