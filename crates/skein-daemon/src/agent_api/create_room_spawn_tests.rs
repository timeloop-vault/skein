//! `create_room` with no client attached starts the first harness itself
//! (#734). The runner comes from a whole daemon built the way the app builds
//! one; the verb's state is a separate one with no client.

use super::create_room_tests::{create_room_args, init_repo};
use super::open_harness_spawn_tests::{build_daemon, clientless_state, wait_until};
use super::tests::{caller_for, harness, room};
use super::verbs::{self, MailContext};
use skein_proto::room_mutations::RoomMutation;

#[tokio::test(flavor = "multi_thread")]
async fn create_room_with_no_client_leaves_the_first_harness_running() {
    let dir = tempfile::TempDir::new().unwrap();
    let repo = init_repo(&dir);
    let daemon = build_daemon(dir.path());
    let mut r1 = room("r1", vec![harness("h1", "claude", "main")]);
    r1.cwd = Some(repo);
    daemon
        .rooms
        .apply("test", vec![RoomMutation::CreateRoom { room: r1 }])
        .unwrap();
    let state = clientless_state(&daemon);
    let caller = caller_for(&daemon.db, "r1", Some("h1"));

    let args = verbs::CreateRoomArgs {
        kind: Some("byoh".to_owned()),
        ..create_room_args("spawn me")
    };
    let out = verbs::create_room(&state, &caller, &args, &MailContext::permissive(), true).await;

    let out = match out {
        Ok(out) => out,
        Err(e) => panic!("create_room failed: {e:?}"),
    };
    let pty_id = daemon.harness_runner.pty_id(&out.harness_id);
    let alive = pty_id
        .as_deref()
        .is_some_and(|id| wait_until(|| daemon.pty.is_alive(id)));
    daemon.harness_runner.stop_harness(&out.harness_id);
    assert!(
        alive,
        "the new room's harness has no live process: {pty_id:?}"
    );
}

/// #583's end-to-end acceptance: a daemon with no client, a room created
/// through the `create_room` verb, its first harness spawned, the phase
/// machine moved by the child's output, and a `harness_events` row written
/// for that harness by the daemon's own phase-out thread.
#[tokio::test(flavor = "multi_thread")]
async fn create_room_with_no_client_spawns_a_harness_whose_phase_and_events_update() {
    use crate::harness_phase::ActivityPhase;

    let dir = tempfile::TempDir::new().unwrap();
    let repo = init_repo(&dir);
    let daemon = build_daemon(dir.path());
    let mut r1 = room("r1", vec![harness("h1", "claude", "main")]);
    r1.cwd = Some(repo);
    daemon
        .rooms
        .apply("test", vec![RoomMutation::CreateRoom { room: r1 }])
        .unwrap();
    let state = clientless_state(&daemon);
    let caller = caller_for(&daemon.db, "r1", Some("h1"));

    let args = verbs::CreateRoomArgs {
        kind: Some("byoh".to_owned()),
        ..create_room_args("spawn me")
    };
    let out = verbs::create_room(&state, &caller, &args, &MailContext::permissive(), true).await;
    let out = match out {
        Ok(out) => out,
        Err(e) => panic!("create_room failed: {e:?}"),
    };

    let pty_id = daemon.harness_runner.pty_id(&out.harness_id);
    let alive = pty_id
        .as_deref()
        .is_some_and(|id| wait_until(|| daemon.pty.is_alive(id)));
    if let Some(id) = pty_id.as_deref() {
        let _ = daemon.pty.write(id, b"echo skein-e2e-583\r\n");
    }
    let moved = wait_until(|| {
        daemon
            .phase
            .get(&out.harness_id)
            .is_some_and(|a| a.phase != ActivityPhase::Spawning)
    });
    let phase = daemon.phase.get(&out.harness_id).map(|a| a.phase);
    let mut rows = Vec::new();
    wait_until(|| {
        rows = daemon
            .db
            .recent_harness_events_by_harness(&out.harness_id, 0, 100)
            .unwrap_or_default();
        !rows.is_empty()
    });
    let stored = daemon
        .rooms
        .rooms()
        .into_iter()
        .find(|r| r.id == out.room_id)
        .map(|r| r.harnesses.iter().any(|h| h.id == out.harness_id));
    let revision = daemon
        .rooms
        .rooms()
        .into_iter()
        .find(|r| r.id == out.room_id)
        .map(|r| r.revision);
    daemon.harness_runner.stop_harness(&out.harness_id);

    assert_eq!(stored, Some(true), "the new room is not in the store");
    assert!(revision > Some(0), "went through the intent path");
    assert!(alive, "the new room's harness has no live process");
    assert!(moved, "the phase never left spawning: {phase:?}");
    assert!(!rows.is_empty(), "no harness_events row was written");
    assert!(rows.iter().all(|e| e.room_id == out.room_id));
}
