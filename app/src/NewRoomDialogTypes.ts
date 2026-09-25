import type { HarnessKind } from "./types.ts";

// Types shared by `NewRoomDialog` and its `useNewRoomForm` hook — split out
// of `NewRoomDialog.tsx` (#19) so both files can import them without one
// depending on the other's JSX.

export interface BranchInfoDto {
	name: string;
	isHead: boolean;
	/** How many commits behind its upstream, if it has one (#367). Absent
	 *  when the branch tracks nothing; a lower bound even when present —
	 *  Skein never fetches, so this is only as fresh as the last fetch. */
	behindUpstream?: number;
}

// What the dialog hands back. The cwd is already the *real* directory
// the spawn should land in — for "New worktree" mode the dialog has
// already called git_add_worktree and resolved the worktree path; for
// "Current branch" mode it's the picked repo path; for non-git rooms
// (chapter 6 phase 3) it's the picked folder verbatim, with branch
// undefined.
export interface CreateRoomArgs {
	cwd: string;
	task: string;
	harness: HarnessKind;
	/** The agent the starting harness spawns as (#247), or absent for
	 *  the tool's own default. */
	agent?: string;
	branch?: string;
	/** The resolved repo root (#76's room-group key), when the folder is
	 *  a git repo — the main checkout even when the picked folder was a
	 *  worktree. Absent for non-git rooms. */
	repoRoot?: string;
	/** #330: set only by the `create_room` agent-request handler — the
	 *  New Room dialog never carries one. */
	createdBy?: { roomId: string; harnessId: string };
}

// What `git_inspect_folder` answers, mirroring `FolderInfoDto` in
// `app/src-tauri/src/git.rs`.
export interface FolderInfoDto {
	exists: boolean;
	isRepo: boolean;
	root: string;
	resolvedFromWorktree: boolean;
	branches: BranchInfoDto[];
	head: string | null;
	/** Remote-tracking refs (e.g. "origin/main"), never origin/HEAD (#367).
	 *  A valid `baseBranch` for worktree creation alongside `branches` — a
	 *  local branch of the same short name wins if both exist. */
	remoteBranches: string[];
}

// `missing` is not cosmetic (#226): a path that does not exist used to
// land on `not-a-repo`, which is a *submittable* state ("harnesses run
// in this folder as-is"). That was harmless while the field started
// blank; with a remembered folder prefilled, one stale path plus one
// Enter would create a room whose cwd does not exist.
export type RepoStatus =
	| { kind: "empty" }
	| { kind: "checking" }
	| { kind: "valid"; branches: BranchInfoDto[]; head: string | null }
	| { kind: "not-a-repo" }
	| { kind: "missing" };
