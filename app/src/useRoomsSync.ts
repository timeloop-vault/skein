// React wiring for roomsSync.ts (#729): diff-based sync of `rooms` with the
// daemon's room store, the change-event subscription, and the exit-path flush.
// Parked while !loaded / loadFailed (#167).

import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { type Dispatch, type SetStateAction, useCallback, useEffect, useRef } from "react";
import { spanAsync } from "./perfTelemetry.ts";
import {
	ROOMS_CHANGED_EVENT,
	type RoomMutation,
	type RoomsApplyOutcome,
	type RoomsChanged,
} from "./roomMutations.ts";
import { registerRoomsFlusher } from "./roomsFlush.ts";
import { createRoomsSync, type RoomsSync } from "./roomsSync.ts";
import type { Room } from "./types.ts";

// One id per webview load; the daemon echoes it back as the event origin.
const CLIENT_ID = crypto.randomUUID();

export interface RoomsSyncDeps {
	rooms: Room[];
	loaded: boolean;
	loadFailed: string | null;
	setRooms: Dispatch<SetStateAction<Room[]>>;
}

export function useRoomsSync(d: RoomsSyncDeps) {
	const { rooms, loaded, loadFailed, setRooms } = d;

	const guardRef = useRef(false);
	guardRef.current = loaded && loadFailed === null;
	const syncRef = useRef<RoomsSync | null>(null);
	if (syncRef.current === null) {
		syncRef.current = createRoomsSync({
			clientId: CLIENT_ID,
			apply: (mutations: RoomMutation[]) =>
				spanAsync("rooms.save", () =>
					invoke<RoomsApplyOutcome>("rooms_apply", { clientId: CLIENT_ID, mutations }),
				),
			flushDaemon: () => invoke<void>("rooms_flush"),
			// Belt and braces (#167): never send from an unloaded / failed state.
			canSync: () => guardRef.current,
		});
	}

	/** Record the rooms exactly as the daemon returned them at load. */
	const setBaseline = useCallback((loadedRooms: Room[]) => {
		syncRef.current?.setBaseline(loadedRooms);
	}, []);

	/** Buffer foreign events from before the load until finishHydrate. */
	const beginHydrate = useCallback(() => {
		syncRef.current?.beginHydrate();
	}, []);

	/** Set the baseline and merge the buffered events into what hydrate installs. */
	const finishHydrate = useCallback(
		(rows: readonly Room[], local: readonly Room[]): Room[] =>
			syncRef.current?.finishHydrate(rows, local) ?? [...local],
		[],
	);

	useEffect(() => {
		if (!loaded || loadFailed !== null) return;
		syncRef.current?.update(rooms);
	}, [rooms, loaded, loadFailed]);

	// Changes from other clients / the daemon. The controller is consulted
	// outside any setRooms updater (StrictMode runs updaters twice); the
	// current rooms come from roomsRef. Subscribed once at mount so an event
	// emitted while `listen` is still resolving is not lost; the controller's
	// `remote` returns null until a baseline is set, which drops events that
	// predate the load.
	useEffect(() => {
		let cancelled = false;
		let off: (() => void) | null = null;
		listen<RoomsChanged>(ROOMS_CHANGED_EVENT, (event) => {
			const rebase = syncRef.current?.remote(event.payload);
			if (rebase) setRooms((prev) => rebase(prev));
		}).then(
			(fn) => {
				if (cancelled) fn();
				else off = fn;
			},
			(e) => console.error("rooms-changed listen failed", e),
		);
		return () => {
			cancelled = true;
			off?.();
		};
	}, [setRooms]);

	const flushRoomsSave = useCallback(async () => {
		await syncRef.current?.flush();
	}, []);

	// Exit paths (close / quit / updater) reach the flush through roomsFlush.
	// Best-effort too: the page is going away (reload, webview teardown).
	useEffect(() => {
		registerRoomsFlusher(flushRoomsSave);
		const onHide = () => {
			if (document.visibilityState === "hidden") void flushRoomsSave();
		};
		const onPageHide = () => void flushRoomsSave();
		document.addEventListener("visibilitychange", onHide);
		window.addEventListener("pagehide", onPageHide);
		return () => {
			registerRoomsFlusher(null);
			document.removeEventListener("visibilitychange", onHide);
			window.removeEventListener("pagehide", onPageHide);
		};
	}, [flushRoomsSave]);

	return { setBaseline, beginHydrate, finishHydrate, flushRoomsSave };
}
