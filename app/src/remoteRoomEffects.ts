// What the client must do after rooms changed under it that it did not
// make itself (#734): the daemon applies agent verbs such as close_room and
// close_harness, and `skein://rooms-changed` only delivers the records. The
// client-side follow-ups the removed `closeRoomForAgent` / `closeHarnessForAgent`
// did are derived here from the rooms before and after, so they hold for any
// change. Both parts are no-ops when this client already did them itself.

import { nextActiveAfterClose } from "./roomGroups.ts";
import type { Room } from "./types.ts";

function openHarnessIds(rooms: readonly Room[]): Set<string> {
	const ids = new Set<string>();
	for (const r of rooms) {
		if (r.archived) continue;
		for (const h of r.harnesses) ids.add(h.id);
	}
	return ids;
}

/** Harness ids that were live in `prev` and are gone (removed or archived) in `next`. */
export function goneHarnessIds(prev: readonly Room[], next: readonly Room[]): string[] {
	const still = openHarnessIds(next);
	return [...openHarnessIds(prev)].filter((id) => !still.has(id));
}

/**
 * The room id to switch to when `activeRoomId` names a room that is now
 * archived or gone, or null when the active room is fine (including "" with
 * nothing selected yet). "" means no open room is left.
 */
export function repairedActiveRoomId(
	rooms: readonly Room[],
	activeRoomId: string,
	lastUsedByGroup: ReadonlyMap<string, string>,
): string | null {
	if (activeRoomId === "") return null;
	const active = rooms.find((r) => r.id === activeRoomId);
	if (active !== undefined && !active.archived) return null;
	// Keep the closed room in the list so its neighbours resolve as they do
	// for the user's own close.
	const basis = rooms.filter((r) => !r.archived || r.id === activeRoomId);
	return nextActiveAfterClose(basis, activeRoomId, lastUsedByGroup)?.id ?? "";
}
