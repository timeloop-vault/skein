// useDesignControl — registers a mounted design pane with the agent seam
// (designControl.ts, #512) and implements `show_element` against the
// iframe. It never touches focus, the selection or the active room: it
// only asks the frame to scroll to and outline an element.

import { useCallback, useEffect, useRef } from "react";
import type { ElementThread } from "./designComments.ts";
import {
	type DesignScroll,
	type DesignSelection,
	registerDesignPane,
	type ShowElementRequest,
	type ShowElementResult,
	showResultFromPlacement,
} from "./designControl.ts";
import type { DesignDevice } from "./designDevice.ts";
import type { InvokedBeacon, InvokeElementRequest, InvokeElementResult } from "./designInvoke.ts";
import { invokeAnchorResult, invokeSelectorResult, placementAccepted } from "./designInvoke.ts";
import type { Beacon, HostMessage } from "./designPreview.ts";
import { makeWhenVisible } from "./designReveal.ts";
import { confirmHighlight, PendingRequests, selectorResult } from "./designShow.ts";
import { type ElementAnchor, type LocateResult, matchElement } from "./elementAnchor.ts";
import { useDesignChanges } from "./useDesignChanges.ts";

type Located = Extract<Beacon, { type: "located" }>;
type Shown = Extract<Beacon, { type: "shownElement" }>;

const RESHOW_ID = "agent-reshow";

export const useDesignControl = ({
	harnessId,
	roomId,
	entry,
	device,
	ready,
	loadFailed,
	errors,
	threads,
	selectedId,
	visible,
	laidOut,
	post,
}: {
	harnessId: string;
	roomId: string;
	entry: string | undefined;
	device: DesignDevice | undefined;
	ready: boolean;
	loadFailed: boolean;
	errors: unknown[];
	threads: readonly ElementThread[];
	selectedId: string | undefined;
	visible: boolean;
	/** True once the preview frame has a non-zero size (#549 reveal). */
	laidOut: () => boolean;
	post: (msg: HostMessage) => void;
}) => {
	const scrollRef = useRef<DesignScroll | null>(null);
	const lastShowRef = useRef<string | null>(null);
	const locates = useRef(new PendingRequests<LocateResult | null>("agent-loc-")).current;
	const shows = useRef(new PendingRequests<Shown>("agent-show-")).current;
	const invokes = useRef(new PendingRequests<InvokedBeacon>("agent-inv-")).current;

	// Latest props, read by the registered api without re-registering.
	const live = {
		entry,
		device,
		ready,
		loadFailed,
		errors,
		threads,
		selectedId,
		visible,
		laidOut,
		post,
	};
	const liveRef = useRef(live);
	liveRef.current = live;

	const showSelector = useCallback(
		(selector: string): Promise<Shown> =>
			shows.begin((requestId) =>
				liveRef.current.post({ type: "showElement", requestId, selector }),
			),
		[shows],
	);

	/** Locate an anchor in the frame and match it: the verdict show_element and
	 *  invoke_element both act on. */
	const placeAnchor = useCallback(
		async (a: ElementAnchor): Promise<ShowElementResult> => {
			const found = await locates.begin((requestId) =>
				liveRef.current.post({
					type: "locate",
					requestId,
					anchors: [
						{
							id: "a",
							...(a.odId ? { odId: a.odId } : {}),
							selector: a.selector,
							tag: a.tag,
							text: a.text,
						},
					],
				}),
			);
			return showResultFromPlacement(
				matchElement(a, found ?? { bySelector: null, byOdId: [], byText: [], sameTag: [] }),
			);
		},
		[locates],
	);

	const showElement = useCallback(
		async (req: ShowElementRequest): Promise<ShowElementResult> => {
			if (!liveRef.current.ready) throw new Error("not_ready: the design page has not loaded yet");
			if (req.anchor) {
				const placed = await placeAnchor(req.anchor);
				if (!placed.highlighted || !placed.element) return placed;
				const selector = placed.element.selector;
				const shown = await showSelector(selector).catch(() => null);
				const result = confirmHighlight(placed, shown);
				if (result.highlighted) lastShowRef.current = selector;
				return result;
			}
			const selector = req.selector ?? "";
			const shown = await showSelector(selector);
			const result = selectorResult(shown.count, shown.element, shown.invalid === true);
			if (result.highlighted) lastShowRef.current = selector;
			return result;
		},
		[placeAnchor, showSelector],
	);
	const showElementRef = useRef(showElement);
	showElementRef.current = showElement;

	const invokeElement = useCallback(
		async (req: InvokeElementRequest): Promise<InvokeElementResult> => {
			if (!liveRef.current.ready) throw new Error("not_ready: the design page has not loaded yet");
			const invoke = (selector: string): Promise<InvokedBeacon> =>
				invokes.begin((requestId) =>
					liveRef.current.post({
						type: "invoke",
						requestId,
						selector,
						action: req.action,
						...(req.direction ? { direction: req.direction } : {}),
						...(req.distance !== undefined ? { distance: req.distance } : {}),
						pointerType: liveRef.current.device?.touch === true ? "touch" : "mouse",
					}),
				);
			if (req.anchor) {
				const placed = await placeAnchor(req.anchor);
				if (!placementAccepted(placed) || !placed.element) {
					return invokeAnchorResult(placed, null, req.action);
				}
				const beacon = await invoke(placed.element.selector);
				return invokeAnchorResult(placed, beacon, req.action);
			}
			return invokeSelectorResult(await invoke(req.selector ?? ""), req.action);
		},
		[invokes, placeAnchor],
	);
	const invokeElementRef = useRef(invokeElement);
	invokeElementRef.current = invokeElement;

	const changes = useDesignChanges({ ready: () => liveRef.current.ready, post });
	const { onBeacon: changesOnBeacon, rejectAll: rejectChanges } = changes;
	const showChangesRef = useRef(changes.showChanges);
	showChangesRef.current = changes.showChanges;

	useEffect(() => {
		const dispose = registerDesignPane(harnessId, {
			roomId,
			ready: () => liveRef.current.ready,
			getState: () => {
				const l = liveRef.current;
				const t =
					l.selectedId === undefined ? undefined : l.threads.find((x) => x.id === l.selectedId);
				const selected: DesignSelection | null = t?.element
					? { threadId: t.id, anchor: t.element.anchor }
					: null;
				return {
					entry: l.entry ?? null,
					device: l.device ?? null,
					ready: l.ready,
					loadFailed: l.loadFailed,
					errors: l.errors,
					selected,
					scroll: scrollRef.current,
				};
			},
			whenVisible: makeWhenVisible(() => liveRef.current.visible && liveRef.current.laidOut()),
			showElement: (req) => showElementRef.current(req),
			invokeElement: (req) => invokeElementRef.current(req),
			showChanges: (req) => showChangesRef.current(req),
		});
		return () => {
			dispose();
			rejectChanges("the design pane closed");
			locates.rejectAll("the design pane closed");
			shows.rejectAll("the design pane closed");
			invokes.rejectAll("the design pane closed");
		};
	}, [harnessId, roomId, locates, shows, invokes, rejectChanges]);

	// A hidden iframe has no layout, so a show while hidden cannot scroll:
	// repeat the last successful one when the pane is seen again.
	useEffect(() => {
		if (!visible || !ready || lastShowRef.current === null) return;
		// Only while the outline is still pending: a dismissal clears it.
		post({ type: "showElement", requestId: RESHOW_ID, selector: lastShowRef.current });
	}, [visible, ready, post]);

	/** True when the beacon was an answer to one of our requests. */
	const onBeacon = useCallback(
		(b: Beacon): boolean => {
			if (b.type === "scroll") {
				scrollRef.current = { x: b.x, y: b.y };
				return true;
			}
			if (b.type === "shownCleared") {
				lastShowRef.current = null;
				return true;
			}
			if (b.type === "shownElement") {
				shows.settle(b.requestId, b as Shown);
				return true;
			}
			if (changesOnBeacon(b)) return true;
			if (b.type === "invoked") {
				invokes.settle(b.requestId, b as InvokedBeacon);
				return true;
			}
			if (b.type === "located" && b.requestId.startsWith("agent-loc-")) {
				locates.settle(b.requestId, (b as Located).results[0]?.found ?? null);
				return true;
			}
			return false;
		},
		[locates, shows, invokes, changesOnBeacon],
	);

	/** A new page load: forget its scroll and what was shown in the old one. */
	const reset = useCallback(() => {
		scrollRef.current = null;
		lastShowRef.current = null;
		locates.rejectAll("the page reloaded");
		shows.rejectAll("the page reloaded");
		invokes.rejectAll("the page reloaded");
		rejectChanges("the page reloaded");
	}, [locates, shows, invokes, rejectChanges]);

	return { onBeacon, reset };
};
