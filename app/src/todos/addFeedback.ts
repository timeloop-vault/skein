// The one place that adds a todo AND tells the user what happened (#335):
// shared by the Mod+T / Mod+Shift+T shortcuts and the tab context menu.
// The confirmation is an "info" toast (toastStack.ts) pushed directly, so it
// never reaches the OS-notification path.

import type { MutableRefObject } from "react";
import type { NewToast } from "../toastStack.ts";
import type { Room } from "../types.ts";
import type { TodoScope } from "./model.ts";

export function todoAddMessage(scope: TodoScope, added: boolean): string {
	const list = scope === "global" ? "global" : "room";
	return added ? `Added to ${list} todos` : `Already in ${list} todos`;
}

export type AddTodoWithFeedback = (scope: TodoScope, roomId: string, harnessId?: string) => void;

export function makeAddTodoWithFeedback(
	roomsRef: MutableRefObject<Room[]>,
	add: (scope: TodoScope, roomId: string, harnessId?: string) => boolean,
	pushToast: (entry: NewToast) => void,
): AddTodoWithFeedback {
	return (scope, roomId, harnessId) => {
		const room = roomsRef.current.find((r) => r.id === roomId);
		if (!room) return;
		const added = add(scope, roomId, harnessId);
		const harness =
			room.harnesses.find((h) => h.id === (harnessId ?? room.activeHarnessId)) ?? room.harnesses[0];
		pushToast({
			id: `t_${Date.now()}_${Math.random().toString(36).slice(2, 8)}`,
			roomId,
			harnessId: harness?.id ?? "",
			kind: harness?.kind ?? "byoh",
			roomName: room.name,
			harnessName: harness?.name ?? "",
			state: "info",
			message: todoAddMessage(scope, added),
		});
	};
}

/** The (room, harness) a shortcut targets: the active room and its active
 *  harness, or room-only when it has none. */
export function activeTodoTarget(
	rooms: readonly Room[],
	activeRoomId: string,
): { roomId: string; harnessId?: string } | null {
	const room = rooms.find((r) => r.id === activeRoomId);
	if (!room) return null;
	const harness = room.harnesses.find((h) => h.id === room.activeHarnessId);
	return harness ? { roomId: room.id, harnessId: harness.id } : { roomId: room.id };
}
