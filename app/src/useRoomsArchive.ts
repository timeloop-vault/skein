// Archive / unarchive / delete / retire half of useRoomsStore (#459
// split of #19). State and refs are owned by useRoomsStore and passed
// in, so hook order there is unchanged.

import { invoke } from "@tauri-apps/api/core";
import {
	type Dispatch,
	type MutableRefObject,
	type SetStateAction,
	useCallback,
	useRef,
} from "react";
import { type CloseRoomAttribution, decideCloseRoom, type RequestResult } from "./agentRequests.ts";
import { claudeVersionStore } from "./claudeVersionStore.ts";
import { confirmDialog } from "./confirmDialog.ts";
import { filesRegistry } from "./filesRegistry.ts";
import { unarchiveRoomTransform } from "./harnessCmd.ts";
import type { RenameTarget } from "./RoomStrip.tsx";
import {
	retireRooms as retireRoomsPure,
	unretireRooms as unretireRoomsPure,
} from "./reopenList.ts";
import { nextActiveAfterClose } from "./roomGroups.ts";
import { checkProbe, probeFolder } from "./roomsStoreProbe.ts";
import type { Room } from "./types.ts";

export interface RoomsArchiveDeps {
	activeRoomId: string;
	activeRooms: Room[];
	roomsRef: MutableRefObject<Room[]>;
	activeRoomIdRef: MutableRefObject<string>;
	mismatchedRef: MutableRefObject<Set<string>>;
	lastUsedByGroupRef: MutableRefObject<Map<string, string>>;
	setRooms: Dispatch<SetStateAction<Room[]>>;
	setActiveRoomId: Dispatch<SetStateAction<string>>;
	setOpencodePorts: Dispatch<SetStateAction<Map<string, number>>>;
	setRenaming: Dispatch<SetStateAction<RenameTarget | null>>;
	setMismatch: (id: string, on: boolean) => void;
	syncRepoMeta: (targets: Room[]) => void;
	checkRoomFolder: (room: Room) => Promise<void>;
}

export function useRoomsArchive(d: RoomsArchiveDeps) {
	const {
		activeRoomId,
		activeRooms,
		roomsRef,
		activeRoomIdRef,
		mismatchedRef,
		lastUsedByGroupRef,
		setRooms,
		setActiveRoomId,
		setOpencodePorts,
		setRenaming,
		setMismatch,
		syncRepoMeta,
		checkRoomFolder,
	} = d;

	const closeRoom = async (id: string) => {
		// Confirm before close — rooms can hold a lot of state and the
		// prototype has no undo (well, now there's the reopen modal —
		// but the user shouldn't have to discover that). #242: an
		// in-app dialog (confirmDialog.ts / ConfirmDialog.tsx), not
		// `plugin-dialog`'s native confirm() — it doesn't pick up
		// Skein's theme, and window.confirm is silently no-op'd in
		// WebKit without a host-side handler.
		// #185: name unsaved editor buffers — archived rooms come back,
		// unsaved buffer text doesn't.
		const target = roomsRef.current.find((r) => r.id === id);
		const dirty = filesRegistry.anyDirty(target?.harnesses.map((h) => h.id) ?? []);
		const msg =
			dirty.length > 0
				? `Close this room? Any running harnesses will be killed and ${dirty.length} unsaved file${dirty.length === 1 ? "" : "s"} (${dirty.join(", ")}) discarded.`
				: "Close this room? Any running harnesses will be killed.";
		let ok: boolean;
		try {
			ok = await confirmDialog({
				title: "Close room",
				message: msg,
				confirmLabel: "Close room",
				cancelLabel: "Cancel",
				kind: "warning",
			});
		} catch (err) {
			console.error("[skein] confirmDialog failed:", err);
			return;
		}
		if (!ok) return;
		// Chapter 6 phase 2: archive instead of delete. Tab strip filters
		// archived out; reopen modal lists them.
		setRooms((prev) => prev.map((r) => (r.id === id ? { ...r, archived: Date.now() } : r)));
		for (const h of target?.harnesses ?? []) claudeVersionStore.forget(h.id);
		if (id === activeRoomId) {
			const nextActive = nextActiveAfterClose(activeRooms, id, lastUsedByGroupRef.current);
			setActiveRoomId(nextActive?.id ?? "");
		}
		// #241: archiving is the only path that can remove a room out from
		// under an in-progress rename (the tab strip only ever renders
		// active rooms, and rename is only ever started on one) — clear
		// explicitly rather than relying on the input's own blur-on-unmount
		// commit, which would otherwise write the room's name right back
		// onto an archived room no tab shows any more.
		setRenaming((cur) => (cur?.roomId === id ? null : cur));
	};

	// #411: `close_room` agent verb — same archive `closeRoom` above
	// performs, but never prompts (there's no dialog for an agent call
	// to answer) and refuses outright instead of asking. The refusal
	// ladder (unknown room / already archived / unsaved Files buffers)
	// lives in the pure `decideCloseRoom` so it's unit-tested without
	// this hook; this is only the state mutation once decided.
	const closeRoomForAgent = (
		roomId: string,
		closedBy: CloseRoomAttribution,
	): RequestResult<{ roomId: string; archived: number }> => {
		const decision = decideCloseRoom(
			roomsRef.current,
			roomId,
			closedBy,
			(ids) => filesRegistry.anyDirty(ids),
			Date.now(),
		);
		if (!decision.ok) return decision;
		const { closedBy: stamped } = decision.value;
		for (const h of roomsRef.current.find((r) => r.id === roomId)?.harnesses ?? [])
			claudeVersionStore.forget(h.id);
		setRooms((prev) =>
			prev.map((r) => (r.id === roomId ? { ...r, archived: stamped.at, closedBy: stamped } : r)),
		);
		if (roomId === activeRoomIdRef.current) {
			const nextActive = nextActiveAfterClose(activeRooms, roomId, lastUsedByGroupRef.current);
			setActiveRoomId(nextActive?.id ?? "");
		}
		// Same reasoning as the user's close, above: archiving can remove
		// a room out from under an in-progress rename.
		setRenaming((cur) => (cur?.roomId === roomId ? null : cur));
		return { ok: true, value: { roomId, archived: stamped.at } };
	};

	// Fresh embedded-server ports for every opencode harness in a room
	// about to be (re)mounted. The ports from sqlite are dead — the run
	// that bound them released them on exit — and resumeCmd needs the
	// new ones to bake into the argv. Allocation failure is survivable:
	// that harness resumes without --port and its L2c-2 SSE adapter
	// simply doesn't attach.
	const allocateOpencodePorts = useCallback(
		async (room: Room | undefined) => {
			const portMap = new Map<string, number>();
			if (!room) return portMap;
			await Promise.all(
				room.harnesses
					.filter((h) => h.kind === "opencode" && h.cmd)
					.map(async (h) => {
						try {
							portMap.set(h.id, await invoke<number>("pick_free_port"));
						} catch (err) {
							console.warn(`[skein] pick_free_port failed for ${h.id} on unarchive`, err);
						}
					}),
			);
			if (portMap.size > 0) {
				setOpencodePorts((prev) => new Map([...prev, ...portMap]));
			}
			return portMap;
		},
		[setOpencodePorts],
	);

	// #153 / #170: un-archiving a room re-mounts it, which re-spawns
	// every harness, so their cmds must be in resume form *before* the
	// state change lands. A harness created this session still carries
	// its fresh-spawn cmd (`claude --session-id <uuid>`); re-running that
	// against an existing session makes Claude reject it with "Session ID
	// is already in use".
	//
	// This is the single un-archive path. #153 fixed the reopen modal,
	// #170 was the OS-notification click doing the same job with the
	// resume half missing — every future caller gets both halves by
	// construction.
	const unarchiveRoom = useCallback(
		async (id: string) => {
			const room = roomsRef.current.find((r) => r.id === id);
			// Already active: it's mounted and running, so rewriting its
			// cmds would change LiveTerminal's mountKey and kill a live
			// harness. Just focus it.
			if (!room?.archived) {
				setActiveRoomId(id);
				return;
			}
			const portMap = await allocateOpencodePorts(room);
			const transformed = unarchiveRoomTransform(room, portMap);
			// #418: decide identity BEFORE the remount so a room whose
			// folder now holds another repo opens on the card, resuming
			// nothing. React batches this with the setRooms below.
			if (room.repoIdentity && room.cwd) {
				const probe = await probeFolder(room.cwd);
				if (probe && checkProbe(room.repoIdentity, probe) === "mismatch") {
					mismatchedRef.current = new Set(mismatchedRef.current).add(id);
					setMismatch(id, true);
				}
			}
			setRooms((prev) => prev.map((r) => (r.id === id ? transformed : r)));
			setActiveRoomId(id);
			// #76 / #418: repoRoot + identity backfill, as hydrate does.
			syncRepoMeta([room]);
			// #164: an archived room's folder may have vanished while it
			// was closed — check on the way back in, same as hydrate does
			// for rooms that were already active.
			void checkRoomFolder(transformed);
		},
		[
			allocateOpencodePorts,
			syncRepoMeta,
			setMismatch,
			checkRoomFolder,
			roomsRef,
			mismatchedRef,
			setRooms,
			setActiveRoomId,
		],
	);
	// The OS-notification listener is []-keyed (re-registering it on
	// every render would leak native listeners), so it reaches the
	// current unarchiveRoom through a ref rather than closing over it.
	const unarchiveRoomRef = useRef(unarchiveRoom);
	unarchiveRoomRef.current = unarchiveRoom;

	// #89: permanently drop an archived room. `db_save_rooms` is a full
	// DELETE + re-insert of the current `rooms` array, so removing it
	// from state *is* the delete — the autosave effect mirrors it out.
	// (Orphaned `harness_actions`/`harness_events` rows for the room are
	// harmless activity-log leftovers; a later pass can vacuum them.)
	const deleteRoomForever = (id: string) => {
		setRooms((prev) => prev.filter((r) => r.id !== id));
	};

	// #89: undo a just-deleted room — re-insert it with its `archived`
	// flag intact, so it returns to the reopen list (not the tab strip).
	// The archivedRooms memo re-sorts it back into place.
	const restoreRoom = (room: Room) => {
		setRooms((prev) => (prev.some((r) => r.id === room.id) ? prev : [...prev, room]));
	};

	// #417: batch forms for Reopen's multi-select.
	const deleteRoomsForever = (ids: readonly string[]) => {
		const gone = new Set(ids);
		setRooms((prev) => prev.filter((r) => !gone.has(r.id)));
	};

	const restoreRooms = (restored: Room[]) => {
		setRooms((prev) => {
			const have = new Set(prev.map((r) => r.id));
			const add = restored.filter((r) => !have.has(r.id));
			return add.length === 0 ? prev : [...prev, ...add];
		});
	};

	const retireRooms = (ids: readonly string[]) => {
		const now = Date.now();
		setRooms((prev) => retireRoomsPure(prev, ids, now));
	};

	const unretireRooms = (ids: readonly string[]) => {
		setRooms((prev) => unretireRoomsPure(prev, ids));
	};

	return {
		closeRoom,
		closeRoomForAgent,
		allocateOpencodePorts,
		unarchiveRoom,
		unarchiveRoomRef,
		deleteRoomForever,
		restoreRoom,
		deleteRoomsForever,
		restoreRooms,
		retireRooms,
		unretireRooms,
	};
}
