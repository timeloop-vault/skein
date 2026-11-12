//! Per-room mutations and the change events they produce (#729). Clients
//! send [`RoomMutation`]s; the daemon applies them and broadcasts
//! [`RoomsChanged`]. Pure data; the apply logic lives in `skein-daemon`.

use serde::{Deserialize, Serialize};

use crate::rooms::{Harness, Room};

/// Name of the event [`RoomsChanged`] is broadcast on.
pub const ROOMS_CHANGED_EVENT: &str = "skein://rooms-changed";

/// One edit a client asks the daemon to make to the room list.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
#[allow(clippy::large_enum_variant)] // wire type, built once per change; boxing would burden every caller
pub enum RoomMutation {
    /// Whole room including harnesses; appended at the end.
    CreateRoom {
        room: Room,
    },
    /// Room-level fields only; `harnesses` in the payload is ignored.
    UpdateRoom {
        room: Room,
    },
    RemoveRoom {
        room_id: String,
    },
    ReorderRooms {
        room_ids: Vec<String>,
    },
    /// Replace by id, or append if new.
    PutHarness {
        room_id: String,
        harness: Harness,
    },
    RemoveHarness {
        room_id: String,
        harness_id: String,
    },
    ReorderHarnesses {
        room_id: String,
        harness_ids: Vec<String>,
    },
}

/// The resulting state after a batch; idempotent to apply.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
#[allow(clippy::large_enum_variant)] // wire type, built once per change; boxing would burden every caller
pub enum RoomChange {
    /// Full room after the change (create, update, any harness change).
    RoomPut {
        room: Room,
    },
    RoomRemoved {
        room_id: String,
    },
    /// Full order of all rooms after the change.
    RoomsOrder {
        room_ids: Vec<String>,
    },
}

/// Event payload; `origin` is the client id of the sender.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RoomsChanged {
    pub origin: String,
    pub changes: Vec<RoomChange>,
}

/// A mutation the daemon did not apply; `index` is into the sent array.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkippedMutation {
    pub index: usize,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RoomsApplyOutcome {
    pub skipped: Vec<SkippedMutation>,
}

#[cfg(test)]
mod tests;
