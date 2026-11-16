import { useEffect, useRef } from "react";
import { useGlobalControlCenterShortcut } from "./controlCenter/useGlobalShortcut.ts";
import { startPhaseMirror } from "./phaseMirror.ts";
import { versionNoticeModeFor } from "./prefs.ts";
import { startScreenReadingFeed } from "./screenPhaseTick.ts";
import type { NewToast } from "./toastStack.ts";
import { useAgentRequests } from "./useAgentRequests.ts";
import { useAgentRoomCreated } from "./useAgentRoomCreated.ts";
import type { useAppSettings } from "./useAppSettings.ts";
import { useClaudeVersionWatch } from "./useClaudeVersionWatch.ts";
import { useDaemonDelivery } from "./useDaemonDelivery.ts";
import type { useHarnessActions } from "./useHarnessActions.ts";
import { useHarnessContextSizes } from "./useHarnessContextSizes.ts";
import { useMailDisplay } from "./useMailDisplay.ts";
import { useOpenRequests } from "./useOpenRequests.ts";
import { useOsNotificationClicks } from "./useOsNotificationClicks.ts";
import type { useRoomStripNav } from "./useRoomStripNav.ts";
import type { useRoomsStore } from "./useRoomsStore.ts";

type Store = ReturnType<typeof useRoomsStore>;

// The hooks that only listen — OS-notification clicks, folder-open
// requests, agent mail, the phase mirror and the agent API's frontend half.
// Nothing here renders; App passes in what they each need.
export function useAppBackgroundWiring(a: {
	store: Store;
	nav: ReturnType<typeof useRoomStripNav>;
	actions: ReturnType<typeof useHarnessActions>;
	settings: ReturnType<typeof useAppSettings>;
	cc: { openPopout: () => void };
	pushToast: (entry: NewToast) => void;
}) {
	const { store, nav, actions, settings } = a;
	const { roomsRef, activeRoomIdRef, unarchiveRoomRef, loaded, loadedRef } = store;
	// #19: OS-notification click handling — see useOsNotificationClicks.ts.
	const switchRoomRef = useRef(store.switchRoom);
	switchRoomRef.current = store.switchRoom;
	const selectHarnessRef = useRef(actions.selectHarness);
	selectHarnessRef.current = actions.selectHarness;
	useOsNotificationClicks(
		roomsRef,
		unarchiveRoomRef,
		switchRoomRef,
		selectHarnessRef,
		loaded,
		loadedRef,
	);

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

	// #329/#733: agent-mail display — the unread marker and the held mirror.
	// The daemon delivers; `useDaemonDelivery` reports the client-only facts
	// it needs. Scoped to active rooms only.
	useMailDisplay(store.activeRooms);
	useDaemonDelivery();
	useHarnessContextSizes(store.activeRooms); // #368

	// #491: Claude Code update notice — installed-version probe + auto policy.
	useClaudeVersionWatch(
		roomsRef,
		actions.restartHarness,
		versionNoticeModeFor(settings.versionNoticeModes, "claude"),
	);

	// #732: the daemon owns the harness phase machine; this mirrors its stream
	// and feeds it the screen readings only the webview can take. Both are
	// idempotent, so StrictMode's double mount is harmless.
	useEffect(() => {
		void startPhaseMirror();
		return startScreenReadingFeed();
	}, []);

	// The webview's half of #328's `skein://agent-request` round trip: the
	// design-pane requests only (the room and harness verbs run in the
	// daemon since #734).
	useAgentRequests(
		roomsRef,
		actions.setHarnessDesignEntry,
		actions.setHarnessDesignDevice,
		activeRoomIdRef,
		actions.switchHarnessInRoom,
		(harnessId) => settings.dockedDesign[harnessId] === true,
		settings.showDockedDesign,
		store.switchRoom,
	);

	// #734: the daemon creates the room for `create_room`; this shows the toast.
	useAgentRoomCreated(a.pushToast);

	// #559: OS-wide raise of the Control Center pop-out. App renders only in
	// the main window (main.tsx swaps in ControlCenterPopout for the pop-out),
	// so this can never register from the second webview.
	return useGlobalControlCenterShortcut(settings.globalCcShortcut, a.cc.openPopout);
}
