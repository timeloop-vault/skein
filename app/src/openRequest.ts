// Epic #255: opening a folder in Skein from outside it — `skein .`, a
// `skein://open?path=…` link, a second launch with a path. Rust
// (`open_request.rs`) owns arrival and resolution: it canonicalizes the
// path and matches it against the stored rooms with the same matcher
// the agent API's `find_rooms_for_path` uses. What is left for the
// frontend is the one thing only it knows — which room you are in now —
// so this module is that decision, pure and table-tested; the glue is
// `useOpenRequests.ts`.

import type { NewToast } from "./toastStack.ts";
import type { Room } from "./types.ts";

export const LINK_ROOM_GONE = "Skein link: that room no longer exists";
export const LINK_HARNESS_GONE = "Skein link: that harness no longer exists";

/** The info toast for a link that named something gone. */
export function linkNoticeToast(
	notice: "roomGone" | "harnessGone",
	room: Pick<Room, "id" | "name"> | undefined,
	now: number,
	rand: number,
): NewToast {
	return {
		id: `t_${now}_${rand.toString(36).slice(2, 8)}`,
		roomId: room?.id ?? "",
		harnessId: "",
		kind: "byoh",
		roomName: room?.name ?? "",
		harnessName: "",
		state: "info",
		message: notice === "roomGone" ? LINK_ROOM_GONE : LINK_HARNESS_GONE,
	};
}

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
	| { kind: "missing"; path: string }
	/** A `skein://` room link: jump like an OS-notification click. */
	| { kind: "jump"; roomId: string; harnessId: string | null };

export type OpenAction =
	/** `notice` says what the link named that no longer exists. */
	| {
			kind: "jump";
			roomId: string;
			harnessId: string | null;
			notice: "roomGone" | "harnessGone" | null;
	  }
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
	if (target.kind === "jump") {
		const room = rooms.find((r) => r.id === target.roomId);
		const harnessGone =
			room !== undefined &&
			target.harnessId !== null &&
			!room.harnesses.some((h) => h.id === target.harnessId);
		return {
			...target,
			notice: room === undefined ? "roomGone" : harnessGone ? "harnessGone" : null,
		};
	}
	if (target.kind !== "room") return target;
	const known = target.roomIds.filter((id) => rooms.some((r) => r.id === id));
	if (activeRoomId !== null && known.includes(activeRoomId)) {
		return { kind: "focus", roomId: activeRoomId };
	}
	const first = known[0];
	if (first !== undefined) return { kind: "focus", roomId: first };
	return { kind: "newRoom", folder: target.folder };
}
