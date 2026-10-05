// Appearance of the Control Center pop-out (#493). The tokens the view reads
// (--bg-*, --fg-*, --st-*, --cfs, --pad-x, --dot, --chip) are defined on the
// `.sk-<theme>` / `.density-<density>` classes AppShell puts on `.sk-app`;
// the pop-out never mounts App, so it re-derives them from the same
// localStorage prefs (written by usePersistedState in useAppSettings.ts).

const PREFIX = "skein:";
export const APPEARANCE_KEYS = [`${PREFIX}theme`, `${PREFIX}density`, `${PREFIX}chromeFontPt`];

export interface PopoutAppearance {
	theme: "dark" | "light";
	density: "compact" | "regular" | "comfy";
	chromeFontPt: number;
}

// Mirrors the defaults and CHROME_FONT_MIN/MAX in useAppSettings.ts.
export const DEFAULT_APPEARANCE: PopoutAppearance = {
	theme: "dark",
	density: "regular",
	chromeFontPt: 12,
};
const FONT_MIN = 10;
const FONT_MAX = 20;

function parseJson(raw: string | null): unknown {
	if (raw === null) return undefined;
	try {
		return JSON.parse(raw);
	} catch {
		return undefined;
	}
}

/** Tolerant read: anything missing or malformed falls back to the default. */
export function readAppearance(get: (key: string) => string | null): PopoutAppearance {
	const theme = parseJson(get(`${PREFIX}theme`));
	const density = parseJson(get(`${PREFIX}density`));
	const pt = parseJson(get(`${PREFIX}chromeFontPt`));
	return {
		theme: theme === "light" || theme === "dark" ? theme : DEFAULT_APPEARANCE.theme,
		density:
			density === "compact" || density === "regular" || density === "comfy"
				? density
				: DEFAULT_APPEARANCE.density,
		chromeFontPt:
			typeof pt === "number" && Number.isFinite(pt)
				? Math.min(FONT_MAX, Math.max(FONT_MIN, pt))
				: DEFAULT_APPEARANCE.chromeFontPt,
	};
}

/** A `storage` event key that can change the appearance (null = cleared). */
export function isAppearanceKey(key: string | null): boolean {
	return key === null || APPEARANCE_KEYS.includes(key);
}

export function appearanceClassName(a: PopoutAppearance): string {
	return `cc-popout-root sk-${a.theme} density-${a.density}`;
}
