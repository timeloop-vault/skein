// OS-notification click handling, extracted out of App.tsx (#19) —
// pure move, no behaviour change. Owns `jumpToTarget` (the decision +
// side-effect shared by every OS-notification click path),
// `drainPendingClick` (Windows-only pending-click poll), the live
// click listener (macOS's id-keyed map lookup, Windows's live event),
// and the post-hydrate pending-click drain. NOT here: `jumpToToast` —
// that already moved to useHarnessNotifications.ts. App.tsx calls this
// hook at the point this code used to be declared.

import { onNotificationClicked } from "@choochmeque/tauri-plugin-notifications-api";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import type { Dispatch, MutableRefObject, SetStateAction } from "react";
import { useCallback, useEffect } from "react";
import { osNotifyTargets } from "./notifications.tsx";
import { type ClickTarget, resolveClickTarget, shouldDrainOnClickEvent } from "./osNotifyClick.ts";
import { isWindows } from "./shortcuts.ts";
import type { Room } from "./types.ts";

export function useOsNotificationClicks(
	roomsRef: MutableRefObject<Room[]>,
	unarchiveRoomRef: MutableRefObject<(id: string) => Promise<void>>,
	setRooms: Dispatch<SetStateAction<Room[]>>,
	loaded: boolean,
	loadedRef: MutableRefObject<boolean>,
) {
	// #294: the decision + side-effect shared by every OS-notification
	// click path (macOS's id-keyed map lookup below, Windows's live
	// event, and Windows's post-hydrate pending-click drain further
	// down) — un-archive the room if needed and switch to the harness
	// that fired the toast. Stable identity ([] deps): only reads refs
	// and calls stable setState setters, so sharing it across effects
	// never forces a re-registration.
	// biome-ignore lint/correctness/useExhaustiveDependencies: roomsRef/setRooms/unarchiveRoomRef come from useRoomsStore (#19) — refs/a setState setter, stable across renders, but biome can't prove that through a destructured custom-hook return.
	const jumpToTarget = useCallback((target: ClickTarget) => {
		const decision = resolveClickTarget(roomsRef.current, target);
		if (!decision.found) return; // closed-and-deleted since the banner fired
		const { roomId, harnessId } = target;
		// #170: un-archive it if it was archived in the meantime, so
		// it's reachable. This used to strip `archived` inline and stop
		// there — no resume rewrite, no fresh opencode port — so the
		// remount respawned the stored fresh-form cmd and Claude died
		// with "Session ID is already in use". unarchiveRoom does both
		// halves (and focuses the room, archived or not).
		void unarchiveRoomRef.current(roomId);
		if (decision.hasHarness) {
			setRooms((prev) =>
				prev.map((r) => (r.id === roomId ? { ...r, activeHarnessId: harnessId } : r)),
			);
		}
	}, []);

	// #294: Windows only — the click target for a toast clicked while
	// Skein was closed lives Rust-side (crates/skein-winnotify) rather
	// than riding an event, so both the live listener below and the
	// post-hydrate effect further down read it through this one call.
	// Idempotent: once one caller has drained it, a second call just
	// gets `null` back.
	const drainPendingClick = useCallback(() => {
		invoke<ClickTarget | null>("os_notify_take_pending")
			.then((target) => {
				if (target) jumpToTarget(target);
			})
			.catch((err: unknown) => {
				const msg = err instanceof Error ? err.message : String(err);
				console.warn("[skein] os_notify_take_pending failed:", msg);
			});
	}, [jumpToTarget]);

	// #118: clicking the OS notification brings Skein to the front and
	// jumps to the harness that fired it. macOS's native plugin delivers
	// the click as `{ id }` and drops the `extra` payload, so we resolve
	// the jump target from the id-keyed `osNotifyTargets` map populated at
	// send time. #155/#294: Windows delivers clicks too, through Skein's
	// own `os_notify.rs` path — both while the toast banner is still on
	// screen AND later from Action Center (or a cold `-Embedding` launch
	// with Skein closed), via a registered unpackaged-app AUMID + COM
	// `CustomActivator` (`crates/skein-winnotify`). The Windows event now
	// carries no payload — it's just a poke — because a cold launch has
	// nowhere to have stashed one client-side; the real target comes from
	// `os_notify_take_pending` (`drainPendingClick` above). Rust itself
	// brings the window to the foreground on that path
	// (`skein_winnotify::bring_to_front`, the `AttachThreadInput` trick —
	// plain `SetForegroundWindow` loses to the foreground lock for a
	// click that didn't originate in this process); the calls below are
	// harmless belt-and-braces once that's already happened. Linux still
	// has no click path at all (notify-rust, #108). Wrapped so a
	// missing-permission rejection doesn't surface as an unhandled error.
	// biome-ignore lint/correctness/useExhaustiveDependencies: loadedRef comes from useRoomsStore (#19) — a ref, stable across renders, but biome can't prove that through a destructured custom-hook return.
	useEffect(() => {
		const focusWindow = () => {
			// Always surface the window — the user clicked a Skein banner.
			const win = getCurrentWindow();
			void win.show();
			void win.unminimize();
			void win.setFocus();
		};

		const focusAndJump = (id: number) => {
			focusWindow();
			const target = osNotifyTargets.get(id);
			osNotifyTargets.delete(id);
			if (!target) return;
			jumpToTarget(target);
		};

		const pluginPromise = onNotificationClicked((clicked) => focusAndJump(clicked.id)).catch(
			(err: unknown) => {
				const msg = err instanceof Error ? err.message : String(err);
				console.warn("[skein] onNotificationClicked unavailable:", msg);
				return null;
			},
		);
		// #294: before hydrate, ignore the poke — draining now would jump
		// into the still-empty boot-time rooms list; the post-hydrate
		// effect below drains once real rooms exist instead.
		const winPromise = isWindows
			? listen("skein://os-notification-clicked", () => {
					focusWindow();
					if (shouldDrainOnClickEvent(loadedRef.current)) drainPendingClick();
				}).catch((err: unknown) => {
					const msg = err instanceof Error ? err.message : String(err);
					console.warn("[skein] os-notification-clicked listener unavailable:", msg);
					return null;
				})
			: null;

		return () => {
			void pluginPromise.then((listener) => listener?.unregister());
			void winPromise?.then((unlisten) => unlisten?.());
		};
	}, [jumpToTarget, drainPendingClick]);

	// #294: the click arrived before Skein finished booting — Rust
	// queued it instead of firing a live event, so pick it up once
	// hydrate has produced a real rooms list to jump into. `loaded`
	// flips false→true at most once per boot and only on a *successful*
	// load (#167 — a failed load must not consume the click), so this
	// fires exactly once, right after that first success.
	useEffect(() => {
		if (!isWindows || !loaded) return;
		drainPendingClick();
	}, [loaded, drainPendingClick]);
}
