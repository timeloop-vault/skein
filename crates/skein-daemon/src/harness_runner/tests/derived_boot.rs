//! The derived argv (#752) against what boot preparation leaves in the
//! store: no spawn path may depend on the boot rewrite having run, and the
//! probe, launch-mode and port rules hold at the runner's seams.

use skein_proto::rooms::{LaunchMode, OpencodeVersion};

use super::*;
use crate::boot_resume::{changed_harnesses, prepare_with};
use crate::harness_kind::HarnessKind;
use crate::harness_runner::gates::{SpawnPlan, stored_opencode_port};

const SID: &str = "7f3c0a52-0000-4000-8000-000000000752";

fn sv(parts: &[&str]) -> Vec<String> {
    parts.iter().map(|s| (*s).to_owned()).collect()
}

fn fx_with(h: Harness) -> Fx {
    let fx = Fx::new();
    fx.create(room("r1", &fx.work, vec![h]));
    fx
}

fn stored(fx: &Fx) -> Harness {
    fx.daemon.rooms.harness_record("r1", "h1").unwrap()
}

fn as_json(h: &Harness) -> Value {
    serde_json::to_value(h).unwrap()
}

/// What the daemon does at the first rooms load: prepare the stored rooms and
/// write the changed harnesses through the store.
fn boot_prepare(fx: &Fx, probe: impl Fn(&Harness) -> Result<bool, String>) {
    let before = fx.daemon.rooms.rooms();
    let out = prepare_with(before.clone(), probe, || Ok(4321), || "minted".to_owned());
    let muts = changed_harnesses(&before, &out.rooms);
    if !muts.is_empty() {
        let res = fx.daemon.rooms.apply("daemon-boot", muts).unwrap();
        assert!(res.skipped.is_empty(), "{:?}", res.skipped);
    }
}

#[test]
fn a_legacy_shell_record_goes_through_boot_and_spawns_the_shell() {
    // Claude kind, no `launch`, a stored cmd that is a shell: pre-#752 shape.
    let fx = fx_with(harness("h1", "claude", Some(shell_cmd()), Some(SID)));
    assert_eq!(stored(&fx).launch, None);
    // Boot probes by kind and id, shell mode or not (as it always has).
    boot_prepare(&fx, |_| Ok(true));
    assert_eq!(
        stored(&fx).launch,
        Some(LaunchMode::Shell { argv: shell_cmd() })
    );

    fx.runner()
        .set_session_probe(Arc::new(|_| panic!("a shell is not probed")));
    assert!(fx.ensure(false, None).unwrap().started);
    assert_eq!(fx.runner().spawned_argv(), vec![shell_cmd()]);
    let h = stored(&fx);
    assert_eq!(h.launch, Some(LaunchMode::Shell { argv: shell_cmd() }));
    assert_eq!(h.session_id.as_deref(), Some(SID), "a shell keeps its id");
}

#[test]
fn a_claude_record_resumes_with_or_without_boot_preparation() {
    for boot in [true, false] {
        let fx = fx_with(harness(
            "h1",
            "claude",
            Some(sv(&["claude", "--session-id", SID])),
            Some(SID),
        ));
        fx.runner().set_stand_in(shell_cmd());
        fx.runner().set_session_probe(Arc::new(|_| Ok(true)));
        if boot {
            boot_prepare(&fx, |_| Ok(true));
            assert_eq!(
                stored(&fx).cmd,
                Some(sv(&["claude", "--resume", SID])),
                "boot did rewrite the stored cmd"
            );
        }
        fx.ensure(false, None).unwrap();
        assert_eq!(
            fx.runner().spawned_argv(),
            vec![sv(&["claude", "--resume", SID])],
            "boot preparation ran: {boot}"
        );
    }
}

#[test]
fn a_probe_error_keeps_the_id_and_writes_nothing_back() {
    let fx = fx_with(harness(
        "h1",
        "claude",
        Some(sv(&["claude", "--session-id", SID])),
        Some(SID),
    ));
    fx.runner().set_stand_in(shell_cmd());
    fx.runner()
        .set_session_probe(Arc::new(|_| Err("no store".to_owned())));
    let before = as_json(&stored(&fx));

    fx.ensure(false, None).unwrap();
    assert_eq!(
        fx.runner().spawned_argv(),
        vec![sv(&["claude", "--resume", SID])]
    );
    assert_eq!(as_json(&stored(&fx)), before);
}

#[test]
fn opencode_v1_with_a_stale_id_starts_fresh_on_the_stored_port() {
    let fx = fx_with(harness(
        "h1",
        "opencode",
        Some(sv(&[
            "opencode",
            "--port",
            "4555",
            "--hostname",
            "127.0.0.1",
            "--session",
            "old",
        ])),
        Some("old"),
    ));
    fx.runner().set_stand_in(shell_cmd());
    fx.runner().set_session_probe(Arc::new(|_| Ok(false)));

    fx.ensure(false, None).unwrap();
    let argv = fx.runner().spawned_argv().pop().unwrap();
    assert_eq!(
        argv,
        sv(&["opencode", "--port", "4555", "--hostname", "127.0.0.1"])
    );
    assert!(!argv.iter().any(|a| a == "--session" || a == "--continue"));
    assert_eq!(stored(&fx).session_id, None);
}

/// Tested at `SpawnPlan::from_record`: a v2 spawn starts a sidecar, which the
/// stand-in cannot stand in for.
#[test]
fn opencode_v2_is_not_probed_and_keeps_its_id() {
    let fx = Fx::new();
    let mut h = harness(
        "h1",
        "opencode",
        Some(sv(&["opencode", "--port", "4555"])),
        Some("v2-sid"),
    );
    h.opencode_version = Some(OpencodeVersion {
        major: 2,
        version: "2.0.1".to_owned(),
    });
    let r = room("r1", &fx.work, vec![h.clone()]);
    let plan = SpawnPlan::from_record(&r, &h, HarnessKind::Opencode, None, &|_| {
        panic!("a v2 session is not probed")
    })
    .unwrap();
    assert_eq!(plan.session_id.as_deref(), Some("v2-sid"));
}

#[test]
fn hermes_with_a_stale_id_starts_without_resume_and_drops_it() {
    let fx = fx_with(harness(
        "h1",
        "hermes",
        Some(sv(&["hermes", "--tui"])),
        Some("old"),
    ));
    fx.runner().set_stand_in(shell_cmd());
    fx.runner().set_session_probe(Arc::new(|_| Ok(false)));

    fx.ensure(false, None).unwrap();
    let argv = fx.runner().spawned_argv().pop().unwrap();
    assert!(!argv.iter().any(|a| a == "--resume"), "{argv:?}");
    assert_eq!(stored(&fx).session_id, None);
}

#[test]
fn expect_launch_is_compared_to_the_record() {
    let mut h = harness("h1", "claude", Some(shell_cmd()), None);
    h.launch = Some(LaunchMode::Shell { argv: shell_cmd() });
    let fx = fx_with(h);
    let ask = |expect_launch| {
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
    // A shell record asked for as the tool, or as a different shell: stale.
    for wrong in [
        LaunchMode::Tool,
        LaunchMode::Shell {
            argv: sv(&["other"]),
        },
    ] {
        let err = ask(Some(wrong)).err().expect("refused");
        assert_eq!(err.code(), "stale_record");
        assert_eq!(fx.runner().spawned_argv().len(), 0);
    }
    let ok = ask(Some(LaunchMode::Shell { argv: shell_cmd() })).unwrap();
    assert!(ok.started);
    assert_eq!(fx.runner().spawned_argv(), vec![shell_cmd()]);
}

#[test]
fn a_tool_record_with_no_launch_accepts_an_expected_tool_and_refuses_a_shell() {
    let fx = fx_with(harness("h1", "byoh", Some(shell_cmd()), None));
    let ask = |expect_launch| {
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
    assert!(ask(Some(LaunchMode::Tool)).unwrap().started);
}

#[test]
fn the_stored_opencode_port_is_ignored_in_shell_mode_and_passed_in_tool_mode() {
    let cmd = sv(&["opencode", "--port", "4555", "--hostname", "127.0.0.1"]);
    let mut h = harness("h1", "opencode", Some(cmd.clone()), None);
    assert_eq!(stored_opencode_port(&h), Some(4555));
    h.launch = Some(LaunchMode::Tool);
    assert_eq!(stored_opencode_port(&h), Some(4555));
    h.launch = Some(LaunchMode::Shell { argv: cmd });
    assert_eq!(stored_opencode_port(&h), None);
}
