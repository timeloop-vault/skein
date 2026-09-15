// Pure pointer-based drag-to-reorder state machine for room and harness
// tabs (#271). HTML5 DnD is retired so `app-src-tauri/tauri.conf.json`
// can turn on the webview's native `dragDropEnabled` handler for #41's
// file drop — the two drag systems can't coexist (native on means
// `draggable`/dragstart/dragover/drop stop firing in the webview).
//
// No React, no DOM here on purpose: this module only knows about
// coordinates and drag identity. `useTabDrag.ts` owns pointer capture,
// `elementFromPoint` hit-testing, and turning the result back into
// React state for `.dragging` / `.drop-before` / `.drop-after` styling.

export type TabDragInfo =
	| { kind: "room"; id: string }
	| { kind: "harness"; roomId: string; id: string };

export type DropSide = "before" | "after";

/** What the DOM layer found under the pointer, translated into terms
 *  this module understands. `null` = nothing draggable there (empty
 *  space, the `+ harness` button, a pane, …) — though as of #271 the
 *  DOM layer resolves most of that "nothing" into an end-zone hit
 *  instead (see useTabDrag's `hitTest`), so a true `null` is now
 *  mostly panes and other chrome outside either tab strip. */
export type TabDragHit =
	| { kind: "room"; id: string; side: DropSide }
	| { kind: "harness"; roomId: string; id: string; side: DropSide };

export interface TabDragTarget {
	id: string;
	side: DropSide;
}

export type TabDragState =
	| { status: "idle" }
	| {
			status: "pressed";
			info: TabDragInfo;
			pointerId: number;
			startX: number;
			startY: number;
			/** Same-kind sibling ids (same room, for a harness drag), in
			 *  strip order, captured once at press. Drives the gap math
			 *  below — it does not change mid-drag since a reorder only
			 *  happens on release. */
			order: string[];
	  }
	| {
			status: "dragging";
			info: TabDragInfo;
			pointerId: number;
			order: string[];
			target: TabDragTarget | null;
			/** True when the pointer is over a same-kind tab that can't be
			 *  a valid target — today that's only a harness tab belonging
			 *  to a different room. Drives the not-allowed cursor. */
			refused: boolean;
	  };

export const idleState: TabDragState = { status: "idle" };

/** Below this many pixels of pointer movement (Chebyshev distance —
 *  the larger of dx/dy), a press is still just a click. A press that
 *  never crosses it produces no reorder on release, and the DOM layer
 *  must not suppress that click. */
export const DRAG_THRESHOLD_PX = 4;

export function press(
	info: TabDragInfo,
	pointerId: number,
	x: number,
	y: number,
	order: string[],
): TabDragState {
	return { status: "pressed", info, pointerId, startX: x, startY: y, order };
}

export function cancel(): TabDragState {
	return idleState;
}

/** Translate a "gap" — the slot a drop would land in, 0..order.length —
 *  into the (targetId, side) pair `reorderRoom`/`reorderHarness` (both
 *  in App.tsx) already expect. Gap `g < order.length` sits *before* the
 *  tab currently at index `g`; `g === order.length` has no tab to sit
 *  before, so it sits *after* the last one. Exported so useTabDrag can
 *  build an end-zone hit (blank strip space, the `+ harness`/`+ room`
 *  button) the same way, and so tests can check a gap lands exactly
 *  where the indicator says it will. */
export function gapToTarget(order: string[], gap: number): TabDragTarget | null {
	if (gap < 0 || gap > order.length || order.length === 0) return null;
	if (gap < order.length) {
		const id = order[gap];
		return id === undefined ? null : { id, side: "before" };
	}
	const id = order[order.length - 1];
	return id === undefined ? null : { id, side: "after" };
}

/** The gap a hit on tab `hit.id` corresponds to: its index in `order`,
 *  plus one if the pointer is over its right half. `null` if the hit's
 *  tab isn't in `order` at all (shouldn't happen for a same-room hit,
 *  but resolveHit's cross-room case never gets here anyway). */
function gapForHit(order: string[], hit: { id: string; side: DropSide }): number | null {
	const idx = order.indexOf(hit.id);
	if (idx < 0) return null;
	return hit.side === "before" ? idx : idx + 1;
}

/** A gap is a no-op iff it's the source's own current position — either
 *  of the two gaps that bound it (its left edge and its right edge).
 *  This is what makes the leftmost tab's own right half, or its right
 *  neighbour's left half, both correctly resolve to "nowhere to go"
 *  instead of lighting up a drop that doesn't move anything (#271). */
function isNoopGap(sourceIndex: number, gap: number): boolean {
	return gap === sourceIndex || gap === sourceIndex + 1;
}

/** Resolve a same-kind, same-room hit into a gap-checked target: `null`
 *  if the gap it represents is the source's current position (no
 *  reorder would happen), the hit's own (id, side) otherwise — kept as
 *  the ORIGINAL hit rather than re-derived from the gap, so the
 *  indicator paints on the tab the pointer is actually over rather
 *  than an equivalent neighbour (gap g can be described as "after tab
 *  g-1" or "before tab g"; only one of those is where the cursor is). */
function resolveGap(
	sourceId: string,
	order: string[],
	hit: { id: string; side: DropSide },
): { target: TabDragTarget | null; refused: boolean } {
	const s = order.indexOf(sourceId);
	const gap = gapForHit(order, hit);
	if (s < 0 || gap === null || isNoopGap(s, gap)) {
		return { target: null, refused: false };
	}
	return { target: { id: hit.id, side: hit.side }, refused: false };
}

/** Resolve a hit against the dragged item's validity rules: a room drag
 *  only targets room tabs; a harness drag only targets harness tabs in
 *  its own room. Cross-kind hits (room drag over a harness tab, or vice
 *  versa) can't happen validly since the two tab strips never overlap,
 *  and are ignored rather than refused. A same-kind hit in the wrong
 *  room is refused. A same-kind, same-room hit that resolves to the
 *  source's own current position is dropped to `null` rather than
 *  refused — it's not invalid, there's just nothing to indicate (#271). */
function resolveHit(
	info: TabDragInfo,
	order: string[],
	hit: TabDragHit,
): { target: TabDragTarget | null; refused: boolean } {
	if (info.kind !== hit.kind) return { target: null, refused: false };
	if (info.kind === "room") {
		return hit.kind === "room" ? resolveGap(info.id, order, hit) : { target: null, refused: false };
	}
	if (hit.kind !== "harness") return { target: null, refused: false };
	if (hit.roomId !== info.roomId) return { target: null, refused: true };
	return resolveGap(info.id, order, hit);
}

function applyHit(
	info: TabDragInfo,
	pointerId: number,
	order: string[],
	hit: TabDragHit | null,
): TabDragState {
	if (hit === null)
		return { status: "dragging", info, pointerId, order, target: null, refused: false };
	const { target, refused } = resolveHit(info, order, hit);
	return { status: "dragging", info, pointerId, order, target, refused };
}

/** Advance the machine on a pointermove. `hit` is the DOM layer's
 *  best guess at what tab is under (clientX, clientY) right now —
 *  computed via `elementFromPoint`, since pointer capture retargets
 *  the move event itself to the source tab. A stale event for a
 *  pointer that isn't the one being tracked is ignored outright, the
 *  same way a second finger's touches would be. */
export function move(
	state: TabDragState,
	pointerId: number,
	x: number,
	y: number,
	hit: TabDragHit | null,
): TabDragState {
	if (state.status === "idle" || state.pointerId !== pointerId) return state;
	if (state.status === "pressed") {
		const dx = x - state.startX;
		const dy = y - state.startY;
		if (Math.max(Math.abs(dx), Math.abs(dy)) < DRAG_THRESHOLD_PX) return state;
		// Threshold just crossed: promote to dragging and resolve this
		// point immediately so the indicator doesn't wait for a second move.
		return applyHit(state.info, pointerId, state.order, hit);
	}
	return applyHit(state.info, pointerId, state.order, hit);
}

/** Finish a drag on pointerup. Idle/pressed (never crossed the
 *  threshold) and a refused/empty target all resolve to idle with no
 *  reorder. `state.target` is already gap-checked by `move`/`applyHit`,
 *  so it can never equal the source's own id here — the explicit check
 *  stays anyway as a cheap belt-and-braces against a future caller
 *  that constructs a `dragging` state some other way. */
export function release(state: TabDragState): {
	state: TabDragState;
	reorder?: { info: TabDragInfo; targetId: string; side: DropSide };
} {
	if (state.status !== "dragging" || state.target === null || state.refused) {
		return { state: idleState };
	}
	if (state.target.id === state.info.id) return { state: idleState };
	return {
		state: idleState,
		reorder: { info: state.info, targetId: state.target.id, side: state.target.side },
	};
}

/** Which side of a tab (by its bounding-rect left edge and width) a
 *  pointer x-coordinate falls on. Shared by the DOM layer's hit-test
 *  so there's one definition of "before" vs "after". */
export function sideFor(x: number, rectLeft: number, rectWidth: number): DropSide {
	return x < rectLeft + rectWidth / 2 ? "before" : "after";
}

/** The gap `x` falls in, given every same-kind tab's rect in strip
 *  order — one rule for every not-directly-on-a-tab case (blank strip
 *  space past the last tab, the `+ harness`/`+ room` button, and the
 *  1-2px sub-pixel seam between two adjacent tabs), instead of
 *  `hitOnStripEnd`'s old "compare against the first tab's left edge or
 *  the last tab's right edge, else whichever end is nearer" — that
 *  fallback made the drop bar jump to a strip's far end while the
 *  pointer was still crossing between two tabs in the middle (#271).
 *
 *  `gap` is the count of tabs whose midpoint sits at or left of `x`:
 *  left of the first tab's midpoint → 0, right of the last → n,
 *  between tab i-1 and i's midpoints → i. Tie rule at an exact
 *  midpoint match: counted as "at or left of x", i.e. that tab
 *  contributes to the gap — the same call `sideFor` makes, which
 *  treats `x < mid` (strict) as "before" and so `x === mid` as
 *  "after". That keeps this helper interchangeable with a tab hit's
 *  own `sideFor` + index math: a hit on tab `i` at exactly its
 *  midpoint resolves to `side: "after"` → gap `i + 1`, and
 *  `gapForX(mid_i, rects)` returns that same `i + 1`. */
export function gapForX(x: number, rects: { left: number; width: number }[]): number {
	let gap = 0;
	for (const rect of rects) {
		if (rect.left + rect.width / 2 <= x) gap++;
	}
	return gap;
}
