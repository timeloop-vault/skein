//! Ports of `app/src/harnessCmdV2.test.ts`.

use skein_proto::harness_kind::HarnessKind;
use skein_proto::rooms::{Harness, OpencodeVersion};

use super::test_util::{harness, sv};
use super::{
    CmdParams, OpencodeMode, cmd_for_kind, is_opencode_v2, opencode_spawn_plan, opencode_v2_argv,
    resume_cmd,
};

fn v(major: u32) -> OpencodeVersion {
    OpencodeVersion {
        major,
        version: format!("{major}.0.0"),
    }
}

fn oc(cmd: &[&str], sid: Option<&str>, agent: Option<&str>) -> Harness {
    Harness {
        cmd: Some(sv(cmd)),
        session_id: sid.map(str::to_owned),
        agent: agent.map(str::to_owned),
        ..harness("opencode")
    }
}

#[test]
fn v2_argv_shape_has_no_agent() {
    assert_eq!(
        opencode_v2_argv("http://127.0.0.1:5000", "ses_1"),
        sv(&[
            "opencode",
            "--server",
            "http://127.0.0.1:5000",
            "--session",
            "ses_1"
        ])
    );
}

#[test]
fn is_opencode_v2_is_major_2_or_later_only() {
    assert!(is_opencode_v2(Some(&v(2))));
    assert!(is_opencode_v2(Some(&v(3))));
    assert!(!is_opencode_v2(Some(&v(1))));
    assert!(!is_opencode_v2(None));
}

// The plan never rewrites argv: v1 and unknown versions keep exactly what
// cmd_for_kind / resume_cmd produced.
fn cmds() -> Vec<Vec<String>> {
    let sh = sv(&["sh"]);
    let port = |agent| CmdParams {
        opencode_port: Some(4100),
        agent,
        ..CmdParams::default()
    };
    vec![
        cmd_for_kind(HarnessKind::Opencode, &sh, &port(None)),
        cmd_for_kind(HarnessKind::Opencode, &sh, &port(Some("build"))),
        resume_cmd(oc(&["opencode"], Some("ses_1"), None), Some(4100)),
        resume_cmd(oc(&["opencode"], None, None), Some(4100)),
        resume_cmd(oc(&["opencode"], Some("ses_1"), Some("plan")), Some(4100)),
    ]
}

#[test]
fn v1_and_null_versions_plan_v1_for_every_stored_argv_shape() {
    for cmd in cmds() {
        assert_eq!(
            opencode_spawn_plan(&cmd, HarnessKind::Opencode, Some(&v(1))),
            OpencodeMode::V1
        );
        assert_eq!(
            opencode_spawn_plan(&cmd, HarnessKind::Opencode, None),
            OpencodeMode::V1
        );
    }
}

#[test]
fn v2_plans_v2_for_an_opencode_owned_argv() {
    for cmd in cmds() {
        assert_eq!(
            opencode_spawn_plan(&cmd, HarnessKind::Opencode, Some(&v(2))),
            OpencodeMode::V2
        );
    }
}

#[test]
fn a_shell_cmd_or_a_non_opencode_kind_is_never_v2() {
    assert_eq!(
        opencode_spawn_plan(&sv(&["pwsh.exe"]), HarnessKind::Opencode, Some(&v(2))),
        OpencodeMode::V1
    );
    assert_eq!(
        opencode_spawn_plan(&sv(&["claude"]), HarnessKind::Claude, Some(&v(2))),
        OpencodeMode::V1
    );
    assert_eq!(
        opencode_spawn_plan(&[], HarnessKind::Opencode, Some(&v(2))),
        OpencodeMode::V1
    );
}
