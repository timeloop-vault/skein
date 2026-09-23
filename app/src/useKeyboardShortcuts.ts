// Window-level keyboard shortcuts, extracted out of App.tsx (#19) —
// pure move, no behaviour change. Uses isAppShortcut as the gate — that
// same predicate also makes LiveTerminal's xterm custom handler return
// false for these combos, so the byte never reaches the PTY.
// preventDefault stops the WebView's defaults (Mod+W close, Mod+= zoom,
// Mod+1..9 tab jump, etc). Mod = ⌘ on macOS, Ctrl elsewhere.
//
// Every input here is a ref (a per-render handler mirror App.tsx
// stages so this listener can stay bound across renders without
// re-listing every callback as a dep) or a stable setState setter /
// useCallback. App.tsx calls this hook at the point the effect used to
// be declared; the ref declarations + `.current =` mirrors themselves
// stay in App.tsx, since they close over values defined there.

import type { Dispatch, MutableRefObject, SetStateAction } from "react";
import { useEffect } from "react";
import type { StripSegment } from "./roomGroups.ts";
import { topLevelTarget } from "./roomGroups.ts";
import { matchShortcut } from "./shortcuts.ts";
import type { Room } from "./types.ts";
import { FONT_MAX, FONT_MIN } from "./useAppSettings.ts";

export function useKeyboardShortcuts(
	visibleOrderRef: MutableRefObject<Room[]>,
	stripSegmentsRef: MutableRefObject<StripSegment[]>,
	activeRoomIdRef: MutableRefObject<string>,
	activeRoomsRef: MutableRefObject<Room[]>,
	switchHarnessInRoomRef: MutableRefObject<(roomId: string, harnessId: string) => void>,
	addHarnessRef: MutableRefObject<(roomId: string) => void>,
	closeRoomRef: MutableRefObject<(id: string) => Promise<void>>,
	cycleAlertedRoomRef: MutableRefObject<(delta: number) => void>,
	cycleAlertedHarnessRef: MutableRefObject<(delta: number) => void>,
	toggleFilesRef: MutableRefObject<() => void>,
	toggleReviewRef: MutableRefObject<() => void>,
	lastUsedByGroupRef: MutableRefObject<Map<string, string>>,
	setActiveRoomId: Dispatch<SetStateAction<string>>,
	setShowPalette: Dispatch<SetStateAction<boolean>>,
	setShowSettings: Dispatch<SetStateAction<boolean>>,
	setFontSize: Dispatch<SetStateAction<number>>,
	openNewRoom: () => Promise<void>,
) {
	// biome-ignore lint/correctness/useExhaustiveDependencies: activeRoomIdRef/setActiveRoomId come from useRoomsStore (#19) — a ref/a setState setter, stable across renders, but biome can't prove that through a destructured custom-hook return.
	useEffect(() => {
		const cycleRoom = (delta: number) => {
			// #76: walk every room in strip order (allRoomOrder), stepping
			// into and out of groups, not the flat active-room array.
			const list = visibleOrderRef.current;
			if (list.length === 0) return;
			const active = activeRoomIdRef.current;
			const idx = list.findIndex((r) => r.id === active);
			if (idx === -1) {
				const first = list[0];
				if (first) setActiveRoomId(first.id);
				return;
			}
			const nextIdx = (idx + delta + list.length) % list.length;
			const next = list[nextIdx];
			if (next) setActiveRoomId(next.id);
		};

		const cycleHarness = (delta: number) => {
			const list = activeRoomsRef.current;
			const active = activeRoomIdRef.current;
			const room = list.find((r) => r.id === active);
			if (!room || room.harnesses.length === 0) return;
			const idx = room.harnesses.findIndex((h) => h.id === room.activeHarnessId);
			const baseIdx = idx === -1 ? 0 : idx;
			const nextIdx = (baseIdx + delta + room.harnesses.length) % room.harnesses.length;
			const next = room.harnesses[nextIdx];
			if (next) switchHarnessInRoomRef.current(room.id, next.id);
		};

		const onKey = (e: KeyboardEvent) => {
			// #185: the CodeMirror editor claims chords like Mod+Arrow
			// (line start/end) with preventDefault; a claimed key must
			// not ALSO drive app navigation. xterm never preventDefaults
			// app shortcuts (the isAppShortcut gate returns them to us),
			// so terminal-originated chords still arrive here unclaimed.
			if (e.defaultPrevented) return;
			const match = matchShortcut(e);
			if (!match) return;
			e.preventDefault();

			const active = activeRoomIdRef.current;
			switch (match.action) {
				case "newRoom":
					void openNewRoom();
					break;
				case "closeRoom":
					if (active) closeRoomRef.current(active);
					break;
				case "palette":
					setShowPalette(true);
					break;
				case "files":
					// #49 phase A: jump to (or create) the room's Files
					// harness, or bounce back to the last terminal.
					toggleFilesRef.current();
					break;
				case "review":
					// #212: flip the active room's right pane to Review, and
					// back again — the same there-and-back shape as Mod+E,
					// so the chord is a toggle rather than a one-way door.
					toggleReviewRef.current();
					break;
				case "settings":
					setShowSettings(true);
					break;
				case "addHarness":
					if (active) addHarnessRef.current(active);
					break;
				case "reloadWindow":
					// #121: recover from a wedged webview (and the #120 black
					// screen on older builds). Rust-side PTYs survive the
					// reload; boot re-hydrates and resumes.
					window.location.reload();
					break;
				case "nextRoom":
					cycleRoom(1);
					break;
				case "prevRoom":
					cycleRoom(-1);
					break;
				case "nextHarness":
					cycleHarness(1);
					break;
				case "prevHarness":
					cycleHarness(-1);
					break;
				case "nextAlertedRoom":
					cycleAlertedRoomRef.current(1); // #67
					break;
				case "prevAlertedRoom":
					cycleAlertedRoomRef.current(-1); // #67
					break;
				case "nextAlertedHarness":
					cycleAlertedHarnessRef.current(1); // #67
					break;
				case "prevAlertedHarness":
					cycleAlertedHarnessRef.current(-1); // #67
					break;
				case "fontInc":
					setFontSize((s) => Math.min(FONT_MAX, s + 1));
					break;
				case "fontDec":
					setFontSize((s) => Math.max(FONT_MIN, s - 1));
					break;
				case "jumpRoom": {
					// #76: Alt+1..9 indexes the strip's TOP-LEVEL segments — one
					// slot per repository, not per room — landing on the same
					// room a click on that tab would (`topLevelTarget`).
					const seg = stripSegmentsRef.current[match.roomIndex ?? 0];
					if (seg) {
						const target = topLevelTarget(seg, lastUsedByGroupRef.current, activeRoomsRef.current);
						if (target) setActiveRoomId(target.id);
					}
					break;
				}
			}
		};

		window.addEventListener("keydown", onKey);
		return () => window.removeEventListener("keydown", onKey);
		// `openNewRoom` is a stable useCallback([]) — listed to satisfy
		// exhaustive-deps, and it never causes a re-subscribe.
	}, [setFontSize, openNewRoom]);
}
