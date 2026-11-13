//! The records decide stops: the runner hears of room changes through the
//! store's observer, never by being called.

use super::*;

fn running() -> (Fx, String, u32) {
    let fx = Fx::new();
    fx.create(room(
        "r1",
        &fx.work,
        vec![
            harness("h1", "byoh", Some(shell_cmd()), None),
            harness("h2", "byoh", Some(shell_cmd()), None),
        ],
    ));
    let out = fx.ensure(false, None).unwrap();
    let pid = fx.daemon.pty.pid(&out.spawn.id).expect("pid");
    (fx, out.spawn.id, pid)
}

#[test]
fn removing_the_harness_record_stops_its_process() {
    let (fx, id, pid) = running();
    fx.apply(RoomMutation::RemoveHarness {
        room_id: "r1".to_owned(),
        harness_id: "h1".to_owned(),
    });
    assert!(fx.runner().pty_id("h1").is_none());
    assert!(!fx.daemon.pty.is_alive(&id));
    assert!(wait_until(|| !pid_exists(pid)), "the process survived");
}

#[test]
fn archiving_the_room_stops_its_process() {
    let (fx, id, pid) = running();
    let mut r = fx.daemon.rooms.rooms().remove(0);
    r.archived = Some(1_700_000_000_000);
    fx.apply(RoomMutation::UpdateRoom { room: r });
    assert!(fx.runner().pty_id("h1").is_none());
    assert!(!fx.daemon.pty.is_alive(&id));
    assert!(wait_until(|| !pid_exists(pid)), "the process survived");
    assert_eq!(
        fx.ensure(false, None).err().map(|e| e.code()),
        Some("room_archived")
    );
}

#[test]
fn an_unrelated_change_stops_nothing() {
    let (fx, id, _pid) = running();
    fx.apply(RoomMutation::RemoveHarness {
        room_id: "r1".to_owned(),
        harness_id: "h2".to_owned(),
    });
    assert_eq!(fx.runner().pty_id("h1").as_deref(), Some(id.as_str()));
    assert!(fx.daemon.pty.is_alive(&id));
}

/// Make the runner call `hook` when it reaches the pause point `name`.
fn pause_at(fx: &Fx, name: &'static str, hook: impl Fn() + Send + Sync + 'static) {
    fx.runner().pause_points.lock().insert(name, Arc::new(hook));
}

#[test]
fn a_stop_during_a_spawn_waits_for_it_and_stops_what_it_made() {
    let fx = Fx::with_shell("byoh");
    let (in_spawn, entered) = std::sync::mpsc::channel::<()>();
    let (go, released) = std::sync::mpsc::channel::<()>();
    let released = Mutex::new(released);
    pause_at(&fx, "before_spawn", move || {
        in_spawn.send(()).unwrap();
        released.lock().recv().unwrap();
    });
    let runner = Arc::clone(fx.runner());
    std::thread::scope(|s| {
        let ensure = s.spawn(|| {
            runner
                .ensure_running(
                    &EnsureRequest {
                        room_id: "r1".to_owned(),
                        harness_id: "h1".to_owned(),
                        rows: 24,
                        cols: 80,
                        env_profile: None,
                        restart: false,
                        expect_cmd: None,
                        expect_launch: None,
                    },
                    None,
                )
                .map(|o| o.started)
        });
        entered.recv().unwrap();
        let stop = s.spawn(|| runner.stop_harness("h1"));
        // Give the stop time to reach the gate; the assertions below hold
        // whichever side wins, and fail without the gate whenever it does.
        std::thread::sleep(Duration::from_millis(200));
        go.send(()).unwrap();
        assert!(ensure.join().unwrap().is_ok());
        assert!(stop.join().unwrap(), "the stop found the spawned process");
    });
    assert!(fx.runner().pty_id("h1").is_none());
}

#[test]
fn a_release_racing_a_stop_leaves_no_adapter_behind() {
    let fx = Fx::new();
    fx.create(room(
        "r1",
        &fx.work,
        vec![in_shell(harness(
            "h1",
            "claude",
            Some(shell_cmd()),
            Some("7f3c0a52-0000-4000-8000-000000000732"),
        ))],
    ));
    fx.ensure(false, None).unwrap();
    let (paused, entered) = std::sync::mpsc::channel::<()>();
    let (go, released) = std::sync::mpsc::channel::<()>();
    let released = Mutex::new(released);
    pause_at(&fx, "release_events", move || {
        paused.send(()).unwrap();
        released.lock().recv().unwrap();
    });
    let runner = Arc::clone(fx.runner());
    std::thread::scope(|s| {
        let release = s.spawn(|| runner.release_events("h1"));
        entered.recv().unwrap();
        let stop = s.spawn(|| runner.stop_harness("h1"));
        std::thread::sleep(Duration::from_millis(200));
        go.send(()).unwrap();
        release.join().unwrap();
        assert!(stop.join().unwrap());
    });
    assert!(
        !fx.daemon.claude.detach("h1"),
        "an adapter was re-attached for a stopped harness"
    );
}

#[test]
fn releasing_events_of_an_exited_process_attaches_nothing() {
    let fx = Fx::new();
    fx.create(room(
        "r1",
        &fx.work,
        vec![in_shell(harness(
            "h1",
            "claude",
            Some(exit_3_cmd()),
            Some("7f3c0a52-0000-4000-8000-000000000733"),
        ))],
    ));
    let out = fx.ensure(false, None).unwrap();
    assert!(wait_until(|| fx
        .daemon
        .pty
        .exit_code(&out.spawn.id)
        .is_some()));
    fx.runner().release_events("h1");
    assert!(
        !fx.daemon.claude.detach("h1"),
        "an adapter was attached for an exited process"
    );
}

#[test]
fn reconcile_judges_the_current_records_not_the_snapshot_it_was_handed() {
    let (fx, id, _pid) = running();
    // A stale (here: empty) snapshot wants nothing; the store still wants h1.
    fx.runner().reconcile(&[]);
    assert_eq!(fx.runner().pty_id("h1").as_deref(), Some(id.as_str()));
    assert!(fx.daemon.pty.is_alive(&id));
}

#[test]
fn a_stopped_harness_leaves_no_gate_behind() {
    let (fx, _id, _pid) = running();
    assert!(fx.runner().spawns.lock().contains_key("h1"));
    assert!(fx.runner().stop_harness("h1"));
    assert!(!fx.runner().spawns.lock().contains_key("h1"));
    // And it can be started again.
    assert!(fx.ensure(false, None).unwrap().started);
}
