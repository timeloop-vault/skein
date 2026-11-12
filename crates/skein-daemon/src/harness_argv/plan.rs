//! The daemon-side entry points the frontend calls instead of building argv
//! itself (#730): each allocates the opencode port the argv needs, so the
//! webview no longer calls `pick_free_port`. The port picker and the
//! session-id minter are injected (`*_with`) so tests are deterministic;
//! the plain functions bind the real ones.

use std::collections::{BTreeMap, HashMap};

use serde::{Deserialize, Serialize};
use skein_proto::harness_kind::HarnessKind;
use skein_proto::rooms::{Harness, RemoteSpec, Room};

use super::{
    CmdParams, cmd_for_kind, resolve_hermes_profile, resume_harness, unarchive_room_transform,
    uuid_session_id, with_resume_cmds,
};
use crate::pty::opencode::pick_free_port;
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
    /// Opencode only; `None` when not opencode or the pick failed.
    pub opencode_port: Option<u16>,
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
    /// `None` when not opencode or the pick failed (the caller decides
    /// whether that refuses the restart).
    pub opencode_port: Option<u16>,
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
    /// Harness id -> freshly allocated opencode port; a failed pick is absent.
    pub ports: BTreeMap<String, u16>,
}

fn non_blank(s: Option<&String>) -> Option<&str> {
    s.map(String::as_str).filter(|s| !s.trim().is_empty())
}

/// [`fresh_cmd`] with the picker and minter injected.
pub fn fresh_cmd_with(
    input: &FreshCmdInput,
    shell: &[String],
    pick: impl FnOnce() -> Result<u16, String>,
    mint: impl FnOnce() -> String,
) -> FreshCmdOutput {
    // A failed pick falls back to bare `opencode`: the harness still runs,
    // only its SSE adapter can't attach (today's silent fallback).
    let opencode_port = if input.kind == HarnessKind::Opencode {
        pick().ok()
    } else {
        None
    };
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
            opencode_port,
            agent: input.agent.as_deref(),
            remote: input.remote.as_ref(),
            hermes_profile: hermes_profile.as_deref(),
        },
    );
    FreshCmdOutput {
        cmd,
        opencode_port,
        session_id,
        hermes_profile,
    }
}

/// The argv for a brand-new harness, with its opencode port allocated.
#[must_use]
pub fn fresh_cmd(spawn_env: &SpawnEnvState, input: &FreshCmdInput) -> FreshCmdOutput {
    fresh_cmd_with(
        input,
        &default_shell_impl(spawn_env),
        pick_free_port,
        uuid_session_id,
    )
}

/// [`resume_one`] with the picker and minter injected.
pub fn resume_one_with(
    input: ResumeOneInput,
    pick: impl FnOnce() -> Result<u16, String>,
    mint: impl FnOnce() -> String,
) -> ResumeOneOutput {
    let is_opencode = HarnessKind::from_name(&input.harness.kind) == Some(HarnessKind::Opencode);
    let opencode_port = if is_opencode { pick().ok() } else { None };
    ResumeOneOutput {
        harness: resume_harness(input.harness, opencode_port, mint),
        opencode_port,
    }
}

/// One harness rebuilt into resume form (restart), with its port allocated.
#[must_use]
pub fn resume_one(input: ResumeOneInput) -> ResumeOneOutput {
    resume_one_with(input, pick_free_port, uuid_session_id)
}

/// [`resume_rooms`] with the picker and minter injected.
pub fn resume_rooms_with(
    input: ResumeRoomsInput,
    mut pick: impl FnMut() -> Result<u16, String>,
    mut mint: impl FnMut() -> String,
) -> ResumeRoomsOutput {
    let mut ports = BTreeMap::new();
    let mut rooms = Vec::with_capacity(input.rooms.len());
    for room in input.rooms {
        // Boot and unarchive both allocate for `kind == opencode && cmd`.
        let mut room_ports: HashMap<String, u16> = HashMap::new();
        for h in &room.harnesses {
            if HarnessKind::from_name(&h.kind) == Some(HarnessKind::Opencode)
                && h.cmd.is_some()
                && let Ok(port) = pick()
            {
                room_ports.insert(h.id.clone(), port);
                ports.insert(h.id.clone(), port);
            }
        }
        rooms.push(if input.unarchive {
            unarchive_room_transform(room, &room_ports, &mut mint)
        } else {
            with_resume_cmds(room, &room_ports, &mut mint)
        });
    }
    ResumeRoomsOutput { rooms, ports }
}

/// Rooms rebuilt into resume form (boot, or unarchive) with every ports
/// they need allocated.
#[must_use]
pub fn resume_rooms(input: ResumeRoomsInput) -> ResumeRoomsOutput {
    resume_rooms_with(input, pick_free_port, uuid_session_id)
}
