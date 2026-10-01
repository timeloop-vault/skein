import { useRef, useState } from "react";
import type { RenameTarget } from "./RoomStrip.tsx";
import type { Room } from "./types.ts";
import { useAppWindowEffects } from "./useAppWindowEffects.ts";
import { useSpawnEnv } from "./useSpawnEnv.ts";

// The app-level UI state that exists before the rooms store does: which
// overlays are open, the inline-rename target, the boot-time platform
// defaults, and the two refs useRoomsStore/useAppWindowEffects share. The
// six standalone window effects (#19) and the spawn-env mirror ride along —
// none of them interacts with anything else in App.
export function useAppUiState() {
	const [showPicker, setShowPicker] = useState<string | null>(null);
	const [showPalette, setShowPalette] = useState(false);
	const [showSettings, setShowSettings] = useState(false);
	const [showReopen, setShowReopen] = useState(false);
	// #241: which room (if any) is mid inline-rename, and which tab hosts
	// the input (`RenameTarget.host` — see RoomStrip.tsx). Display only —
	// commit touches `Room.name` alone.
	const [renaming, setRenaming] = useState<RenameTarget | null>(null);

	// Phase 1: platform defaults pulled once at boot (useAppWindowEffects).
	// New harnesses spawn into these.
	const [defaultShell, setDefaultShell] = useState<string[]>([]);
	const [defaultCwd, setDefaultCwd] = useState<string>("");
	// #331: `rooms` for the status-popover breakdown — created before
	// useRoomsStore because useAppWindowEffects (which attaches the
	// popover) mounts first; App keeps it in sync after useRoomsStore.
	const popoverRoomsRef = useRef<readonly Room[]>([]);
	// #76: the room last used in each group — created before
	// useRoomsStore so #334's closeRoom can read it; useRoomStripNav owns
	// writing to it.
	const lastUsedByGroupRef = useRef<Map<string, string>>(new Map());
	// Esc-closes-picker, quit confirmation, open-settings listener, #132
	// status popover, #120 stray-file-drop swallow, the default shell/cwd
	// probe — see useAppWindowEffects.ts.
	useAppWindowEffects(
		showPicker,
		setShowPicker,
		setShowSettings,
		setDefaultShell,
		setDefaultCwd,
		() => popoverRoomsRef.current,
	);
	const { spawnEnv, saveSpawnSettings } = useSpawnEnv(setDefaultShell);

	return {
		showPicker,
		setShowPicker,
		showPalette,
		setShowPalette,
		showSettings,
		setShowSettings,
		showReopen,
		setShowReopen,
		renaming,
		setRenaming,
		defaultShell,
		defaultCwd,
		popoverRoomsRef,
		lastUsedByGroupRef,
		spawnEnv,
		saveSpawnSettings,
	};
}
