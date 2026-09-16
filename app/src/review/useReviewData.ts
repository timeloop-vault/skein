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
import { createCoalescer, sameJson } from "./coalesce.ts";
import { type SignoffStatus, fetchSignoff, setSignoff } from "./signoff.ts";

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

/// Start review discovery (#221) for `cwd`: a filesystem watcher that
/// baselines whatever changed — a shell write, a hand edit, the `files`
/// harness — not only what a harness `patch` row named. Modelled on
/// `useWorktreeWatcher`, but owns its own backend watcher rather than
/// piggybacking on the review pane's: discovery has to run for every
/// active room, not only the one whose Review tab is currently open, so
/// a file someone edited while looking at Live Context is still
/// pending by the time they switch tabs.
export function useReviewDiscovery(roomId: string, cwd: string | undefined) {
	useEffect(() => {
		if (!roomId || !cwd) return;
		let cancelled = false;
		let watchId: string | null = null;
		invoke<string>("review_discovery_start", { roomId, cwd })
			.then((id) => {
				if (cancelled) {
					void invoke("git_watch_stop", { id });
					return;
				}
				watchId = id;
			})
			.catch((err: unknown) => {
				const msg = err instanceof Error ? err.message : String(err);
				console.error("[skein] review_discovery_start failed:", msg);
			});
		return () => {
			cancelled = true;
			if (watchId) void invoke("git_watch_stop", { id: watchId });
		};
	}, [roomId, cwd]);
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

		// Single-flight with trailing rerun (#171): a key change (scope,
		// commit, room/cwd) gets its own coalescer, disposed on cleanup so
		// a fetch still in flight for the OLD key never lands here. Within
		// one key, `refresh()` — driven by watcher ticks and agent writes,
		// which arrive far faster than the backend's own worktree diff —
		// collapses into at most one trailing run per in-flight fetch.
		const coalescer = createCoalescer(
			() => fetchScope(roomId, cwd, scope, commitSha),
			(result) => {
				if (cancelled) return;
				if (result.ok) {
					// The backend's own `error` field (an unresolvable base
					// ref, say) is part of the data, not a failed call — it
					// is rendered in the header beside a working file list.
					setData((prev) => (sameJson(prev, result.value) ? prev : result.value));
					setError(undefined);
				} else {
					setError(message(result.error));
				}
				setLoading(false);
			},
		);
		runRef.current = () => coalescer.request();
		coalescer.request();
		return () => {
			cancelled = true;
			coalescer.dispose();
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
	const runRef = useRef<() => void>(() => {});

	// The "real" inputs: a change here has to fetch promptly and win over
	// whatever the old key's coalescer still has in flight, so it gets a
	// fresh coalescer rather than sharing one across keys.
	useEffect(() => {
		if (!enabled || !path || !roomId || !cwd) {
			setFile(undefined);
			runRef.current = () => {};
			return;
		}
		let cancelled = false;
		setLoading(true);

		const coalescer = createCoalescer(
			() => fetchFile(roomId, cwd, path, scope, commitSha),
			(result) => {
				if (cancelled) return;
				if (result.ok) {
					setFile((prev) => (sameJson(prev, result.value) ? prev : result.value));
					setError(undefined);
				} else {
					setError(message(result.error));
				}
				setLoading(false);
			},
		);
		runRef.current = () => coalescer.request();
		coalescer.request();
		return () => {
			cancelled = true;
			coalescer.dispose();
			runRef.current = () => {};
		};
	}, [roomId, cwd, path, scope, commitSha, enabled]);

	// `nonce` is a refresh signal, not a key: writing a comment changes
	// only sqlite, so nothing above moves and nothing would re-anchor
	// without it. Routed through the same coalescer as the effect above
	// (via `runRef`) rather than its own fetch, so a burst of comment
	// writes collapses the same way a burst of watcher ticks does.
	const firstNonce = useRef(true);
	// biome-ignore lint/correctness/useExhaustiveDependencies: nonce is the deliberate trigger here; runRef is a ref and reading .current is not a reactive dependency
	useEffect(() => {
		if (firstNonce.current) {
			firstNonce.current = false;
			return;
		}
		runRef.current();
	}, [nonce]);

	return { file, error, loading };
}

/// The room's sign-off (#214), and the two ways to change it.
///
/// A hook rather than state inside the control, because two places
/// render it: the header button and the lapsed-sign-off notice in the
/// body. A lapsed sign-off is the one state the reviewer has to
/// notice, and a tooltip on a button is not noticing.
///
/// `nonce` is the whole staleness mechanism from this side: a commit
/// is what makes a sign-off lapse, and the pane already bumps the
/// nonce on every refresh, so the control cannot go on claiming
/// clearance after the agent has moved HEAD.
export function useSignoff(
	roomId: string,
	cwd: string,
	enabled: boolean,
	nonce: number,
): {
	status: SignoffStatus | undefined;
	error: string | undefined;
	busy: boolean;
	set: (approved: boolean, note?: string) => void;
} {
	const [status, setStatus] = useState<SignoffStatus | undefined>(undefined);
	const [error, setError] = useState<string | undefined>(undefined);
	const [busy, setBusy] = useState(false);

	// biome-ignore lint/correctness/useExhaustiveDependencies: nonce is the trigger — a commit is what makes a sign-off lapse, and nothing else in this dependency list moves when HEAD does
	useEffect(() => {
		if (!enabled || !roomId || !cwd) return;
		let cancelled = false;
		fetchSignoff(roomId, cwd)
			.then((next) => {
				if (cancelled) return;
				setStatus(next);
				setError(undefined);
			})
			.catch((err: unknown) => {
				if (!cancelled) setError(message(err));
			});
		return () => {
			cancelled = true;
		};
	}, [roomId, cwd, enabled, nonce]);

	const set = useCallback(
		(approved: boolean, note?: string) => {
			setBusy(true);
			setError(undefined);
			setSignoff(roomId, cwd, approved, note)
				.then(setStatus)
				.catch((err: unknown) => setError(message(err)))
				.finally(() => setBusy(false));
		},
		[roomId, cwd],
	);

	return { status, error, busy, set };
}
