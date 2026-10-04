// The props AppOverlays takes, assembled from App's state (moved out of
// App.tsx to keep it under its size limit, #492).

import type { ComponentProps } from "react";
import type { AppOverlays } from "./AppOverlays.tsx";
import type { useAppSettings } from "./useAppSettings.ts";
import type { useRoomStripNav } from "./useRoomStripNav.ts";
import type { useRoomsStore } from "./useRoomsStore.ts";

type Overlays = ComponentProps<typeof AppOverlays>;

export function buildOverlayProps(
	nav: ReturnType<typeof useRoomStripNav>,
	store: ReturnType<typeof useRoomsStore>,
	settings: ReturnType<typeof useAppSettings>,
	rest: Omit<
		Overlays,
		| "showNewRoom"
		| "newRoomSeed"
		| "defaultAgents"
		| "branchTemplate"
		| "recentRoomFolders"
		| "rememberRoomFolder"
		| "setShowNewRoom"
		| "archivedRooms"
		| "allRooms"
		| "deleteRoomsForever"
		| "restoreRooms"
		| "retireRooms"
		| "unretireRooms"
		| "quarantinedCount"
		| "setQuarantinedCount"
		| "backupRoomCount"
		| "setBackupRoomCount"
		| "newRoomMemory"
	>,
): Overlays {
	return {
		...rest,
		showNewRoom: nav.showNewRoom,
		newRoomSeed: nav.newRoomSeed,
		defaultAgents: settings.defaultAgents,
		newRoomMemory: nav.newRoomMemory,
		branchTemplate: settings.branchTemplate,
		recentRoomFolders: nav.recentRoomFolders,
		rememberRoomFolder: nav.rememberRoomFolder,
		setShowNewRoom: nav.setShowNewRoom,
		archivedRooms: store.archivedRooms,
		allRooms: store.roomsRef.current,
		deleteRoomsForever: store.deleteRoomsForever,
		restoreRooms: store.restoreRooms,
		retireRooms: store.retireRooms,
		unretireRooms: store.unretireRooms,
		quarantinedCount: store.quarantinedCount,
		setQuarantinedCount: store.setQuarantinedCount,
		backupRoomCount: store.backupRoomCount,
		setBackupRoomCount: store.setBackupRoomCount,
	};
}
