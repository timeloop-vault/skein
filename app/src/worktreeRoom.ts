import { invoke } from "@tauri-apps/api/core";
import type { CreateRoomArgs, FolderInfoDto } from "./NewRoomDialogTypes.ts";
import {
	applyBranchTemplate,
	branchFieldProblem,
	taskSlug,
	templateFromBranch,
	worktreeLeaf,
} from "./branchName.ts";
import type { HarnessKind } from "./types.ts";

// One place that turns "folder + task + harness + branch choice" into a
// `CreateRoomArgs` (#328), creating the worktree when asked. Split out of
// `useNewRoomForm.tsx`'s `submit()` so the New Room dialog and the future
// agent-API room-creation handler (#330) share one set of rules instead of
// two copies that can drift.

/** The git-backed operations this needs, injected so tests don't spawn a
 *  real `invoke`. `defaultWorktreeGit` below is the real implementation,
 *  wired to the exact Tauri commands / arg keys the dialog used inline. */
export interface WorktreeGit {
	inspectFolder(path: string): Promise<FolderInfoDto>;
	proposeWorktreePath(repoPath: string, leaf: string): Promise<string>;
	addWorktree(args: {
		repoPath: string;
		branch: string;
		baseBranch: string;
		worktreePath: string;
	}): Promise<{ name: string; path: string }>;
}

export const defaultWorktreeGit: WorktreeGit = {
	inspectFolder: (path) => invoke<FolderInfoDto>("git_inspect_folder", { path }),
	proposeWorktreePath: (repoPath, leaf) =>
		invoke<string>("git_propose_worktree_path", { repoPath, taskSlug: leaf }),
	addWorktree: ({ repoPath, branch, baseBranch, worktreePath }) =>
		invoke<{ name: string; path: string }>("git_add_worktree", {
			repoPath,
			branch,
			baseBranch,
			worktreePath,
		}),
};

export interface CreateRoomSpec {
	folder: string;
	task: string;
	harness: HarnessKind;
	/** The agent the starting harness spawns as (#247), or absent for the
	 *  tool's own default. */
	agent?: string;
	branchMode: "worktree" | "current";
	/** Worktree mode only. Omitted → `branchTemplate` applied to the task
	 *  slug. Current mode ignores this — the branch is always the repo's
	 *  own HEAD, exactly as the dialog does today. */
	branch?: string;
	/** Omitted → the repo's HEAD, falling back to its first branch, falling
	 *  back to "" for a folder with no branches at all. */
	baseBranch?: string;
	/** The template a blank `branch` fills in via `applyBranchTemplate`. */
	branchTemplate: string;
}

/** What the caller should remember about a successful create — mirrors
 *  the `remember()` closure `submit()` used to run inline. */
export interface CreateRoomRemember {
	baseBranch: string;
	branchTemplate?: string;
}

export type CreateRoomOutcome =
	| { ok: true; args: CreateRoomArgs; remember: CreateRoomRemember }
	| { ok: false; error: string };

/**
 * Validate `spec` and produce the `CreateRoomArgs` for a new room,
 * creating the worktree when `branchMode` is `"worktree"`.
 *
 * Validation failures (empty task, missing folder, a repo-only mode on a
 * non-repo folder, a bad/colliding branch name, a worktree folder already
 * sitting at the proposed path, an unknown base branch) come back as
 * `{ ok: false, error }` — no throw. A failure from an underlying git call
 * itself (a network hiccup, `git_add_worktree` failing partway) propagates
 * as a thrown error, same as the inline `submit()` this replaces.
 */
export async function createRoomArgs(
	spec: CreateRoomSpec,
	git: WorktreeGit = defaultWorktreeGit,
): Promise<CreateRoomOutcome> {
	const task = spec.task.trim();
	if (!task) return { ok: false, error: "task can't be empty" };

	const info = await git.inspectFolder(spec.folder);
	if (!info.exists) return { ok: false, error: "folder not found" };

	if (!info.isRepo) {
		if (spec.branchMode === "worktree") {
			return { ok: false, error: "not a git repo" };
		}
		// Non-git folder, "current" mode: a plain folder room, no branch —
		// exactly what the dialog has always done for a non-repo folder.
		return {
			ok: true,
			args: {
				cwd: spec.folder,
				task,
				harness: spec.harness,
				...(spec.agent ? { agent: spec.agent } : {}),
			},
			remember: { baseBranch: "" },
		};
	}

	const repoRoot = info.root;
	const defaultBaseBranch = info.head || info.branches[0]?.name || "";

	if (spec.branchMode === "current") {
		const baseBranch = spec.baseBranch ?? defaultBaseBranch;
		return {
			ok: true,
			args: {
				cwd: spec.folder,
				task,
				harness: spec.harness,
				...(spec.agent ? { agent: spec.agent } : {}),
				branch: info.head ?? "HEAD",
				repoRoot,
			},
			remember: { baseBranch },
		};
	}

	// Worktree mode.
	const branch = spec.branch?.trim() || applyBranchTemplate(spec.branchTemplate, taskSlug(task));
	const baseBranch = spec.baseBranch ?? defaultBaseBranch;
	if (!baseBranch) return { ok: false, error: "no base branch available" };
	if (spec.baseBranch && !info.branches.some((b) => b.name === spec.baseBranch)) {
		return { ok: false, error: "unknown base branch" };
	}

	const worktreePath = await git.proposeWorktreePath(spec.folder, worktreeLeaf(branch));
	const worktreeInfo = await git.inspectFolder(worktreePath);
	const problem = branchFieldProblem(
		branch,
		info.branches.map((b) => b.name),
		worktreeInfo.exists,
	);
	if (problem) return { ok: false, error: problem };

	const wt = await git.addWorktree({
		repoPath: spec.folder,
		branch,
		baseBranch,
		worktreePath,
	});

	return {
		ok: true,
		args: {
			cwd: wt.path,
			task,
			harness: spec.harness,
			...(spec.agent ? { agent: spec.agent } : {}),
			branch,
			repoRoot,
		},
		remember: { baseBranch, branchTemplate: templateFromBranch(branch) },
	};
}
