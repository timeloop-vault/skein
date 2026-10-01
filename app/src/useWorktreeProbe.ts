import { invoke } from "@tauri-apps/api/core";
import { useEffect, useState } from "react";
import { worktreeLeaf } from "./branchName.ts";
import type { FolderInfoDto } from "./NewRoomDialogTypes.ts";

// Split out of `useNewRoomForm.tsx` (#461): the debounced probe for the
// worktree a submit would create.
export const useWorktreeProbe = ({
	isRepo,
	branchMode,
	branch,
	cwd,
}: {
	isRepo: boolean;
	branchMode: "worktree" | "current";
	branch: string;
	cwd: string;
}) => {
	const [worktreePath, setWorktreePath] = useState("");
	const [worktreeFolderExists, setWorktreeFolderExists] = useState(false);
	// #227 review: `worktreeFolderExists` only reflects the *last landed*
	// probe, which `canCreate` read synchronously — a branch typed after
	// the last probe answered could submit before this one comes back.
	// Tracked separately from `busy`/`repoStatus.kind === "checking"`
	// because it is its own async gate with its own debounce.
	const [worktreeCheckPending, setWorktreeCheckPending] = useState(false);
	useEffect(() => {
		if (!isRepo || branchMode !== "worktree" || !branch.trim()) {
			setWorktreePath("");
			setWorktreeFolderExists(false);
			setWorktreeCheckPending(false);
			return undefined;
		}
		setWorktreeCheckPending(true);
		let cancelled = false;
		const handle = window.setTimeout(() => {
			void (async () => {
				try {
					const path = await invoke<string>("git_propose_worktree_path", {
						repoPath: cwd,
						taskSlug: worktreeLeaf(branch),
					});
					if (cancelled) return;
					setWorktreePath(path);
					const info = await invoke<FolderInfoDto>("git_inspect_folder", { path });
					if (cancelled) return;
					setWorktreeFolderExists(info.exists);
				} catch {
					if (cancelled) return;
					setWorktreePath("");
					setWorktreeFolderExists(false);
				} finally {
					if (!cancelled) setWorktreeCheckPending(false);
				}
			})();
		}, 200);
		return () => {
			cancelled = true;
			window.clearTimeout(handle);
		};
	}, [isRepo, branchMode, branch, cwd]);
	return { worktreePath, worktreeFolderExists, worktreeCheckPending };
};
