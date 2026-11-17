//! Intent commands for the room list (#756). The finer-grained sibling of
//! [`crate::room_mutations::RoomMutation`]: a client says what it means
//! (rename, archive, add a harness) and the daemon works out the record.
//! Pure data; the logic lives in `skein-daemon`'s `room_store/commands.rs`.

use serde::{Deserialize, Serialize};

use crate::rooms::{Harness, Room};

/// Who closed a room, as the caller names it. The daemon adds the time.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ClosedByRef {
    pub room_id: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub harness_id: Option<String>,
}

/// One intent a client asks the daemon to carry out.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
#[allow(clippy::large_enum_variant)] // wire type, built once per change; boxing would burden every caller
pub enum RoomCommand {
    /// Whole new room including harnesses, appended. Refused `exists`.
    CreateRoom {
        room: Room,
    },
    RenameRoom {
        room_id: String,
        name: String,
    },
    /// Sets `archived` to the daemon's clock (epoch ms). `closed_by` becomes
    /// `Room.closed_by` with `at` the same ms; `None` clears it. Refused
    /// `already_archived` if the room is archived.
    ArchiveRoom {
        room_id: String,
        #[serde(default)]
        closed_by: Option<ClosedByRef>,
    },
    /// Clears `archived`, `retired` and `closed_by`. Refused `not_archived`.
    UnarchiveRoom {
        room_id: String,
    },
    ReorderRooms {
        room_ids: Vec<String>,
    },
    /// Appends a new harness. Refused `exists` if the id is already in the
    /// room. `activate` also makes it the room's active harness.
    AddHarness {
        room_id: String,
        harness: Harness,
        #[serde(default)]
        activate: bool,
    },
    /// Removes the harness; if it was the active one, the active harness
    /// moves to the first remaining one (or `""`). Refused `absent`.
    CloseHarness {
        room_id: String,
        harness_id: String,
    },
    /// The record half of "Restart harness": back to the tool. Clears
    /// `launch` and `shell_claim`; nothing changes (no touch) if both were
    /// already clear.
    RestartHarness {
        room_id: String,
        harness_id: String,
    },
    /// `None` = the tool's own default. Takes effect at the next spawn.
    SetHarnessAgent {
        room_id: String,
        harness_id: String,
        #[serde(default)]
        agent: Option<String>,
    },
    ReorderHarnesses {
        room_id: String,
        harness_ids: Vec<String>,
    },
    /// Field-level patch of room-level fields under optimistic concurrency:
    /// `expected_revision` must be the room's revision when the batch began.
    /// Top-level JSON merge; a `null` removes the key.
    PatchRoom {
        room_id: String,
        expected_revision: u64,
        fields: serde_json::Map<String, serde_json::Value>,
    },
    /// As `PatchRoom`, for one harness record.
    PatchHarness {
        room_id: String,
        harness_id: String,
        expected_revision: u64,
        fields: serde_json::Map<String, serde_json::Value>,
    },
}

/// A command the daemon did not carry out; `index` is into the sent array.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RefusedCommand {
    pub index: usize,
    pub reason: String,
}

/// A room's revision after a batch.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RoomRevision {
    pub room_id: String,
    pub revision: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RoomsExecuteOutcome {
    pub refused: Vec<RefusedCommand>,
    /// Revision of every room this batch touched, after the batch.
    pub revisions: Vec<RoomRevision>,
}

#[cfg(test)]
mod tests;
