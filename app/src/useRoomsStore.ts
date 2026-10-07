// Room lifecycle + persistence state, extracted out of App.tsx (#19) —
// pure move, no behaviour change. Owns the `rooms` array itself (hydrate
// from sqlite on boot, autosave on every change, #167 quarantine/backup
// surfacing), the active-room/harness selection, per-room folder-missing
// tracking (#164), opencode embedded-server port allocation, and the
// close/unarchive/delete/restore room lifecycle. App.tsx destructures the
// return value at the spot this state used to be declared.
//
// #459: the state and refs stay here; the behaviour is split by the
// question a reader has — useRoomsHydrate (boot load, autosave, repo
// metadata sync), useRoomsArchive (close / unarchive / delete / retire)
// and useRoomsRepoIdentity (missing-folder recovery, #418 identity
// actions) — over roomsStoreProbe (the non-React probes).

import {
	type Dispatch,
	type MutableRefObject,
	type SetStateAction,
	useCallback,
	useMemo,
	useRef,
	useState,
} from "react";
import type { RenameTarget } from "./RoomStrip.tsx";
import { inspectFolderMissing } from "./roomsStoreProbe.ts";
import type { Room } from "./types.ts";
import { useRoomsArchive } from "./useRoomsArchive.ts";
import { useRoomsHydrate } from "./useRoomsHydrate.ts";
import { useRoomsRepoIdentity } from "./useRoomsRepoIdentity.ts";

export function useRoomsStore(
	defaultShell: string[],
	setRenaming: Dispatch<SetStateAction<RenameTarget | null>>,
	lastUsedByGroupRef: MutableRefObject<Map<string, string>>,
) {
	const [rooms, setRooms] = useState<Room[]>([]);
	const roomsRef = useRef(rooms);
	roomsRef.current = rooms;
	const [activeRoomId, setActiveRoomId] = useState<string>("");
	const activeRoomIdRef = useRef(activeRoomId);
	activeRoomIdRef.current = activeRoomId;
	// Epic #50 L2c-2: per-opencode-harness embedded-server port.
	// Ephemeral — each spawn gets a fresh port via `pick_free_port`,
	// kept here so LiveTerminal can pass it to attachOpencodeEvents
	// without re-allocating. Not persisted: a port is meaningless
	// after the process that bound it dies.
	const [opencodePorts, setOpencodePorts] = useState<Map<string, number>>(new Map());

	// Chapter 6 phase 2: split rooms into active (rendered as tabs) and
	// archived (hidden, listed in the reopen modal). Tab strip, command
	// palette, the room useMemo below, and Mod+1..9 all key off active.
	const activeRooms = useMemo(() => rooms.filter((r) => !r.archived), [rooms]);
	const archivedRooms = useMemo(
		() =>
			rooms
				.filter((r) => r.archived)
				.slice()
				.sort((a, b) => (b.archived ?? 0) - (a.archived ?? 0)),
		[rooms],
	);
	// #76: mirrors `unarchiveRoomRef` — a placeholder-lead click
	// needs the current archived list without becoming a dep of the
	// callback that's stashed in a ref itself.
	const archivedRoomsRef = useRef(archivedRooms);
	archivedRoomsRef.current = archivedRooms;

	// Phase 3: hydrate rooms from sqlite on boot. Until that round-trips,
	// `loaded` stays false and the auto-save effect stays parked —
	// otherwise the empty initial state would clobber the DB before we read it.
	const [loaded, setLoaded] = useState(false);
	// #167: the boot load failed wholesale (sqlite open/read error).
	// While set, `loaded` stays false so the autosave stays parked —
	// the empty in-memory state must never overwrite the unread DB —
	// and the boot shell shows a retry surface instead of a blank pane.
	const [loadFailed, setLoadFailed] = useState<string | null>(null);
	// #167: rows the backend quarantined during an otherwise-good load.
	const [quarantinedCount, setQuarantinedCount] = useState(0);
	// #167: the live table was empty but skein.db.bak holds N rooms —
	// a vanished/recreated db must not masquerade as a fresh install.
	const [backupRoomCount, setBackupRoomCount] = useState<number | null>(null);
	// #164: room ids whose `cwd` does not currently exist on disk.
	// Runtime-only — never persisted, never a `Room` field (a folder
	// coming back doesn't change anything stored about the room) — so a
	// missing room renders MissingFolderCard instead of HarnessColumn
	// and nothing spawns into the wrong place. Filled during hydrate
	// before rooms mount, on unarchive, and whenever a room becomes
	// active.
	const [missingFolders, setMissingFolders] = useState<Set<string>>(new Set());
	// #164: guards `checkRoomFolder` against a stale in-flight result.
	// It runs both on unarchive and from the active-room effect
	// with no ordering guarantee between them, and a slow check can
	// resolve after a recovery action (recreateMissingWorktree /
	// pickMissingFolder) already cleared the flag — re-adding the room
	// to `missingFolders` and unmounting its freshly spawned
	// LiveTerminals. Every check and every recovery action bumps this
	// room's token first; a check applies its result only if its token
	// is still the latest when the await returns.
	// #418: room ids whose folder holds a DIFFERENT repo than the one the
	// room was made for (see repoIdentity.ts). Runtime-only like
	// `missingFolders`; filled before first mount in hydrate and on
	// unarchive, so a mismatched room never resumes a harness. The
	// ref mirrors it for callbacks that must not depend on it.
	const [mismatchedRooms, setMismatchedRooms] = useState<Set<string>>(new Set());
	const mismatchedRef = useRef(mismatchedRooms);
	mismatchedRef.current = mismatchedRooms;
	const setMismatch = useCallback((id: string, on: boolean) => {
		setMismatchedRooms((prev) => {
			if (prev.has(id) === on) return prev;
			const next = new Set(prev);
			if (on) next.add(id);
			else next.delete(id);
			return next;
		});
	}, []);
	// #418: rooms whose identity backfill already ran this session, so a
	// non-repo folder is not re-probed on every `rooms` change.
	const identityTriedRef = useRef(new Set<string>());
	const folderCheckTokenRef = useRef(new Map<string, number>());
	// #167: once any hydrate has succeeded, a late rejection from a
	// concurrent sibling call (dev StrictMode double-mount) must not
	// set loadFailed — that would park the autosave for the whole
	// session with no visible surface (the retry card only renders
	// pre-load).
	const hydratedOnceRef = useRef(false);
	// #294: mirrors `loaded`, set at the exact same call site — unlike
	// `hydratedOnceRef` (flipped earlier, before the resume/port-alloc
	// await chain), this is only true once `rooms` actually holds
	// the hydrated set. A live click-listener poke that lands in that
	// gap must not drain against the still-empty `roomsRef`.
	const loadedRef = useRef(false);
	// #164: re-check a single room and flip its membership in
	// `missingFolders` if the answer changed. Used on unarchive and
	// whenever a room becomes active — hydrate's own initial pass
	// fills the whole set at once, before any room mounts.
	const checkRoomFolder = useCallback(async (room: Room) => {
		const token = (folderCheckTokenRef.current.get(room.id) ?? 0) + 1;
		folderCheckTokenRef.current.set(room.id, token);
		if (!room.cwd) {
			setMissingFolders((prev) => {
				if (!prev.has(room.id)) return prev;
				const next = new Set(prev);
				next.delete(room.id);
				return next;
			});
			return;
		}
		const missing = await inspectFolderMissing(room.cwd, room.id);
		// A newer check or a recovery action already ran for this room
		// while this one was in flight — its answer is stale, drop it.
		if (folderCheckTokenRef.current.get(room.id) !== token) return;
		setMissingFolders((prev) => {
			if (prev.has(room.id) === missing) return prev;
			const next = new Set(prev);
			if (missing) next.add(room.id);
			else next.delete(room.id);
			return next;
		});
	}, []);
	const { syncRepoMeta, hydrateRooms } = useRoomsHydrate({
		rooms,
		loaded,
		loadFailed,
		setRooms,
		setActiveRoomId,
		setOpencodePorts,
		setLoaded,
		setLoadFailed,
		setQuarantinedCount,
		setBackupRoomCount,
		setMissingFolders,
		setMismatchedRooms,
		mismatchedRef,
		identityTriedRef,
		hydratedOnceRef,
		loadedRef,
	});

	const room = useMemo(() => rooms.find((r) => r.id === activeRoomId), [rooms, activeRoomId]);
	const activeHarness = room?.harnesses.find((h) => h.id === room.activeHarnessId);

	// #560: bumps on every user room selection, even of the already-active
	// room, so the Control Center can close on a re-select (an id-change effect
	// can't see that).
	const [roomSelectSeq, setRoomSelectSeq] = useState(0);
	const switchRoom = (id: string) => {
		setActiveRoomId(id);
		setRoomSelectSeq((n) => n + 1);
		// Pending-notification clearing for the now-displayed harness
		// is handled by a useEffect elsewhere — it covers every path
		// that changes the (active room, active harness) tuple, not
		// just tab clicks (Mod+1..9, Mod+Tab, palette, initial load).
	};

	const {
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
	} = useRoomsArchive({
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
	});

	const { recreateMissingWorktree, pickMissingFolder, confirmSameRepo, retireMismatchedRoom } =
		useRoomsRepoIdentity({
			defaultShell,
			activeRooms,
			roomsRef,
			activeRoomIdRef,
			mismatchedRef,
			identityTriedRef,
			folderCheckTokenRef,
			lastUsedByGroupRef,
			setRooms,
			setActiveRoomId,
			setRenaming,
			setMissingFolders,
			setMismatch,
			syncRepoMeta,
			allocateOpencodePorts,
		});

	return {
		retireRooms,
		unretireRooms,
		deleteRoomsForever,
		restoreRooms,
		rooms,
		setRooms,
		roomsRef,
		activeRoomId,
		setActiveRoomId,
		activeRoomIdRef,
		opencodePorts,
		setOpencodePorts,
		loaded,
		loadedRef,
		loadFailed,
		setLoadFailed,
		quarantinedCount,
		setQuarantinedCount,
		backupRoomCount,
		setBackupRoomCount,
		missingFolders,
		mismatchedRooms,
		confirmSameRepo,
		retireMismatchedRoom,
		checkRoomFolder,
		hydrateRooms,
		activeRooms,
		archivedRooms,
		archivedRoomsRef,
		room,
		activeHarness,
		allocateOpencodePorts,
		unarchiveRoom,
		unarchiveRoomRef,
		recreateMissingWorktree,
		pickMissingFolder,
		deleteRoomForever,
		restoreRoom,
		closeRoom,
		closeRoomForAgent,
		switchRoom,
		roomSelectSeq,
	};
}
