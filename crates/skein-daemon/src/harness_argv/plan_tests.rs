//! The plan entry points with an injected port picker and session minter.

use std::cell::Cell;

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

fn fresh(i: &FreshCmdInput, port: Result<u16, String>) -> FreshCmdOutput {
    fresh_cmd_with(i, &shell(), || port, || "minted".to_owned())
}

fn no_pick() -> Result<u16, String> {
    panic!("must not pick");
}

#[test]
fn fresh_claude_uses_given_session_id_and_agent() {
    let mut i = input(HarnessKind::Claude);
    i.session_id = Some(SID.to_owned());
    i.agent = Some("rev".to_owned());
    let out = fresh_cmd_with(&i, &shell(), no_pick, no_mint);
    assert_eq!(
        out.cmd,
        sv(&["claude", "--session-id", SID, "--agent", "rev"])
    );
    assert_eq!(out.session_id.as_deref(), Some(SID));
    assert_eq!(out.opencode_port, None);
}

#[test]
fn fresh_claude_with_the_flag_and_no_session_id_mints_one() {
    let mut i = input(HarnessKind::Claude);
    i.new_harness = true;
    let out = fresh_cmd_with(&i, &shell(), no_pick, || "m1".into());
    assert_eq!(out.cmd, sv(&["claude", "--session-id", "m1"]));
    assert_eq!(out.session_id.as_deref(), Some("m1"));
}

#[test]
fn fresh_claude_without_the_flag_is_bare_and_mints_nothing() {
    let mut i = input(HarnessKind::Claude);
    i.agent = Some("rev".to_owned());
    let out = fresh_cmd_with(&i, &shell(), no_pick, no_mint);
    assert_eq!(out.cmd, sv(&["claude", "--agent", "rev"]));
    assert_eq!(out.session_id, None);
}

#[test]
fn fresh_opencode_gets_a_port() {
    let mut i = input(HarnessKind::Opencode);
    i.agent = Some("build".to_owned());
    let out = fresh(&i, Ok(4242));
    assert_eq!(
        out.cmd,
        sv(&[
            "opencode",
            "--port",
            "4242",
            "--hostname",
            "127.0.0.1",
            "--agent",
            "build"
        ])
    );
    assert_eq!(out.opencode_port, Some(4242));
}

#[test]
fn fresh_opencode_failed_pick_is_bare() {
    let out = fresh(&input(HarnessKind::Opencode), Err("no".into()));
    assert_eq!(out.cmd, sv(&["opencode"]));
    assert_eq!(out.opencode_port, None);
}

#[test]
fn fresh_other_kinds_never_pick() {
    for kind in [
        HarnessKind::Copilot,
        HarnessKind::Hermes,
        HarnessKind::Byoh,
        HarnessKind::Files,
        HarnessKind::Design,
    ] {
        let out = fresh_cmd_with(&input(kind), &shell(), no_pick, no_mint);
        assert_eq!(out.opencode_port, None, "{kind:?}");
    }
}

#[test]
fn fresh_copilot() {
    let out = fresh(&input(HarnessKind::Copilot), Ok(1));
    assert_eq!(out.cmd, sv(&["gh", "copilot", "suggest"]));
}

#[test]
fn fresh_hermes_new_harness_defaults_and_names_the_profile() {
    let mut i = input(HarnessKind::Hermes);
    i.new_harness = true;
    let out = fresh(&i, Ok(1));
    assert_eq!(out.cmd, sv(&["hermes", "--tui", "-p", "skein"]));
    assert_eq!(out.hermes_profile.as_deref(), Some("skein"));

    i.hermes_profile = Some(" work ".to_owned());
    let out = fresh(&i, Ok(1));
    assert_eq!(out.cmd, sv(&["hermes", "--tui", "-p", "work"]));
    assert_eq!(out.hermes_profile.as_deref(), Some("work"));
}

#[test]
fn fresh_hermes_without_the_flag_and_no_profile_is_bare() {
    let mut i = input(HarnessKind::Hermes);
    let out = fresh(&i, Ok(1));
    assert_eq!(out.cmd, sv(&["hermes", "--tui"]));
    assert_eq!(out.hermes_profile, None);

    i.hermes_profile = Some("  ".to_owned());
    let out = fresh(&i, Ok(1));
    assert_eq!(out.cmd, sv(&["hermes", "--tui"]));
    assert_eq!(out.hermes_profile, None);
}

#[test]
fn fresh_hermes_without_the_flag_passes_a_given_profile_through() {
    let mut i = input(HarnessKind::Hermes);
    i.hermes_profile = Some("work".to_owned());
    let out = fresh(&i, Ok(1));
    assert_eq!(out.cmd, sv(&["hermes", "--tui", "-p", "work"]));
    assert_eq!(out.hermes_profile.as_deref(), Some("work"));
}

#[test]
fn fresh_byoh_uses_the_given_daemon_shell() {
    let out = fresh(&input(HarnessKind::Byoh), Ok(1));
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
    let out = fresh(&i, Ok(1));
    assert_eq!(out.cmd.first().map(String::as_str), Some("ssh"));
    assert!(out.cmd.iter().any(|a| a.contains("skein-x")));
}

#[test]
fn fresh_files_and_design_are_empty() {
    for kind in [HarnessKind::Files, HarnessKind::Design] {
        assert_eq!(fresh(&input(kind), Ok(1)).cmd, Vec::<String>::new());
    }
}

#[test]
fn resume_one_claude_with_stored_session() {
    let mut h = with("claude", &["claude", "--session-id", SID], Some(SID));
    h.agent = Some("rev".to_owned());
    let out = resume_one_with(ResumeOneInput { harness: h }, no_pick, no_mint);
    assert_eq!(
        out.harness.cmd,
        Some(sv(&["claude", "--resume", SID, "--agent", "rev"]))
    );
    assert_eq!(out.opencode_port, None);
}

#[test]
fn resume_one_claude_without_session_mints_onto_record() {
    let h = with("claude", &["claude"], None);
    let out = resume_one_with(ResumeOneInput { harness: h }, no_pick, || "m2".into());
    assert_eq!(out.harness.session_id.as_deref(), Some("m2"));
    assert_eq!(out.harness.cmd, Some(sv(&["claude", "--session-id", "m2"])));
}

#[test]
fn resume_one_opencode_port_in_argv_and_result() {
    let h = with("opencode", &["opencode"], Some("ses_1"));
    let out = resume_one_with(ResumeOneInput { harness: h }, || Ok(5000), no_mint);
    assert_eq!(out.opencode_port, Some(5000));
    assert_eq!(
        out.harness.cmd,
        Some(sv(&[
            "opencode",
            "--port",
            "5000",
            "--hostname",
            "127.0.0.1",
            "--session",
            "ses_1"
        ]))
    );
}

#[test]
fn resume_one_opencode_failed_pick_still_transforms() {
    let h = with("opencode", &["opencode"], Some("ses_1"));
    let out = resume_one_with(ResumeOneInput { harness: h }, || Err("x".into()), no_mint);
    assert_eq!(out.opencode_port, None);
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
    let out = resume_one_with(ResumeOneInput { harness: h }, no_pick, no_mint);
    // #745/#752: restart is the one path that leaves the post-exit shell.
    assert_eq!(out.harness.cmd, Some(sv(&["claude", "--resume", SID])));
    assert!(out.harness.shell_claim.is_none());
}

#[test]
fn resume_rooms_ports_keyed_by_harness_id_and_cmdless_untouched() {
    let mut oc = with("opencode", &["opencode"], Some("s"));
    oc.id = "oc".to_owned();
    let mut files = harness("files");
    files.id = "f".to_owned();
    let mut cl = with("claude", &["claude"], Some(SID));
    cl.id = "cl".to_owned();
    let next = Cell::new(6000_u16);
    let out = resume_rooms_with(
        ResumeRoomsInput {
            rooms: vec![room(vec![oc, files, cl])],
            unarchive: false,
        },
        || {
            next.set(next.get() + 1);
            Ok(next.get())
        },
        no_mint,
    );
    assert_eq!(out.ports.len(), 1);
    assert_eq!(out.ports.get("oc"), Some(&6001));
    let hs = &out.rooms[0].harnesses;
    assert_eq!(hs[1].cmd, None);
    assert_eq!(hs[2].cmd, Some(sv(&["claude", "--resume", SID])));
}

#[test]
fn resume_rooms_failed_pick_is_omitted() {
    let mut oc = with("opencode", &["opencode"], Some("s"));
    oc.id = "oc".to_owned();
    let out = resume_rooms_with(
        ResumeRoomsInput {
            rooms: vec![room(vec![oc])],
            unarchive: false,
        },
        || Err("x".into()),
        no_mint,
    );
    assert!(out.ports.is_empty());
    assert_eq!(
        out.rooms[0].harnesses[0].cmd,
        Some(sv(&["opencode", "--session", "s"]))
    );
}

#[test]
fn resume_rooms_two_opencode_harnesses_get_two_picks() {
    let mk = |id: &str| -> Harness {
        let mut h = with("opencode", &["opencode"], Some("s"));
        h.id = id.to_owned();
        h
    };
    let picks = Cell::new(0_u16);
    let out = resume_rooms_with(
        ResumeRoomsInput {
            rooms: vec![room(vec![mk("a"), mk("b")])],
            unarchive: false,
        },
        || {
            picks.set(picks.get() + 1);
            Ok(7000 + picks.get())
        },
        no_mint,
    );
    assert_eq!(picks.get(), 2);
    assert_eq!(out.ports.get("a"), Some(&7001));
    assert_eq!(out.ports.get("b"), Some(&7002));
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
        no_pick,
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
    let out = fresh(&i, Ok(1));
    let v = serde_json::to_value(&out).unwrap();
    assert_eq!(v["opencodePort"], serde_json::Value::Null);
    assert_eq!(v["hermesProfile"], "w");
}
