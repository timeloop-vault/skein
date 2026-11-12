//! The daemon-prepared opencode spawn (#730): probe the CLI version and,
//! on v2, start the sidecar and swap in the `--server` argv, so a caller
//! need not do it before `pty_spawn`. The decision takes the probe and the
//! starter as parameters, so it is tested without a binary.

use skein_proto::rooms::OpencodeVersion;

use crate::harness_argv::{OpencodeMode, opencode_spawn_plan, opencode_v2_argv};
use crate::harness_kind::HarnessKind;
use crate::opencode_sidecar::start::SidecarStartResult;
use crate::spawn_env::lookups::OpencodeVersionDto;

/// What a spawn runs after preparation.
#[derive(Debug, PartialEq, Eq)]
pub struct Prepared {
    pub cmd: Vec<String>,
    /// The sidecar generation the PTY binds to: the one started here.
    pub generation: Option<u64>,
    /// Set only when this call probed (an opencode harness).
    pub version: Option<OpencodeVersionDto>,
    /// Set when the sidecar created or confirmed the session.
    pub session_id: Option<String>,
    /// True when this call started the sidecar (so it owns the cleanup).
    pub started_here: bool,
}

fn non_blank(s: Option<&str>) -> Option<&str> {
    s.filter(|s| !s.trim().is_empty())
}

/// Decide the argv. `probe` runs only for an opencode harness the caller
/// has not already prepared; `start(agent, session_id)` only on a v2 plan.
///
/// # Errors
/// The starter's error, unchanged.
pub fn prepare_opencode_spawn(
    cmd: Vec<String>,
    kind: HarnessKind,
    agent: Option<&str>,
    session_id: Option<&str>,
    probe: impl FnOnce() -> Option<OpencodeVersionDto>,
    start: impl FnOnce(Option<&str>, Option<&str>) -> Result<SidecarStartResult, String>,
) -> Result<Prepared, String> {
    let untouched = |cmd, version| Prepared {
        cmd,
        generation: None,
        version,
        session_id: None,
        started_here: false,
    };
    if kind != HarnessKind::Opencode {
        return Ok(untouched(cmd, None));
    }
    let version = probe();
    let proto = version.as_ref().map(|v| OpencodeVersion {
        major: v.major,
        version: v.version.clone(),
    });
    if opencode_spawn_plan(&cmd, kind, proto.as_ref()) == OpencodeMode::V1 {
        return Ok(untouched(cmd, version));
    }
    let started = start(non_blank(agent), non_blank(session_id))?;
    Ok(Prepared {
        cmd: opencode_v2_argv(&started.url, &started.session_id),
        generation: Some(started.generation),
        version,
        session_id: Some(started.session_id),
        started_here: true,
    })
}

#[cfg(test)]
mod tests {
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

    #[test]
    fn v2_starts_the_sidecar_with_the_record_and_uses_the_v2_argv() {
        let calls = RefCell::new(Vec::new());
        let got = prepare_opencode_spawn(
            argv(&["opencode", "--port", "1"]),
            HarnessKind::Opencode,
            Some("build"),
            Some("ses_rec"),
            || v(2),
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
        }
    }

    #[test]
    fn other_kinds_are_untouched() {
        let cmd = argv(&["claude"]);
        let got = prepare_opencode_spawn(
            cmd.clone(),
            HarnessKind::Claude,
            None,
            None,
            || panic!("must not probe"),
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
            |_, _| panic!("must not start"),
        )
        .unwrap();
        assert_eq!(got.cmd, cmd);
        assert_eq!(got.version, v(2));
        assert_eq!(got.generation, None);
    }

    #[test]
    fn a_starter_failure_is_the_spawn_error() {
        let err = prepare_opencode_spawn(
            argv(&["opencode"]),
            HarnessKind::Opencode,
            None,
            None,
            || v(2),
            |_, _| Err("opencode server exited with code 1".to_owned()),
        )
        .unwrap_err();
        assert_eq!(err, "opencode server exited with code 1");
    }
}
