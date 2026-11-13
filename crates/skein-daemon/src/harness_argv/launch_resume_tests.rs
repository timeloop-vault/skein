//! #752: the resume builders and the restart path honour `launch`.

use skein_proto::rooms::{Harness, LaunchMode, ShellClaim};

use super::launch::{effective_shell_argv, infer_launch};
use super::test_util::{SID, no_mint, ports, room, sv, with};
use super::{ResumeOneInput, resume_harness, resume_one_with, with_resume_cmds};

fn shell_mode(argv: &[&str]) -> LaunchMode {
    LaunchMode::Shell { argv: sv(argv) }
}

fn claim(sid: &str) -> Option<ShellClaim> {
    serde_json::from_value(serde_json::json!({"sessionId": sid})).ok()
}

/// Legacy shape: shell argv in `cmd`, `launch` unset.
fn legacy(kind: &str, sid: Option<&str>) -> Harness {
    with(kind, &["pwsh.exe", "-NoLogo"], sid)
}

/// New shape: `cmd` is the tool's argv, `launch` says shell.
fn new_shape(kind: &str, sid: Option<&str>) -> Harness {
    let cmd: &[&str] = if kind == "claude" {
        &["claude", "--resume", "x"]
    } else {
        &["opencode", "--continue"]
    };
    Harness {
        launch: Some(shell_mode(&["pwsh.exe", "-NoLogo"])),
        ..with(kind, cmd, sid)
    }
}

#[test]
fn helper_sees_both_shapes_and_no_other() {
    let want = Some(sv(&["pwsh.exe", "-NoLogo"]));
    assert_eq!(effective_shell_argv(&legacy("claude", None)), want);
    assert_eq!(effective_shell_argv(&new_shape("claude", None)), want);
    assert_eq!(
        effective_shell_argv(&with("claude", &["claude"], None)),
        None
    );
    assert_eq!(effective_shell_argv(&with("claude", &[], None)), None);
}

#[test]
fn stale_or_no_claim_keeps_the_shell_and_migrates() {
    for h in [legacy("claude", Some("a")), legacy("opencode", Some("a"))] {
        let cmd = h.cmd.clone();
        let out = resume_harness(h, Some(1), no_mint);
        assert_eq!(out.cmd, cmd);
        assert_eq!(out.launch, Some(shell_mode(&["pwsh.exe", "-NoLogo"])));
    }
    let stale = Harness {
        shell_claim: claim("other"),
        ..legacy("claude", Some("a"))
    };
    let out = resume_harness(stale, None, no_mint);
    assert_eq!(out.launch, Some(shell_mode(&["pwsh.exe", "-NoLogo"])));
    assert!(out.shell_claim.is_none());
    assert_eq!(out.cmd, Some(sv(&["pwsh.exe", "-NoLogo"])));
}

#[test]
fn live_claim_returns_to_the_tool_and_clears_launch() {
    for h in [
        Harness {
            shell_claim: claim(SID),
            ..legacy("claude", Some(SID))
        },
        Harness {
            shell_claim: claim(SID),
            ..new_shape("claude", Some(SID))
        },
    ] {
        let out = resume_harness(h, None, no_mint);
        assert_eq!(out.cmd, Some(sv(&["claude", "--resume", SID])));
        assert_eq!(out.launch, None);
        assert!(out.shell_claim.is_none());
    }
}

#[test]
fn new_shape_without_claim_keeps_shell_and_cmd() {
    let h = new_shape("claude", Some("x"));
    let out = resume_harness(h.clone(), None, no_mint);
    assert_eq!(out.cmd, h.cmd);
    assert_eq!(out.launch, h.launch);
}

#[test]
fn empty_cmd_still_takes_the_shell_path() {
    let out = resume_harness(with("claude", &[], Some("a")), None, no_mint);
    assert_eq!(out.cmd, Some(vec![]));
    assert_eq!(out.launch, None);
}

#[test]
fn restart_returns_to_the_tool() {
    type Case<'a> = (&'a str, Harness, Option<u16>, Vec<String>, Option<&'a str>);
    let cases: Vec<Case> = vec![
        (
            "claude legacy with session",
            legacy("claude", Some("sid1")),
            None,
            sv(&["claude", "--resume", "sid1"]),
            Some("sid1"),
        ),
        (
            "claude new shape with session",
            new_shape("claude", Some("sid1")),
            None,
            sv(&["claude", "--resume", "sid1"]),
            Some("sid1"),
        ),
        (
            "claude legacy without session",
            legacy("claude", None),
            None,
            sv(&["claude", "--session-id", "minted"]),
            Some("minted"),
        ),
        (
            "opencode legacy with port",
            legacy("opencode", Some("o1")),
            Some(5000),
            sv(&[
                "opencode",
                "--port",
                "5000",
                "--hostname",
                "127.0.0.1",
                "--session",
                "o1",
            ]),
            Some("o1"),
        ),
        (
            "opencode new shape with port",
            new_shape("opencode", Some("o1")),
            Some(5000),
            sv(&[
                "opencode",
                "--port",
                "5000",
                "--hostname",
                "127.0.0.1",
                "--session",
                "o1",
            ]),
            Some("o1"),
        ),
    ];
    for (name, h, port, cmd, sid) in cases {
        let h = Harness {
            shell_claim: claim("whatever"),
            ..h
        };
        let out = resume_one_with(
            ResumeOneInput { harness: h },
            move || port.ok_or_else(|| "none".to_owned()),
            || "minted".to_owned(),
        );
        assert_eq!(out.harness.cmd, Some(cmd), "{name}");
        assert_eq!(out.harness.launch, None, "{name}");
        assert!(out.harness.shell_claim.is_none(), "{name}");
        assert_eq!(out.harness.session_id.as_deref(), sid, "{name}");
        assert_eq!(infer_launch(&out.harness), None, "{name}");
    }
}

#[test]
fn restart_leaves_a_tool_harness_alone() {
    let h = with("claude", &["claude", "--resume", "s"], Some("s"));
    let out = resume_one_with(
        ResumeOneInput { harness: h },
        || Err("x".into()),
        || "m".into(),
    );
    assert_eq!(out.harness.cmd, Some(sv(&["claude", "--resume", "s"])));
    assert_eq!(out.harness.launch, None);
}

#[test]
fn room_wide_resume_keeps_the_shell_for_both_shapes() {
    let r = room(vec![
        Harness {
            id: "a".into(),
            ..legacy("claude", Some("s"))
        },
        Harness {
            id: "b".into(),
            ..new_shape("claude", Some("s"))
        },
    ]);
    let out = with_resume_cmds(r, &ports(&[]), no_mint);
    assert_eq!(out.harnesses[0].cmd, Some(sv(&["pwsh.exe", "-NoLogo"])));
    assert_eq!(
        out.harnesses[0].launch,
        Some(shell_mode(&["pwsh.exe", "-NoLogo"]))
    );
    assert_eq!(out.harnesses[1].cmd, Some(sv(&["claude", "--resume", "x"])));
    assert_eq!(
        out.harnesses[1].launch,
        Some(shell_mode(&["pwsh.exe", "-NoLogo"]))
    );
}
