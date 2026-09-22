// DOM wiring for #271's pointer-based tab drag-to-reorder. The state
// machine itself lives in tabDrag.ts (pure, unit-tested); this hook
// owns pointer capture, `elementFromPoint` hit-testing, the drag
// ghost, the cursor for a refused cross-room harness drop, and
// swallowing the click that follows a real drag. Kept out of App.tsx
// to keep that file's growth minimal (#19).
//
// Pointer capture retargets pointermove/up/cancel to the element that
// called `setPointerCapture` — the tab the drag started on — no matter
// where the pointer visually is. That means the event's own `target`
// is useless for hit-testing "what tab is under the cursor now"; we
// use `document.elementFromPoint(clientX, clientY)` instead, walking
// up to the nearest element carrying the `data-drag-*` attributes
// RoomTab/HarnessTab render on their tab root — or, failing that, the
// `data-drag-strip` attribute App.tsx puts on the two strip containers,
// so hovering blank strip space (or the `+ harness`/`+ room` button)
// still resolves to an end-of-strip drop instead of nothing (#271).

import {
	type PointerEvent as ReactPointerEvent,
	useCallback,
	useEffect,
	useRef,
	useState,
} from "react";
import {
	type DropSide,
	type TabDragHit,
	type TabDragInfo,
	type TabDragState,
	cancel as cancelDrag,
	gapForX,
	gapToTarget,
	idleState,
	move as moveDrag,
	press as pressDrag,
	release as releaseDrag,
	sideFor,
} from "./tabDrag.ts";

export type { DropSide };

export type TabDropTarget =
	| { kind: "room"; id: string; side: DropSide }
	| { kind: "harness"; roomId: string; id: string; side: DropSide }
	| null;

export const DRAG_KIND_ATTR = "data-drag-kind";
export const DRAG_ID_ATTR = "data-drag-id";
export const DRAG_ROOM_ATTR = "data-drag-room";
export const DRAG_STRIP_ATTR = "data-drag-strip";
// #76: room-tab-only, read by hitOnTab/hitOnStripEnd so tabDrag.ts's
// resolveHit can refuse a group-invalid target without RoomStrip having
// to pass business logic down into this generic DOM layer — see
// roomGroups.ts's `resolveRoomDrop` for what these mean.
export const DRAG_SEG_ATTR = "data-drag-seg";
export const DRAG_ROLE_ATTR = "data-drag-role";

/** Same-kind sibling tab elements, in strip (= visual) order: every
 *  room tab, or every harness tab belonging to `roomId`. Backs both
 *  the `order` press() needs for its gap math and the end-zone
 *  hit-test below — both need the same list of "what's in this strip
 *  right now, left to right". */
function stripTabElements(kind: "room" | "harness", roomId?: string): HTMLElement[] {
	const selector =
		kind === "room"
			? `[${DRAG_KIND_ATTR}="room"]`
			: `[${DRAG_KIND_ATTR}="harness"][${DRAG_ROOM_ATTR}="${roomId ? CSS.escape(roomId) : ""}"]`;
	return Array.from(document.querySelectorAll<HTMLElement>(selector));
}

function stripOrder(kind: "room" | "harness", roomId?: string): string[] {
	return stripTabElements(kind, roomId)
		.map((el) => el.getAttribute(DRAG_ID_ATTR))
		.filter((id): id is string => id !== null);
}

/** A hit on a specific tab, or `null` if `clientX, clientY` isn't over
 *  a tab at all. */
function hitOnTab(clientX: number, clientY: number): TabDragHit | null {
	const el = document.elementFromPoint(clientX, clientY);
	const target = el instanceof Element ? el.closest<HTMLElement>(`[${DRAG_KIND_ATTR}]`) : null;
	if (!target) return null;
	const kind = target.getAttribute(DRAG_KIND_ATTR);
	const id = target.getAttribute(DRAG_ID_ATTR);
	if (!id) return null;
	const rect = target.getBoundingClientRect();
	const side = sideFor(clientX, rect.left, rect.width);
	if (kind === "room") {
		const segId = target.getAttribute(DRAG_SEG_ATTR) ?? id;
		const role: "segment" | "member" =
			target.getAttribute(DRAG_ROLE_ATTR) === "member" ? "member" : "segment";
		return { kind: "room", id, side, segId, role };
	}
	if (kind === "harness") {
		const roomId = target.getAttribute(DRAG_ROOM_ATTR);
		if (!roomId) return null;
		return { kind: "harness", roomId, id, side };
	}
	return null;
}

/** A hit on the strip's empty area — blank space past the last tab,
 *  the `+ harness`/`+ room` button, the harness meta text, or the
 *  1-2px sub-pixel seam between two tabs — resolved via `gapForX`
 *  (#271): every tab's midpoint against `clientX` gives one gap for
 *  the whole strip, so a pointer crossing between two tabs in the
 *  middle lands on the gap between THEM, not on whichever end of the
 *  strip happens to be nearer. */
function hitOnStripEnd(clientX: number, clientY: number): TabDragHit | null {
	const el = document.elementFromPoint(clientX, clientY);
	const strip = el instanceof Element ? el.closest<HTMLElement>(`[${DRAG_STRIP_ATTR}]`) : null;
	if (!strip) return null;
	const kind = strip.getAttribute(DRAG_STRIP_ATTR);
	if (kind !== "room" && kind !== "harness") return null;
	const roomId = strip.getAttribute(DRAG_ROOM_ATTR) ?? undefined;
	const tabs = stripTabElements(kind, roomId);
	if (tabs.length === 0) return null;
	const order = tabs
		.map((t) => t.getAttribute(DRAG_ID_ATTR))
		.filter((id): id is string => id !== null);
	const rects = tabs.map((t) => t.getBoundingClientRect());
	const gap = gapForX(clientX, rects);
	const target = gapToTarget(order, gap);
	if (!target) return null;
	if (kind === "room") {
		const tabEl = tabs.find((t) => t.getAttribute(DRAG_ID_ATTR) === target.id);
		const segId = tabEl?.getAttribute(DRAG_SEG_ATTR) ?? target.id;
		const role: "segment" | "member" =
			tabEl?.getAttribute(DRAG_ROLE_ATTR) === "member" ? "member" : "segment";
		return { kind: "room", id: target.id, side: target.side, segId, role };
	}
	return { kind: "harness", roomId: roomId ?? "", id: target.id, side: target.side };
}

function hitTest(clientX: number, clientY: number): TabDragHit | null {
	return hitOnTab(clientX, clientY) ?? hitOnStripEnd(clientX, clientY);
}

const sameTarget = (a: TabDropTarget, b: TabDropTarget): boolean => {
	if (a === null || b === null) return a === b;
	if (a.kind !== b.kind || a.id !== b.id || a.side !== b.side) return false;
	return a.kind === "harness" && b.kind === "harness" ? a.roomId === b.roomId : true;
};

const toDropTarget = (state: TabDragState): TabDropTarget => {
	if (state.status !== "dragging" || !state.target) return null;
	return state.info.kind === "room"
		? { kind: "room", id: state.target.id, side: state.target.side }
		: { kind: "harness", roomId: state.info.roomId, id: state.target.id, side: state.target.side };
};

// Forces the cursor everywhere during a drag — without it, hovering a
// button/link underneath the moving pointer flashes its own `cursor`
// (pointer capture doesn't stop hover styling). `refused` swaps it to
// not-allowed for a cross-room harness drop.
const DRAGGING_CLASS = "tab-dragging";
const REFUSED_CLASS = "tab-drag-refused";

// #271: a floating clone of the tab being dragged, so the user sees
// what they're holding instead of just an indicator bar elsewhere on
// screen. Positioned with `transform` on every pointermove — no React
// state per move, since that would re-render the whole app at pointer
// rate.
const GHOST_CLASS = "tab-drag-ghost";

interface GhostGrab {
	/** Pointer position at press, relative to the source tab's own
	 *  top-left — kept constant for the whole drag (the source tab
	 *  itself never moves; only its clone does). */
	dx: number;
	dy: number;
	width: number;
	height: number;
}

export function useTabDrag(
	reorderRoom: (fromId: string, targetId: string, side: DropSide) => void,
	reorderHarness: (roomId: string, fromId: string, targetId: string, side: DropSide) => void,
) {
	const stateRef = useRef<TabDragState>(idleState);
	const [drag, setDrag] = useState<TabDragInfo | null>(null);
	const [dropTarget, setDropTarget] = useState<TabDropTarget>(null);
	const [refused, setRefused] = useState(false);
	// One-shot: set when a press crosses the drag threshold, consumed by
	// the tab's onClick wrapper so the click that follows a real drag
	// doesn't also select/activate the tab. Cleared at the start of every
	// new press so an aborted drag can't swallow an unrelated later click.
	const draggedRef = useRef(false);
	// The source tab's own DOM node, captured at press — cloned into the
	// ghost once the drag threshold is crossed. Not the pointermove
	// event's target: pointer capture retargets that to this same node
	// regardless of where the cursor visually is, which is exactly why
	// it's safe to read here too.
	const sourceElRef = useRef<HTMLElement | null>(null);
	const grabRef = useRef<GhostGrab | null>(null);
	const ghostRef = useRef<HTMLElement | null>(null);

	const removeGhost = useCallback(() => {
		ghostRef.current?.remove();
		ghostRef.current = null;
		grabRef.current = null;
		sourceElRef.current = null;
	}, []);

	const createGhost = useCallback(() => {
		const src = sourceElRef.current;
		const grab = grabRef.current;
		if (!src || !grab || ghostRef.current) return;
		const clone = src.cloneNode(true);
		if (!(clone instanceof HTMLElement)) return;
		clone.classList.add(GHOST_CLASS);
		clone.style.width = `${grab.width}px`;
		clone.style.height = `${grab.height}px`;
		// The clone isn't a real tab — strip anything that would make it
		// look like one to hitTest (belt-and-braces: pointer-events:none
		// already keeps elementFromPoint from ever returning it) or that
		// gives it a clickable-looking close button it doesn't need. This
		// also has to cover every descendant, not just the root: nested
		// chips/dots (data-harness-id, data-kind, data-agent-key,
		// data-agent-value, data-status) and ids would otherwise duplicate
		// the real tab's, and the ghost lives in document.body where a
		// document-wide query (e.g. statusPopover.ts) could match it (#271).
		for (const el of [clone, ...clone.querySelectorAll("*")]) {
			el.removeAttribute("id");
			for (const attr of [...el.attributes]) {
				if (attr.name.startsWith("data-")) el.removeAttribute(attr.name);
			}
		}
		clone.setAttribute("aria-hidden", "true");
		for (const btn of clone.querySelectorAll(".sk-tab-close, .ht-x")) btn.remove();
		document.body.appendChild(clone);
		ghostRef.current = clone;
	}, []);

	const moveGhost = useCallback((clientX: number, clientY: number) => {
		const ghost = ghostRef.current;
		const grab = grabRef.current;
		if (!ghost || !grab) return;
		ghost.style.transform = `translate(${clientX - grab.dx}px, ${clientY - grab.dy}px)`;
	}, []);

	const applyVisual = useCallback((state: TabDragState) => {
		const nextDrag = state.status === "dragging" ? state.info : null;
		setDrag((prev) => (prev === nextDrag ? prev : nextDrag));
		const nextTarget = toDropTarget(state);
		setDropTarget((prev) => (sameTarget(prev, nextTarget) ? prev : nextTarget));
		const nextRefused = state.status === "dragging" && state.refused;
		setRefused((prev) => (prev === nextRefused ? prev : nextRefused));
		const html = document.documentElement;
		html.classList.toggle(DRAGGING_CLASS, state.status === "dragging");
		html.classList.toggle(REFUSED_CLASS, nextRefused);
	}, []);

	const endDrag = useCallback(() => {
		stateRef.current = cancelDrag();
		applyVisual(stateRef.current);
		removeGhost();
	}, [applyVisual, removeGhost]);

	const startDrag = useCallback((e: ReactPointerEvent<HTMLDivElement>, info: TabDragInfo) => {
		if (e.button !== 0) return;
		const closeHit = e.target instanceof Element && e.target.closest(".sk-tab-close, .ht-x");
		if (closeHit) return;
		draggedRef.current = false;
		e.currentTarget.setPointerCapture(e.pointerId);
		const order = info.kind === "room" ? stripOrder("room") : stripOrder("harness", info.roomId);
		stateRef.current = pressDrag(info, e.pointerId, e.clientX, e.clientY, order);
		const rect = e.currentTarget.getBoundingClientRect();
		sourceElRef.current = e.currentTarget;
		grabRef.current = {
			dx: e.clientX - rect.left,
			dy: e.clientY - rect.top,
			width: rect.width,
			height: rect.height,
		};
	}, []);

	const onPointerMove = useCallback(
		(e: ReactPointerEvent<HTMLDivElement>) => {
			const state = stateRef.current;
			if (state.status === "idle" || state.pointerId !== e.pointerId) return;
			const wasDragging = state.status === "dragging";
			const hit = hitTest(e.clientX, e.clientY);
			const next = moveDrag(state, e.pointerId, e.clientX, e.clientY, hit);
			stateRef.current = next;
			if (!wasDragging && next.status === "dragging") {
				draggedRef.current = true;
				// #271: WebKit (Tauri's macOS WKWebView) can have already
				// started a text selection by the time the press crosses the
				// drag threshold — the CSS `user-select: none` added at drag
				// start only stops NEW selection, not one already underway.
				// Drop it here, once, not on every plain click.
				window.getSelection()?.removeAllRanges();
				createGhost();
			}
			if (next.status === "dragging") moveGhost(e.clientX, e.clientY);
			applyVisual(next);
		},
		[applyVisual, createGhost, moveGhost],
	);

	const onPointerUp = useCallback(
		(e: ReactPointerEvent<HTMLDivElement>) => {
			const state = stateRef.current;
			if (state.status === "idle" || state.pointerId !== e.pointerId) return;
			const { state: next, reorder } = releaseDrag(state);
			stateRef.current = next;
			applyVisual(next);
			removeGhost();
			if (!reorder) return;
			if (reorder.info.kind === "room") {
				reorderRoom(reorder.info.id, reorder.targetId, reorder.side);
			} else {
				reorderHarness(reorder.info.roomId, reorder.info.id, reorder.targetId, reorder.side);
			}
		},
		[applyVisual, removeGhost, reorderRoom, reorderHarness],
	);

	const onPointerCancel = useCallback(
		(e: ReactPointerEvent<HTMLDivElement>) => {
			const state = stateRef.current;
			if (state.status === "idle" || state.pointerId !== e.pointerId) return;
			endDrag();
		},
		[endDrag],
	);

	// Escape and losing window focus abort a live drag — otherwise a
	// dialog stealing focus or the user hitting Escape would leave
	// `.dragging` / the forced cursor stuck on until another pointerdown.
	useEffect(() => {
		const onKey = (e: KeyboardEvent) => {
			if (e.key !== "Escape" || stateRef.current.status !== "dragging") return;
			endDrag();
		};
		const onBlur = () => {
			if (stateRef.current.status === "idle") return;
			endDrag();
		};
		window.addEventListener("keydown", onKey);
		window.addEventListener("blur", onBlur);
		return () => {
			window.removeEventListener("keydown", onKey);
			window.removeEventListener("blur", onBlur);
		};
	}, [endDrag]);

	// Belt-and-braces: never leave the forced cursor, or a ghost node
	// appended to <body>, stuck around past unmount.
	useEffect(
		() => () => {
			document.documentElement.classList.remove(DRAGGING_CLASS, REFUSED_CLASS);
			removeGhost();
		},
		[removeGhost],
	);

	// One-shot: true exactly once per real drag, right after it ends —
	// callers check this in their tab's onClick before treating a click
	// as a select/activate.
	const suppressClick = useCallback(() => {
		if (!draggedRef.current) return false;
		draggedRef.current = false;
		return true;
	}, []);

	const dragHandlers = {
		onPointerMove,
		onPointerUp,
		onPointerCancel,
		onLostPointerCapture: onPointerCancel,
	};

	return { drag, dropTarget, refused, startDrag, dragHandlers, suppressClick };
}
