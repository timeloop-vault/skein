//! Ports of the `shellClaim (#520)` cases of `app/src/harnessCmd.test.ts`,
//! opencode (#517) arm included.

use skein_proto::rooms::{Harness, ShellClaim};

use super::test_util::{SID, harness, no_mint, ports, room, shell, sv, with};
use super::{resume_harness, unarchive_room_transform, with_resume_cmds};

fn claim(sid: &str, port: Option<u16>) -> ShellClaim {
    ShellClaim {
        session_id: sid.to_owned(),
        port,
    }
}

fn claimed() -> Harness {
    Harness {
        cmd: Some(shell()),
        session_id: Some(SID.to_owned()),
        shell_claim: Some(claim(SID, None)),
        ..harness("claude")
    }
}

#[test]
fn a_live_claim_puts_the_shell_harness_back_on_claude_resume_and_is_consumed() {
    let out = resume_harness(claimed(), None, no_mint);
    assert_eq!(out.cmd, Some(sv(&["claude", "--resume", SID])));
    assert!(out.shell_claim.is_none());
}

#[test]
fn re_passes_agent_for_a_live_claim() {
    let h = Harness {
        agent: Some("reviewer".to_owned()),
        ..claimed()
    };
    assert_eq!(
        resume_harness(h, None, no_mint).cmd,
        Some(sv(&["claude", "--resume", SID, "--agent", "reviewer"]))
    );
}

#[test]
fn a_claim_that_no_longer_matches_session_id_restores_the_shell() {
    let h = Harness {
        session_id: Some("other".to_owned()),
        ..claimed()
    };
    let out = resume_harness(h, None, no_mint);
    assert_eq!(out.cmd, Some(shell()));
    assert!(out.shell_claim.is_none());
}

#[test]
fn a_claim_with_no_session_id_restores_the_shell() {
    let h = Harness {
        session_id: None,
        ..claimed()
    };
    let out = resume_harness(h, None, no_mint);
    assert_eq!(out.cmd, Some(shell()));
    assert!(out.shell_claim.is_none());
}

#[test]
fn an_empty_claim_session_id_never_counts_as_live() {
    let h = Harness {
        session_id: Some(String::new()),
        shell_claim: Some(claim("", None)),
        ..claimed()
    };
    let out = resume_harness(h, None, no_mint);
    assert_eq!(out.cmd, Some(shell()));
    assert!(out.shell_claim.is_none());
}

#[test]
fn no_claim_keeps_the_shell() {
    let h = with("claude", &["/bin/zsh", "-l"], Some(SID));
    assert_eq!(resume_harness(h, None, no_mint).cmd, Some(shell()));
}

#[test]
fn strips_a_claim_from_a_harness_already_on_claude() {
    let h = Harness {
        cmd: Some(sv(&["claude", "--resume", SID])),
        ..claimed()
    };
    let out = resume_harness(h, None, no_mint);
    assert_eq!(out.cmd, Some(sv(&["claude", "--resume", SID])));
    assert!(out.shell_claim.is_none());
}

#[test]
fn leaves_kinds_without_a_claim_arm_copilot_untouched() {
    let h = Harness {
        cmd: Some(shell()),
        shell_claim: Some(claim("ses_1", Some(1))),
        ..harness("copilot")
    };
    let out = resume_harness(h, Some(5), no_mint);
    assert_eq!(out.shell_claim, Some(claim("ses_1", Some(1))));
}

fn oc() -> Harness {
    Harness {
        cmd: Some(shell()),
        session_id: Some("ses_1".to_owned()),
        shell_claim: Some(claim("ses_1", Some(1))),
        ..harness("opencode")
    }
}

#[test]
fn opencode_a_live_claim_resumes_on_the_fresh_port_and_is_consumed() {
    let out = resume_harness(oc(), Some(5), no_mint);
    assert_eq!(
        out.cmd,
        Some(sv(&[
            "opencode",
            "--port",
            "5",
            "--hostname",
            "127.0.0.1",
            "--session",
            "ses_1"
        ]))
    );
    assert!(out.shell_claim.is_none());
}

#[test]
fn opencode_ignores_the_persisted_claim_port() {
    let cmd = resume_harness(oc(), Some(9), no_mint).cmd.unwrap();
    assert!(!cmd.contains(&"1".to_owned()));
}

#[test]
fn opencode_re_passes_agent() {
    let h = Harness {
        agent: Some("build".to_owned()),
        ..oc()
    };
    assert_eq!(
        resume_harness(h, Some(5), no_mint).cmd,
        Some(sv(&[
            "opencode",
            "--port",
            "5",
            "--hostname",
            "127.0.0.1",
            "--session",
            "ses_1",
            "--agent",
            "build"
        ]))
    );
}

#[test]
fn opencode_no_port_passed_resumes_without_port() {
    assert_eq!(
        resume_harness(oc(), None, no_mint).cmd,
        Some(sv(&["opencode", "--session", "ses_1"]))
    );
}

#[test]
fn opencode_a_mismatched_session_id_restores_the_shell() {
    let h = Harness {
        session_id: Some("other".to_owned()),
        ..oc()
    };
    let out = resume_harness(h, Some(5), no_mint);
    assert_eq!(out.cmd, Some(shell()));
    assert!(out.shell_claim.is_none());
}

#[test]
fn opencode_a_missing_session_id_restores_the_shell() {
    let h = Harness {
        session_id: None,
        ..oc()
    };
    let out = resume_harness(h, Some(5), no_mint);
    assert_eq!(out.cmd, Some(shell()));
    assert!(out.shell_claim.is_none());
}

#[test]
fn opencode_with_resume_cmds_is_idempotent_for_a_claim() {
    let p = ports(&[("h1", 5)]);
    let once = with_resume_cmds(room(vec![oc()]), &p, no_mint);
    assert_eq!(once.harnesses[0].cmd.as_ref().unwrap()[0], "opencode");
    assert!(once.harnesses[0].shell_claim.is_none());
    let twice = with_resume_cmds(once.clone(), &p, no_mint);
    assert_eq!(twice.harnesses[0].cmd, once.harnesses[0].cmd);
    assert_eq!(
        twice.harnesses[0].shell_claim,
        once.harnesses[0].shell_claim
    );
}

#[test]
fn with_resume_cmds_and_unarchive_carry_it_idempotently() {
    let r = room(vec![claimed()]);
    let want = Some(sv(&["claude", "--resume", SID]));
    let via_resume = with_resume_cmds(r.clone(), &ports(&[]), no_mint);
    assert_eq!(via_resume.harnesses[0].cmd, want);
    let mut archived = r;
    archived.archived = Some(1);
    let via_unarchive = unarchive_room_transform(archived, &ports(&[]), no_mint);
    assert_eq!(via_unarchive.harnesses[0].cmd, want);
    assert!(via_unarchive.harnesses[0].shell_claim.is_none());
    let again = with_resume_cmds(via_unarchive.clone(), &ports(&[]), no_mint);
    assert_eq!(again.harnesses[0].cmd, via_unarchive.harnesses[0].cmd);
}
