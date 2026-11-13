// Merging the daemon's argv results back onto the frontend's records (#730).
//
// The Rust Room/Harness structs have no catch-all, so a record that
// round-trips through the daemon comes back WITHOUT any field Rust does
// not know. A returned record is therefore never installed wholesale:
// these helpers copy the few fields the daemon returns (cmd, launch,
// shellClaim) and leave everything else on the frontend record alone.
// NOT sessionId or opencodeVersion (#755): the daemon owns them in the
// store, the local copy is kept as is, and the spawn derives resume-versus-
// fresh (and any minted or dropped id) from the stored id itself.
// Absence is copied too - a returned harness without `shellClaim` means
// the claim was consumed; without `cmd`, there is no argv to store.
// Pure; matching is by id, so apply against CURRENT state.

import type { Harness, Room } from "./types.ts";

/** The daemon-owned fields of `returned` laid over `h`. */
export const applyResumedHarness = (h: Harness, returned: Harness | undefined): Harness => {
	if (!returned) return h;
	const { cmd: _cmd, shellClaim: _claim, launch: _launch, ...rest } = h;
	return {
		...rest,
		...(returned.launch ? { launch: returned.launch } : {}),
		...(returned.cmd ? { cmd: returned.cmd } : {}),
		...(returned.shellClaim ? { shellClaim: returned.shellClaim } : {}),
	};
};

/** `applyResumedHarness` for every harness of `room`, matched by id. With
 *  `unarchive`, also drops the room fields the daemon clears on reopen
 *  (`archived`, `retired`, `closedBy`). */
export const applyResumedRoom = (
	room: Room,
	returned: Room | undefined,
	opts: { unarchive?: boolean } = {},
): Room => {
	if (!returned) return room;
	const byId = new Map(returned.harnesses.map((h) => [h.id, h]));
	const harnesses = room.harnesses.map((h) => applyResumedHarness(h, byId.get(h.id)));
	if (!opts.unarchive) return { ...room, harnesses };
	const { archived: _a, retired: _r, closedBy: _c, ...rest } = room;
	return { ...rest, harnesses };
};

/** `applyResumedRoom` across a list, matched by room id. */
export const applyResumedRooms = (rooms: Room[], returned: Room[]): Room[] => {
	const byId = new Map(returned.map((r) => [r.id, r]));
	return rooms.map((r) => applyResumedRoom(r, byId.get(r.id)));
};
