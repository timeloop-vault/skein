// #734: keeps the client consistent when the daemon changes rooms under it
// (agent verbs close_room / close_harness arrive only as `rooms-changed`).
// Runs on every `rooms` change; both parts are no-ops after this client's own
// close paths, which already moved the selection and forgot the harnesses.

import {
	type Dispatch,
	type MutableRefObject,
	type SetStateAction,
	useEffect,
	useRef,
} from "react";
import { claudeVersionStore } from "./claudeVersionStore.ts";
import type { RenameTarget } from "./RoomStrip.tsx";
import { goneHarnessIds, repairedActiveRoomId } from "./remoteRoomEffects.ts";
import { forgetReported } from "./reportedSessions.ts";
import type { Room } from "./types.ts";

export function useRemoteRoomRepair(
	rooms: Room[],
	loaded: boolean,
	activeRoomId: string,
	lastUsedByGroupRef: MutableRefObject<Map<string, string>>,
	setActiveRoomId: Dispatch<SetStateAction<string>>,
	setRenaming: Dispatch<SetStateAction<RenameTarget | null>>,
) {
	const prevRef = useRef<Room[] | null>(null);

	useEffect(() => {
		if (!loaded) return;
		const prev = prevRef.current;
		prevRef.current = rooms;
		if (prev !== null && prev !== rooms) {
			for (const id of goneHarnessIds(prev, rooms)) {
				claudeVersionStore.forget(id);
				forgetReported(id);
			}
		}
		const next = repairedActiveRoomId(rooms, activeRoomId, lastUsedByGroupRef.current);
		if (next === null) return;
		setActiveRoomId(next);
		setRenaming((cur) => (cur?.roomId === activeRoomId ? null : cur));
	}, [rooms, loaded, activeRoomId, lastUsedByGroupRef, setActiveRoomId, setRenaming]);
}
