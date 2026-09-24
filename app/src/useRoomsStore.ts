// Room lifecycle + persistence state, extracted out of App.tsx (#19) —
// pure move, no behaviour change. Owns the `rooms` array itself (hydrate
// from sqlite on boot, autosave on every change, #167 quarantine/backup
// surfacing), the active-room/harness selection, per-room folder-missing
// tracking (#164), opencode embedded-server port allocation, and the
// close/unarchive/delete/restore room lifecycle. App.tsx destructures the
// return value at the spot this state used to be declared.

import { invoke } from "@tauri-apps/api/core";
import { confirm } from "@tauri-apps/plugin-dialog";
import {
	type Dispatch,
	type MutableRefObject,
	type SetStateAction,
	useCallback,
	useEffect,
	useMemo,
	useRef,
	useState,
} from "react";
import type { FolderInfoDto } from "./NewRoomDialog.tsx";
import type { RenameTarget } from "./RoomStrip.tsx";
import { filesRegistry } from "./filesRegistry.ts";
import { unarchiveRoomTransform, withResumeCmds } from "./harnessCmd.ts";
import { repointRoom } from "./missingFolder.ts";
import { nextActiveAfterClose } from "./roomGroups.ts";
import type { Harness, Room } from "./types.ts";

/** Wire shape of `db_load_rooms` (#167): the rooms that parsed plus
 *  any rows the backend quarantined instead of failing the load.
 *  `backupRooms` arrives only when the live table was empty but the
 *  skein.db.bak snapshot still holds rooms. */
interface DbLoadOutcome {
	rooms: Room[];
	skipped: { id: string; error: string }[];
	backupRooms?: number;
}

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
	// #76: mirrors `unarchiveRoomRef` below — a placeholder-lead click
	// needs the current archived list without becoming a dep of the
	// callback that's stashed in a ref itself.
	const archivedRoomsRef = useRef(archivedRooms);
	archivedRoomsRef.current = archivedRooms;

	// Phase 3: hydrate rooms from sqlite on boot. Until that round-trips,
	// `loaded` stays false and the auto-save effect below stays parked —
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
	// It runs both on unarchive and from the active-room effect below
	// with no ordering guarantee between them, and a slow check can
	// resolve after a recovery action (recreateMissingWorktree /
	// pickMissingFolder) already cleared the flag — re-adding the room
	// to `missingFolders` and unmounting its freshly spawned
	// LiveTerminals. Every check and every recovery action bumps this
	// room's token first; a check applies its result only if its token
	// is still the latest when the await returns.
	const folderCheckTokenRef = useRef(new Map<string, number>());
	// #167: once any hydrate has succeeded, a late rejection from a
	// concurrent sibling call (dev StrictMode double-mount) must not
	// set loadFailed — that would park the autosave for the whole
	// session with no visible surface (the retry card only renders
	// pre-load).
	const hydratedOnceRef = useRef(false);
	// #294: mirrors `loaded`, set at the exact same call site — unlike
	// `hydratedOnceRef` (flipped earlier, before the resume/port-alloc
	// await chain below), this is only true once `rooms` actually holds
	// the hydrated set. A live click-listener poke that lands in that
	// gap must not drain against the still-empty `roomsRef`.
	const loadedRef = useRef(false);
	// #164: does `cwd` exist right now? A failed invoke is treated as
	// "unknown" rather than missing — the check is conservative in the
	// same direction as the #153 sessionId-existence probes above: a
	// transient rusqlite/fs hiccup must not park a perfectly good room
	// behind a false "folder missing" card.
	const inspectFolderMissing = useCallback(
		async (cwd: string, roomId: string): Promise<boolean> => {
			try {
				const info = await invoke<FolderInfoDto>("git_inspect_folder", { path: cwd });
				return !info.exists;
			} catch (err) {
				console.warn(`[skein] git_inspect_folder folder-check failed for room ${roomId}:`, err);
				return false;
			}
		},
		[],
	);
	// #164: re-check a single room and flip its membership in
	// `missingFolders` if the answer changed. Used on unarchive and
	// whenever a room becomes active — hydrate's own initial pass
	// below fills the whole set at once, before any room mounts.
	const checkRoomFolder = useCallback(
		async (room: Room) => {
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
		},
		[inspectFolderMissing],
	);
	// #76: rooms persisted before `repoRoot` existed have no group key.
	// Resolve it in the background, one `git_inspect_folder` per room in
	// parallel, and patch it in as each settles — never awaited by a
	// caller, so a slow or failing repo can't delay or break hydrate/
	// reopen. Never clears an existing `repoRoot`.
	const backfillRepoRoots = useCallback((targets: Room[]) => {
		const pending = targets.filter((r) => !r.repoRoot && r.cwd);
		if (pending.length === 0) return;
		void Promise.all(
			pending.map(async (r) => {
				try {
					const info = await invoke<FolderInfoDto>("git_inspect_folder", { path: r.cwd });
					if (!info.exists || !info.isRepo) return;
					setRooms((prev) =>
						prev.map((x) => (x.id === r.id && !x.repoRoot ? { ...x, repoRoot: info.root } : x)),
					);
				} catch (err) {
					console.warn(`[skein] git_inspect_folder backfill failed for room ${r.id}:`, err);
				}
			}),
		);
	}, []);
	const hydrateRooms = useCallback(() => {
		// Chapter 5 phase 4: drop any stored sessionId that no longer
		// exists on disk before resumeCmd uses it. claude --resume <id>
		// or opencode --session <id> against a deleted conversation
		// would either error or attach to nothing useful; falling back
		// to picker / --continue is the safer default.
		//
		// Errors from the existence checks are conservative: keep the
		// id (return true). The follow-up resume might fail noisily,
		// but at least we don't drop a legitimate id over a transient
		// rusqlite or fs hiccup.
		const stillExists = async (h: Harness): Promise<boolean> => {
			if (!h.sessionId) return true;
			try {
				if (h.kind === "claude") {
					return await invoke<boolean>("claude_session_exists", { id: h.sessionId });
				}
				if (h.kind === "opencode") {
					return await invoke<boolean>("opencode_session_exists", { id: h.sessionId });
				}
			} catch (err) {
				console.warn(`[skein] ${h.kind} session_exists failed for ${h.sessionId}:`, err);
				return true;
			}
			return true;
		};

		invoke<DbLoadOutcome>("db_load_rooms")
			.then(async ({ rooms: rows, skipped, backupRooms }) => {
				hydratedOnceRef.current = true;
				// Clear any failure from a concurrent sibling call —
				// success wins, and a stale loadFailed would silently
				// park the autosave (#167 review).
				setLoadFailed(null);
				if (skipped.length > 0) {
					console.warn(
						`[skein] ${skipped.length} room row(s) failed to parse and were quarantined:`,
						skipped,
					);
					setQuarantinedCount(skipped.length);
				}
				if (rows.length === 0 && backupRooms !== undefined && backupRooms > 0) {
					console.warn(`[skein] rooms table is empty but skein.db.bak holds ${backupRooms}`);
					setBackupRoomCount(backupRooms);
				}
				if (rows.length > 0) {
					const verified = await Promise.all(
						rows.map(async (s) => ({
							...s,
							harnesses: await Promise.all(
								s.harnesses.map(async (h) => {
									if (await stillExists(h)) return h;
									console.info(
										`[skein] dropping stale ${h.kind} sessionId ${h.sessionId} on harness ${h.id}`,
									);
									const { sessionId, ...rest } = h;
									return rest;
								}),
							),
						})),
					);
					// Epic #50 L2c-2: pre-allocate fresh embedded-server
					// ports for every resumed opencode harness. The
					// previous Skein run released its ports on exit;
					// resumeCmd needs the new ones to bake into the
					// argv. Awaiting in parallel keeps boot fast even
					// with many opencode rooms.
					const opencodeHarnesses = verified.flatMap((r) =>
						r.harnesses.filter((h) => h.kind === "opencode" && h.cmd),
					);
					const allocations = await Promise.all(
						opencodeHarnesses.map(async (h) => {
							try {
								const port = await invoke<number>("pick_free_port");
								return [h.id, port] as const;
							} catch (err) {
								console.warn(
									`[skein] pick_free_port failed for ${h.id}; L2c-2 adapter disabled for this harness`,
									err,
								);
								return null;
							}
						}),
					);
					const portMap = new Map<string, number>(
						allocations.filter((a): a is readonly [string, number] => a !== null),
					);
					setOpencodePorts(portMap);
					// Rewrite each harness's cmd to its resume form before
					// mounting, so the PTY spawn re-attaches to the prior
					// conversation instead of starting fresh.
					const withResume = verified.map((r) => withResumeCmds(r, portMap));
					// #164: check every active room's folder in parallel and
					// have the full missing-set ready BEFORE rooms mount, so
					// a vanished folder never gets even one spawn attempt.
					// Archived rooms aren't mounted, so they're not checked
					// here — unarchive (below) checks on the way back in.
					const initialMissing = new Set<string>();
					await Promise.all(
						withResume.map(async (r) => {
							if (r.archived || !r.cwd) return;
							if (await inspectFolderMissing(r.cwd, r.id)) initialMissing.add(r.id);
						}),
					);
					setMissingFolders(initialMissing);
					setRooms(withResume);
					backfillRepoRoots(withResume);
					// Pick the first *active* room; archived ones aren't
					// supposed to be the boot-time selection.
					const first = withResume.find((r) => !r.archived);
					if (first) setActiveRoomId(first.id);
				}
				loadedRef.current = true;
				setLoaded(true);
			})
			.catch((err: unknown) => {
				const msg = err instanceof Error ? err.message : String(err);
				console.error("[skein] db_load_rooms failed:", msg);
				if (hydratedOnceRef.current) return;
				// #167: deliberately NOT setLoaded(true) here. Flipping
				// `loaded` with the initial [] still in state arms the
				// autosave, whose next write is DELETE FROM sessions —
				// the boot-wipe chain this issue exists to break.
				setLoadFailed(msg);
			});
	}, [backfillRepoRoots, inspectFolderMissing]);

	useEffect(() => {
		hydrateRooms();
	}, [hydrateRooms]);

	// Phase 3: any time `rooms` changes after the initial load, mirror
	// the new state to sqlite. Wipe-and-insert is fine at prototype scale.
	useEffect(() => {
		if (!loaded || loadFailed !== null) return;
		void invoke("db_save_rooms", { rooms }).catch((err: unknown) => {
			const msg = err instanceof Error ? err.message : String(err);
			console.error("[skein] db_save_rooms failed:", msg);
		});
	}, [rooms, loaded, loadFailed]);

	const room = useMemo(() => rooms.find((r) => r.id === activeRoomId), [rooms, activeRoomId]);
	const activeHarness = room?.harnesses.find((h) => h.id === room.activeHarnessId);

	const switchRoom = (id: string) => {
		setActiveRoomId(id);
		// Pending-notification clearing for the now-displayed harness
		// is handled by a useEffect below — it covers every path
		// that changes the (active room, active harness) tuple, not
		// just tab clicks (Mod+1..9, Mod+Tab, palette, initial load).
	};

	const closeRoom = async (id: string) => {
		// Confirm before close — rooms can hold a lot of state and the
		// prototype has no undo (well, now there's the reopen modal —
		// but the user shouldn't have to discover that). Tauri's
		// plugin-dialog gives us a native confirm; window.confirm is
		// silently no-op'd in WebKit without a host-side handler.
		// #185: name unsaved editor buffers — archived rooms come back,
		// unsaved buffer text doesn't.
		const target = roomsRef.current.find((r) => r.id === id);
		const dirty = filesRegistry.anyDirty(target?.harnesses.map((h) => h.id) ?? []);
		const msg =
			dirty.length > 0
				? `Close this room? Any running harnesses will be killed and ${dirty.length} unsaved file${dirty.length === 1 ? "" : "s"} (${dirty.join(", ")}) discarded.`
				: "Close this room? Any running harnesses will be killed.";
		const ok = await confirm(msg, {
			title: "Skein",
			kind: "warning",
		});
		if (!ok) return;
		// Chapter 6 phase 2: archive instead of delete. Tab strip filters
		// archived out; reopen modal lists them.
		setRooms((prev) => prev.map((r) => (r.id === id ? { ...r, archived: Date.now() } : r)));
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

	// Fresh embedded-server ports for every opencode harness in a room
	// about to be (re)mounted. The ports from sqlite are dead — the run
	// that bound them released them on exit — and resumeCmd needs the
	// new ones to bake into the argv. Allocation failure is survivable:
	// that harness resumes without --port and its L2c-2 SSE adapter
	// simply doesn't attach.
	const allocateOpencodePorts = useCallback(async (room: Room | undefined) => {
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
	}, []);

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
			setRooms((prev) => prev.map((r) => (r.id === id ? transformed : r)));
			setActiveRoomId(id);
			// #76: an archived room predating `repoRoot` gets the same
			// background backfill hydrate does.
			backfillRepoRoots([room]);
			// #164: an archived room's folder may have vanished while it
			// was closed — check on the way back in, same as hydrate does
			// for rooms that were already active.
			void checkRoomFolder(transformed);
		},
		[allocateOpencodePorts, backfillRepoRoots, checkRoomFolder],
	);
	// The OS-notification listener is []-keyed (re-registering it on
	// every render would leak native listeners), so it reaches the
	// current unarchiveRoom through a ref rather than closing over it.
	const unarchiveRoomRef = useRef(unarchiveRoom);
	unarchiveRoomRef.current = unarchiveRoom;

	// #164: "Recreate worktree" on a MissingFolderCard. Only ever called
	// when `recoveryOptions` offered it (room.branch/repoRoot set, cwd a
	// worktree, branch still in the repo) — `git_restore_worktree`
	// re-attaches that branch at the room's existing `cwd`, so no argv
	// or sessionId needs rebuilding: the folder just reappears where
	// every harness already expects it.
	const recreateMissingWorktree = useCallback(
		async (room: Room): Promise<{ ok: true } | { ok: false; error: string }> => {
			if (!room.repoRoot || !room.branch || !room.cwd) {
				return { ok: false, error: "This room is missing a branch or repository record." };
			}
			try {
				await invoke("git_restore_worktree", {
					repoPath: room.repoRoot,
					branch: room.branch,
					worktreePath: room.cwd,
				});
				// #164: bump before clearing so an older in-flight
				// checkRoomFolder for this room can't re-add it after.
				folderCheckTokenRef.current.set(
					room.id,
					(folderCheckTokenRef.current.get(room.id) ?? 0) + 1,
				);
				setMissingFolders((prev) => {
					if (!prev.has(room.id)) return prev;
					const next = new Set(prev);
					next.delete(room.id);
					return next;
				});
				return { ok: true };
			} catch (err) {
				return { ok: false, error: err instanceof Error ? err.message : String(err) };
			}
		},
		[],
	);

	// #164: "Pick another folder" on a MissingFolderCard. Unlike the
	// worktree recreate above, the room's `cwd` itself changes, so every
	// harness that resumed into the old folder needs `repointRoom`'s
	// full treatment (dropped sessionId, fresh argv) — same port
	// allocation unarchive already does for opencode harnesses.
	const pickMissingFolder = useCallback(
		async (room: Room, newCwd: string) => {
			const portMap = await allocateOpencodePorts(room);
			// The picked folder may belong to a different repo (or none at
			// all) — inspect it before committing the repoint rather than
			// clearing `repoRoot` and relying on backfill: a repo gets its
			// own root, a plain folder gets no group, and a failed invoke
			// keeps the room's old repoRoot rather than losing it.
			let nextRepoRoot = room.repoRoot;
			try {
				const info = await invoke<FolderInfoDto>("git_inspect_folder", { path: newCwd });
				nextRepoRoot = info.isRepo ? info.root : undefined;
			} catch (err) {
				console.warn(`[skein] git_inspect_folder repoint check failed for room ${room.id}:`, err);
			}
			const base = repointRoom(room, newCwd, {
				fallbackShell: defaultShell,
				opencodePorts: portMap,
			});
			// `exactOptionalPropertyTypes` forbids `repoRoot: undefined` —
			// a plain (non-repo) folder, or one with no known root, must
			// drop the key entirely rather than set it to undefined.
			const { repoRoot: _droppedRepoRoot, ...rest } = base;
			const repointed: Room = nextRepoRoot ? { ...rest, repoRoot: nextRepoRoot } : rest;
			setRooms((prev) => prev.map((r) => (r.id === room.id ? repointed : r)));
			// #164: bump before clearing so an older in-flight
			// checkRoomFolder for this room can't re-add it after.
			folderCheckTokenRef.current.set(room.id, (folderCheckTokenRef.current.get(room.id) ?? 0) + 1);
			setMissingFolders((prev) => {
				if (!prev.has(room.id)) return prev;
				const next = new Set(prev);
				next.delete(room.id);
				return next;
			});
		},
		[allocateOpencodePorts, defaultShell],
	);

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

	return {
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
		switchRoom,
	};
}
