//! `close_room` (#411): archiving a room its caller created, once signed off.

use std::time::Duration;

use serde::{Deserialize, Serialize};

use skein_proto::room_commands::{ClosedByRef, RoomCommand};

use super::close_apply::{current_room, execute_all, lock_verbs, refuse_unsaved_files, store_of};
use super::harness_build::open_room;
use super::review::signoff_block;
use super::{VerbError, VerbResult, internal};
use crate::agent_api::auth::Caller;
use crate::agent_api::state::AgentApiState;

/// How many `close_room` attempts (successful or refused past this
/// point) one calling room may make in [`CLOSE_ROOM_WINDOW`] — the same
/// anti-runaway reasoning as `create_room`'s own
/// [`ROOM_CREATION_RATE_LIMIT`], on its own bucket
/// (`AgentApiState::check_verb_rate`'s `"close_room"`) so the two calls
/// never share a budget.
const CLOSE_ROOM_RATE_LIMIT: usize = 5;
const CLOSE_ROOM_WINDOW: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct CloseRoomArgs {
    pub room: String,
}

/// Attribution for a `close_room` or `close_harness` call (#411) —
/// echoed back to the caller. `close_room`'s frontend writes it onto
/// `Room.closedBy`; there is no equivalent `Harness.closedBy` field, so
/// `close_harness`'s copy exists only in the reply and in
/// [`log_close_harness_outcome`]'s `tracing::info!` line. `harness_id`
/// is `None` when the caller sent no `X-Skein-Harness`.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AgentClosedBy {
    pub room_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub harness_id: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CloseRoomOut {
    pub room_id: String,
    /// Archive timestamp (epoch ms), as the frontend recorded it —
    /// mirrors `Room.archived`.
    pub archived: i64,
    pub closed_by: AgentClosedBy,
}

/// One `tracing::info!` per call outcome (#176) — an archive triggered
/// through the agent API is exactly the shape of action a later
/// incident would need reconstructed from the log, not the database.
fn log_close_room_outcome(caller_room: &str, target_room: &str, outcome: &str) {
    tracing::info!(
        caller_room = %caller_room,
        target_room = %target_room,
        outcome = %outcome,
        "agent_api: close_room"
    );
}

/// Archive a room this room created, once it is signed off on its
/// current HEAD (#411) — the narrow amendment to D9 recorded in
/// `CLAUDE.md`: closing a room is archiving it, and archiving is
/// reversible (the worktree, the branch and the room record all stay;
/// Reopen restores it, #153), so it no longer needs the blanket refusal
/// `archive_room`/`remove_worktree`/`delete_room` still carry.
///
/// Guards run in order, cheapest first: the Settings kill switch, a
/// per-calling-room rate cap, then a chain of facts about the TARGET
/// room that must all hold before anything is asked of the frontend —
/// unknown, self, not the creator, archived, not signed off, a stale
/// sign-off. Only once every one of those passes does this ask the
/// daemon to archive it in the room store, exactly like the user's own
/// close (no confirm dialog, refusing on unsaved Files buffers).
///
/// Every refusal message starts with a stable `snake_case` code (see
/// `docs/agent-api.md`).
pub async fn close_room(
    state: &AgentApiState,
    caller: &Caller,
    args: &CloseRoomArgs,
    room_closing_enabled: bool,
) -> VerbResult<CloseRoomOut> {
    let db = &state.db;
    let room_id = args.room.trim();
    if room_id.is_empty() {
        return Err(VerbError::Refused("close_room needs a room id".into()));
    }

    if !room_closing_enabled {
        log_close_room_outcome(&caller.room_id, room_id, "disabled");
        return Err(VerbError::Refused(
            "disabled: the Rooms tools are turned off in Skein's Settings".into(),
        ));
    }
    if !state.check_verb_rate(
        "close_room",
        &caller.room_id,
        CLOSE_ROOM_WINDOW,
        CLOSE_ROOM_RATE_LIMIT,
    ) {
        log_close_room_outcome(&caller.room_id, room_id, "rate_limited");
        return Err(VerbError::Refused(format!(
            "rate_limited: this room has attempted {CLOSE_ROOM_RATE_LIMIT} close_room \
             calls in the last minute"
        )));
    }

    let rooms = db.all_rooms().map_err(internal)?;
    let Some(target) = rooms.iter().find(|r| r.id == room_id) else {
        log_close_room_outcome(&caller.room_id, room_id, "not_found");
        return Err(VerbError::NotFound(format!(
            "not_found: no such room: {room_id:?}"
        )));
    };

    if target.id == caller.room_id {
        log_close_room_outcome(&caller.room_id, room_id, "self");
        return Err(VerbError::Refused(
            "self: a room cannot close itself — close_room only closes a room \
             this one created"
                .into(),
        ));
    }
    let is_creator = target
        .created_by
        .as_ref()
        .is_some_and(|c| c.room_id == caller.room_id);
    if !is_creator {
        log_close_room_outcome(&caller.room_id, room_id, "not_creator");
        return Err(VerbError::Refused(format!(
            "not_creator: {} was not created by this room with create_room, so it \
             cannot close it",
            target.name
        )));
    }
    if target.archived.is_some() {
        log_close_room_outcome(&caller.room_id, room_id, "archived");
        return Err(VerbError::Refused(format!(
            "archived: {} is already archived",
            target.name
        )));
    }

    let signed_off = match target.cwd.as_deref() {
        None => None,
        Some(cwd) => Some(signoff_block(db, &target.id, cwd)?),
    };
    match signed_off {
        Some(s) if s.approved => {}
        Some(s) if s.stale => {
            log_close_room_outcome(&caller.room_id, room_id, "stale_signoff");
            return Err(VerbError::Refused(format!(
                "stale_signoff: {}'s sign-off is stale — the reviewer approved an \
                 earlier commit and HEAD has moved since. Ask them to look again \
                 before closing it.",
                target.name
            )));
        }
        _ => {
            log_close_room_outcome(&caller.room_id, room_id, "not_signed_off");
            return Err(VerbError::Refused(format!(
                "not_signed_off: {} is not signed off yet — the reviewer has not \
                 approved its current HEAD",
                target.name
            )));
        }
    }

    let closed_by = AgentClosedBy {
        room_id: caller.room_id.clone(),
        harness_id: caller.harness_id.clone(),
    };
    match archive(state, &target.id, &closed_by).await {
        Ok(archived) => {
            log_close_room_outcome(&caller.room_id, room_id, "ok");
            Ok(CloseRoomOut {
                room_id: target.id.clone(),
                archived,
                closed_by,
            })
        }
        Err(err) => {
            log_close_room_outcome(&caller.room_id, room_id, err.message());
            Err(err)
        }
    }
}

/// The checks the webview used to make (unknown, already archived, unsaved
/// Files buffers), then the archive itself against the store. Returns the
/// archive timestamp.
async fn archive(
    state: &AgentApiState,
    room_id: &str,
    closed_by: &AgentClosedBy,
) -> VerbResult<i64> {
    let store = store_of(state)?;
    let room = open_room(store, room_id)?;
    refuse_unsaved_files(state, &room.harnesses).await?;

    // The question above may have taken seconds: re-read under the lock so a
    // client change made meanwhile (a rename, a badge) is not written back
    // over, and a concurrent verb's result is seen.
    let (_guard, store) = lock_verbs(state).await?;
    open_room(store, room_id)?;
    execute_all(
        store,
        vec![RoomCommand::ArchiveRoom {
            room_id: room_id.to_owned(),
            closed_by: Some(ClosedByRef {
                room_id: closed_by.room_id.clone(),
                harness_id: closed_by.harness_id.clone(),
            }),
        }],
        &format!("room \"{room_id}\""),
    )?;
    // The store stamped the time; read it back so the reply cannot disagree.
    current_room(store, room_id)
        .and_then(|r| r.archived)
        .ok_or_else(|| VerbError::NotFound(format!("not_found: room \"{room_id}\" not found")))
}
