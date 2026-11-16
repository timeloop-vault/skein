// The one-way `skein://agent-room-created` event (#734): the daemon now
// creates the room for the `create_room` verb itself and only tells the
// webview, so it can show the "opened by <room>" toast.

import { HARNESS_ORDER } from "./data.tsx";
import type { NewToast } from "./toastStack.ts";
import type { HarnessKind } from "./types.ts";

export const AGENT_ROOM_CREATED_EVENT = "skein://agent-room-created";

export interface AgentRoomCreated {
	roomId: string;
	roomName: string;
	harnessId: string;
	harnessName: string;
	kind: HarnessKind;
	requesterRoomName: string;
}

/** The payload, or null unless every field is a string, `harnessId` is
 *  non-empty and `kind` is a known harness kind. */
export function parseAgentRoomCreated(raw: unknown): AgentRoomCreated | null {
	if (typeof raw !== "object" || raw === null) return null;
	const { roomId, roomName, harnessId, harnessName, kind, requesterRoomName } = raw as Record<
		string,
		unknown
	>;
	if (
		typeof roomId !== "string" ||
		typeof roomName !== "string" ||
		typeof harnessId !== "string" ||
		harnessId === "" ||
		typeof harnessName !== "string" ||
		typeof kind !== "string" ||
		!HARNESS_ORDER.includes(kind as HarnessKind) ||
		typeof requesterRoomName !== "string"
	) {
		return null;
	}
	return { roomId, roomName, harnessId, harnessName, kind: kind as HarnessKind, requesterRoomName };
}

/** The "created" toast, built from the payload alone. */
export function agentRoomCreatedToast(p: AgentRoomCreated, id: string): NewToast {
	return {
		id,
		roomId: p.roomId,
		harnessId: p.harnessId,
		kind: p.kind,
		roomName: p.roomName,
		harnessName: p.harnessName,
		state: "created",
		requesterRoomName: p.requesterRoomName,
	};
}
