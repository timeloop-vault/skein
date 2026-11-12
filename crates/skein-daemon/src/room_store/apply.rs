//! Applying one mutation to the in-memory room list (#729).

use std::collections::HashSet;

use skein_proto::room_mutations::RoomMutation;
use skein_proto::rooms::Room;

use super::structural::{harness_key_eq, room_key_eq};

/// What one applied mutation did, for the batch's change list and
/// persistence bookkeeping.
#[derive(Debug, Default)]
pub(super) struct Applied {
    /// A room whose final state must be reported and written.
    pub touched: Option<String>,
    /// A room that was removed.
    pub removed: Option<String>,
    /// The order of the rooms changed (create, remove or reorder).
    pub order: bool,
    pub structural: bool,
}

/// Apply `mutation` to `rooms`. `Err` carries the skip reason.
pub(super) fn apply_one(
    rooms: &mut Vec<Room>,
    mutation: RoomMutation,
) -> Result<Applied, &'static str> {
    match mutation {
        RoomMutation::CreateRoom { room } => {
            if rooms.iter().any(|r| r.id == room.id) {
                return Err("exists");
            }
            let id = room.id.clone();
            rooms.push(room);
            Ok(Applied {
                touched: Some(id),
                order: true,
                structural: true,
                ..Applied::default()
            })
        }
        RoomMutation::UpdateRoom { room } => {
            let slot = find(rooms, &room.id)?;
            let mut next = room;
            next.harnesses = std::mem::take(&mut slot.harnesses);
            let structural = !room_key_eq(slot, &next);
            let id = next.id.clone();
            *slot = next;
            Ok(touched(id, structural))
        }
        RoomMutation::RemoveRoom { room_id } => {
            let Some(pos) = rooms.iter().position(|r| r.id == room_id) else {
                return Err("absent");
            };
            rooms.remove(pos);
            Ok(Applied {
                removed: Some(room_id),
                order: true,
                structural: true,
                ..Applied::default()
            })
        }
        RoomMutation::ReorderRooms { room_ids } => {
            reorder(rooms, &room_ids, |r| &r.id);
            Ok(Applied {
                order: true,
                structural: true,
                ..Applied::default()
            })
        }
        RoomMutation::PutHarness { room_id, harness } => {
            let room = find(rooms, &room_id)?;
            let structural =
                if let Some(slot) = room.harnesses.iter_mut().find(|h| h.id == harness.id) {
                    let s = !harness_key_eq(slot, &harness);
                    *slot = harness;
                    s
                } else {
                    room.harnesses.push(harness);
                    true
                };
            Ok(touched(room_id, structural))
        }
        RoomMutation::RemoveHarness {
            room_id,
            harness_id,
        } => {
            let room = find(rooms, &room_id)?;
            let Some(pos) = room.harnesses.iter().position(|h| h.id == harness_id) else {
                return Err("absent");
            };
            room.harnesses.remove(pos);
            Ok(touched(room_id, true))
        }
        RoomMutation::ReorderHarnesses {
            room_id,
            harness_ids,
        } => {
            let room = find(rooms, &room_id)?;
            reorder(&mut room.harnesses, &harness_ids, |h| &h.id);
            Ok(touched(room_id, true))
        }
    }
}

fn touched(id: String, structural: bool) -> Applied {
    Applied {
        touched: Some(id),
        structural,
        ..Applied::default()
    }
}

fn find<'a>(rooms: &'a mut [Room], id: &str) -> Result<&'a mut Room, &'static str> {
    rooms.iter_mut().find(|r| r.id == id).ok_or("unknown_room")
}

/// Listed ids take the listed order; unknown ids are ignored; unlisted
/// items keep their relative order after the listed ones.
fn reorder<T>(items: &mut [T], ids: &[String], id_of: impl Fn(&T) -> &str) {
    let mut rank: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
    let mut seen = HashSet::new();
    for id in ids {
        if seen.insert(id.as_str()) {
            let next = rank.len();
            rank.insert(id.as_str(), next);
        }
    }
    // Stable sort: unlisted items share the highest rank and keep order.
    items.sort_by_key(|item| rank.get(id_of(item)).copied().unwrap_or(usize::MAX));
}
