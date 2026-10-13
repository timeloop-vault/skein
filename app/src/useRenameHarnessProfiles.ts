import { type Dispatch, type SetStateAction, useEffect } from "react";
import { onProfilesRenamed } from "./envProfileStore.ts";
import { renameHarnessProfiles } from "./envProfiles.ts";
import type { Room } from "./types.ts";

// #619: a profile renamed in Settings keeps the harnesses that overrode it.
export function useRenameHarnessProfiles(setRooms: Dispatch<SetStateAction<Room[]>>) {
	useEffect(
		() => onProfilesRenamed((renames) => setRooms((prev) => renameHarnessProfiles(prev, renames))),
		[setRooms],
	);
}
