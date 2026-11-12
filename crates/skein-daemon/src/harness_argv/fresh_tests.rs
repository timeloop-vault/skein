//! Ports of the fresh-spawn cases of `app/src/harnessCmd.test.ts`:
//! `cmdForKind`, hermes profile (#627), hermes (#623), `cmdForKind - agent`.

use skein_proto::harness_kind::HarnessKind;

use super::test_util::{SID, harness, no_mint, shell, sv, with};
use super::{CmdParams, cmd_for_kind, resolve_hermes_profile, resume_cmd, resume_harness};

fn fresh(kind: HarnessKind, p: &CmdParams<'_>) -> Vec<String> {
    cmd_for_kind(kind, &shell(), p)
}

fn p() -> CmdParams<'static> {
    CmdParams::default()
}

#[test]
fn pre_allocates_claudes_session_id_when_given_one() {
    let with_sid = CmdParams {
        session_id: Some(SID),
        ..p()
    };
    assert_eq!(
        fresh(HarnessKind::Claude, &with_sid),
        sv(&["claude", "--session-id", SID])
    );
    assert_eq!(fresh(HarnessKind::Claude, &p()), sv(&["claude"]));
}

#[test]
fn pins_opencodes_port_and_degrades_without_one() {
    let port = CmdParams {
        opencode_port: Some(4096),
        ..p()
    };
    assert_eq!(
        fresh(HarnessKind::Opencode, &port),
        sv(&["opencode", "--port", "4096", "--hostname", "127.0.0.1"])
    );
    assert_eq!(fresh(HarnessKind::Opencode, &p()), sv(&["opencode"]));
}

#[test]
fn builds_the_non_resumable_kinds() {
    assert_eq!(
        fresh(HarnessKind::Copilot, &p()),
        sv(&["gh", "copilot", "suggest"])
    );
    assert_eq!(fresh(HarnessKind::Byoh, &p()), shell());
    assert_eq!(
        cmd_for_kind(HarnessKind::Byoh, &[], &p()),
        sv(&["pwsh.exe"])
    );
    assert_eq!(fresh(HarnessKind::Files, &p()), Vec::<String>::new());
    assert_eq!(fresh(HarnessKind::Design, &p()), Vec::<String>::new());
}

#[test]
fn hermes_cmd_passes_p_only_for_a_non_blank_profile() {
    let skein = CmdParams {
        hermes_profile: Some("skein"),
        ..p()
    };
    assert_eq!(
        fresh(HarnessKind::Hermes, &skein),
        sv(&["hermes", "--tui", "-p", "skein"])
    );
    let blank = CmdParams {
        hermes_profile: Some("  "),
        ..p()
    };
    assert_eq!(fresh(HarnessKind::Hermes, &blank), sv(&["hermes", "--tui"]));
}

#[test]
fn hermes_resume_no_profile_profile_no_sid_profile_and_sid() {
    let base = || with("hermes", &["hermes", "--tui"], None);
    assert_eq!(resume_cmd(base(), None), sv(&["hermes", "--tui"]));
    let mut named = base();
    named.hermes_profile = Some("work".to_owned());
    assert_eq!(
        resume_cmd(named, None),
        sv(&["hermes", "--tui", "-p", "work"])
    );
    let mut h = base();
    h.hermes_profile = Some("work".to_owned());
    h.session_id = Some(SID.to_owned());
    let once = resume_harness(h, None, no_mint);
    assert_eq!(
        once.cmd,
        Some(sv(&["hermes", "--tui", "-p", "work", "--resume", SID]))
    );
    let twice = resume_harness(once.clone(), None, no_mint);
    assert_eq!(twice.cmd, once.cmd);
    assert_eq!(twice.session_id, once.session_id);
}

#[test]
fn resolve_hermes_profile_defaults_to_skein_for_hermes_only() {
    let r = |k, p| resolve_hermes_profile(k, p);
    assert_eq!(r(HarnessKind::Hermes, None).as_deref(), Some("skein"));
    assert_eq!(r(HarnessKind::Hermes, Some(" ")).as_deref(), Some("skein"));
    assert_eq!(
        r(HarnessKind::Hermes, Some(" work ")).as_deref(),
        Some("work")
    );
    assert_eq!(r(HarnessKind::Claude, Some("work")), None);
}

#[test]
fn hermes_spawns_the_tui_explicitly() {
    assert_eq!(fresh(HarnessKind::Hermes, &p()), sv(&["hermes", "--tui"]));
}

#[test]
fn hermes_resumes_the_stored_session_ignoring_any_agent() {
    let mut h = with("hermes", &["hermes", "--tui"], Some(SID));
    h.agent = Some("x".to_owned());
    let want = sv(&["hermes", "--tui", "--resume", SID]);
    assert_eq!(resume_cmd(h.clone(), None), want);
    let out = resume_harness(h.clone(), None, no_mint);
    assert_eq!(out.cmd, Some(want));
    assert_eq!(out.agent, h.agent);
    assert_eq!(out.session_id, h.session_id);
}

#[test]
fn hermes_without_a_session_id_rebuilds_fresh_and_mints_nothing() {
    let h = with("hermes", &["hermes", "--tui", "--resume", SID], None);
    let out = resume_harness(h, None, no_mint);
    assert_eq!(out.cmd, Some(sv(&["hermes", "--tui"])));
    assert_eq!(out.session_id, None);
}

#[test]
fn hermes_is_rewritten_by_with_resume_cmds_from_its_record() {
    use super::test_util::room;
    use super::with_resume_cmds;
    use std::collections::HashMap;
    let r = room(vec![with("hermes", &["hermes", "--tui"], Some(SID))]);
    let out = with_resume_cmds(r, &HashMap::<String, u16>::default(), no_mint);
    assert_eq!(
        out.harnesses[0].cmd,
        Some(sv(&["hermes", "--tui", "--resume", SID]))
    );
}

#[test]
fn appends_agent_for_the_kinds_that_take_one() {
    let claude = CmdParams {
        session_id: Some(SID),
        agent: Some("reviewer"),
        ..p()
    };
    assert_eq!(
        fresh(HarnessKind::Claude, &claude),
        sv(&["claude", "--session-id", SID, "--agent", "reviewer"])
    );
    // No session id is the picker-fallback shape; the agent still applies.
    let claude_no_sid = CmdParams {
        agent: Some("reviewer"),
        ..p()
    };
    assert_eq!(
        fresh(HarnessKind::Claude, &claude_no_sid),
        sv(&["claude", "--agent", "reviewer"])
    );
    let oc = CmdParams {
        opencode_port: Some(4096),
        agent: Some("plan"),
        ..p()
    };
    assert_eq!(
        fresh(HarnessKind::Opencode, &oc),
        sv(&[
            "opencode",
            "--port",
            "4096",
            "--hostname",
            "127.0.0.1",
            "--agent",
            "plan"
        ])
    );
    // Port allocation failed: the agent is not collateral damage.
    let oc_no_port = CmdParams {
        agent: Some("plan"),
        ..p()
    };
    assert_eq!(
        fresh(HarnessKind::Opencode, &oc_no_port),
        sv(&["opencode", "--agent", "plan"])
    );
}

#[test]
fn omits_the_flag_entirely_with_no_agent() {
    let claude = CmdParams {
        session_id: Some(SID),
        ..p()
    };
    assert!(!fresh(HarnessKind::Claude, &claude).contains(&"--agent".to_owned()));
    let oc = CmdParams {
        opencode_port: Some(4096),
        ..p()
    };
    assert!(!fresh(HarnessKind::Opencode, &oc).contains(&"--agent".to_owned()));
}

#[test]
fn treats_a_blank_agent_as_none_not_an_empty_flag() {
    let blank = CmdParams {
        session_id: Some(SID),
        agent: Some("   "),
        ..p()
    };
    assert_eq!(
        fresh(HarnessKind::Claude, &blank),
        sv(&["claude", "--session-id", SID])
    );
}

#[test]
fn passes_the_untrimmed_agent_name_through() {
    let padded = CmdParams {
        agent: Some(" x "),
        ..p()
    };
    assert_eq!(
        fresh(HarnessKind::Claude, &padded),
        sv(&["claude", "--agent", " x "])
    );
}

#[test]
fn a_harness_of_an_unknown_kind_passes_through_resume_unchanged() {
    let h = with("something-new", &["x"], Some(SID));
    let out = resume_harness(h, Some(1), no_mint);
    assert_eq!(out.cmd, Some(sv(&["x"])));
    // No cmd stays no cmd: unknown kinds are returned as they came.
    let bare = resume_harness(harness("something-new"), None, no_mint);
    assert_eq!(bare.cmd, None);
}
