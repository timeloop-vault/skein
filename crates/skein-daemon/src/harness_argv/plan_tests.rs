//! The plan entry points with an injected session minter. None of them
//! carries an opencode port (#754).

use skein_proto::harness_kind::HarnessKind;
use skein_proto::rooms::{Harness, RemoteSpec, ShellClaim};

use super::plan::{
    FreshCmdInput, FreshCmdOutput, ResumeOneInput, ResumeRoomsInput, fresh_cmd_with,
    resume_one_with, resume_rooms_with,
};
use super::test_util::{SID, harness, no_mint, room, shell, sv, with};

fn input(kind: HarnessKind) -> FreshCmdInput {
    FreshCmdInput {
        kind,
        session_id: None,
        new_harness: false,
        agent: None,
        remote: None,
        hermes_profile: None,
    }
}

fn fresh(i: &FreshCmdInput) -> FreshCmdOutput {
    fresh_cmd_with(i, &shell(), || "minted".to_owned())
}

#[test]
fn fresh_claude_uses_given_session_id_and_agent() {
    let mut i = input(HarnessKind::Claude);
    i.session_id = Some(SID.to_owned());
    i.agent = Some("rev".to_owned());
    let out = fresh_cmd_with(&i, &shell(), no_mint);
    assert_eq!(
        out.cmd,
        sv(&["claude", "--session-id", SID, "--agent", "rev"])
    );
    assert_eq!(out.session_id.as_deref(), Some(SID));
}

#[test]
fn fresh_claude_with_the_flag_and_no_session_id_mints_one() {
    let mut i = input(HarnessKind::Claude);
    i.new_harness = true;
    let out = fresh_cmd_with(&i, &shell(), || "m1".into());
    assert_eq!(out.cmd, sv(&["claude", "--session-id", "m1"]));
    assert_eq!(out.session_id.as_deref(), Some("m1"));
}

#[test]
fn fresh_claude_without_the_flag_is_bare_and_mints_nothing() {
    let mut i = input(HarnessKind::Claude);
    i.agent = Some("rev".to_owned());
    let out = fresh_cmd_with(&i, &shell(), no_mint);
    assert_eq!(out.cmd, sv(&["claude", "--agent", "rev"]));
    assert_eq!(out.session_id, None);
}

#[test]
fn fresh_opencode_stores_no_port_hostname_or_continue() {
    let mut i = input(HarnessKind::Opencode);
    i.agent = Some("build".to_owned());
    assert_eq!(fresh(&i).cmd, sv(&["opencode", "--agent", "build"]),);
    assert_eq!(fresh(&input(HarnessKind::Opencode)).cmd, sv(&["opencode"]));
}

#[test]
fn fresh_copilot() {
    let out = fresh(&input(HarnessKind::Copilot));
    assert_eq!(out.cmd, sv(&["gh", "copilot", "suggest"]));
}

#[test]
fn fresh_hermes_new_harness_defaults_and_names_the_profile() {
    let mut i = input(HarnessKind::Hermes);
    i.new_harness = true;
    let out = fresh(&i);
    assert_eq!(out.cmd, sv(&["hermes", "--tui", "-p", "skein"]));
    assert_eq!(out.hermes_profile.as_deref(), Some("skein"));

    i.hermes_profile = Some(" work ".to_owned());
    let out = fresh(&i);
    assert_eq!(out.cmd, sv(&["hermes", "--tui", "-p", "work"]));
    assert_eq!(out.hermes_profile.as_deref(), Some("work"));
}

#[test]
fn fresh_hermes_without_the_flag_and_no_profile_is_bare() {
    let mut i = input(HarnessKind::Hermes);
    let out = fresh(&i);
    assert_eq!(out.cmd, sv(&["hermes", "--tui"]));
    assert_eq!(out.hermes_profile, None);

    i.hermes_profile = Some("  ".to_owned());
    let out = fresh(&i);
    assert_eq!(out.cmd, sv(&["hermes", "--tui"]));
    assert_eq!(out.hermes_profile, None);
}

#[test]
fn fresh_hermes_without_the_flag_passes_a_given_profile_through() {
    let mut i = input(HarnessKind::Hermes);
    i.hermes_profile = Some("work".to_owned());
    let out = fresh(&i);
    assert_eq!(out.cmd, sv(&["hermes", "--tui", "-p", "work"]));
    assert_eq!(out.hermes_profile.as_deref(), Some("work"));
}

#[test]
fn fresh_byoh_uses_the_given_daemon_shell() {
    let out = fresh(&input(HarnessKind::Byoh));
    assert_eq!(out.cmd, shell());
    assert_eq!(out.hermes_profile, None);
}

#[test]
fn fresh_remote_builds_ssh_argv() {
    let mut i = input(HarnessKind::Remote);
    i.remote = Some(RemoteSpec {
        host: "devbox".to_owned(),
        tool: "claude".to_owned(),
        session: "skein-x".to_owned(),
        dir: None,
    });
    let out = fresh(&i);
    assert_eq!(out.cmd.first().map(String::as_str), Some("ssh"));
    assert!(out.cmd.iter().any(|a| a.contains("skein-x")));
}

#[test]
fn fresh_files_and_design_are_empty() {
    for kind in [HarnessKind::Files, HarnessKind::Design] {
        assert_eq!(fresh(&input(kind)).cmd, Vec::<String>::new());
    }
}

#[test]
fn resume_one_claude_with_stored_session() {
    let mut h = with("claude", &["claude", "--session-id", SID], Some(SID));
    h.agent = Some("rev".to_owned());
    let out = resume_one_with(ResumeOneInput { harness: h }, no_mint);
    assert_eq!(
        out.harness.cmd,
        Some(sv(&["claude", "--resume", SID, "--agent", "rev"]))
    );
}

#[test]
fn resume_one_claude_without_session_mints_onto_record() {
    let h = with("claude", &["claude"], None);
    let out = resume_one_with(ResumeOneInput { harness: h }, || "m2".into());
    assert_eq!(out.harness.session_id.as_deref(), Some("m2"));
    assert_eq!(out.harness.cmd, Some(sv(&["claude", "--session-id", "m2"])));
}

#[test]
fn resume_one_opencode_has_the_session_and_no_port() {
    let h = with("opencode", &["opencode"], Some("ses_1"));
    let out = resume_one_with(ResumeOneInput { harness: h }, no_mint);
    assert_eq!(
        out.harness.cmd,
        Some(sv(&["opencode", "--session", "ses_1"]))
    );
}

#[test]
fn resume_one_shell_swapped_returns_to_its_tool() {
    let mut h = with("claude", &["/bin/zsh", "-l"], Some(SID));
    h.shell_claim = Some(ShellClaim {
        session_id: "other".to_owned(),
        port: None,
    });
    let out = resume_one_with(ResumeOneInput { harness: h }, no_mint);
    // #745/#752: restart is the one path that leaves the post-exit shell.
    assert_eq!(out.harness.cmd, Some(sv(&["claude", "--resume", SID])));
    assert!(out.harness.shell_claim.is_none());
}

#[test]
fn resume_rooms_opencode_has_no_port_and_cmdless_is_untouched() {
    let mut oc = with("opencode", &["opencode"], Some("s"));
    oc.id = "oc".to_owned();
    let mut files = harness("files");
    files.id = "f".to_owned();
    let mut cl = with("claude", &["claude"], Some(SID));
    cl.id = "cl".to_owned();
    let out = resume_rooms_with(
        ResumeRoomsInput {
            rooms: vec![room(vec![oc, files, cl])],
            unarchive: false,
        },
        no_mint,
    );
    let hs = &out.rooms[0].harnesses;
    assert_eq!(hs[0].cmd, Some(sv(&["opencode", "--session", "s"])));
    assert_eq!(hs[1].cmd, None);
    assert_eq!(hs[2].cmd, Some(sv(&["claude", "--resume", SID])));
}

#[test]
fn resume_rooms_unarchive_opencode_has_no_port() {
    let mk = |id: &str| -> Harness {
        let mut h = with("opencode", &["opencode"], None);
        h.id = id.to_owned();
        h
    };
    let mut r = room(vec![mk("a"), mk("b")]);
    r.archived = Some(1);
    let out = resume_rooms_with(
        ResumeRoomsInput {
            rooms: vec![r],
            unarchive: true,
        },
        no_mint,
    );
    for h in &out.rooms[0].harnesses {
        assert_eq!(h.cmd, Some(sv(&["opencode"])));
    }
}

#[test]
fn resume_rooms_unarchive_clears_stamps() {
    let mut r = room(vec![with("claude", &["claude"], Some(SID))]);
    r.archived = Some(1);
    r.retired = Some(2);
    r.closed_by = Some(skein_proto::rooms::ClosedBy {
        room_id: "x".to_owned(),
        harness_id: None,
        at: 3,
    });
    let out = resume_rooms_with(
        ResumeRoomsInput {
            rooms: vec![r],
            unarchive: true,
        },
        no_mint,
    );
    let r = &out.rooms[0];
    assert!(r.archived.is_none() && r.retired.is_none() && r.closed_by.is_none());
    assert_eq!(r.harnesses[0].cmd, Some(sv(&["claude", "--resume", SID])));
}

#[test]
fn json_shapes_are_camel_case() {
    let i: FreshCmdInput =
        serde_json::from_str(r#"{"kind":"hermes","hermesProfile":"w","sessionId":"x"}"#).unwrap();
    let out = fresh(&i);
    let v = serde_json::to_value(&out).unwrap();
    assert!(v.get("opencodePort").is_none());
    assert_eq!(v["hermesProfile"], "w");
}
