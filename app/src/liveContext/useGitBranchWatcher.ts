// Lightweight git-branch watcher for the Live Context pane.
//
// The status bar only needs the live HEAD branch (issue #18). The watch
// is on the one file that decides it: `git_head_watch_start` watches the
// git dir holding HEAD, non-recursively, and ticks only when HEAD itself
// changes (branch switch, detached commit, rebase) — a build in the
// worktree ticks nothing (#595). We refresh once on mount and again each
// time the room becomes visible, which also covers a folder that became
// a repo after the watch started (nothing was watched then) and any
// missed tick, without restarting the watch.

import { Channel, invoke } from "@tauri-apps/api/core";
import { useEffect, useRef } from "react";

export function useGitBranchWatcher(
	cwd: string,
	visible: boolean,
	onBranchChange?: (branch: string | null) => void,
) {
	const cbRef = useRef(onBranchChange);
	cbRef.current = onBranchChange;
	const refreshRef = useRef<(() => void) | null>(null);

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
		refreshRef.current = () => void refreshBranch();

		void refreshBranch();

		const channel = new Channel<null>();
		channel.onmessage = () => {
			void refreshBranch();
		};
		let watchId: string | null = null;
		invoke<string | null>("git_head_watch_start", { path: cwd, onChange: channel })
			.then((id) => {
				if (id === null) return;
				if (cancelled) {
					void invoke("git_watch_stop", { id });
					return;
				}
				watchId = id;
			})
			.catch((err: unknown) => {
				const msg = err instanceof Error ? err.message : String(err);
				console.error("[skein] git_head_watch_start failed:", msg);
			});

		return () => {
			cancelled = true;
			refreshRef.current = null;
			if (watchId) void invoke("git_watch_stop", { id: watchId });
		};
	}, [cwd]);

	// Re-sync whenever the room becomes visible. The mount refresh is the
	// effect above; skip the first run here so it is not doubled.
	const first = useRef(true);
	useEffect(() => {
		if (first.current) {
			first.current = false;
			return;
		}
		if (visible) refreshRef.current?.();
	}, [visible]);
}
