//! The cross-room read verbs (#356): `list_rooms`, `get_room` and
//! `list_harnesses`, and the phase round trip they share.

use std::collections::BTreeMap;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use super::mail::MailContext;
use super::mail_routing::mail_lead_harness;
use super::review::{StatusOut, signoff_block};
use super::shared::first_non_empty_line;
use super::{VerbError, VerbResult, internal};
use crate::agent_api::auth::Caller;
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
const KNOWN_PHASES: &[&str] = &[
    "spawning",
    "running",
    "idle",
    "waiting",
    "permission",
    "exited",
];

/// How long [`list_rooms`], [`get_room`] and [`list_harnesses`] wait
/// for the webview to answer the `"harness_phases"` round trip before
/// giving up and reporting every requested harness `"unknown"`. Short,
/// deliberately: unlike `create_room`'s minute-long round trips, a
/// read-only listing call must never hang on a busy or absent webview
/// — a phase this backend cannot vouch for is exactly what `"unknown"`
/// is for.
const HARNESS_PHASE_TIMEOUT: Duration = Duration::from_secs(3);

/// Ask the webview what phase each of `harness_ids` is in right now
/// (#328's request/answer round trip), and fall back to `"unknown"` for
/// every one of them on any failure — no webview, a timeout, an answer
/// that doesn't parse, a harness id the answer never named, or a value
/// outside [`KNOWN_PHASES`]. Never `"idle"` as a default: that is a
/// real phase, and guessing it would be a lie the frontend never told.
///
/// At most one round trip per call, and none at all when `harness_ids`
/// is empty — every caller already filters out archived-room harnesses
/// (no PTY is mounted for one, so there is nothing to ask) before
/// building that list.
async fn harness_phases(state: &AgentApiState, harness_ids: &[String]) -> BTreeMap<String, String> {
    let mut phases: BTreeMap<String, String> = harness_ids
        .iter()
        .map(|id| (id.clone(), "unknown".to_owned()))
        .collect();
    if harness_ids.is_empty() {
        return phases;
    }
    let Ok(answer) = state
        .request_frontend(
            "harness_phases",
            serde_json::json!({}),
            HARNESS_PHASE_TIMEOUT,
        )
        .await
    else {
        return phases;
    };
    let Some(reported) = answer.get("phases").and_then(serde_json::Value::as_object) else {
        return phases;
    };
    for (id, phase) in &mut phases {
        if let Some(p) = reported.get(id).and_then(serde_json::Value::as_str)
            && KNOWN_PHASES.contains(&p)
        {
            p.clone_into(phase);
        }
    }
    phases
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

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ListRoomsArgs {
    /// The only accepted value is `"me"` — every other non-`None` value
    /// is refused rather than silently ignored, since a typo here
    /// (`"mine"`, `"self"`) would otherwise read as "show me
    /// everything" and a director would never notice its filter never
    /// applied.
    #[serde(default)]
    pub created_by: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub struct RoomSummary {
    pub room_id: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repo_root: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    pub archived: bool,
    pub harness_count: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created_by: Option<AgentCreatedBy>,
    /// The harness `send_message` would actually reach if addressed to
    /// this room id — `None` when nothing in the room could read mail
    /// at all (see `mail_lead_harness`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lead_harness_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_sha: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt_first_line: Option<String>,
    /// `"archived"`, one of [`KNOWN_PHASES`] (the lead harness's own),
    /// or `"unknown"` — never a guess.
    pub lifecycle: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_status: Option<LastStatus>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub struct ListRoomsOut {
    pub rooms: Vec<RoomSummary>,
    /// See [`FindRoomsForPathOut::unreadable_rooms`] — the same
    /// "rooms Skein holds but could not parse" count, so a director
    /// reading a short list here knows whether it might be short for
    /// that reason rather than because it truly created no more.
    pub unreadable_rooms: u32,
}

/// Every room Skein holds, across every project — archived rooms
/// included, and never dropped: "that room landed and was archived" is
/// exactly what a director needs to see after its own context is
/// compacted (issue #356's addendum). `created_by: "me"` narrows that
/// to only the rooms the *caller's* room created.
pub async fn list_rooms(
    state: &AgentApiState,
    caller: &Caller,
    args: &ListRoomsArgs,
    mail: &MailContext,
) -> VerbResult<ListRoomsOut> {
    let db = &state.db;
    if let Some(cb) = args.created_by.as_deref()
        && cb != "me"
    {
        return Err(VerbError::Refused(format!(
            "unknown created_by {cb:?} — the only supported value is \"me\""
        )));
    }

    let rooms = db.all_rooms().map_err(internal)?;
    let unreadable_rooms = db.unreadable_room_count().map_err(internal)?;

    let filtered: Vec<&Room> = if args.created_by.is_some() {
        rooms
            .iter()
            .filter(|r| {
                r.created_by
                    .as_ref()
                    .is_some_and(|c| c.room_id == caller.room_id)
            })
            .collect()
    } else {
        rooms.iter().collect()
    };

    let needed: Vec<String> = filtered
        .iter()
        .filter(|r| r.archived.is_none())
        .filter_map(|r| {
            mail_lead_harness(r, mail.policy, mail.agent_sees_mcp).map(|h| h.id.clone())
        })
        .collect();
    let phases = harness_phases(state, &needed).await;

    let mut out = Vec::with_capacity(filtered.len());
    for r in filtered {
        let lead = mail_lead_harness(r, mail.policy, mail.agent_sees_mcp);
        let lifecycle = if r.archived.is_some() {
            "archived".to_owned()
        } else {
            lead.map_or_else(
                || "unknown".to_owned(),
                |h| {
                    phases
                        .get(&h.id)
                        .cloned()
                        .unwrap_or_else(|| "unknown".to_owned())
                },
            )
        };
        let last_status = db
            .latest_message_from_room(&r.id, &caller.room_id)
            .map_err(internal)?
            .map(|m| LastStatus {
                first_line: first_non_empty_line(&m.body),
                created_ms: m.created_ms,
            });
        out.push(RoomSummary {
            room_id: r.id.clone(),
            name: r.name.clone(),
            cwd: r.cwd.clone(),
            repo_root: r.repo_root.clone(),
            branch: r.branch.clone(),
            archived: r.archived.is_some(),
            harness_count: r.harnesses.len(),
            created_by: r.created_by.as_ref().map(AgentCreatedBy::from),
            lead_harness_id: lead.map(|h| h.id.clone()),
            base_sha: r.created_by.as_ref().and_then(|c| c.base_sha.clone()),
            prompt_first_line: r
                .created_by
                .as_ref()
                .and_then(|c| c.prompt_first_line.clone()),
            lifecycle,
            last_status,
        });
    }

    Ok(ListRoomsOut {
        rooms: out,
        unreadable_rooms,
    })
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
pub async fn get_room(state: &AgentApiState, args: &GetRoomArgs) -> VerbResult<GetRoomOut> {
    let db = &state.db;
    let room_id = args.room.trim();
    if room_id.is_empty() {
        return Err(VerbError::Refused("get_room needs a room id".into()));
    }
    let rooms = db.all_rooms().map_err(internal)?;
    let room = require_open_room(&rooms, room_id)?;

    let harness_ids: Vec<String> = room.harnesses.iter().map(|h| h.id.clone()).collect();
    let phases = harness_phases(state, &harness_ids).await;
    let harnesses = room
        .harnesses
        .iter()
        .map(|h| HarnessSummary {
            harness_id: h.id.clone(),
            name: h.name.clone(),
            kind: h.kind.clone(),
            agent: h.agent.clone(),
            session_id: h.session_id.clone(),
            phase: phases
                .get(&h.id)
                .cloned()
                .unwrap_or_else(|| "unknown".to_owned()),
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
    let phases = harness_phases(state, &all_harness_ids).await;

    let mut out = Vec::new();
    for r in selected {
        for h in &r.harnesses {
            out.push(HarnessListing {
                room_id: r.id.clone(),
                room_name: r.name.clone(),
                harness_id: h.id.clone(),
                name: h.name.clone(),
                kind: h.kind.clone(),
                agent: h.agent.clone(),
                session_id: h.session_id.clone(),
                phase: phases
                    .get(&h.id)
                    .cloned()
                    .unwrap_or_else(|| "unknown".to_owned()),
            });
        }
    }
    Ok(ListHarnessesOut { harnesses: out })
}
