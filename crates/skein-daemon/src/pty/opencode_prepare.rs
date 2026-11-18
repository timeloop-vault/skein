//! The daemon-prepared opencode spawn (#730): probe the CLI version and,
//! on v2, start the sidecar and swap in the `--server` argv, so a caller
//! need not do it before `pty_spawn`. The decision takes the probe, the
//! session-presence check and the starter as parameters, so it is tested
//! without a binary.
//!
//! This is also where the version-exact session check lives (#679). The
//! earlier probe only knows the id is in EITHER opencode.db table; which
//! opencode runs is known here. v2's `--session <unknown id>` silently
//! creates an empty session, and a v1 session created after v2's one-time
//! migration is invisible to v2, so a stored id the running version cannot
//! see is dropped rather than resumed into an empty conversation.

use skein_harness::opencode::presence::SessionPresence;
use skein_proto::rooms::OpencodeVersion;

use crate::harness_argv::{OpencodeMode, opencode_spawn_plan, opencode_v2_argv};
use crate::harness_kind::HarnessKind;
use crate::opencode_sidecar::start::SidecarStartResult;
use crate::spawn_env::lookups::OpencodeVersionDto;

/// Why a stored session was not resumed although opencode has it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum SessionNoticeKind {
    /// Only in v1's table, and v2 is running.
    #[serde(rename = "v1SessionUnderV2")]
    V1SessionUnderV2,
    /// Only in v2's table, and v1 is running.
    #[serde(rename = "v2SessionUnderV1")]
    V2SessionUnderV1,
}

/// A conversation the running opencode cannot open, for the pane to say so.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionNotice {
    pub kind: SessionNoticeKind,
    /// The id that was not resumed.
    pub session_id: String,
}

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
    /// The stored session id was not used: the record must not keep it.
    pub session_dropped: bool,
    /// Set when the dropped session exists, but under the other version.
    pub notice: Option<SessionNotice>,
}

fn non_blank(s: Option<&str>) -> Option<&str> {
    s.filter(|s| !s.trim().is_empty())
}

/// `cmd` without the `--session <session_id>` pair; nothing else moves.
fn without_session(mut cmd: Vec<String>, session_id: &str) -> Vec<String> {
    if let Some(i) = cmd
        .windows(2)
        .position(|w| w[0] == "--session" && w[1] == session_id)
    {
        cmd.drain(i..i + 2);
    }
    cmd
}

/// The notice for a dropped session that lives under the other version.
fn notice_for(kind: SessionNoticeKind, session_id: &str) -> SessionNotice {
    tracing::warn!(
        ?kind,
        session_id,
        "opencode: the stored session is not visible to the running version; starting fresh"
    );
    SessionNotice {
        kind,
        session_id: session_id.to_owned(),
    }
}

/// Decide the argv. `probe` runs only for an opencode harness the caller
/// has not already prepared; `start(agent, session_id)` only on a v2 plan.
/// `presence(session_id)` runs at most once, only when opencode's own argv
/// is spawned with a stored session id; `None` means it could not tell,
/// and the id is then kept.
///
/// # Errors
/// The starter's error, unchanged.
pub fn prepare_opencode_spawn(
    cmd: Vec<String>,
    kind: HarnessKind,
    agent: Option<&str>,
    session_id: Option<&str>,
    probe: impl FnOnce() -> Option<OpencodeVersionDto>,
    presence: impl FnOnce(&str) -> Option<SessionPresence>,
    start: impl FnOnce(Option<&str>, Option<&str>) -> Result<SidecarStartResult, String>,
) -> Result<Prepared, String> {
    let untouched = |cmd, version| Prepared {
        cmd,
        generation: None,
        version,
        session_id: None,
        started_here: false,
        session_dropped: false,
        notice: None,
    };
    if kind != HarnessKind::Opencode {
        return Ok(untouched(cmd, None));
    }
    let version = probe();
    let proto = version.as_ref().map(|v| OpencodeVersion {
        major: v.major,
        version: v.version.clone(),
    });
    let mode = opencode_spawn_plan(&cmd, kind, proto.as_ref());
    // Not a post-exit shell: only opencode's own argv has a session to check.
    let stored = non_blank(session_id)
        .filter(|_| cmd.first().map(String::as_str) == HarnessKind::Opencode.program());
    let seen = stored.and_then(|sid| presence(sid).map(|p| (sid, p)));
    if mode == OpencodeMode::V1 {
        let Some((sid, p)) = seen.filter(|(_, p)| !p.v1) else {
            return Ok(untouched(cmd, version));
        };
        let notice =
            p.v2.then(|| notice_for(SessionNoticeKind::V2SessionUnderV1, sid));
        return Ok(Prepared {
            session_dropped: true,
            notice,
            ..untouched(without_session(cmd, sid), version)
        });
    }
    let dropped = seen.filter(|(_, p)| !p.v2);
    let started = start(non_blank(agent), stored.filter(|_| dropped.is_none()))?;
    let notice = dropped
        .filter(|(_, p)| p.v1)
        .map(|(sid, _)| notice_for(SessionNoticeKind::V1SessionUnderV2, sid));
    Ok(Prepared {
        cmd: opencode_v2_argv(&started.url, &started.session_id),
        generation: Some(started.generation),
        version,
        session_id: Some(started.session_id),
        started_here: true,
        session_dropped: dropped.is_some(),
        notice,
    })
}

#[cfg(test)]
mod tests;
