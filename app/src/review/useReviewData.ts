// The review pane's data plumbing (#212).
//
// Generalises `useReviewPending` (#211) to the whole review surface: the
// same watcher shape — fetch on mount, re-fetch on each debounced
// WatcherManager tick — over the scope summary and the open file.
//
// `refresh()` matters for the same reason it did in #211, and more so
// here. Accepting writes nothing to disk, and writing a comment touches
// only sqlite; neither produces a filesystem event, so nothing would
// re-fetch. Every mutating action calls refresh itself.
//
// Errors surface rather than being swallowed (#176): a review pane that
// silently shows nothing is indistinguishable from a branch with no
// work on it, and those mean opposite things.

import { Channel, invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { useCallback, useEffect, useRef, useState } from "react";
import {
	type ReviewFileDetail,
	type ReviewScope,
	type ReviewScopeData,
	fetchFile,
	fetchScope,
} from "./api.ts";

const message = (err: unknown): string => (err instanceof Error ? err.message : String(err));

/// Watch `cwd` and call `onTick` on every debounced change.
///
/// Owned by the pane rather than by either data hook, because a tick
/// has to refresh both at once: the file list and the open file's diff
/// disagreeing about what is on disk is exactly the confusion the
/// review is meant to remove.
///
/// Only ever started for the visible room: every room's right pane
/// stays mounted, so watching them all would fan one filesystem change
/// out into N review_scope calls.
export function useWorktreeWatcher(cwd: string, enabled: boolean, onTick: () => void) {
	const cb = useRef(onTick);
	cb.current = onTick;

	useEffect(() => {
		if (!enabled || !cwd) return;
		let cancelled = false;
		let watchId: string | null = null;
		const channel = new Channel<null>();
		channel.onmessage = () => cb.current();
		invoke<string>("git_watch_start", { path: cwd, onChange: channel })
			.then((id) => {
				if (cancelled) {
					void invoke("git_watch_stop", { id });
					return;
				}
				watchId = id;
			})
			.catch(() => {
				// A folder we cannot watch still renders; it just will not
				// live-update until the next explicit refresh.
			});
		return () => {
			cancelled = true;
			if (watchId) void invoke("git_watch_stop", { id: watchId });
		};
	}, [cwd, enabled]);
}

/// Re-fetch when an agent writes to this room's review (#213).
///
/// The sibling of `useWorktreeWatcher`, and needed for the same reason
/// `refresh()` is: an agent replying through the MCP endpoint touches
/// sqlite and nothing else, so no filesystem event fires and the pane
/// would sit there quietly out of date while the user watched it.
export function useAgentWrites(roomId: string, enabled: boolean, onChange: () => void) {
	const cb = useRef(onChange);
	cb.current = onChange;

	useEffect(() => {
		if (!enabled || !roomId) return;
		let cancelled = false;
		const unlisten = listen<{ roomId: string }>("skein://review-changed", (event) => {
			if (event.payload.roomId === roomId) cb.current();
		});
		return () => {
			cancelled = true;
			void unlisten.then((off) => {
				if (cancelled) off();
			});
		};
	}, [roomId, enabled]);
}

export function useReviewScope(
	roomId: string,
	cwd: string,
	scope: ReviewScope,
	commitSha: string | undefined,
	enabled: boolean,
): {
	data: ReviewScopeData | undefined;
	error: string | undefined;
	loading: boolean;
	refresh: () => void;
	setData: (next: ReviewScopeData) => void;
} {
	const [data, setData] = useState<ReviewScopeData | undefined>(undefined);
	const [error, setError] = useState<string | undefined>(undefined);
	const [loading, setLoading] = useState(false);
	const runRef = useRef<() => void>(() => {});

	const refresh = useCallback(() => runRef.current(), []);

	useEffect(() => {
		if (!enabled || !roomId || !cwd) return;
		let cancelled = false;
		setLoading(true);

		const run = () => {
			fetchScope(roomId, cwd, scope, commitSha)
				.then((next) => {
					if (cancelled) return;
					setData(next);
					// The backend's own `error` field (an unresolvable base
					// ref, say) is part of the data, not a failed call — it
					// is rendered in the header beside a working file list.
					setError(undefined);
				})
				.catch((err: unknown) => {
					if (cancelled) return;
					setError(message(err));
				})
				.finally(() => {
					if (!cancelled) setLoading(false);
				});
		};
		runRef.current = run;
		run();
		return () => {
			cancelled = true;
			runRef.current = () => {};
		};
	}, [roomId, cwd, scope, commitSha, enabled]);

	return { data, error, loading, refresh, setData };
}

export function useReviewFile(
	roomId: string,
	cwd: string,
	path: string | undefined,
	scope: ReviewScope,
	commitSha: string | undefined,
	enabled: boolean,
	/** Bumped by the pane after any mutation, to re-pull threads. */
	nonce: number,
): { file: ReviewFileDetail | undefined; error: string | undefined; loading: boolean } {
	const [file, setFile] = useState<ReviewFileDetail | undefined>(undefined);
	const [error, setError] = useState<string | undefined>(undefined);
	const [loading, setLoading] = useState(false);

	// biome-ignore lint/correctness/useExhaustiveDependencies: nonce is a deliberate trigger — writing a comment changes only sqlite, so no other dependency moves and nothing would re-anchor
	useEffect(() => {
		if (!enabled || !path || !roomId || !cwd) {
			setFile(undefined);
			return;
		}
		let cancelled = false;
		setLoading(true);
		fetchFile(roomId, cwd, path, scope, commitSha)
			.then((next) => {
				if (cancelled) return;
				setFile(next);
				setError(undefined);
			})
			.catch((err: unknown) => {
				if (cancelled) return;
				setError(message(err));
			})
			.finally(() => {
				if (!cancelled) setLoading(false);
			});
		return () => {
			cancelled = true;
		};
	}, [roomId, cwd, path, scope, commitSha, enabled, nonce]);

	return { file, error, loading };
}
