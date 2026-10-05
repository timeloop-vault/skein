// Pure logic behind ContextMenu.tsx: where the menu sits and which item the
// arrow keys land on. No React, no DOM, so both are table-testable.

export interface Point {
	x: number;
	y: number;
}

export interface Size {
	width: number;
	height: number;
}

/** Keep the gap between a menu and the viewport edge. */
export const MENU_EDGE_MARGIN = 4;

/** The top-left for a menu opened at `at`: the cursor, unless the menu would
 *  spill past the right/bottom edge, in which case it is pushed back inside
 *  (never past the left/top edge, even when the menu is larger than the view). */
export function clampMenuPosition(at: Point, menu: Size, viewport: Size): Point {
	const maxX = viewport.width - menu.width - MENU_EDGE_MARGIN;
	const maxY = viewport.height - menu.height - MENU_EDGE_MARGIN;
	return {
		x: Math.max(MENU_EDGE_MARGIN, Math.min(at.x, maxX)),
		y: Math.max(MENU_EDGE_MARGIN, Math.min(at.y, maxY)),
	};
}

/** The next focused item for a key, wrapping at both ends. `current` is -1
 *  when nothing is focused yet. Null for a key the menu does not move on. */
export function nextMenuIndex(key: string, current: number, count: number): number | null {
	if (count <= 0) return null;
	switch (key) {
		case "ArrowDown":
			return current < 0 ? 0 : (current + 1) % count;
		case "ArrowUp":
			return current < 0 ? count - 1 : (current - 1 + count) % count;
		case "Home":
			return 0;
		case "End":
			return count - 1;
		default:
			return null;
	}
}
