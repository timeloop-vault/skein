import { type RequestResult, asRecord, isOmitted } from "./agentRequestsShared.ts";
import type { Room } from "./types.ts";

// ── close_room ──────────────────────────────────────────────────────
//
// #411: the frontend half of the `close_room` agent verb — Rust has
// already checked the creator/sign-off/rate-limit rules by the time
// this request lands, so all that's left here is the same refusal
// ladder the user's own close already has to clear implicitly (a room
// that doesn't exist or is already archived can't be closed, and
// dirty Files buffers can't be discarded without asking — except an
// agent call has no one to ask, so it refuses instead).

export interface CloseRoomAttribution {
	roomId: string;
	harnessId?: string;
}

export interface CloseRoomArgs {
	roomId: string;
	closedBy: CloseRoomAttribution;
}

/** Parse+validate the raw JSON `args` of a `close_room` request.
 *  `closedBy.harnessId` follows the same "absent means omitted" rule
 *  as `create_room`'s `createdBy` (see `isOmitted`'s doc comment) —
 *  Rust sends JSON `null` for a `None`, not a missing key. */
export function parseCloseRoomArgs(raw: unknown): RequestResult<CloseRoomArgs> {
	const r = asRecord(raw);
	if (!r) return { ok: false, error: "args must be an object" };
	if (typeof r.roomId !== "string" || !r.roomId) {
		return { ok: false, error: "roomId is required" };
	}
	const closedByRaw = asRecord(r.closedBy);
	if (!closedByRaw || typeof closedByRaw.roomId !== "string" || !closedByRaw.roomId) {
		return { ok: false, error: "closedBy must be {roomId, harnessId?}" };
	}
	if (!isOmitted(closedByRaw.harnessId) && typeof closedByRaw.harnessId !== "string") {
		return { ok: false, error: "closedBy.harnessId must be a string" };
	}
	return {
		ok: true,
		value: {
			roomId: r.roomId,
			closedBy: {
				roomId: closedByRaw.roomId,
				...(typeof closedByRaw.harnessId === "string" ? { harnessId: closedByRaw.harnessId } : {}),
			},
		},
	};
}

export interface CloseRoomDecision {
	room: Room;
	closedBy: NonNullable<Room["closedBy"]>;
}

/** The `close_room` refusal ladder, pure so it's testable without a
 *  rooms hook or a real `filesRegistry`: unknown room, then already
 *  archived, then unsaved Files buffers — in that order, since naming
 *  dirty files in a room that doesn't exist or is already closed would
 *  be a strange error to receive. `dirtyNamesFor` stands in for
 *  `filesRegistry.anyDirty`, injected rather than imported so a test
 *  doesn't have to register/unregister real registry entries. On
 *  success, `closedBy` is the stamped attribution ready to write onto
 *  the room — `at` filled in from `now` here rather than by the
 *  caller, so every accepted close is stamped the same way. */
export function decideCloseRoom(
	rooms: readonly Room[],
	roomId: string,
	closedBy: CloseRoomAttribution,
	dirtyNamesFor: (harnessIds: string[]) => string[],
	now: number,
): RequestResult<CloseRoomDecision> {
	const room = rooms.find((r) => r.id === roomId);
	if (!room) return { ok: false, error: `not_found: room "${roomId}" not found` };
	if (room.archived) return { ok: false, error: `archived: room "${roomId}" is already archived` };
	const dirty = dirtyNamesFor(room.harnesses.map((h) => h.id));
	if (dirty.length > 0) return { ok: false, error: `unsaved_files: ${dirty.join(", ")}` };
	return {
		ok: true,
		value: {
			room,
			closedBy: {
				roomId: closedBy.roomId,
				...(closedBy.harnessId ? { harnessId: closedBy.harnessId } : {}),
				at: now,
			},
		},
	};
}
