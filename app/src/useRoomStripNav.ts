// Room-strip navigation + New Room, extracted out of App.tsx (#19) —
// pure move, no behaviour change. Owns drag-reorder (`reorderRoom`,
// `reorderHarness`), the two-level strip's derived segments
// (`stripSegments`/`stripSegmentsRef`, `activeSegment`), the group
// "last used" memory (`lastUsedByGroupRef`) and top-level tab click
// resolution (`onSelectSegment`), and New Room opening + its persisted
// per-folder memory (`showNewRoom`, `newRoomMemory`, `newRoomSeed`,
// `openNewRoom`, `openNewRoomAt`, `openGroupPlaceholder`,
// `rememberRoomFolder`, `recentRoomFolders`).
//
// App.tsx calls this right after `useRoomsStore`, earlier than the
// code below used to sit: `reorderRoom`/`reorderHarness` feed
// `useTabDrag` and `setShowNewRoom` feeds `useHarnessCreation`, both
// of which run before this block's old position did. `activeRoomsRef`
// stays a parameter (declared in App.tsx, relocated up next to this
// call) rather than moving in, since `useKeyboardShortcuts` also
// needs it later and it has no other reason to live here.

import { invoke } from "@tauri-apps/api/core";
import type { Dispatch, MutableRefObject, SetStateAction } from "react";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { FolderInfoDto } from "./NewRoomDialog.tsx";
import {
	EMPTY_NEW_ROOM_MEMORY,
	type FolderDefaults,
	type NewRoomMemory,
	defaultsFor,
	recentFolders,
	rememberFolder,
	usePersistedState,
} from "./prefs.ts";
import {
	type StripSegment,
	buildStrip,
	resolveRowDrop,
	resolveTopDrop,
	roomIsGroupMain,
	segmentId,
	segmentOfRoom,
	topLevelTarget,
} from "./roomGroups.ts";
import type { Room } from "./types.ts";

export function useRoomStripNav(
	activeRooms: Room[],
	archivedRoomsRef: MutableRefObject<Room[]>,
	roomsRef: MutableRefObject<Room[]>,
	activeRoomId: string,
	activeRoomIdRef: MutableRefObject<string>,
	activeRoomsRef: MutableRefObject<Room[]>,
	setRooms: Dispatch<SetStateAction<Room[]>>,
	switchRoom: (id: string) => void,
	unarchiveRoomRef: MutableRefObject<(id: string) => Promise<void>>,
	lastUsedByGroupRef: MutableRefObject<Map<string, string>>,
) {
	const [showNewRoom, setShowNewRoom] = useState(false);

	// Issue #26 / #76: room drag-and-drop reorder, two-level-strip aware.
	// Decides, fresh against the CURRENT `rooms` (never a stale
	// drag-time snapshot) via `buildStrip`/`segmentOfRoom`, whether
	// `fromId` is a non-lead group MEMBER — reorders within its own
	// group only, via `resolveRowDrop` (the second row) — or a whole
	// top-level SEGMENT — a plain room or a group, dragged by its own
	// id or (a group tab has no room behind it) its `segmentId` — which
	// moves via `resolveTopDrop` (the top row), landing before/after the
	// target's whole segment even when the drop was actually over one
	// of that segment's second-row members. An invalid combination (a
	// member dropped outside its group, a segment dropped "inside" the
	// very group it's already in) is a no-op: both resolvers hand back
	// the same array reference, so `prev` passes straight through
	// unchanged.
	const reorderRoom = (fromId: string, targetId: string, side: "before" | "after") => {
		setRooms((prev) => {
			if (fromId === targetId) return prev;
			const segments = buildStrip(prev);
			const dragSeg = segmentOfRoom(segments, fromId);
			const dragIsMember =
				dragSeg?.kind === "group" && dragSeg.lead?.id !== fromId
					? dragSeg.members.some((m) => m.id === fromId)
					: false;
			if (dragIsMember) {
				// `resolveRowDrop` itself refuses a target outside the
				// drag's own group (or a lead on either end), so there's
				// nothing more to check here.
				return resolveRowDrop(prev, fromId, targetId, side);
			}
			// fromId is either a plain room, or (RoomStrip mints this as
			// the drag id for a group tab, which has no single room of
			// its own) already a segment id.
			const dragSegId = dragSeg ? segmentId(dragSeg) : fromId;
			const targetSeg = segmentOfRoom(segments, targetId);
			const targetSegId = targetSeg ? segmentId(targetSeg) : targetId;
			return resolveTopDrop(prev, dragSegId, targetSegId, side);
		});
	};

	const reorderHarness = (
		roomId: string,
		fromId: string,
		targetId: string,
		side: "before" | "after",
	) => {
		setRooms((prev) =>
			prev.map((r) => {
				if (r.id !== roomId) return r;
				const fromIdx = r.harnesses.findIndex((h) => h.id === fromId);
				const targetIdx = r.harnesses.findIndex((h) => h.id === targetId);
				if (fromIdx < 0 || targetIdx < 0 || fromId === targetId) return r;
				const adjustedTarget = side === "after" ? targetIdx + 1 : targetIdx;
				const insertIdx = fromIdx < adjustedTarget ? adjustedTarget - 1 : adjustedTarget;
				if (fromIdx === insertIdx) return r;
				const harnesses = [...r.harnesses];
				const [item] = harnesses.splice(fromIdx, 1);
				if (!item) return r;
				harnesses.splice(insertIdx, 0, item);
				return { ...r, harnesses };
			}),
		);
	};

	// #76: the strip's top-level segments — plain tabs unchanged,
	// worktree rooms grouped with their main room under one repository
	// tab. Keyboard nav (cycleRoom, jumpRoom) and the RoomStrip/GroupRow
	// render all walk this, not `activeRooms` directly.
	const stripSegments = useMemo(() => buildStrip(activeRooms), [activeRooms]);
	const stripSegmentsRef = useRef(stripSegments);
	stripSegmentsRef.current = stripSegments;

	// #76: which segment the active room is in — a group segment gets
	// the second row rendered under it; a plain segment (or no match,
	// e.g. during a brief state transition) gets none.
	const activeSegment = useMemo(
		() => segmentOfRoom(stripSegments, activeRoomId),
		[stripSegments, activeRoomId],
	);

	// #76: the room last used in each group, in memory only (not
	// persisted — see the design note). Updated whenever the active
	// room changes to one that's in a group; every existing way of
	// reaching a room (toasts, the urgent slot, OS notification clicks,
	// Alt+J/L, the palette, reopen, create) already goes through
	// `setActiveRoomId`, so this derives for free without touching any
	// of those call sites. Created in App.tsx (issue #334: `closeRoom`,
	// in useRoomsStore, needs to read it too, and that hook runs before
	// this one) and passed down as a parameter rather than created here.
	// biome-ignore lint/correctness/useExhaustiveDependencies: lastUsedByGroupRef is a ref passed in from App.tsx — stable across renders, but biome can't prove that through a parameter.
	useEffect(() => {
		if (activeSegment?.kind === "group") {
			lastUsedByGroupRef.current.set(activeSegment.key, activeRoomId);
		}
	}, [activeSegment, activeRoomId]);

	// #76: a click on a top-level tab (plain room or group) — resolved
	// to the room last used in that group, falling back to the lead or
	// first member (`topLevelTarget`); a plain tab always resolves to
	// its own room. Not memoized — `switchRoom` itself isn't, and
	// nothing downstream needs referential stability.
	const onSelectSegment = (seg: StripSegment) => {
		const target = topLevelTarget(seg, lastUsedByGroupRef.current, activeRoomsRef.current);
		if (target) switchRoom(target.id);
	};

	// ── New Room memory (#226, #231) ───────────────────────────────
	//
	// Opening the dialog is async because the seed has to be resolved
	// first: the active room's `cwd` may be a *worktree* path, and the
	// defaults are keyed on the folder it resolves to. Resolving here
	// rather than inside the dialog means the fields are already right
	// on the first paint — no blank frame, no jump.
	const [newRoomMemory, setNewRoomMemory] = usePersistedState<NewRoomMemory>(
		"newRoomMemory",
		EMPTY_NEW_ROOM_MEMORY,
	);
	const newRoomMemoryRef = useRef(newRoomMemory);
	newRoomMemoryRef.current = newRoomMemory;
	const [newRoomSeed, setNewRoomSeed] = useState<{
		cwd: string;
		defaults: FolderDefaults | undefined;
	}>({ cwd: "", defaults: undefined });

	// biome-ignore lint/correctness/useExhaustiveDependencies: roomsRef/activeRoomIdRef come from useRoomsStore (#19) — refs, stable across renders, but biome can't prove that through a destructured custom-hook return.
	const openNewRoom = useCallback(async () => {
		const memory = newRoomMemoryRef.current;
		// The room you are in is the strongest signal of where you mean
		// to work; the last folder used is the fallback when no room is
		// open. A git room contributes its repo root — every room in a
		// repo then shares one entry — and a plain folder contributes
		// itself (#231): "another room in the folder I am in" is the same
		// journey either way, and dropping the non-git case made those
		// rooms prefill nothing, or worse, an unrelated repo.
		const active = roomsRef.current.find((r) => r.id === activeRoomIdRef.current);
		let seed = memory.last ?? "";
		if (active?.cwd) {
			try {
				const info = await invoke<FolderInfoDto>("git_inspect_folder", {
					path: active.cwd,
				});
				if (info.exists) seed = info.isRepo ? info.root : active.cwd;
			} catch {
				// Resolution is a convenience, never a gate — fall back to
				// the remembered folder and let the dialog validate it.
			}
		}
		setNewRoomSeed({ cwd: seed, defaults: defaultsFor(memory, seed) });
		setShowNewRoom(true);
	}, []);

	// #76: open New Room already prefilled to a known folder, bypassing
	// `openNewRoom`'s active-room resolution — the caller (a group's
	// placeholder lead) already knows exactly which repo root it means.
	const openNewRoomAt = useCallback((folder: string) => {
		const memory = newRoomMemoryRef.current;
		setNewRoomSeed({ cwd: folder, defaults: defaultsFor(memory, folder) });
		setShowNewRoom(true);
	}, []);

	// #76: a group's placeholder-lead click. An archived room whose own
	// cwd IS the group's main worktree reopens (the same one-path
	// unarchive every other reopen surface uses); otherwise there's no
	// room to reopen at all, so open New Room prefilled to the repo root.
	// biome-ignore lint/correctness/useExhaustiveDependencies: archivedRoomsRef/unarchiveRoomRef come from useRoomsStore (#19) — refs, stable across renders, but biome can't prove that through a destructured custom-hook return.
	const openGroupPlaceholder = useCallback(
		(key: string, folder: string) => {
			const archivedMain = archivedRoomsRef.current.find((r) => roomIsGroupMain(r, key));
			if (archivedMain) {
				void unarchiveRoomRef.current(archivedMain.id);
				return;
			}
			openNewRoomAt(folder);
		},
		[openNewRoomAt],
	);

	const rememberRoomFolder = useCallback(
		(folder: string, defaults: Omit<FolderDefaults, "lastUsed">) => {
			setNewRoomMemory((prev) => rememberFolder(prev, folder, defaults));
		},
		[setNewRoomMemory],
	);
	const recentRoomFolders = useMemo(() => recentFolders(newRoomMemory), [newRoomMemory]);

	return {
		reorderRoom,
		reorderHarness,
		stripSegments,
		stripSegmentsRef,
		activeSegment,
		onSelectSegment,
		showNewRoom,
		setShowNewRoom,
		newRoomMemory,
		newRoomSeed,
		openNewRoom,
		openNewRoomAt,
		openGroupPlaceholder,
		rememberRoomFolder,
		recentRoomFolders,
	};
}
