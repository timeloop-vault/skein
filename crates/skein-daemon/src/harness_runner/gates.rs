//! What the runner checks before it spawns from a record: the refusals and
//! the plan a passing record turns into. Pure over the rooms it is given.

use std::path::Path;

use skein_proto::rooms::{Harness, Room};

use crate::boot_resume::{opencode_port_from_cmd, session_survives};
use crate::git::{IdentityCheck, check_identity};
use crate::harness_argv::{SpawnArgv, shell_argv, spawn_argv, uuid_session_id};
use crate::harness_kind::HarnessKind;

/// Why a harness was not spawned. The `code` strings are a wire contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// No such harness in that room.
    UnknownHarness,
    /// The room is archived: nothing in it runs.
    RoomArchived,
    /// A kind without a PTY (files, design) or one the daemon does not know.
    NoPty,
    /// The record has no argv.
    NoCmd,
    /// The room's folder does not exist.
    FolderMissing,
    /// The folder holds a different repository than the room recorded.
    RepoMismatch,
    /// The caller expected a different argv than the record now holds.
    StaleRecord,
}

impl Refusal {
    #[must_use]
    pub fn code(self) -> &'static str {
        match self {
            Self::UnknownHarness => "unknown_harness",
            Self::RoomArchived => "room_archived",
            Self::NoPty => "no_pty",
            Self::NoCmd => "no_cmd",
            Self::FolderMissing => "folder_missing",
            Self::RepoMismatch => "repo_mismatch",
            Self::StaleRecord => "stale_record",
        }
    }
}

/// Why `ensure_running` did not return a process.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EnsureError {
    Refused(Refusal),
    /// The spawn itself failed (program missing, sidecar would not start, ...).
    SpawnFailed(String),
}

impl EnsureError {
    /// `spawn_failed` or one of [`Refusal::code`].
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            Self::Refused(r) => r.code(),
            Self::SpawnFailed(_) => "spawn_failed",
        }
    }
}

impl std::fmt::Display for EnsureError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Refused(r) => f.write_str(r.code()),
            Self::SpawnFailed(e) => write!(f, "spawn_failed: {e}"),
        }
    }
}

impl std::error::Error for EnsureError {}

/// Does the room still want this harness running: it exists, in an
/// unarchived room, and its kind has a PTY?
pub(super) fn lookup<'a>(
    rooms: &'a [Room],
    room_id: &str,
    harness_id: &str,
) -> Result<(&'a Room, &'a Harness, HarnessKind), Refusal> {
    let room = rooms
        .iter()
        .find(|r| r.id == room_id)
        .ok_or(Refusal::UnknownHarness)?;
    let harness = room
        .harnesses
        .iter()
        .find(|h| h.id == harness_id)
        .ok_or(Refusal::UnknownHarness)?;
    if room.archived.is_some() {
        return Err(Refusal::RoomArchived);
    }
    match HarnessKind::from_name(&harness.kind) {
        None | Some(HarnessKind::Files | HarnessKind::Design) => Err(Refusal::NoPty),
        Some(kind) => Ok((room, harness, kind)),
    }
}

/// Whether the records still want `harness_id` to run (reconcile's test).
pub(super) fn wanted(rooms: &[Room], harness_id: &str) -> bool {
    rooms
        .iter()
        .any(|r| r.archived.is_none() && r.harnesses.iter().any(|h| h.id == harness_id))
}

/// The `--port` the record's STORED cmd pins for opencode, which the derived
/// argv re-passes; `None` in shell launch mode (the stored cmd then holds
/// whatever the shell was, not a port Skein allocated).
///
/// This is the seam issue #754 replaces when the runner allocates the port
/// itself instead of recovering it from the stored text.
pub(super) fn stored_opencode_port(h: &Harness) -> Option<u16> {
    if shell_argv(h).is_some() {
        return None;
    }
    opencode_port_from_cmd(h.cmd.as_deref()?)
}

/// Everything a spawn takes from the record.
pub(super) struct SpawnPlan {
    /// The argv derived at spawn time (#752), not the stored `cmd`.
    pub cmd: Vec<String>,
    pub cwd: String,
    pub kind: HarnessKind,
    pub agent: Option<String>,
    /// The session id the record should hold after this spawn (minted, or
    /// the stale one dropped); may differ from the stored one.
    pub session_id: Option<String>,
    pub env_profile: Option<String>,
    pub hermes_profile: Option<String>,
}

impl SpawnPlan {
    /// The spawn gates, in order: a stored argv, folder, repository identity
    /// (an unknown identity passes), then the argv derived from the record
    /// (this is the only step that probes for the session). `request_profile`
    /// beats the record's.
    pub(super) fn from_record(
        room: &Room,
        harness: &Harness,
        kind: HarnessKind,
        request_profile: Option<&str>,
        probe: &dyn Fn(&Harness) -> Result<bool, String>,
    ) -> Result<Self, Refusal> {
        // A record never given an argv is not spawnable, whatever it derives.
        harness
            .cmd
            .as_ref()
            .filter(|c| c.first().is_some_and(|p| !p.is_empty()))
            .ok_or(Refusal::NoCmd)?;
        // The harness's own folder, else the room's: what the pane used.
        let cwd = harness
            .cwd
            .clone()
            .or_else(|| room.cwd.clone())
            .filter(|c| Path::new(c).is_dir())
            .ok_or(Refusal::FolderMissing)?;
        let repo_dir = room.cwd.as_deref().unwrap_or(&cwd);
        if check_identity(room.repo_identity.as_ref(), Path::new(repo_dir))
            == IdentityCheck::Mismatch
        {
            return Err(Refusal::RepoMismatch);
        }
        let SpawnArgv { cmd, session_id } = spawn_argv(
            harness,
            kind,
            || session_survives(harness, &probe),
            stored_opencode_port(harness),
            uuid_session_id,
        )
        .ok_or(Refusal::NoCmd)?;
        Ok(Self {
            cmd,
            cwd,
            kind,
            agent: harness.agent.clone(),
            session_id,
            env_profile: request_profile
                .map(str::to_owned)
                .or_else(|| harness.env_profile.clone()),
            // Trimmed, blank = none: what the work-poller hook path does.
            hermes_profile: harness
                .hermes_profile
                .as_deref()
                .map(str::trim)
                .filter(|p| !p.is_empty())
                .map(str::to_owned),
        })
    }
}
