// D2f (#80) — graduated error treatment, steps 2+3 (handover §6)
// (#459 split of useHarnessNotifications; no behaviour change).
// api_error rows don't flow through harnessActivity (they're
// harness_actions rows), so this dedicated listener feeds the
// existing notification surfaces: an error-variant toast when the
// error lands in a room the user isn't looking at, and a
// pendingNotifications bump so the tab badge + status-bar urgent
// segment persist until the room gets attention (the stream carries
// no "resolved" signal — attention is the only clearing semantic).
// A retry burst is one incident (real data: 4 rows in 11 s) — the
// badge bumps once per window, and the toast updates in place while
// it's still showing rather than stacking.

import { listen } from "@tauri-apps/api/event";
import type { Dispatch, MutableRefObject, SetStateAction } from "react";
import { useEffect } from "react";
import { newToastId, setHarnessPending } from "./harnessNotifyLogic.ts";
import {
	ACTION_EVENT,
	apiErrorToastText,
	type HarnessAction,
	parsePayload,
} from "./liveContext/index.ts";
import { API_ERROR_INCIDENT_MS } from "./notifications.tsx";
import { appendToast, coalesceToast, type NewToast, type ToastEntry } from "./toastStack.ts";
import type { Room } from "./types.ts";

export function useApiErrorToasts(
	roomsRef: MutableRefObject<Room[]>,
	activeRoomIdRef: MutableRefObject<string>,
	setRooms: Dispatch<SetStateAction<Room[]>>,
	setToasts: Dispatch<SetStateAction<ToastEntry[]>>,
	notifyBadgeRef: MutableRefObject<boolean>,
	notifyToastRef: MutableRefObject<boolean>,
	windowFocusedRef: MutableRefObject<boolean>,
	lastApiErrorAtRef: MutableRefObject<Map<string, number>>,
) {
	// biome-ignore lint/correctness/useExhaustiveDependencies: roomsRef/activeRoomIdRef/setRooms come from useRoomsStore (#19) — refs/a setState setter, stable across renders, but biome can't prove that through a destructured custom-hook return.
	useEffect(() => {
		const unlistenPromise = listen<HarnessAction>(ACTION_EVENT, (event) => {
			const a = event.payload;
			if (a.kind !== "api_error") return;
			// §6: the inline ApiErrorRow covers the active room; the toast
			// exists for errors the user can't currently see.
			if (a.roomId === activeRoomIdRef.current) return;
			const owningRoom = roomsRef.current.find((r) => r.id === a.roomId);
			const harness = owningRoom?.harnesses.find((h) => h.id === a.harnessId);
			if (!owningRoom || !harness) return;
			const now = Date.now();
			const last = lastApiErrorAtRef.current.get(a.harnessId) ?? 0;
			const newIncident = now - last > API_ERROR_INCIDENT_MS;
			lastApiErrorAtRef.current.set(a.harnessId, now);
			if (notifyBadgeRef.current && newIncident) {
				setRooms((prev) => setHarnessPending(prev, a.harnessId));
			}
			if (!notifyToastRef.current || !windowFocusedRef.current) return;
			const detail = apiErrorToastText(parsePayload(a.payload));
			setToasts((prev) => {
				const i = prev.findIndex((t) => t.state === "error" && t.harnessId === a.harnessId);
				if (i !== -1) {
					// #180: coalesce onto the live toast (same id, same
					// `expiresAt`) with fresh detail, so a fast burst is one
					// toast, not a stack — but the update does NOT extend the
					// toast's life. It lapses TOAST_DISMISS_MS after the
					// incident first surfaced, even mid-burst: retries that
					// outpace that window (529 backoff spaces them out —
					// ~0.5/1/2/4s and growing) let the toast lapse between
					// rows, and the next retry re-surfaces a fresh one. The
					// incident keeps re-announcing itself, which is fine —
					// the badge (one bump per incident) is the persistent
					// signal, not this toast. Extending the expiry here would
					// pin an error toast for the whole of a retry storm,
					// which is the bug #180 fixes.
					return coalesceToast(prev, i, { detail });
				}
				const entry: NewToast = {
					id: newToastId(),
					roomId: owningRoom.id,
					harnessId: a.harnessId,
					kind: harness.kind,
					roomName: owningRoom.name,
					harnessName: harness.name,
					state: "error",
					detail,
				};
				return appendToast(prev, entry, Date.now());
			});
		});
		return () => {
			void unlistenPromise.then((un) => un());
		};
	}, []);
}
