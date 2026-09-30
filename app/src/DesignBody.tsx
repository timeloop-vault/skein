// DesignBody — the body of a `design` harness (#433): a live preview
// of an HTML file from the room's worktree, served by the Rust preview
// server, in a sandboxed iframe. The page is arbitrary worktree code:
// the frame gets `allow-scripts` and NEVER `allow-same-origin`, and its
// `postMessage` beacons are untrusted (see designPreview.ts).

import { Channel, invoke } from "@tauri-apps/api/core";
import { emit, listen } from "@tauri-apps/api/event";
import { useCallback, useEffect, useRef, useState } from "react";
import {
	type ElementThread,
	buildLocateAnchors,
	buildPins,
	displayState,
	elementSummary,
	pinNumbers,
	placeThreads,
	placementSignature,
	seenWrites,
	sourceLabel,
} from "./designComments.ts";
import { subscribeDesignFocus, takeDesignFocus } from "./designFocus.ts";
import {
	type Beacon,
	type HostMessage,
	hostMessage,
	parseBeacon,
	previewUrl,
	pushBeacon,
	retryDelay,
} from "./designPreview.ts";
import type { ElementAnchor, ElementDescriptor, Placement } from "./elementAnchor.ts";
import { Composer, ThreadView } from "./review/Thread.tsx";
import {
	addThread,
	deleteComment,
	deleteThread,
	editComment,
	fetchElementThreads,
	replyToThread,
	reportElementSeen,
	resolveThread,
} from "./review/api.ts";

/** Run `fn`, retrying with backoff while `isCancelled()` is false. A new
 *  room's db row lands only after the pane's first invoke, so the first
 *  tries can fail with "unknown room". Rejects with the last error once
 *  the budget is spent; resolves/rejects only if not cancelled is the
 *  caller's concern (it checks its own flag). */
const invokeWithRetry = async <T,>(
	fn: () => Promise<T>,
	isCancelled: () => boolean,
): Promise<T> => {
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

/** How long after the iframe's `load` we wait for the `ready` beacon
 *  before saying Skein's preview script did not run. */
const READY_TIMEOUT_MS = 3000;

interface DesignBodyProps {
	harnessId: string;
	roomId: string;
	cwd: string;
	visible: boolean;
	entry: string | undefined;
	onEntryChange: (entry: string) => void;
}

/** One thread in the side list. Every string here came from the page or
 *  the comment author and is rendered as text. */
const ElementThreadItem = ({
	n,
	thread,
	state,
	selected,
	busy,
	onSelect,
	onReply,
	onResolve,
	onDelete,
	onEditComment,
	onDeleteComment,
}: {
	n: number;
	thread: ElementThread;
	state: string;
	selected: boolean;
	busy: boolean;
	onSelect: () => void;
	onReply: (body: string) => void;
	onResolve: (resolved: boolean) => void;
	onDelete: () => void;
	onEditComment: (commentId: string, body: string) => void;
	onDeleteComment: (commentId: string) => void;
}) => {
	const a = thread.element?.anchor;
	const src = a ? sourceLabel(a) : undefined;
	return (
		<div className={`dp-comment-item${selected ? " selected" : ""}`} data-state={state}>
			<button type="button" className="dp-comment-head" onClick={onSelect}>
				<span className="dp-comment-n">{n}</span>
				<span className={`dp-comment-state ${state}`}>{state}</span>
				{a && <span className="dp-comment-el">{elementSummary(a)}</span>}
			</button>
			{state === "lost" && a && (
				<div className="dp-comment-lost">
					element not found
					{src !== undefined && <span className="dp-comment-src"> · was at {src}</span>}
				</div>
			)}
			{state !== "lost" && src !== undefined && <div className="dp-comment-src">{src}</div>}
			<ThreadView
				thread={thread}
				busy={busy}
				onReply={onReply}
				onResolve={onResolve}
				onDelete={onDelete}
				onEditComment={onEditComment}
				onDeleteComment={onDeleteComment}
			/>
		</div>
	);
};

export const DesignBody = ({
	harnessId,
	roomId,
	cwd,
	visible,
	entry,
	onEntryChange,
}: DesignBodyProps) => {
	const [base, setBase] = useState<string | null>(null);
	const [entries, setEntries] = useState<string[] | null>(null);
	const [error, setError] = useState<string | null>(null);
	const [attempt, setAttempt] = useState(0);
	const [version, setVersion] = useState(0);
	const [beacons, setBeacons] = useState<Beacon[]>([]);
	const [ready, setReady] = useState(false);
	const [noReady, setNoReady] = useState(false);
	const frameRef = useRef<HTMLIFrameElement | null>(null);
	const readyRef = useRef(false);
	const timerRef = useRef<number | null>(null);

	// Element comments (#434).
	const [picking, setPicking] = useState(false);
	const [draft, setDraft] = useState<ElementDescriptor | null>(null);
	const [threads, setThreads] = useState<ElementThread[]>([]);
	const [placements, setPlacements] = useState<Map<string, Placement>>(new Map());
	const [selected, setSelected] = useState<{ id: string; tick: number } | null>(null);
	const [busy, setBusy] = useState(false);
	const [commentError, setCommentError] = useState<string | null>(null);
	const [readyCount, setReadyCount] = useState(0);
	const threadsRef = useRef<ElementThread[]>([]);
	threadsRef.current = threads;
	const requestRef = useRef(0);
	const writtenRef = useRef<Set<string>>(new Set());

	const post = useCallback((msg: HostMessage) => {
		frameRef.current?.contentWindow?.postMessage(hostMessage(msg), "*");
	}, []);

	const fetchThreads = useCallback(async () => {
		if (entry === undefined) {
			setThreads([]);
			return;
		}
		try {
			setThreads(await fetchElementThreads(roomId, entry));
		} catch (e) {
			setCommentError(String(e));
		}
	}, [roomId, entry]);

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

	// One entry and none chosen: pick it and persist.
	useEffect(() => {
		if (entry === undefined && entries?.length === 1 && entries[0] !== undefined) {
			onEntryChange(entries[0]);
		}
	}, [entry, entries, onEntryChange]);

	const url = base !== null && entry !== undefined ? previewUrl(base, entry, version) : null;

	// A new load: forget the last one's beacons and readiness.
	// biome-ignore lint/correctness/useExhaustiveDependencies: url is the trigger
	useEffect(() => {
		setBeacons([]);
		setReady(false);
		setNoReady(false);
		readyRef.current = false;
		setPicking(false);
		setPlacements(new Map());
		if (timerRef.current !== null) window.clearTimeout(timerRef.current);
		timerRef.current = null;
	}, [url]);

	// Threads for this entry: on mount / entry change, and whenever the
	// review changes (an agent reply, the review pane, our own writes).
	useEffect(() => {
		void fetchThreads();
	}, [fetchThreads]);
	useEffect(() => {
		let cancelled = false;
		const unlisten = listen<{ roomId: string }>("skein://review-changed", (event) => {
			if (event.payload.roomId === roomId) void fetchThreads();
		});
		return () => {
			cancelled = true;
			void unlisten.then((off) => {
				if (cancelled) off();
			});
		};
	}, [roomId, fetchThreads]);

	// Placement: ask the page where the open threads' elements are — on
	// every load (`ready`) and when the set of threads to locate changes.
	const signature = placementSignature(threads);
	// biome-ignore lint/correctness/useExhaustiveDependencies: readyCount and signature are the triggers; threads are read through a ref
	useEffect(() => {
		if (readyCount === 0) return;
		writtenRef.current = new Set();
		const anchors = buildLocateAnchors(threadsRef.current);
		requestRef.current += 1;
		if (anchors.length === 0) {
			setPlacements(new Map());
			post({ type: "pins", pins: [] });
			return;
		}
		post({ type: "locate", requestId: String(requestRef.current), anchors });
	}, [readyCount, signature, post]);

	// Pins follow the side list's numbering and the latest placements.
	useEffect(() => {
		if (readyCount === 0) return;
		post({ type: "pins", pins: buildPins(threads, placements) });
	}, [threads, placements, readyCount, post]);

	// Highlight the selected thread's pin once it has one.
	const selectedId = selected?.id;
	const selectedTick = selected?.tick;
	// biome-ignore lint/correctness/useExhaustiveDependencies: tick re-fires a repeat click; threads/placements settle a pending focus
	useEffect(() => {
		if (selectedId === undefined || readyCount === 0) return;
		const n = pinNumbers(threads).get(selectedId);
		if (n !== undefined && placements.has(selectedId)) post({ type: "highlight", n });
	}, [selectedId, selectedTick, threads, placements, readyCount, post]);

	// Focus requests (#434): a thread opened from elsewhere.
	const focusThread = useCallback(
		(f: { entry: string; threadId: string }) => {
			if (f.entry !== entry) onEntryChange(f.entry);
			setSelected((s) => ({ id: f.threadId, tick: (s?.tick ?? 0) + 1 }));
		},
		[entry, onEntryChange],
	);
	useEffect(() => subscribeDesignFocus(harnessId, focusThread), [harnessId, focusThread]);
	useEffect(() => {
		if (!visible) return;
		const f = takeDesignFocus(harnessId);
		if (f) focusThread(f);
	}, [visible, harnessId, focusThread]);

	// Escape leaves pick mode even when the host, not the frame, has focus.
	useEffect(() => {
		if (!picking) return;
		const onKey = (e: KeyboardEvent) => {
			if (e.key !== "Escape") return;
			setPicking(false);
			post({ type: "pick-cancel" });
		};
		window.addEventListener("keydown", onKey);
		return () => window.removeEventListener("keydown", onKey);
	}, [picking, post]);

	const togglePick = () => {
		if (picking) {
			setPicking(false);
			post({ type: "pick-cancel" });
		} else {
			setPicking(true);
			post({ type: "pick-start" });
		}
	};

	// The review pane refreshes on the backend's event; a write from this
	// pane touches only sqlite, so send the same event (same payload).
	const wrote = async () => {
		await fetchThreads();
		await emit("skein://review-changed", { roomId });
	};

	const act = async (fn: () => Promise<unknown>) => {
		setBusy(true);
		setCommentError(null);
		try {
			await fn();
			await wrote();
		} catch (e) {
			setCommentError(String(e));
		} finally {
			setBusy(false);
		}
	};

	const submitDraft = (body: string) => {
		if (!draft || entry === undefined) return;
		const element: ElementAnchor = { ...draft, entry };
		void act(async () => {
			await addThread(roomId, cwd, {
				scope: "element",
				filePath: entry,
				anchorLines: [],
				body,
				element,
			});
			setDraft(null);
		});
	};

	useEffect(
		() => () => {
			if (timerRef.current !== null) window.clearTimeout(timerRef.current);
		},
		[],
	);

	const onLocatedRef = useRef<(b: Extract<Beacon, { type: "located" }>) => void>(() => {});
	onLocatedRef.current = (b) => {
		if (b.requestId !== String(requestRef.current)) return;
		const current = threadsRef.current;
		const next = placeThreads(current, b.results);
		setPlacements(next);
		const writes = seenWrites(current, next, b.files, writtenRef.current);
		if (writes.length === 0) return;
		for (const w of writes) writtenRef.current.add(w.threadId);
		void Promise.all(writes.map((w) => reportElementSeen(roomId, w.threadId, w.seen)))
			.catch((e) => setCommentError(String(e)))
			.then(() => fetchThreads());
	};

	// Beacons from the frame: only its own window is believed.
	useEffect(() => {
		const onMessage = (e: MessageEvent) => {
			const frame = frameRef.current;
			if (!frame || e.source !== frame.contentWindow) return;
			const b = parseBeacon(e.data);
			if (!b) return;
			if (b.type === "ready") {
				readyRef.current = true;
				setReady(true);
				setNoReady(false);
				setReadyCount((n) => n + 1);
			} else if (b.type === "picked") {
				setPicking(false);
				setDraft(b.element);
			} else if (b.type === "pick-cancelled") {
				setPicking(false);
			} else if (b.type === "located") {
				onLocatedRef.current(b);
			} else {
				setBeacons((prev) => pushBeacon(prev, b));
			}
		};
		window.addEventListener("message", onMessage);
		return () => window.removeEventListener("message", onMessage);
	}, []);

	const onFrameLoad = () => {
		if (readyRef.current) return;
		if (timerRef.current !== null) window.clearTimeout(timerRef.current);
		timerRef.current = window.setTimeout(() => {
			if (!readyRef.current) setNoReady(true);
		}, READY_TIMEOUT_MS);
	};

	let status = error !== null ? "" : "loading…";
	if (entries !== null && entries.length === 0) status = "no HTML files";
	else if (url !== null) status = ready ? `live · ${entry}` : (entry ?? "");

	return (
		<div
			className="dp-body"
			data-harness={harnessId}
			data-visible={visible}
			style={{ display: "flex", flexDirection: "column", flex: 1, minHeight: 0 }}
		>
			<div className="dp-toolbar">
				<select
					value={entry ?? ""}
					disabled={!entries || entries.length === 0}
					onChange={(e) => onEntryChange(e.target.value)}
				>
					{entry === undefined && <option value="">choose an entry…</option>}
					{entry !== undefined && !entries?.includes(entry) && (
						<option value={entry}>{entry}</option>
					)}
					{entries?.map((p) => (
						<option key={p} value={p}>
							{p}
						</option>
					))}
				</select>
				<button type="button" title="Reload" onClick={() => setVersion((n) => n + 1)}>
					↻
				</button>
				<button
					type="button"
					className={`dp-comment-toggle${picking ? " on" : ""}`}
					title="Pick an element to comment on (Esc cancels)"
					aria-pressed={picking}
					disabled={!ready || entry === undefined}
					onClick={togglePick}
				>
					Comment
				</button>
				<span className="dp-status">{status}</span>
			</div>
			{error !== null && (
				<div className="fp-notice warn" role="alert">
					{error}{" "}
					<button type="button" onClick={() => setAttempt((n) => n + 1)}>
						Retry
					</button>
				</div>
			)}
			{noReady && (
				<div className="fp-notice warn">
					Skein's preview script did not run — a Content-Security-Policy on the page or a script
					error can cause this. Error reporting is unavailable.
				</div>
			)}
			{entries !== null && entries.length === 0 && error === null ? (
				<div className="fp-empty">
					<div className="glyph">◐</div>
					<div className="title">No HTML files in this worktree</div>
					<div className="hint">Add an .html file and it will show up here.</div>
				</div>
			) : url !== null ? (
				<div className="dp-main">
					<iframe
						ref={frameRef}
						className="dp-frame"
						title="Design preview"
						sandbox="allow-scripts"
						src={url}
						onLoad={onFrameLoad}
					/>
					{(draft !== null || threads.length > 0 || commentError !== null) && (
						<div className="dp-comments">
							{commentError !== null && (
								<div className="fp-notice warn" role="alert">
									{commentError}
								</div>
							)}
							{draft !== null && (
								<div className="dp-comment-draft">
									<div className="dp-comment-el">
										{elementSummary({ ...draft, entry: entry ?? "" })}
									</div>
									{sourceLabel(draft) !== undefined && (
										<div className="dp-comment-src">{sourceLabel(draft)}</div>
									)}
									<Composer
										placeholder="Comment on this element…"
										busy={busy}
										submitLabel="Comment"
										autoFocus
										onSubmit={submitDraft}
										onCancel={() => setDraft(null)}
									/>
								</div>
							)}
							{threads.map((t, i) => (
								<ElementThreadItem
									key={t.id}
									n={i + 1}
									thread={t}
									state={displayState(t, placements)}
									selected={selected?.id === t.id}
									busy={busy}
									onSelect={() => setSelected((s) => ({ id: t.id, tick: (s?.tick ?? 0) + 1 }))}
									onReply={(body) => void act(() => replyToThread(roomId, t.id, body))}
									onResolve={(r) => void act(() => resolveThread(t.id, r))}
									onDelete={() => void act(() => deleteThread(t.id))}
									onEditComment={(id, body) => void act(() => editComment(id, body))}
									onDeleteComment={(id) => void act(() => deleteComment(id))}
								/>
							))}
						</div>
					)}
				</div>
			) : (
				<div className="fp-empty" />
			)}
			{beacons.length > 0 && (
				<ul className="dp-beacons">
					<li>{beacons.length} problem(s) in this load</li>
					{beacons.map((b, i) => (
						<li key={i}>
							{b.type === "resource-error"
								? `${b.tag} failed to load: ${b.url}`
								: b.type === "script-error"
									? `${b.message}${b.url ? ` (${b.url}${b.line !== undefined ? `:${b.line}` : ""})` : ""}`
									: ""}
						</li>
					))}
				</ul>
			)}
		</div>
	);
};
