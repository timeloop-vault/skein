// Live worktree diff for the Diff card — issue #80 D3.
//
// Mirrors useGitBranchWatcher: fetch `git_diff` once on mount, then
// re-fetch on every debounced WatcherManager tick (git_watch_start), so
// the diff body tracks file changes within the watcher's ~200ms window.
// Non-git folders / errors resolve to an empty list — the Diff card then
// leans entirely on the harness-reported patches (diff.ts fallback).

import { Channel, invoke } from "@tauri-apps/api/core";
import { useEffect, useState } from "react";
import type { FileDiff } from "./diff.ts";

export function useWorktreeDiff(cwd: string, enabled: boolean): FileDiff[] {
	const [files, setFiles] = useState<FileDiff[]>([]);

	useEffect(() => {
		// Only the visible room watches: every room's card stays mounted,
		// so without this an FS change would fan out to N git_diff calls.
		// A hidden room re-fetches fresh the moment it becomes visible.
		if (!enabled) return;
		let cancelled = false;
		setFiles([]);

		const refresh = async () => {
			try {
				const next = await invoke<FileDiff[]>("git_diff", { path: cwd });
				if (!cancelled) setFiles(next);
			} catch {
				// Non-repo or transient git error — the card falls back to
				// harness patches, so an empty worktree diff is fine.
				if (!cancelled) setFiles([]);
			}
		};

		void refresh();

		const channel = new Channel<null>();
		channel.onmessage = () => {
			void refresh();
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
			.catch(() => {
				// A folder we can't watch still renders from harness patches.
			});

		return () => {
			cancelled = true;
			if (watchId) void invoke("git_watch_stop", { id: watchId });
		};
	}, [cwd, enabled]);

	return files;
}
