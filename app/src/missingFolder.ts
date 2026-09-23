// Issue #164: a room whose folder (`Room.cwd`) no longer exists must
// never silently run its harnesses somewhere else. Pure logic lives
// here — App.tsx owns the runtime `missingFolders` set (detection is
// async, via `git_inspect_folder`, and deliberately not a `Room`
// field: it is a fact about the filesystem right now, not something
// to persist) and MissingFolderCard.tsx owns the chrome.

import { HARNESS_KINDS } from "./data.tsx";
import { cmdForKind } from "./harnessCmd.ts";
import type { Harness, Room } from "./types.ts";

/** The slice of `FolderInfoDto` (git.rs / App.tsx) that `recoveryOptions`
 *  needs about the room's `repoRoot` — never about the missing
 *  `room.cwd` itself, which is a different folder. */
export interface RepoRootInfo {
	exists: boolean;
	isRepo: boolean;
	branches: { name: string }[];
}

export type RecoveryOptions =
	| { canRecreateWorktree: true }
	| { canRecreateWorktree: false; reason: string };

/** Whether "Recreate worktree" should be offered for a room whose
 *  folder is missing, and why not when it can't be. Never offered
 *  speculatively: `git_restore_worktree` re-attaches an *existing*
 *  branch at the room's old path, so it only makes sense when the
 *  room really was a worktree (its cwd differs from its repoRoot),
 *  the repo it came from is still there, and the branch is still in
 *  it. `repoInfo` undefined means "not checked yet" — same as any
 *  other reason not to offer it. */
export function recoveryOptions(room: Room, repoInfo: RepoRootInfo | undefined): RecoveryOptions {
	if (!room.branch) {
		return { canRecreateWorktree: false, reason: "This room has no branch on record." };
	}
	if (!room.repoRoot) {
		return { canRecreateWorktree: false, reason: "This room's repository isn't known." };
	}
	if (room.cwd === room.repoRoot) {
		return {
			canRecreateWorktree: false,
			reason: "This room is the main checkout, not a worktree.",
		};
	}
	if (!repoInfo) {
		return { canRecreateWorktree: false, reason: "Checking the repository…" };
	}
	if (!repoInfo.exists) {
		return {
			canRecreateWorktree: false,
			reason: `The repository at ${room.repoRoot} is also missing.`,
		};
	}
	if (!repoInfo.isRepo) {
		return {
			canRecreateWorktree: false,
			reason: `${room.repoRoot} is no longer a git repository.`,
		};
	}
	if (!repoInfo.branches.some((b) => b.name === room.branch)) {
		return {
			canRecreateWorktree: false,
			reason: `Branch "${room.branch}" no longer exists in the repository.`,
		};
	}
	return { canRecreateWorktree: true };
}

/** Move a room to a newly picked folder. Every harness whose `cwd`
 *  equalled the room's old `cwd` moves with it — which today is every
 *  harness, since `createHarnessInRoom` always stamps the room's cwd,
 *  but the field is per-harness so this only touches the ones that
 *  actually pointed at the folder that went missing.
 *
 *  A resumed conversation is keyed to the OLD folder — Claude's own
 *  store is keyed by cwd — so a moved harness that can resume
 *  (`capabilities.resume`) can't resume into the new one: drop
 *  `sessionId` and rebuild a fresh spawn argv via `cmdForKind`, never
 *  by pattern-matching the old argv (see harnessCmd.ts's header
 *  comment on exactly that trap). A harness that can't resume anyway
 *  (copilot, a shell that may carry a user-swapped command) just
 *  moves — there's no conversation to lose, and rebuilding its argv
 *  from scratch would discard that customization. `spawnGen` is
 *  bumped either way so LiveTerminal remounts even when the argv is
 *  unchanged. */
export function repointRoom(
	room: Room,
	newCwd: string,
	opts: { fallbackShell?: string[]; opencodePorts?: ReadonlyMap<string, number> } = {},
): Room {
	const { fallbackShell = [], opencodePorts = new Map() } = opts;
	const oldCwd = room.cwd;
	const harnesses = room.harnesses.map((h): Harness => {
		if (h.cwd !== oldCwd) return h;
		if (!h.cmd) return { ...h, cwd: newCwd };
		if (!HARNESS_KINDS[h.kind].capabilities.resume) {
			return { ...h, cwd: newCwd, spawnGen: (h.spawnGen ?? 0) + 1 };
		}
		const { sessionId, ...rest } = h;
		const cmd = cmdForKind(h.kind, fallbackShell, undefined, opencodePorts.get(h.id), h.agent);
		return { ...rest, cwd: newCwd, cmd, spawnGen: (h.spawnGen ?? 0) + 1 };
	});
	return { ...room, cwd: newCwd, harnesses };
}
