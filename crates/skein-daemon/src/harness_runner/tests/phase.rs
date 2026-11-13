//! The runner feeds the shadow phase machine (#732): spawn, output, exit,
//! stop, and the adapter events of a daemon with no client at all.

use super::*;
use crate::harness_events_claude::ClaudeEvent;
use crate::harness_phase::ActivityPhase;
use crate::harness_runner::Refusal;
use crate::stream_sink::StreamSink;

fn phase_of(fx: &Fx, id: &str) -> Option<ActivityPhase> {
    fx.daemon.phase.get(id).map(|a| a.phase)
}

#[test]
fn a_spawn_registers_the_harness_and_a_stop_forgets_it() {
    let fx = Fx::with_shell("byoh");
    assert_eq!(phase_of(&fx, "h1"), None, "nothing runs yet");

    let out = fx.ensure(false, None).unwrap();
    assert!(out.started);
    assert!(
        phase_of(&fx, "h1").is_some(),
        "spawned() ran before ensure_running returned"
    );
    assert!(fx.daemon.phase.phase_snapshot().contains_key("h1"));

    // A reuse is not a spawn.
    let before = fx.daemon.phase.get("h1").unwrap().spawned_at;
    assert!(!fx.ensure(false, None).unwrap().started);
    assert_eq!(fx.daemon.phase.get("h1").unwrap().spawned_at, before);

    assert!(fx.runner().stop_harness("h1"));
    assert_eq!(phase_of(&fx, "h1"), None, "stop forgets the record");
}

#[test]
fn pty_output_reaches_the_machine_with_no_view_attached() {
    let fx = Fx::with_shell("byoh");
    let out = fx.ensure(false, None).unwrap();
    fx.daemon
        .pty
        .write(&out.spawn.id, b"echo skein-tap-732\r\n")
        .unwrap();
    assert!(
        wait_until(|| fx
            .daemon
            .phase
            .get("h1")
            .is_some_and(|a| a.last_output_at.is_some())),
        "the machine never heard the child's output"
    );
}

#[test]
fn a_natural_exit_keeps_the_entry_and_the_record_reads_exited() {
    let fx = Fx::new();
    fx.create(room(
        "r1",
        &fx.work,
        vec![harness("h1", "byoh", Some(exit_3_cmd()), None)],
    ));
    fx.ensure(false, None).unwrap();
    assert!(
        wait_until(|| phase_of(&fx, "h1") == Some(ActivityPhase::Exited)),
        "phase {:?}",
        phase_of(&fx, "h1")
    );
    assert_eq!(fx.daemon.phase.get("h1").unwrap().exit_code, Some(3));
    assert!(fx.runner().pty_id("h1").is_some(), "the entry stays");

    // A view attaching afterwards is told of the exit but does not change
    // the record or repeat the exit into it.
    let rec = Recording::default();
    let again = fx.ensure(false, Some(&rec)).unwrap();
    assert!(!again.started);
    assert_eq!(phase_of(&fx, "h1"), Some(ActivityPhase::Exited));
}

#[test]
fn a_restart_replaces_the_record_and_the_old_exit_does_not_land_on_it() {
    let fx = Fx::with_shell("byoh");
    fx.ensure(false, None).unwrap();
    let first = fx.runner().pty_id("h1").unwrap();
    let pid = fx.daemon.pty.pid(&first).expect("pid");

    fx.ensure(true, None).unwrap();
    assert!(wait_until(|| !pid_exists(pid)), "old process still alive");
    // Give the old waiter thread time to report its (retired) exit.
    std::thread::sleep(Duration::from_millis(500));
    let phase = phase_of(&fx, "h1");
    assert!(
        phase.is_some() && phase != Some(ActivityPhase::Exited),
        "the dead process's exit marked the new one: {phase:?}"
    );
}

/// The old process is stopped before the plan is built: a plan that then
/// fails must not leave its phase record behind.
#[test]
fn a_restart_that_fails_at_the_plan_forgets_the_record() {
    let fx = Fx::with_shell("byoh");
    fx.ensure(false, None).unwrap();
    assert!(phase_of(&fx, "h1").is_some());

    fx.apply(RoomMutation::PutHarness {
        room_id: "r1".to_owned(),
        harness: harness("h1", "byoh", None, None),
    });
    assert!(matches!(
        fx.ensure(true, None),
        Err(EnsureError::Refused(Refusal::NoCmd))
    ));
    assert_eq!(phase_of(&fx, "h1"), None, "no process, no record");
    assert_eq!(fx.runner().pty_id("h1"), None);
}

#[test]
fn a_refused_spawn_leaves_no_record() {
    let fx = Fx::new();
    fx.create(room(
        "r1",
        &fx.work,
        vec![harness("h1", "byoh", None, None)],
    ));
    assert!(fx.ensure(false, None).is_err());
    assert_eq!(phase_of(&fx, "h1"), None);
}

/// No view, no client event channel: the daemon's own sink drives the phase.
#[test]
fn the_daemons_own_claude_sink_moves_the_phase_with_no_client() {
    let fx = Fx::new();
    fx.create(room(
        "r1",
        &fx.work,
        vec![harness("h1", "claude", Some(shell_cmd()), Some("sess-732"))],
    ));
    fx.ensure(false, None).unwrap();
    assert_eq!(phase_of(&fx, "h1"), Some(ActivityPhase::Spawning));
    assert!(
        fx.daemon.claude.is_attached("h1"),
        "the runner attached the Claude tail"
    );

    // The sink the runner attached is `ClaudePhaseSink`; the helper builds
    // the same thing, which is what the app tees with the client's.
    let sink = fx.runner().claude_phase_sink("h1");
    sink.send(ClaudeEvent::AssistantTurn).unwrap();
    assert_eq!(phase_of(&fx, "h1"), Some(ActivityPhase::Running));
    sink.send(ClaudeEvent::AwaitingPrompt).unwrap();
    assert_eq!(phase_of(&fx, "h1"), Some(ActivityPhase::Waiting));
}

#[test]
fn releasing_events_of_an_exited_process_detaches_the_phase_source() {
    let fx = Fx::new();
    fx.create(room(
        "r1",
        &fx.work,
        vec![harness(
            "h1",
            "claude",
            Some(exit_3_cmd()),
            Some("sess-732"),
        )],
    ));
    fx.ensure(false, None).unwrap();
    assert!(wait_until(
        || phase_of(&fx, "h1") == Some(ActivityPhase::Exited)
    ));
    fx.runner().release_events("h1");
    assert!(!fx.daemon.claude.is_attached("h1"));
    assert_eq!(phase_of(&fx, "h1"), Some(ActivityPhase::Exited));
}
