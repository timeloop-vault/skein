//! Ports of the `shQuote`, `validateRemoteHost` and `remoteArgv` cases of
//! `app/src/remoteCmd.test.ts`, plus the `remote kind (#568)` cases of
//! `app/src/harnessCmd.test.ts`. (`remoteSessionName` / `buildRemoteSpec`
//! stay in TypeScript and are not ported.)

use skein_proto::harness_kind::HarnessKind;
use skein_proto::rooms::RemoteSpec;

use super::test_util::{shell, sv, with};
use super::{CmdParams, cmd_for_kind, remote_argv, resume_harness, sh_quote, validate_remote_host};

fn spec(host: &str, tool: &str, session: &str, dir: Option<&str>) -> RemoteSpec {
    RemoteSpec {
        host: host.to_owned(),
        tool: tool.to_owned(),
        session: session.to_owned(),
        dir: dir.map(str::to_owned),
    }
}

/// Minimal POSIX unquoter: '...' segments and \x outside quotes only.
fn unquote(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        match chars[i] {
            '\'' => {
                let end = (i + 1..chars.len())
                    .find(|&j| chars[j] == '\'')
                    .expect("unterminated quote");
                out.extend(&chars[i + 1..end]);
                i = end + 1;
            }
            '\\' => {
                out.extend(chars.get(i + 1));
                i += 2;
            }
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    out
}

// ---- shQuote ----

#[test]
fn sh_quote_single_quotes() {
    assert_eq!(sh_quote("abc def"), "'abc def'");
}

#[test]
fn sh_quote_escapes_an_embedded_single_quote() {
    assert_eq!(sh_quote("a'b"), "'a'\\''b'");
    assert_eq!(sh_quote("it's"), "'it'\\''s'");
}

#[test]
fn sh_quote_round_trips_through_a_posix_unquoter() {
    for x in ["plain", "a'b", "it's 'q'", "~/x y", "$HOME;rm"] {
        assert_eq!(unquote(&sh_quote(x)), x);
    }
}

// ---- validateRemoteHost ----

#[test]
fn accepts_user_at_host_host_and_aliases() {
    assert_eq!(validate_remote_host("user@example-host"), None);
    assert_eq!(validate_remote_host("devbox"), None);
    assert_eq!(validate_remote_host("my-alias.lan"), None);
}

#[test]
fn rejects_empty_whitespace_control_chars_and_leading_dash() {
    for bad in ["", "   ", "a b", "a\u{1}b", "-oProxyCommand=x"] {
        assert!(validate_remote_host(bad).is_some(), "{bad:?}");
    }
}

#[test]
fn host_rejection_messages_match_the_typescript() {
    assert_eq!(validate_remote_host(""), Some("Host is required."));
    assert_eq!(
        validate_remote_host("a b"),
        Some("Host must not contain whitespace.")
    );
    assert_eq!(
        validate_remote_host("-x"),
        Some("Host must not start with '-'.")
    );
    // JS `\s` includes U+FEFF and excludes U+0085; interior only, since
    // `trim` treats the ends alike on both sides.
    assert!(validate_remote_host("a\u{feff}b").is_some());
    assert!(validate_remote_host("a\u{7f}b").is_some());
}

// ---- remoteArgv ----

fn inner_of(cmd: &str) -> String {
    unquote(&cmd["exec \"$SHELL\" -lc ".len()..])
}

#[test]
fn builds_the_basic_argv() {
    let a = remote_argv(&spec("user@example-host", "claude", "skein-r-h", None));
    assert_eq!(
        a,
        Some(sv(&[
            "ssh",
            "-t",
            "--",
            "user@example-host",
            "exec \"$SHELL\" -lc 'tmux new-session -A -s '\\''skein-r-h'\\'' '\\''claude'\\'' \\; set-option -t '\\''skein-r-h'\\'' status off'",
        ]))
    );
}

#[test]
fn separates_tmux_commands_with_a_backslash_semicolon() {
    let a = remote_argv(&spec("devbox", "claude", "s", None)).unwrap();
    assert!(a[4].contains(" \\; set-option"));
    let inner = inner_of(&a[4]);
    assert_eq!(inner.split("\\;").count(), 2);
    assert!(inner.contains(" \\; set-option"));
}

#[test]
fn adds_c_for_a_dir() {
    let a = remote_argv(&spec("devbox", "claude", "s", Some("/srv/app"))).unwrap();
    assert!(a[4].contains("-c '\\''/srv/app'\\''"));
}

#[test]
fn expands_a_leading_tilde_slash_via_home_and_a_bare_tilde() {
    let home = remote_argv(&spec("devbox", "claude", "s", Some("~/proj"))).unwrap();
    assert!(inner_of(&home[4]).contains(" -c \"$HOME\"/'proj' "));
    let bare = remote_argv(&spec("devbox", "claude", "s", Some("~"))).unwrap();
    assert!(inner_of(&bare[4]).contains(" -c \"$HOME\" "));
}

#[test]
fn quotes_a_tool_with_a_quote_and_flags() {
    let tool = "~/.local/bin/claude --foo 'x'";
    let a = remote_argv(&spec("devbox", tool, "s", None)).unwrap();
    assert!(inner_of(&a[4]).contains(&sh_quote(tool)));
}

#[test]
fn is_none_on_a_bad_host_or_empty_tool() {
    assert_eq!(remote_argv(&spec("-oX", "claude", "s", None)), None);
    assert_eq!(remote_argv(&spec("devbox", "  ", "s", None)), None);
}

#[test]
fn trims_the_host_and_a_blank_dir_adds_no_c() {
    let a = remote_argv(&spec(" devbox ", "claude", "s", Some("  "))).unwrap();
    assert_eq!(a[3], "devbox");
    assert!(!a[4].contains(" -c "));
}

// ---- remote kind (#568) ----

fn kind_spec() -> RemoteSpec {
    spec("user@example-host", "opencode", "skein-r1-h1", None)
}

#[test]
fn cmd_for_kind_builds_the_argv_from_the_spec() {
    let s = kind_spec();
    let expected = remote_argv(&s).unwrap();
    let params = CmdParams {
        remote: Some(&s),
        ..CmdParams::default()
    };
    assert_eq!(
        cmd_for_kind(HarnessKind::Remote, &shell(), &params),
        expected
    );
    assert_eq!(
        cmd_for_kind(HarnessKind::Remote, &shell(), &CmdParams::default()),
        Vec::<String>::new()
    );
}

#[test]
fn resume_rebuilds_from_the_record_ignoring_a_stale_ssh_argv() {
    let s = kind_spec();
    let expected = remote_argv(&s).unwrap();
    let mut h = with("remote", &["ssh", "-t", "--", "old-host", "x"], None);
    h.remote = Some(s);
    let once = resume_harness(h, None, super::test_util::no_mint);
    assert_eq!(once.cmd.as_ref(), Some(&expected));
    let twice = resume_harness(once, None, super::test_util::no_mint);
    assert_eq!(twice.cmd, Some(expected));
}

#[test]
fn remote_keeps_a_shell_swapped_cmd() {
    let mut h = with("remote", &["pwsh.exe"], None);
    h.remote = Some(kind_spec());
    let out = resume_harness(h, None, super::test_util::no_mint);
    assert_eq!(out.cmd, Some(sv(&["pwsh.exe"])));
}

#[test]
fn remote_leaves_cmd_unchanged_when_remote_is_missing() {
    let cmd = ["ssh", "-t", "--", "devbox", "x"];
    let out = resume_harness(with("remote", &cmd, None), None, super::test_util::no_mint);
    assert_eq!(out.cmd, Some(sv(&cmd)));
}
