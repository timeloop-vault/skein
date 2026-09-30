// useDesignComments — element comments on a design pane (#434): the
// entry's threads, where the page says their elements are (locate, with
// re-asks and backoff), the pins and highlight pushed back into the page,
// and the writes. The frame itself stays with DesignBody; this hook only
// talks to it through `post`.

import { listen } from "@tauri-apps/api/event";
import { useCallback, useEffect, useRef, useState } from "react";
import {
	type ElementThread,
	type WrittenSeen,
	buildLocateAnchors,
	buildPins,
	pinNumbers,
	placeThreads,
	placementSignature,
	seenWrites,
	unplaced,
} from "./designComments.ts";
import { subscribeDesignFocus, takeDesignFocus } from "./designFocus.ts";
import type { Beacon, HostMessage } from "./designPreview.ts";
import type { ElementAnchor, ElementDescriptor, Placement } from "./elementAnchor.ts";
import { addThread, fetchElementThreads, reportElementSeen } from "./review/api.ts";

/** Re-asks for an unanswered thread: 400 ms doubling, 5 tries. */
const LOCATE_RETRY_MS = 400;
const LOCATE_RETRIES = 5;

type Located = Extract<Beacon, { type: "located" }>;

export const useDesignComments = ({
	harnessId,
	roomId,
	cwd,
	visible,
	entry,
	onEntryChange,
	post,
	ready,
	readyCount,
}: {
	harnessId: string;
	roomId: string;
	cwd: string;
	visible: boolean;
	entry: string | undefined;
	onEntryChange: (entry: string) => void;
	post: (msg: HostMessage) => void;
	ready: boolean;
	readyCount: number;
}) => {
	const [draft, setDraft] = useState<ElementDescriptor | null>(null);
	const [threads, setThreads] = useState<ElementThread[]>([]);
	const [placements, setPlacements] = useState<Map<string, Placement>>(new Map());
	const [selected, setSelected] = useState<{ id: string; tick: number } | null>(null);
	const [busy, setBusy] = useState(false);
	const [commentError, setCommentError] = useState<string | null>(null);
	const threadsRef = useRef<ElementThread[]>([]);
	threadsRef.current = threads;
	const requestRef = useRef(0);
	const locateNowRef = useRef<() => void>(() => {});
	const writtenRef = useRef<Map<string, WrittenSeen>>(new Map());
	const readyCountRef = useRef(0);
	readyCountRef.current = readyCount;

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
	const locateNow = useCallback(() => {
		const anchors = buildLocateAnchors(threadsRef.current);
		requestRef.current += 1;
		if (anchors.length === 0) {
			setPlacements(new Map());
			post({ type: "pins", pins: [] });
			return;
		}
		post({ type: "locate", requestId: String(requestRef.current), anchors });
	}, [post]);
	locateNowRef.current = locateNow;
	const signature = placementSignature(threads);
	// biome-ignore lint/correctness/useExhaustiveDependencies: readyCount and signature are the triggers; threads are read through a ref
	useEffect(() => {
		if (readyCount === 0) return;
		locateNow();
	}, [readyCount, signature, locateNow]);

	// A located that left a thread unanswered (or a locate that never
	// reached the page, e.g. posted while a reload was in flight) must not
	// leave it without a pin: ask again, a few times, with backoff.
	const retriesRef = useRef(0);
	// biome-ignore lint/correctness/useExhaustiveDependencies: readyCount and signature reset the budget
	useEffect(() => {
		retriesRef.current = 0;
	}, [readyCount, signature]);
	useEffect(() => {
		if (readyCount === 0 || !ready) return;
		if (unplaced(threads, placements).length === 0) return;
		if (retriesRef.current >= LOCATE_RETRIES) return;
		const id = window.setTimeout(
			() => {
				retriesRef.current += 1;
				locateNowRef.current();
			},
			LOCATE_RETRY_MS * 2 ** retriesRef.current,
		);
		return () => window.clearTimeout(id);
	}, [threads, placements, readyCount, ready]);

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

	// The api.ts write wrappers emit skein://review-changed themselves.
	const act = async (fn: () => Promise<unknown>) => {
		setBusy(true);
		setCommentError(null);
		try {
			await fn();
			await fetchThreads();
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

	/** The page answered a locate. */
	const onLocatedRef = useRef<(b: Located) => void>(() => {});
	onLocatedRef.current = (b) => {
		if (b.requestId !== String(requestRef.current)) return;
		const current = threadsRef.current;
		const next = placeThreads(current, b.results);
		setPlacements(next);
		const load = readyCountRef.current;
		const writes = seenWrites(current, next, b.files, writtenRef.current, load);
		if (writes.length === 0) return;
		for (const w of writes) writtenRef.current.set(w.threadId, { ...w.seen, load });
		void Promise.all(writes.map((w) => reportElementSeen(roomId, w.threadId, w.seen)))
			.catch((e) => setCommentError(String(e)))
			.then(() => fetchThreads());
	};

	return {
		draft,
		setDraft,
		threads,
		placements,
		selected,
		setSelected,
		busy,
		commentError,
		act,
		submitDraft,
		// Stable, so a long-lived message listener can call them.
		relocate: useCallback(() => locateNowRef.current(), []),
		onLocated: useCallback((b: Located) => onLocatedRef.current(b), []),
		clearPlacements: useCallback(() => setPlacements(new Map()), []),
	};
};
