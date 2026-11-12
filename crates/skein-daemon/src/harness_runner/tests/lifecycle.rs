//! Reuse, restart, detach and stop.

use super::*;

#[test]
fn a_second_ensure_reuses_the_process_and_restart_replaces_it() {
    let fx = Fx::with_shell("byoh");
    let first = fx.ensure(false, None).unwrap();
    assert!(first.started);
    let id1 = fx.runner().pty_id("h1").unwrap();
    assert_eq!(first.spawn.id, id1);
    let pid1 = fx.daemon.pty.pid(&id1).expect("pid");

    let again = fx.ensure(false, None).unwrap();
    assert!(!again.started, "an existing process is reused");
    assert_eq!(again.spawn.id, id1);
    assert_eq!(fx.runner().pty_id("h1").as_deref(), Some(id1.as_str()));

    let restarted = fx.ensure(true, None).unwrap();
    assert!(restarted.started);
    let id2 = fx.runner().pty_id("h1").unwrap();
    assert_ne!(id1, id2);
    assert!(!fx.daemon.pty.is_alive(&id1), "the old PTY is gone");
    assert!(fx.daemon.pty.is_alive(&id2));
    assert!(
        wait_until(|| !pid_exists(pid1)),
        "the old process was not killed"
    );
}

#[test]
fn detaching_a_view_does_not_kill_the_process() {
    let fx = Fx::with_shell("byoh");
    let first = Recording::default();
    let out = fx.ensure(false, Some(&first)).unwrap();
    let id = out.spawn.id.clone();
    let pid = fx.daemon.pty.pid(&id).expect("pid");
    fx.answer_cursor_query(&id, &first);

    assert!(fx.runner().detach_view("h1"), "a sink was attached");
    assert!(!fx.runner().detach_view("h1"), "nothing left to detach");
    assert!(fx.daemon.pty.is_alive(&id));
    assert!(pid_exists(pid));
    assert_eq!(fx.runner().pty_id("h1").as_deref(), Some(id.as_str()));

    // A later view gets the same process, and its new output.
    let second = Recording::default();
    let again = fx.ensure(false, Some(&second)).unwrap();
    assert!(!again.started);
    assert_eq!(again.spawn.id, id);
    fx.daemon
        .pty
        .write(&id, b"echo skein-after-detach-731\r\n")
        .unwrap();
    assert!(
        second.wait_text("skein-after-detach-731"),
        "no output after re-attach; got {:?}",
        second.text()
    );
}

#[test]
fn a_stale_detach_leaves_the_newer_sink_attached() {
    let fx = Fx::with_shell("byoh");
    let first = Recording::default();
    let out = fx.ensure(false, Some(&first)).unwrap();
    let id = out.spawn.id.clone();
    let old = out.attach_id.expect("attach id");
    fx.answer_cursor_query(&id, &first);

    let second = Recording::default();
    let again = fx.ensure(false, Some(&second)).unwrap();
    let new = again.attach_id.expect("attach id");
    assert!(new > old, "ids increase");

    assert!(!fx.runner().detach_view_if("h1", old), "stale id");
    fx.daemon
        .pty
        .write(&id, b"echo skein-stale-detach-731\r\n")
        .unwrap();
    assert!(
        second.wait_text("skein-stale-detach-731"),
        "newer sink was blanked; got {:?}",
        second.text()
    );
    assert!(fx.runner().detach_view_if("h1", new));
    assert!(!fx.runner().detach_view_if("h1", new), "already detached");
}

#[test]
fn an_unknown_harness_cannot_be_detached() {
    let fx = Fx::new();
    assert!(!fx.runner().detach_view("nope"));
}

#[test]
fn stop_harness_kills_forgets_and_is_idempotent() {
    let fx = Fx::with_shell("byoh");
    let out = fx.ensure(false, None).unwrap();
    let id = out.spawn.id;
    let pid = fx.daemon.pty.pid(&id).expect("pid");

    assert!(fx.runner().stop_harness("h1"), "the runner had it");
    assert!(fx.runner().pty_id("h1").is_none());
    assert!(!fx.daemon.pty.is_alive(&id));
    assert!(wait_until(|| !pid_exists(pid)), "the process survived");

    assert!(!fx.runner().stop_harness("h1"), "second stop is a no-op");
    assert!(!fx.runner().stop_harness("never-started"));

    // And a stopped harness can be started again.
    assert!(fx.ensure(false, None).unwrap().started);
}

#[test]
fn a_process_that_exits_by_itself_is_not_restarted() {
    let fx = Fx::new();
    fx.create(room(
        "r1",
        &fx.work,
        vec![harness("h1", "byoh", Some(exit_3_cmd()), None)],
    ));
    let first = fx.ensure(false, None).unwrap();
    assert!(first.started);
    let id = first.spawn.id;
    assert!(
        wait_until(|| fx.daemon.pty.exit_code(&id).is_some()),
        "the stand-in never exited"
    );
    // Nothing starts a replacement, however long we look.
    std::thread::sleep(Duration::from_secs(1));
    assert_eq!(fx.runner().pty_id("h1").as_deref(), Some(id.as_str()));

    // A view arriving later is told, and still no second spawn.
    let view = Recording::default();
    let again = fx.ensure(false, Some(&view)).unwrap();
    assert!(!again.started);
    assert_eq!(again.spawn.id, id);
    assert_eq!(again.exit.map(|e| e.code), Some(Some(3)));
    assert_eq!(view.exits(), vec![Some(3)]);
    assert_eq!(fx.runner().pty_id("h1").as_deref(), Some(id.as_str()));

    // Without a view the exit is reported too.
    let quiet = fx.ensure(false, None).unwrap();
    assert!(!quiet.started);
    assert_eq!(quiet.exit.map(|e| e.code), Some(Some(3)));
}
