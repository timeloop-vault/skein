//! Harness argv construction, fresh spawn and resume (#730, skeind phase
//! 2a slice 2): the daemon, not the webview, decides what to run. Ported
//! from the frontend builders `harnessCmd.ts`, `remoteCmd.ts` (argv half)
//! and `agents.ts` (`validateAgent`), which are now deleted; behaviour was
//! ported exactly, odd corners included. Pure and Tauri-free; the app
//! reaches it through the `harness_fresh_cmd`, `harness_resume_cmd`,
//! `rooms_resume_cmds` and `pty_spawn` commands.
//!
//! Every property Skein decides *before* the child starts lives here:
//! Claude's pre-allocated `--session-id`, opencode's pinned `--port`, and
//! the `--agent` both CLIs bind at launch (#247). The argv is rebuilt
//! from the harness RECORD, never by matching the shape of the argv it
//! replaces: that pattern-matching was #153 and #170, and a new spawn
//! flag silently stopped matching until the record became the source.
//!
//! A harness whose `kind` string does not parse to a [`HarnessKind`]
//! (a future kind, a hand-edited blob) passes through the resume
//! functions unchanged; the TypeScript could not hold one, so this is the
//! only place the Rust port is total where the original was typed.

mod agent_check;
mod launch;
mod opencode_v2;
mod plan;
mod remote;
mod resume;
mod spawn;

#[cfg(test)]
mod agent_check_tests;
#[cfg(test)]
mod claim_tests;
#[cfg(test)]
mod fresh_tests;
#[cfg(test)]
mod launch_resume_tests;
#[cfg(test)]
mod launch_tests;
#[cfg(test)]
mod opencode_v2_tests;
#[cfg(test)]
mod plan_tests;
#[cfg(test)]
mod remote_tests;
#[cfg(test)]
mod resume_agent_tests;
#[cfg(test)]
mod resume_tests;
#[cfg(test)]
mod spawn_tests;
#[cfg(test)]
mod test_util;

pub use agent_check::{AgentListReply, AgentVerdict, validate_agent, with_verdict};
pub use launch::{effective_shell_argv, infer_launch, migrate_launch, shell_argv};
pub use opencode_v2::{OpencodeMode, is_opencode_v2, opencode_spawn_plan, opencode_v2_argv};
pub use plan::{
    FreshCmdInput, FreshCmdOutput, ResumeOneInput, ResumeOneOutput, ResumeRoomsInput,
    ResumeRoomsOutput, fresh_cmd, fresh_cmd_with, resume_one, resume_one_with, resume_rooms,
    resume_rooms_with,
};
pub use remote::{remote_argv, sh_quote, validate_remote_host};
pub use resume::{
    resume_cmd, resume_harness, unarchive_room_transform, uuid_session_id, with_resume_cmds,
};
use skein_proto::harness_kind::HarnessKind;
use skein_proto::rooms::RemoteSpec;
pub use spawn::{SpawnArgv, spawn_argv};

/// Skein's own Hermes profile (#626); the default for new Hermes harnesses.
pub const SKEIN_HERMES_PROFILE: &str = "skein";

/// Append `--agent <name>` when the harness names one.
///
/// Both CLIs spell it the same way, so this is shared. An empty or
/// whitespace-only name is treated as absent rather than passed through:
/// `--agent ""` is refused by Claude at spawn, and the only way to get
/// one is a stored blob that has been hand-edited. The name itself goes
/// through untrimmed.
pub(crate) fn with_agent(mut args: Vec<String>, agent: Option<&str>) -> Vec<String> {
    if let Some(name) = agent.filter(|a| !a.trim().is_empty()) {
        args.push("--agent".to_owned());
        args.push(name.to_owned());
    }
    args
}

/// #627: the Hermes argv head, `hermes --tui` plus `-p <name>` when the
/// harness names a profile (blank counts as none).
pub(crate) fn hermes_base(profile: Option<&str>) -> Vec<String> {
    let mut args = vec!["hermes".to_owned(), "--tui".to_owned()];
    if let Some(p) = profile.map(str::trim).filter(|p| !p.is_empty()) {
        args.push("-p".to_owned());
        args.push(p.to_owned());
    }
    args
}

/// The profile a NEW Hermes harness gets: the given one (trimmed), else
/// Skein's own. Other kinds never carry one.
#[must_use]
pub fn resolve_hermes_profile(kind: HarnessKind, profile: Option<&str>) -> Option<String> {
    if kind != HarnessKind::Hermes {
        return None;
    }
    let given = profile.map(str::trim).filter(|p| !p.is_empty());
    Some(given.unwrap_or(SKEIN_HERMES_PROFILE).to_owned())
}

/// The optional inputs of a fresh spawn argv.
///
/// `session_id`: callers always set it for Claude (never for other
/// kinds) so Claude's conversation id is pre-allocated via
/// `--session-id <uuid>`; storing the same id on the record is what lets
/// resume skip the picker.
///
/// `opencode_port`: opencode embeds an HTTP server, so Skein allocates a
/// free port up front and passes `--port <N> --hostname 127.0.0.1` for
/// the SSE adapter. Fresh per spawn, never persisted.
///
/// `agent` (#247): the third spawn-time-locked decision. `None` means "no
/// flag", so the tool's own `agent` setting decides, which is a real
/// choice and not the absence of one.
#[derive(Debug, Clone, Default)]
pub struct CmdParams<'a> {
    pub session_id: Option<&'a str>,
    pub opencode_port: Option<u16>,
    pub agent: Option<&'a str>,
    pub remote: Option<&'a RemoteSpec>,
    pub hermes_profile: Option<&'a str>,
}

/// The argv for a fresh spawn of `kind`.
#[must_use]
pub fn cmd_for_kind(
    kind: HarnessKind,
    fallback_shell: &[String],
    params: &CmdParams<'_>,
) -> Vec<String> {
    match kind {
        HarnessKind::Claude => {
            let mut args = vec!["claude".to_owned()];
            if let Some(sid) = params.session_id.filter(|s| !s.is_empty()) {
                args.push("--session-id".to_owned());
                args.push(sid.to_owned());
            }
            with_agent(args, params.agent)
        }
        HarnessKind::Opencode => {
            // No port = the SSE adapter can't attach, but the harness
            // still runs. Letting opencode pick port 0 would look
            // identical and break telemetry.
            let mut args = vec!["opencode".to_owned()];
            if let Some(port) = params.opencode_port {
                args.extend(opencode_port_args(port));
            }
            with_agent(args, params.agent)
        }
        HarnessKind::Copilot => ["gh", "copilot", "suggest"].map(str::to_owned).to_vec(),
        // `--tui` explicit: otherwise the front-end depends on the user's
        // display.interface setting and TTY detection.
        HarnessKind::Hermes => hermes_base(params.hermes_profile),
        HarnessKind::Byoh => {
            if fallback_shell.is_empty() {
                vec!["pwsh.exe".to_owned()]
            } else {
                fallback_shell.to_vec()
            }
        }
        HarnessKind::Remote => params.remote.and_then(remote_argv).unwrap_or_default(),
        // Unreachable: `files`/`design` have no process (no pty
        // capability, so creation paths never ask for an argv). The empty
        // argv is a totality placeholder.
        HarnessKind::Files | HarnessKind::Design => Vec::new(),
    }
}

pub(crate) fn opencode_port_args(port: u16) -> [String; 4] {
    [
        "--port".to_owned(),
        port.to_string(),
        "--hostname".to_owned(),
        "127.0.0.1".to_owned(),
    ]
}
