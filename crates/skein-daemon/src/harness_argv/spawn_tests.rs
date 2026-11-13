use std::cell::Cell;

use skein_proto::harness_kind::HarnessKind;
use skein_proto::rooms::{Harness, LaunchMode, RemoteSpec};

use super::spawn::{SpawnArgv, spawn_argv};
use super::test_util::{SID, harness, sv, with};
use super::{CmdParams, cmd_for_kind, remote_argv};

type OpencodeCase<'a> = (
    &'a str,
    Option<&'a str>,
    bool,
    Option<u16>,
    Vec<&'a str>,
    Option<&'a str>,
);

const NEW: &str = "11111111-2222-3333-4444-555555555555";

fn run(h: &Harness, kind: &str, exists: bool, port: Option<u16>) -> Option<SpawnArgv> {
    spawn_argv(
        h,
        HarnessKind::from_name(kind).unwrap(),
        || exists,
        port,
        || NEW.to_owned(),
    )
}

#[allow(clippy::unnecessary_wraps)]
fn out(cmd: &[&str], sid: Option<&str>) -> Option<SpawnArgv> {
    Some(SpawnArgv {
        cmd: sv(cmd),
        session_id: sid.map(str::to_owned),
    })
}

fn shell_launch(h: Harness, argv: &[&str]) -> Harness {
    Harness {
        launch: Some(LaunchMode::Shell { argv: sv(argv) }),
        ..h
    }
}

fn with_agent_name(h: Harness, a: &str) -> Harness {
    Harness {
        agent: Some(a.to_owned()),
        ..h
    }
}

fn spec() -> RemoteSpec {
    RemoteSpec {
        host: "user@example-host".to_owned(),
        tool: "opencode".to_owned(),
        session: "skein-r1-h1".to_owned(),
        dir: None,
    }
}

#[test]
fn shell_launch_runs_its_argv_and_never_probes() {
    for kind in ["claude", "opencode", "hermes"] {
        let h = shell_launch(with(kind, &[kind], Some(SID)), &["pwsh.exe", "-NoLogo"]);
        let got = spawn_argv(
            &h,
            HarnessKind::from_name(kind).unwrap(),
            || panic!("must not probe"),
            Some(1),
            || panic!("must not mint"),
        );
        assert_eq!(got, out(&["pwsh.exe", "-NoLogo"], Some(SID)), "{kind}");
    }
}

#[test]
fn shell_launch_with_empty_argv_is_nothing() {
    for argv in [&[][..], &[""][..]] {
        let h = shell_launch(with("claude", &["claude"], None), argv);
        assert_eq!(run(&h, "claude", true, None), None);
    }
}

#[test]
fn claude_table() {
    let cases = [
        (
            "no id",
            None,
            true,
            &["claude", "--session-id", NEW][..],
            NEW,
        ),
        (
            "blank id",
            Some("  "),
            true,
            &["claude", "--session-id", NEW],
            NEW,
        ),
        (
            "id exists",
            Some(SID),
            true,
            &["claude", "--resume", SID],
            SID,
        ),
        (
            "id gone",
            Some(SID),
            false,
            &["claude", "--session-id", SID],
            SID,
        ),
    ];
    for (name, sid, exists, argv, want_sid) in cases {
        let h = with("claude", &["claude"], sid);
        assert_eq!(
            run(&h, "claude", exists, None),
            out(argv, Some(want_sid)),
            "{name}"
        );
        let h = with_agent_name(h, "reviewer");
        let mut with_agent = sv(argv);
        with_agent.extend(sv(&["--agent", "reviewer"]));
        assert_eq!(
            run(&h, "claude", exists, None),
            Some(SpawnArgv {
                cmd: with_agent,
                session_id: Some(want_sid.to_owned())
            }),
            "{name} + agent"
        );
    }
}

#[test]
fn claude_does_not_probe_without_an_id_and_does_not_mint_with_one() {
    let h = with("claude", &["claude"], None);
    let got = spawn_argv(
        &h,
        HarnessKind::Claude,
        || panic!("must not probe"),
        None,
        || NEW.to_owned(),
    );
    assert_eq!(got.unwrap().session_id.as_deref(), Some(NEW));
    let h = with("claude", &["claude"], Some(SID));
    let probes = Cell::new(0);
    let got = spawn_argv(
        &h,
        HarnessKind::Claude,
        || {
            probes.set(probes.get() + 1);
            true
        },
        None,
        || panic!("must not mint"),
    );
    assert!(got.is_some());
    assert_eq!(probes.get(), 1);
}

#[test]
fn stored_cmd_does_not_decide_resume() {
    // #744 shape: stored `--session-id X`, transcript exists -> resume.
    let h = with("claude", &["claude", "--session-id", SID], Some(SID));
    assert_eq!(
        run(&h, "claude", true, None),
        out(&["claude", "--resume", SID], Some(SID))
    );
    // Stored `--resume X`, transcript gone -> fresh with the same id.
    let h = with("claude", &["claude", "--resume", SID], Some(SID));
    assert_eq!(
        run(&h, "claude", false, None),
        out(&["claude", "--session-id", SID], Some(SID))
    );
}

#[test]
fn shell_looking_cmd_without_launch_is_still_the_tool() {
    let h = with("claude", &["pwsh.exe"], Some(SID));
    assert_eq!(
        run(&h, "claude", true, None),
        out(&["claude", "--resume", SID], Some(SID))
    );
    let h = with("opencode", &["/bin/zsh", "-l"], None);
    assert_eq!(
        run(&h, "opencode", true, Some(7)).unwrap().cmd[0],
        "opencode"
    );
}

#[test]
fn opencode_table() {
    let port = ["--port", "4100", "--hostname", "127.0.0.1"];
    let cases: Vec<OpencodeCase> = vec![
        (
            "no id",
            None,
            true,
            Some(4100),
            [&["opencode"][..], &port].concat(),
            None,
        ),
        (
            "blank id",
            Some(" "),
            true,
            Some(4100),
            [&["opencode"][..], &port].concat(),
            None,
        ),
        (
            "id exists",
            Some("ses_1"),
            true,
            Some(4100),
            [&["opencode"][..], &port, &["--session", "ses_1"]].concat(),
            Some("ses_1"),
        ),
        (
            "id gone",
            Some("ses_1"),
            false,
            Some(4100),
            [&["opencode"][..], &port].concat(),
            None,
        ),
        (
            "no port, id exists",
            Some("ses_1"),
            true,
            None,
            vec!["opencode", "--session", "ses_1"],
            Some("ses_1"),
        ),
        ("no port, no id", None, true, None, vec!["opencode"], None),
    ];
    for (name, sid, exists, p, argv, want_sid) in cases {
        let h = with("opencode", &["opencode"], sid);
        assert_eq!(
            run(&h, "opencode", exists, p),
            out(&argv, want_sid),
            "{name}"
        );
        let mut a = sv(&argv);
        a.extend(sv(&["--agent", "build"]));
        let h = with_agent_name(h, "build");
        assert_eq!(
            run(&h, "opencode", exists, p).map(|s| s.cmd),
            Some(a),
            "{name} + agent"
        );
    }
}

#[test]
fn opencode_never_continues() {
    let h = with("opencode", &["opencode", "--continue"], None);
    let cmd = run(&h, "opencode", true, Some(1)).unwrap().cmd;
    assert!(!cmd.iter().any(|a| a == "--continue"));
}

#[test]
fn hermes_table() {
    let cases = [
        (
            "no id",
            None,
            true,
            &["hermes", "--tui", "-p", "skein"][..],
            None,
        ),
        (
            "blank",
            Some(""),
            true,
            &["hermes", "--tui", "-p", "skein"],
            None,
        ),
        (
            "exists",
            Some("h_1"),
            true,
            &["hermes", "--tui", "-p", "skein", "--resume", "h_1"],
            Some("h_1"),
        ),
        (
            "gone",
            Some("h_1"),
            false,
            &["hermes", "--tui", "-p", "skein"],
            None,
        ),
    ];
    for (name, sid, exists, argv, want) in cases {
        let h = Harness {
            hermes_profile: Some("skein".to_owned()),
            ..with("hermes", &["hermes"], sid)
        };
        assert_eq!(run(&h, "hermes", exists, None), out(argv, want), "{name}");
    }
    let h = with("hermes", &["hermes"], None);
    assert_eq!(
        run(&h, "hermes", true, None),
        out(&["hermes", "--tui"], None)
    );
}

#[test]
fn remote_uses_spec_else_stored_cmd_and_never_probes() {
    let want = remote_argv(&spec()).unwrap();
    let h = Harness {
        remote: Some(spec()),
        ..with("remote", &["stale"], Some("x"))
    };
    let got = spawn_argv(
        &h,
        HarnessKind::Remote,
        || panic!("must not probe"),
        None,
        || panic!("must not mint"),
    );
    assert_eq!(
        got,
        Some(SpawnArgv {
            cmd: want,
            session_id: Some("x".to_owned())
        })
    );
    let h = with("remote", &["ssh", "box"], None);
    assert_eq!(run(&h, "remote", true, None), out(&["ssh", "box"], None));
    assert_eq!(run(&harness("remote"), "remote", true, None), None);
}

#[test]
fn other_kinds_keep_stored_cmd_and_never_probe() {
    for kind in ["byoh", "copilot", "files", "design"] {
        let h = with(kind, &["pwsh.exe", "-x"], Some("keep"));
        let got = spawn_argv(
            &h,
            HarnessKind::from_name(kind).unwrap(),
            || panic!("must not probe"),
            Some(1),
            || panic!("must not mint"),
        );
        assert_eq!(got, out(&["pwsh.exe", "-x"], Some("keep")), "{kind}");
        assert_eq!(run(&harness(kind), kind, true, None), None, "{kind} absent");
        assert_eq!(
            run(&with(kind, &[], None), kind, true, None),
            None,
            "{kind} empty"
        );
    }
}

#[test]
fn fresh_forms_equal_cmd_for_kind() {
    let fallback = sv(&["pwsh.exe"]);
    for agent in [None, Some("a1")] {
        let base = |kind: &str| Harness {
            agent: agent.map(str::to_owned),
            hermes_profile: Some("skein".to_owned()),
            ..with(kind, &[kind], None)
        };
        // Claude, no id and id-not-exists.
        for stored in [None, Some(NEW)] {
            let h = Harness {
                session_id: stored.map(str::to_owned),
                ..base("claude")
            };
            let want = cmd_for_kind(
                HarnessKind::Claude,
                &fallback,
                &CmdParams {
                    session_id: Some(NEW),
                    agent,
                    ..CmdParams::default()
                },
            );
            assert_eq!(run(&h, "claude", false, None).unwrap().cmd, want);
        }
        for port in [None, Some(4100)] {
            let want = cmd_for_kind(
                HarnessKind::Opencode,
                &fallback,
                &CmdParams {
                    opencode_port: port,
                    agent,
                    ..CmdParams::default()
                },
            );
            assert_eq!(
                run(&base("opencode"), "opencode", false, port).unwrap().cmd,
                want
            );
        }
        let want = cmd_for_kind(
            HarnessKind::Hermes,
            &fallback,
            &CmdParams {
                hermes_profile: Some("skein"),
                ..CmdParams::default()
            },
        );
        assert_eq!(
            run(&base("hermes"), "hermes", false, None).unwrap().cmd,
            want
        );
    }
}
