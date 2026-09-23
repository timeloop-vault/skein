// The command-palette item array, extracted out of App.tsx (#19) —
// pure move, no behaviour change. Built fresh every render from
// current state — cheap at prototype scale (a few dozen rows). App.tsx
// still calls this as a plain (non-memoized) function each render,
// same as before the move: the cost of one filter+map per Ctrl+K open
// is invisible, and memoizing would mean tracking every callback as a
// dep.

import type { Dispatch, MutableRefObject, SetStateAction } from "react";
import type { PaletteItem } from "./CommandPalette.tsx";
import { HARNESS_KINDS } from "./data.tsx";
import { hints } from "./shortcuts.ts";
import type { Room, Theme } from "./types.ts";

export interface BuildPaletteItemsParams {
	activeRooms: Room[];
	archivedRooms: Room[];
	room: Room | undefined;
	activeRoomId: string;
	theme: Theme;
	setActiveRoomId: Dispatch<SetStateAction<string>>;
	setRooms: Dispatch<SetStateAction<Room[]>>;
	setTheme: Dispatch<SetStateAction<Theme>>;
	setShowReopen: Dispatch<SetStateAction<boolean>>;
	openNewRoom: () => Promise<void>;
	toggleFilesRef: MutableRefObject<() => void>;
	toggleReviewRef: MutableRefObject<() => void>;
	addHarness: (roomId: string) => void;
	startRenameRoom: (roomId: string, host?: "group" | "tab") => void;
	closeRoom: (id: string) => Promise<void>;
	cycleAlertedRoom: (delta: number) => void;
	cycleAlertedHarness: (delta: number) => void;
}

export function buildPaletteItems(params: BuildPaletteItemsParams): PaletteItem[] {
	const {
		activeRooms,
		archivedRooms,
		room,
		activeRoomId,
		theme,
		setActiveRoomId,
		setRooms,
		setTheme,
		setShowReopen,
		openNewRoom,
		toggleFilesRef,
		toggleReviewRef,
		addHarness,
		startRenameRoom,
		closeRoom,
		cycleAlertedRoom,
		cycleAlertedHarness,
	} = params;

	const paletteItems: PaletteItem[] = [];
	for (const r of activeRooms) {
		paletteItems.push({
			id: `room:${r.id}`,
			label: `${r.name}`,
			hint: `room · ${r.branch}`,
			invoke: () => setActiveRoomId(r.id),
		});
	}
	for (const r of activeRooms) {
		for (const h of r.harnesses) {
			paletteItems.push({
				id: `harness:${h.id}`,
				label: `${HARNESS_KINDS[h.kind].name} · ${h.name}`,
				hint: `harness in ${r.name}`,
				invoke: () => {
					setActiveRoomId(r.id);
					setRooms((prev) =>
						prev.map((p) => (p.id === r.id ? { ...p, activeHarnessId: h.id } : p)),
					);
				},
			});
		}
	}
	paletteItems.push({
		id: "cmd:new-room",
		label: "New room",
		hint: hints.newRoom,
		invoke: () => void openNewRoom(),
	});
	if (room?.cwd) {
		paletteItems.push({
			id: "cmd:browse-files",
			label: "Files harness",
			hint: hints.files,
			invoke: () => toggleFilesRef.current(),
		});
		paletteItems.push({
			id: "cmd:review",
			label: "Review this branch",
			hint: hints.review,
			invoke: () => toggleReviewRef.current(),
		});
	}
	if (archivedRooms.length > 0) {
		paletteItems.push({
			id: "cmd:reopen-room",
			label: `Reopen room… (${archivedRooms.length})`,
			invoke: () => setShowReopen(true),
		});
	}
	if (activeRoomId) {
		paletteItems.push({
			id: "cmd:add-harness",
			label: "Add harness to active room",
			hint: hints.addHarness,
			invoke: () => addHarness(activeRoomId),
		});
		// #241: no shortcut (out of scope) — palette-only, like reopen-room.
		// Acts on the active room's own tab (`"tab"` host) — if that room is
		// a group's open main room, it's the second-row lead tab, never the
		// top-row GroupTab (see RenameTarget in RoomStrip.tsx).
		paletteItems.push({
			id: "cmd:rename-room",
			label: "Rename room",
			invoke: () => startRenameRoom(activeRoomId, "tab"),
		});
		paletteItems.push({
			id: "cmd:close-room",
			label: "Close active room",
			hint: hints.closeRoom,
			invoke: () => closeRoom(activeRoomId),
		});
	}
	paletteItems.push({
		id: "cmd:toggle-theme",
		label: `Toggle theme (currently ${theme})`,
		invoke: () => setTheme(theme === "dark" ? "light" : "dark"),
	});
	paletteItems.push({
		id: "cmd:reload-window",
		label: "Reload window",
		hint: hints.reload,
		invoke: () => window.location.reload(),
	});
	// #67: inbox navigation — only surfaced when something is actually
	// pending, so Cmd+K stays uncluttered otherwise.
	const otherAlertedRooms = activeRooms.filter(
		(r) =>
			r.id !== activeRoomId &&
			r.harnesses.reduce((a, h) => a + (h.pendingNotifications ?? 0), 0) > 0,
	);
	if (otherAlertedRooms.length > 0) {
		paletteItems.push({
			id: "cmd:next-alerted-room",
			label: `Jump to next alerted room (${otherAlertedRooms.length})`,
			hint: hints.nextAlertedRoom,
			invoke: () => cycleAlertedRoom(1),
		});
		paletteItems.push({
			id: "cmd:prev-alerted-room",
			label: `Jump to previous alerted room (${otherAlertedRooms.length})`,
			hint: hints.prevAlertedRoom,
			invoke: () => cycleAlertedRoom(-1),
		});
	}
	const alertedHarnessCount = activeRooms.reduce(
		(a, r) => a + r.harnesses.filter((h) => (h.pendingNotifications ?? 0) > 0).length,
		0,
	);
	if (alertedHarnessCount > 0) {
		paletteItems.push({
			id: "cmd:next-alerted-harness",
			label: `Jump to next alerted harness (${alertedHarnessCount})`,
			hint: hints.nextAlertedHarness,
			invoke: () => cycleAlertedHarness(1),
		});
		paletteItems.push({
			id: "cmd:prev-alerted-harness",
			label: `Jump to previous alerted harness (${alertedHarnessCount})`,
			hint: hints.prevAlertedHarness,
			invoke: () => cycleAlertedHarness(-1),
		});
	}

	return paletteItems;
}
