//! Rebuilding a harness into its "resume the previous conversation"
//! form (port of `resumeHarness` and friends in `app/src/harnessCmd.ts`).

use std::collections::HashMap;
use std::hash::BuildHasher;

use skein_proto::harness_kind::HarnessKind;
use skein_proto::rooms::{Harness, Room};

use super::{hermes_base, opencode_port_args, remote_argv, with_agent};

/// A random v4 uuid, the production minter for Claude session ids.
#[must_use]
pub fn uuid_session_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

/// Rebuild a harness's argv into its resume form. Applied at boot and on
/// room reopen, so a fresh PTY spawn re-attaches instead of starting over.
///
/// Derived entirely from the harness record (`kind`, `session_id`, plus
/// the freshly allocated `opencode_port`), never from the argv it
/// replaces. Idempotent: feeding a resume-form cmd back in yields the
/// same cmd (with the new port for opencode).
///
/// A shell argv is passed through, except that a LIVE `shell_claim`
/// (#520, see `resume_shell`) turns it back into the tool's resume form.
///
/// Claude with no stored session id starts a FRESH session
/// (`--session-id <new uuid>`, #486), never bare `--resume` (picker) or
/// `--continue` (could latch onto another harness's conversation). That
/// id cannot be persisted by this function, which returns only an argv;
/// callers that need it persisted use [`resume_harness`].
#[must_use]
pub fn resume_cmd(h: Harness, opencode_port: Option<u16>) -> Vec<String> {
    resume_harness(h, opencode_port, uuid_session_id)
        .cmd
        .unwrap_or_default()
}

fn has_session_id(h: &Harness) -> bool {
    h.session_id
        .as_deref()
        .is_some_and(|s| !s.trim().is_empty())
}

fn without_claim(mut h: Harness) -> Harness {
    h.shell_claim = None;
    h
}

/// #520: a harness whose stored argv is the user's shell (Enter after the
/// child exited). Derived from the record, never the argv: a `shell_claim`
/// is LIVE when its `session_id` is non-empty and equals
/// `h.session_id`. Hydrate/unarchive's existence probe drops a
/// `session_id` whose transcript is gone, so a claim that no longer
/// matches was a phantom or was dropped. A live claim puts the harness
/// back on its own program and the claim is consumed; a stale one
/// restores the shell (claim removed, cmd unchanged). Claude and opencode
/// only (#517); other kinds keep their `shell_claim` untouched.
fn resume_shell(
    h: Harness,
    kind: HarnessKind,
    cmd: Vec<String>,
    opencode_port: Option<u16>,
) -> Harness {
    let claim_sid = match (&h.shell_claim, kind) {
        (Some(c), HarnessKind::Claude | HarnessKind::Opencode) => c.session_id.clone(),
        _ => {
            return Harness {
                cmd: Some(cmd),
                ..h
            };
        }
    };
    if !claim_sid.trim().is_empty() && h.session_id.as_deref() == Some(claim_sid.as_str()) {
        // The persisted `claim.port` is deliberately NOT reused: the
        // previous run's port is dead and may be taken now, and opencode
        // fails hard on a busy explicit --port rather than falling back.
        let argv = if kind == HarnessKind::Claude {
            vec!["claude".to_owned(), "--resume".to_owned(), claim_sid]
        } else {
            opencode_resume_argv(Some(&claim_sid), opencode_port)
        };
        let argv = with_agent(argv, h.agent.as_deref());
        return Harness {
            cmd: Some(argv),
            ..without_claim(h)
        };
    }
    Harness {
        cmd: Some(cmd),
        ..without_claim(h)
    }
}

/// The opencode resume argv (before `--agent`), shared by the rebuild and
/// the shell-claim paths so they cannot drift. Plain truthiness on the
/// id: an empty string means `--continue`, whitespace is passed through.
fn opencode_resume_argv(session_id: Option<&str>, port: Option<u16>) -> Vec<String> {
    let mut args = vec!["opencode".to_owned()];
    if let Some(port) = port {
        args.extend(opencode_port_args(port));
    }
    match session_id.filter(|s| !s.is_empty()) {
        Some(sid) => {
            args.push("--session".to_owned());
            args.push(sid.to_owned());
        }
        None => args.push("--continue".to_owned()),
    }
    args
}

/// [`resume_cmd`], but returning the whole harness so a session id minted
/// here (Claude with none stored, #486) lands on the record.
#[must_use]
pub fn resume_harness(
    h: Harness,
    opencode_port: Option<u16>,
    mint_session_id: impl FnOnce() -> String,
) -> Harness {
    // Not a kind this build knows: nothing to rebuild it from.
    let Some(kind) = HarnessKind::from_name(&h.kind) else {
        return h;
    };
    let cmd = h.cmd.clone().unwrap_or_default();
    // Capability gate first (#184): kinds without a resume concept
    // (copilot, shell, files) pass through untouched.
    if !kind.resumes() {
        return Harness {
            cmd: Some(cmd),
            ..h
        };
    }
    // Only rebuild argvs Skein still owns. The app has exactly one
    // cmd-mutation path, Enter-for-shell after a child exits, which
    // replaces the whole argv with the user's shell, so argv[0] is a
    // sufficient ownership test and, unlike argv-length matching, does not
    // care how many flags Skein adds. A shell-swapped harness keeps its
    // shell; rebuilding it would resurrect a harness the user retired.
    if cmd.first().map(String::as_str) != kind.program() {
        return resume_shell(h, kind, cmd, opencode_port);
    }
    if kind == HarnessKind::Claude && h.shell_claim.is_some() {
        return resume_harness(without_claim(h), opencode_port, mint_session_id);
    }
    match kind {
        HarnessKind::Claude => {
            if !has_session_id(&h) {
                let id = mint_session_id();
                let argv = with_agent(
                    vec!["claude".to_owned(), "--session-id".to_owned(), id.clone()],
                    h.agent.as_deref(),
                );
                return Harness {
                    session_id: Some(id),
                    cmd: Some(argv),
                    ..h
                };
            }
            // #247: the agent is re-passed, not left to the session.
            // `claude --resume <sid>` with no flag restores whatever agent
            // the conversation *started* as, so dropping it here would
            // make the record and the process disagree the first time the
            // user changes a harness's agent, silently and permanently.
            // Re-passing overrides cleanly.
            let sid = h.session_id.clone().unwrap_or_default();
            let argv = with_agent(
                vec!["claude".to_owned(), "--resume".to_owned(), sid],
                h.agent.as_deref(),
            );
            Harness {
                cmd: Some(argv),
                ..h
            }
        }
        HarnessKind::Opencode => {
            // The port from sqlite is dead (the previous run released it
            // when the harness exited), so a fresh one is baked in on
            // every rebuild.
            let argv = with_agent(
                opencode_resume_argv(h.session_id.as_deref(), opencode_port),
                h.agent.as_deref(),
            );
            Harness {
                cmd: Some(argv),
                ..h
            }
        }
        HarnessKind::Hermes => {
            // #624: Hermes mints its own id and can't be told one, so none
            // is minted here; the capture fills `session_id` in later.
            let mut argv = hermes_base(h.hermes_profile.as_deref());
            if has_session_id(&h) {
                argv.push("--resume".to_owned());
                argv.push(h.session_id.clone().unwrap_or_default());
            }
            Harness {
                cmd: Some(argv),
                ..h
            }
        }
        HarnessKind::Remote => {
            // #568: tmux `-A` reattaches, so the same argv is right every time.
            let argv = h.remote.as_ref().and_then(remote_argv);
            Harness {
                cmd: Some(argv.unwrap_or(cmd)),
                ..h
            }
        }
        _ => Harness {
            cmd: Some(cmd),
            ..h
        },
    }
}

/// Every harness in the room rewritten to resume form. Harnesses with no
/// cmd (the `files` kind, or a record that never spawned) pass through
/// untouched: an absent cmd must stay absent rather than become an empty
/// argv. A present-but-empty cmd counts as present (JS `[]` is truthy).
#[must_use]
pub fn with_resume_cmds<S: BuildHasher>(
    mut room: Room,
    ports: &HashMap<String, u16, S>,
    mut mint_session_id: impl FnMut() -> String,
) -> Room {
    room.harnesses = room
        .harnesses
        .into_iter()
        .map(|h| {
            if h.cmd.is_some() {
                let port = ports.get(&h.id).copied();
                resume_harness(h, port, &mut mint_session_id)
            } else {
                h
            }
        })
        .collect();
    room
}

/// Un-archive a room: drop the `archived` stamp and put every harness
/// back into resume form, so the callers that un-archive share one
/// transform instead of each remembering both halves (#170). #411: also
/// drops `closed_by`, since a reopened room was never "closed by"
/// anyone any more; `retired` goes too.
#[must_use]
pub fn unarchive_room_transform<S: BuildHasher>(
    mut room: Room,
    ports: &HashMap<String, u16, S>,
    mint_session_id: impl FnMut() -> String,
) -> Room {
    room.archived = None;
    room.retired = None;
    room.closed_by = None;
    with_resume_cmds(room, ports, mint_session_id)
}
