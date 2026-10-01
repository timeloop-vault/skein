import { type Dispatch, type MutableRefObject, type SetStateAction, useMemo, useRef } from "react";
import { allRoomOrder } from "./roomGroups.ts";
import type { Room } from "./types.ts";
import type { useAppSettings } from "./useAppSettings.ts";
import type { useHarnessActions } from "./useHarnessActions.ts";
import type { useHarnessCreation } from "./useHarnessCreation.ts";
import { useKeyboardShortcuts } from "./useKeyboardShortcuts.ts";
import type { useRoomStripNav } from "./useRoomStripNav.ts";
import type { useRoomsStore } from "./useRoomsStore.ts";

type Actions = ReturnType<typeof useHarnessActions>;
type Store = ReturnType<typeof useRoomsStore>;
type Nav = ReturnType<typeof useRoomStripNav>;
type Creation = ReturnType<typeof useHarnessCreation>;
type Settings = ReturnType<typeof useAppSettings>;

// Window-level keyboard shortcuts. Uses isAppShortcut as the gate —
// that same predicate also makes LiveTerminal's xterm custom handler
// return false for these combos, so the byte never reaches the PTY.
// preventDefault stops the WebView's defaults (Mod+W close, Mod+=
// zoom, Mod+1..9 tab jump, etc). Mod = ⌘ on macOS, Ctrl elsewhere.
//
// Stashes the per-render handler refs so the listener can stay bound
// across renders without re-listing every callback as a dep. Returns the
// two toggle refs the command palette also reads (#212 Mod+R, #185 Mod+E).
export function useAppShortcuts(a: {
	store: Store;
	nav: Nav;
	actions: Actions;
	creation: Creation;
	settings: Settings;
	activeRoomsRef: MutableRefObject<Room[]>;
	lastUsedByGroupRef: MutableRefObject<Map<string, string>>;
	setShowPalette: Dispatch<SetStateAction<boolean>>;
	setShowSettings: Dispatch<SetStateAction<boolean>>;
}) {
	const { store, nav, actions, creation, settings } = a;
	const { activeRoomId } = store;
	// #76: derived from `stripSegments` — keyboard nav is the only consumer.
	const visibleOrderRooms = useMemo(() => allRoomOrder(nav.stripSegments), [nav.stripSegments]);
	const visibleOrderRef = useRef(visibleOrderRooms);
	visibleOrderRef.current = visibleOrderRooms;

	const addHarnessRef = useRef(actions.addHarness);
	addHarnessRef.current = actions.addHarness;
	const closeRoomRef = useRef(store.closeRoom);
	closeRoomRef.current = store.closeRoom;
	const switchHarnessInRoomRef = useRef(actions.switchHarnessInRoom);
	switchHarnessInRoomRef.current = actions.switchHarnessInRoom;
	const cycleAlertedRoomRef = useRef(actions.cycleAlertedRoom);
	cycleAlertedRoomRef.current = actions.cycleAlertedRoom;
	const cycleAlertedHarnessRef = useRef(actions.cycleAlertedHarness);
	cycleAlertedHarnessRef.current = actions.cycleAlertedHarness;
	// toggleFilesHarness comes from useHarnessCreation (#19); the palette
	// reads it through the ref, not the hook's return value directly.
	const toggleFilesRef = useRef<() => void>(() => {});
	toggleFilesRef.current = creation.toggleFilesHarness;
	// Mod+R (#212). Unlike Mod+E this needs nothing created — the review
	// pane is always mounted. Reassigned every render so it closes over
	// the current active room.
	const toggleReviewRef = useRef<() => void>(() => {});
	toggleReviewRef.current = () => {
		if (!activeRoomId) return;
		settings.setRightPaneTab(
			activeRoomId,
			(settings.rightPaneTabs[activeRoomId] ?? "context") === "review" ? "context" : "review",
		);
	};

	// #19: the window-level keyboard shortcut listener (cycleRoom,
	// cycleHarness, and the Mod+… switch) — see useKeyboardShortcuts.ts.
	useKeyboardShortcuts(
		visibleOrderRef,
		nav.stripSegmentsRef,
		store.activeRoomIdRef,
		a.activeRoomsRef,
		switchHarnessInRoomRef,
		addHarnessRef,
		closeRoomRef,
		cycleAlertedRoomRef,
		cycleAlertedHarnessRef,
		toggleFilesRef,
		toggleReviewRef,
		a.lastUsedByGroupRef,
		store.setActiveRoomId,
		a.setShowPalette,
		a.setShowSettings,
		settings.setFontSize,
		nav.openNewRoom,
	);
	return { toggleFilesRef, toggleReviewRef };
}
