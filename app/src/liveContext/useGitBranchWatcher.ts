// Lightweight git-branch watcher for the Live Context pane.
//
// LiveStatus used to run a full git status/diff watcher; the card
// stack only needs the live HEAD branch for the status bar (issue
// #18) until the Diff card's status/diff fetch lands in D3. One
// debounced filesystem watcher → git_head_branch on each tick.

import { Channel, invoke } from "@tauri-apps/api/core";
import { useEffect, useRef } from "react";

export function useGitBranchWatcher(cwd: string, onBranchChange?: (branch: string | null) => void) {
	const cbRef = useRef(onBranchChange);
	cbRef.current = onBranchChange;

	useEffect(() => {
		let cancelled = false;

		const refreshBranch = async () => {
			try {
				const repo = await invoke<boolean>("git_is_repo", { path: cwd });
				if (cancelled) return;
				if (!repo) {
					cbRef.current?.(null);
					return;
				}
				const branch = await invoke<string | null>("git_head_branch", { path: cwd });
				if (!cancelled) cbRef.current?.(branch);
			} catch (err: unknown) {
				const msg = err instanceof Error ? err.message : String(err);
				console.warn(`[skein] branch watch failed for ${cwd}:`, msg);
			}
		};

		void refreshBranch();

		const channel = new Channel<null>();
		channel.onmessage = () => {
			void refreshBranch();
		};
		let watchId: string | null = null;
		invoke<string>("git_watch_start", { path: cwd, onChange: channel })
			.then((id) => {
				if (cancelled) {
					void invoke("git_watch_stop", { id });
					return;
				}
				watchId = id;
			})
			.catch((err: unknown) => {
				const msg = err instanceof Error ? err.message : String(err);
				console.error("[skein] git_watch_start failed:", msg);
			});

		return () => {
			cancelled = true;
			if (watchId) void invoke("git_watch_stop", { id: watchId });
		};
	}, [cwd]);
}
