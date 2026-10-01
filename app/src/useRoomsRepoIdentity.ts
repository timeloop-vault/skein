// Missing-folder recovery + repo-identity half of useRoomsStore (#459
// split of #19): #164 recreate/repoint, #418 "same repo" / retire.
// State and refs are owned by useRoomsStore and passed in.

import { invoke } from "@tauri-apps/api/core";
import { type Dispatch, type MutableRefObject, type SetStateAction, useCallback } from "react";
import type { FolderInfoDto } from "./NewRoomDialog.tsx";
import type { RenameTarget } from "./RoomStrip.tsx";
import { repointRoom } from "./missingFolder.ts";
import { type IdentityCheck, isStorable } from "./repoIdentity.ts";
import { nextActiveAfterClose } from "./roomGroups.ts";
import { checkProbe, probeFolder } from "./roomsStoreProbe.ts";
import type { RepoIdentity, Room } from "./types.ts";

export interface RoomsRepoIdentityDeps {
	defaultShell: string[];
	activeRooms: Room[];
	roomsRef: MutableRefObject<Room[]>;
	activeRoomIdRef: MutableRefObject<string>;
	mismatchedRef: MutableRefObject<Set<string>>;
	identityTriedRef: MutableRefObject<Set<string>>;
	folderCheckTokenRef: MutableRefObject<Map<string, number>>;
	lastUsedByGroupRef: MutableRefObject<Map<string, string>>;
	setRooms: Dispatch<SetStateAction<Room[]>>;
	setActiveRoomId: Dispatch<SetStateAction<string>>;
	setRenaming: Dispatch<SetStateAction<RenameTarget | null>>;
	setMissingFolders: Dispatch<SetStateAction<Set<string>>>;
	setMismatch: (id: string, on: boolean) => void;
	syncRepoMeta: (targets: Room[]) => void;
	allocateOpencodePorts: (room: Room | undefined) => Promise<Map<string, number>>;
}

export function useRoomsRepoIdentity(d: RoomsRepoIdentityDeps) {
	const {
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
	} = d;

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
		[folderCheckTokenRef, setMissingFolders],
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
			// #418: the new folder is judged against the room's stored
			// identity; a room with none adopts the new folder's.
			let identityCheck: IdentityCheck = "unknown";
			let adopted: RepoIdentity | undefined;
			const probe = await probeFolder(newCwd);
			if (probe) {
				identityCheck = checkProbe(room.repoIdentity, probe);
				if (!room.repoIdentity && isStorable(probe.identity)) adopted = probe.identity;
			}
			const base = repointRoom(room, newCwd, {
				fallbackShell: defaultShell,
				opencodePorts: portMap,
			});
			// `exactOptionalPropertyTypes` forbids `repoRoot: undefined` —
			// a plain (non-repo) folder, or one with no known root, must
			// drop the key entirely rather than set it to undefined.
			const { repoRoot: _droppedRepoRoot, ...rest } = base;
			const withRoot: Room = nextRepoRoot ? { ...rest, repoRoot: nextRepoRoot } : rest;
			const repointed: Room = adopted ? { ...withRoot, repoIdentity: adopted } : withRoot;
			identityTriedRef.current.add(room.id);
			mismatchedRef.current = new Set(mismatchedRef.current);
			if (identityCheck === "mismatch") mismatchedRef.current.add(room.id);
			else mismatchedRef.current.delete(room.id);
			setMismatch(room.id, identityCheck === "mismatch");
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
		[
			allocateOpencodePorts,
			defaultShell,
			setMismatch,
			identityTriedRef,
			mismatchedRef,
			folderCheckTokenRef,
			setRooms,
			setMissingFolders,
		],
	);

	// #418: the user says the folder's repo IS the room's (a re-clone,
	// a history rewrite). Store the folder's current identity, lift the
	// gate, then let the normal repoRoot re-derive run against it.
	const confirmSameRepo = async (id: string) => {
		const target = roomsRef.current.find((r) => r.id === id);
		if (!target) return;
		const probe = target.cwd ? await probeFolder(target.cwd) : null;
		// Not storable (not a repo / unborn / shallow: empty roots) lifts the
		// gate for this session but stores nothing, so the next hydrate may
		// flag the room again.
		const identity = probe && isStorable(probe.identity) ? probe.identity : undefined;
		const updated: Room = identity ? { ...target, repoIdentity: identity } : target;
		if (identity) setRooms((prev) => prev.map((r) => (r.id === id ? updated : r)));
		mismatchedRef.current = new Set(mismatchedRef.current);
		mismatchedRef.current.delete(id);
		setMismatch(id, false);
		syncRepoMeta([updated]);
	};

	// #418: the folder holds another repo and the room's history is
	// what is left worth keeping: archive + retire (#417) without the
	// close confirm, moving focus as closeRoom does.
	const retireMismatchedRoom = (id: string) => {
		const now = Date.now();
		setRooms((prev) => prev.map((r) => (r.id === id ? { ...r, archived: now, retired: now } : r)));
		setMismatch(id, false);
		if (id === activeRoomIdRef.current) {
			const nextActive = nextActiveAfterClose(activeRooms, id, lastUsedByGroupRef.current);
			setActiveRoomId(nextActive?.id ?? "");
		}
		setRenaming((cur) => (cur?.roomId === id ? null : cur));
	};

	return { recreateMissingWorktree, pickMissingFolder, confirmSameRepo, retireMismatchedRoom };
}
