// Persisted pop-out state (#493), localStorage so both windows (same
// origin) share it. Who writes what:
//   - main writes `open: true` when it creates the pop-out;
//   - the POP-OUT window writes `open: false` on a user close / dock-back
//     (never main: the main window's own destroy also closes the pop-out and
//     must leave `open` true so the next launch restores it), and writes
//     `geometry` and `alwaysOnTop` as the user changes them;
//   - main only reads `alwaysOnTop` and `geometry` when creating the window.

const KEY = "skein:ccPopout";

export interface CcPopoutGeometry {
	x: number;
	y: number;
	width: number;
	height: number;
}

export interface CcPopoutPrefs {
	open: boolean;
	alwaysOnTop: boolean;
	geometry?: CcPopoutGeometry;
}

export const DEFAULT_CC_POPOUT: CcPopoutPrefs = { open: false, alwaysOnTop: false };

const isNum = (n: unknown): n is number => typeof n === "number" && Number.isFinite(n);

function parseGeometry(g: unknown): CcPopoutGeometry | undefined {
	if (typeof g !== "object" || g === null) return undefined;
	const { x, y, width, height } = g as Record<string, unknown>;
	if (!isNum(x) || !isNum(y) || !isNum(width) || !isNum(height)) return undefined;
	if (width < 100 || height < 100) return undefined;
	return { x, y, width, height };
}

/** Tolerant parse: anything malformed degrades to the defaults. */
export function parseCcPopout(raw: string | null): CcPopoutPrefs {
	if (raw === null) return { ...DEFAULT_CC_POPOUT };
	try {
		const p: unknown = JSON.parse(raw);
		if (typeof p !== "object" || p === null) return { ...DEFAULT_CC_POPOUT };
		const o = p as Record<string, unknown>;
		const geometry = parseGeometry(o.geometry);
		return {
			open: o.open === true,
			alwaysOnTop: o.alwaysOnTop === true,
			...(geometry ? { geometry } : {}),
		};
	} catch {
		return { ...DEFAULT_CC_POPOUT };
	}
}

export function loadCcPopout(): CcPopoutPrefs {
	try {
		return parseCcPopout(localStorage.getItem(KEY));
	} catch {
		return { ...DEFAULT_CC_POPOUT };
	}
}

/** Merge `patch` over what is stored now (re-read, so two windows never
 *  clobber each other's fields) and persist. Returns the merged value. */
export function saveCcPopout(patch: Partial<CcPopoutPrefs>): CcPopoutPrefs {
	const next = { ...loadCcPopout(), ...patch };
	try {
		localStorage.setItem(KEY, JSON.stringify(next));
	} catch {
		// Storage unavailable: the pop-out just won't be remembered.
	}
	return next;
}
