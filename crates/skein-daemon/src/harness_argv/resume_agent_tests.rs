//! Ports of `resumeCmd - agent` and the #247 round-trip cases of
//! `app/src/harnessCmd.test.ts`.

use skein_proto::harness_kind::HarnessKind;
use skein_proto::rooms::Harness;

use super::test_util::{SID, no_mint, ports, room, shell, sv, with};
use super::{CmdParams, cmd_for_kind, resume_cmd, unarchive_room_transform, with_resume_cmds};

fn agent_h(kind: &str, cmd: &[&str], sid: Option<&str>, agent: &str) -> Harness {
    Harness {
        agent: Some(agent.to_owned()),
        ..with(kind, cmd, sid)
    }
}

#[test]
fn re_passes_the_records_agent_for_claude() {
    // `claude --resume <sid>` alone restores the agent the session
    // *started* as, so a record that says otherwise has to say it out
    // loud on every resume.
    let h = agent_h(
        "claude",
        &["claude", "--session-id", SID, "--agent", "reviewer"],
        Some(SID),
        "reviewer",
    );
    assert_eq!(
        resume_cmd(h, None),
        sv(&["claude", "--resume", SID, "--agent", "reviewer"])
    );
}

#[test]
fn re_passes_the_records_agent_for_opencode_with_a_fresh_port() {
    let h = agent_h(
        "opencode",
        &[
            "opencode",
            "--port",
            "4096",
            "--hostname",
            "127.0.0.1",
            "--agent",
            "plan",
        ],
        Some("ses_abc"),
        "plan",
    );
    assert_eq!(
        resume_cmd(h, Some(5150)),
        sv(&[
            "opencode",
            "--port",
            "5150",
            "--hostname",
            "127.0.0.1",
            "--session",
            "ses_abc",
            "--agent",
            "plan"
        ])
    );
}

#[test]
fn is_idempotent_with_an_agent_so_every_boot_can_run_it() {
    let h = agent_h("claude", &["claude"], Some(SID), "reviewer");
    let once = resume_cmd(h.clone(), None);
    let again = Harness {
        cmd: Some(once.clone()),
        ..h
    };
    assert_eq!(resume_cmd(again, None), once);
}

#[test]
fn keeps_a_shell_swapped_harness_on_its_shell_agent_or_not() {
    // The record outlives the process it described: Enter-for-shell
    // leaves `agent` set, and the shell must not grow a flag it does
    // not understand.
    let h = agent_h("claude", &["pwsh.exe"], Some(SID), "reviewer");
    assert_eq!(resume_cmd(h, None), sv(&["pwsh.exe"]));
}

#[test]
fn ignores_an_agent_on_a_kind_that_has_no_agent_concept() {
    let copilot = agent_h("copilot", &["gh", "copilot", "suggest"], None, "reviewer");
    assert_eq!(resume_cmd(copilot, None), sv(&["gh", "copilot", "suggest"]));
    let byoh = agent_h("byoh", &["/bin/zsh", "-l"], None, "reviewer");
    assert_eq!(resume_cmd(byoh, None), shell());
}

fn fresh_h(kind: HarnessKind, agent: &str) -> Harness {
    let claude = kind == HarnessKind::Claude;
    let params = CmdParams {
        session_id: claude.then_some(SID),
        opencode_port: Some(4096),
        agent: Some(agent),
        ..CmdParams::default()
    };
    let cmd = cmd_for_kind(kind, &shell(), &params);
    Harness {
        cmd: Some(cmd),
        session_id: Some(if claude { SID } else { "ses_abc" }.to_owned()),
        agent: Some(agent.to_owned()),
        ..with(kind.as_str(), &[], None)
    }
}

#[test]
fn the_selection_survives_a_skein_restart_for_claude_and_opencode() {
    for (kind, agent) in [
        (HarnessKind::Claude, "reviewer"),
        (HarnessKind::Opencode, "plan"),
    ] {
        let out = with_resume_cmds(
            room(vec![fresh_h(kind, agent)]),
            &ports(&[("h1", 5150)]),
            no_mint,
        );
        let cmd = out.harnesses[0].cmd.clone().unwrap_or_default();
        let at = cmd.iter().position(|a| a == "--agent");
        assert!(at.is_some(), "{kind}: {cmd:?}");
        assert_eq!(cmd[at.unwrap() + 1], agent, "{kind}");
        // The #153 guard, restated where the agent could reintroduce it:
        // a resumed argv must never carry --session-id.
        assert!(!cmd.contains(&"--session-id".to_owned()), "{kind}");
        assert_eq!(out.harnesses[0].agent.as_deref(), Some(agent), "{kind}");
    }
}

#[test]
fn the_selection_survives_archive_then_reopen() {
    let mut r = room(vec![fresh_h(HarnessKind::Claude, "reviewer")]);
    r.archived = Some(1_700_000_000_000);
    let out = unarchive_room_transform(r, &ports(&[]), no_mint);
    assert_eq!(
        out.harnesses[0].cmd,
        Some(sv(&["claude", "--resume", SID, "--agent", "reviewer"]))
    );
    assert_eq!(out.harnesses[0].agent.as_deref(), Some("reviewer"));
}
