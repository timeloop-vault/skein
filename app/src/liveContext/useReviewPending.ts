// The room's pending review, refreshed on every worktree tick (#211).
//
// Replaces useWorktreeDiff for the Diff card. Same watcher shape — fetch
// once on mount, re-fetch on each debounced WatcherManager tick — with
// one addition that matters: `refresh()`. Accepting does not write to
// disk, so it produces no filesystem event and nothing would re-fetch;
// the card calls refresh itself after every accept and reject.
//
// Errors are surfaced rather than swallowed (#176). A review pane that
// silently shows nothing is indistinguishable from one that has nothing
// to show, and those mean opposite things here.

import { Channel, invoke } from "@tauri-apps/api/core";
import { useCallback, useEffect, useRef, useState } from "react";
import { type PendingFile, fetchPending } from "./review.ts";

export function useReviewPending(
	roomId: string,
	cwd: string,
	enabled: boolean,
): { files: PendingFile[]; error: string | undefined; refresh: () => void } {
	const [files, setFiles] = useState<PendingFile[]>([]);
	const [error, setError] = useState<string | undefined>(undefined);
	// Bumped by refresh(); read by the effect's closure so a manual
	// refresh and a watcher tick go through exactly the same path.
	const refreshRef = useRef<() => void>(() => {});

	const refresh = useCallback(() => {
		refreshRef.current();
	}, []);

	useEffect(() => {
		// Only the visible room polls: every room's card stays mounted,
		// so without this one filesystem change would fan out to N
		// review_pending calls. A hidden room re-fetches the moment it
		// becomes visible.
		if (!enabled || !roomId || !cwd) return;
		let cancelled = false;
		setFiles([]);
		setError(undefined);

		const run = () => {
			fetchPending(roomId, cwd)
				.then((next) => {
					if (cancelled) return;
					setFiles(next);
					setError(undefined);
				})
				.catch((err: unknown) => {
					if (cancelled) return;
					const msg = err instanceof Error ? err.message : String(err);
					console.warn(`[skein] review_pending failed for ${roomId}:`, msg);
					setError(msg);
				});
		};
		refreshRef.current = run;
		run();

		const channel = new Channel<null>();
		channel.onmessage = run;
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
				// A folder we can't watch still renders; it just won't
				// live-update until the next accept/reject refresh.
			});

		return () => {
			cancelled = true;
			refreshRef.current = () => {};
			if (watchId) void invoke("git_watch_stop", { id: watchId });
		};
	}, [roomId, cwd, enabled]);

	return { files, error, refresh };
}
