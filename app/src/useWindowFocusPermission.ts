// L5b — window-focus state + OS-notification permission, for
// useHarnessNotifications (#459 split; no behaviour change). The
// notification logic skips firing an OS banner when Skein is the focused
// app, because the user is already looking at the badge update in real
// time and an extra OS-level banner is just noise. Refs (not state) since
// the transition callback reads these synchronously and we don't want
// them to retrigger the subscription effect on every focus change.

import {
	isPermissionGranted,
	requestPermission,
} from "@choochmeque/tauri-plugin-notifications-api";
import { getCurrentWindow } from "@tauri-apps/api/window";
import type { Dispatch, MutableRefObject, SetStateAction } from "react";
import { useEffect } from "react";
import { clearHarnessPending } from "./harnessNotifyLogic.ts";
import type { Room } from "./types.ts";

export type NotificationPermission = "unknown" | "granted" | "denied";

export function useWindowFocusPermission(
	roomsRef: MutableRefObject<Room[]>,
	activeRoomIdRef: MutableRefObject<string>,
	setRooms: Dispatch<SetStateAction<Room[]>>,
	windowFocusedRef: MutableRefObject<boolean>,
	notificationPermissionRef: MutableRefObject<NotificationPermission>,
) {
	// biome-ignore lint/correctness/useExhaustiveDependencies: roomsRef/activeRoomIdRef/setRooms come from useRoomsStore (#19) — refs/a setState setter, stable across renders, but biome can't prove that through a destructured custom-hook return.
	useEffect(() => {
		const win = getCurrentWindow();
		let unlisten: (() => void) | null = null;
		// Clear the badge on the currently-displayed harness. Used
		// both by the (activeRoomId, displayedHarnessId) effect
		// AND by the focus listener — coming back to Skein on the
		// same harness you alt+tabbed away from also counts as
		// "viewing it now," but that effect doesn't re-fire because
		// neither tuple value changed. Hook the focus→true edge
		// instead.
		const clearDisplayedHarnessPending = () => {
			const room = roomsRef.current.find((r) => r.id === activeRoomIdRef.current);
			if (!room) return;
			const displayedH = room.activeHarnessId;
			if (!displayedH) return;
			setRooms((prev) => clearHarnessPending(prev, room.id, displayedH));
		};
		void win.isFocused().then((f) => {
			windowFocusedRef.current = f;
		});
		void win
			.onFocusChanged(({ payload }) => {
				windowFocusedRef.current = payload;
				if (payload) clearDisplayedHarnessPending();
			})
			.then((u) => {
				unlisten = u;
			});
		// Permission flow: prompt once on first run if the user
		// hasn't decided yet. The OS remembers the choice and
		// future `isPermissionGranted` calls return granted/denied
		// without re-prompting.
		void (async () => {
			try {
				const granted = await isPermissionGranted();
				if (granted) {
					notificationPermissionRef.current = "granted";
					return;
				}
				const result = await requestPermission();
				notificationPermissionRef.current = result === "granted" ? "granted" : "denied";
			} catch (err: unknown) {
				const msg = err instanceof Error ? err.message : String(err);
				console.warn("[skein] notification permission flow failed:", msg);
			}
		})();
		return () => {
			unlisten?.();
		};
	}, []);
}
