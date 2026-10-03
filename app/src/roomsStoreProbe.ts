// The non-React halves of useRoomsStore (#459 split of #19): folder
// probing (#164 / #418) and the wire shape of `db_load_rooms` (#167).

import { invoke } from "@tauri-apps/api/core";
import type { FolderInfoDto } from "./NewRoomDialog.tsx";
import {
	compareIdentity,
	type IdentityCheck,
	identityFromDto,
	type RepoIdentityDto,
} from "./repoIdentity.ts";
import type { Harness, RepoIdentity, Room } from "./types.ts";

/** #418: what is at `path` right now: the folder facts and its repo
 *  identity. null on a failed invoke: callers treat that as "unknown",
 *  never as a mismatch. */
export interface FolderProbe {
	info: FolderInfoDto;
	identity: RepoIdentity | null;
	shallow: boolean;
}
export async function probeFolder(path: string): Promise<FolderProbe | null> {
	try {
		const info = await invoke<FolderInfoDto>("git_inspect_folder", { path });
		if (!info.exists) return { info, identity: null, shallow: false };
		const dto = await invoke<RepoIdentityDto | null>("git_repo_identity", { path });
		return {
			info,
			identity: dto ? identityFromDto(dto) : null,
			shallow: dto?.shallow === true,
		};
	} catch (err) {
		console.warn(`[skein] folder probe failed for ${path}:`, err);
		return null;
	}
}
export const checkProbe = (stored: RepoIdentity | undefined, p: FolderProbe): IdentityCheck =>
	compareIdentity(stored, {
		exists: p.info.exists,
		identity: p.identity,
		shallow: p.shallow,
	});

/** Wire shape of `db_load_rooms` (#167): the rooms that parsed plus
 *  any rows the backend quarantined instead of failing the load.
 *  `backupRooms` arrives only when the live table was empty but the
 *  skein.db.bak snapshot still holds rooms. */
export interface DbLoadOutcome {
	rooms: Room[];
	skipped: { id: string; error: string }[];
	backupRooms?: number;
}

// #164: does `cwd` exist right now? A failed invoke is treated as
// "unknown" rather than missing — the check is conservative in the
// same direction as the #153 sessionId-existence probes: a
// transient rusqlite/fs hiccup must not park a perfectly good room
// behind a false "folder missing" card.
export async function inspectFolderMissing(cwd: string, roomId: string): Promise<boolean> {
	try {
		const info = await invoke<FolderInfoDto>("git_inspect_folder", { path: cwd });
		return !info.exists;
	} catch (err) {
		console.warn(`[skein] git_inspect_folder folder-check failed for room ${roomId}:`, err);
		return false;
	}
}

/** #508: strip the sessionId of every Claude harness whose transcript
 *  was never written, so the reopen path mints a fresh `--session-id`
 *  (#486) instead of spawning `claude --resume <id>` against nothing.
 *  A fresh harness's id is pre-allocated by `cmdForKind`, and a room
 *  archived in the same run never passed hydrate's probe. opencode
 *  harnesses are returned untouched. `exists` errors are the caller's
 *  to make conservative (`stillExists` returns true on a failed invoke). */
export async function dropUnwrittenClaudeSessions(
	room: Room,
	exists: (h: Harness) => Promise<boolean>,
): Promise<Room> {
	const harnesses = await Promise.all(
		room.harnesses.map(async (h) => {
			if (h.kind !== "claude" || !h.sessionId) return h;
			if (await exists(h)) return h;
			console.info(`[skein] dropping unwritten claude session ${h.sessionId} on reopen`);
			const { sessionId: _dropped, ...rest } = h;
			return rest;
		}),
	);
	return { ...room, harnesses };
}

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
export async function stillExists(h: Harness): Promise<boolean> {
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
}
