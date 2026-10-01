// The notification engine, extracted out of App.tsx (#19) — pure move,
// no behaviour change. Owns the toast stack (`toasts`/`dismissToast`/
// `jumpToToast`), the notification-toggle/focus/permission refs, and the
// clear-pending-on-view effect, and calls the (#459) sub-hooks that hold
// the rest, in this order: useWindowFocusPermission, useHarnessHookEvents
// (`harness-permission` / `harness-session-start`),
// useTransitionNotifications (badge/toast/OS, L5a/L5b/L5c),
// useApiErrorToasts (D2f), useTransitionLog (L6). NOT here: the OS-notification
// click handling (`jumpToTarget`/`drainPendingClick`) — that stays in
// App.tsx as a separate, later extraction. App.tsx calls this hook at
// the point this state used to be declared and destructures the
// return value the same way it does `useRoomsStore`/`useHarnessCreation`.

import type { Dispatch, MutableRefObject, SetStateAction } from "react";
import { useEffect, useRef, useState } from "react";
import { clearHarnessPending, dropToastsFor } from "./harnessNotifyLogic.ts";
import { clearAttention } from "./roomAttention.ts";
import { type NewToast, type ToastEntry, appendToast } from "./toastStack.ts";
import type { Room } from "./types.ts";
import { useApiErrorToasts } from "./useApiErrorToasts.ts";
import { useHarnessHookEvents } from "./useHarnessHookEvents.ts";
import { useTransitionLog } from "./useTransitionLog.ts";
import { useTransitionNotifications } from "./useTransitionNotifications.ts";
import {
	type NotificationPermission,
	useWindowFocusPermission,
} from "./useWindowFocusPermission.ts";

export function useHarnessNotifications(
	roomsRef: MutableRefObject<Room[]>,
	activeRoomIdRef: MutableRefObject<string>,
	setRooms: Dispatch<SetStateAction<Room[]>>,
	activeRoomId: string,
	displayedHarnessId: string | null,
	notifyBadge: boolean,
	notifyToast: boolean,
	notifyOs: boolean,
	replaceHarnessSessionId: (targetRoomId: string, harnessId: string, sessionId: string) => void,
	jumpToHarness: (roomId: string, harnessId: string) => void,
) {
	// L5c — in-app toasts. Ephemeral (no DB mirror) since they
	// represent "right now, look here" state that doesn't survive
	// a restart. Capped at TOAST_MAX_VISIBLE so a burst of
	// transitions doesn't cover the screen.
	const [toasts, setToasts] = useState<ToastEntry[]>([]);

	const dismissToast = (id: string) => {
		setToasts((prev) => prev.filter((t) => t.id !== id));
	};

	const jumpToToast = (toast: ToastEntry) => {
		jumpToHarness(toast.roomId, toast.harnessId);
		dismissToast(toast.id);
	};

	// #330: lets a caller outside this hook (the `create_room` agent
	// request handler, which has no harness-activity transition to key
	// off) push a toast of its own onto the same stack, capped the same
	// way every transition-driven toast already is.
	const pushToast = (entry: NewToast) => {
		setToasts((prev) => appendToast(prev, entry, Date.now()));
	};

	// L5e — notification toggles read inside the transition listener
	// (mounted once with empty deps); refs let preference toggles
	// take effect without re-subscribing.
	const notifyBadgeRef = useRef(notifyBadge);
	notifyBadgeRef.current = notifyBadge;
	const notifyToastRef = useRef(notifyToast);
	notifyToastRef.current = notifyToast;
	const notifyOsRef = useRef(notifyOs);
	notifyOsRef.current = notifyOs;
	// D2f — last api_error arrival per harness, for coalescing a retry
	// burst into one badge-worthy incident.
	const lastApiErrorAtRef = useRef<Map<string, number>>(new Map());
	// Per-harness time of the last badge bump, for the coalesce window
	// (#62/#64 — collapse a burst/chatter of transitions into one badge).
	const lastBadgeAtRef = useRef<Map<string, number>>(new Map());

	// L5b — window-focus state + OS-notification permission. The
	// notification logic below skips firing an OS banner when Skein
	// is the focused app, because the user is already looking at
	// the badge update in real time and an extra OS-level banner is
	// just noise. Refs (not state) since the transition callback
	// reads these synchronously and we don't want them to retrigger
	// the subscription effect on every focus change.
	const windowFocusedRef = useRef(true);
	const notificationPermissionRef = useRef<NotificationPermission>("unknown");
	useWindowFocusPermission(
		roomsRef,
		activeRoomIdRef,
		setRooms,
		windowFocusedRef,
		notificationPermissionRef,
	);
	useHarnessHookEvents(roomsRef, replaceHarnessSessionId);
	useTransitionNotifications(
		roomsRef,
		activeRoomIdRef,
		setRooms,
		setToasts,
		notifyBadgeRef,
		notifyToastRef,
		notifyOsRef,
		lastBadgeAtRef,
		windowFocusedRef,
		notificationPermissionRef,
	);
	useApiErrorToasts(
		roomsRef,
		activeRoomIdRef,
		setRooms,
		setToasts,
		notifyBadgeRef,
		notifyToastRef,
		windowFocusedRef,
		lastApiErrorAtRef,
	);
	useTransitionLog(roomsRef);

	// L5a — clear pending on view. Runs every time the (active room,
	// active harness of active room) tuple changes — covers tab
	// click, keyboard nav (Mod+1..9, Mod+Tab), command palette,
	// initial load. Only the harness that's now displayed gets
	// cleared; other harnesses in the same room keep their pending
	// counts so a multi-harness room only loses badges as the user
	// visits each tab.
	// biome-ignore lint/correctness/useExhaustiveDependencies: setRooms comes from useRoomsStore (#19) — a setState setter, stable across renders, but biome can't prove that through a destructured custom-hook return.
	useEffect(() => {
		// #328: the room-level `attention` mark clears independently of
		// `displayedHarnessId` below — a room becoming active is what
		// "visited" means for it, not which harness inside it happens to
		// be showing.
		setRooms((prev) => clearAttention(prev, activeRoomId));
		if (!activeRoomId || !displayedHarnessId) return;
		setRooms((prev) => clearHarnessPending(prev, activeRoomId, displayedHarnessId));
		// Landing on a harness means you're now looking at it, so drop any
		// lingering toast for it — regardless of how you got here (Mod+J/L,
		// palette, tab, Mod+1..9). Clicking a toast already dismisses it via
		// jumpToToast; this covers every other path. Return the same array
		// when nothing matches so we don't trigger a needless re-render.
		setToasts((prev) => dropToastsFor(prev, activeRoomId, displayedHarnessId));
	}, [activeRoomId, displayedHarnessId]);

	return {
		toasts,
		dismissToast,
		jumpToToast,
		pushToast,
	};
}
