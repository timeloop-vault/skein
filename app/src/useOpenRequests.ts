// Epic #255's frontend half: act on a folder opened from outside Skein
// (`skein .`, a `skein://open` link, a second launch with a path).
//
// Same delivery shape as useOsNotificationClicks.ts's Windows path
// (#294): Rust parks the request in a pending slot, raises the window
// and emits a payload-less poke; this hook takes the slot through
// `open_request_take` — on the poke once rooms have loaded, and once
// right after hydrate, which is how a path handed to a cold launch
// waits for the rooms it has to be matched against. Taking twice is
// harmless: the second take gets `null`.
//
// Rust resolves the path (canonicalized, matched against every stored
// room); `decideOpen` makes the one call only the frontend can, and
// this hook carries it out with the primitives every other entry point
// already uses — `unarchiveRoom` (reopen if archived, else just focus)
// and `openNewRoomAt`.

import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { MutableRefObject } from "react";
import { useCallback, useEffect, useRef } from "react";
import { confirmDialog } from "./confirmDialog.ts";
import { decideOpen, type OpenTarget } from "./openRequest.ts";
import type { Room } from "./types.ts";

export function useOpenRequests(
	roomsRef: MutableRefObject<Room[]>,
	activeRoomIdRef: MutableRefObject<string>,
	unarchiveRoomRef: MutableRefObject<(id: string) => Promise<void>>,
	openNewRoom: () => Promise<void>,
	openNewRoomAt: (folder: string) => void,
	loaded: boolean,
	loadedRef: MutableRefObject<boolean>,
) {
	// Both listeners below are []-keyed, so they reach the current
	// openers through refs rather than closing over one render's.
	const openNewRoomRef = useRef(openNewRoom);
	openNewRoomRef.current = openNewRoom;
	const openNewRoomAtRef = useRef(openNewRoomAt);
	openNewRoomAtRef.current = openNewRoomAt;

	// biome-ignore lint/correctness/useExhaustiveDependencies: roomsRef/activeRoomIdRef/unarchiveRoomRef come from useRoomsStore (#19) — refs, stable across renders, but biome can't prove that through a destructured custom-hook return.
	const drain = useCallback(() => {
		invoke<OpenTarget | null>("open_request_take")
			.then(async (target) => {
				if (!target) return;
				const action = decideOpen(target, roomsRef.current, activeRoomIdRef.current || null);
				switch (action.kind) {
					case "focus":
						await unarchiveRoomRef.current(action.roomId);
						return;
					case "newRoom":
						openNewRoomAtRef.current(action.folder);
						return;
					case "missing": {
						// The `skein` command checks the path itself, so this is
						// a hand-made link. Say so rather than raising the window
						// onto nothing, and offer the next best thing.
						const pick = await confirmDialog({
							title: "Can't open that folder",
							message: `${action.path} doesn't exist.`,
							confirmLabel: "New room…",
							cancelLabel: "Close",
						});
						if (pick) await openNewRoomRef.current();
						return;
					}
				}
			})
			.catch((err: unknown) => {
				const msg = err instanceof Error ? err.message : String(err);
				console.warn("[skein] open_request_take failed:", msg);
			});
	}, []);

	// Before hydrate, ignore the poke: matching against the still-empty
	// boot-time rooms list would open New room for a folder that has a
	// room. The post-hydrate drain below picks the request up instead.
	// biome-ignore lint/correctness/useExhaustiveDependencies: loadedRef comes from useRoomsStore (#19) — a ref, stable across renders, but biome can't prove that through a destructured custom-hook return.
	useEffect(() => {
		const unlisten = listen("skein://open-request", () => {
			if (loadedRef.current) drain();
		}).catch((err: unknown) => {
			const msg = err instanceof Error ? err.message : String(err);
			console.warn("[skein] open-request listener unavailable:", msg);
			return null;
		});
		return () => {
			void unlisten.then((fn) => fn?.());
		};
	}, [drain]);

	// `loaded` flips false→true at most once per boot, and only on a
	// successful load (#167), so this runs exactly once.
	useEffect(() => {
		if (loaded) drain();
	}, [loaded, drain]);
}
