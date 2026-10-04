// Pure helpers for the pop-out window's own bookkeeping (#493): the local
// clock between snapshots, physical -> logical geometry, and a debounce.

import type { CcPopoutGeometry } from "./popoutPrefs.ts";

/** How far the local clock runs ahead of (or behind) the snapshot's `now`. */
export function clockOffset(snapshotNow: number, localNow: number): number {
	return snapshotNow - localNow;
}

/** The snapshot's notion of "now" at local time `localNow`. */
export function effectiveNow(offset: number, localNow: number): number {
	return localNow + offset;
}

/** Tauri reports window position/size in physical pixels; WebviewWindow
 *  creation options and popoutPrefs take LOGICAL ones. Pass the OUTER position
 *  and the INNER size: creation width/height are inner. */
export function toLogicalGeometry(
	pos: { x: number; y: number },
	size: { width: number; height: number },
	scaleFactor: number,
): CcPopoutGeometry {
	const s = scaleFactor > 0 ? scaleFactor : 1;
	return {
		x: Math.round(pos.x / s),
		y: Math.round(pos.y / s),
		width: Math.round(size.width / s),
		height: Math.round(size.height / s),
	};
}

/** Windows parks a minimized window at about (-32000, -32000). */
export function isSentinelPosition(pos: { x: number; y: number }): boolean {
	return pos.x <= -30000 || pos.y <= -30000;
}

export interface LogicalRect {
	x: number;
	y: number;
	width: number;
	height: number;
}

const MIN_VISIBLE_WIDTH = 100;
const MIN_VISIBLE_HEIGHT = 50;

/** True when the saved window rect shows at least 100x50 logical px inside one
 *  of the monitor work areas (all logical). A sentinel position never does. */
export function isRectVisible(
	rect: LogicalRect,
	monitors: readonly LogicalRect[],
	minWidth = MIN_VISIBLE_WIDTH,
	minHeight = MIN_VISIBLE_HEIGHT,
): boolean {
	if (isSentinelPosition(rect)) return false;
	return monitors.some((m) => {
		const w = Math.min(rect.x + rect.width, m.x + m.width) - Math.max(rect.x, m.x);
		const h = Math.min(rect.y + rect.height, m.y + m.height) - Math.max(rect.y, m.y);
		return w >= minWidth && h >= minHeight;
	});
}

export interface Debounced {
	(): void;
	cancel(): void;
}

export function debounce(fn: () => void, ms: number): Debounced {
	let t: ReturnType<typeof setTimeout> | undefined;
	const d = (() => {
		if (t !== undefined) clearTimeout(t);
		t = setTimeout(() => {
			t = undefined;
			fn();
		}, ms);
	}) as Debounced;
	d.cancel = () => {
		if (t !== undefined) clearTimeout(t);
		t = undefined;
	};
	return d;
}

export const PIN_TOOLTIP =
	"Keep on top. Shows over windowed and borderless-fullscreen apps, but not over exclusive-fullscreen games.";
