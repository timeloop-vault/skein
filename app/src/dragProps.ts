import type { PointerEvent as ReactPointerEvent } from "react";

// Drop indicator side relative to a tab. useTabDrag's hit-test picks
// "before" if the cursor is left of the tab's horizontal midpoint,
// "after" if right. CSS pseudo-elements render an accent-colored bar
// on the matching edge so the user can see where the drop will land.
// Issue #26; rebuilt on pointer events for #271 (see tabDrag.ts /
// useTabDrag.ts) so the webview's native file-drop handler (#41) can
// share the window — HTML5 DnD and `dragDropEnabled: true` can't
// coexist.
export type DropSide = "before" | "after" | null;

// Drag-related props that both tab kinds share. All optional so the
// presentational tab can render without drag wiring (e.g. in tests).
// `dragKind`/`dragId`/`dragRoomId` are hit-test data attributes read by
// useTabDrag's `elementFromPoint` walk — pointer capture retargets
// pointermove/up to the tab the drag started on, so the DOM event's own
// `target` can't say what's under the cursor now.
export interface DragProps {
	dragging?: boolean;
	dropSide?: DropSide;
	dragKind?: "room" | "harness";
	dragId?: string;
	dragRoomId?: string;
	/** #76: group-aware room drag — which strip segment this tab's drag
	 *  belongs to, and whether the tab IS that segment (a plain room, a
	 *  real or placeholder group lead) or only a non-lead MEMBER of one
	 *  (see roomGroups.ts's `resolveTopDrop`/`resolveRowDrop`). RoomTab-only; HarnessTab
	 *  never sets these. */
	dragSegId?: string;
	dragRole?: "segment" | "member";
	onPointerDown?: (e: ReactPointerEvent<HTMLDivElement>) => void;
	onPointerMove?: (e: ReactPointerEvent<HTMLDivElement>) => void;
	onPointerUp?: (e: ReactPointerEvent<HTMLDivElement>) => void;
	onPointerCancel?: (e: ReactPointerEvent<HTMLDivElement>) => void;
	onLostPointerCapture?: (e: ReactPointerEvent<HTMLDivElement>) => void;
	/** One-shot: true if the click following pointerup is the tail of a
	 *  real drag (threshold crossed) and should not select/activate the
	 *  tab. Consumed by the wrapper below, not passed through to onClick. */
	suppressClick?: () => boolean;
}
