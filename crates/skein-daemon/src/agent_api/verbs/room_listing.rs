//! The cross-room read verbs (#356): `get_room` and `list_harnesses`
//! (`list_rooms` lives in `list_rooms.rs`), and the phase read they
//! share.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::review::{StatusOut, signoff_block};
use super::{VerbError, VerbResult, internal};
use crate::agent_api::state::AgentApiState;
use crate::db::Room;

// Three read-only verbs, all deliberately **not** scoped to the
// caller's own room — the same considered exception `find_rooms_for_path`
// (#354) documents: a director that opened several rooms with
// `create_room` needs to see them, and no single room id could scope
// that question. `list_rooms` alone takes a room-scoped filter
// (`created_by: "me"`), applied against the *caller's* room rather than
// widening what a token can see.

/// Harness activity phases the frontend's `ActivityPhase` union can
/// report (`app/src/harnessActivity.ts`). Anything else a webview
/// answer names is untrusted and becomes `"unknown"` rather than passed
/// through — this backend has no independent way to check it.
pub(super) const KNOWN_PHASES: &[&str] = &[
    "spawning",
    "running",
    "idle",
    "waiting",
    "permission",
    "exited",
];

/// What phase each of `harness_ids` is in right now, read from the
/// daemon's own [`PhaseService`] (#732; it was a round trip to the
/// webview before). A harness the service has no record of — never
/// spawned in this process, or forgotten — is `"unknown"`, as is every
/// harness when the state has no service. Never `"idle"` as a default:
/// that is a real phase, and guessing it would be a lie.
///
/// The outstanding-work counts (#448) are `Some` exactly when the
/// service knows the harness, `None` otherwise.
pub(super) fn harness_snapshots(
    state: &AgentApiState,
    harness_ids: &[String],
) -> BTreeMap<String, HarnessSnapshot> {
    let mut snapshots: BTreeMap<String, HarnessSnapshot> = harness_ids
        .iter()
        .map(|id| (id.clone(), HarnessSnapshot::default()))
        .collect();
    let Some(service) = state.phase_service() else {
        return snapshots;
    };
    if harness_ids.is_empty() {
        return snapshots;
    }
    let phases = service.phase_snapshot();
    let work = service.work_snapshot(harness_ids.iter().map(String::as_str));
    for (id, snap) in &mut snapshots {
        let Some(phase) = phases.get(id) else {
            continue;
        };
        phase.as_str().clone_into(&mut snap.phase);
        if let Some(w) = work.get(id) {
            snap.subagents = u32::try_from(w.subagents).ok();
            snap.background_tasks = u32::try_from(w.background_tasks).ok();
        }
    }
    snapshots
}

/// One harness as the phase service reports it: its phase (`"unknown"`
/// when it has no record) and the outstanding work (#448) the
/// end-of-turn deferral counts, `None` when unknown.
pub(super) struct HarnessSnapshot {
    pub(super) phase: String,
    pub(super) subagents: Option<u32>,
    pub(super) background_tasks: Option<u32>,
}

impl Default for HarnessSnapshot {
    fn default() -> Self {
        Self {
            phase: "unknown".to_owned(),
            subagents: None,
            background_tasks: None,
        }
    }
}

/// The room named `room_id`, refusing the way every #356 single-room
/// verb needs to: an id nobody recognises names itself in the refusal
/// (this trio is cross-room by design, so there is no "wrong room"
/// ambiguity to protect the way thread lookups do), and one that
/// resolves to an archived room says so explicitly — never a bare
/// `None` two very different failures could hide behind.
fn require_open_room<'a>(rooms: &'a [Room], room_id: &str) -> VerbResult<&'a Room> {
    let room = rooms
        .iter()
        .find(|r| r.id == room_id)
        .ok_or_else(|| VerbError::NotFound(format!("no such room: {room_id:?}")))?;
    if room.archived.is_some() {
        return Err(VerbError::Refused(format!(
            "room {room_id:?} is archived; its harnesses are no longer live"
        )));
    }
    Ok(room)
}

/// `CreatedBy`, as an agent reads it — never the extra fields
/// `promptFirstLine`/`baseSha` carry on the wire the room is stored
/// with, which are surfaced as their own top-level fields instead so a
/// caller reading only `created_by` still gets attribution and nothing
/// more.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub struct AgentCreatedBy {
    pub room_id: String,
    pub harness_id: String,
}

impl From<&crate::db::CreatedBy> for AgentCreatedBy {
    fn from(c: &crate::db::CreatedBy) -> Self {
        Self {
            room_id: c.room_id.clone(),
            harness_id: c.harness_id.clone(),
        }
    }
}

/// The latest message a room sent the caller, as [`list_rooms`] reports
/// it — enough for a director to rebuild its table without replaying
/// any mail.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub struct LastStatus {
    pub first_line: String,
    pub created_ms: i64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct GetRoomArgs {
    pub room: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub struct HarnessSummary {
    pub harness_id: String,
    pub name: String,
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    pub phase: String,
    /// Subagents the end-of-turn deferral counts as still working
    /// (#448); `null` when the webview did not say.
    pub outstanding_subagents: Option<u32>,
    /// Background tasks running and not overdue (#448); `null` when
    /// the webview did not say.
    pub outstanding_background_tasks: Option<u32>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct GetRoomOut {
    pub room_id: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repo_root: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created_by: Option<AgentCreatedBy>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_sha: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt_first_line: Option<String>,
    pub harnesses: Vec<HarnessSummary>,
    /// `None` exactly when `signoff_unavailable` is `Some` — the room
    /// has no worktree, so there is nothing `status_impl` could read.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signoff: Option<StatusOut>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signoff_unavailable: Option<String>,
}

/// One room, by id — archived or unknown ids are refused loudly (never
/// a bare `None`) since #356's whole point is a director inspecting a
/// room it does not own. Not scoped to the caller's own room, the same
/// considered exception as [`find_rooms_for_path`] and [`list_rooms`].
#[allow(
    clippy::unused_async,
    reason = "kept async for its many callers; it once awaited the webview"
)]
pub async fn get_room(state: &AgentApiState, args: &GetRoomArgs) -> VerbResult<GetRoomOut> {
    let db = &state.db;
    let room_id = args.room.trim();
    if room_id.is_empty() {
        return Err(VerbError::Refused("get_room needs a room id".into()));
    }
    let rooms = db.all_rooms().map_err(internal)?;
    let room = require_open_room(&rooms, room_id)?;

    let harness_ids: Vec<String> = room.harnesses.iter().map(|h| h.id.clone()).collect();
    let snapshots = harness_snapshots(state, &harness_ids);
    let harnesses = room
        .harnesses
        .iter()
        .map(|h| {
            let snap = snapshots.get(&h.id);
            HarnessSummary {
                harness_id: h.id.clone(),
                name: h.name.clone(),
                kind: h.kind.clone(),
                agent: h.agent.clone(),
                session_id: h.session_id.clone(),
                phase: snap.map_or_else(|| "unknown".to_owned(), |s| s.phase.clone()),
                outstanding_subagents: snap.and_then(|s| s.subagents),
                outstanding_background_tasks: snap.and_then(|s| s.background_tasks),
            }
        })
        .collect();

    let (signoff, signoff_unavailable) = match room.cwd.as_deref() {
        Some(cwd) => (Some(signoff_block(db, &room.id, cwd)?), None),
        None => (
            None,
            Some("this room has no worktree, so there is nothing to sign off on".to_owned()),
        ),
    };

    Ok(GetRoomOut {
        room_id: room.id.clone(),
        name: room.name.clone(),
        cwd: room.cwd.clone(),
        repo_root: room.repo_root.clone(),
        branch: room.branch.clone(),
        created_by: room.created_by.as_ref().map(AgentCreatedBy::from),
        base_sha: room.created_by.as_ref().and_then(|c| c.base_sha.clone()),
        prompt_first_line: room
            .created_by
            .as_ref()
            .and_then(|c| c.prompt_first_line.clone()),
        harnesses,
        signoff,
        signoff_unavailable,
    })
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ListHarnessesArgs {
    /// Omitted — every harness in every open room. Given, and blank
    /// after trimming, is treated the same as omitted.
    #[serde(default)]
    pub room: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub struct HarnessListing {
    pub room_id: String,
    pub room_name: String,
    pub harness_id: String,
    pub name: String,
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    pub phase: String,
    /// See [`HarnessSummary::outstanding_subagents`].
    pub outstanding_subagents: Option<u32>,
    /// See [`HarnessSummary::outstanding_background_tasks`].
    pub outstanding_background_tasks: Option<u32>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub struct ListHarnessesOut {
    pub harnesses: Vec<HarnessListing>,
}

/// Every harness in every open room — archived rooms are skipped
/// entirely, the same as an absent `room` filter skips them, since none
/// of their harnesses have a live PTY to ask a phase of. `room` narrows
/// to one room, with the same unknown/archived refusals as
/// [`get_room`]. Not scoped to the caller's own room, the same
/// considered exception as [`find_rooms_for_path`], [`list_rooms`] and
/// [`get_room`].
#[allow(
    clippy::unused_async,
    reason = "kept async for its many callers; it once awaited the webview"
)]
pub async fn list_harnesses(
    state: &AgentApiState,
    args: &ListHarnessesArgs,
) -> VerbResult<ListHarnessesOut> {
    let db = &state.db;
    let rooms = db.all_rooms().map_err(internal)?;

    let selected: Vec<&Room> = match args
        .room
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        Some(room_id) => vec![require_open_room(&rooms, room_id)?],
        None => rooms.iter().filter(|r| r.archived.is_none()).collect(),
    };

    let all_harness_ids: Vec<String> = selected
        .iter()
        .flat_map(|r| r.harnesses.iter().map(|h| h.id.clone()))
        .collect();
    let snapshots = harness_snapshots(state, &all_harness_ids);

    let mut out = Vec::new();
    for r in selected {
        for h in &r.harnesses {
            let snap = snapshots.get(&h.id);
            out.push(HarnessListing {
                room_id: r.id.clone(),
                room_name: r.name.clone(),
                harness_id: h.id.clone(),
                name: h.name.clone(),
                kind: h.kind.clone(),
                agent: h.agent.clone(),
                session_id: h.session_id.clone(),
                phase: snap.map_or_else(|| "unknown".to_owned(), |s| s.phase.clone()),
                outstanding_subagents: snap.and_then(|s| s.subagents),
                outstanding_background_tasks: snap.and_then(|s| s.background_tasks),
            });
        }
    }
    Ok(ListHarnessesOut { harnesses: out })
}
