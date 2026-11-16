//! The record half of `create_room` (#734): the `Room` that goes into the
//! store, and the one-way event that tells clients about it. Ports
//! `buildRoom` (`app/src/harnessCreation.ts`) and the toast the webview used
//! to push when it created the room itself.

use std::collections::HashSet;

use serde::Serialize;
use skein_proto::room_mutations::RoomMutation;

use super::super::close_apply::{apply_all, lock_verbs};
use super::super::harness_build::mint_room_id;
use super::super::room_naming::default_room_name;
use super::super::{VerbError, VerbResult};
use super::worktree::Checkout;
use crate::agent_api::state::AgentApiState;
use crate::db::{CreatedBy, Harness, Room};
use crate::room_store::RoomStore;

/// Sent to every client once an agent's `create_room` has stored a room, so
/// the webview can raise its "room created" toast.
pub const AGENT_ROOM_CREATED_EVENT: &str = "skein://agent-room-created";

/// Payload of [`AGENT_ROOM_CREATED_EVENT`]. Frontend contract.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AgentRoomCreated {
    pub room_id: String,
    pub room_name: String,
    /// The new room's first harness, always `main`.
    pub harness_name: String,
    /// The first harness's id.
    pub harness_id: String,
    /// The first harness's kind, as `Harness.kind` spells it in the Room JSON.
    pub kind: String,
    /// The room whose agent asked for it.
    pub requester_room_name: String,
}

/// The first harness of a new room is always called this.
pub(super) const FIRST_HARNESS_NAME: &str = "main";

/// What a new room is made of beyond its folder and first harness.
pub(super) struct NewRoom<'a> {
    pub(super) task: &'a str,
    pub(super) created_by: CreatedBy,
}

/// The room record: one harness, active, with an unread mark because no one
/// has looked at it yet.
pub(super) fn build_room(
    store: &RoomStore,
    spec: NewRoom<'_>,
    checkout: &Checkout,
    harness: Harness,
) -> Room {
    let name = default_room_name(&checkout.cwd);
    let taken: HashSet<String> = store.rooms().into_iter().map(|r| r.id).collect();
    Room {
        id: mint_room_id(&taken),
        repo: checkout.branch.as_ref().map(|_| name.clone()),
        name,
        task: spec.task.to_owned(),
        status: harness.status.clone(),
        badge: 0,
        active_harness_id: harness.id.clone(),
        harnesses: vec![harness],
        cwd: Some(checkout.cwd.clone()),
        branch: checkout.branch.clone(),
        archived: None,
        repo_root: checkout.repo_root.clone(),
        attention: Some(true),
        created_by: Some(spec.created_by),
        closed_by: None,
        retired: None,
        repo_identity: None,
        todos: None,
    }
}

/// Build the room and append it to the store, under the room-verb lock: the
/// id is drawn against the store as it is at the apply, so a concurrent
/// create cannot collide. A skipped entry is a refusal, never a success.
pub(super) async fn store_room(
    state: &AgentApiState,
    spec: NewRoom<'_>,
    checkout: &Checkout,
    harness: Harness,
) -> VerbResult<Room> {
    let (_guard, store) = lock_verbs(state).await?;
    let room = build_room(store, spec, checkout, harness);
    apply_all(
        store,
        vec![RoomMutation::CreateRoom { room: room.clone() }],
        "the new room",
    )?;
    Ok(room)
}

/// Tell clients about the room. Called only after the store applied it (and
/// so broadcast `rooms-changed`), because the listener looks the room up.
pub(super) fn emit_created(state: &AgentApiState, room: &Room, requester_room_name: &str) {
    let Some(events) = state.events.as_ref() else {
        return;
    };
    let first = room.harnesses.first();
    let payload = AgentRoomCreated {
        room_id: room.id.clone(),
        room_name: room.name.clone(),
        harness_name: FIRST_HARNESS_NAME.to_owned(),
        harness_id: first.map(|h| h.id.clone()).unwrap_or_default(),
        kind: first.map(|h| h.kind.clone()).unwrap_or_default(),
        requester_room_name: requester_room_name.to_owned(),
    };
    if let Err(e) = crate::event_sink::emit(events.as_ref(), AGENT_ROOM_CREATED_EVENT, &payload) {
        tracing::warn!(room_id = %room.id, error = %e, "agent api: agent-room-created emit failed");
    }
}

/// Whether a failure after the worktree was cut should say where it is.
pub(super) fn leave_note(err: VerbError, checkout: &Checkout) -> VerbError {
    if !checkout.created_worktree {
        return err;
    }
    let note = format!(
        " (the worktree at {} was created and left in place)",
        checkout.cwd
    );
    match err {
        VerbError::NotFound(m) => VerbError::NotFound(m + &note),
        VerbError::Refused(m) => VerbError::Refused(m + &note),
        VerbError::Unavailable(m) => VerbError::Unavailable(m + &note),
        VerbError::Internal(m) => VerbError::Internal(m + &note),
    }
}
