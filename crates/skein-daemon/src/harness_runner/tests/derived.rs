//! The argv is derived from the record at spawn time (#752), not spawned as
//! stored. `claude` need not be installed: the derived argv is recorded and a
//! stand-in shell is spawned in its place.

use skein_proto::rooms::LaunchMode;

use super::*;

const SID: &str = "7f3c0a52-0000-4000-8000-000000000752";

fn sv(parts: &[&str]) -> Vec<String> {
    parts.iter().map(|s| (*s).to_owned()).collect()
}

/// One Claude harness `h1`; the stand-in shell is spawned for it and the
/// probe reports every session as existing.
fn claude_fx(cmd: &[&str], session: Option<&str>) -> Fx {
    let fx = Fx::new();
    fx.create(room(
        "r1",
        &fx.work,
        vec![harness("h1", "claude", Some(sv(cmd)), session)],
    ));
    fx.runner().set_stand_in(shell_cmd());
    fx.runner().set_session_probe(Arc::new(|_| Ok(true)));
    fx
}

fn stored(fx: &Fx) -> Harness {
    fx.daemon.rooms.harness_record("r1", "h1").unwrap()
}

#[test]
fn a_claude_record_with_a_transcript_is_spawned_as_resume() {
    let fx = claude_fx(&["claude", "--session-id", SID], Some(SID));
    fx.ensure(false, None).unwrap();
    assert_eq!(
        fx.runner().spawned_argv().last(),
        Some(&sv(&["claude", "--resume", SID]))
    );

    fx.ensure(true, None).unwrap();
    assert_eq!(fx.runner().spawned_argv().len(), 2);
    assert_eq!(
        fx.runner().spawned_argv().last(),
        Some(&sv(&["claude", "--resume", SID])),
        "a restart derives it again"
    );
}

#[test]
fn a_shell_launch_spawns_the_shell_argv_whatever_cmd_holds() {
    let fx = Fx::new();
    let mut h = harness(
        "h1",
        "claude",
        Some(sv(&["claude", "--resume", SID])),
        Some(SID),
    );
    h.launch = Some(LaunchMode::Shell { argv: shell_cmd() });
    fx.create(room("r1", &fx.work, vec![h]));
    fx.runner()
        .set_session_probe(Arc::new(|_| panic!("a shell is not probed")));

    assert!(fx.ensure(false, None).unwrap().started);
    assert_eq!(fx.runner().spawned_argv(), vec![shell_cmd()]);
}

#[test]
fn an_unexpected_launch_mode_is_refused_as_stale() {
    let fx = Fx::with_shell("byoh");
    let ask = |expect_launch: Option<LaunchMode>| {
        fx.runner().ensure_running(
            &EnsureRequest {
                room_id: "r1".to_owned(),
                harness_id: "h1".to_owned(),
                rows: 24,
                cols: 80,
                env_profile: None,
                restart: false,
                expect_cmd: None,
                expect_launch,
            },
            None,
        )
    };
    let err = ask(Some(LaunchMode::Shell { argv: shell_cmd() }))
        .err()
        .expect("refused");
    assert_eq!(err.code(), "stale_record");
    assert!(fx.runner().pty_id("h1").is_none(), "nothing was spawned");

    // An absent launch reads as the tool form.
    assert!(ask(Some(LaunchMode::Tool)).unwrap().started);
}

#[test]
fn a_claude_record_without_a_session_gets_a_minted_id_and_keeps_it() {
    let fx = claude_fx(&["claude"], None);
    fx.ensure(false, None).unwrap();
    let argv = fx.runner().spawned_argv().pop().unwrap();
    assert_eq!(argv.len(), 3, "{argv:?}");
    assert_eq!(
        (argv[0].as_str(), argv[1].as_str()),
        ("claude", "--session-id")
    );
    assert_eq!(argv[2].len(), 36, "a uuid: {}", argv[2]);
    assert_eq!(stored(&fx).session_id.as_deref(), Some(argv[2].as_str()));

    // The next spawn reuses the stored id rather than minting another.
    fx.ensure(true, None).unwrap();
    assert_eq!(fx.runner().spawned_argv().pop().unwrap()[1], "--resume");
    assert_eq!(stored(&fx).session_id.as_deref(), Some(argv[2].as_str()));
}
