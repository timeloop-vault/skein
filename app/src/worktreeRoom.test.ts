import { describe, expect, it } from "vitest";
import type { FolderInfoDto } from "./NewRoomDialogTypes.ts";
import { type CreateRoomSpec, type WorktreeGit, createRoomArgs } from "./worktreeRoom.ts";

function folder(opts: Partial<FolderInfoDto> = {}): FolderInfoDto {
	return {
		exists: true,
		isRepo: true,
		root: "/repo",
		resolvedFromWorktree: false,
		branches: [
			{ name: "main", isHead: true },
			{ name: "feat/taken", isHead: false },
		],
		head: "main",
		...opts,
	};
}

/** A fake `WorktreeGit`: `inspectFolder` answers `folder` for the repo
 *  root and `worktreeExists` for anything else (the proposed worktree
 *  path), `proposeWorktreePath` and `addWorktree` are recorded so tests
 *  can assert what they were called with. */
function fakeGit(
	opts: {
		folder?: FolderInfoDto;
		worktreeExists?: boolean;
		proposedPath?: string;
	} = {},
): WorktreeGit & { addWorktreeCalls: unknown[]; proposeCalls: unknown[] } {
	const info = opts.folder ?? folder();
	const proposedPath = opts.proposedPath ?? "/repo-wt/leaf";
	const addWorktreeCalls: unknown[] = [];
	const proposeCalls: unknown[] = [];
	return {
		addWorktreeCalls,
		proposeCalls,
		inspectFolder: async (path: string) => {
			if (path === proposedPath) {
				return folder({ exists: opts.worktreeExists ?? false, isRepo: false });
			}
			return info;
		},
		proposeWorktreePath: async (repoPath: string, leaf: string) => {
			proposeCalls.push({ repoPath, leaf });
			return proposedPath;
		},
		addWorktree: async (args) => {
			addWorktreeCalls.push(args);
			return { name: args.branch, path: `${proposedPath}` };
		},
	};
}

const baseSpec: CreateRoomSpec = {
	folder: "/repo",
	task: "fix the thing",
	harness: "claude",
	branchMode: "worktree",
	branch: "feat/fix-the-thing",
	baseBranch: "main",
	branchTemplate: "skein/{slug}",
};

describe("createRoomArgs", () => {
	it("rejects an empty task", async () => {
		const outcome = await createRoomArgs({ ...baseSpec, task: "   " }, fakeGit());
		expect(outcome).toEqual({ ok: false, error: "task can't be empty" });
	});

	it("rejects a missing folder", async () => {
		const git = fakeGit({ folder: folder({ exists: false }) });
		const outcome = await createRoomArgs(baseSpec, git);
		expect(outcome).toEqual({ ok: false, error: "folder not found" });
	});

	it("rejects worktree mode on a non-repo folder", async () => {
		const git = fakeGit({ folder: folder({ isRepo: false }) });
		const outcome = await createRoomArgs(baseSpec, git);
		expect(outcome).toEqual({ ok: false, error: "not a git repo" });
	});

	it("treats a non-repo folder in current mode as a plain-folder room", async () => {
		const git = fakeGit({ folder: folder({ isRepo: false }) });
		const outcome = await createRoomArgs({ ...baseSpec, branchMode: "current" }, git);
		expect(outcome).toEqual({
			ok: true,
			args: { cwd: "/repo", task: "fix the thing", harness: "claude" },
			remember: { baseBranch: "" },
		});
	});

	it("rejects a colliding branch name", async () => {
		const outcome = await createRoomArgs({ ...baseSpec, branch: "feat/taken" }, fakeGit());
		expect(outcome).toEqual({ ok: false, error: "branch already exists" });
	});

	it("rejects an invalid branch name", async () => {
		const outcome = await createRoomArgs({ ...baseSpec, branch: "-bad" }, fakeGit());
		expect(outcome).toEqual({ ok: false, error: "can't start with -" });
	});

	it("rejects a worktree path that already exists", async () => {
		const git = fakeGit({ worktreeExists: true });
		const outcome = await createRoomArgs(baseSpec, git);
		expect(outcome).toEqual({ ok: false, error: "worktree folder already exists" });
	});

	it("rejects an unknown base branch", async () => {
		const outcome = await createRoomArgs({ ...baseSpec, baseBranch: "nope" }, fakeGit());
		expect(outcome).toEqual({ ok: false, error: "unknown base branch" });
	});

	it("applies the branch template when branch is omitted", async () => {
		const git = fakeGit();
		const { branch: _branch, ...rest } = baseSpec;
		const spec: CreateRoomSpec = rest;
		const outcome = await createRoomArgs(spec, git);
		expect(outcome.ok).toBe(true);
		if (outcome.ok) {
			expect(outcome.args.branch).toBe("skein/fix-the-thing");
		}
	});

	it("creates a worktree on the happy path", async () => {
		const git = fakeGit();
		const outcome = await createRoomArgs(baseSpec, git);
		expect(outcome).toEqual({
			ok: true,
			args: {
				cwd: "/repo-wt/leaf",
				task: "fix the thing",
				harness: "claude",
				branch: "feat/fix-the-thing",
				repoRoot: "/repo",
			},
			remember: { baseBranch: "main", branchTemplate: "feat/{slug}" },
		});
		expect(git.proposeCalls).toEqual([{ repoPath: "/repo", leaf: "fix-the-thing" }]);
		expect(git.addWorktreeCalls).toEqual([
			{
				repoPath: "/repo",
				branch: "feat/fix-the-thing",
				baseBranch: "main",
				worktreePath: "/repo-wt/leaf",
			},
		]);
	});

	it("passes an agent through when given", async () => {
		const git = fakeGit();
		const outcome = await createRoomArgs({ ...baseSpec, agent: "reviewer" }, git);
		expect(outcome.ok).toBe(true);
		if (outcome.ok) {
			expect(outcome.args.agent).toBe("reviewer");
		}
	});

	it("uses the repo HEAD for current mode, falling back to HEAD", async () => {
		const git = fakeGit();
		const outcome = await createRoomArgs({ ...baseSpec, branchMode: "current" }, git);
		expect(outcome).toEqual({
			ok: true,
			args: {
				cwd: "/repo",
				task: "fix the thing",
				harness: "claude",
				branch: "main",
				repoRoot: "/repo",
			},
			remember: { baseBranch: "main" },
		});

		const gitNoHead = fakeGit({ folder: folder({ head: null }) });
		const { baseBranch: _baseBranch, ...restSpec } = baseSpec;
		const noHeadOutcome = await createRoomArgs({ ...restSpec, branchMode: "current" }, gitNoHead);
		expect(noHeadOutcome.ok).toBe(true);
		if (noHeadOutcome.ok) {
			expect(noHeadOutcome.args.branch).toBe("HEAD");
			expect(noHeadOutcome.remember.baseBranch).toBe("main");
		}
	});
});
