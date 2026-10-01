import { useEffect } from "react";
import { startSupervisor } from "./supervisor/runtime.ts";
import { useAgentRequests } from "./useAgentRequests.ts";
import type { useAppSettings } from "./useAppSettings.ts";
import type { useHarnessActions } from "./useHarnessActions.ts";
import type { useHarnessCreation } from "./useHarnessCreation.ts";
import { useMailDelivery } from "./useMailDelivery.ts";
import { useOpenRequests } from "./useOpenRequests.ts";
import { useOsNotificationClicks } from "./useOsNotificationClicks.ts";
import type { useRoomStripNav } from "./useRoomStripNav.ts";
import type { useRoomsStore } from "./useRoomsStore.ts";

type Store = ReturnType<typeof useRoomsStore>;

// The hooks that only listen — OS-notification clicks, folder-open
// requests, agent mail, the supervisor and the agent API's frontend half.
// Nothing here renders; App passes in what they each need.
export function useAppBackgroundWiring(a: {
	store: Store;
	nav: ReturnType<typeof useRoomStripNav>;
	actions: ReturnType<typeof useHarnessActions>;
	creation: ReturnType<typeof useHarnessCreation>;
	settings: ReturnType<typeof useAppSettings>;
	pushToast: Parameters<typeof useAgentRequests>[4];
}) {
	const { store, nav, actions, creation, settings } = a;
	const { roomsRef, activeRoomIdRef, unarchiveRoomRef, setRooms, loaded, loadedRef } = store;
	// #19: OS-notification click handling — see useOsNotificationClicks.ts.
	useOsNotificationClicks(roomsRef, unarchiveRoomRef, setRooms, loaded, loadedRef);

	// Epic #255: a folder opened from outside Skein (`skein .`, a
	// `skein://open` link, a second launch with a path); see useOpenRequests.ts.
	useOpenRequests(
		roomsRef,
		activeRoomIdRef,
		unarchiveRoomRef,
		nav.openNewRoom,
		nav.openNewRoomAt,
		loaded,
		loadedRef,
	);

	// #329: agent-mail delivery — nudges a waiting harness's mailbox and
	// keeps the tab marker fresh. Scoped to active rooms only: an archived
	// room's harnesses have no live PTY to nudge.
	useMailDelivery(store.activeRooms);

	// #423: the harness supervisor rides the activity tick; idempotent, so
	// StrictMode's double mount is harmless.
	useEffect(() => startSupervisor(), []);

	// #330: the `create_room` agent verb's frontend half — answers
	// #328's `skein://agent-request` round trip. Unlike `useMailDelivery`
	// this isn't scoped to `activeRooms`: a request can name any folder
	// on the machine, not just an already-open room.
	useAgentRequests(
		creation.createRoom,
		nav.newRoomMemory,
		settings.defaultAgents,
		settings.branchTemplate,
		a.pushToast,
		store.closeRoomForAgent,
		roomsRef,
		creation.createHarnessInRoom,
		actions.closeHarnessForAgent,
	);
}
