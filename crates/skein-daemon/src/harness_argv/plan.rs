//! The daemon-side entry points the frontend calls instead of building argv
//! itself (#730). None of them picks an opencode port (#754): that exists
//! only at spawn. The session-id minter is injected (`*_with`) so tests are
//! deterministic; the plain functions bind the real one.

use serde::{Deserialize, Serialize};
use skein_proto::harness_kind::HarnessKind;
use skein_proto::rooms::{Harness, RemoteSpec, Room};

use super::launch::effective_shell_argv;
use super::{
    CmdParams, cmd_for_kind, resolve_hermes_profile, resume_harness, unarchive_room_transform,
    uuid_session_id, with_resume_cmds,
};
use crate::spawn_env::lookups::default_shell_impl;
use crate::spawn_env::state::SpawnEnvState;

/// Input of [`fresh_cmd`].
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FreshCmdInput {
    pub kind: HarnessKind,
    /// Claude only; used as given. Absent or blank gets one minted only
    /// when `new_harness` is set, else the argv is a bare `claude`.
    #[serde(default)]
    pub session_id: Option<String>,
    /// A brand-new harness: mint a Claude session id when none is given and
    /// default the Hermes profile. Creation sets it; repointing a moved room
    /// does not, so session id and profile pass through exactly as given
    /// (no profile means `hermes --tui`).
    #[serde(default)]
    pub new_harness: bool,
    #[serde(default)]
    pub agent: Option<String>,
    #[serde(default)]
    pub remote: Option<RemoteSpec>,
    /// Hermes only; with `new_harness`, absent or blank resolves to Skein's
    /// own profile, otherwise it passes through as given.
    #[serde(default)]
    pub hermes_profile: Option<String>,
}

/// Result of [`fresh_cmd`]: the argv plus the decisions made for the record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FreshCmdOutput {
    pub cmd: Vec<String>,
    /// The id baked into a Claude argv (given or minted); store it on the record.
    pub session_id: Option<String>,
    /// The resolved Hermes profile; store it on the record.
    pub hermes_profile: Option<String>,
}

/// Input of [`resume_one`].
#[derive(Debug, Clone, Deserialize)]
pub struct ResumeOneInput {
    pub harness: Harness,
}

/// Result of [`resume_one`].
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResumeOneOutput {
    pub harness: Harness,
}

/// Input of [`resume_rooms`].
#[derive(Debug, Clone, Deserialize)]
pub struct ResumeRoomsInput {
    pub rooms: Vec<Room>,
    #[serde(default)]
    pub unarchive: bool,
}

/// Result of [`resume_rooms`].
#[derive(Debug, Clone, Serialize)]
pub struct ResumeRoomsOutput {
    pub rooms: Vec<Room>,
}

fn non_blank(s: Option<&String>) -> Option<&str> {
    s.map(String::as_str).filter(|s| !s.trim().is_empty())
}

/// [`fresh_cmd`] with the minter injected.
pub fn fresh_cmd_with(
    input: &FreshCmdInput,
    shell: &[String],
    mint: impl FnOnce() -> String,
) -> FreshCmdOutput {
    let session_id = if input.kind == HarnessKind::Claude {
        match non_blank(input.session_id.as_ref()) {
            Some(given) => Some(given.to_owned()),
            None if input.new_harness => Some(mint()),
            None => None,
        }
    } else {
        input.session_id.clone()
    };
    let hermes_profile = if input.kind != HarnessKind::Hermes {
        None
    } else if input.new_harness {
        resolve_hermes_profile(input.kind, input.hermes_profile.as_deref())
    } else {
        non_blank(input.hermes_profile.as_ref()).map(|p| p.trim().to_owned())
    };
    let cmd = cmd_for_kind(
        input.kind,
        shell,
        &CmdParams {
            session_id: session_id.as_deref(),
            agent: input.agent.as_deref(),
            remote: input.remote.as_ref(),
            hermes_profile: hermes_profile.as_deref(),
        },
    );
    FreshCmdOutput {
        cmd,
        session_id,
        hermes_profile,
    }
}

/// The argv for a brand-new harness.
#[must_use]
pub fn fresh_cmd(spawn_env: &SpawnEnvState, input: &FreshCmdInput) -> FreshCmdOutput {
    fresh_cmd_with(input, &default_shell_impl(spawn_env), uuid_session_id)
}

/// Restart is the one path that leaves a post-exit shell (#745 part 1, #752):
/// the harness goes back on its kind's own tool. `launch` and the claim are
/// cleared and `cmd` is reduced to the program, so `resume_harness` takes
/// its ordinary tool arm (a stored session id resumes; Claude with none
/// mints one). Boot, unarchive and the room-wide paths never call this.
fn back_to_tool(mut h: Harness) -> Harness {
    if effective_shell_argv(&h).is_none() {
        return h;
    }
    let program = HarnessKind::from_name(&h.kind).and_then(HarnessKind::program);
    if let Some(program) = program {
        h.cmd = Some(vec![program.to_owned()]);
        h.launch = None;
        h.shell_claim = None;
    }
    h
}

/// [`resume_one`] with the minter injected.
pub fn resume_one_with(input: ResumeOneInput, mint: impl FnOnce() -> String) -> ResumeOneOutput {
    ResumeOneOutput {
        harness: resume_harness(back_to_tool(input.harness), mint),
    }
}

/// One harness rebuilt into resume form (restart).
#[must_use]
pub fn resume_one(input: ResumeOneInput) -> ResumeOneOutput {
    resume_one_with(input, uuid_session_id)
}

/// [`resume_rooms`] with the minter injected.
pub fn resume_rooms_with(
    input: ResumeRoomsInput,
    mut mint: impl FnMut() -> String,
) -> ResumeRoomsOutput {
    let rooms = input
        .rooms
        .into_iter()
        .map(|room| {
            if input.unarchive {
                unarchive_room_transform(room, &mut mint)
            } else {
                with_resume_cmds(room, &mut mint)
            }
        })
        .collect();
    ResumeRoomsOutput { rooms }
}

/// Rooms rebuilt into resume form (boot, or unarchive).
#[must_use]
pub fn resume_rooms(input: ResumeRoomsInput) -> ResumeRoomsOutput {
    resume_rooms_with(input, uuid_session_id)
}
