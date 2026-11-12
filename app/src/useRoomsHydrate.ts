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
} from "react";
import { roomsResumeCmds } from "./harnessArgvApi.ts";
import { applyResumedRooms } from "./harnessArgvApply.ts";
import { hydrateFailure, ResumePrepareError } from "./hydrateFailure.ts";
import { isStorable, nextRepoRoot } from "./repoIdentity.ts";
import {
	checkProbe,
	type DbLoadOutcome,
	inspectFolderMissing,
	probeFolder,
	stillExists,
} from "./roomsStoreProbe.ts";
import type { Room } from "./types.ts";
import { useRoomsSync } from "./useRoomsSync.ts";

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
	// #729: diff-based sync with the daemon's room store; parked until loaded.
	const { beginHydrate, finishHydrate, flushRoomsSave } = useRoomsSync({
		rooms,
		loaded,
		loadFailed,
		setRooms,
	});
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
		// #729: buffer foreign room events until the rooms are installed;
		// finishHydrate sets the baseline (the rows) and replays them.
		beginHydrate();
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
					// #730: the daemon rewrites each harness's cmd to its resume
					// form (so the PTY spawn re-attaches to the prior conversation
					// instead of starting fresh) and pre-allocates fresh
					// embedded-server ports for every resumed opencode harness
					// (Epic #50 L2c-2; the previous run released its ports on
					// exit). Stale sessionIds were dropped above, before this
					// call. Only the daemon-owned fields merge back by id, since
					// the Rust structs would drop any field they don't know. A
					// harness whose port allocation failed just resumes without
					// --port and its SSE adapter doesn't attach.
					// A rejection throws ResumePrepareError before anything is
					// applied (no ports, rooms or loaded flag) and reaches the
					// retry card via the catch below.
					const resumed = await roomsResumeCmds(verified).catch((err: unknown) => {
						throw new ResumePrepareError(err);
					});
					setOpencodePorts(new Map(Object.entries(resumed.ports)));
					const withResume = applyResumedRooms(verified, resumed.rooms);
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
					// Baseline = rows; the transforms diff out as mutations.
					const installed = finishHydrate(rows, withResume);
					setRooms(installed);
					syncRepoMeta(installed);
					// Pick the first *active* room; archived ones aren't
					// supposed to be the boot-time selection.
					const first = installed.find((r) => !r.archived);
					if (first) setActiveRoomId(first.id);
				} else {
					// Nothing stored, but a foreign client may have created rooms
					// while we loaded.
					const installed = finishHydrate(rows, rows);
					if (installed.length > 0) {
						setRooms(installed);
						syncRepoMeta(installed);
						const first = installed.find((r) => !r.archived);
						if (first) setActiveRoomId(first.id);
					}
				}
				loadedRef.current = true;
				setLoaded(true);
				// A sibling's failure card may have landed while we were
				// resuming; success wins, or it would park the autosave.
				setLoadFailed(null);
			})
			.catch((err: unknown) => {
				console.error("[skein] hydrate failed:", err);
				const failure = hydrateFailure(err, loadedRef.current, hydratedOnceRef.current);
				hydratedOnceRef.current = failure.hydratedOnce;
				if (failure.message === null) return;
				// #167: deliberately NOT setLoaded(true) here. Flipping
				// `loaded` with the initial [] still in state arms the
				// autosave, whose next write is DELETE FROM sessions —
				// the boot-wipe chain this issue exists to break.
				setLoadFailed(failure.message);
			});
	}, [
		syncRepoMeta,
		beginHydrate,
		finishHydrate,
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

	// Phase 3: sync `rooms` to the daemon as mutations (#729); the daemon
	// coalesces the writes (#594) — structural changes at once, the rest
	// after a debounce. See roomsSync.ts. Parked while !loaded / loadFailed
	// (#167), and a flush while parked is a no-op because nothing was ever
	// sent from unloaded state.
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
