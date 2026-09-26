// Epic #255: opening a folder in Skein from outside it — `skein .`, a
// `skein://open?path=…` link, a second launch with a path. Rust
// (`open_request.rs`) owns arrival and resolution: it canonicalizes the
// path and matches it against the stored rooms with the same matcher
// the agent API's `find_rooms_for_path` uses. What is left for the
// frontend is the one thing only it knows — which room you are in now —
// so this module is that decision, pure and table-tested; the glue is
// `useOpenRequests.ts`.

import type { Room } from "./types.ts";

/** Mirror of `open_request::OpenTarget`. */
export type OpenTarget =
	| {
			kind: "room";
			/** Equally good owners, best first: all open, or all archived
			 *  (most recently archived first). */
			roomIds: string[];
			archived: boolean;
			/** The requested folder, canonical — New room's seed if none
			 *  of `roomIds` still exists here. */
			folder: string;
	  }
	| { kind: "newRoom"; folder: string }
	| { kind: "missing"; path: string };

export type OpenAction =
	/** Focus the room — `unarchiveRoom` reopens it first when archived,
	 *  and only focuses it when it is already open. */
	| { kind: "focus"; roomId: string }
	| { kind: "newRoom"; folder: string }
	| { kind: "missing"; path: string };

/** What to do with a resolved request, given the rooms loaded now and
 *  the room on screen. When several open rooms share the folder and
 *  you are already in one of them, you stay there — `skein .` from a
 *  room's own terminal must never bounce you to a sibling. A match
 *  Rust saw but this list no longer has (closed and deleted in between)
 *  falls through to New room rather than doing nothing. */
export function decideOpen(
	target: OpenTarget,
	rooms: readonly Room[],
	activeRoomId: string | null,
): OpenAction {
	if (target.kind !== "room") return target;
	const known = target.roomIds.filter((id) => rooms.some((r) => r.id === id));
	if (activeRoomId !== null && known.includes(activeRoomId)) {
		return { kind: "focus", roomId: activeRoomId };
	}
	const first = known[0];
	if (first !== undefined) return { kind: "focus", roomId: first };
	return { kind: "newRoom", folder: target.folder };
}
