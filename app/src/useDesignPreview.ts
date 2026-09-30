// useDesignPreview — the preview server side of a `design` harness (#433):
// the preview base URL, the entry list, and the watcher whose ticks bump
// `version` so the frame reloads.

import { Channel, invoke } from "@tauri-apps/api/core";
import { useCallback, useEffect, useState } from "react";
import { retryDelay } from "./designPreview.ts";

/** Run `fn`, retrying with backoff while `isCancelled()` is false. A new
 *  room's db row lands only after the pane's first invoke, so the first
 *  tries can fail with "unknown room". Rejects with the last error once
 *  the budget is spent; resolves/rejects only if not cancelled is the
 *  caller's concern (it checks its own flag). */
const invokeWithRetry = async <T>(fn: () => Promise<T>, isCancelled: () => boolean): Promise<T> => {
	for (let failures = 1; ; failures++) {
		try {
			return await fn();
		} catch (e) {
			const delay = retryDelay(failures);
			if (delay === null || isCancelled()) throw e;
			await new Promise((r) => window.setTimeout(r, delay));
			if (isCancelled()) throw e;
		}
	}
};

export const useDesignPreview = (roomId: string) => {
	const [base, setBase] = useState<string | null>(null);
	const [entries, setEntries] = useState<string[] | null>(null);
	const [error, setError] = useState<string | null>(null);
	const [attempt, setAttempt] = useState(0);
	const [version, setVersion] = useState(0);

	const refreshEntries = useCallback(async () => {
		try {
			setEntries(await invoke<string[]>("design_list_entries", { roomId }));
		} catch (e) {
			setError(String(e));
		}
	}, [roomId]);

	// Preview base + entry list; `attempt` is the inline retry.
	// biome-ignore lint/correctness/useExhaustiveDependencies: attempt is the retry trigger
	useEffect(() => {
		let cancelled = false;
		const isCancelled = () => cancelled;
		setError(null);
		invokeWithRetry(() => invoke<string>("design_preview_base", { roomId }), isCancelled)
			.then((b) => {
				if (!cancelled) setBase(b);
			})
			.catch((e) => {
				if (!cancelled) setError(String(e));
			});
		invokeWithRetry(() => invoke<string[]>("design_list_entries", { roomId }), isCancelled)
			.then((list) => {
				if (!cancelled) setEntries(list);
			})
			.catch((e) => {
				if (!cancelled) setError(String(e));
			});
		return () => {
			cancelled = true;
		};
	}, [roomId, attempt]);

	// The watcher runs only while mounted; a tick reloads the frame.
	// biome-ignore lint/correctness/useExhaustiveDependencies: attempt is the retry trigger
	useEffect(() => {
		const channel = new Channel<null>();
		channel.onmessage = () => {
			setVersion((n) => n + 1);
			void refreshEntries();
		};
		let watchId: string | null = null;
		let cancelled = false;
		invokeWithRetry(
			() => invoke<string>("design_watch_start", { roomId, onChange: channel }),
			() => cancelled,
		)
			.then((id) => {
				if (cancelled) {
					void invoke("git_watch_stop", { id });
					return;
				}
				watchId = id;
			})
			.catch((e) => {
				if (!cancelled) setError(String(e));
			});
		return () => {
			cancelled = true;
			if (watchId !== null) void invoke("git_watch_stop", { id: watchId });
		};
	}, [roomId, attempt, refreshEntries]);

	return {
		base,
		entries,
		error,
		version,
		retry: () => setAttempt((n) => n + 1),
		reload: () => setVersion((n) => n + 1),
	};
};
