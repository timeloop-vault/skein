// Hydrate + autosave half of useRoomsStore (#459 split of #19): the
// boot load from sqlite, the #167 hardening (autosave parked until a
// successful load, quarantine/backup surfacing), and the #76 / #418
// repo-metadata sync. State and refs are owned by useRoomsStore and
// passed in, so hook order there is unchanged.

import { invoke } from "@tauri-apps/api/core";
import {
	type Dispatch,
	type MutableRefObject,
	type SetStateAction,
	useCallback,
	useEffect,
	useRef,
} from "react";
import { withResumeCmds } from "./harnessCmd.ts";
import { isStorable, nextRepoRoot } from "./repoIdentity.ts";
import { registerRoomsFlusher } from "./roomsFlush.ts";
import { createRoomsSaveScheduler, type RoomsSaveScheduler } from "./roomsSaveScheduler.ts";
import {
	checkProbe,
	type DbLoadOutcome,
	inspectFolderMissing,
	probeFolder,
	stillExists,
} from "./roomsStoreProbe.ts";
import type { Room } from "./types.ts";

export interface RoomsHydrateDeps {
	rooms: Room[];
	loaded: boolean;
	loadFailed: string | null;
	setRooms: Dispatch<SetStateAction<Room[]>>;
	setActiveRoomId: Dispatch<SetStateAction<string>>;
	setOpencodePorts: Dispatch<SetStateAction<Map<string, number>>>;
	setLoaded: Dispatch<SetStateAction<boolean>>;
	setLoadFailed: Dispatch<SetStateAction<string | null>>;
	setQuarantinedCount: Dispatch<SetStateAction<number>>;
	setBackupRoomCount: Dispatch<SetStateAction<number | null>>;
	setMissingFolders: Dispatch<SetStateAction<Set<string>>>;
	setMismatchedRooms: Dispatch<SetStateAction<Set<string>>>;
	mismatchedRef: MutableRefObject<Set<string>>;
	identityTriedRef: MutableRefObject<Set<string>>;
	hydratedOnceRef: MutableRefObject<boolean>;
	loadedRef: MutableRefObject<boolean>;
}

export function useRoomsHydrate(d: RoomsHydrateDeps) {
	const {
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
	} = d;
	// #76 + #418: background repo metadata for rooms; never awaited by a
	// caller, so a slow or failing repo cannot delay or break hydrate/
	// reopen. Per room, in parallel: (1) a room without `repoRoot` gets
	// it filled (#76); (2) a room without `repoIdentity` gets it stored
	// unless it is currently mismatched (the folder's repo is not the
	// room's, so it must not be recorded as the room's); (3) a stored
	// `repoRoot` is re-derived when it differs, but ONLY when the
	// identity check says "same": unknown or mismatch keep the old
	// group. Never clears anything.
	const syncRepoMeta = useCallback(
		(targets: Room[]) => {
			const pending = targets.filter((r) => r.cwd);
			if (pending.length === 0) return;
			void Promise.all(
				pending.map(async (r) => {
					identityTriedRef.current.add(r.id);
					if (!r.cwd) return;
					const probe = await probeFolder(r.cwd);
					if (!probe) return;
					const check = checkProbe(r.repoIdentity, probe);
					if (check === "mismatch" || mismatchedRef.current.has(r.id)) return;
					const derived = probe.info.exists && probe.info.isRepo ? probe.info.root : undefined;
					const fillIdentity =
						!r.repoIdentity && isStorable(probe.identity) ? probe.identity : undefined;
					setRooms((prev) =>
						prev.map((x) => {
							if (x.id !== r.id || x.cwd !== r.cwd) return x;
							const root = nextRepoRoot(x.repoRoot, derived, check);
							const identity = x.repoIdentity ?? fillIdentity;
							if (root === x.repoRoot && identity === x.repoIdentity) return x;
							return {
								...x,
								...(root ? { repoRoot: root } : {}),
								...(identity ? { repoIdentity: identity } : {}),
							};
						}),
					);
				}),
			);
		},
		[identityTriedRef, mismatchedRef, setRooms],
	);
	const hydrateRooms = useCallback(() => {
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
					// here — unarchive checks on the way back in.
					const initialMissing = new Set<string>();
					await Promise.all(
						withResume.map(async (r) => {
							if (r.archived || !r.cwd) return;
							if (await inspectFolderMissing(r.cwd, r.id)) initialMissing.add(r.id);
						}),
					);
					setMissingFolders(initialMissing);
					// #418: same gate for a folder that now holds a different
					// repo, decided before mount so nothing resumes there.
					const initialMismatch = new Set<string>();
					await Promise.all(
						withResume.map(async (r) => {
							if (r.archived || !r.cwd || !r.repoIdentity || initialMissing.has(r.id)) return;
							const probe = await probeFolder(r.cwd);
							if (probe && checkProbe(r.repoIdentity, probe) === "mismatch") {
								initialMismatch.add(r.id);
							}
						}),
					);
					mismatchedRef.current = initialMismatch;
					setMismatchedRooms(initialMismatch);
					setRooms(withResume);
					syncRepoMeta(withResume);
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
	}, [
		syncRepoMeta,
		hydratedOnceRef,
		loadedRef,
		mismatchedRef,
		setActiveRoomId,
		setBackupRoomCount,
		setLoaded,
		setLoadFailed,
		setMismatchedRooms,
		setMissingFolders,
		setOpencodePorts,
		setQuarantinedCount,
		setRooms,
	]);

	useEffect(() => {
		hydrateRooms();
	}, [hydrateRooms]);

	// Phase 3: mirror `rooms` to sqlite, coalesced (#594) — structural
	// changes save at once, the rest trail a debounce; see
	// roomsSaveScheduler.ts. Parked while !loaded / loadFailed (#167), and
	// a flush while parked is a no-op because nothing was ever scheduled
	// from unloaded state.
	const guardRef = useRef(false);
	guardRef.current = loaded && loadFailed === null;
	const schedulerRef = useRef<RoomsSaveScheduler | null>(null);
	if (schedulerRef.current === null) {
		schedulerRef.current = createRoomsSaveScheduler({
			save: (r) => invoke("db_save_rooms", { rooms: r }),
			// Belt and braces for the debounced fire (#167): never save
			// from an unloaded / failed state.
			canSave: () => guardRef.current,
		});
	}
	useEffect(() => {
		if (!loaded || loadFailed !== null) return;
		schedulerRef.current?.update(rooms);
	}, [rooms, loaded, loadFailed]);
	const flushRoomsSave = useCallback(async () => {
		await schedulerRef.current?.flush();
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

	// #418: a room created this session (New room, worktree room,
	// agent create_room) reaches `rooms` through setRooms with no
	// identity; one effect keyed on that fact covers every creation path
	// instead of patching each. Rooms already handled by hydrate or
	// unarchive are in `identityTriedRef`.
	useEffect(() => {
		if (!loaded) return;
		const fresh = rooms.filter(
			(r) => !r.repoIdentity && r.cwd && !identityTriedRef.current.has(r.id),
		);
		if (fresh.length > 0) syncRepoMeta(fresh);
	}, [rooms, loaded, syncRepoMeta, identityTriedRef]);

	return { syncRepoMeta, hydrateRooms, flushRoomsSave };
}
