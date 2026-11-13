use skein_proto::rooms::{Harness, LaunchMode};

use super::launch::{infer_launch, migrate_launch, shell_argv};
use super::test_util::{harness, room, sv, with};

fn shell_mode(argv: &[&str]) -> LaunchMode {
    LaunchMode::Shell { argv: sv(argv) }
}

#[test]
fn inference_table() {
    let cases: Vec<(&str, Harness, Option<LaunchMode>)> = vec![
        (
            "claude tool",
            with("claude", &["claude", "--resume", "x"], None),
            None,
        ),
        (
            "claude shell",
            with("claude", &["pwsh.exe"], None),
            Some(shell_mode(&["pwsh.exe"])),
        ),
        (
            "opencode shell",
            with("opencode", &["/bin/zsh", "-l"], None),
            Some(shell_mode(&["/bin/zsh", "-l"])),
        ),
        ("byoh", with("byoh", &["pwsh.exe"], None), None),
        ("copilot", with("copilot", &["pwsh.exe"], None), None),
        ("files no cmd", harness("files"), None),
        ("claude no cmd", harness("claude"), None),
        ("claude empty cmd", with("claude", &[], None), None),
        ("claude empty argv0", with("claude", &[""], None), None),
        ("unknown kind", with("future", &["pwsh.exe"], None), None),
        (
            "explicit tool",
            Harness {
                launch: Some(LaunchMode::Tool),
                ..with("claude", &["pwsh.exe"], None)
            },
            None,
        ),
        (
            "explicit shell",
            Harness {
                launch: Some(shell_mode(&["a"])),
                ..with("claude", &["pwsh.exe"], None)
            },
            None,
        ),
    ];
    for (name, h, want) in cases {
        assert_eq!(infer_launch(&h), want, "{name}");
    }
}

#[test]
fn normal_remote_and_hermes_records_are_not_shell() {
    let remote = with(
        "remote",
        &["ssh", "-t", "--", "host", "tmux new -A -s s"],
        None,
    );
    let hermes = with("hermes", &["hermes", "--tui", "-p", "skein"], Some("s"));
    assert_eq!(infer_launch(&remote), None);
    assert_eq!(infer_launch(&hermes), None);
}

#[test]
fn shell_argv_follows_the_field_not_cmd() {
    let tool = with("claude", &["pwsh.exe"], None);
    assert_eq!(shell_argv(&tool), None);
    let explicit_tool = Harness {
        launch: Some(LaunchMode::Tool),
        ..with("claude", &["pwsh.exe"], None)
    };
    assert_eq!(shell_argv(&explicit_tool), None);
    let shell = Harness {
        launch: Some(shell_mode(&["pwsh.exe", "-NoLogo"])),
        ..with("claude", &["claude"], None)
    };
    assert_eq!(shell_argv(&shell), Some(&sv(&["pwsh.exe", "-NoLogo"])[..]));
}

/// Pins today's behaviour, not an endorsement: `cmd[0]` is compared to the
/// kind's program EXACTLY, so a legacy Claude record stored as `claude.exe`
/// migrates to shell mode and is then spawned verbatim, which is the same argv
/// the pre-#752 runner spawned. This is the comparison `resume.rs` has used
/// since #730; making it smarter is a behaviour change, not a fix.
#[test]
fn a_legacy_claude_exe_record_migrates_to_shell_and_spawns_verbatim() {
    use super::spawn::spawn_argv;
    use crate::harness_kind::HarnessKind;

    let stored = ["claude.exe", "--resume", "x"];
    let mut r = room(vec![with("claude", &stored, Some("x"))]);
    assert!(migrate_launch(&mut r));
    let h = &r.harnesses[0];
    assert_eq!(h.launch, Some(shell_mode(&stored)));

    let out = spawn_argv(
        h,
        HarnessKind::Claude,
        || panic!("a shell is not probed"),
        None,
        super::test_util::no_mint,
    )
    .expect("an argv");
    assert_eq!(out.cmd, sv(&stored));
}

#[test]
fn migration_is_idempotent_and_touches_only_launch() {
    let mut r = room(vec![
        with("claude", &["claude", "--resume", "x"], Some("x")),
        with("opencode", &["pwsh.exe"], Some("o")),
    ]);
    let before = r.harnesses.clone();
    assert!(migrate_launch(&mut r));
    assert_eq!(r.harnesses[0].launch, None);
    assert_eq!(r.harnesses[1].launch, Some(shell_mode(&["pwsh.exe"])));
    for (a, b) in before.iter().zip(&mut r.harnesses) {
        b.launch = None;
        assert_eq!(
            serde_json::to_value(a).unwrap(),
            serde_json::to_value(&*b).unwrap()
        );
    }
    let mut r = room(before);
    assert!(migrate_launch(&mut r));
    assert!(!migrate_launch(&mut r));
}
