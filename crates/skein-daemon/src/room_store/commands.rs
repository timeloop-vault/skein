//! Applying one intent command to the in-memory room list (#756).
//!
//! The same bookkeeping as `apply_one` ([`Applied`]), so the store's batch
//! loop treats both alike. A command that leaves the room JSON-identical
//! touches nothing: no revision bump, no event.

use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Map, Value};
use skein_proto::room_commands::RoomCommand;
use skein_proto::rooms::{ClosedBy, Harness, Room};

use super::apply::{Applied, find, reorder};
use super::structural::{harness_key_eq, room_key_eq};

/// Per-batch inputs, read once before the batch runs.
pub(super) struct Ctx {
    /// The daemon's wall clock, epoch ms.
    pub now_ms: i64,
}

pub(super) fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|d| i64::try_from(d.as_millis()).ok())
        .unwrap_or(0)
}

/// Room-level keys a patch may not set.
const OWNED_ROOM_KEYS: [&str; 3] = ["id", "harnesses", "revision"];
/// Harness keys a patch may not set: the daemon owns `sessionId` and
/// `opencodeVersion` (#755), and a running process must not disagree with
/// its record about `kind`.
const OWNED_HARNESS_KEYS: [&str; 4] = ["id", "kind", "sessionId", "opencodeVersion"];

// Every wire name of the record, the one list a patch key is checked against.
// Optional fields are left out of a serialized record when unset, so neither
// "absent from the record" nor "absent after the merge" can tell a real
// optional field from a typo (a `null` patch of a typo would pass silently).
// A list can drift, so `tests/patches.rs` reads the struct definitions in
// skein-proto and fails when a field is added or removed without updating it.
pub(super) const ROOM_KEYS: &[&str] = &[
    "id",
    "name",
    "task",
    "status",
    "badge",
    "harnesses",
    "activeHarnessId",
    "cwd",
    "branch",
    "repo",
    "archived",
    "repoRoot",
    "attention",
    "createdBy",
    "closedBy",
    "retired",
    "repoIdentity",
    "todos",
    "revision",
];
pub(super) const HARNESS_KEYS: &[&str] = &[
    "id",
    "kind",
    "name",
    "status",
    "model",
    "tokens",
    "live",
    "cmd",
    "cwd",
    "sessionId",
    "agent",
    "designEntry",
    "designDevice",
    "pendingNotifications",
    "createdBy",
    "shellClaim",
    "remote",
    "envProfile",
    "hermesProfile",
    "opencodeVersion",
    "launch",
];

/// Apply `cmd` to `rooms`. `Err` carries the refusal reason.
///
/// A patch's `expected_revision` is compared with the stored revision as it
/// is now. The store bumps revisions only after the whole batch ran, so that
/// is the revision the batch started with.
pub(super) fn execute_one(
    rooms: &mut Vec<Room>,
    cmd: RoomCommand,
    ctx: &Ctx,
) -> Result<Applied, &'static str> {
    match cmd {
        RoomCommand::CreateRoom { mut room } => {
            if rooms.iter().any(|r| r.id == room.id) {
                return Err("exists");
            }
            room.revision = 0; // the store owns it
            let id = room.id.clone();
            rooms.push(room);
            Ok(Applied {
                touched: Some(id),
                order: true,
                structural: true,
                ..Applied::default()
            })
        }
        RoomCommand::ReorderRooms { room_ids } => {
            let before: Vec<String> = rooms.iter().map(|r| r.id.clone()).collect();
            reorder(rooms, &room_ids, |r| &r.id);
            if rooms
                .iter()
                .map(|r| r.id.as_str())
                .eq(before.iter().map(String::as_str))
            {
                return Ok(Applied::default());
            }
            Ok(Applied {
                order: true,
                structural: true,
                ..Applied::default()
            })
        }
        other => {
            // The runner keys its process table by harness id alone, so an id
            // is taken if ANY room has it. An unknown room is refused first.
            if let RoomCommand::AddHarness {
                room_id, harness, ..
            } = &other
                && rooms
                    .iter()
                    .any(|r| r.harnesses.iter().any(|h| h.id == harness.id))
            {
                find(rooms, room_id)?;
                return Err("exists");
            }
            let room = find(rooms, room_id(&other))?;
            let before = room.clone();
            edit_room(room, other, ctx)?;
            Ok(compare(&before, room))
        }
    }
}

fn room_id(cmd: &RoomCommand) -> &str {
    match cmd {
        RoomCommand::CreateRoom { room } => &room.id,
        RoomCommand::ReorderRooms { .. } => "",
        RoomCommand::RenameRoom { room_id, .. }
        | RoomCommand::ArchiveRoom { room_id, .. }
        | RoomCommand::UnarchiveRoom { room_id }
        | RoomCommand::AddHarness { room_id, .. }
        | RoomCommand::CloseHarness { room_id, .. }
        | RoomCommand::RestartHarness { room_id, .. }
        | RoomCommand::SetHarnessAgent { room_id, .. }
        | RoomCommand::ReorderHarnesses { room_id, .. }
        | RoomCommand::PatchRoom { room_id, .. }
        | RoomCommand::PatchHarness { room_id, .. } => room_id,
    }
}

/// Mutate one room for a command that targets a room.
fn edit_room(room: &mut Room, cmd: RoomCommand, ctx: &Ctx) -> Result<(), &'static str> {
    match cmd {
        RoomCommand::CreateRoom { .. } | RoomCommand::ReorderRooms { .. } => {}
        RoomCommand::RenameRoom { name, .. } => room.name = name,
        RoomCommand::ArchiveRoom { closed_by, .. } => {
            if room.archived.is_some() {
                return Err("already_archived");
            }
            room.archived = Some(ctx.now_ms);
            room.closed_by = closed_by.map(|c| ClosedBy {
                room_id: c.room_id,
                harness_id: c.harness_id,
                at: ctx.now_ms,
            });
        }
        RoomCommand::UnarchiveRoom { .. } => {
            if room.archived.is_none() {
                return Err("not_archived");
            }
            room.archived = None;
            room.retired = None;
            room.closed_by = None;
        }
        RoomCommand::AddHarness {
            harness, activate, ..
        } => {
            if room.harnesses.iter().any(|h| h.id == harness.id) {
                return Err("exists");
            }
            if activate {
                room.active_harness_id.clone_from(&harness.id);
            }
            room.harnesses.push(harness);
        }
        RoomCommand::CloseHarness { harness_id, .. } => {
            let Some(pos) = room.harnesses.iter().position(|h| h.id == harness_id) else {
                return Err("absent");
            };
            room.harnesses.remove(pos);
            if room.active_harness_id == harness_id {
                room.active_harness_id = room
                    .harnesses
                    .first()
                    .map(|h| h.id.clone())
                    .unwrap_or_default();
            }
        }
        RoomCommand::RestartHarness { harness_id, .. } => {
            let h = harness_mut(room, &harness_id)?;
            h.launch = None;
            h.shell_claim = None;
        }
        RoomCommand::SetHarnessAgent {
            harness_id, agent, ..
        } => harness_mut(room, &harness_id)?.agent = agent,
        RoomCommand::ReorderHarnesses { harness_ids, .. } => {
            reorder(&mut room.harnesses, &harness_ids, |h| &h.id);
        }
        RoomCommand::PatchRoom {
            expected_revision,
            fields,
            ..
        } => {
            if fields.keys().any(|k| OWNED_ROOM_KEYS.contains(&k.as_str())) {
                return Err("owned_field");
            }
            if expected_revision != room.revision {
                return Err("stale_revision");
            }
            if fields.is_empty() {
                return Ok(());
            }
            if fields.keys().any(|k| !ROOM_KEYS.contains(&k.as_str())) {
                return Err("invalid_patch");
            }
            // The harnesses are not part of a room patch: keep them aside.
            let harnesses = std::mem::take(&mut room.harnesses);
            let patched = merge::<Room>(room, &fields);
            let mut next = match patched {
                Ok(next) => next,
                Err(e) => {
                    room.harnesses = harnesses;
                    return Err(e);
                }
            };
            next.harnesses = harnesses;
            next.revision = room.revision;
            *room = next;
        }
        RoomCommand::PatchHarness {
            harness_id,
            expected_revision,
            fields,
            ..
        } => {
            if fields
                .keys()
                .any(|k| OWNED_HARNESS_KEYS.contains(&k.as_str()))
            {
                return Err("owned_field");
            }
            let revision = room.revision;
            let h = harness_mut(room, &harness_id)?;
            if expected_revision != revision {
                return Err("stale_revision");
            }
            if fields.is_empty() {
                return Ok(());
            }
            if fields.keys().any(|k| !HARNESS_KEYS.contains(&k.as_str())) {
                return Err("invalid_patch");
            }
            *h = merge::<Harness>(h, &fields)?;
        }
    }
    Ok(())
}

fn harness_mut<'a>(room: &'a mut Room, id: &str) -> Result<&'a mut Harness, &'static str> {
    room.harnesses
        .iter_mut()
        .find(|h| h.id == id)
        .ok_or("unknown_harness")
}

/// Top-level JSON merge of `fields` over the serialized `record`: each key
/// replaces that key, a `null` removes it. `invalid_patch` if the result no
/// longer parses.
fn merge<T>(record: &T, fields: &Map<String, Value>) -> Result<T, &'static str>
where
    T: serde::Serialize + serde::de::DeserializeOwned,
{
    let Ok(Value::Object(mut obj)) = serde_json::to_value(record) else {
        return Err("invalid_patch");
    };
    for (k, v) in fields {
        if v.is_null() {
            obj.remove(k);
        } else {
            obj.insert(k.clone(), v.clone());
        }
    }
    serde_json::from_value(Value::Object(obj)).map_err(|_| "invalid_patch")
}

/// What changed between a room before and after a command: nothing (JSON
/// identical), or a touch, structural when the room key, the harness set or
/// order, or any harness key differs.
fn compare(before: &Room, after: &Room) -> Applied {
    if serde_json::to_value(before).ok() == serde_json::to_value(after).ok() {
        return Applied::default();
    }
    let harnesses_eq = before.harnesses.len() == after.harnesses.len()
        && before
            .harnesses
            .iter()
            .zip(&after.harnesses)
            .all(|(a, b)| harness_key_eq(a, b));
    Applied {
        touched: Some(after.id.clone()),
        structural: !room_key_eq(before, after) || !harnesses_eq,
        ..Applied::default()
    }
}
