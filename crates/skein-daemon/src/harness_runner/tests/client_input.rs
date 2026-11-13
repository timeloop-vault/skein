//! The client's `startFailed` / `restartRequested` phase inputs are checked
//! against the stored records and the live process before they act (#732).

use super::*;
use crate::harness_events_claude::ClaudeEvent;
use crate::harness_phase::client_input::RestartSource;
use crate::harness_phase::{ActivityPhase, ClientPhaseInput};
use crate::stream_sink::StreamSink;

fn phase_of(fx: &Fx, id: &str) -> Option<ActivityPhase> {
    fx.daemon.phase.get(id).map(|a| a.phase)
}

/// `(room id, to phase, source)` of every logged row of `id`, oldest first.
fn rows(fx: &Fx, id: &str) -> Vec<(String, String, Option<String>)> {
    fx.daemon.phase.flush_outputs();
    let mut rows = fx
        .daemon
        .db
        .recent_harness_events_by_harness(id, 0, 100)
        .unwrap();
    rows.reverse();
    rows.into_iter()
        .map(|e| (e.room_id, e.to_phase, e.source))
        .collect()
}

#[test]
fn start_failed_for_a_running_harness_changes_nothing() {
    let fx = Fx::new();
    fx.create(room(
        "r1",
        &fx.work,
        vec![harness("h1", "claude", Some(shell_cmd()), Some("sess-732"))],
    ));
    fx.ensure(false, None).unwrap();
    assert_eq!(phase_of(&fx, "h1"), Some(ActivityPhase::Spawning));

    fx.runner()
        .client_phase_input("h1", ClientPhaseInput::StartFailed);
    assert_eq!(phase_of(&fx, "h1"), Some(ActivityPhase::Spawning));

    // The translator survived: an adapter event still moves the phase.
    fx.runner()
        .claude_phase_sink("h1")
        .send(ClaudeEvent::AssistantTurn)
        .unwrap();
    assert_eq!(phase_of(&fx, "h1"), Some(ActivityPhase::Running));
}

#[test]
fn start_failed_for_an_unknown_harness_creates_no_record() {
    let fx = Fx::with_shell("byoh");
    fx.runner()
        .client_phase_input("ghost", ClientPhaseInput::StartFailed);
    assert_eq!(phase_of(&fx, "ghost"), None);
    assert_eq!(fx.daemon.phase.stream_snapshot().len(), 0);
    assert_eq!(rows(&fx, "ghost").len(), 0);
}

#[test]
fn start_failed_for_a_stored_harness_with_no_process_reads_exited() {
    let fx = Fx::with_shell("byoh");
    fx.runner()
        .client_phase_input("h1", ClientPhaseInput::StartFailed);
    assert_eq!(phase_of(&fx, "h1"), Some(ActivityPhase::Exited));
    let got = rows(&fx, "h1");
    assert_eq!(got.len(), 1, "{got:?}");
    assert_eq!(got[0].0, "r1", "the stored room id");
    assert_eq!(got[0].1, "exited");
}

#[test]
fn restart_requested_for_an_unknown_harness_writes_no_row() {
    let fx = Fx::with_shell("byoh");
    fx.runner().client_phase_input(
        "ghost",
        ClientPhaseInput::RestartRequested {
            source: RestartSource::UserRestart,
        },
    );
    assert_eq!(rows(&fx, "ghost").len(), 0);
}

/// An archived room's harness and a non-PTY kind: not spawnable, so neither
/// input may create a record or a row for them.
fn assert_both_inputs_dropped(fx: &Fx, id: &str) {
    fx.runner()
        .client_phase_input(id, ClientPhaseInput::StartFailed);
    fx.runner().client_phase_input(
        id,
        ClientPhaseInput::RestartRequested {
            source: RestartSource::UserRestart,
        },
    );
    assert_eq!(phase_of(fx, id), None);
    assert_eq!(rows(fx, id).len(), 0);
}

#[test]
fn both_inputs_are_dropped_for_a_harness_in_an_archived_room() {
    let fx = Fx::new();
    let mut r = room(
        "r1",
        &fx.work,
        vec![harness("h1", "byoh", Some(shell_cmd()), None)],
    );
    r.archived = Some(1);
    fx.create(r);
    assert_both_inputs_dropped(&fx, "h1");
}

#[test]
fn both_inputs_are_dropped_for_a_non_pty_harness() {
    let fx = Fx::new();
    fx.create(room(
        "r1",
        &fx.work,
        vec![
            harness("f1", "files", None, None),
            harness("d1", "design", None, None),
        ],
    ));
    assert_both_inputs_dropped(&fx, "f1");
    assert_both_inputs_dropped(&fx, "d1");
}

#[test]
fn restart_requested_for_a_stored_harness_logs_the_stored_room_id() {
    let fx = Fx::with_shell("byoh");
    fx.runner().client_phase_input(
        "h1",
        ClientPhaseInput::RestartRequested {
            source: RestartSource::UserRestart,
        },
    );
    let got = rows(&fx, "h1");
    assert_eq!(
        got,
        vec![(
            "r1".to_owned(),
            "spawning".to_owned(),
            Some("user-restart".to_owned())
        )]
    );
}
