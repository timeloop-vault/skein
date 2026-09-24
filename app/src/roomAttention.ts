// Room-level attention mark (#328): the pure half of `Room.attention`
// (see its doc comment in types.ts for why the mark lives on the room
// rather than any harness). Pure module — no React — so the "clear on
// view" rule is testable without a DOM.

import type { Room } from "./types";

/// Clear `attention` on `activeId`, leaving every other room untouched.
/// Returns the SAME array reference when there is nothing to clear —
/// no active id, the active room not found, or the active room already
/// unmarked — so a caller can feed this straight into a `setRooms`
/// updater without forcing an extra re-render.
export function clearAttention(rooms: Room[], activeId: string | null): Room[] {
	if (!activeId) return rooms;
	const target = rooms.find((r) => r.id === activeId);
	if (!target?.attention) return rooms;
	return rooms.map((r) => {
		if (r.id !== activeId) return r;
		// `exactOptionalPropertyTypes` forbids `attention: undefined` —
		// drop the key entirely rather than set it to undefined.
		const { attention: _droppedAttention, ...rest } = r;
		return rest;
	});
}
