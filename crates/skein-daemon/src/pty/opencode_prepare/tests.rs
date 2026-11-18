use std::cell::{Cell, RefCell};

use super::*;

#[allow(clippy::unnecessary_wraps)]
fn v(major: u32) -> Option<OpencodeVersionDto> {
    Some(OpencodeVersionDto {
        major,
        version: format!("{major}.0.1"),
    })
}

fn argv(parts: &[&str]) -> Vec<String> {
    parts.iter().map(|s| (*s).to_owned()).collect()
}

#[allow(clippy::unnecessary_wraps)]
fn started() -> Result<SidecarStartResult, String> {
    Ok(SidecarStartResult {
        url: "http://127.0.0.1:4100".to_owned(),
        port: 4100,
        session_id: "ses_new".to_owned(),
        generation: 7,
    })
}

#[allow(clippy::unnecessary_wraps)]
fn pres(v1: bool, v2: bool) -> Option<SessionPresence> {
    Some(SessionPresence { v1, v2 })
}

/// A presence closure that must not be called.
fn no_presence(_: &str) -> Option<SessionPresence> {
    panic!("must not check presence")
}

/// Run a prepare on opencode with a version, a stored id and a presence
/// answer; returns the result and what the starter was given.
#[allow(clippy::option_option)]
fn run_with(
    cmd: &[&str],
    version: Option<OpencodeVersionDto>,
    stored: Option<&str>,
    presence: Option<SessionPresence>,
) -> (Prepared, Option<Option<String>>) {
    let start_arg = RefCell::new(None);
    let got = prepare_opencode_spawn(
        argv(cmd),
        HarnessKind::Opencode,
        Some("build"),
        stored,
        || version,
        |_| presence,
        |_, s| {
            *start_arg.borrow_mut() = Some(s.map(str::to_owned));
            started()
        },
    )
    .unwrap();
    (got, start_arg.into_inner())
}

#[test]
fn v2_starts_the_sidecar_with_the_record_and_uses_the_v2_argv() {
    let calls = RefCell::new(Vec::new());
    let got = prepare_opencode_spawn(
        argv(&["opencode", "--port", "1"]),
        HarnessKind::Opencode,
        Some("build"),
        Some("ses_rec"),
        || v(2),
        |_| pres(false, true),
        |a, s| {
            calls
                .borrow_mut()
                .push((a.map(str::to_owned), s.map(str::to_owned)));
            started()
        },
    )
    .unwrap();
    assert_eq!(
        *calls.borrow(),
        vec![(Some("build".to_owned()), Some("ses_rec".to_owned()))]
    );
    assert_eq!(
        got.cmd,
        argv(&[
            "opencode",
            "--server",
            "http://127.0.0.1:4100",
            "--session",
            "ses_new"
        ])
    );
    assert_eq!(got.generation, Some(7));
    assert_eq!(got.session_id.as_deref(), Some("ses_new"));
    assert_eq!(got.version, v(2));
    assert!(got.started_here);
    assert!(!got.session_dropped);
    assert_eq!(got.notice, None);
}

#[test]
fn blank_agent_and_session_reach_the_starter_as_none() {
    let seen = RefCell::new(None);
    prepare_opencode_spawn(
        argv(&["opencode"]),
        HarnessKind::Opencode,
        Some("  "),
        Some(""),
        || v(2),
        no_presence,
        |a, s| {
            *seen.borrow_mut() = Some((a.is_none(), s.is_none()));
            started()
        },
    )
    .unwrap();
    assert_eq!(*seen.borrow(), Some((true, true)));
}

#[test]
fn v1_and_unknown_versions_leave_the_argv_and_report_the_version() {
    for version in [v(1), None] {
        let started_calls = Cell::new(0);
        let cmd = argv(&["opencode", "--port", "9", "--session", "s"]);
        let got = prepare_opencode_spawn(
            cmd.clone(),
            HarnessKind::Opencode,
            Some("build"),
            Some("s"),
            || version.clone(),
            |_| pres(true, false),
            |_, _| {
                started_calls.set(started_calls.get() + 1);
                started()
            },
        )
        .unwrap();
        assert_eq!(started_calls.get(), 0);
        assert_eq!(got.cmd, cmd);
        assert_eq!(got.version, version);
        assert_eq!(got.generation, None);
        assert_eq!(got.session_id, None);
        assert!(!got.started_here);
        assert!(!got.session_dropped);
    }
}

#[test]
fn other_kinds_are_untouched() {
    let cmd = argv(&["claude"]);
    let got = prepare_opencode_spawn(
        cmd.clone(),
        HarnessKind::Claude,
        None,
        Some("s"),
        || panic!("must not probe"),
        no_presence,
        |_, _| panic!("must not start"),
    )
    .unwrap();
    assert_eq!(got.cmd, cmd);
    assert_eq!(got.version, None);
}

#[test]
fn a_post_exit_shell_stays_v1_even_on_a_v2_cli() {
    let cmd = argv(&["pwsh.exe"]);
    let got = prepare_opencode_spawn(
        cmd.clone(),
        HarnessKind::Opencode,
        None,
        Some("s"),
        || v(2),
        no_presence,
        |_, _| panic!("must not start"),
    )
    .unwrap();
    assert_eq!(got.cmd, cmd);
    assert_eq!(got.version, v(2));
    assert_eq!(got.generation, None);
    assert!(!got.session_dropped);
}

#[test]
fn a_starter_failure_is_the_spawn_error() {
    let err = prepare_opencode_spawn(
        argv(&["opencode"]),
        HarnessKind::Opencode,
        None,
        None,
        || v(2),
        no_presence,
        |_, _| Err("opencode server exited with code 1".to_owned()),
    )
    .unwrap_err();
    assert_eq!(err, "opencode server exited with code 1");
}

#[test]
fn no_stored_id_never_asks_for_presence() {
    for stored in [None, Some(""), Some("  ")] {
        for version in [v(1), v(2)] {
            prepare_opencode_spawn(
                argv(&["opencode"]),
                HarnessKind::Opencode,
                None,
                stored,
                || version.clone(),
                no_presence,
                |_, _| started(),
            )
            .unwrap();
        }
    }
}

#[test]
fn v2_with_a_v1_only_session_starts_fresh_and_says_why() {
    let (got, start_arg) = run_with(&["opencode"], v(2), Some("ses_old"), pres(true, false));
    assert_eq!(start_arg, Some(None));
    assert!(got.session_dropped);
    assert_eq!(
        got.notice,
        Some(SessionNotice {
            kind: SessionNoticeKind::V1SessionUnderV2,
            session_id: "ses_old".to_owned(),
        })
    );
    assert_eq!(got.session_id.as_deref(), Some("ses_new"));
    assert!(got.started_here);
}

#[test]
fn v2_with_a_session_in_neither_table_starts_fresh_silently() {
    let (got, start_arg) = run_with(&["opencode"], v(2), Some("ses_old"), pres(false, false));
    assert_eq!(start_arg, Some(None));
    assert!(got.session_dropped);
    assert_eq!(got.notice, None);
}

#[test]
fn v2_with_unknown_presence_keeps_the_id() {
    let (got, start_arg) = run_with(&["opencode"], v(2), Some("ses_old"), None);
    assert_eq!(start_arg, Some(Some("ses_old".to_owned())));
    assert!(!got.session_dropped);
    assert_eq!(got.notice, None);
}

#[test]
fn v1_with_a_v2_only_session_strips_the_session_pair_and_says_why() {
    let (got, start_arg) = run_with(
        &[
            "opencode",
            "--port",
            "9",
            "--session",
            "ses_old",
            "--agent",
            "b",
        ],
        v(1),
        Some("ses_old"),
        pres(false, true),
    );
    assert_eq!(start_arg, None);
    assert_eq!(got.cmd, argv(&["opencode", "--port", "9", "--agent", "b"]));
    assert!(got.session_dropped);
    assert_eq!(
        got.notice,
        Some(SessionNotice {
            kind: SessionNoticeKind::V2SessionUnderV1,
            session_id: "ses_old".to_owned(),
        })
    );
    assert_eq!(got.version, v(1));
    assert!(!got.started_here);
}

#[test]
fn v1_with_a_session_in_neither_table_strips_silently() {
    let (got, _) = run_with(
        &["opencode", "--session", "ses_old", "--port", "9"],
        None,
        Some("ses_old"),
        pres(false, false),
    );
    assert_eq!(got.cmd, argv(&["opencode", "--port", "9"]));
    assert!(got.session_dropped);
    assert_eq!(got.notice, None);
}

#[test]
fn v1_with_unknown_presence_or_a_v1_session_is_untouched() {
    for presence in [None, pres(true, false), pres(true, true)] {
        let cmd = ["opencode", "--session", "ses_old"];
        let (got, _) = run_with(&cmd, v(1), Some("ses_old"), presence);
        assert_eq!(got.cmd, argv(&cmd));
        assert!(!got.session_dropped);
        assert_eq!(got.notice, None);
    }
}

#[test]
fn v2_with_a_session_in_both_tables_resumes() {
    let (got, start_arg) = run_with(&["opencode"], v(2), Some("ses_old"), pres(true, true));
    assert_eq!(start_arg, Some(Some("ses_old".to_owned())));
    assert!(!got.session_dropped);
}

#[test]
fn without_session_removes_only_the_exact_pair() {
    assert_eq!(
        without_session(argv(&["opencode", "--session", "a", "--session", "b"]), "b"),
        argv(&["opencode", "--session", "a"])
    );
    assert_eq!(
        without_session(argv(&["opencode", "--session", "a"]), "x"),
        argv(&["opencode", "--session", "a"])
    );
    assert_eq!(
        without_session(argv(&["opencode", "a", "--session"]), "a"),
        argv(&["opencode", "a", "--session"])
    );
}

#[test]
fn a_notice_serializes_camel_case() {
    let json = serde_json::to_value(SessionNotice {
        kind: SessionNoticeKind::V1SessionUnderV2,
        session_id: "ses_1".to_owned(),
    })
    .unwrap();
    assert_eq!(
        json,
        serde_json::json!({"kind": "v1SessionUnderV2", "sessionId": "ses_1"})
    );
    let json = serde_json::to_value(SessionNoticeKind::V2SessionUnderV1).unwrap();
    assert_eq!(json, serde_json::json!("v2SessionUnderV1"));
}
