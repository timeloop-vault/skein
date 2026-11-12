//! Ports of `resumeCmd` (Claude, opencode, no-resume kinds),
//! `withResumeCmds` and `unarchiveRoomTransform` from
//! `app/src/harnessCmd.test.ts`.

use super::test_util::{SID, harness, is_uuid, no_mint, ports, room, shell, sv, with};
use super::{
    resume_cmd, resume_harness, unarchive_room_transform, uuid_session_id, with_resume_cmds,
};

fn resume_ok(h: skein_proto::rooms::Harness, port: Option<u16>) -> Vec<String> {
    resume_cmd(h, port)
}

// ---- Claude ----

#[test]
fn claude_rewrites_a_fresh_spawn_into_a_resume() {
    let h = with("claude", &["claude", "--session-id", SID], Some(SID));
    assert_eq!(resume_ok(h, None), sv(&["claude", "--resume", SID]));
}

#[test]
fn claude_never_yields_bare_resume_with_no_session_id() {
    let cmd = resume_ok(with("claude", &["claude"], None), None);
    assert_ne!(cmd, sv(&["claude", "--resume"]));
    assert_eq!(cmd[..2], sv(&["claude", "--session-id"])[..]);
}

#[test]
fn claude_mints_a_fresh_session_id_and_records_it_with_the_agent() {
    let mut h = with("claude", &["claude"], None);
    h.agent = Some("X".to_owned());
    let out = resume_harness(h, None, uuid_session_id);
    let cmd = out.cmd.expect("cmd");
    assert_eq!(cmd[..2], sv(&["claude", "--session-id"])[..]);
    assert_eq!(cmd[3..], sv(&["--agent", "X"])[..]);
    assert!(is_uuid(&cmd[2]), "{}", cmd[2]);
    assert_eq!(out.session_id.as_deref(), Some(cmd[2].as_str()));
}

#[test]
fn claude_uses_the_injected_minter_and_treats_a_blank_id_as_absent() {
    let h = with("claude", &["claude"], Some("  "));
    let out = resume_harness(h, None, || SID.to_owned());
    assert_eq!(out.cmd, Some(sv(&["claude", "--session-id", SID])));
    assert_eq!(out.session_id.as_deref(), Some(SID));
}

#[test]
fn claude_resumes_an_existing_session_unchanged_with_the_agent() {
    let mut h = with("claude", &["claude"], Some(SID));
    h.agent = Some("X".to_owned());
    let out = resume_harness(h, None, || "never".to_owned());
    assert_eq!(
        out.cmd,
        Some(sv(&["claude", "--resume", SID, "--agent", "X"]))
    );
    assert_eq!(out.session_id.as_deref(), Some(SID));
}

#[test]
fn with_resume_cmds_persists_the_minted_id_on_the_room() {
    let r = room(vec![with("claude", &["claude"], None)]);
    let out = with_resume_cmds(r, &ports(&[]), uuid_session_id);
    let h = &out.harnesses[0];
    let sid = h.session_id.clone().expect("sid");
    assert!(is_uuid(&sid));
    assert_eq!(h.cmd, Some(sv(&["claude", "--session-id", &sid])));
}

#[test]
fn claude_mints_nothing_for_a_shell_swapped_or_cmd_less_harness() {
    let mut a = with("claude", &["pwsh.exe"], None);
    a.id = "a".to_owned();
    let mut b = harness("claude");
    b.id = "b".to_owned();
    let out = with_resume_cmds(room(vec![a, b]), &ports(&[]), no_mint);
    assert_eq!(out.harnesses[0].session_id, None);
    assert_eq!(out.harnesses[0].cmd, Some(sv(&["pwsh.exe"])));
    assert_eq!(out.harnesses[1].cmd, None);
    assert_eq!(out.harnesses[1].session_id, None);
}

#[test]
fn claude_is_idempotent_so_every_boot_and_reopen_can_run_it() {
    let h = with("claude", &["claude", "--resume", SID], Some(SID));
    let want = sv(&["claude", "--resume", SID]);
    assert_eq!(resume_ok(h.clone(), None), want);
    let again = skein_proto::rooms::Harness {
        cmd: Some(resume_ok(h.clone(), None)),
        ..h
    };
    assert_eq!(resume_ok(again, None), want);
}

// The #153 / #170 regression guard, and the reason the module exists:
// length of the old argv must not enter into it. The argv carries
// `--agent reviewer` and the *record* does not, and the rebuild drops it:
// the record is the authority, not the argv.
#[test]
fn claude_rebuilds_regardless_of_how_many_spawn_flags_the_argv_carries() {
    let cmds: [&[&str]; 3] = [
        &["claude", "--session-id", SID, "--agent", "reviewer"],
        &["claude", "--agent", "reviewer"],
        &[
            "claude",
            "--session-id",
            SID,
            "--model",
            "opus",
            "--permission-mode",
            "plan",
        ],
    ];
    for cmd in cmds {
        let h = with("claude", cmd, Some(SID));
        assert_eq!(
            resume_ok(h, None),
            sv(&["claude", "--resume", SID]),
            "{}",
            cmd.join(" ")
        );
    }
}

#[test]
fn claude_leaves_a_shell_swapped_harness_alone() {
    // Enter-for-shell after an exit replaces the argv wholesale.
    // Rebuilding it would resurrect a retired harness.
    let h = with("claude", &["pwsh.exe"], Some(SID));
    assert_eq!(resume_ok(h, None), sv(&["pwsh.exe"]));
}

// ---- opencode ----

fn oc_resume(cmd: &[&str], sid: Option<&str>) -> skein_proto::rooms::Harness {
    with("opencode", cmd, sid)
}

#[test]
fn opencode_bakes_in_the_fresh_port_and_the_stored_session() {
    let h = oc_resume(
        &["opencode", "--port", "4096", "--hostname", "127.0.0.1"],
        Some("ses_abc"),
    );
    assert_eq!(
        resume_ok(h, Some(5150)),
        sv(&[
            "opencode",
            "--port",
            "5150",
            "--hostname",
            "127.0.0.1",
            "--session",
            "ses_abc"
        ])
    );
}

#[test]
fn opencode_resumes_most_recent_in_cwd_without_a_captured_session_id() {
    let h = oc_resume(&["opencode"], None);
    assert_eq!(
        resume_ok(h, Some(5150)),
        sv(&[
            "opencode",
            "--port",
            "5150",
            "--hostname",
            "127.0.0.1",
            "--continue"
        ])
    );
}

#[test]
fn opencode_still_resumes_when_port_allocation_failed() {
    let h = oc_resume(&["opencode"], Some("ses_abc"));
    assert_eq!(
        resume_ok(h, None),
        sv(&["opencode", "--session", "ses_abc"])
    );
}

#[test]
fn opencode_is_idempotent_apart_from_the_port_which_must_change() {
    let h = oc_resume(&["opencode"], Some("ses_abc"));
    let once = resume_ok(h.clone(), Some(5150));
    let again = skein_proto::rooms::Harness {
        cmd: Some(once),
        ..h
    };
    assert_eq!(
        resume_ok(again, Some(6200)),
        sv(&[
            "opencode",
            "--port",
            "6200",
            "--hostname",
            "127.0.0.1",
            "--session",
            "ses_abc"
        ])
    );
}

#[test]
fn opencode_leaves_a_shell_swapped_harness_alone() {
    let h = oc_resume(&["pwsh.exe"], Some("ses_abc"));
    assert_eq!(resume_ok(h, None), sv(&["pwsh.exe"]));
}

#[test]
fn opencode_plain_truthiness_on_the_session_id() {
    // Empty means --continue; whitespace-only is passed through as an id.
    assert_eq!(
        resume_ok(oc_resume(&["opencode"], Some("")), None),
        sv(&["opencode", "--continue"])
    );
    assert_eq!(
        resume_ok(oc_resume(&["opencode"], Some(" ")), None),
        sv(&["opencode", "--session", " "])
    );
}

// ---- kinds without a resume concept ----

#[test]
fn kinds_without_resume_pass_through_untouched() {
    let copilot = with("copilot", &["gh", "copilot", "suggest"], None);
    assert_eq!(resume_ok(copilot, None), sv(&["gh", "copilot", "suggest"]));
    let byoh = with("byoh", &["/bin/zsh", "-l"], None);
    assert_eq!(resume_ok(byoh, None), shell());
    // A `files` harness has no process at all. Even with a stray
    // session id on the record it must not gain an argv.
    let mut files = harness("files");
    files.session_id = Some(SID.to_owned());
    assert_eq!(resume_ok(files.clone(), None), Vec::<String>::new());
    // resumeHarness returns `cmd ?? []`, i.e. Some(vec![]).
    assert_eq!(resume_harness(files, None, no_mint).cmd, Some(Vec::new()));
}

// ---- withResumeCmds ----

#[test]
fn with_resume_cmds_rewrites_per_harness_keyed_by_its_own_port() {
    let mut a = with("opencode", &["opencode"], Some("ses_a"));
    a.id = "a".to_owned();
    let mut b = with("opencode", &["opencode"], Some("ses_b"));
    b.id = "b".to_owned();
    let out = with_resume_cmds(
        room(vec![a, b]),
        &ports(&[("a", 4096), ("b", 4097)]),
        no_mint,
    );
    assert!(
        out.harnesses[0]
            .cmd
            .as_ref()
            .unwrap()
            .contains(&"4096".to_owned())
    );
    assert!(
        out.harnesses[1]
            .cmd
            .as_ref()
            .unwrap()
            .contains(&"4097".to_owned())
    );
}

#[test]
fn with_resume_cmds_leaves_a_cmd_less_harness_without_a_cmd() {
    let out = with_resume_cmds(room(vec![harness("files")]), &ports(&[]), no_mint);
    assert_eq!(out.harnesses[0].cmd, None);
}

#[test]
fn with_resume_cmds_treats_an_empty_cmd_as_present() {
    // JS `[]` is truthy: an empty argv still goes through resumeHarness,
    // and for Claude (argv[0] is not "claude") that is a shell pass-through.
    let mut h = harness("claude");
    h.cmd = Some(Vec::new());
    let out = with_resume_cmds(room(vec![h]), &ports(&[]), no_mint);
    assert_eq!(out.harnesses[0].cmd, Some(Vec::new()));
}

// ---- unarchiveRoomTransform ----

#[test]
fn unarchive_drops_the_archived_stamp_and_resumes_every_harness_at_once() {
    let h = with("claude", &["claude", "--session-id", SID], Some(SID));
    let mut r = room(vec![h]);
    r.archived = Some(1_700_000_000_000);
    let out = unarchive_room_transform(r, &ports(&[]), no_mint);
    // Both halves, together: #170 shipped a third un-archive path that
    // did the first and forgot the second.
    assert_eq!(out.archived, None);
    assert_eq!(out.harnesses[0].cmd, Some(sv(&["claude", "--resume", SID])));
}

#[test]
fn unarchive_drops_the_retired_stamp_too() {
    let mut r = room(vec![with("claude", &["claude"], None)]);
    r.archived = Some(1);
    r.retired = Some(2);
    let out = unarchive_room_transform(r, &ports(&[]), uuid_session_id);
    assert_eq!(out.retired, None);
}

#[test]
fn unarchive_drops_closed_by() {
    let mut r = room(vec![harness("files")]);
    r.archived = Some(1);
    r.closed_by = Some(skein_proto::rooms::ClosedBy {
        room_id: "x".to_owned(),
        harness_id: None,
        at: 5,
    });
    let out = unarchive_room_transform(r, &ports(&[]), no_mint);
    assert!(out.closed_by.is_none());
}

#[test]
fn unarchive_preserves_the_rest_of_the_room() {
    let mut r = room(vec![with("claude", &["claude"], None)]);
    r.archived = Some(1);
    r.branch = Some("feat/x".to_owned());
    r.repo = Some("skein".to_owned());
    r.task = "t".to_owned();
    let out = unarchive_room_transform(r, &ports(&[]), uuid_session_id);
    assert_eq!(out.branch.as_deref(), Some("feat/x"));
    assert_eq!(out.repo.as_deref(), Some("skein"));
    assert_eq!(out.task, "t");
}
