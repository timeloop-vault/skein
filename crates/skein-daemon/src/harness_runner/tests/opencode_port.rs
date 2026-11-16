//! The opencode `--port` is picked by the runner at spawn (#754), reported in
//! the spawn result, and never read back from the stored `cmd`.

use std::sync::atomic::{AtomicUsize, Ordering};

use skein_proto::rooms::LaunchMode;

use super::*;

fn sv(parts: &[&str]) -> Vec<String> {
    parts.iter().map(|s| (*s).to_owned()).collect()
}

/// A fixture with `h` in room `r1`, a picker that counts its calls and
/// answers `answer`, and a stand-in for the CLI.
fn fx_with_picker(h: Harness, answer: Result<u16, String>) -> (Fx, Arc<AtomicUsize>) {
    let fx = Fx::new();
    fx.create(room("r1", &fx.work, vec![h]));
    fx.runner().set_stand_in(shell_cmd());
    fx.runner().set_session_probe(Arc::new(|_| Ok(false)));
    let picks = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&picks);
    fx.runner().set_port_picker(Arc::new(move || {
        counter.fetch_add(1, Ordering::SeqCst);
        answer.clone()
    }));
    (fx, picks)
}

fn opencode(cmd: Option<Vec<String>>) -> Harness {
    harness("h1", "opencode", cmd, None)
}

#[test]
fn the_picked_port_is_in_the_result_and_the_argv_and_a_second_ensure_reuses_it() {
    let (fx, picks) = fx_with_picker(opencode(Some(sv(&["opencode"]))), Ok(4777));

    let first = fx.ensure(false, None).unwrap();
    assert!(first.started);
    assert_eq!(first.spawn.opencode_port, Some(4777));
    assert_eq!(
        fx.runner().spawned_argv(),
        vec![sv(&[
            "opencode",
            "--port",
            "4777",
            "--hostname",
            "127.0.0.1"
        ])]
    );

    let second = fx.ensure(false, None).unwrap();
    assert!(!second.started);
    assert_eq!(second.spawn.opencode_port, Some(4777));
    assert_eq!(picks.load(Ordering::SeqCst), 1, "a reuse picks nothing");
    assert_eq!(fx.runner().spawned_argv().len(), 1);

    // A restart is a new spawn and picks again.
    fx.ensure(true, None).unwrap();
    assert_eq!(picks.load(Ordering::SeqCst), 2);
}

#[test]
fn a_failing_picker_still_spawns_without_a_port() {
    let (fx, picks) = fx_with_picker(
        opencode(Some(sv(&["opencode"]))),
        Err("no ports".to_owned()),
    );

    let out = fx.ensure(false, None).unwrap();
    assert!(out.started);
    assert_eq!(out.spawn.opencode_port, None);
    assert_eq!(fx.runner().spawned_argv(), vec![sv(&["opencode"])]);
    assert_eq!(picks.load(Ordering::SeqCst), 1);
}

#[test]
fn a_stored_port_does_not_influence_the_port() {
    let stored = sv(&["opencode", "--port", "4555", "--hostname", "127.0.0.1"]);
    let (fx, _) = fx_with_picker(opencode(Some(stored.clone())), Ok(4888));
    let out = fx.ensure(false, None).unwrap();
    assert_eq!(out.spawn.opencode_port, Some(4888));

    // And with a failing picker the stored port is not a fallback.
    let (fx, _) = fx_with_picker(opencode(Some(stored)), Err("no ports".to_owned()));
    let out = fx.ensure(false, None).unwrap();
    assert_eq!(out.spawn.opencode_port, None);
    assert!(
        !fx.runner().spawned_argv()[0].iter().any(|a| a == "--port"),
        "{:?}",
        fx.runner().spawned_argv()
    );
}

#[test]
fn shell_mode_opencode_and_other_kinds_pick_nothing() {
    let mut shell = opencode(Some(shell_cmd()));
    shell.launch = Some(LaunchMode::Shell { argv: shell_cmd() });
    let (fx, picks) = fx_with_picker(shell, Ok(4999));
    let out = fx.ensure(false, None).unwrap();
    assert_eq!(out.spawn.opencode_port, None);
    assert_eq!(picks.load(Ordering::SeqCst), 0);

    let claude = harness("h1", "claude", Some(sv(&["claude"])), None);
    let (fx, picks) = fx_with_picker(claude, Ok(4999));
    let out = fx.ensure(false, None).unwrap();
    assert_eq!(out.spawn.opencode_port, None);
    assert_eq!(picks.load(Ordering::SeqCst), 0);
}
