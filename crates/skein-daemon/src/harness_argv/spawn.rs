//! The argv to spawn, derived from the harness record at spawn time (#752).
//! The stored `cmd` text never decides resume-versus-fresh or
//! shell-versus-tool: the record's `launch`, `kind`, `session_id` and the
//! caller's existence probe do. Pure; the runner's `SpawnPlan::from_record`
//! calls it at every spawn.

use skein_proto::harness_kind::HarnessKind;
use skein_proto::rooms::Harness;

use super::{hermes_base, opencode_port_args, remote_argv, shell_argv, with_agent};

/// What to spawn and what the record should hold afterwards.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpawnArgv {
    pub cmd: Vec<String>,
    /// The session id the record should hold after this spawn. Differs from
    /// the stored one when an id was minted (Claude with none) or a stale one
    /// was dropped (opencode, Hermes).
    pub session_id: Option<String>,
}

fn non_empty(cmd: &[String]) -> Option<Vec<String>> {
    cmd.first().filter(|s| !s.is_empty()).map(|_| cmd.to_vec())
}

/// Derive the spawn argv for `h`, or `None` when there is nothing to spawn
/// (the caller refuses with "no cmd").
///
/// `session_exists` is called at most once, and only when a non-blank
/// session id is stored and the kind is Claude, opencode or Hermes.
/// opencode and Hermes without a live session start fresh: never
/// `--continue`, which can open another harness's conversation.
#[must_use]
pub fn spawn_argv(
    h: &Harness,
    kind: HarnessKind,
    session_exists: impl FnOnce() -> bool,
    opencode_port: Option<u16>,
    mint_session_id: impl FnOnce() -> String,
) -> Option<SpawnArgv> {
    let stored = h.session_id.clone();
    if let Some(argv) = shell_argv(h) {
        return Some(SpawnArgv {
            cmd: non_empty(argv)?,
            session_id: stored,
        });
    }
    let live_sid = h.session_id.as_deref().filter(|s| !s.trim().is_empty());
    match kind {
        HarnessKind::Claude => {
            let (flag, sid) = match live_sid {
                None => ("--session-id", mint_session_id()),
                Some(sid) if session_exists() => ("--resume", sid.to_owned()),
                // No transcript: the pre-allocated id is still free.
                Some(sid) => ("--session-id", sid.to_owned()),
            };
            let cmd = with_agent(
                vec!["claude".to_owned(), flag.to_owned(), sid.clone()],
                h.agent.as_deref(),
            );
            Some(SpawnArgv {
                cmd,
                session_id: Some(sid),
            })
        }
        HarnessKind::Opencode => {
            let mut cmd = vec!["opencode".to_owned()];
            if let Some(port) = opencode_port {
                cmd.extend(opencode_port_args(port));
            }
            let sid = live_sid.filter(|_| session_exists()).map(str::to_owned);
            if let Some(sid) = &sid {
                cmd.push("--session".to_owned());
                cmd.push(sid.clone());
            }
            Some(SpawnArgv {
                cmd: with_agent(cmd, h.agent.as_deref()),
                session_id: sid,
            })
        }
        HarnessKind::Hermes => {
            let mut cmd = hermes_base(h.hermes_profile.as_deref());
            let sid = live_sid.filter(|_| session_exists()).map(str::to_owned);
            if let Some(sid) = &sid {
                cmd.push("--resume".to_owned());
                cmd.push(sid.clone());
            }
            Some(SpawnArgv {
                cmd,
                session_id: sid,
            })
        }
        HarnessKind::Remote => {
            let cmd = h
                .remote
                .as_ref()
                .and_then(remote_argv)
                .or_else(|| h.cmd.clone())?;
            Some(SpawnArgv {
                cmd: non_empty(&cmd)?,
                session_id: stored,
            })
        }
        _ => Some(SpawnArgv {
            cmd: non_empty(h.cmd.as_deref()?)?,
            session_id: stored,
        }),
    }
}
